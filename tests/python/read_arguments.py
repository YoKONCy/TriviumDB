"""基础读取的字段诊断、请求借用、完整结果和资源释放契约。"""

import json
import threading
from pathlib import Path
import triviumdb
from query_arguments import check_input_error


def edges(values):
    return sorted([dict(target_id=value.target_id, label=value.label,
                        weight=value.weight, metadata=value.metadata) for value in values],
                  key=lambda value: (value["target_id"], value["label"]))


def incoming(values):
    return [dict(source_id=value.source_id, target_id=value.target_id, label=value.label,
                 weight=value.weight, metadata=value.metadata) for value in values]


def node(value):
    assert value is not None
    return dict(id=value.id, vector=value.vector, payload=value.payload,
                num_edges=value.num_edges, edges=edges(value.edges))


def run_read_arguments(root):
    contracts = Path(__file__).parents[1] / "contracts"
    graph = json.loads((contracts / "graph_arguments.json").read_text(encoding="utf-8"))
    case = json.loads((contracts / "read_arguments.json").read_text(encoding="utf-8"))
    for dtype in ("f32", "f16", "u64"):
        directory = Path(root) / f"read-arguments-{dtype}"
        directory.mkdir()
        file = str(directory / "database.tdb")
        options = dict(dim=2, dtype=dtype, auto_build_quiver=False)
        db = triviumdb.TriviumDB(file, **options)
        try:
            for value in graph["nodes"]:
                db.insert_with_id(value["id"], value["vector"], value["payload"])
            for value in graph["edges"] + case["extra_edges"]:
                db.upsert_edge(value["source_id"], value["target_id"], value["label"],
                               value["weight"], value["metadata"])
            before = Path(file + ".wal").read_bytes()
            readers = (
                ("get", lambda value: node(db.get(value)), 7, case["source"]),
                ("get_payload", db.get_payload, 7, case["source"]["payload"]),
                ("contains", lambda value: value in db, 7, True),
                ("get_edges", lambda value: edges(db.get_edges(value)), 7, case["source"]["edges"]),
                ("get_incoming_edges", lambda value: incoming(db.get_incoming_edges(value)), 11, case["incoming"]),
                ("get_edge/src", lambda value: edges([db.get_edge(value, 11, "route")]), 7, [case["source"]["edges"][1]]),
                ("get_edge/dst", lambda value: edges([db.get_edge(7, value, "route")]), 11, [case["source"]["edges"][1]]),
            )

            def reject_reentry():
                for _, action, value, _ in readers:
                    try:
                        action(value)
                    except triviumdb.TriviumDBError as error:
                        assert error.schema_version == 1 and error.stage is None and error.target is None
                        assert error.code == "TDB_REENTRANT_ACCESS"
                        assert error.details == {"operation": "python_callback"}
                        assert error.write is None
                    else:
                        raise AssertionError("整数回调中的同库读取必须拒绝")
                try:
                    db.update_payload(7, {"name": "回调写入"})
                except triviumdb.TriviumDBError as error:
                    assert error.code == "TDB_REENTRANT_ACCESS"
                    assert error.write == {"phase": "admission", "outcome": "not_committed"}
                else:
                    raise AssertionError("整数回调中的同库写入必须拒绝")

            for name, action, valid, expected in readers:
                field = name.split("/")[-1] if "/" in name else "id"
                for value, kind in ((None, TypeError), ("7", TypeError), (1.5, TypeError),
                                    ([], TypeError), ({}, TypeError), (-1, OverflowError),
                                    (1 << 64, OverflowError)):
                    try:
                        action(value)
                    except kind as error:
                        check_input_error(error, field)
                    else:
                        raise AssertionError(f"必须拒绝读取 ID {name}/{value!r}")
                calls = []
                class Id:
                    def __index__(self):
                        reject_reentry()
                        calls.append(True)
                        return valid
                assert action(Id()) == expected
                assert calls == [True]
                cause = RuntimeError("读取 ID 协议原因")
                sentinel = TypeError("读取 ID 协议失败")
                sentinel.__cause__ = cause
                class BrokenId:
                    def __index__(self):
                        raise sentinel
                try:
                    action(BrokenId())
                except TypeError as error:
                    assert error is sentinel and error.__cause__ is cause
                    assert not hasattr(error, "code")
                else:
                    raise AssertionError("必须保留读取整数协议原异常")
                assert action(valid) == expected
            for action in (lambda label: db.get_edge(7, 11, label),
                           lambda label: db.get_incoming_edges(11, label)):
                for value, kind in ((1, TypeError), (True, TypeError), ([], TypeError),
                                    ({}, TypeError), (b"route", TypeError),
                                    ("\ud800", UnicodeEncodeError)):
                    try:
                        action(value)
                    except kind as error:
                        check_input_error(error, "label")
                    else:
                        raise AssertionError("读取标签必须为可编码字符串")
            class Label(str):
                def __str__(self):
                    raise AssertionError("读取字符串子类应复用原始字符")
            for expected in case["source"]["edges"]:
                label = Label(expected["label"])
                assert edges([db.get_edge(7, 11, label)]) == [expected]
                assert incoming(db.get_incoming_edges(11, label)) == [
                    dict(source_id=7, **expected)]
            assert incoming(db.get_incoming_edges(11, None)) == case["incoming"]
            try:
                db.get_edge(7, 11, None)
            except TypeError as error:
                check_input_error(error, "label")
            else:
                raise AssertionError("精确边读取标签为必需实参")
            for absent in (0, 999, (1 << 64) - 1):
                assert db.get(absent) is None and db.get_payload(absent) is None
                assert absent not in db
                assert db.get_edges(absent) == [] and db.get_incoming_edges(absent) == []
                assert db.get_edge(absent, 11, "route") is None
            completed = threading.Event()
            failures = []
            def concurrent_read():
                try:
                    for _, action, value, expected in readers:
                        assert action(value) == expected
                    try:
                        db.update_payload(7, {})
                    except triviumdb.TriviumDBError as error:
                        assert error.code == "TDB_BUSY"
                        assert error.details == {"resource": "python_database"}
                        assert error.write == {"phase": "admission", "outcome": "not_committed"}
                    else:
                        raise AssertionError("共享读取期间的写争用必须拒绝")
                except BaseException as error:
                    failures.append(error)
                finally:
                    completed.set()
            class ConcurrentId:
                def __index__(self):
                    worker = threading.Thread(target=concurrent_read, daemon=True)
                    worker.start()
                    # Event 确认真实读取及争用结束，期限只防止测试挂起。
                    assert completed.wait(10)
                    worker.join(10)
                    assert not worker.is_alive() and failures == []
                    return 7
            assert node(db.get(ConcurrentId())) == case["source"]
            assert completed.is_set() and failures == []
            held = db.get(7)
            held_payload = db.get_payload(7)
            held_edges = db.get_edges(7)
            held_incoming = db.get_incoming_edges(11)
            assert Path(file + ".wal").read_bytes() == before
            db.update_payload(7, {"name": "已更新"})
            db.update_vector([0, 1], 7)
        finally:
            db.close()
        moved = directory.with_name(directory.name + "-moved")
        assert directory.is_absolute() and moved.parent == directory.parent
        directory.rename(moved)
        assert node(held) == case["source"] and held_payload == {"name": "源"}
        assert edges(held_edges) == case["source"]["edges"] and incoming(held_incoming) == case["incoming"]
        try:
            db.get(7)
        except triviumdb.TriviumDBError as error:
            assert error.code == "TDB_DATABASE_CLOSED"
        else:
            raise AssertionError("关闭后的旧实例必须明确拒绝读取")
        for _ in range(2):
            reopened = triviumdb.TriviumDB(str(moved / "database.tdb"), **options)
            try:
                assert reopened.all_node_ids() == [7, 11, 13]
                assert node(reopened.get(7)) == case["source"] | {"vector": [0, 1], "payload": {"name": "已更新"}}
                for value in graph["nodes"][1:]:
                    expected_edges = [dict(target_id=edge["target_id"], label=edge["label"],
                                           weight=edge["weight"], metadata=edge["metadata"])
                                      for edge in graph["edges"] if edge["source_id"] == value["id"]]
                    assert node(reopened.get(value["id"])) == value | {"num_edges": len(expected_edges), "edges": expected_edges}
                assert incoming(reopened.get_incoming_edges(11)) == case["incoming"]
                reopened.flush()
            finally:
                reopened.close()
