"""安装制品的认知算子大数与图指标公共契约。"""

import math
import struct
from pathlib import Path
import triviumdb


def run_cognitive_boundaries(root: str) -> None:
    db = triviumdb.TriviumDB(str(Path(root) / "cognitive-numeric.tdb"), dim=2, auto_build_quiver=False)
    try:
        for node_id, vector in [(1, [1e30, 0]), (2, [0, 1e30]), (3, [1e30, 1e30])]:
            db.insert_with_id(node_id, vector, {})
            db.index_keyword(node_id, "alpha")
        weight = struct.unpack("f", struct.pack("f", 1e-40))[0]
        db.link(1, 2, "rel", weight)
        prefix = "SEARCH VECTOR [1, 0] TOP 3 AS seed WITH seed"
        topics = [result.row for result in db.tql(f"{prefix} TOPICS seed K 2 ITERATIONS 8 AS ranked WITH ranked RETURN ranked.id AS id, topic(ranked) AS topic, topic_score(ranked) AS score ORDER BY id")]
        assert [row["id"] for row in topics] == [1, 2, 3]
        for row in topics:
            assert row["topic"] in [0, 1]
            assert math.isfinite(row["score"]) and row["score"] > 0
        harmonic = [result.row for result in db.tql(f"{prefix} HARMONIC_CENTRALITY seed LABEL rel AS ranked WITH ranked RETURN ranked.id AS id, harmonic_centrality(ranked) AS score, graph_score(ranked) AS graph ORDER BY id")]
        assert harmonic == [
            {"id": 1, "score": 1 / weight, "graph": 1 / weight},
            {"id": 2, "score": 0, "graph": 0},
            {"id": 3, "score": 0, "graph": 0},
        ]
        text = "alpha " * 100
        diverse = [result.row for result in db.tql(f'TEXT AC "{text}" TOP 3 AS hits WITH hits DIVERSIFY hits TOP 2 QUALITY_WEIGHT 10 AS ranked WITH ranked RETURN ranked.id AS id, text_score(ranked) AS quality, diversity_score(ranked) AS rank ORDER BY rank DESC')]
        assert diverse == [{"id": 1, "quality": 100, "rank": 1}, {"id": 2, "quality": 100, "rank": 0.5}]
    finally:
        db.close()
