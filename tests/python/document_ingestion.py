"""实际wheel的解析、原文证据和分批发布契约。"""
import hashlib
import json
from pathlib import Path

import triviumdb


def run_ingestion(root: str) -> None:
    fixture = json.loads((Path(__file__).parent / "../contracts/document_ingestion_cases.json").read_text(encoding="utf-8"))
    for dtype in ("f32", "f16", "u64"):
        path = str(Path(root) / f"ingestion-{dtype}.tdb")
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)

        def request(command: dict):
            return json.loads(db.document_request(json.dumps(command, ensure_ascii=False)))

        for item in fixture["formats"]:
            source = {"source_id": item["name"], "source_revision": "file-1", "content": item["raw"], "format": item.get("format", "auto"), "media_type": item.get("media_type")}
            parsed = request({"operation": "parse", "input": source})
            assert parsed["text"] == item["text"]
            assert parsed["source"]["content_hash"] == "sha256:" + hashlib.sha256(item["raw"].encode()).hexdigest()
            snapshot = request({"operation": "ingest", "input": source, "root": {"id": fixture["document_id"], "vector": [1, 0]}})
            assert snapshot["content"] == item["text"]
            assert snapshot["blocks"] == parsed["blocks"]
            evidence = request({"operation": "resolve", "reference": {"document_id": fixture["document_id"], "revision": snapshot["revision"], "start_byte": 0, "end_byte": len(item["text"].encode())}})
            assert evidence["text"] == item["text"]
            assert evidence["source_spans"]
            for span in evidence["source_spans"]:
                assert span["text"] == item["raw"].encode()[span["source_start"]:span["source_end"]].decode()
            db.close()
            db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)
            assert request({"operation": "get", "document_id": fixture["document_id"]}) == snapshot
            request({"operation": "delete", "document_id": fixture["document_id"], "expected_revision": snapshot["revision"]})
        if dtype == "f16":
            try:
                request({"operation": "ingest", "input": {"source_id": "bad.txt", "source_revision": "1", "content": "bad"}, "root": {"id": fixture["document_id"], "vector": [65536, 0]}})
            except Exception as error:
                assert error.code == "TDB_INVALID_VECTOR"
                assert error.target == {"field": "/root/vector/0"}
                assert error.write["outcome"] == "not_committed"
            else:
                raise AssertionError("f16超出有限范围必须拒绝")
            assert request({"operation": "list_uploads"}) == []
        begin = {"operation": "begin_upload", "upload": {"upload_id": fixture["upload_id"], "source_id": "stream.txt", "source_revision": "2", "parser_id": "host-v1", "content_hash": "sha256:" + hashlib.sha256(fixture["stream_text"].encode()).hexdigest(), "root": {"id": fixture["document_id"], "vector": [1, 0]}}}
        status = request(begin)
        assert status["state"] == "pending"
        assert request(begin) == status
        batch = {"operation": "append_upload", "upload_id": fixture["upload_id"], "batch": {"sequence": 0, "content": "new ", "blocks": [{"key": "p", "kind": "paragraph", "start_byte": 0, "end_byte": 7, "node": {"id": fixture["block_id"], "vector": [0, 1], "payload": {"key": "p"}}}]}}
        ack = request(batch)
        assert ack["next_sequence"] == 1
        assert request(batch) == ack
        assert db.node_count() == 0
        db.close()
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False)
        assert request({"operation": "upload_status", "upload_id": fixture["upload_id"]}) == ack
        request({"operation": "append_upload", "upload_id": fixture["upload_id"], "batch": {"sequence": 1, "content": "猫"}})
        published = request({"operation": "publish_upload", "upload_id": fixture["upload_id"]})
        assert published["content"] == fixture["stream_text"]
        assert db.node_count() == 2
        result = request({"operation": "search_vector", "query": {}, "vector": [0, 1], "options": {"strategy": "exact"}})
        assert len(result["groups"]) == 1
        assert result["groups"][0]["document_id"] == fixture["document_id"]
        assert len(result["groups"][0]["hits"]) == 1
        hit = result["groups"][0]["hits"][0]
        assert hit["node_id"] == fixture["block_id"]
        assert hit["score"] == (128 if dtype == "u64" else 1)
        assert hit["evidence"]["text"] == fixture["stream_text"]
        assert request({"operation": "publish_upload", "upload_id": fixture["upload_id"]}) == published
        changed = {**batch, "batch": {**batch["batch"], "content": "changed"}}
        try:
            request(changed)
        except Exception as error:
            assert error.code == "TDB_DOCUMENT_UPLOAD_CONFLICT"
        else:
            raise AssertionError("相同序号的不同批次必须拒绝")
        db.close()
        db = triviumdb.TriviumDB(path, dim=2, dtype=dtype, auto_build_quiver=False, access_mode="read_only")
        assert request({"operation": "get", "document_id": fixture["document_id"]}) == published
        assert request({"operation": "upload_status", "upload_id": fixture["upload_id"]})["visible"] is True
        try:
            request({"operation": "abort_upload", "upload_id": fixture["upload_id"]})
        except Exception as error:
            assert error.code == "TDB_READ_ONLY"
        else:
            raise AssertionError("只读上传变更必须拒绝")
        db.close()
