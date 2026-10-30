"""已安装 wheel 的控制台协议、实例绑定和资源生命周期验收。"""

import json
import threading
from pathlib import Path

import triviumdb


def run_console_contract(root: str) -> None:
    fixture = json.loads((Path(__file__).parents[1] / "contracts/console_cases.json").read_text(encoding="utf-8"))

    def wire(command):
        return json.dumps({"version": 1, "requestId": "验收请求", "command": command})

    for index, case in enumerate(fixture["cases"]):
        file = str(Path(root) / f"console-{index}.tdb")
        db = triviumdb.TriviumDB(file, dim=2, auto_build_quiver=False)
        session = triviumdb.ConsoleSession(json.dumps(case["options"]))
        if case.get("detached"):
            session.detach()
        try:
            for node in fixture["setup"]:
                db.insert_with_id(node["id"], node["vector"], node["payload"])
            if "error" in case:
                try:
                    db.console_request(session, wire(case["command"]))
                except (RuntimeError, ValueError) as error:
                    assert error.code == case["error"]["code"], case["name"]
                    assert error.details == case["error"]["details"], case["name"]
                else:
                    raise AssertionError(f"必须拒绝请求：{case['name']}")
            else:
                response = json.loads(db.console_request(session, wire(case["command"])))
                assert response == {"version": 1, "requestId": "验收请求", "data": case["expected"]}, case["name"]

            def verify(database):
                for expected in case.get("verify", []):
                    node = database.get(int(expected["id"]))
                    assert node is not None
                    assert node.id == int(expected["id"])
                    assert node.vector == expected["vector"]
                    assert node.payload == expected["payload"]
                    if "edges" in expected:
                        assert [{"target": str(edge.target_id), "label": edge.label, "weight": edge.weight, "metadata": edge.metadata} for edge in node.edges] == expected["edges"]
                for absent in case.get("absent", []):
                    assert database.get(int(absent)) is None

            verify(db)
            db.close()
            reopened = triviumdb.TriviumDB(file, dim=2, auto_build_quiver=False)
            try:
                verify(reopened)
                try:
                    reopened.console_request(session, wire({"operation": "info"}))
                except (ValueError, RuntimeError) as error:
                    assert error.code == ("TDB_CONSOLE_SESSION_CLOSED" if case.get("detached") else "TDB_INVALID_INPUT")
                else:
                    raise AssertionError("旧会话不得绑定同路径的新实例")
            finally:
                reopened.close()
            session.detach()
            assert session.is_detached()
            assert not session.cancel("验收请求")
        finally:
            db.close()

    for dtype in ["f16", "u64"]:
        db = triviumdb.TriviumDB(str(Path(root) / f"console-{dtype}.tdb"), dim=2, dtype=dtype)
        try:
            db.insert_with_id(1, [1, 0], {"dtype": dtype})
            session = triviumdb.ConsoleSession()
            node = json.loads(db.console_request(session, wire({"operation": "getNodes", "ids": ["1"]})))["data"]["nodes"][0]
            assert node == {"id": "1", "vector": ["1", "0"] if dtype == "u64" else [1.0, 0.0], "payload": {"dtype": dtype}, "edges": []}
            if dtype == "f16":
                writable = triviumdb.ConsoleSession('{"readOnly":false}')
                db.console_request(writable, wire({"operation": "importNodes", "records": [{"id": "2", "vector": [1.0007, 1.0004882812500002], "payload": {}}]}))
                assert db.get(2).vector == [1.0009765625, 1.0]
        finally:
            db.close()

    file = str(Path(root) / "console-cancel.tdb")
    db = triviumdb.TriviumDB(file, dim=2)
    session = triviumdb.ConsoleSession()
    entered = threading.Event()
    release = threading.Event()
    failures = []

    class BlockingHook:
        def on_pre_search(self, query, ctx):
            entered.set()
            assert release.wait(30), "必须收到受控 Hook 释放信号"
            return query

    def run():
        try:
            db.console_request(session, wire({"operation": "search", "vector": [1, 0], "topK": 1}))
        except RuntimeError as error:
            failures.append(error)

    db.insert_with_id(1, [1, 0], {"owner": "host"})
    db.set_hook(BlockingHook())
    worker = threading.Thread(target=run)
    worker.start()
    try:
        assert entered.wait(30), "真实原生请求必须进入 Hook"
        session.detach()
    finally:
        release.set()
        worker.join(30)
    assert not worker.is_alive(), "原生请求必须实际退出"
    assert len(failures) == 1
    assert failures[0].code == "TDB_QUERY_CANCELLED"
    assert db.get_payload(1) == {"owner": "host"}
    db.close()
    reopened = triviumdb.TriviumDB(file, dim=2)
    assert reopened.get_payload(1) == {"owner": "host"}
    reopened.close()
