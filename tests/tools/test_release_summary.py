"""通过实际命令入口检查发布摘要，不把上传意图当作成功证据。"""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[2] / "scripts/write_release_summary.py"


class ReleaseSummaryTests(unittest.TestCase):
    def summary(self, *, result="success", npm_result=None, should_publish="true", gate="success"):
        needs = {
            "version-gate": {"result": gate, "outputs": {
                "version": "0.9.0-beta.5", "pypi_should_publish": should_publish,
                "npm_should_publish": should_publish,
            }},
            "release": {"result": result},
            "release-npm": {"result": result if npm_result is None else npm_result},
        }
        with tempfile.TemporaryDirectory() as root:
            output = Path(root) / "summary.md"
            output.write_text("已有摘要\n", encoding="utf-8")
            subprocess.run([sys.executable, str(SCRIPT)], check=True, env={
                **os.environ, "RELEASE_NEEDS": json.dumps(needs),
                "GITHUB_STEP_SUMMARY": str(output),
            })
            return output.read_text(encoding="utf-8")

    def test_success_requires_successful_upload_and_uses_python_version(self):
        self.assertEqual(self.summary(),
            "已有摘要\n## 发布摘要 (Release Summary): 0.9.0-beta.5\n\n"
            "- **crates.io**: 由维护者在本地手动发布 (Requires manual publication by the maintainer)\n"
            "- **PyPI**: triviumdb==0.9.0b5 — 上传作业成功 (Upload job succeeded)\n"
            "- **NPM**: triviumdb@0.9.0-beta.5 — 上传作业成功 (Upload job succeeded)\n")

    def test_failed_cancelled_and_blocked_uploads_are_distinct(self):
        for result, expected in [
            ("failure", "上传失败 (Upload failed)"),
            ("cancelled", "上传已取消 (Upload cancelled)"),
            ("skipped", "未执行：上游门禁或构建未成功 (Not run: upstream gate or build did not succeed)"),
        ]:
            with self.subTest(result=result):
                summary = self.summary(result=result)
                self.assertEqual(summary.count(expected), 2)
                self.assertNotIn("上传作业成功", summary)
                self.assertNotIn("版本已存在", summary)

    def test_each_registry_uses_its_own_upload_result(self):
        summary = self.summary(result="failure", npm_result="success")
        self.assertIn("**PyPI**: triviumdb==0.9.0b5 — 上传失败 (Upload failed)", summary)
        self.assertIn("**NPM**: triviumdb@0.9.0-beta.5 — 上传作业成功 (Upload job succeeded)", summary)

    def test_existing_versions_are_only_reported_after_successful_gate(self):
        summary = self.summary(result="skipped", should_publish="false")
        self.assertEqual(summary.count("版本已存在，跳过上传 (Version already exists; upload skipped)"), 2)
        for gate in ["failure", "cancelled", "skipped"]:
            with self.subTest(gate=gate):
                summary = self.summary(result="skipped", should_publish="false", gate=gate)
                self.assertEqual(summary.count("未执行：版本门禁未成功 (Not run: version gate did not succeed)"), 2)
                self.assertNotIn("版本已存在", summary)
                self.assertNotIn("上传作业成功", summary)

    def test_inconsistent_states_never_report_success(self):
        for result, should_publish in [("success", "false"), ("unknown", "true")]:
            with self.subTest(result=result, should_publish=should_publish):
                summary = self.summary(result=result, should_publish=should_publish)
                self.assertEqual(summary.count("发布状态不一致，请检查作业 (Inconsistent release state; inspect jobs)"), 2)
                self.assertNotIn("上传作业成功", summary)


if __name__ == "__main__":
    unittest.main()
