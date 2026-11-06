"""按共享独立样例核对实际 Python API 的错误类别与完整字段。"""

import json
from collections import UserDict
from pathlib import Path
from types import MappingProxyType
import triviumdb
from write_outcomes import run_write_outcomes


def run_error_contract(root):
    contract = json.loads((Path(__file__).parents[1] / "contracts/error_cases.json").read_text(encoding="utf-8"))
    for case in contract["cases"]:
        operation = case["operation"]
        directory = Path(root) / f"errors-{operation}"
        directory.mkdir()
        file = str(directory / "database.tdb")
        db = triviumdb.TriviumDB(file, dim=2, auto_build_quiver=False)
        try:
            db.insert_with_id(1, [1, 0], {"kind": "alpha"})
            db.insert_with_id(2, [0, 1], {"kind": "beta"})
            if operation in ("readonly", "rows"):
                db.close()
                db = triviumdb.TriviumDB(file, dim=2, access_mode="read_only", max_query_rows=1)
            if operation == "unique":
                db.create_unique_index("kind")
            if operation == "closed":
                db.close()
            if operation == "io":
                (directory / "blocked").write_text("文件占据父路径", encoding="utf-8")
            if operation == "version":
                header = bytearray(58)
                header[:4] = b"TVDB"
                header[4:6] = (65535).to_bytes(2, "little")
                (directory / "future.tdb").write_bytes(header)
            actions = {
                "missing_parameter": lambda: db.execute_prepared_tql(db.prepare_tql("MATCH (n) RETURN $required AS value"), {}),
                "unexpected_parameter": lambda: db.execute_prepared_tql(db.prepare_tql("MATCH (n) RETURN $required AS value"), {"required": 1, "extra": 2}),
                "dimension": lambda: db.insert([1], {}),
                "missing_node": lambda: db.update_payload(999, {}),
                "duplicate_node": lambda: db.insert_with_id(1, [1, 0], {}),
                "unique": lambda: db.insert_with_id(3, [1, 0], {"kind": "alpha"}),
                "readonly": lambda: db.insert([1, 0], {}),
                "rows": lambda: db.tql("MATCH (n) RETURN n"),
                "migration": lambda: db.tql_mut("MATCH (n) RETURN n"),
                "closed": lambda: db.get_payload(1),
                "locked": lambda: triviumdb.TriviumDB(file, dim=2),
                "io": lambda: triviumdb.TriviumDB(str(directory / "blocked/database.tdb"), dim=2),
                "version": lambda: triviumdb.TriviumDB(str(directory / "future.tdb"), dim=2),
            }
            try:
                actions[operation]()
            except triviumdb.TriviumDBError as error:
                assert error.schema_version == 1, operation
                assert error.message == str(error), operation
                assert error.code == case["code"], (operation, error.code)
                details = dict(error.details)
                for field in case.get("string_fields", []):
                    value = details.pop(field)
                    assert isinstance(value, str) and value, (operation, field)
                for field in case.get("integer_fields", []):
                    assert type(details.pop(field)) is int, (operation, field)
                assert details == case["details"], (operation, details)
                assert error.write == case.get("write"), operation
                for field, value in error.details.items():
                    assert getattr(error, field) == value, (operation, field)
            else:
                raise AssertionError(f"操作应返回结构化错误：{operation}")
        finally:
            db.close()
    run_query_diagnostics(root)
    run_prepared_diagnostics(root)
    run_write_outcomes(root)


def run_prepared_diagnostics(root):
    cases = json.loads((Path(__file__).parents[1] / "contracts/prepared_diagnostics.json").read_text(encoding="utf-8"))["cases"]
    for dtype in ("f32", "f16", "u64"):
        file = str(Path(root) / f"prepared-diagnostics-{dtype}.tdb")
        db = triviumdb.TriviumDB(file, dim=2, dtype=dtype, auto_build_quiver=False)
        try:
            db.insert_with_id(7, [1, 0], {"saved": "完整事实"})
            session = triviumdb.ConsoleSession()
            before = Path(file + ".wal").read_bytes()
            for case in cases:
                prepared = db.prepare_tql(case["query"])
                request = json.dumps({"version": 1, "requestId": case["name"], "command": {
                    "operation": "query", "query": case["query"], "parameters": case["parameters"],
                }})
                for action in (
                    lambda: db.execute_prepared_tql(prepared, case["parameters"]),
                    lambda: db.console_request(session, request),
                ):
                    try:
                        action()
                    except (triviumdb.TriviumDBError, triviumdb.InvalidInputError) as error:
                        assert error.code == case["code"], case
                        assert error.schema_version == 1
                        assert error.stage == "bind"
                        assert error.target == {"parameter": case["parameter"]}
                        if "details" in case:
                            assert error.details == case["details"]
                        else:
                            assert list(error.details) == ["reason"]
                            assert isinstance(error.details["reason"], str) and error.details["reason"]
                        if case["code"] == "TDB_INVALID_INPUT":
                            assert isinstance(error, triviumdb.InvalidInputError)
                    else:
                        raise AssertionError(f"必须拒绝参数：{case['name']}")
            prepared = db.prepare_tql("MATCH (n) RETURN $x AS value")
            assert [row.row for row in db.execute_prepared_tql(prepared, {"x": 42})] == [{"value": 42}]
            for parameters in (MappingProxyType({"x": 42}), UserDict({"x": 42})):
                assert [row.row for row in db.execute_prepared_tql(prepared, parameters)] == [{"value": 42}]
            for parameters in (None, [], [("x", 42)], 42, "x", {7: 42}):
                try:
                    db.execute_prepared_tql(prepared, parameters)
                except TypeError as error:
                    assert error.schema_version == 1
                    assert error.code == "TDB_INVALID_INPUT"
                    assert error.target == {"field": "parameters"}
                    assert error.stage is None and error.write is None
                else:
                    raise AssertionError("参数容器或键必须符合映射契约")
            for value, exception in ((float("nan"), ValueError), (float("inf"), ValueError),
                                     (1 << 100, OverflowError), (object(), TypeError)):
                try:
                    db.execute_prepared_tql(prepared, {"x": value})
                except exception as error:
                    assert error.code == "TDB_INVALID_INPUT"
                    assert error.target == {"parameter": "x"}
                    assert error.stage == "bind" and error.write is None
                else:
                    raise AssertionError("参数值必须经过具名绑定诊断")
            cause = RuntimeError("宿主映射原因")
            sentinel = TypeError("宿主映射失败")
            sentinel.__cause__ = cause
            class BrokenMapping(UserDict):
                def items(self):
                    raise sentinel
            try:
                db.execute_prepared_tql(prepared, BrokenMapping(x=42))
            except TypeError as error:
                assert error is sentinel and error.__cause__ is cause
                assert not hasattr(error, "code")
            else:
                raise AssertionError("必须保留宿主映射异常")
            callbacks = []
            class CallbackMapping(UserDict):
                def items(self):
                    try:
                        db.all_node_ids()
                    except triviumdb.TriviumDBError as error:
                        assert error.code == "TDB_REENTRANT_ACCESS"
                        assert error.write is None
                    else:
                        raise AssertionError("映射转换中的重入必须被拒绝")
                    callbacks.append(True)
                    return super().items()
            assert [row.row for row in db.execute_prepared_tql(prepared, CallbackMapping(x=42))] == [{"value": 42}]
            assert callbacks == [True]
            assert Path(file + ".wal").read_bytes() == before
        finally:
            db.close()
        reopened = triviumdb.TriviumDB(file, dim=2, dtype=dtype, auto_build_quiver=False)
        try:
            assert reopened.all_node_ids() == [7]
            node = reopened.get(7)
            assert node.vector == [1, 0] and node.payload == {"saved": "完整事实"} and node.edges == []
        finally:
            reopened.close()


def run_query_diagnostics(root):
    from query_arguments import run_query_arguments
    run_query_arguments(root)
    cases = json.loads((Path(__file__).parents[1] / "contracts/query_diagnostics.json").read_text(encoding="utf-8"))["cases"]
    for dtype in ("f32", "f16", "u64"):
        file = str(Path(root) / f"diagnostics-{dtype}.tdb")
        db = triviumdb.TriviumDB(file, dim=2, dtype=dtype, auto_build_quiver=False)
        try:
            db.insert_with_id(7, [1, 0], {"saved": "完整事实"})
            session = triviumdb.ConsoleSession('{"readOnly":false}')
            for case in cases:
                query = case["query"]
                actions = [lambda: db.tql_mut(query)] if case.get("mutation") else [
                    lambda: db.tql(query), lambda: db.prepare_tql(query),
                ]
                request = json.dumps({"version": 1, "requestId": case["name"], "command": {
                    "operation": "mutate" if case.get("mutation") else "query", "query": query,
                }})
                actions.append(lambda: db.console_request(session, request))
                for action in actions:
                    try:
                        action()
                    except triviumdb.TriviumDBError as error:
                        assert error.schema_version == 1
                        assert error.code == "TDB_QUERY_PARSE", case
                        assert error.stage == case["stage"], case
                        assert error.write == ({"phase": "validation", "outcome": "not_committed"} if case.get("mutation") else None), case
                        assert error.target == ({"query_span": case["span"]} if case["span"] else None), case
                        assert list(error.details) == ["reason"]
                        assert isinstance(error.details["reason"], str) and error.details["reason"]
                    else:
                        raise AssertionError(f"必须拒绝查询：{case['name']}")
        finally:
            db.close()
        reopened = triviumdb.TriviumDB(file, dim=2, dtype=dtype, auto_build_quiver=False)
        try:
            assert reopened.all_node_ids() == [7]
            node = reopened.get(7)
            assert node.vector == [1, 0] and node.payload == {"saved": "完整事实"} and node.edges == []
        finally:
            reopened.close()
