"""已关闭对象、原生句柄和跨语言 Hook 的生命周期回归。"""

from contextlib import contextmanager
import ctypes
import gc
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
import weakref

import triviumdb

CASE = json.loads((Path(__file__).resolve().parents[1] / "contracts/lifecycle_cases.json").read_text(encoding="utf-8"))


class Hook:
    pass


class ResourceLifecycleTests(unittest.TestCase):
    @contextmanager
    def assert_core_error(self, code):
        with self.assertRaises(triviumdb.TriviumDBError) as raised:
            yield
        self.assertEqual(raised.exception.code, code)
        if code == "TDB_DATABASE_CLOSED":
            self.assertEqual(raised.exception.details, {})
        else:
            self.assertEqual(set(raised.exception.details), {"reason"})
            self.assertIsInstance(raised.exception.details["reason"], str)
            self.assertTrue(raised.exception.details["reason"])

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="tdb Python 资源生命周期-"))
        self.held = []
        self.held_values = []
        self.sequence = 0

    def tearDown(self):
        for db in self.held:
            db.close()
        self.held.clear()
        self.held_values.clear()
        gc.collect()
        shutil.rmtree(self.root)

    def create(self, **options):
        directory = self.root / str(self.sequence)
        self.sequence += 1
        directory.mkdir()
        file = directory / "sample.tdb"
        db = triviumdb.TriviumDB(str(file), dim=CASE["dimension"], auto_build_quiver=False, **options)
        db.insert_with_id(CASE["node_id"], CASE["vector"], CASE["original_payload"])
        return db, file, directory

    def assert_released(self, directory):
        if sys.platform == "linux":
            mappings = Path("/proc/self/maps").read_text(encoding="utf-8", errors="replace")
            self.assertNotIn(str(directory.resolve()) + os.sep, mappings, "关闭后仍保留数据库文件映射")
        if sys.platform == "win32":
            from ctypes import wintypes
            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
            kernel.CreateFileW.restype = wintypes.HANDLE
            kernel.CloseHandle.argtypes = [wintypes.HANDLE]
            for file in directory.iterdir():
                if not file.is_file():
                    continue
                handle = kernel.CreateFileW(str(file), 0xC0000000, 0, None, 3, 0x80, None)
                self.assertNotEqual(handle, ctypes.c_void_p(-1).value, f"关闭后仍占用 {file}：{ctypes.get_last_error()}")
                kernel.CloseHandle(handle)
        moved = directory.with_suffix(".moved")
        os.rename(directory, moved)
        os.rename(moved, directory)

    def test_close_modes_release_files_with_live_objects(self):
        for dtype in CASE["dtypes"]:
            for mode in ["close", "with", "with_exception", "timeout", "shutdown"]:
                with self.subTest(dtype=dtype, mode=mode):
                    db, file, directory = self.create(dtype=dtype)
                    if mode == "close":
                        db.close()
                    elif mode.startswith("with"):
                        try:
                            with db:
                                if mode == "with_exception":
                                    raise ValueError("测试调用方异常")
                        except ValueError:
                            pass
                    elif mode == "timeout":
                        self.assertTrue(db.close_timeout(CASE["close_timeout_ms"]))
                    else:
                        db.request_shutdown()
                        deadline = time.monotonic() + CASE["close_timeout_ms"] / 1000
                        while db.shutdown_status()["phase"] != "closed":
                            self.assertLess(time.monotonic(), deadline, "关闭未在期限内完成")
                            time.sleep(0.001)
                    self.held.append(db)
                    for read in [
                        lambda: db.get(CASE["node_id"]), lambda: db.get_payload(CASE["node_id"]),
                        lambda: CASE["node_id"] in db, lambda: len(db), db.all_node_ids,
                        lambda: db.get_edges(CASE["node_id"]), lambda: db.get_edge(CASE["node_id"], 2, "关联"),
                        lambda: db.get_incoming_edges(CASE["node_id"]), lambda: db.neighbors(CASE["node_id"]),
                        db.graph_stats, db.validate_graph, db.list_unique_indexes, db.index_info, db.clear_search_state,
                    ]:
                        with self.assert_core_error("TDB_DATABASE_CLOSED"):
                            read()
                    self.assert_released(directory)
                    reader = triviumdb.TriviumDB(str(file), dim=CASE["dimension"], dtype=dtype, access_mode="read_only", auto_build_quiver=False)
                    self.assertEqual(reader.get_payload(CASE["node_id"]), CASE["original_payload"])
                    reader.close()
                    self.held.append(reader)
                    self.assert_released(directory)

    def test_manifest_survives_sync_and_async_close(self):
        for asynchronous in [False, True]:
            db, file, directory = self.create()
            db.publish_generation_manifest("关闭制品")
            before = {p.name: p.read_bytes() for p in directory.iterdir() if not p.name.endswith(".lock")}
            if asynchronous:
                self.assertTrue(db.close_timeout(CASE["close_timeout_ms"]))
            else:
                db.close()
            self.assertEqual(before, {name: (directory / name).read_bytes() for name in before})
            reader = triviumdb.TriviumDB(str(file), dim=CASE["dimension"], access_mode="immutable", auto_build_quiver=False)
            self.assertEqual(len(reader), 1)
            reader.close()
            self.held.extend([db, reader])
            self.assert_released(directory)

    def test_hook_clear_close_and_unreachable_cycle_release_references(self):
        for mode in ["clear", "replace", "close", "timeout", "cycle", "rejected"]:
            db, _, directory = self.create()
            hook = Hook()
            reference = weakref.ref(hook)
            if mode == "cycle":
                hook.db = db
            if mode == "rejected":
                db.close()
                with self.assert_core_error("TDB_DATABASE_CLOSED"):
                    db.set_hook(hook)
            else:
                db.set_hook(hook)
            if mode == "clear":
                db.clear_hook()
            elif mode == "replace":
                db.set_hook(Hook())
            elif mode == "close":
                db.close()
            elif mode == "timeout":
                self.assertTrue(db.close_timeout(CASE["close_timeout_ms"]))
            if mode != "cycle":
                self.held.append(db)
            del hook, db
            gc.collect()
            self.assertIsNone(reference(), f"{mode} 未释放 Hook")
            if mode in ["close", "timeout", "cycle"]:
                self.assert_released(directory)

    def test_close_releases_ffi_library(self):
        source = self.root / "hook.rs"
        source.write_text("pub fn marker() -> u32 { 1 }", encoding="utf-8")
        extension = "dll" if sys.platform == "win32" else "dylib" if sys.platform == "darwin" else "so"
        for asynchronous in [False, True]:
            library = self.root / f"hook-{asynchronous}.{extension}"
            subprocess.run(["rustc", "--crate-type=cdylib", str(source), "-o", str(library)], check=True)
            db, _, _ = self.create()
            db.load_ffi_hook(str(library))
            if asynchronous:
                self.assertTrue(db.close_timeout(CASE["close_timeout_ms"]))
            else:
                db.close()
            self.held.append(db)
            library.unlink()

    def test_retained_transactions_and_results_survive_directory_replacement(self):
        for dtype in CASE["dtypes"]:
            for asynchronous in [False, True]:
                with self.subTest(dtype=dtype, asynchronous=asynchronous):
                    runtime = triviumdb.ResourceRuntime(memory_limit_mb=64, maintenance_memory_limit_mb=8, max_threads=1)
                    db, file, directory = self.create(dtype=dtype, runtime=runtime, memory_limit_mb=8)
                    db.insert_with_id(CASE["later_node_id"], CASE["vector"], {"phase": "neighbor", "tenant": "agent-a"})
                    db.link(CASE["node_id"], CASE["later_node_id"], "原关系", 1.0)
                    db.create_index("tenant")
                    db.index_text(CASE["node_id"], "originaltoken")
                    prepared = db.prepare_tql('FIND {tenant: "agent-a"} AS n WITH n RETURN n')
                    node = db.get(CASE["node_id"])
                    rows = db.tql('FIND {tenant: "agent-a"} AS n WITH n RETURN n')
                    expected_rows = [
                        {"n": {"id": CASE["node_id"], "payload": CASE["original_payload"], "num_edges": 1}},
                        {"n": {"id": CASE["later_node_id"], "payload": {"phase": "neighbor", "tenant": "agent-a"}, "num_edges": 0}},
                    ]
                    self.assertEqual(sorted([row.row for row in rows], key=lambda row: row["n"]["id"]), expected_rows)
                    edges = db.get_edges(CASE["node_id"])
                    transaction = db.transaction()
                    transaction.insert_with_id(1001, CASE["vector"], {"phase": "pending"})
                    rolled_back = db.transaction()
                    rolled_back.insert_with_id(1002, CASE["vector"], {})
                    if asynchronous:
                        self.assertTrue(db.close_timeout(CASE["close_timeout_ms"]))
                    else:
                        db.close()
                    self.held.append(db)
                    self.held_values.extend([transaction, rolled_back, node, rows, prepared, edges])
                    self.assertEqual(runtime.stats()["attached_databases"], 0)
                    self.assert_released(directory)
                    old = directory.with_name(directory.name + ".old")
                    directory.rename(old)
                    directory.mkdir()
                    writer = triviumdb.TriviumDB(str(file), dim=CASE["dimension"], dtype=dtype,
                        runtime=runtime, memory_limit_mb=8, auto_build_quiver=False)
                    self.held.append(writer)
                    writer.insert_with_id(CASE["node_id"], CASE["vector"], {"phase": "replacement", "tenant": "agent-a"})
                    writer.flush()
                    db.close()
                    self.assertTrue(db.close_timeout(0))
                    transaction.insert_with_id(1003, CASE["vector"], {})
                    with self.assert_core_error("TDB_DATABASE_CLOSED"):
                        transaction.commit()
                    rolled_back.rollback()
                    self.assertEqual(runtime.stats()["attached_databases"], 1)
                    self.assertEqual(runtime.stats()["reserved_database_bytes"], 8 * 1024 * 1024)
                    with self.assert_core_error("TDB_DATABASE_LOCKED"):
                        triviumdb.TriviumDB(str(file), dim=CASE["dimension"], dtype=dtype)
                    self.assertEqual(writer.node_count(), 1)
                    for pending in [1001, 1002, 1003]:
                        self.assertFalse(pending in writer)
                    self.assertEqual(node.payload, CASE["original_payload"])
                    self.assertEqual(sorted([row.row for row in rows], key=lambda row: row["n"]["id"]), expected_rows)
                    self.assertEqual(edges[0].target_id, CASE["later_node_id"])
                    self.assertEqual([row.row for row in writer.execute_prepared_tql(prepared, {})], [
                        {"n": {"id": CASE["node_id"], "payload": {"phase": "replacement", "tenant": "agent-a"}, "num_edges": 0}},
                    ])
                    shutil.rmtree(old)
                    writer.close()
                    self.assertEqual(runtime.stats()["attached_databases"], 0)
                    self.assert_released(directory)

    def test_multiple_readers_release_only_their_own_resources(self):
        for dtype in CASE["dtypes"]:
            for access in ["read_only", "immutable"]:
                with self.subTest(dtype=dtype, access=access):
                    db, file, directory = self.create(dtype=dtype)
                    db.insert_with_id(CASE["later_node_id"], CASE["vector"], {"tenant": "agent-a", "state": "neighbor"})
                    db.link(CASE["node_id"], CASE["later_node_id"], "共享图边", 1.0)
                    db.create_index("tenant")
                    db.index_text(CASE["node_id"], "originaltoken")
                    db.publish_generation_manifest("持有读者制品")
                    db.close()
                    self.held.append(db)
                    options = dict(dim=CASE["dimension"], dtype=dtype, load_text_index=True, access_mode=access)
                    first = triviumdb.TriviumDB(str(file), **options)
                    second = triviumdb.TriviumDB(str(file), **options)
                    self.held.extend([first, second])
                    saved = second.get(CASE["node_id"])
                    self.held_values.append(saved)
                    first.close()
                    self.assertEqual(second.get_payload(CASE["node_id"]), CASE["original_payload"])
                    self.assertEqual(second.get_edges(CASE["node_id"])[0].target_id, CASE["later_node_id"])
                    actual_rows = [row.row for row in second.tql('FIND {tenant: "agent-a"} AS n WITH n RETURN n')]
                    self.assertEqual(sorted(actual_rows, key=lambda row: row["n"]["id"]), [
                        {"n": {"id": CASE["node_id"], "payload": CASE["original_payload"], "num_edges": 1}},
                        {"n": {"id": CASE["later_node_id"], "payload": {"tenant": "agent-a", "state": "neighbor"}, "num_edges": 0}},
                    ])
                    if access == "read_only":
                        with self.assert_core_error("TDB_DATABASE_LOCKED"):
                            triviumdb.TriviumDB(str(file), dim=CASE["dimension"], dtype=dtype)
                    self.assertTrue(second.close_timeout(CASE["close_timeout_ms"]))
                    self.assert_released(directory)
                    writer = triviumdb.TriviumDB(str(file), dim=CASE["dimension"], dtype=dtype)
                    self.held.append(writer)
                    first.close()
                    second.close()
                    with self.assert_core_error("TDB_DATABASE_LOCKED"):
                        triviumdb.TriviumDB(str(file), dim=CASE["dimension"], dtype=dtype)
                    writer.update_payload(CASE["node_id"], {"phase": "updated"})
                    writer.close()
                    self.assertEqual(saved.payload, CASE["original_payload"])
                    self.assert_released(directory)

    def test_failed_flush_and_close_keep_the_live_handle_until_retry(self):
        for dtype in CASE["dtypes"]:
            db, file, directory = self.create(dtype=dtype)
            db.create_index("tenant")
            db.index_text(CASE["node_id"], "originaltoken")
            db.index_keyword(CASE["node_id"], "originalalias")
            db.close()
            self.held.append(db)
            db = triviumdb.TriviumDB(str(file), dim=CASE["dimension"], dtype=dtype, auto_build_quiver=False)
            self.held.append(db)
            db.insert_with_id(CASE["later_node_id"], CASE["vector"], CASE["later_payload"])
            obstacle = Path(str(file) + ".text.meta.tmp")
            obstacle.mkdir()
            with self.assertRaises(RuntimeError):
                db.flush()
            with self.assertRaises(RuntimeError):
                db.close()
            self.assertNotEqual(db.shutdown_status()["phase"], "closed")
            with self.assert_core_error("TDB_DATABASE_LOCKED"):
                triviumdb.TriviumDB(str(file), dim=CASE["dimension"], dtype=dtype)
            obstacle.rmdir()
            self.assertTrue(db.close_timeout(CASE["close_timeout_ms"]))
            self.assert_released(directory)
            reader = triviumdb.TriviumDB(str(file), dim=CASE["dimension"], dtype=dtype,
                load_text_index=True, access_mode="read_only")
            self.held.append(reader)
            self.assertEqual(reader.node_count(), 2)
            self.assertEqual(reader.get_payload(CASE["later_node_id"]), CASE["later_payload"])
            self.assertEqual([row.row["hits"]["id"] for row in reader.tql(
                'TEXT AC "originalalias" TOP 10 AS hits WITH hits RETURN hits')], [CASE["node_id"]])
            self.assertEqual([row.row["hits"]["id"] for row in reader.tql(
                'TEXT BM25 "originaltoken" TOP 10 AS hits WITH hits RETURN hits')], [CASE["node_id"]])
            reader.close()
            self.assert_released(directory)


if __name__ == "__main__":
    unittest.main()
