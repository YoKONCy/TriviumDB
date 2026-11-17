"""外部脚本调用真实原生工具，验证检查、交付和协议拒绝。"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class MigrationEntry(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / "source"
        shutil.copytree(ROOT / "tests/fixtures/release_090/u64/mmap/wal", self.source)
        (self.source / "sample.tdb.lock").write_bytes(b"")
        self.before = {p.name: p.read_bytes() for p in self.source.iterdir()}
        self.target = self.root / "target"
        self.tool = Path(os.environ.get("TDB_MIGRATE_TOOL", ROOT / "target/migration/debug" / ("triviumdb-migrate.exe" if os.name == "nt" else "triviumdb-migrate")))
        self.assertTrue(self.tool.is_file(), "必须先构建实际迁移工具")

    def run_tool(self, *args, tool=None, source=None, destination=None):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/migrate.py"),
            "--tool", str(tool or self.tool), "--source", str(source or self.source / "sample.tdb"),
            "--destination", str(destination or self.target), "--memory-mib", "64", *args],
            capture_output=True)
        # 严格解码在请求线程完成，错误编码直接使验收失败。
        result.stdout = result.stdout.decode("utf-8")
        result.stderr = result.stderr.decode("utf-8")
        return result

    def assert_source(self):
        self.assertEqual({p.name: p.read_bytes() for p in self.source.iterdir()}, self.before)

    def test_default_check_preserves_source_and_leaves_no_target(self):
        result = self.run_tool()
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertTrue(report["checked"])
        self.assertFalse(report["committed"])
        self.assertEqual(report["facts"]["nodes"], 2)
        self.assertFalse(self.target.exists())
        self.assert_source()

    def test_execute_and_retry_publish_the_same_facts(self):
        first = self.run_tool("--execute")
        self.assertEqual(first.returncode, 0, first.stderr)
        first = json.loads(first.stdout)
        second = self.run_tool("--execute")
        self.assertEqual(second.returncode, 0, second.stderr)
        second = json.loads(second.stdout)
        self.assertTrue(first["committed"] and second["reused"])
        self.assertEqual(first["facts"], second["facts"])
        self.assertEqual(first["facts"]["next_id"], 10)
        self.assertFalse((self.target / "database.tdb.migration").exists())
        self.assert_source()

    def test_protocol_failure_stops_before_source_or_target_changes(self):
        result = self.run_tool("--execute", tool=Path(sys.executable))
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertFalse(self.target.exists())
        self.assert_source()

    def test_explicit_dtype_mismatch_preserves_structured_details_and_source(self):
        result = self.run_tool("--source-dtype", "f32")
        self.assertEqual(result.returncode, 1, result.stderr)
        error = json.loads(result.stderr)["error"]
        self.assertEqual(error["code"], "TDB_DTYPE_MISMATCH")
        self.assertEqual(error["details"], {"stored": "u64", "requested": "f32"})
        self.assertFalse(self.target.exists())
        self.assert_source()

    def test_historical_formats_check_execute_and_retry_through_script(self):
        for format_id, family in [(11, "pre_named_graph_format11"), (12, "pre_document_publication_format12")]:
            with self.subTest(format=format_id):
                source = self.root / f"source-{format_id}"
                target = self.root / f"target-{format_id}"
                shutil.copytree(ROOT / "tests/fixtures" / family / "u64", source)
                (source / "sample.tdb.lock").write_bytes(b"")
                before = {p.name: p.read_bytes() for p in source.iterdir()}
                checked = self.run_tool("--check", source=source / "sample.tdb", destination=target)
                self.assertEqual(checked.returncode, 0, checked.stderr)
                checked = json.loads(checked.stdout)
                self.assertFalse(checked["committed"])
                self.assertFalse(target.exists())
                first = self.run_tool("--execute", source=source / "sample.tdb", destination=target)
                self.assertEqual(first.returncode, 0, first.stderr)
                first = json.loads(first.stdout)
                self.assertEqual(first["source_format"], format_id)
                self.assertEqual(first["target_format"], 15)
                self.assertEqual(first["dtype"], "u64")
                self.assertEqual(first["facts"]["nodes"], 3)
                self.assertEqual(first["facts"]["next_id"], 34)
                self.assertEqual(first["facts"], checked["facts"])
                second = self.run_tool("--execute", source=source / "sample.tdb", destination=target)
                self.assertEqual(second.returncode, 0, second.stderr)
                second = json.loads(second.stdout)
                self.assertTrue(first["committed"] and second["reused"])
                self.assertEqual(first["facts"], second["facts"])
                self.assertFalse((target / "database.tdb.migration").exists())
                self.assertEqual({p.name: p.read_bytes() for p in source.iterdir()}, before)


if __name__ == "__main__":
    unittest.main()
