"""文档来源、UTF-8范围和跨重开修订的实际绑定契约。"""
import json
from pathlib import Path

import triviumdb


def run_documents(root: str) -> None:
    from document_ingestion import run_ingestion
    run_ingestion(root)
    run_retrieval(root)
    run_positions(root)
    run_fields(root)
    run_vectors(root)
    fixture = json.loads((Path(__file__).parent / "../contracts/document_cases.json").read_text(encoding="utf-8"))
    for dtype in ("f32", "f16", "u64"):
        path = str(Path(root) / f"documents-{dtype}.tdb")
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)

        def request(command: dict) -> object:
            return json.loads(db.document_request(json.dumps(command, ensure_ascii=False)))

        snapshot = request({"operation": "put", "document": fixture["document"]})
        assert isinstance(snapshot, dict)
        assert snapshot["document_id"] == "9007199254740993"
        assert snapshot["source_id"] == "vault://notes/cat.md"
        assert snapshot["source_revision"] == "file-v1"
        assert snapshot["content"] == fixture["document"]["content"]
        assert snapshot["content_hash"] == fixture["expected"]["content_hash"]
        assert [block["node_id"] for block in snapshot["blocks"]] == [None, "9007199254740995", "9007199254740997", "9007199254740999"]
        reference = {"document_id": snapshot["document_id"], "revision": snapshot["revision"], "start_byte": 10, "end_byte": 23}
        evidence = request({"operation": "resolve", "reference": reference})
        assert isinstance(evidence, dict)
        assert {key: evidence[key] for key in fixture["expected"]["evidence"]} == fixture["expected"]["evidence"]
        assert evidence["reference"] == reference
        try:
            request({"operation": "resolve", "reference": {**reference, "start_byte": 11}})
        except ValueError as error:
            assert error.code == "TDB_INVALID_INPUT"
            assert error.target == {"field": "reference"}
        else:
            raise AssertionError("UTF-8字符内部的引用起点必须拒绝")
        db.close()
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)
        assert request({"operation": "get", "document_id": snapshot["document_id"]}) == snapshot
        changed = json.loads(json.dumps(fixture["document"]))
        changed["source_revision"] = "file-v2"
        updated = request({"operation": "put", "document": changed, "expected_revision": snapshot["revision"]})
        assert isinstance(updated, dict)
        assert updated["source_revision"] == "file-v2"
        assert updated["revision"] != snapshot["revision"]
        try:
            request({"operation": "resolve", "reference": reference})
        except triviumdb.TriviumDBError as error:
            assert error.code == "TDB_DOCUMENT_REVISION_MISMATCH"
            assert error.details == {"document_id": snapshot["document_id"], "expected": snapshot["revision"], "actual": updated["revision"]}
        else:
            raise AssertionError("旧来源引用必须报告修订失配")
        assert request({"operation": "delete", "document_id": snapshot["document_id"], "expected_revision": updated["revision"]}) == {"deleted": True}
        db.close()
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)
        assert request({"operation": "get", "document_id": snapshot["document_id"]}) is None
        assert db.node_count() == 0
        db.close()


def run_retrieval(root: str) -> None:
    fixture = json.loads((Path(__file__).parent / "../contracts/document_retrieval_cases.json").read_text(encoding="utf-8"))
    for dtype in ("f32", "f16", "u64"):
        path = str(Path(root) / f"document-retrieval-{dtype}.tdb")
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)

        def request(command: dict) -> object:
            return json.loads(db.document_request(json.dumps(command)))

        snapshots = [request({"operation": "put", "document": document}) for document in fixture["documents"]]
        result = request({"operation": "search", "query": fixture["query"]})
        assert isinstance(result, dict)
        for key in ("corpus_documents", "corpus_units", "matched_units"):
            assert result[key] == fixture["expected"][key]
        assert [group["document_id"] for group in result["groups"]] == fixture["expected"]["document_ids"]
        assert [group["hits"][0]["node_id"] for group in result["groups"]] == fixture["expected"]["node_ids"]
        assert [group["hits"][0]["evidence"]["text"] for group in result["groups"]] == fixture["expected"]["texts"]
        for index, (group, snapshot) in enumerate(zip(result["groups"], snapshots)):
            assert isinstance(snapshot, dict)
            assert group["revision"] == snapshot["revision"]
            assert group["source_id"] == snapshot["source_id"]
            hit = group["hits"][0]
            text = fixture["expected"]["texts"][index]
            expected_evidence = {
                "reference": {"document_id": fixture["expected"]["document_ids"][index], "revision": snapshot["revision"], "start_byte": 0, "end_byte": len(text)},
                "source_id": fixture["documents"][index]["source_id"], "source_revision": "v1", "parser_id": "plain-v1",
                "content_hash": fixture["expected"]["content_hashes"][index], "text": text, "line": 1, "column": 1, "end_line": 1, "end_column": len(text)+1,
            }
            assert hit["evidence"] == expected_evidence
            assert group["score"] == hit["score"]
            assert group["score"] > 0
        assert request({"operation": "search", "query": {**fixture["query"], "use_cache": False}}) == result
        assert request({"operation": "clear_index"}) == {"cleared": True}
        assert request({"operation": "search", "query": fixture["query"]}) == result
        context_query = {"seeds": [{"reference": result["groups"][0]["hits"][0]["evidence"]["reference"], "block_key": "first"}], "before": 0, "after": 1}
        context = request({"operation": "context", "query": context_query})
        assert isinstance(context, list)
        assert len(context) == 1
        assert context[0]["block_keys"] == ["second", "first"]
        assert len(context[0]["windows"]) == 1
        assert context[0]["windows"][0]["text"] == "alpha alpha\nalpha alpha"
        assert context[0]["windows"][0]["reference"] == {**context_query["seeds"][0]["reference"], "end_byte": 23}
        for command, code in (
            ({"operation": "search", "query": {**fixture["query"], "limits": {"max_candidates": 1}}}, "QueueFull"),
            ({"operation": "search", "query": {**fixture["query"], "expected_revisions": {"100": "stale"}}}, "TDB_DOCUMENT_REVISION_MISMATCH"),
        ):
            try:
                request(command)
            except triviumdb.TriviumDBError as error:
                assert error.code == code
                assert error.write is None
            else:
                raise AssertionError("检索预算和修订冲突必须拒绝")
        db.close()
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)
        assert request({"operation": "search", "query": fixture["query"]}) == result
        assert request({"operation": "context", "query": context_query}) == context
        first = snapshots[0]
        assert isinstance(first, dict)
        assert request({"operation": "delete", "document_id": "100", "expected_revision": first["revision"]}) == {"deleted": True}
        remaining = request({"operation": "search", "query": fixture["query"]})
        assert isinstance(remaining, dict)
        assert [group["document_id"] for group in remaining["groups"]] == ["200"]
        db.close()


def run_positions(root: str) -> None:
    fixture = json.loads((Path(__file__).parent / "../contracts/document_positions_cases.json").read_text(encoding="utf-8"))
    for dtype in ("f32", "f16", "u64"):
        path = str(Path(root) / f"document-positions-{dtype}.tdb")
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)

        def request(command: dict) -> object:
            return json.loads(db.document_request(json.dumps(command, ensure_ascii=False)))

        snapshots = [request({"operation": "put", "document": document}) for document in fixture["documents"]]
        results = []
        for case in fixture["cases"]:
            query = {"level": "document", "include_matches": True, "max_documents": 10, "hits_per_document": 10, **case["query"]}
            result = request({"operation": "search", "query": query})
            assert isinstance(result, dict)
            assert result["analyzer_id"] == fixture["analyzer_id"]
            assert len(result["groups"]) == 1
            group = result["groups"][0]
            assert group["document_id"] == query["document_ids"][0]
            assert len(group["hits"]) == 1
            hit = group["hits"][0]
            assert hit["node_id"] == case["node_id"]
            index = next(i for i, document in enumerate(fixture["documents"]) if document["root"]["id"] == group["document_id"])
            snapshot = snapshots[index]
            assert isinstance(snapshot, dict)
            expected = [{
                "evidence": {
                    "reference": {"document_id": group["document_id"], "revision": snapshot["revision"], "start_byte": match["start_byte"], "end_byte": match["end_byte"]},
                    "source_id": fixture["documents"][index]["source_id"], "source_revision": "v1", "parser_id": "plain-v1", "content_hash": fixture["hashes"][index],
                    **{key: match[key] for key in ("text", "line", "column", "end_line", "end_column")},
                }, "terms": match["terms"],
            } for match in case["matches"]]
            assert hit["matches"] == expected, case["name"]
            assert request({"operation": "search", "query": {**query, "use_cache": False}}) == result
            results.append(result)
        context = request({"operation": "context", "query": {"seeds": [{"reference": results[0]["groups"][0]["hits"][0]["matches"][1]["evidence"]["reference"], "block_key": "second"}], "before": 0, "after": 0}})
        assert isinstance(context, list)
        assert context[0]["windows"][0]["text"] == "alpha beta beta"
        base = {"level": "document", "text": "alpha beta", "match_mode": "phrase", "include_matches": True}
        for extra, code, field in (
            ({"analyzer_id": "unknown"}, "TDB_INVALID_INPUT", "query.analyzer_id"),
            ({"max_gap": 1}, "TDB_INVALID_INPUT", "query.max_gap"),
            ({"limits": {"max_matches": 1}}, "QueueFull", None),
        ):
            try:
                request({"operation": "search", "query": {**base, **extra}})
            except (triviumdb.InvalidInputError, triviumdb.ResourceBudgetError) as error:
                assert error.code == code
                assert error.write is None
                if field is not None:
                    assert isinstance(error, ValueError)
                    assert error.target == {"field": field}
                else:
                    assert isinstance(error, triviumdb.ResourceBudgetError)
                    assert error.details["resource"] == "document_matches"
            else:
                raise AssertionError("位置参数及预算拒绝必须执行")
        db.close()
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)
        for case, expected in zip(fixture["cases"], results):
            query = {"level": "document", "include_matches": True, "max_documents": 10, "hits_per_document": 10, **case["query"]}
            assert request({"operation": "search", "query": query}) == expected
        changed = json.loads(json.dumps(fixture["documents"][0]))
        changed["source_revision"] = "v2"
        changed["content"] = changed["content"].replace("Alpha", "Delta").replace("alpha", "delta")
        first = snapshots[0]
        assert isinstance(first, dict)
        updated = request({"operation": "put", "document": changed, "expected_revision": first["revision"]})
        assert isinstance(updated, dict)
        old = request({"operation": "search", "query": base})
        assert isinstance(old, dict)
        assert old["groups"] == []
        new = request({"operation": "search", "query": {**base, "text": "delta beta"}})
        assert isinstance(new, dict)
        assert new["groups"][0]["revision"] == updated["revision"]
        assert [match["evidence"]["text"] for match in new["groups"][0]["hits"][0]["matches"]] == ["Delta, beta", "delta beta"]
        assert request({"operation": "delete", "document_id": "1000", "expected_revision": updated["revision"]}) == {"deleted": True}
        deleted = request({"operation": "search", "query": {**base, "text": "delta beta"}})
        assert isinstance(deleted, dict)
        assert deleted["groups"] == []
        db.close()


def run_vectors(root: str) -> None:
    fixture = json.loads((Path(__file__).parent / "../contracts/document_vector_cases.json").read_text(encoding="utf-8"))
    source = json.loads((Path(__file__).parent / "../contracts" / fixture["documents_fixture"]).read_text(encoding="utf-8"))
    for dtype in ("f32", "f16", "u64"):
        path = str(Path(root) / f"document-vectors-{dtype}.tdb")
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)

        def request(command: dict) -> dict:
            result = json.loads(db.document_request(json.dumps(command, ensure_ascii=False)))
            assert isinstance(result, dict)
            return result

        snapshots = [request({"operation": "put", "document": document}) for document in source["documents"]]

        def search(case: dict, use_cache: bool = True) -> dict:
            options = case.get("options", {"strategy": "exact"} if case["operation"] == "search_vector" else {"vector": {"strategy": "exact"}})
            return request({"operation": case["operation"], "query": {"hits_per_document": 10, "use_cache": use_cache, **case["query"]}, "vector": fixture["vector"], "options": options})

        def check(case: dict) -> dict:
            result = search(case)
            assert [group["document_id"] for group in result["groups"]] == case["documents"], case["name"]
            assert [[hit["node_id"] for hit in group["hits"]] for group in result["groups"]] == case["hits"], case["name"]
            assert result["matched_units"] == sum(len(hits) for hits in case["hits"])
            assert result["retrieval"] == {"hybrid": case["operation"] == "search_hybrid", "strategy": "exact", "approximate": False, "eligible_units": case["eligible"], "vector_candidates": case.get("vector_candidates", case["eligible"]), "vector_scores": case["eligible"], "ann_visits": 0, "ann_rounds": 0, "stop": "exact_complete"}
            for group in result["groups"]:
                snapshot = next(row for row in snapshots if row["document_id"] == group["document_id"])
                document = next(row for row in source["documents"] if row["root"]["id"] == group["document_id"])
                assert group["revision"] == snapshot["revision"]
                assert group["source_id"] == document["source_id"]
                assert group["source_revision"] == "v1"
                assert group["score"] == group["hits"][0]["score"]
                for hit in group["hits"]:
                    block = {"key": None, "kind": "document", "start_byte": 0, "end_byte": len(document["content"].encode())} if case["query"].get("level") == "document" else next(row for row in document["blocks"] if row["node"]["id"] == hit["node_id"])
                    assert (hit["kind"], hit["block_key"]) == (block["kind"], block["key"])
                    assert hit["evidence"]["reference"] == {"document_id": group["document_id"], "revision": snapshot["revision"], "start_byte": block["start_byte"], "end_byte": block["end_byte"]}
                    assert hit["evidence"]["text"] == document["content"].encode()[block["start_byte"]:block["end_byte"]].decode()
                    title = hit["kind"] in ("title", "document")
                    if case["name"] == "hybrid-sparse-only":
                        assert hit["scores"]["vector_score"] is None
                        assert hit["scores"]["vector_rank"] is None
                        rank = fixture["text_field_order"].index(hit["node_id"]) + 1
                        assert hit["scores"]["text_rank"] == rank
                        assert abs(hit["score"] - 120 / (60 + rank)) < 1e-6
                    else:
                        assert hit["scores"]["vector_score"] == ((128 if title else 126) if dtype == "u64" else (1 if title else 0))
                    if case["name"] == "hybrid-fields":
                        assert abs(hit["score"] - fixture["fused_scores"][hit["node_id"]]) < 1e-6
                        assert hit["scores"]["vector_rank"] == fixture["dense_block_order"].index(hit["node_id"]) + 1
                        if hit["node_id"] in fixture["text_field_order"]:
                            assert hit["scores"]["text_rank"] == fixture["text_field_order"].index(hit["node_id"]) + 1
                            assert abs(hit["scores"]["text_score"] - source["scores"][hit["node_id"]]) < 1e-6
                        else:
                            assert hit["scores"]["text_rank"] is None
                            assert hit["scores"]["text_score"] is None
                    if case["name"] == "hybrid-phrase-constraint":
                        assert abs(hit["score"] - 120 / 61) < 1e-6
                        assert len(hit["matches"]) == 1
                        assert hit["matches"][0]["evidence"]["text"] == "alpha beta"
                        assert hit["matches"][0]["terms"] == [{"term": "alpha", "position": 0, "start_byte": 0, "end_byte": 5}, {"term": "beta", "position": 1, "start_byte": 6, "end_byte": 10}]
                    else:
                        assert "matches" not in hit
                    if case["operation"] == "search_vector" or case["name"] == "hybrid-dense-only":
                        assert hit["score"] == hit["scores"]["vector_score"]
                        assert hit["scores"]["text_score"] is None
            assert search(case, False) == result
            return result

        for case in fixture["cases"]:
            check(case)
        for invalid in fixture["invalid"]:
            try:
                search(invalid)
            except triviumdb.InvalidInputError as error:
                assert error.code == "TDB_INVALID_INPUT"
                assert error.target == {"field": invalid["field"]}
                assert error.write is None
            else:
                raise AssertionError(f"无效向量检索参数必须拒绝 {invalid}")
        try:
            request({"operation": "search_vector", "query": {}, "vector": [1]})
        except (ValueError, triviumdb.TriviumDBError) as error:
            assert error.code == "TDB_DIMENSION_MISMATCH"
            assert error.target == {"field": "vector"}
        else:
            raise AssertionError("文档查询维度失配必须拒绝")
        retained = check(fixture["cases"][5])
        db.close()
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)
        assert check(fixture["cases"][5]) == retained
        changed = json.loads(json.dumps(source["documents"][0]))
        changed["source_revision"] = "v2"
        changed["blocks"][1]["node"]["vector"] = [1, 0]
        updated = request({"operation": "put", "document": changed, "expected_revision": snapshots[0]["revision"]})
        current = request({"operation": "search_vector", "query": {"document_ids": ["10"], "kinds": ["body"]}, "vector": [1, 0]})
        assert current["groups"][0]["hits"][0]["node_id"] == "12"
        assert current["groups"][0]["hits"][0]["score"] == (128 if dtype == "u64" else 1)
        assert current["groups"][0]["revision"] == updated["revision"]
        assert current["retrieval"]["strategy"] == "exact"
        request({"operation": "delete", "document_id": "10", "expected_revision": updated["revision"]})
        db.close()
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)
        assert request({"operation": "search_vector", "query": {"document_ids": ["10"]}, "vector": [1, 0]})["groups"] == []
        db.close()

    db = triviumdb.TriviumDB(str(Path(root) / "document-vectors-full-u64.tdb"), dim=2, dtype="u64", auto_build_quiver=False)
    db.document_request(json.dumps({"operation": "put", "document": {"source_id": "bits://full", "source_revision": "v1", "parser_id": "plain-v1", "content": "", "root": {"id": "10", "vector": [0, 0]}, "blocks": [
        {"key": "exact", "kind": "body", "start_byte": 0, "end_byte": 0, "node": {"id": "11", "vector": [18446744073709551615, 1]}},
        {"key": "near", "kind": "body", "start_byte": 0, "end_byte": 0, "node": {"id": "12", "vector": [18446744073709551614, 1]}}
    ]}}))
    full = json.loads(db.document_request('{"operation":"search_vector","query":{"hits_per_document":2},"vector":[18446744073709551615,1]}'))
    assert full["corpus_units"] == 0
    assert full["retrieval"]["eligible_units"] == 2
    assert [(hit["node_id"], hit["score"], hit["evidence"]["text"]) for hit in full["groups"][0]["hits"]] == [("11", 128, ""), ("12", 127, "")]
    db.close()


def run_fields(root: str) -> None:
    fixture = json.loads((Path(__file__).parent / "../contracts/document_fields_cases.json").read_text(encoding="utf-8"))
    for dtype in ("f32", "f16", "u64"):
        path = str(Path(root) / f"document-fields-{dtype}.tdb")
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)

        def request(command: dict) -> object:
            return json.loads(db.document_request(json.dumps(command, ensure_ascii=False)))

        snapshots = [request({"operation": "put", "document": document}) for document in fixture["documents"]]

        def search(case: dict, use_cache: bool = True) -> dict:
            extra = json.loads(case["query_json"]) if "query_json" in case else case["query"]
            result = request({"operation": "search", "query": {**fixture["query"], "use_cache": use_cache, **extra}})
            assert isinstance(result, dict)
            return result

        def check(case: dict) -> dict:
            result = search(case)
            assert [group["document_id"] for group in result["groups"]] == case["documents"], case["name"]
            assert [[hit["node_id"] for hit in group["hits"]] for group in result["groups"]] == case["hits"], case["name"]
            assert result["matched_units"] == sum(len(hits) for hits in case["hits"])
            assert (result["corpus_documents"], result["corpus_units"]) == (4, 7)
            if case["name"] != "unknown-field":
                assert len(result["field_corpora"]) == 2
                for field, expected in zip(result["field_corpora"], fixture["field_corpora"]):
                    assert {key: field[key] for key in ("kind", "weight", "corpus_units")} == {key: expected[key] for key in ("kind", "weight", "corpus_units")}
                    assert abs(field["average_length"] - expected["average_length"]) < 1e-6
            for group in result["groups"]:
                snapshot = next(row for row in snapshots if row["document_id"] == group["document_id"])
                document = next(row for row in fixture["documents"] if row["root"]["id"] == group["document_id"])
                assert group["revision"] == snapshot["revision"]
                assert group["source_id"] == document["source_id"]
                assert group["source_revision"] == "v1"
                assert group["score"] == group["hits"][0]["score"]
                for hit in group["hits"]:
                    block = next(row for row in document["blocks"] if row["node"]["id"] == hit["node_id"])
                    assert (hit["kind"], hit["block_key"]) == (block["kind"], block["key"])
                    assert hit["evidence"]["reference"] == {"document_id": group["document_id"], "revision": snapshot["revision"], "start_byte": block["start_byte"], "end_byte": block["end_byte"]}
                    assert hit["evidence"]["text"] == document["content"].encode()[block["start_byte"]:block["end_byte"]].decode()
                    if not case.get("query", {}).get("include_matches"):
                        assert abs(hit["score"] - fixture["scores"][hit["node_id"]]) < 1e-6
                    else:
                        assert hit["matches"][0]["terms"] == [{"term": "alpha", "position": 0, "start_byte": 0, "end_byte": 5}, {"term": "beta", "position": 1, "start_byte": 6, "end_byte": 10}]
            assert search(case, False) == result
            return result

        for case in fixture["cases"]:
            check(case)
        for invalid in fixture["invalid"]:
            try:
                search(invalid)
            except triviumdb.InvalidInputError as error:
                assert isinstance(error, ValueError)
                assert error.code == "TDB_INVALID_INPUT"
                assert error.target == {"field": invalid["field"]}
                assert error.write is None
            else:
                raise AssertionError(f"无效字段与过滤参数必须拒绝 {invalid}")
        selected = next(case for case in fixture["cases"] if case["name"] == "scope-intersection")
        retained = check(selected)
        db.close()
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)
        assert check(selected) == retained
        changed = json.loads(json.dumps(fixture["documents"][0]))
        changed["source_revision"] = "v2"
        changed["root"]["payload"]["tags"] = ["new"]
        updated = request({"operation": "put", "document": changed, "expected_revision": snapshots[0]["revision"]})
        assert search(selected)["groups"] == []
        current = request({"operation": "search", "query": {**fixture["query"], "members": [{"scope": "document", "field": "tags", "mode": "any", "values": ["new"]}]}})
        assert [[hit["node_id"] for hit in group["hits"]] for group in current["groups"]] == [["11", "12"]]
        assert current["groups"][0]["revision"] == updated["revision"]
        assert request({"operation": "delete", "document_id": "10", "expected_revision": updated["revision"]}) == {"deleted": True}
        db.close()
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)
        assert search(selected)["groups"] == []
        db.close()
