"""按实际事务入口验证写入结果、缓冲所有权及响应失败后的确认。"""

import json
from pathlib import Path
import triviumdb


def expect_write(action, code, details, write):
    try:
        action()
    except (triviumdb.TriviumDBError, triviumdb.InvalidInputError) as error:
        assert error.schema_version == 1
        assert error.code == code
        assert error.details == details
        assert error.write == write
    else:
        raise AssertionError("请求必须返回明确写入错误")


def run_write_outcomes(root):
    contract = json.loads((Path(__file__).parents[1] / "contracts/write_outcomes.json").read_text(encoding="utf-8"))
    for dtype in ("f32", "f16", "u64"):
        file = str(Path(root) / f"write-outcomes-{dtype}.tdb")
        db = triviumdb.TriviumDB(file, dim=2, dtype=dtype, auto_build_quiver=False)
        try:
            db.insert_with_id(1, [1, 0], {"state": "saved"})
            before = Path(file + ".wal").read_bytes()
            expect_write(lambda: db.insert([1], {}), "TDB_DIMENSION_MISMATCH", {"expected": 2, "got": 1}, contract["direct_validation"])
            expect_write(lambda: db.update_payload(999, {}), "TDB_NODE_NOT_FOUND", {"id": 999}, contract["direct_validation"])
            expect_write(lambda: db.tql_mut("MATCH (n) SET VECTOR(n) == [1]"), "TDB_DIMENSION_MISMATCH", {"expected": 2, "got": 1}, contract["direct_validation"])
            for context in (False, True):
                tx = db.transaction()
                tx.insert_with_id(2, [1], {})
                action = (lambda: tx.__exit__(None, None, None)) if context else tx.commit
                expect_write(action, "TDB_DIMENSION_MISMATCH", {"expected": 2, "got": 1}, contract["validation"])
                assert tx.pending_count() == 0
                expect_write(tx.commit, "TDB_TRANSACTION_FINISHED", {}, contract["admission"])
            assert Path(file + ".wal").read_bytes() == before
            with db.transaction() as tx:
                tx.insert_with_id(2, [0, 1], {"state": "valid"})
            session = triviumdb.ConsoleSession('{"readOnly":false}')
            def call(session, command):
                return db.console_request(session, json.dumps({"version": 1, "requestId": "写入结果", "command": command}))
            revision = json.loads(call(session, {"operation": "getNodes", "ids": ["1"], "withRevision": True}))["data"]["revision"]
            command = {"operation": "guardedTransaction", "idempotencyKey": "response-outcome", "expectedRevision": revision,
                       "operations": [{"op": "updatePayload", "id": "1", "payload": {"state": "committed"}}]}
            ack = json.loads(call(session, command))["data"]
            small = triviumdb.ConsoleSession('{"readOnly":false,"maxResponseBytes":1}')
            expect_write(lambda: call(small, command), "TDB_CONSOLE_RESPONSE_TOO_LARGE", {"max_bytes": 1, "committed": True}, contract["response"])
            status = dict(command, operation="transactionStatus")
            assert json.loads(call(session, status))["data"] == {"outcome": "committed", "createdIds": [], "revision": ack["revision"]}
            tx = db.transaction()
            tx.insert_with_id(3, [1, 0], {"state": "closed"})
            db.close()
            expect_write(lambda: db.insert([1, 0], {}), "TDB_DATABASE_CLOSED", {}, contract["direct_admission"])
            expect_write(tx.commit, "TDB_DATABASE_CLOSED", {}, contract["admission"])
            assert tx.pending_count() == 1
            tx.rollback()
        finally:
            db.close()
        reopened = triviumdb.TriviumDB(file, dim=2, dtype=dtype, auto_build_quiver=False)
        try:
            assert reopened.all_node_ids() == [1, 2]
            for id, vector, state in ((1, [1, 0], "committed"), (2, [0, 1], "valid")):
                node = reopened.get(id)
                assert node.vector == vector and node.payload == {"state": state} and node.edges == []
        finally:
            reopened.close()
        readonly = triviumdb.TriviumDB(file, dim=2, dtype=dtype, access_mode="read_only", auto_build_quiver=False)
        try:
            expect_write(lambda: readonly.insert_with_id(3, [1, 0], {}), "TDB_READ_ONLY", {"operation": "insert_with_id"}, contract["direct_admission"])
            tx = readonly.transaction()
            tx.insert_with_id(3, [1, 0], {})
            expect_write(tx.commit, "TDB_READ_ONLY", {"operation": "transaction commit"}, contract["admission"])
            assert tx.pending_count() == 1
            tx.rollback()
            assert readonly.all_node_ids() == [1, 2]
        finally:
            readonly.close()
