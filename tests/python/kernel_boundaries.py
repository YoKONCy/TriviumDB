"""通过已安装的原生绑定验证类型身份、完整解析和打开准入。"""
import json
from pathlib import Path
import triviumdb


def expect_error(action, code):
    try:
        action()
    except (triviumdb.TriviumDBError, triviumdb.CapacityError) as error:
        assert error.code == code
        return error
    raise AssertionError(f"必须返回结构化错误：{code}")


def run_kernel_boundaries(root):
    from system_boundaries import run_system_boundaries
    run_system_boundaries(root)
    graph_contract = json.loads((Path(__file__).parents[1] / "contracts/graph_query_boundaries.json").read_text(encoding="utf-8"))
    file = str(Path(root) / "graph-query.tdb")
    db = triviumdb.TriviumDB(file, dim=2, auto_build_quiver=False)
    for node in graph_contract["nodes"]:
        db.insert_with_id(node["id"], [1, 0], {"kind": node["kind"]})
    for source, target, label in graph_contract["edges"]:
        db.link(source, target, label, 1)
    for indexed in [False, True]:
        if indexed:
            db.create_index("kind")
        for case in graph_contract["counts"]:
            assert [row.row for row in db.tql(case["query"])] == [{"total": case["expected"]}]
    for invalid in [float("nan"), float("inf"), -float("inf")]:
        for action in [
            lambda: db.search([invalid, 0], top_k=1, expand_depth=0, min_score=-1),
            lambda: db.search_exact([invalid, 0], 1),
            lambda: db.search_graph_first([invalid, 0], [], 1),
            lambda: db.search_graph_first([invalid, 0], [1], 1),
        ]:
            error = expect_error(action, "TDB_INVALID_VECTOR")
            assert isinstance(error.details["reason"], str) and error.details["reason"]
    assert [hit.id for hit in db.search([1, 0], top_k=(1 << 64) - 1, expand_depth=0, min_score=-1)] == [1, 2, 3]
    db.close()
    reader = triviumdb.TriviumDB(file, dim=2, access_mode="read_only")
    for case in graph_contract["counts"]:
        assert [row.row for row in reader.tql(case["query"])] == [{"total": case["expected"]}]
    reader.close()
    retrieval = json.loads((Path(__file__).parents[1] / "contracts/retrieval_boundaries.json").read_text(encoding="utf-8"))
    for name in ["binary", "metric"]:
        test = retrieval[name]
        file = str(Path(root) / f"typed-retrieval-{name}.tdb")
        db = triviumdb.TriviumDB(file, dim=1, dtype="u64", auto_build_quiver=False)
        for index, vector in enumerate(test["vectors"]):
            db.insert_with_id(index + 1, vector, {"index": index})
        for result in [db.search([test["query"]], top_k=2, expand_depth=0, min_score=-1), db.search_exact([test["query"]], 2)]:
            assert [hit.id for hit in result] == test["expected_ids"]
            assert [hit.score for hit in result] == test["expected_scores"]
        db.set_auto_build_quiver(True)
        db.close()
        reader = triviumdb.TriviumDB(file, dim=1, dtype="u64")
        assert [hit.id for hit in reader.search([test["query"]], top_k=2, expand_depth=0, min_score=-1)] == test["expected_ids"]
        reader.close()
    for index, scale in enumerate(retrieval["float_scales"]):
        file = str(Path(root) / f"float-scale-{index}.tdb")
        db = triviumdb.TriviumDB(file, dim=2, auto_build_quiver=False)
        db.insert_with_id(1, [scale, 0], {"self": True})
        for result in [db.search_exact([scale, 0], 1), db.search([scale, 0], top_k=1, expand_depth=0, min_score=-1)]:
            assert [(hit.id, hit.score) for hit in result] == [(1, 1)]
        db.close()
    source_file = str(Path(root) / "migrate-source.tdb")
    target_file = str(Path(root) / "migrate-existing.tdb")
    source = triviumdb.TriviumDB(source_file, dim=1, auto_build_quiver=False)
    source.insert_with_id(1, [1], {"origin": "source"})
    target = triviumdb.TriviumDB(target_file, dim=2, auto_build_quiver=False)
    target.insert_with_id(2, [0, 1], {"origin": "target"})
    target.close()
    before = {p.name: p.read_bytes() for p in Path(root).glob("migrate-existing.tdb*")}
    error = expect_error(lambda: source.migrate(target_file, 2), "TDB_IO")
    assert error.details["io_kind"] == "AlreadyExists"
    assert {p.name: p.read_bytes() for p in Path(root).glob("migrate-existing.tdb*")} == before
    source.close()
    schema = json.loads((Path(__file__).parents[1] / "contracts/index_schema_boundaries.json").read_text(encoding="utf-8"))
    for unique in [False, True]:
        file = str(Path(root) / f"schema-fields-{unique}.tdb")
        db = triviumdb.TriviumDB(file, dim=2, auto_build_quiver=False)
        db.insert_with_id(1, [1, 0], schema["payload"])
        for fields in schema["definitions"]:
            if unique:
                db.create_unique_composite_index(fields)
            else:
                db.create_composite_index(fields)
        db.close()
        for access in ["read_only", "read_write"]:
            before = {p.name: p.read_bytes() for p in Path(root).glob(Path(file).name + "*")}
            reader = triviumdb.TriviumDB(file, dim=2, access_mode=access, auto_build_quiver=False)
            try:
                assert reader.get(1).payload == schema["payload"]
                assert sorted(item["fields"] for item in reader.index_info()) == sorted(schema["definitions"])
                for fields in schema["definitions"]:
                    assert reader.indexed_lookup({field: schema["payload"][field] for field in fields}, 10) == schema["expected_ids"]
            finally:
                reader.close()
            if access == "read_only":
                assert {p.name: p.read_bytes() for p in Path(root).glob(Path(file).name + "*")} == before
    contract = json.loads((Path(__file__).parents[1] / "contracts/kernel_boundaries.json").read_text(encoding="utf-8"))
    for dtype in contract["dtypes"]:
        file = str(Path(root) / f"kernel-{dtype}.tdb")
        db = triviumdb.TriviumDB(file, dim=2, dtype=dtype, auto_build_quiver=False)
        try:
            for node in contract["nodes"]:
                db.insert_with_id(node["id"], node["vector"], node["payload"])
            before = Path(file + ".wal").read_bytes()
            for query in contract["invalid_mutations"]:
                expect_error(lambda: db.tql_mut(query), "TDB_QUERY_PARSE")
                assert Path(file + ".wal").read_bytes() == before
                for node in contract["nodes"]:
                    assert db.get(node["id"]).payload == node["payload"]
            expect_error(lambda: db.tql(contract["invalid_read"]), "TDB_QUERY_PARSE")
            expect_error(lambda: db.tql("MATCH (n) WHERE " + "NOT " * 16384 + "n.x == 1 RETURN n"), "TDB_QUERY_PARSE")
        finally:
            db.close()
        for requested in contract["dtypes"]:
            if requested == dtype:
                continue
            error = expect_error(lambda: triviumdb.TriviumDB(file, dim=2, dtype=requested), "TDB_DTYPE_MISMATCH")
            assert dict(error.details) == {"stored": dtype, "requested": requested}
        reopened = triviumdb.TriviumDB(file, dim=2, dtype=dtype)
        try:
            for node in contract["nodes"]:
                actual = reopened.get(node["id"])
                assert actual.vector == node["vector"]
                assert actual.payload == node["payload"]
        finally:
            reopened.close()

    file = str(Path(root) / "kernel-load-budget.tdb")
    db = triviumdb.TriviumDB(file, dim=2, auto_build_quiver=False)
    db.insert_with_id(1, [1, 0], {"text": "x" * (2 * 1024 * 1024)})
    db.close()
    before = {p.name: p.read_bytes() for p in Path(root).glob("kernel-load-budget.tdb*")}
    error = expect_error(lambda: triviumdb.TriviumDB(file, dim=2, memory_limit_mb=1), "TDB_CAPACITY_RESERVATION_REJECTED")
    assert error.details["memory_limit"] == 1024 * 1024
    assert error.details["current_bytes"] + error.details["estimated_bytes"] > error.details["memory_limit"]
    assert {p.name: p.read_bytes() for p in Path(root).glob("kernel-load-budget.tdb*")} == before
