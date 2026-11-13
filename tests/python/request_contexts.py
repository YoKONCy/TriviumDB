"""实际安装 wheel 验证类型化请求隔离、事务消费和关闭重开。"""

import asyncio
from contextlib import ExitStack
import json
from pathlib import Path

import triviumdb


def run_request_contexts(root):
    fixture = json.loads((Path(__file__).parents[1] / "contracts/request_contexts.json").read_text(encoding="utf-8"))
    try:
        triviumdb.RequestContext("")
    except ValueError as error:
        assert error.code == "TDB_INVALID_INPUT"
        assert error.target == {"field": "request_id"}
    else:
        raise AssertionError("空请求 ID 必须拒绝")
    for dtype in ("f32", "f16", "u64"):
        file = Path(root) / f"typed-request-{dtype}.tdb"
        db = triviumdb.TriviumDB(str(file), dim=fixture["dimension"], dtype=dtype, auto_build_quiver=False)
        db.insert_with_id(fixture["id"], fixture["vector"], fixture["payload"])
        transaction = db.transaction()
        transaction.insert([0, 1], {"body": "后续提交"})
        context = triviumdb.RequestContext(fixture["requestId"])
        assert context.request_id == fixture["requestId"]
        before = Path(str(file) + ".wal").read_bytes()
        calls = []

        class Identifier:
            def __index__(self):
                calls.append("index")
                context.cancel()
                return fixture["id"]

        try:
            with context:
                db.update_payload(Identifier(), {"body": "取消正文"})
        except RuntimeError as error:
            assert error.code == fixture["cancelCode"]
            assert error.write == fixture["admission"]
        else:
            raise AssertionError("转换期间取消必须拒绝写入")
        assert calls == ["index"]
        assert Path(str(file) + ".wal").read_bytes() == before
        try:
            with context:
                transaction.commit()
        except RuntimeError as error:
            assert error.code == fixture["cancelCode"]
            assert error.write == fixture["transactionAdmission"]
        else:
            raise AssertionError("取消请求必须保留事务")
        assert transaction.pending_count() == 1
        transaction.rollback()
        assert transaction.pending_count() == 0

        async def isolated():
            ready = asyncio.Event()
            release = asyncio.Event()

            async def cancelled():
                with context:
                    ready.set()
                    await release.wait()
                    db.get(fixture["id"])

            task = asyncio.create_task(cancelled())
            await ready.wait()
            child = triviumdb.RequestContext("independent")
            with child:
                resumed = asyncio.get_running_loop().create_future()
                asyncio.get_running_loop().call_soon(resumed.set_result, None)
                await resumed
                assert db.get_payload(fixture["id"]) == fixture["payload"]
                with context:
                    try:
                        with child:
                            db.get(fixture["id"])
                    except RuntimeError as error:
                        assert error.code == fixture["cancelCode"]
                    else:
                        raise AssertionError("内层上下文必须保留外层取消")
            release.set()
            try:
                await task
            except RuntimeError as error:
                assert error.code == fixture["cancelCode"]
            else:
                raise AssertionError("任务局部请求应拒绝取消")

        asyncio.run(isolated())
        batch_context = triviumdb.RequestContext("batch-worker")
        hook_calls = []

        class CancelSecondQuery:
            def on_pre_search(self, query, _context):
                hook_calls.append(list(query))
                if len(hook_calls) == 2:
                    batch_context.cancel()
                return query

        db.set_hook(CancelSecondQuery())
        try:
            with batch_context:
                db.search_batch([fixture["vector"], fixture["vector"]], top_k=1, parallelism=1)
        except RuntimeError as error:
            assert error.code == fixture["cancelCode"]
        else:
            raise AssertionError("工作线程取消必须拒绝整批结果")
        assert hook_calls == [fixture["vector"], fixture["vector"]]
        db.clear_hook()
        score = fixture["binaryScore"] if dtype == "u64" else fixture["floatScore"]
        hits = db.search_batch([fixture["vector"], fixture["vector"]], top_k=1, parallelism=1)
        assert [[(hit.id, hit.score, hit.payload) for hit in rows] for rows in hits] == [
            [(fixture["id"], score, fixture["payload"])], [(fixture["id"], score, fixture["payload"])],
        ]
        nested = triviumdb.RequestContext("nested")
        with ExitStack() as stack:
            for _ in range(32):
                stack.enter_context(nested)
            assert db.search_batch([fixture["vector"]], top_k=1)[0][0].id == fixture["id"]
            try:
                nested.__enter__()
            except ValueError as error:
                assert error.code == "TDB_INVALID_INPUT"
                assert error.target == {"field": "request_context"}
            else:
                raise AssertionError("超出用户作用域上限必须拒绝")
        assert db.get_payload(fixture["id"]) == fixture["payload"]
        with context:
            db.close()
        renamed = Path(str(file) + ".saved")
        for entry in file.parent.iterdir():
            if entry.name == file.name or entry.name.startswith(file.name + "."):
                entry.rename(entry.with_name(renamed.name + entry.name[len(file.name):]))
        reopened = triviumdb.TriviumDB(str(renamed), dim=fixture["dimension"], dtype=dtype, access_mode="read_only")
        assert [row.row["n"]["id"] for row in reopened.tql("MATCH (n) RETURN n")] == [fixture["id"]]
        node = reopened.get(fixture["id"])
        assert node is not None
        assert node.id == fixture["id"]
        assert node.vector == fixture["vector"]
        assert node.payload == fixture["payload"]
        assert node.edges == []
        reopened.close()
