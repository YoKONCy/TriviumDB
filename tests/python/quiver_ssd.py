"""共享方向样例验证INT8协议、缓存、驻留与跨重开结果。"""
import json
from pathlib import Path
import triviumdb


def run_quiver_ssd(root: str) -> None:
    fixture = json.loads((Path(__file__).parent / "../contracts/quiver_ssd_cases.json").read_text(encoding="utf-8"))
    for dtype in ("f32", "f16"):
        file = str(Path(root) / f"quiver-ssd-{dtype}.tdb")
        db = triviumdb.TriviumDB(file, dim=fixture["dimension"], dtype=dtype, auto_build_quiver=False)

        def vector(direction: int) -> list[int]:
            return [0] * (fixture["dimension"] - 1) + [direction]

        def request(command: dict) -> object:
            return json.loads(db.quiver_request(json.dumps(command)))

        for node in fixture["nodes"]:
            db.insert_with_id(int(node["id"]), vector(node["direction"]), {"id": node["id"]})
        stats = request({"operation": "configure", "options": fixture["options"]})
        assert stats["metadata"]["dimension"] == fixture["dimension"]
        assert stats["residency"] == "ssd"
        search = dict(fixture["search"], vector=vector(1))
        assert request(search) == {"hits": fixture["expected_hits"]}
        first = request({"operation": "stats"})
        assert first["physical_read_calls"] > 0
        assert first["physical_read_bytes"] >= first["cold_read_bytes"]
        if first["io_mode"] == "direct":
            assert 0 < first["io_alignment_bytes"] <= 65536
            assert 0 < first["io_memory_alignment_bytes"] <= 65536
            assert isinstance(first["io_alignment_detected"], bool)
            assert 0 < first["io_window_bytes"] <= 65536
        else:
            assert first["io_alignment_bytes"] == 0
            assert first["io_window_bytes"] == 0
        assert request(search) == {"hits": fixture["expected_hits"]}
        cached = request({"operation": "stats"})
        assert cached["cold_read_calls"] == first["cold_read_calls"]
        assert cached["physical_read_calls"] == first["physical_read_calls"]
        assert cached["physical_read_bytes"] == first["physical_read_bytes"]
        request({"operation": "configure", "options": dict(fixture["options"], cache_bytes=0)})
        before_window = request({"operation": "stats"})
        assert request(search) == {"hits": fixture["expected_hits"]}
        assert request(search) == {"hits": fixture["expected_hits"]}
        reused = request({"operation": "stats"})
        assert reused["cache_limit_bytes"] == 0
        assert reused["cache_used_bytes"] == 0
        assert reused["cold_read_calls"] == before_window["cold_read_calls"] + 8
        if reused["io_mode"] == "direct":
            assert reused["physical_read_calls"] == before_window["physical_read_calls"]
            assert reused["physical_read_bytes"] == before_window["physical_read_bytes"]
            assert reused["window_reuse_hits"] == before_window["window_reuse_hits"] + 2
            assert reused["window_reuse_bytes"] == before_window["window_reuse_bytes"] + 8 * reused["metadata"]["record_bytes"]
        request({"operation": "configure", "options": dict(fixture["options"], residency="hot")})
        hot = request({"operation": "stats"})
        assert hot["io_active"] is False
        assert hot["io_window_bytes"] == 0
        assert hot["io_alignment_bytes"] == 0
        assert request(search) == {"hits": fixture["expected_hits"]}
        assert request({"operation": "stats"})["cold_read_calls"] == hot["cold_read_calls"]
        for command in ({"operation": "configure", "options": {"io_depth": 0}}, {"operation": "stats", "unexpected": True}):
            try:
                request(command)
            except triviumdb.InvalidInputError as error:
                assert error.code == "TDB_INVALID_INPUT"
            else:
                raise AssertionError("无效请求必须拒绝")
        db.close()
        db = triviumdb.TriviumDB(file, dim=fixture["dimension"], dtype=dtype, access_mode="read_only", auto_build_quiver=False)
        request({"operation": "configure", "options": fixture["options"]})
        assert request(search) == {"hits": fixture["expected_hits"]}
        assert request({"operation": "close"}) is None
        assert request({"operation": "stats"}) is None
        db.close()
