"""实际wheel的压缩文档分组、评分精度与来源证据契约。"""
import json
import math
from pathlib import Path

import triviumdb


def run_compressed(root: str) -> None:
    fixture = json.loads((Path(__file__).parent / "../contracts/document_compressed_cases.json").read_text(encoding="utf-8"))
    for dtype in ("f32", "f16"):
        db = triviumdb.TriviumDB(str(Path(root) / f"document-compressed-{dtype}.tdb"), dim=fixture["dimension"], dtype=dtype, auto_build_quiver=False)

        def direction(value: int) -> list[float]:
            return [0.0] * (fixture["dimension"] - 1) + [float(value)]

        def request(command: dict) -> dict:
            return json.loads(db.document_request(json.dumps(command)))

        snapshots = {}
        for row in fixture["documents"]:
            snapshots[row["id"]] = request({"operation": "put", "document": {
                "source_id": f"notes://{row['id']}", "source_revision": "v1", "parser_id": "plain-v1", "content": "alpha",
                "root": {"id": row["id"], "vector": direction(row["root_direction"]), "payload": {"vault": row["vault"]}},
                "blocks": [{"key": "body", "kind": "body", "start_byte": 0, "end_byte": 5, "node": {
                    "id": row["node_id"], "vector": direction(row["direction"]), "payload": {"tags": ["evidence"], "state": "ready"},
                }}],
            }})
        vector = direction(1)
        vector[0] = 0.1
        for encoding in fixture["encodings"]:
            for precision in fixture["precisions"]:
                for residency in ("ssd", "hot"):
                    json.loads(db.quiver_request(json.dumps({"operation": "configure", "options": {
                        "encoding": encoding, "query_precision": precision, "residency": residency,
                        "cache_bytes": 0, "io_depth": 1, "io_mode": "auto", "use_for_ann": False,
                    }})))
                    for case in fixture["cases"]:
                        options = {"rerank": "compressed", "initial_candidates": 1, "candidate_limit": 64, "ef_search": 64}
                        result = request({"operation": case["operation"], "query": {"max_documents": 3, "hits_per_document": 1, **case["query"]}, "vector": vector,
                                          "options": {"vector": options} if case["operation"] == "search_hybrid" else options})
                        assert [group["document_id"] for group in result["groups"]] == case["documents"]
                        assert [[hit["node_id"] for hit in group["hits"]] for group in result["groups"]] == [[node] for node in case["nodes"]]
                        info = result["retrieval"]
                        assert info["rerank"] == "compressed" and info["strategy"] == "ann" and info["approximate"] is True
                        assert info["compression"] == {"encoding": encoding, "query_precision": precision, "residency": residency}
                        assert info["stop"] == case["stop"]
                        for group in result["groups"]:
                            snapshot = snapshots[group["document_id"]]
                            hit = group["hits"][0]
                            node = hit["node_id"]
                            expected = fixture["positive_scores"][dtype][precision] * fixture["signs"][node]
                            assert abs(hit["scores"]["vector_score"] - expected) < 1e-6
                            assert hit["evidence"]["text"] == "alpha"
                            assert hit["evidence"]["reference"] == {"document_id": group["document_id"], "revision": snapshot["revision"], "start_byte": 0, "end_byte": 5}
                            assert hit["evidence"]["source_id"] == snapshot["source_id"]
                            assert hit["evidence"]["source_revision"] == "v1"
                            if case["operation"] == "search_hybrid":
                                dense, text = fixture["dense_ranks"][node], fixture["text_ranks"][node]
                                assert hit["scores"]["vector_rank"] == dense
                                assert hit["scores"]["text_rank"] == text
                                assert abs(hit["scores"]["text_score"] - math.log(8 / 7)) < 1e-6
                                assert abs(hit["score"] - (60 / (60 + dense) + 60 / (60 + text))) < 1e-6
                            else:
                                assert abs(hit["score"] - expected) < 1e-6
                                assert hit["scores"]["text_score"] is None
                            assert group["score"] == hit["score"]
        try:
            request({"operation": "search_hybrid", "query": {"text": "alpha"}, "vector": vector, "options": {"vector": {"strategy": "exact", "rerank": "compressed"}}})
        except triviumdb.InvalidInputError as error:
            assert error.code == "TDB_INVALID_INPUT"
            assert error.target == {"field": "options.vector.rerank"}
        else:
            raise AssertionError("精确策略与压缩重排的组合必须拒绝")
        db.close()
