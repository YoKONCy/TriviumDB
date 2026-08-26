"""End-to-end type-stub test for the packaged ``triviumdb`` wheel.

This file is not executed for behavior; it is fed to a *type checker*
(mypy and pyright) in CI against the pip-installed wheel. It asserts the
whole chain works from an end user's point of view:

* ``py.typed`` is packaged and honored — otherwise ``triviumdb`` is treated
  as ``Any`` and every ``assert_type`` below fails.
* each public API resolves to the exact expected type (``assert_type``).
* misuse is rejected — the ``# type: ignore`` lines must correspond to a real
  error, or ``mypy --warn-unused-ignores`` reports the ignore as unused and
  the check fails.

Run locally:

    maturin build --profile dev --features python -i python3 --out dist
    pip install --force-reinstall dist/*.whl typing_extensions mypy pyright
    mypy --warn-unused-ignores tests/python/test_type_stubs.py
    pyright tests/python/test_type_stubs.py
"""

from typing_extensions import assert_type

import triviumdb
from triviumdb import Edge, EdgeDirection, NodeView, SearchHit, Transaction, TriviumDB

db = TriviumDB("app.tdb", dim=8)

# ── module / constructor ──
assert_type(db, TriviumDB)
assert_type(triviumdb.init_logger(), None)

# ── writes ──
assert_type(db.insert([0.1] * 8, {"name": "Alice"}), int)
assert_type(db.batch_insert([[0.1] * 8], [{"k": 1}]), list[int])

# ── retrieval ──
hits = db.search([0.1] * 8, top_k=5)
assert_type(hits, list[SearchHit])
assert_type(hits[0].id, int)
assert_type(hits[0].score, float)

# ── reads ──
assert_type(db.get(1), NodeView | None)
assert_type(db.get_edges(1), list[Edge])
assert_type(db.node_count(), int)

# ── graph edge fields ──
edge = db.get_edges(1)[0]
assert_type(edge.target_id, int)
assert_type(edge.label, str)
assert_type(edge.weight, float)

# ── transaction (context manager) ──
with db.transaction() as tx:
    assert_type(tx, Transaction)
    assert_type(tx.commit(), list[int])

# ── graph expansion controls ──
direction: EdgeDirection = "both"
assert_type(
    db.search_advanced(
        [0.1] * 8,
        max_edges_per_node=5,
        min_edge_weight=0.2,
        edge_direction=direction,
    ),
    list[SearchHit],
)

# ── negative assertions: the stub MUST reject these ──
# If a line below stops being an error, --warn-unused-ignores fails the build.
db.insert("not-a-vector", {})  # type: ignore[arg-type]
db.node_count("extra-arg")  # type: ignore[call-arg]
db.nonexistent_method()  # type: ignore[attr-defined]
takes_str: str = db.node_count()  # type: ignore[assignment]
db.search_advanced([0.1] * 8, edge_direction="sideways")  # type: ignore[arg-type]
