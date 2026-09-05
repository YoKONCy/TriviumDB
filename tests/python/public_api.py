"""Python 公共 API 跨层契约测试。

覆盖 Prepared TQL、一等 Path/List、四类属性索引、存储诊断和历史入口迁移错误。
"""

import json
import os
import tempfile

import triviumdb


def run_shared_contract(root: str) -> None:
    contract_path = os.path.join(os.path.dirname(__file__), "..", "contracts", "public_cases.json")
    with open(contract_path, encoding="utf-8") as source:
        contract = json.load(source)
    assert contract["schema_version"] == 1
    path = os.path.join(root, "shared-contract.tdb")
    db = triviumdb.TriviumDB(path, dim=contract["setup"]["dimension"])
    for node in contract["setup"]["nodes"]:
        db.insert_with_id(node["id"], node["vector"], node["payload"])
    for edge in contract["setup"]["edges"]:
        db.link(edge["source"], edge["target"], edge["label"], edge["weight"])
    for case in contract["cases"]:
        expected = case["expected"]
        if case["operation"] == "get_payload":
            payload = db.get_payload(case["node_id"])
            assert payload[expected["field"]] == expected["value"], case["name"]
        elif case["operation"] == "prepared_tql":
            prepared = db.prepare_tql(case["tql"])
            rows = db.execute_prepared_tql(prepared, case.get("parameters", {}))
            assert len(rows) == expected["row_count"], case["name"]
            assert rows[0].row[expected["column"]] == expected["value"], case["name"]
        elif case["operation"] == "tql_path":
            rows = db.tql(case["tql"])
            assert rows[0].row["path"] == expected["path"], case["name"]
        elif case["operation"] == "prepared_missing_parameter":
            prepared = db.prepare_tql(case["tql"])
            try:
                db.execute_prepared_tql(prepared, case.get("parameters", {}))
            except RuntimeError as error:
                assert any(needle in str(error) for needle in expected["error_contains_any"])
            else:
                raise AssertionError(case["name"])
        else:
            raise AssertionError(f"未知共享契约操作: {case['operation']}")
    db.close()


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="triviumdb-python-public-api-") as root:
        run_shared_contract(root)
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
        assert storage["database_format_current"] == 9
        assert storage["property_index_format"] == 6
        assert storage["node_count"] == 2

        vector_prepared = db.prepare_tql(
            "SEARCH VECTOR [$x, $y] TOP 1 RETURN *"
        )
        assert vector_prepared.parameter_names() == ["x", "y"]
        vector_rows = db.execute_prepared_tql(vector_prepared, {"x": 1, "y": 0.0})
        assert vector_rows[0].row["_"].get("id") == 1

        prepared = db.prepare_tql(
            'FIND {kind: "a"} AS seed WITH seed RETURN seed, $bonus + 1 AS score'
        )
        assert prepared.parameter_names() == ["bonus"]
        prepared_rows = db.execute_prepared_tql(prepared, {"bonus": 4})
        assert len(prepared_rows) == 1
        assert prepared_rows[0].row["score"] == 5
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
