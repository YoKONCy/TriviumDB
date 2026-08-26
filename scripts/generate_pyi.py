#!/usr/bin/env python3
"""Generate ``triviumdb.pyi`` from the compiled ``triviumdb`` extension.

The drift-prone parts of a stub — which classes/methods exist, parameter
names, ordering and defaults — are read from the installed extension with
``inspect.signature`` (PyO3 exposes them via ``__text_signature__``), so they
can never fall out of sync with the Rust bindings. PyO3 does not expose
*types* (the interesting params are ``Bound<PyAny>``), so types come from the
tables below — the only hand-maintained surface.

Usage::

    python scripts/generate_pyi.py            # regenerate the committed stub
    python scripts/generate_pyi.py --check    # CI: fail if the stub is stale

The module must be importable first (build & install the wheel, e.g.
``maturin develop --features python``).
"""

from __future__ import annotations

import argparse
import importlib
import inspect
import sys
from pathlib import Path

EXCEPTIONS = [
    "ReadOnlyError", "RecoveryRequiredError",
    "ImmutableArtifactError", "GenerationBusyError",
]
CLASS_ORDER = [
    "SearchHit", "GroupedSearchResult", "Edge", "IncomingEdge", "NodeView",
    "ReachabilityStep", "ReachabilityResult", "QueryRow", "HookContext",
    "Transaction", "TriviumDB",
]
FUNCTIONS = ["init_logger"]
CONSTRUCTIBLE = {"TriviumDB"}  # only these expose a real #[new]

# Dunders emitted from the method loop (__new__ is handled by the ctor branch).
KEEP_DUNDERS = ["__len__", "__contains__", "__repr__", "__enter__", "__exit__"]

# getter attributes: {class: {attr: type}} (declared order is emitted order).
ATTRS = {
    "SearchHit": {"id": "int", "score": "float", "payload": "Any"},
    "GroupedSearchResult": {"semantic_hits": "list[SearchHit]", "graph_hits": "list[SearchHit]"},
    "Edge": {"target_id": "int", "label": "str", "weight": "float"},
    "IncomingEdge": {"source_id": "int", "target_id": "int", "label": "str", "weight": "float"},
    "NodeView": {"id": "int", "vector": "list[float]", "payload": "Any",
                 "edges": "list[Edge]", "num_edges": "int"},
    "ReachabilityStep": {"from_id": "int", "to_id": "int", "label": "str"},
    "ReachabilityResult": {"source_id": "int", "target_id": "int", "depth": "int",
                           "path": "list[int]", "steps": "list[ReachabilityStep]"},
    "QueryRow": {"row": "dict[str, dict[str, Any]]"},
    "HookContext": {"timings": "dict[str, float]", "counts": "dict[str, int]",
                    "custom_data": "Any", "observations": "dict[str, Any]", "aborted": "bool"},
    "TriviumDB": {"dtype": "str"},
}

# Param type by name. A param whose default is None is auto-wrapped in Optional.
PARAM_TYPES = {
    "vector": "Sequence[float]", "query_vector": "Sequence[float]",
    "vectors": "Sequence[Sequence[float]]", "query_vectors": "Sequence[Sequence[float]]",
    "ids": "Sequence[int]", "anchor_ids": "Sequence[int]",
    "payload": "Any", "payloads": "Sequence[Any]", "patch": "Mapping[str, Any]",
    "payload_filter": "Optional[Mapping[str, Any]]",
    "labels": "Optional[Sequence[str]]", "expand_labels": "Optional[Sequence[str]]",
    "hook": "Any",
    "exc_type": "object", "_exc_type": "object", "_exc_val": "object", "_exc_tb": "object",
}
# bulk name→type groups (space-separated names sharing one type)
for _t, _names in {
    "int": "id src dst key depth expand_depth min_depth max_depth max_visited_nodes "
           "max_anchor_nodes parallelism additional mb interval_secs min_community_size "
           "max_iterations dim new_dim memory_limit_mb expected_nodes top_k recall_k rerank_k",
    "float": "weight min_score teleport_alpha fista_lambda fista_threshold "
             "dpp_quality_weight text_boost hybrid_alpha",
    "str": "text keyword field query query_text mode lib_path path new_path generation_id "
           "dtype sync_mode access_mode missing_index_policy direction label custom_query_text",
    "bool": "load_text_index auto_build_quiver enabled compute_centroids "
            "enable_advanced_pipeline enable_sparse_residual enable_dpp "
            "enable_refractory_fatigue enable_text_hybrid_search force_brute_force",
}.items():
    PARAM_TYPES.update(dict.fromkeys(_names.split(), _t))

# Return type → space-separated "Class.method" (or bare function). Any method
# not listed emits Any plus a warning, so a new Rust method is loud, not silent.
RETURNS = {}
for _ret, _quals in {
    "None": "init_logger "
            "TriviumDB.set_sync_mode TriviumDB.load_ffi_hook TriviumDB.clear_hook "
            "TriviumDB.set_hook TriviumDB.insert_with_id TriviumDB.batch_insert_with_ids "
            "TriviumDB.update_payload TriviumDB.patch_payload TriviumDB.update_vector "
            "TriviumDB.delete TriviumDB.link TriviumDB.unlink TriviumDB.index_text "
            "TriviumDB.index_keyword TriviumDB.build_text_index TriviumDB.create_index "
            "TriviumDB.drop_index TriviumDB.flush TriviumDB.compact "
            "TriviumDB.enable_auto_compaction TriviumDB.disable_auto_compaction "
            "TriviumDB.set_auto_build_quiver TriviumDB.clear_search_state "
            "TriviumDB.reserve_nodes TriviumDB.set_memory_limit TriviumDB.close "
            "Transaction.insert Transaction.insert_with_id Transaction.link "
            "Transaction.delete Transaction.unlink Transaction.update_payload "
            "Transaction.update_vector Transaction.rollback",
    "int": "TriviumDB.insert TriviumDB.node_count TriviumDB.dim TriviumDB.estimated_memory "
           "TriviumDB.__len__ Transaction.pending_count",
    "list[int]": "TriviumDB.all_node_ids TriviumDB.batch_insert TriviumDB.migrate "
                 "TriviumDB.neighbors Transaction.commit",
    "list[SearchHit]": "TriviumDB.search TriviumDB.search_advanced TriviumDB.search_hybrid "
                       "TriviumDB.search_graph_first TriviumDB.search_exact",
    "list[list[SearchHit]]": "TriviumDB.search_batch",
    "list[Edge]": "TriviumDB.get_edges",
    "list[IncomingEdge]": "TriviumDB.get_incoming_edges",
    "list[ReachabilityResult]": "TriviumDB.reachable",
    "list[QueryRow]": "TriviumDB.tql",
    "dict[str, Any]": "TriviumDB.tql_mut TriviumDB.leiden_cluster",
    "GroupedSearchResult": "TriviumDB.search_grouped",
    "tuple[list[SearchHit], HookContext]": "TriviumDB.search_with_context",
    "Optional[NodeView]": "TriviumDB.get",
    "Optional[Any]": "TriviumDB.get_payload",
    "Transaction": "TriviumDB.transaction",
    "Any": "TriviumDB.publish_generation_manifest",
    "str": "TriviumDB.__repr__ Transaction.__repr__ QueryRow.__repr__ HookContext.__repr__",
    "bool": "TriviumDB.__contains__ TriviumDB.__exit__ Transaction.__exit__",
    '"TriviumDB"': "TriviumDB.__new__ TriviumDB.__enter__",
    '"Transaction"': "Transaction.__enter__",
}.items():
    RETURNS.update(dict.fromkeys(_quals.split(), _ret))

HEADER = '''\
# AUTO-GENERATED by scripts/generate_pyi.py — DO NOT EDIT BY HAND.
#
# Structure is reflected from the compiled `triviumdb`; types come from the
# generator's tables. Regenerate after changing src/bindings/python.rs:
#     python scripts/generate_pyi.py
# PEP 561: maturin renames this to triviumdb/__init__.pyi and adds py.typed.

from collections.abc import Mapping, Sequence
from typing import Any, Optional, final
'''

_EMPTY = inspect.Parameter.empty


def param_type(name: str, default) -> str:
    t = PARAM_TYPES.get(name, "Any")
    return f"Optional[{t}]" if default is None and not t.startswith("Optional") else t


def render_params(sig: inspect.Signature, receiver: str) -> str:
    parts = [receiver] if receiver else []
    params = [p for p in sig.parameters.values()
              if p.name not in ("self", "cls")
              and p.kind not in (p.VAR_POSITIONAL, p.VAR_KEYWORD)]
    for i, p in enumerate(params):
        piece = f"{p.name}: {param_type(p.name, p.default)}"
        if p.default is not _EMPTY:
            piece += f" = {p.default!r}"
        parts.append(piece)
        nxt = params[i + 1] if i + 1 < len(params) else None
        if p.kind == p.POSITIONAL_ONLY and (nxt is None or nxt.kind != p.POSITIONAL_ONLY):
            parts.append("/")
    return ", ".join(parts)


def return_type(qual: str) -> str:
    r = RETURNS.get(qual)
    if r is None:
        print(f"warning: no return type for {qual}; using Any", file=sys.stderr)
        return "Any"
    return r


def render_def(qual: str, name: str, obj, receiver: str) -> str:
    try:
        sig = inspect.signature(obj)
    except (TypeError, ValueError):
        return ""
    return f"    def {name}({render_params(sig, receiver)}) -> {return_type(qual)}: ..."


def class_methods(cls_obj) -> list[str]:
    regular, dunders = [], []
    for name, member in cls_obj.__dict__.items():
        if not callable(member) or getattr(member, "__text_signature__", None) is None:
            continue
        if not name.startswith("__"):
            regular.append(name)
        elif name in KEEP_DUNDERS:
            dunders.append(name)
    return sorted(regular) + sorted(dunders, key=KEEP_DUNDERS.index)


def render_class(mod, cls: str) -> str:
    obj = getattr(mod, cls)
    body = [f"    {attr}: {typ}" for attr, typ in ATTRS.get(cls, {}).items()]
    if cls in CONSTRUCTIBLE:
        body.append(render_def(f"{cls}.__new__", "__new__", obj, "cls"))
    for name in class_methods(obj):
        body.append(render_def(f"{cls}.{name}", name, obj.__dict__[name], "self"))
    body = [line for line in body if line] or ["    ..."]
    return f"@final\nclass {cls}:\n" + "\n".join(body)


def build_stub(mod) -> str:
    blocks = [
        HEADER,
        "__all__ = [\n" + "".join(f'    "{n}",\n'
                                  for n in EXCEPTIONS + CLASS_ORDER + FUNCTIONS) + "]",
        "# Exceptions",
        *(f"class {e}({getattr(mod, e).__bases__[0].__name__}): ..." for e in EXCEPTIONS),
        "# Classes",
        *(render_class(mod, c) for c in CLASS_ORDER),
        "# Functions",
        *(f"def {f}({render_params(inspect.signature(getattr(mod, f)), '')}) "
          f"-> {return_type(f)}: ..." for f in FUNCTIONS),
    ]
    return "\n\n".join(blocks) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="fail if the stub is stale")
    ap.add_argument("--output", type=Path,
                    default=Path(__file__).resolve().parent.parent / "triviumdb.pyi")
    args = ap.parse_args()

    try:
        mod = importlib.import_module("triviumdb")
    except ImportError as e:
        print(f"error: cannot import triviumdb ({e}); build & install the wheel first "
              f"(maturin develop --features python).", file=sys.stderr)
        return 2

    stub = build_stub(mod)
    if args.check:
        current = args.output.read_text() if args.output.exists() else ""
        if current != stub:
            print(f"error: {args.output} is out of date. Run: python scripts/generate_pyi.py",
                  file=sys.stderr)
            return 1
        print(f"ok: {args.output} is up to date")
        return 0

    args.output.write_text(stub)
    print(f"wrote {args.output} ({stub.count(chr(10))} lines)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
