"""公开调用形状、原异常传播与事务缓冲的实际 wheel 契约。"""

import inspect
import json
from pathlib import Path

import triviumdb
from read_arguments import node


def reject(action, kind, field, write=None):
    try:
        action()
    except TypeError as error:
        assert error.schema_version == 1 and error.code == "TDB_INVALID_INPUT"
        assert error.target == {"field": field} and error.stage is None
        assert set(error.details) == {"reason", "argument_kind"}
        assert error.details["argument_kind"] == kind and error.details["reason"]
        assert error.write == write
    else:
        raise AssertionError(f"调用形状必须拒绝 {kind} {field}")


def run_call_arguments(root):
    contracts = Path(__file__).parents[1] / "contracts"
    cases = json.loads((contracts / "call_arguments.json").read_text(encoding="utf-8"))
    graph = json.loads((contracts / "graph_arguments.json").read_text(encoding="utf-8"))
    validation = {"phase": "validation", "outcome": "not_committed"}
    transaction_validation = validation | {"transaction_consumed": False}
    for dtype in ("f32", "f16", "u64"):
        directory = Path(root) / f"call-arguments-{dtype}"
        directory.mkdir()
        file = str(directory / "database.tdb")
        options = dict(dim=2, dtype=dtype, auto_build_quiver=False)
        db = triviumdb.TriviumDB(file, **options)
        try:
            for value in graph["nodes"]:
                db.insert_with_id(**value)
            before = Path(file + ".wal").read_bytes()
            for case in cases["missing"]:
                method = getattr(db, case["python"])
                field = case.get("python_field", case.get("field"))
                reject(lambda: method(*case["args"]), "missing", field,
                       validation if case.get("write") else None)
            for field in ("unexpected", "证据.路径/[]\x00尾", "\ud800"):
                reject(lambda: db.get(7, **{field: 1}), "unknown_keyword", field)
            # 多个问题按固定结构优先级定位，保持原始关键字内容。
            reject(lambda: db.get(z=1, a=2), "unknown_keyword", "a")
            reject(lambda: db.get(7, id=11), "multiple_values", "id")
            reject(lambda: db.__contains__(key=7), "positional_only", "key")
            reject(lambda: db.get(7, 11), "too_many_positional", "arguments")
            reject(lambda: db.get(7, 11, unexpected=True), "too_many_positional", "arguments")
            reject(lambda: db.search([1, 0], unexpected=True), "unknown_keyword", "unexpected")
            reject(lambda: db.insert([1, 0], {}, unexpected=True), "unknown_keyword", "unexpected", validation)
            reject(lambda: db.flush(unexpected=True), "unknown_keyword", "unexpected", validation)
            reject(lambda: db.close(unexpected=True), "unknown_keyword", "unexpected")
            reject(lambda: triviumdb.verify_snapshot(), "missing", "directory")
            reject(lambda: triviumdb.restore_snapshot("unused"), "missing", "destination", validation)
            reject(lambda: triviumdb.init_logger(unexpected=True), "unknown_keyword", "unexpected")
            session = triviumdb.ConsoleSession()
            reject(lambda: session.cancel(), "missing", "request_id")
            runtime = triviumdb.ResourceRuntime()
            reject(lambda: runtime.stats(1), "too_many_positional", "arguments")

            class Probe:
                calls = 0

                def __index__(self):
                    self.calls += 1
                    raise AssertionError("错误调用形状不得执行整数协议")

                def __iter__(self):
                    self.calls += 1
                    raise AssertionError("错误调用形状不得执行序列协议")

            probe = Probe()
            class Keyword(str):
                def __str__(self):
                    raise AssertionError("错误诊断应保留关键字的原始字符串内容")

            reject(lambda: db.get(7, **{Keyword("原始关键字"): probe}), "unknown_keyword", "原始关键字")
            reject(lambda: db.get(probe, unexpected=True), "unknown_keyword", "unexpected")
            reject(lambda: db.insert(probe), "missing", "payload", validation)
            assert probe.calls == 0
            cause = RuntimeError("宿主 cause")
            sentinel = TypeError("宿主整数异常")
            sentinel.code = "HOST_INTEGER"
            sentinel.target = {"host": True}

            class HostId:
                def __index__(self):
                    raise sentinel from cause

            try:
                db.get(id=HostId())
            except TypeError as error:
                assert error is sentinel and error.__cause__ is cause
                assert error.code == "HOST_INTEGER" and error.target == {"host": True}
                assert not hasattr(error, "details")
            else:
                raise AssertionError("合法调用形状中的宿主异常必须原样传播")
            assert db.get(id=7).payload == {"name": "源"} and 7 in db
            assert db.get_incoming_edges(id=7, label=None) == []
            db.set_memory_limit()
            assert inspect.signature(db.get) == inspect.Signature([
                inspect.Parameter("id", inspect.Parameter.POSITIONAL_OR_KEYWORD)])
            assert isinstance(db, triviumdb.TriviumDB)
            held = [db.get(value["id"]) for value in graph["nodes"]]
            assert [node(value) for value in held] == [value | {"num_edges": 0, "edges": []}
                                                      for value in graph["nodes"]]
            tx = db.transaction()
            tx.insert_with_id(19, [1, 1], {"name": "待提交"})
            for action, kind, field in (
                (lambda: tx.insert([1, 0]), "missing", "payload"),
                (lambda: tx.update_payload(7), "missing", "payload"),
                (lambda: tx.commit(unexpected=True), "unknown_keyword", "unexpected"),
                (lambda: tx.rollback(1), "too_many_positional", "arguments"),
            ):
                reject(action, kind, field, transaction_validation)
                assert tx.pending_count() == 1
            assert Path(file + ".wal").read_bytes() == before
            assert db.all_node_ids() == [7, 11, 13]
            assert tx.commit() == [19]
            assert node(db.get(19)) == dict(id=19, vector=[1, 1], payload={"name": "待提交"}, num_edges=0, edges=[])
        finally:
            db.close()
        moved = directory.with_name(directory.name + "-moved")
        assert directory.is_absolute() and moved.parent == directory.parent
        directory.rename(moved)
        assert [node(value) for value in held] == [value | {"num_edges": 0, "edges": []}
                                                  for value in graph["nodes"]]
        for _ in range(2):
            reopened = triviumdb.TriviumDB(str(moved / "database.tdb"), **options)
            try:
                assert reopened.all_node_ids() == [7, 11, 13, 19]
                expected = graph["nodes"] + [dict(id=19, vector=[1, 1], payload={"name": "待提交"})]
                assert [node(reopened.get(value["id"])) for value in expected] == [
                    value | {"num_edges": 0, "edges": []} for value in expected]
                reopened.flush()
            finally:
                reopened.close()
