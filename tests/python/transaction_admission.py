"""事务对象的真实转换回调、线程竞争、结束状态与 GC 所有权契约。"""

import gc
from pathlib import Path
import threading
import weakref

import triviumdb
from request_admission import ADMISSION, expect_error, open_database


def writes(tx):
    return [
        lambda: tx.insert([1, 0], {}), lambda: tx.insert_with_id(99, [1, 0], {}),
        lambda: tx.link(1, 2), lambda: tx.delete(1), lambda: tx.unlink(1, 2),
        lambda: tx.update_payload(1, {}), lambda: tx.update_vector(1, [0, 1]),
        tx.commit, tx.rollback, tx.__enter__,
    ]


def all_requests(tx):
    return writes(tx) + [tx.pending_count, lambda: repr(tx), lambda: tx.__exit__(None, None, None)]


def seed(file, dtype):
    db = open_database(file, dtype)
    db.insert_with_id(1, [1, 0], {"state": "saved"})
    return db


def verify(db, expected):
    assert sorted(db.all_node_ids()) == sorted(expected)
    for node_id, state in expected.items():
        node = db.get(node_id)
        assert node.id == node_id
        assert node.vector == [1, 0]
        assert node.payload == {"state": state}
        assert node.edges == []


def reopened(file, dtype, expected):
    db = open_database(file, dtype)
    try:
        verify(db, expected)
    finally:
        db.close()


def reentry(root, dtype):
    file = root / f"reentry-{dtype}.tdb"
    db = seed(file, dtype)
    try:
        tx, other = db.transaction(), db.transaction()
        tx.insert_with_id(2, [1, 0], {"state": "prepared"})
        calls = []

        def probe():
            for action in all_requests(tx):
                expect_error(action, ADMISSION["reentrant_code"], {
                    "operation": ADMISSION["python_transaction_operation"],
                })
            other.insert_with_id(4, [1, 0], {"state": "other"})
            assert other.pending_count() == 1
            assert db.get_payload(1) == {"state": "saved"}
            calls.append("converted")

        class Scalar:
            def __float__(self):
                probe()
                return 1.0

            def __index__(self):
                probe()
                return 1

        tx.insert_with_id(3, [Scalar(), 0], {"state": "outer"})
        assert calls == ["converted"]
        assert tx.pending_count() == 2
        assert tx.commit() == [2, 3]
        assert other.commit() == [4]
        verify(db, {1: "saved", 2: "prepared", 3: "outer", 4: "other"})
    finally:
        db.close()
    reopened(file, dtype, {1: "saved", 2: "prepared", 3: "outer", 4: "other"})


def contention(root, dtype):
    file = root / f"contention-{dtype}.tdb"
    db = seed(file, dtype)
    entered, release = threading.Event(), threading.Event()
    worker = None
    failures = []
    try:
        tx = db.transaction()
        tx.insert_with_id(2, [1, 0], {"state": "prepared"})

        def block():
            entered.set()
            # 事件控制交错；期限只作为阻止故障挂住进程的看门狗。
            assert release.wait(30), "必须放行转换线程"

        class Scalar:
            def __float__(self):
                block()
                return 1.0

            def __index__(self):
                block()
                return 1

        def append():
            try:
                tx.insert_with_id(3, [Scalar(), 0], {"state": "outer"})
            except BaseException as error:
                failures.append(error)

        worker = threading.Thread(target=append)
        worker.start()
        try:
            assert entered.wait(30), "转换必须真实进入回调"
            for action in all_requests(tx):
                expect_error(action, ADMISSION["busy_code"], {
                    "resource": ADMISSION["python_transaction_resource"],
                })
            with db.transaction() as other:
                other.insert_with_id(4, [1, 0], {"state": "other"})
            assert db.get_payload(4) == {"state": "other"}
        finally:
            release.set()
            worker.join(30)
        assert not worker.is_alive(), "放行后的转换必须完成"
        assert failures == [], failures
        assert tx.pending_count() == 2
        assert tx.commit() == [2, 3]
    finally:
        release.set()
        if worker is not None:
            worker.join(30)
        db.close()
    reopened(file, dtype, {1: "saved", 2: "prepared", 3: "outer", 4: "other"})


def conversion_failure(root, dtype):
    file = root / f"conversion-{dtype}.tdb"
    db = seed(file, dtype)
    try:
        tx = db.transaction()
        tx.insert_with_id(2, [1, 0], {"state": "prepared"})

        class ConversionFailure(Exception):
            pass

        original = ConversionFailure("宿主转换失败")

        class Scalar:
            def __float__(self):
                raise original

            def __index__(self):
                raise original

        try:
            tx.insert_with_id(3, [Scalar(), 0], {"state": "rejected"})
        except ConversionFailure as error:
            assert error is original
        else:
            raise AssertionError("宿主转换异常必须保留")
        assert tx.pending_count() == 1
        tx.insert_with_id(3, [1, 0], {"state": "retry"})
        assert tx.commit() == [2, 3]
    finally:
        db.close()
    reopened(file, dtype, {1: "saved", 2: "prepared", 3: "retry"})


def finished(root, dtype, outcome):
    file = root / f"finished-{dtype}-{outcome}.tdb"
    db = seed(file, dtype)
    expected = {1: "saved", 4: "new transaction"}
    try:
        tx = db.transaction()
        tx.insert_with_id(2, [1] if outcome == "failed" else [1, 0], {"state": "committed"})
        if outcome == "committed":
            assert tx.commit() == [2]
            expected[2] = "committed"
        elif outcome == "rolled_back":
            tx.rollback()
        elif outcome == "context_exception":
            original = ValueError("宿主业务异常")
            try:
                with tx:
                    raise original
            except ValueError as error:
                assert error is original
            else:
                raise AssertionError("上下文退出必须保留宿主异常")
        else:
            expect_error(tx.commit, "TDB_DIMENSION_MISMATCH", {"expected": 2, "got": 1})
        assert tx.pending_count() == 0
        for action in writes(tx):
            expect_error(action, ADMISSION["transaction_finished_code"], {})
        assert tx.__exit__(None, None, None) is False
        assert tx.__exit__(ValueError, ValueError("原异常"), None) is False
        with db.transaction() as other:
            other.insert_with_id(4, [1, 0], {"state": "new transaction"})
        verify(db, expected)
    finally:
        db.close()
    reopened(file, dtype, expected)


def gc_cycle(root, dtype):
    directory = root / f"gc-{dtype}"
    directory.mkdir()
    file = directory / "database.tdb"
    db = seed(file, dtype)
    tx = db.transaction()
    tx.insert_with_id(2, [1, 0], {"state": "uncommitted"})

    class Hook:
        pass

    hook = Hook()
    hook.transaction = tx
    reference = weakref.ref(hook)
    db.set_hook(hook)
    db.flush()
    del hook, db
    gc.collect()
    assert reference() is not None, "仍被调用方持有的事务必须保留数据库所有权"
    assert tx.pending_count() == 1
    try:
        open_database(file, dtype)
    except triviumdb.TriviumDBError as error:
        assert error.code == "TDB_DATABASE_LOCKED"
        assert isinstance(error.details["reason"], str) and error.details["reason"]
    else:
        raise AssertionError("活动事务仍持有数据库文件租约")
    del tx
    gc.collect()
    assert reference() is None, "Transaction → Database → Hook 引用环必须回收"
    moved = root / f"gc-moved-{dtype}"
    directory.rename(moved)
    reopened(moved / "database.tdb", dtype, {1: "saved"})


def run_transaction_admission_contract(root):
    directory = Path(root) / "transaction-admission"
    directory.mkdir()
    for dtype in ("f32", "f16", "u64"):
        reentry(directory, dtype)
        contention(directory, dtype)
        conversion_failure(directory, dtype)
        for outcome in ("committed", "rolled_back", "failed", "context_exception"):
            finished(directory, dtype, outcome)
        gc_cycle(directory, dtype)
