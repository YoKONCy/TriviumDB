"""实际 wheel 的查询实参、转换重入与拒绝后完整事实。"""

from pathlib import Path
from collections import UserDict
from types import MappingProxyType
from functools import partial
import math
import triviumdb

READ_QUERY = "MATCH (n) RETURN $x AS value"
WRITE_QUERY = "MATCH (n) SET VECTOR(n) == [0, 1]"
PAYLOAD = {"saved": "完整事实"}


def check_hits(hits, score):
    assert [(hit.id, hit.score, hit.payload) for hit in hits] == [(7, score, PAYLOAD)]


def check_grouped(result, score):
    check_hits(result.semantic_hits, score)
    assert result.graph_hits == []


def check_batch(result, score):
    assert len(result) == 1
    check_hits(result[0], score)


def check_context(result, score):
    check_hits(result[0], score)


def check_hybrid(result, _score):
    assert [(hit.id, hit.payload) for hit in result] == [(7, PAYLOAD)]
    assert all(math.isfinite(hit.score) for hit in result)


def check_input_error(error, field, write=None):
    assert error.schema_version == 1
    assert error.code == "TDB_INVALID_INPUT"
    assert error.target == {"field": field}
    assert error.stage is None
    assert error.write == write
    assert list(error.details) == ["reason"]
    assert isinstance(error.details["reason"], str) and error.details["reason"]


def run_query_arguments(root):
    for dtype in ("f32", "f16", "u64"):
        # 相同浮点向量的余弦为 1；两段 u64 的匹配位数为 128。
        score = 128 if dtype == "u64" else 1
        directory = Path(root) / f"query-arguments-{dtype}"
        directory.mkdir()
        file = str(directory / "database.tdb")
        options = dict(dim=2, dtype=dtype, auto_build_quiver=False)
        db = triviumdb.TriviumDB(file, **options)
        try:
            db.insert_with_id(7, [1, 0], PAYLOAD)
            before = Path(file + ".wal").read_bytes()
            for method, fields, vector, check in (
                (db.search, ("top_k", "recall_k", "rerank_k", "expand_depth", "max_edges_per_node"), [1, 0], check_hits),
                (db.search_grouped, ("top_k", "recall_k", "rerank_k", "expand_depth", "max_edges_per_node"), [1, 0], check_grouped),
                (db.search_batch, ("top_k", "recall_k", "rerank_k", "expand_depth", "parallelism"), [[1, 0]], check_batch),
                (db.search_exact, ("top_k",), [1, 0], check_hits),
                (partial(db.search_graph_first, anchor_ids=[7], top_k=1), ("top_k", "max_anchor_nodes"), [1, 0], check_hits),
                (partial(db.search_with_context, expand_depth=0), ("top_k", "recall_k", "rerank_k", "expand_depth"), [1, 0], check_context),
                (partial(db.search_advanced, expand_depth=0, enable_advanced_pipeline=False), ("top_k", "recall_k", "rerank_k", "expand_depth", "max_edges_per_node"), [1, 0], check_hits),
                (partial(db.search_hybrid, query_text="", expand_depth=0), ("top_k", "expand_depth"), [1, 0], check_hybrid),
            ):
                for field in fields:
                    # 深度为零隔离图扩散，数量和并行度使用一个有效项。
                    valid = 0 if field == "expand_depth" else 1
                    for value, exception in ((None, TypeError), ("1", TypeError), (1.5, TypeError),
                                             ([], TypeError), ({}, TypeError), (-1, OverflowError),
                                             (1 << 100, OverflowError)):
                        try:
                            method(vector, **{field: value})
                        except exception as error:
                            check_input_error(error, field)
                        else:
                            raise AssertionError(f"必须拒绝查询参数 {field}={value!r}")
                    callbacks = []
                    class Index:
                        def __index__(self):
                            try:
                                db.tql_mut(WRITE_QUERY)
                            except triviumdb.TriviumDBError as error:
                                assert error.code == "TDB_REENTRANT_ACCESS"
                                assert error.write == {"phase": "admission", "outcome": "not_committed"}
                            else:
                                raise AssertionError("整数转换中的同库写入必须被拒绝")
                            callbacks.append(True)
                            return valid
                    check(method(vector, **{field: Index()}), score)
                    assert callbacks == [True]
                    cause = RuntimeError("宿主整数协议原因")
                    sentinel = TypeError("宿主整数协议失败")
                    sentinel.__cause__ = cause
                    class BrokenIndex:
                        def __index__(self):
                            raise sentinel
                    try:
                        method(vector, **{field: BrokenIndex()})
                    except TypeError as error:
                        assert error is sentinel and error.__cause__ is cause
                        assert not hasattr(error, "code")
                    else:
                        raise AssertionError("必须保留宿主整数异常")
                    check(method(vector, **{field: valid}), score)
            anchor_callbacks = []
            class AnchorId:
                def __index__(self):
                    try:
                        db.tql_mut(WRITE_QUERY)
                    except triviumdb.TriviumDBError as error:
                        assert error.code == "TDB_REENTRANT_ACCESS"
                        assert error.write == {"phase": "admission", "outcome": "not_committed"}
                    else:
                        raise AssertionError("锚点转换中的写入必须被拒绝")
                    anchor_callbacks.append(True)
                    return 7
            class Anchors:
                def __len__(self):
                    return 1
                def __getitem__(self, index):
                    if index == 0:
                        return AnchorId()
                    raise IndexError
            check_hits(db.search_graph_first([1, 0], Anchors(), 1), score)
            assert anchor_callbacks == [True]
            cause = RuntimeError("锚点协议原因")
            sentinel = TypeError("锚点协议失败")
            sentinel.__cause__ = cause
            class BrokenAnchors(Anchors):
                def __getitem__(self, index):
                    raise sentinel
            try:
                db.search_graph_first([1, 0], BrokenAnchors(), 1)
            except TypeError as error:
                assert error is sentinel and error.__cause__ is cause
                assert not hasattr(error, "code")
            else:
                raise AssertionError("必须保留锚点序列原异常")
            for method, field, write in ((db.tql, "query", None), (db.prepare_tql, "query", None),
                                        (db.tql_mut, "query", {"phase": "validation", "outcome": "not_committed"}),
                                        (partial(db.search_hybrid, [1, 0]), "query_text", None)):
                for value, exception in ((None, TypeError), (1, TypeError), (True, TypeError),
                                         ([], TypeError), ({}, TypeError), (b"MATCH", TypeError),
                                         ("\ud800", UnicodeEncodeError)):
                    try:
                        method(value)
                    except exception as error:
                        check_input_error(error, field, write)
                    else:
                        raise AssertionError("查询文本必须是可编码字符串")
            check_hits(db.search([1, 0]), score)
            grouped = db.search_grouped([1, 0])
            check_hits(grouped.semantic_hits, score)
            assert grouped.graph_hits == []
            batches = db.search_batch([[1, 0], [1, 0]])
            assert len(batches) == 2
            for hits in batches:
                check_hits(hits, score)
            class Text(str):
                def __str__(self):
                    raise AssertionError("字符串子类应读取原始字符")
            assert [row.row for row in db.tql(Text("MATCH (n) RETURN 42 AS value"))] == [{"value": 42}]
            prepared = db.prepare_tql(Text(READ_QUERY))
            assert [row.row for row in db.execute_prepared_tql(prepared, {"x": 42})] == [{"value": 42}]
            for action in (
                lambda value: db.search([1, 0], payload_filter=value),
                lambda value: db.search_grouped([1, 0], payload_filter=value).semantic_hits,
                lambda value: db.search_with_context([1, 0], expand_depth=0, payload_filter=value)[0],
                lambda value: db.search_advanced([1, 0], expand_depth=0, enable_advanced_pipeline=False, payload_filter=value),
                lambda value: db.search_hybrid([1, 0], "", expand_depth=0, payload_filter=value),
            ):
                for value in (MappingProxyType({"saved": "完整事实"}), UserDict({"saved": "完整事实"})):
                    assert [(hit.id, hit.payload) for hit in action(value)] == [(7, PAYLOAD)]
                assert action(UserDict({"saved": "其他内容"})) == []
                for value in ([], 1, b"filter"):
                    try:
                        action(value)
                    except TypeError as error:
                        check_input_error(error, "payload_filter")
                    else:
                        raise AssertionError("过滤器必须遵守 Mapping 契约")
                try:
                    action({"saved": {"$gt": True}})
                except triviumdb.InvalidInputError as error:
                    check_input_error(error, "payload_filter")
                else:
                    raise AssertionError("过滤器数值条件必须拒绝布尔值")
                callbacks = []
                class CallbackMapping(UserDict):
                    def keys(self):
                        try:
                            db.tql_mut(WRITE_QUERY)
                        except triviumdb.TriviumDBError as error:
                            assert error.code == "TDB_REENTRANT_ACCESS"
                            assert error.write == {"phase": "admission", "outcome": "not_committed"}
                        else:
                            raise AssertionError("过滤器转换中的重入必须被拒绝")
                        callbacks.append(True)
                        return super().keys()
                assert [(hit.id, hit.payload) for hit in action(CallbackMapping(saved="完整事实"))] == [(7, PAYLOAD)]
                assert callbacks == [True]
                cause = RuntimeError("过滤器映射原因")
                sentinel = TypeError("过滤器映射失败")
                sentinel.__cause__ = cause
                class BrokenMapping(UserDict):
                    def keys(self):
                        raise sentinel
                try:
                    action(BrokenMapping(saved="完整事实"))
                except TypeError as error:
                    assert error is sentinel and error.__cause__ is cause
                    assert not hasattr(error, "code")
                else:
                    raise AssertionError("必须保留过滤器宿主原异常")
            assert Path(file + ".wal").read_bytes() == before
            assert db.tql_mut(Text(WRITE_QUERY)) == {"affected": 1, "created_ids": []}
        finally:
            db.close()
        moved = directory.with_name(directory.name + "-moved")
        assert directory.is_absolute() and moved.parent == directory.parent
        directory.rename(moved)
        try:
            db.node_count()
        except triviumdb.TriviumDBError as error:
            assert error.code == "TDB_DATABASE_CLOSED"
        else:
            raise AssertionError("持有的旧实例必须保持关闭状态")
        file = str(moved / "database.tdb")
        reopened = triviumdb.TriviumDB(file, **options)
        try:
            assert reopened.all_node_ids() == [7]
            node = reopened.get(7)
            assert node.vector == [0, 1] and node.payload == PAYLOAD and node.edges == []
        finally:
            reopened.close()
