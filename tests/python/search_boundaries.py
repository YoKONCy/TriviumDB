"""检索预过滤与 Hook 向量转换的安装制品契约。"""

import json
import math
from pathlib import Path

import triviumdb


def verify_search_entries(root):
    fixture = json.loads((Path(__file__).parents[1] / "contracts/search_entry_cases.json").read_text(encoding="utf-8"))
    records = {record["id"]: record for record in fixture["records"]}
    query = fixture["query"]

    def verify(hits, case):
        assert [hit.id for hit in hits] == case["ids"], case
        for hit in hits:
            assert hit.payload == records[hit.id]["payload"]
            assert math.isclose(hit.score, records[hit.id]["score"], abs_tol=1e-6), (case, hit.id, hit.score)

    for dtype in ("f32", "f16"):
        file = str(Path(root) / f"search-entry-{dtype}.tdb")
        db = triviumdb.TriviumDB(file, dim=2, dtype=dtype)
        try:
            for record in records.values():
                db.insert_with_id(record["id"], record["vector"], record["payload"])
            for reopened in (False, True):
                if reopened:
                    db.close()
                    db = triviumdb.TriviumDB(file, dim=2, dtype=dtype)
                for case in fixture["cases"]:
                    options = dict(top_k=case.get("top_k", len(records)), expand_depth=0, exact=case["exact"])
                    if "min_score" in case:
                        options["min_score"] = case["min_score"]
                    filtered = dict(options, payload_filter=case.get("filter"))
                    verify(db.search(query, **filtered), case)
                    hits, context = db.search_with_context(query, **filtered)
                    verify(hits, case)
                    assert context.observations["vector_route_quiver"] == 0
                    verify(db.search_grouped(query, **filtered).semantic_hits, case)
                    verify(db.search_advanced(query, enable_advanced_pipeline=False, **filtered), case)
                    verify(db.search_hybrid(query, "", **filtered), case)
                    if "filter" not in case:
                        batch = db.search_batch([query, query], parallelism=2, **options)
                        assert len(batch) == 2
                        for hits in batch:
                            verify(hits, case)
        finally:
            db.close()

    # 超过自动建图阈值，使用公开上下文确认精确请求跳过 ANN 导航。
    db = triviumdb.TriviumDB(str(Path(root) / "search-exact-route.tdb"), dim=2)
    try:
        for node_id in range(900, 0, -1):
            db.insert_with_id(node_id, [1, 0], {"id": node_id})
        for indexed in (False, True):
            if indexed:
                db.search([1, 0], top_k=1)
                assert db.search_with_context([1, 0], top_k=1)[1].observations["vector_route_quiver"] == 1
            hits, context = db.search_with_context([1, 0], top_k=3, expand_depth=0, exact=True)
            assert context.observations["vector_route_quiver"] == 0
            assert [(hit.id, hit.score, hit.payload) for hit in hits] == [(i, 1.0, {"id": i}) for i in (1, 2, 3)]
            assert [hit.id for hit in db.search([1, 0], top_k=3, exact=True)] == [1, 2, 3]
        # 兼容参数与新参数各自能够开启精确召回。
        for exact, legacy in ((True, False), (False, True)):
            hits = db.search_advanced([1, 0], top_k=3, expand_depth=0,
                                      enable_advanced_pipeline=False, exact=exact, force_brute_force=legacy)
            assert [(hit.id, hit.score) for hit in hits] == [(1, 1.0), (2, 1.0), (3, 1.0)]
    finally:
        db.close()


def verify_filters(db, fixture):
    for case in fixture["cases"]:
        hits = db.search_advanced(
            [1, 0], top_k=len(fixture["records"]), recall_k=len(fixture["records"]),
            rerank_k=len(fixture["records"]), expand_depth=0, min_score=-1,
            enable_advanced_pipeline=False, force_brute_force=True, payload_filter=case["filter"],
        )
        assert [hit.id for hit in hits] == case["ids"], case
        for hit in hits:
            record = next(record for record in fixture["records"] if record["id"] == hit.id)
            assert hit.payload == record["payload"]
            assert hit.score == 1


def verify_hybrid_weights(root):
    db = triviumdb.TriviumDB(str(Path(root) / "hybrid-weights.tdb"), dim=2, auto_build_quiver=False)
    try:
        db.insert_with_id(1, [1, 0], {"name": "z"})
        db.insert_with_id(2, [0.8, 0.6], {"name": "a"})
        db.index_text(2, "rareword")
        db.build_text_index()

        def search(alpha):
            return db.search_hybrid([1, 0], "rareword", top_k=2, expand_depth=0, hybrid_alpha=alpha, exact=True)

        expected = [(hit.id, hit.score) for hit in db.search_exact([1, 0], 2)]
        assert [(hit.id, hit.score) for hit in search(1)] == expected
        assert [hit.id for hit in search(0)] == [2]
        assert [hit.id for hit in search(0.2)] == [2, 1]
        for alpha in [-1, 2, 1.00000001, float("nan"), float("inf")]:
            try:
                search(alpha)
            except triviumdb.InvalidInputError as exc:
                assert exc.code == "TDB_INVALID_INPUT"
            else:
                raise AssertionError(f"混合权重超出范围：{alpha}")
    finally:
        db.close()


def run_search_boundaries(root):
    verify_hybrid_weights(root)
    verify_search_entries(root)
    fixture = json.loads((Path(__file__).parents[1] / "contracts/search_filter_cases.json").read_text(encoding="utf-8"))
    file = str(Path(root) / "search-filter.tdb")
    db = triviumdb.TriviumDB(file, dim=2, auto_build_quiver=False)
    try:
        for record in fixture["records"]:
            db.insert_with_id(record["id"], [1, 0], record["payload"])
        verify_filters(db, fixture)
        db.close()
        db = triviumdb.TriviumDB(file, dim=2, auto_build_quiver=False)
        verify_filters(db, fixture)
        for record in fixture["records"]:
            db.update_payload(record["id"], record["payload"])
        verify_filters(db, fixture)
    finally:
        db.close()

    class Rewrite:
        def __init__(self, value):
            self.value = value

        def on_pre_search(self, query, ctx):
            return self.value

    half = triviumdb.TriviumDB(str(Path(root) / "hook-f16.tdb"), dim=2, dtype="f16", auto_build_quiver=False)
    try:
        half.insert_with_id(1, [1, 0], {"value": "kept"})
        half.set_hook(Rewrite([70000, 0]))
        try:
            half.search([1, 0], top_k=1, expand_depth=0, min_score=-1)
        except triviumdb.TriviumDBError as error:
            assert error.code == "TDB_INVALID_VECTOR"
        else:
            raise AssertionError("f16 转换溢出必须拒绝")
        half.set_hook(Rewrite([60000, 0]))
        hits = half.search([1, 0], top_k=1, expand_depth=0, min_score=-1)
        assert [hit.id for hit in hits] == [1]
        assert hits[0].payload == {"value": "kept"}
        assert hits[0].score == 1
    finally:
        half.close()
