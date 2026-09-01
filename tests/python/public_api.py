"""Python 公共 API 跨层契约测试。

覆盖 Prepared TQL、一等 Path/List、四类属性索引、存储诊断和历史入口迁移错误。
"""

import os
import tempfile

import triviumdb


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="triviumdb-python-public-api-") as root:
        path = os.path.join(root, "api.tdb")
        db = triviumdb.TriviumDB(path, dim=2)
        db.insert_with_id(1, [1.0, 0.0], {"kind": "a", "score": 1, "tenant": "t", "region": "r"})
        db.insert_with_id(2, [0.9, 0.1], {"kind": "b", "score": 2, "tenant": "t", "region": "r"})
        db.link(1, 2, "next", 1.0)

        stages: list[str] = []

        class FullHook:
            def on_pre_search(self, query, ctx):
                stages.append("pre")
                return query

            def on_custom_recall(self, query, ctx):
                stages.append("custom")
                return None

            def on_post_recall(self, hits, ctx):
                stages.append("post_recall")
                return hits

            def on_pre_graph_expand(self, hits, ctx):
                stages.append("pre_graph")
                return hits

            def on_rerank(self, hits, ctx):
                stages.append("rerank")
                return hits

            def on_post_search(self, hits, ctx):
                stages.append("post")
                return hits

        db.set_hook(FullHook())
        db.search_with_context([1.0, 0.0], top_k=2, expand_depth=1, min_score=0.0)
        assert {"pre", "custom", "post_recall", "pre_graph", "rerank", "post"} <= set(stages)
        db.clear_hook()

        class BrokenHook:
            def on_pre_search(self, query, ctx):
                raise RuntimeError("python-hook-failure")

        db.set_hook(BrokenHook())
        try:
            db.search_with_context([1.0, 0.0], top_k=2)
            raise AssertionError("Hook 异常必须传播")
        except RuntimeError as error:
            assert "python-hook-failure" in str(error)
        db.clear_hook()

        db.create_index("kind")
        db.create_ordered_index("score")
        db.create_composite_index(["tenant", "region"])
        db.create_bitmap_index("region")
        indexes = db.index_info()
        assert len(indexes) == 4
        assert next(item for item in indexes if item["kind"] == "composite")["fields"] == [
            "tenant",
            "region",
        ]

        storage = db.storage_info()
        assert storage["database_format_current"] == 7
        assert storage["property_index_format"] == 4
        assert storage["node_count"] == 2

        prepared = db.prepare_tql('FIND {kind: "a"} RETURN $bonus + 1 AS score')
        assert prepared.parameter_names() == ["bonus"]
        assert len(db.execute_prepared_tql(prepared, {"bonus": 4})) == 1
        try:
            db.execute_prepared_tql(prepared, {})
        except RuntimeError as error:
            assert "缺少参数" in str(error) or "missing parameter" in str(error)
        else:
            raise AssertionError("缺少 Prepared TQL 参数必须失败")

        rows = db.tql(
            "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed shortest_paths seed TO [2] "
            "LABEL next AS route WITH route RETURN path(route) AS path"
        )
        assert rows[0].row["path"] == [1, 2]

        try:
            db.tql_mut('FIND {kind: "a"} RETURN *')
        except RuntimeError as error:
            assert "API 已移除" in str(error) or "API removed" in str(error)
        else:
            raise AssertionError("tql_mut 读查询必须失败")
        db.close()


if __name__ == "__main__":
    main()
