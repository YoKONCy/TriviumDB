"""检索预过滤与 Hook 向量转换的安装制品契约。"""

import json
from pathlib import Path

import triviumdb


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


def run_search_boundaries(root):
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
