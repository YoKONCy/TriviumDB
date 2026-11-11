"""真实借用竞争与回调重入的请求隔离、事务保留及关闭契约。"""

import json
from pathlib import Path
import threading

import triviumdb


CONTRACT = json.loads((Path(__file__).parents[1] / "contracts/error_cases.json").read_text(encoding="utf-8"))
ADMISSION = CONTRACT["admission"]
ORIGINAL = {"body": "原持有者的数据"}
ADDED = {"body": "拒绝后再次提交"}


def expect_error(action, code, details):
    try:
        action()
    except triviumdb.TriviumDBError as error:
        assert error.schema_version == CONTRACT["schema_version"]
        assert error.code == code, (code, error.code)
        assert error.details == details, error.details
        assert error.message == str(error)
    else:
        raise AssertionError(f"请求必须返回 {code}")


def open_database(file, dtype):
    return triviumdb.TriviumDB(str(file), dim=2, dtype=dtype, auto_build_quiver=False)


def verify_contents(database):
    assert sorted(database.all_node_ids()) == [1, 2]
    for node_id, payload in ((1, ORIGINAL), (2, ADDED)):
        node = database.get(node_id)
        assert node.id == node_id
        assert node.vector == [1, 0]
        assert node.payload == payload
        assert node.edges == []


def run_reentry(root, dtype):
    file = root / f"reentry-{dtype}.tdb"
    db = open_database(file, dtype)
    other = open_database(root / f"other-{dtype}.tdb", dtype)
    session = triviumdb.ConsoleSession()
    callbacks = []
    try:
        db.insert_with_id(1, [1, 0], ORIGINAL)
        other.insert_with_id(1, [1, 0], ORIGINAL)
        tx = db.transaction()
        tx.insert_with_id(2, [1, 0], ADDED)

        class Hook:
            def on_pre_search(self, vector, context):
                actions = [
                    lambda: db.get_payload(1), lambda: db.dtype, lambda: len(db),
                    db.close, db.request_shutdown, lambda: db.close_timeout(0),
                    db.transaction, tx.commit, lambda: tx.__exit__(None, None, None),
                    lambda: db.console_request(session, '{"version":1,"requestId":"inner","command":{"operation":"info"}}'),
                ]
                for action in actions:
                    expect_error(action, ADMISSION["reentrant_code"], {"operation": ADMISSION["python_operation"]})
                assert tx.pending_count() == 1
                other.update_payload(1, {"body": "另一个实例正常写入"})
                callbacks.append("完成")
                return vector

        db.set_hook(Hook())
        hits = db.search([1, 0], top_k=1)
        assert [(hit.id, hit.payload) for hit in hits] == [(1, ORIGINAL)]
        result = json.loads(db.console_request(session, '{"version":1,"requestId":"outer","command":{"operation":"search","vector":[1,0],"topK":1}}'))
        assert [(hit["id"], hit["payload"]) for hit in result["data"]["hits"]] == [("1", ORIGINAL)]
        batches = db.search_batch([[1, 0], [1, 0]], top_k=1, parallelism=2)
        assert [[(hit.id, hit.payload) for hit in hits] for hits in batches] == [[(1, ORIGINAL)], [(1, ORIGINAL)]]
        assert callbacks == ["完成"] * 4
        assert other.get_payload(1) == {"body": "另一个实例正常写入"}
        db.clear_hook()
        assert tx.commit() == [2]
        verify_contents(db)
    finally:
        db.close()
        other.close()
    reopened = open_database(file, dtype)
    try:
        verify_contents(reopened)
    finally:
        reopened.close()


def run_contention(root, dtype, exclusive):
    file = root / f"contention-{dtype}-{exclusive}.tdb"
    db = open_database(file, dtype)
    session = triviumdb.ConsoleSession()
    entered, release = threading.Event(), threading.Event()
    failures, results = [], []
    worker = None
    try:
        db.insert_with_id(1, [1, 0], ORIGINAL)
        tx = db.transaction()
        tx.insert_with_id(2, [1, 0], ADDED)

        class Hook:
            def on_pre_search(self, vector, context):
                entered.set()
                # 事件决定交错；超时只防止故障挂住验收进程。
                assert release.wait(30), "必须释放真实查询"
                return vector

        def query():
            try:
                if exclusive:
                    value = json.loads(db.console_request(session, '{"version":1,"requestId":"held","command":{"operation":"search","vector":[1,0],"topK":1}}'))
                    results.append([(int(hit["id"]), hit["payload"]) for hit in value["data"]["hits"]])
                else:
                    results.append([(hit.id, hit.payload) for hit in db.search([1, 0], top_k=1)])
            except BaseException as error:
                failures.append(error)

        db.set_hook(Hook())
        worker = threading.Thread(target=query)
        worker.start()
        try:
            assert entered.wait(30), "查询必须实际进入 Hook"
            actions = [db.close, db.request_shutdown, lambda: db.close_timeout(0), tx.commit,
                       lambda: tx.__exit__(None, None, None),
                       lambda: db.update_payload(1, {"body": "应被拒绝"})]
            if exclusive:
                actions.extend([lambda: db.get_payload(1), lambda: db.dtype, db.transaction])
            else:
                assert db.get_payload(1) == ORIGINAL
                assert db.dtype == dtype
            for action in actions:
                expect_error(action, ADMISSION["busy_code"], {"resource": ADMISSION["python_resource"]})
            assert tx.pending_count() == 1
        finally:
            release.set()
            worker.join(30)
        assert not worker.is_alive(), "已放行查询必须结束"
        assert failures == [], failures
        assert results == [[(1, ORIGINAL)]]
        db.clear_hook()
        assert db.shutdown_status()["phase"] == "open"
        assert tx.commit() == [2]
        verify_contents(db)
    finally:
        release.set()
        if worker is not None:
            worker.join(30)
        db.close()
    reopened = open_database(file, dtype)
    try:
        verify_contents(reopened)
    finally:
        reopened.close()


def run_closed_transaction(root, dtype):
    file = root / f"closed-{dtype}.tdb"
    db = open_database(file, dtype)
    try:
        db.insert_with_id(1, [1, 0], ORIGINAL)
        tx = db.transaction()
        tx.insert_with_id(2, [1, 0], ADDED)
        db.close()
        expect_error(db.transaction, ADMISSION["closed_code"], {})
        expect_error(tx.commit, ADMISSION["closed_code"], {})
        expect_error(lambda: tx.__exit__(None, None, None), ADMISSION["closed_code"], {})
        assert tx.pending_count() == 1
        tx.rollback()
        assert tx.pending_count() == 0
    finally:
        db.close()
    reopened = open_database(file, dtype)
    try:
        assert reopened.all_node_ids() == [1]
        node = reopened.get(1)
        assert node.vector == [1, 0] and node.payload == ORIGINAL and node.edges == []
    finally:
        reopened.close()


def run_request_admission_contract(root):
    directory = Path(root) / "request-admission"
    directory.mkdir()
    for dtype in ("f32", "f16", "u64"):
        run_reentry(directory, dtype)
        run_contention(directory, dtype, False)
        run_contention(directory, dtype, True)
        run_closed_transaction(directory, dtype)
