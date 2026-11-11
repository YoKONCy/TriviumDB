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
from typing import Any, Literal
from collections import UserDict
from types import MappingProxyType
from inspect import Signature

import triviumdb
from triviumdb import Edge, EdgeDirection, NodeView, SearchHit, Transaction, TriviumDB


def request_context_types(database: TriviumDB) -> None:
    context = triviumdb.RequestContext("typed-request", timeout_ms=0)
    assert_type(context.request_id, str)
    with context as active:
        assert_type(active, triviumdb.RequestContext)
        assert_type(database.get(7), NodeView | None)
    assert_type(context.cancel(), None)
    assert_type(context.check(), None)
    triviumdb.RequestContext(1)  # type: ignore[arg-type]
    context.request_id = "changed"  # type: ignore[misc]

db = TriviumDB("app.tdb", dim=8)
assert_type(TriviumDB("cache.tdb", dim=2, payload_cache_mb=0, payload_cache_entry_mb=0), TriviumDB)
TriviumDB("cache.tdb", payload_cache_mb="64")  # type: ignore[arg-type]
TriviumDB("cache.tdb", payload_cache_entry_mb=[])  # type: ignore[arg-type]
TriviumDB("boolean.tdb", load_text_index=1)  # type: ignore[arg-type]
TriviumDB("boolean.tdb", auto_build_quiver="false")  # type: ignore[arg-type]
TriviumDB("runtime.tdb", runtime={})  # type: ignore[arg-type]

# 控制台边界只收发原始 JSON 字符串，策略由宿主创建。
console = triviumdb.ConsoleSession()
assert_type(console.cancel("q1"), bool)
assert_type(console.is_detached(), bool)
assert_type(console.is_drained(), bool)
assert_type(db.console_request(console, '{"version":1,"requestId":"q1","command":{"operation":"info"}}'), str)
assert_type(console.detach(), None)
triviumdb.ConsoleSession({})  # type: ignore[arg-type]
db.console_request(console, {})  # type: ignore[arg-type]

# ── module / constructor ──
assert_type(db, TriviumDB)
assert_type(TriviumDB.__signature__, Signature)
assert_type(triviumdb.ResourceRuntime.__signature__, Signature)
assert_type(triviumdb.ConsoleSession.__signature__, Signature)
assert_type(triviumdb.init_logger(), None)

# ── writes ──
assert_type(db.insert([0.1] * 8, {"name": "Alice"}), int)
assert_type(db.batch_insert([[0.1] * 8], [{"k": 1}]), list[int])
assert_type(db.search_advanced((1, 0), min_score=0.0, expand_labels=("route",)), list[SearchHit])
assert_type(db.index_text(7, "正文"), None)
assert_type(db.index_keyword(7, "关键词"), None)
assert_type(db.rebuild_text_index((MappingProxyType({"id": 7, "text": "正文"}),)), None)
assert_type(db.substring_lookup("tag", "bet", max_results=1), list[int])
db.search([1, 0], min_score="0")  # type: ignore[arg-type]
db.search_advanced([1, 0], enable_dpp=1)  # type: ignore[arg-type]
db.search_advanced([1, 0], expand_labels=[1])  # type: ignore[list-item]
db.search_graph_first([1, 0], ["7"], 1)  # type: ignore[list-item]
db.index_text("7", "正文")  # type: ignore[arg-type]
db.index_keyword(7, 1)  # type: ignore[arg-type]
db.indexed_lookup({"tag": "alpha"}, max_results="1")  # type: ignore[arg-type]
db.substring_lookup("tag", 1)  # type: ignore[arg-type]
assert_type(db.insert_with_id(7, [1, 0], {}), None)
assert_type(db.upsert_with_id(7, (1, 0), {}), None)
assert_type(db.batch_insert_with_ids((7, 11), [[1, 0], [0, 1]], [{}, {}]), None)
assert_type(db.update_payload(7, {}), None)
assert_type(db.patch_payload(7, {"$set": {"name": "文档"}}), None)
assert_type(db.update_vector((1, 1), 7), None)
assert_type(db.link(7, 11), None)
assert_type(db.upsert_edge(7, 11, "route", metadata={}), None)
assert_type(db.update_edge(7, 11, "route", metadata={}), None)
assert_type(db.unlink(7, 11, None), None)
assert_type(db.delete(7), None)
TriviumDB()  # type: ignore[call-arg]
triviumdb.ResourceRuntime(unexpected=1)  # type: ignore[call-arg]
triviumdb.ConsoleSession("{}", "{}")  # type: ignore[call-arg]
db.insert_with_id("7", [1, 0], {})  # type: ignore[arg-type]
db.batch_insert_with_ids(["7"], [[1, 0]], [{}])  # type: ignore[list-item]
db.batch_insert([[1, "x"]], [{}])  # type: ignore[list-item]
db.batch_insert(((1, 0), (0, 1)), [{}, {}])  # type: ignore[arg-type]
db.batch_insert_with_ids([7], [[1, 0]], ({},))  # type: ignore[arg-type]
db.update_vector([1, "x"], 7)  # type: ignore[list-item]
db.link(7, 11, label=1)  # type: ignore[arg-type]
db.upsert_edge(7, 11, "route", weight="1")  # type: ignore[arg-type]
db.delete_many_atomic([7, "11"])  # type: ignore[list-item]

# ── retrieval ──
hits = db.search([0.1] * 8, top_k=5)
assert_type(hits, list[SearchHit])
assert_type(hits[0].id, int)
assert_type(hits[0].score, float)
db.search([0.1] * 8, top_k="1")  # type: ignore[arg-type]
db.search_grouped([0.1] * 8, expand_depth="1")  # type: ignore[arg-type]
db.search_batch([[0.1] * 8], parallelism="1")  # type: ignore[arg-type]
db.tql(1)  # type: ignore[arg-type]
db.prepare_tql(1)  # type: ignore[arg-type]
db.tql_mut(1)  # type: ignore[arg-type]
db.search_exact([0.1] * 8, "1")  # type: ignore[arg-type]
db.neighbors("7")  # type: ignore[arg-type]
db.neighbors(7, depth="2")  # type: ignore[arg-type]
db.reachable(7, max_visited_nodes="3")  # type: ignore[arg-type]
db.reachable_detailed(7, direction="sideways")  # type: ignore[arg-type]
db.query_subgraph(7, labels=[1])  # type: ignore[list-item]
db.search_graph_first([0.1] * 8, [1], 1, max_anchor_nodes="1")  # type: ignore[arg-type]
db.search_with_context([0.1] * 8, recall_k="1")  # type: ignore[arg-type]
db.search_advanced([0.1] * 8, max_edges_per_node="1")  # type: ignore[arg-type]
db.search_hybrid([0.1] * 8, 1)  # type: ignore[arg-type]
assert_type(db.search([0.1] * 8, payload_filter=MappingProxyType({"name": "Alice"})), list[SearchHit])
assert_type(db.search_grouped([0.1] * 8, payload_filter=UserDict({"name": "Alice"})), triviumdb.GroupedSearchResult)

# ── reads ──
assert_type(db.get(1), NodeView | None)
assert_type(db.get_edge(7, 11, "route"), triviumdb.Edge | None)
assert_type(db.get_edges(7), list[triviumdb.Edge])
assert_type(db.get_incoming_edges(11), list[triviumdb.IncomingEdge])
assert_type(db.__contains__(7), bool)
db.get("7")  # type: ignore[arg-type]
db.get_payload([])  # type: ignore[arg-type]
"7" in db  # type: ignore[operator]
db.get_edges(None)  # type: ignore[arg-type]
db.get_incoming_edges(11, label=1)  # type: ignore[arg-type]
db.get_edge(7, "11", "route")  # type: ignore[arg-type]
db.get_edge(7, 11, 1)  # type: ignore[arg-type]
assert_type(db.get_edges(1), list[Edge])
assert_type(db.node_count(), int)
assert_type(db.rebuild_text_index([{"id": 1, "text": "完整正文", "keywords": ["别名"]}]), None)
assert_type(db.instance_stats(), dict[str, Any])
db.set_sync_mode(1)  # type: ignore[arg-type]
db.publish_generation_manifest(1)  # type: ignore[arg-type]
prepared = db.prepare_tql("MATCH (n) RETURN $x AS value")
assert_type(db.execute_prepared_tql(prepared, MappingProxyType({"x": 42})), list[triviumdb.QueryRow])
assert_type(db.execute_prepared_tql(prepared, UserDict({"x": 42})), list[triviumdb.QueryRow])
assert_type(db.compare_and_set_payload_field(1, "state", "old", "new"), None)
assert_type(db.delete_many_atomic([1, 2]), int)
assert_type(db.indexed_lookup({"state": "new"}), list[int])
assert_type(db.substring_lookup("text", "正文"), list[int])
assert_type(db.create_ngram_index("text"), None)
assert_type(db.drop_ngram_index("text"), None)
assert_type(db.create_unique_index("key"), None)
assert_type(db.create_unique_composite_index(["scope", "key"]), None)
assert_type(db.list_unique_indexes(), list[list[str]])

# 资源异常保留内核预算字段。
def check_resource_error(error: triviumdb.ResourceBudgetError) -> None:
    assert_type(error.code, str)
    assert_type(error.details, dict[str, Any])
    assert_type(error.resource, str)
    assert_type(error.requested, int)
    assert_type(error.used, int)
    assert_type(error.limit, int)

def check_core_error(error: triviumdb.TriviumDBError) -> None:
    assert_type(error.schema_version, int)
    assert_type(error.message, str)
    assert_type(error.code, str)
    assert_type(error.details, dict[str, Any])
    assert_type(error.stage, Literal['lex', 'parse', 'bind', 'plan', 'execute'] | None)
    assert_type(error.target, dict[str, str | dict[str, int]] | None)
    assert_type(error.write, dict[str, str | bool] | None)
    if error.target is not None:
        location = error.target.get('query_span')
        if isinstance(location, dict):
            assert_type(location['start_byte'], int)
        field = error.target.get('field')
        if isinstance(field, str):
            assert_type(field, str)

def check_input_error(error: triviumdb.InvalidInputError) -> None:
    assert_type(error.schema_version, int)
    assert_type(error.message, str)
    assert_type(error.code, str)
    assert_type(error.details, dict[str, Any])

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
db.substring_lookup("text", 123)  # type: ignore[arg-type]
db.indexed_lookup([("key", "value")])  # type: ignore[arg-type]
takes_str: str = db.node_count()  # type: ignore[assignment]
db.search_advanced([0.1] * 8, edge_direction="sideways")  # type: ignore[arg-type]
