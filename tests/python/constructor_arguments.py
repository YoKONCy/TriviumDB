"""构造参数值、打开失败配额归还与 Windows 文件租约的实际 wheel 契约。"""

import json
import inspect
from pathlib import Path
import struct

import triviumdb
from call_arguments import reject
from read_arguments import node


def invalid(action, field, exception=Exception, write=None):
    try:
        action()
    except exception as error:
        assert error.code == "TDB_INVALID_INPUT" and error.schema_version == 1
        assert error.target == {"field": field} and error.stage is None and error.write == write
        assert set(error.details) == {"reason"} and error.details["reason"]
    else:
        raise AssertionError(f"构造参数必须拒绝 {field}")


def run_constructor_arguments(root):
    fixture = json.loads((Path(__file__).parents[1] / "contracts/constructor_arguments.json").read_text(encoding="utf-8"))
    for name, expected in fixture["python_signatures"].items():
        assert str(inspect.signature(getattr(triviumdb, name))) == expected
    parent = Path(root) / "constructor-rejected"
    file = str(parent / "database.tdb")
    reject(lambda: triviumdb.TriviumDB(), "missing", "path")
    reject(lambda: triviumdb.TriviumDB(file, path=file), "multiple_values", "path")
    for constructor, args in ((triviumdb.TriviumDB, [file] + [None] * 15),
                              (triviumdb.ResourceRuntime, [None] * 5),
                              (triviumdb.ConsoleSession, [None] * 2)):
        reject(lambda: constructor(*args), "too_many_positional", "arguments")
        for field in ("unexpected", "词.行/[]\x00😀", "\ud800"):
            reject(lambda: constructor(**{field: 1}), "unknown_keyword", field)

    class Keyword(str):
        def __str__(self):
            raise AssertionError("构造诊断必须保留关键字原始内容")

    class ShapeProbe:
        calls = 0

        def __index__(self):
            type(self).calls += 1
            raise AssertionError("错误构造形状不得执行值转换")

    reject(lambda: triviumdb.TriviumDB(file, dim=ShapeProbe(), **{Keyword("未知"): 1}), "unknown_keyword", "未知")
    reject(lambda: triviumdb.ResourceRuntime(ShapeProbe(), memory_limit_mb=1), "multiple_values", "memory_limit_mb")
    reject(lambda: triviumdb.ConsoleSession("{}", options_json="{}"), "multiple_values", "options_json")
    assert ShapeProbe.calls == 0
    assert triviumdb.ResourceRuntime.__name__ == "ResourceRuntime"
    assert isinstance(triviumdb.ResourceRuntime(max_threads=1), triviumdb.ResourceRuntime)
    assert isinstance(triviumdb.ConsoleSession(options_json="{}"), triviumdb.ConsoleSession)
    for case in fixture["integers"]:
        field = case["python"]
        for value in ("1", 0.5, [], -1, 1 << (struct.calcsize("P") * 8)):
            invalid(lambda: triviumdb.TriviumDB(file, **{field: value}), field, (TypeError, OverflowError))
    for case in fixture["strings"]:
        field = case["python"]
        invalid(lambda: triviumdb.TriviumDB(file, **{field: 1}), field, TypeError)
        invalid(lambda: triviumdb.TriviumDB(file, **{field: "invalid"}), field, ValueError)
        invalid(lambda: triviumdb.TriviumDB(file, **{field: "\ud800"}), field, UnicodeEncodeError)
    invalid(lambda: triviumdb.TriviumDB(1), "path", TypeError)
    invalid(lambda: triviumdb.TriviumDB("\ud800"), "path", UnicodeEncodeError)
    for case in fixture["booleans"]:
        field = case["python"]
        for value in (None, 0, 1, "false", [], {}):
            invalid(lambda: triviumdb.TriviumDB(file, **{field: value}), field, TypeError)
    for value in (1, "runtime", [], {}, triviumdb.ConsoleSession(), triviumdb.ResourceBudgetError()):
        invalid(lambda: triviumdb.TriviumDB(file, runtime=value), "runtime", TypeError)
        invalid(lambda: triviumdb.verify_snapshot("unused", value), "runtime", TypeError)
        invalid(lambda: triviumdb.restore_snapshot("unused", "unused-destination", value),
                "runtime", TypeError, {"phase": "validation", "outcome": "not_committed"})
    class OpaqueRuntime:
        calls = 0

        def __getattribute__(self, name):
            type(self).calls += 1
            raise AssertionError("原生 runtime 类型校验不得执行宿主属性")

    invalid(lambda: triviumdb.TriviumDB(file, runtime=OpaqueRuntime()), "runtime", TypeError)
    assert OpaqueRuntime.calls == 0
    for case in fixture["runtime_integers"]:
        field = case["python"]
        for value in ("1", 0.5, [], -1):
            invalid(lambda: triviumdb.ResourceRuntime(**{field: value}), field, (TypeError, OverflowError))
    for field in ("memory_limit_mb", "payload_cache_mb", "payload_cache_entry_mb"):
        invalid(lambda: triviumdb.TriviumDB(file, **{field: 1 << 63}), field, OverflowError)
    for field in ("memory_limit_mb", "maintenance_memory_limit_mb"):
        invalid(lambda: triviumdb.ResourceRuntime(**{field: 1 << 63}), field, OverflowError)
    invalid(lambda: triviumdb.ResourceRuntime(max_threads=65), "max_threads", ValueError)
    invalid(lambda: triviumdb.ResourceRuntime(memory_limit_mb=1, maintenance_memory_limit_mb=2), "maintenance_memory_limit_mb", ValueError)
    invalid(lambda: triviumdb.ConsoleSession(1), "options_json", TypeError)
    invalid(lambda: triviumdb.ConsoleSession("\ud800"), "options_json", UnicodeEncodeError)
    try:
        triviumdb.ConsoleSession("{")
    except triviumdb.InvalidInputError as error:
        assert error.target == {"field": "options_json"} and error.write is None
    else:
        raise AssertionError("无效会话配置必须拒绝")
    assert not parent.exists()
    cause = RuntimeError("宿主 cause")
    sentinel = TypeError("宿主构造整数异常")
    sentinel.code = "HOST_INTEGER"
    class HostInteger:
        def __index__(self):
            raise sentinel from cause
    for action in (lambda: triviumdb.TriviumDB(file, dim=HostInteger()),
                   lambda: triviumdb.ResourceRuntime(max_threads=HostInteger())):
        try:
            action()
        except TypeError as error:
            assert error is sentinel and error.__cause__ is cause and error.code == "HOST_INTEGER"
            assert not hasattr(error, "target")
        else:
            raise AssertionError("宿主协议异常必须原样传播")
    for dtype in ("f32", "f16", "u64"):
        runtime = triviumdb.ResourceRuntime(memory_limit_mb=8, maintenance_memory_limit_mb=1, max_threads=2)
        options = dict(dim=2, dtype=dtype, auto_build_quiver=False, memory_limit_mb=1,
                       load_text_index=True, payload_cache_mb=0, payload_cache_entry_mb=0, runtime=runtime)
        directory = Path(root) / f"constructor-{dtype}"
        path = str(directory / "database.tdb")
        def released(attached=0):
            stats = runtime.stats()
            assert stats["attached_databases"] == attached
            assert stats["reserved_database_bytes"] == attached * 1024 * 1024
            assert stats["reserved_workspace_bytes"] == 0 and stats["active_tasks"] == 0
        try:
            triviumdb.TriviumDB(path, **(options | {"memory_limit_mb": 9}))
        except triviumdb.ResourceBudgetError as error:
            assert error.code == "QueueFull"
            assert error.details == dict(resource="memory_bytes", requested=9 * 1024 * 1024,
                                         used=0, limit=7 * 1024 * 1024)
            assert error.write is None
        else:
            raise AssertionError("共享运行时必须在创建文件前拒绝超额配额")
        released()
        assert not directory.exists()
        for dim in (0, 65537, (1 << (struct.calcsize("P") * 8)) - 1):
            invalid(lambda: triviumdb.TriviumDB(path, **(options | {"dim": dim})), "dim", ValueError)
            released()
            assert not directory.exists()
        db = triviumdb.TriviumDB(path, **options)
        try:
            db.insert_with_id(**fixture["saved"])
            wal = Path(path + ".wal").read_bytes()
            for case in fixture["booleans"]:
                invalid(lambda: triviumdb.TriviumDB(path, **(options | {case["python"]: 1})), case["python"], TypeError)
                released(1)
            invalid(lambda: triviumdb.TriviumDB(path, **(options | {"runtime": {}})), "runtime", TypeError)
            assert Path(path + ".wal").read_bytes() == wal
            released(1)
            try:
                triviumdb.TriviumDB(path, **options)
            except triviumdb.TriviumDBError as error:
                assert error.code == "TDB_DATABASE_LOCKED"
                held_error = error
            else:
                raise AssertionError("重复 writer 必须拒绝")
            released(1)
            held = db.get(7)
            snapshot = str(directory.with_name(directory.name + "-snapshot"))
            restored = str(directory.with_name(directory.name + "-restored"))
            db.create_snapshot(snapshot, "runtime-containers")
            assert triviumdb.verify_snapshot(snapshot, None)["node_count"] == 1
            assert triviumdb.restore_snapshot(snapshot, restored, None)["generation_id"] == "runtime-containers"
            recovered = triviumdb.TriviumDB(str(Path(restored) / "database.tdb"), **options)
            try:
                assert recovered.all_node_ids() == [7]
                assert node(recovered.get(7)) == fixture["saved"] | {"num_edges": 0, "edges": []}
            finally:
                recovered.close()
            released(1)
        finally:
            db.close()
        released()
        moved = directory.with_name(directory.name + "-moved")
        directory.rename(moved)
        assert held_error.code == "TDB_DATABASE_LOCKED"
        assert node(held) == fixture["saved"] | {"num_edges": 0, "edges": []}
        path = str(moved / "database.tdb")
        try:
            triviumdb.TriviumDB(path, **(options | {"dim": 3}))
        except triviumdb.TriviumDBError as error:
            assert error.code == "TDB_DIMENSION_MISMATCH"
        else:
            raise AssertionError("打开维度与保存身份不符时必须拒绝")
        released()
        for _ in range(2):
            reopened = triviumdb.TriviumDB(path, **options)
            try:
                assert reopened.all_node_ids() == [7]
                assert node(reopened.get(7)) == fixture["saved"] | {"num_edges": 0, "edges": []}
                reopened.flush()
            finally:
                reopened.close()
            released()
