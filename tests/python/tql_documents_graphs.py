"""实际 wheel 的文档来源、命名图身份与结构化 TQL 契约。"""
import json
from pathlib import Path

import triviumdb


def check(actual, expected):
    if isinstance(expected, float):
        assert isinstance(actual, (int, float)) and abs(actual - expected) < 1e-6, (actual, expected)
    elif isinstance(expected, list):
        assert len(actual) == len(expected), (actual, expected)
        for value, reference in zip(actual, expected):
            check(value, reference)
    elif isinstance(expected, dict):
        assert actual.keys() == expected.keys(), (actual, expected)
        for key in expected:
            check(actual[key], expected[key])
    else:
        assert actual == expected, (actual, expected)


def run_tql_documents_graphs(root: str) -> None:
    fixture = json.loads((Path(__file__).parents[1] / "contracts/tql_documents_graphs_cases.json").read_text(encoding="utf-8"))

    def rows(result):
        converted = []
        for result_row in result:
            row = dict(result_row.row)
            for key in ("id", "a", "b"):
                if key in row and row[key] is not None:
                    row[key] = str(row[key])
            if "path" in row:
                row["path"] = [str(node) for node in row["path"]]
            converted.append(row)
        return converted

    for dtype in ("f32", "f16", "u64"):
        path = str(Path(root) / f"tql-documents-graphs-{dtype}.tdb")
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)
        for source in fixture["documents"]:
            db.document_request(json.dumps({"operation": "put", "document": {
                "source_id": f"notes://{source['id']}", "source_revision": "v1", "parser_id": "plain-v1", "content": "alpha",
                "root": {"id": source["id"], "vector": [0, 0] if dtype == "u64" else source["root_vector"], "payload": {"active": True}},
                "blocks": [{"key": "body", "kind": "body", "start_byte": 0, "end_byte": 5,
                            "node": {"id": source["node"], "vector": source["binary_vector" if dtype == "u64" else "vector"], "payload": {"active": True}}}],
            }}))
        db.link(11, 31, "rel", 50.0)
        for graph in fixture["graphs"]:
            db.graph_request(json.dumps({"operation": "put", "graph": graph}))
        for case in fixture["cases"]:
            if dtype in case["dtypes"]:
                check(rows(db.tql(case["query"])), case["expected"])
        edge = db.tql("GRAPH {graph_ids:['words']} MATCH (a)-[e:rel]->(b) RETURN e")[0].row["e"]
        assert edge == {"type": "edge", "source": 11, "target": 21, "label": "rel", "weight": 2.0, "metadata": {"owner": "words"}, "graph_id": "words"}
        prepared = db.prepare_tql("GRAPH {graph_ids:[$graph]} DOCUMENTS {operation:'search',query:{text:$text}} WITH _ AS seed,document AS evidence EXPAND seed [:rel*1..1] AS related RETURN related.id AS id,evidence.document_id AS doc")
        assert prepared.parameter_names() == ["graph", "text"]
        check(rows(db.execute_prepared_tql(prepared, {"graph": "words", "text": "alpha"})), [{"id": "21", "doc": "10"}])
        try:
            db.tql("GRAPH {graph_ids:['missing']} MATCH (n) RETURN n")
        except triviumdb.TriviumDBError as error:
            assert error.code == "TDB_GRAPH_NOT_FOUND"
            assert error.stage == "plan"
            assert error.target["query_span"]["start_byte"] == 0
            assert error.target["query_span"]["end_byte"] == len("GRAPH {graph_ids:['missing']}")
        else:
            raise AssertionError("缺失的图必须结构化拒绝")
        if dtype != "u64":
            for residency in ("ssd", "hot"):
                db.quiver_request(json.dumps({"operation": "configure", "options": {"encoding": "centered_int4", "query_precision": "fp32", "residency": residency, "cache_bytes": 8192, "io_depth": 1, "use_for_ann": False}}))
                case = fixture["cases"][0]
                check(rows(db.tql(case["query"].replace("strategy:'exact'", "strategy:'ann',rerank:'compressed'"))), case["expected"])
        db.close()
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False, access_mode="read_only")
        check(rows(db.tql(fixture["cases"][2]["query"])), fixture["cases"][2]["expected"])
        db.close()
