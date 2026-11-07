"""检索转换与文本维护的独立预期、宿主异常和实际制品契约。"""

import json
from pathlib import Path
import threading
from types import MappingProxyType
from collections.abc import Mapping

import triviumdb
from query_arguments import check_hits, check_input_error
from read_arguments import node

CONTRACTS = Path(__file__).parents[1] / "contracts"
CASE = json.loads((CONTRACTS / "retrieval_fields.json").read_text(encoding="utf-8"))
TEXT = json.loads((CONTRACTS / "index_maintenance_cases.json").read_text(encoding="utf-8"))
VALIDATION = {"phase": "validation", "outcome": "not_committed"}


def invalid(action, field, write=None):
    try:
        action()
    except (TypeError, ValueError, OverflowError, UnicodeError, triviumdb.TriviumDBError) as error:
        check_input_error(error, field, write)
        return error
    raise AssertionError(f"入口必须拒绝 {field}")


def reentry(db):
    for action, outcome in ((lambda: db.get(7), None),
                            (lambda: db.index_text(7, "回调"), {"phase": "admission", "outcome": "not_committed"})):
        try:
            action()
        except triviumdb.TriviumDBError as error:
            assert error.code == "TDB_REENTRANT_ACCESS" and error.write == outcome
        else:
            raise AssertionError("转换回调必须在请求借用内执行")


def queries(db):
    return [
        (lambda vector: db.search(vector, top_k=1, min_score=0), lambda result: result, "query_vector"),
        (lambda vector: db.search_grouped(vector, top_k=1, expand_depth=0, min_score=0), lambda result: result.semantic_hits, "query_vector"),
        (lambda vector: db.search_batch(vector, top_k=1, min_score=0), lambda result: result[0], "query_vectors"),
        (lambda vector: db.search_exact(vector, 1), lambda result: result, "query_vector"),
        (lambda vector: db.search_graph_first(vector, [7], 1), lambda result: result, "query_vector"),
        (lambda vector: db.search_with_context(vector, top_k=1, expand_depth=0, min_score=0), lambda result: result[0], "query_vector"),
        (lambda vector: db.search_advanced(vector, top_k=1, expand_depth=0, min_score=0, enable_advanced_pipeline=False), lambda result: result, "query_vector"),
        (lambda vector: db.search_hybrid(vector, "", top_k=1, expand_depth=0, min_score=0), lambda result: result, "query_vector"),
    ]


def run_retrieval_fields(root):
    for dtype in ("f32", "f16", "u64"):
        directory = Path(root) / f"retrieval-fields-{dtype}"
        path = str(directory / "database.tdb")
        options = dict(dim=2, dtype=dtype, auto_build_quiver=False)
        db = triviumdb.TriviumDB(path, **options)
        try:
            db.insert_with_id(**CASE["query_seed"])
            wal = Path(path + ".wal").read_bytes()
            session = triviumdb.ConsoleSession()
            for action, field, write in [
                (lambda: db.migrate(1, 2), "new_path", VALIDATION),
                (lambda: db.migrate(str(directory / "unused.tdb"), "2"), "new_dim", VALIDATION),
                (lambda: db.create_snapshot(1, "generation"), "destination", VALIDATION),
                (lambda: db.create_snapshot(str(directory / "unused"), 1), "generation_id", VALIDATION),
                (lambda: db.close_timeout("0"), "timeout_ms", None),
                (lambda: db.load_ffi_hook(1), "lib_path", None),
                (lambda: db.console_request(session, 1), "request_json", None),
                (lambda: db.console_request(None, "{}"), "session", None),
                (lambda: db.execute_prepared_tql(object(), {}), "prepared", None),
                (lambda: session.cancel(1), "request_id", None),
                (lambda: triviumdb.verify_snapshot(1), "directory", None),
                (lambda: triviumdb.restore_snapshot(1, "unused"), "source", VALIDATION),
                (lambda: triviumdb.restore_snapshot("unused", 1), "destination", VALIDATION),
            ]:
                invalid(action, field, write)
            for case in CASE["index_fields"]:
                method = getattr(db, case["python"])
                for value in (None, 1, "\ud800"):
                    invalid(lambda: method(value), "field", VALIDATION)
            for case in CASE["composite_fields"]:
                method = getattr(db, case["python"])
                invalid(lambda: method(None), "fields", VALIDATION)
                invalid(lambda: method(["saved", 1]), "/fields/1", VALIDATION)
                class Fields:
                    def __len__(self):
                        reentry(db)
                        return 2
                    def __getitem__(self, index):
                        reentry(db)
                        return ("saved", "\ud800")[index]
                invalid(lambda: method(Fields()), "/fields/1", VALIDATION)
            for field, value in (("min_community_size", "3"), ("max_iterations", -1), ("compute_centroids", "true")):
                invalid(lambda: db.leiden_cluster(**{field: value}), field)
            class CommunitySize:
                def __index__(self):
                    reentry(db)
                    return 1
            clustered = db.leiden_cluster(min_community_size=CommunitySize(), compute_centroids=False)
            assert clustered == {"communities": [[7]], "centroids": {}, "num_clusters": 1}
            for action, unpack, field in queries(db):
                values = ((None, field), ("10", field), ([1, "x"], f"/{field}/1"))
                if field == "query_vectors":
                    values = ((None, field), ([None], f"/{field}/0"), ([[1, 0], [1, "x"]], f"/{field}/1/1"))
                for value, location in values:
                    invalid(lambda: action(value), location)
                class Sequence:
                    def __len__(self):
                        reentry(db)
                        return 1 << 61

                    def __getitem__(self, index):
                        return (1, 0)[index]
                vector = Sequence()
                check_hits(unpack(action([vector] if field == "query_vectors" else vector)), 128 if dtype == "u64" else 1)
                class FailedHint(Sequence):
                    def __len__(self):
                        raise TypeError("长度提示沿现有策略忽略")
                check_hits(unpack(action([FailedHint()] if field == "query_vectors" else FailedHint())), 128 if dtype == "u64" else 1)
            for value, field in ((None, "anchor_ids"), ([7, "x"], "/anchor_ids/1")):
                invalid(lambda: db.search_graph_first([1, 0], value, 1), field)
            for entry in CASE["floats"]:
                field = entry["python"]
                for value in ("1", [], {}, None, object()):
                    invalid(lambda: db.search_advanced([1, 0], **{field: value}), field)
                for value in (float("nan"), float("inf"), -float("inf"), 1e100, -1e100):
                    invalid(lambda: db.search_advanced([1, 0], **{field: value}), field)
            for entry in CASE["booleans"]:
                field = entry["python"]
                for value in (None, 0, 1, "false", [], {}):
                    invalid(lambda: db.search_advanced([1, 0], **{field: value}), field)
                base = dict(top_k=1, expand_depth=0, enable_advanced_pipeline=False)
                base[field] = False
                check_hits(db.search_advanced([1, 0], **base), 128 if dtype == "u64" else 1)
            for method in (db.search, db.search_grouped, db.search_batch, db.search_with_context, db.search_hybrid):
                vector = [[1, 0]] if method == db.search_batch else [1, 0]
                args = (vector, "") if method == db.search_hybrid else (vector,)
                invalid(lambda: method(*args, min_score="1"), "min_score")
            for value in (float("nan"), float("inf"), -float("inf")):
                invalid(lambda: db.search_hybrid([1, 0], "", hybrid_alpha=value), "hybrid_alpha")
                invalid(lambda: db.search_batch([], min_score=value), "min_score")
            assert db.search_batch([]) == []
            for value, field in ((1, "custom_query_text"), ("\ud800", "custom_query_text"),
                                 ("route", "expand_labels"), (["route", 1], "/expand_labels/1"),
                                 (1, "edge_direction"), ("sideways", "edge_direction")):
                argument = "expand_labels" if field.startswith("/") else field
                invalid(lambda: db.search_advanced([1, 0], **{argument: value}), field)
            invalid(lambda: db.search([1, 0], payload_filter={"a/b~c": [object()]}), "/payload_filter/a~1b~0c/0")
            calls = []
            class Scalar:
                def __float__(self):
                    calls.append("float")
                    reentry(db)
                    return 0.0
            check_hits(db.search([1, 0], top_k=1, min_score=Scalar()), 128 if dtype == "u64" else 1)
            assert calls == ["float"]
            cause = RuntimeError("宿主 cause")
            sentinel = TypeError("宿主浮点原异常")
            sentinel.code = "HOST_FLOAT"
            class BrokenScalar:
                def __float__(self):
                    raise sentinel from cause
            try:
                db.search([1, 0], min_score=BrokenScalar())
            except TypeError as error:
                assert error is sentinel and error.__cause__ is cause and error.code == "HOST_FLOAT"
                assert not hasattr(error, "target")
            else:
                raise AssertionError("宿主浮点异常必须原样传播")
            assert node(db.get(7)) == CASE["query_seed"] | {"num_edges": 0, "edges": []}
            assert Path(path + ".wal").read_bytes() == wal
            held = db.get(7)
        finally:
            db.close()
        moved = directory.with_name(directory.name + "-moved")
        directory.rename(moved)
        assert node(held) == CASE["query_seed"] | {"num_edges": 0, "edges": []}
        for _ in range(2):
            reopened = triviumdb.TriviumDB(str(moved / "database.tdb"), **options)
            try:
                assert reopened.all_node_ids() == [7]
                assert node(reopened.get(7)) == CASE["query_seed"] | {"num_edges": 0, "edges": []}
                reopened.flush()
            finally:
                reopened.close()
    run_text_fields(root)


def text_facts(db, after):
    for value in TEXT["nodes"]:
        expected = dict(id=value["id"], vector=value["vector"], payload=value["payload"],
                        num_edges=len(value["edges"]), edges=[dict(target_id=edge["target"], label=edge["label"],
                        weight=edge["weight"], metadata=edge["metadata"]) for edge in value["edges"]])
        assert node(db.get(value["id"])) == expected
    assert db.all_node_ids() == [1, 2]
    for query in TEXT["queries"]:
        rows = db.tql(f'TEXT {query["mode"]} "{query["term"]}" TOP 10 AS hits WITH hits RETURN hits')
        assert sorted(row.row["hits"]["id"] for row in rows) == query["after" if after else "before"], query
    assert db.indexed_lookup({"tag": "alpha"}) == [1]
    assert db.substring_lookup("tag", "bet") == [2]


def run_text_fields(root):
    for dtype in ("f32", "f16", "u64"):
        directory = Path(root) / f"text-fields-{dtype}"
        file = str(directory / "database.tdb")
        options = dict(dim=2, dtype=dtype, auto_build_quiver=False, load_text_index=True)
        db = triviumdb.TriviumDB(file, **options)
        try:
            for value in TEXT["nodes"]:
                db.insert_with_id(value["id"], [int(component) for component in value["vector"]], value["payload"])
            db.link(1, 2)
            db.rebuild_text_index([dict(id=1, text="oldalpha", keywords=["oldanchor"]), dict(id=2, text="oldbeta")])
            db.create_index("tag")
            db.create_ngram_index("tag")
            db.build_text_index()
            wal = Path(file + ".wal").read_bytes()
            for method, field in ((db.index_text, "text"), (db.index_keyword, "keyword")):
                invalid(lambda: method("1", "x"), "id", VALIDATION)
                invalid(lambda: method(1, 1), field, VALIDATION)
                invalid(lambda: method(1, "\ud800"), field, VALIDATION)
            for case in CASE["bad_documents"]:
                invalid(lambda: db.rebuild_text_index(case["value"]), case["field"], VALIDATION)
                text_facts(db, False)
            for value in (1, "documents", None):
                invalid(lambda: db.rebuild_text_index(value), "documents", VALIDATION)
            invalid(lambda: db.rebuild_text_index([{"id": 1, "text": "\ud800"}]), "/documents/0/text", VALIDATION)
            for method, args in ((db.indexed_lookup, ({"tag": "alpha"},)), (db.substring_lookup, ("tag", "bet"))):
                for value in ("1", -1, 1 << 100):
                    invalid(lambda: method(*args, max_results=value), "max_results")
                invalid(lambda: method(*args, max_results=0), "max_results")
            invalid(lambda: db.indexed_lookup([]), "equalities")
            invalid(lambda: db.indexed_lookup({"a/b~c": object()}), "/equalities/a~1b~0c")
            invalid(lambda: db.substring_lookup(1, "x"), "field")
            invalid(lambda: db.substring_lookup("tag", 1), "needle")
            assert Path(file + ".wal").read_bytes() == wal
            cause = RuntimeError("宿主映射 cause")
            sentinel = TypeError("宿主映射原异常")
            sentinel.code = "HOST_MAPPING"
            class BrokenMapping(Mapping):
                def __len__(self):
                    return 1
                def __iter__(self):
                    reentry(db)
                    return iter(("id",))
                def __getitem__(self, key):
                    raise sentinel from cause
            try:
                db.rebuild_text_index([BrokenMapping()])
            except TypeError as error:
                assert error is sentinel and error.__cause__ is cause and error.code == "HOST_MAPPING"
                assert not hasattr(error, "target")
            else:
                raise AssertionError("映射协议异常必须原样传播")
            completed = threading.Event()
            failures = []
            class Id:
                def __index__(self):
                    def contend():
                        try:
                            db.index_keyword(1, "回调")
                        except BaseException as error:
                            if not isinstance(error, triviumdb.TriviumDBError) or error.code != "TDB_BUSY" or error.write != {"phase": "admission", "outcome": "not_committed"}:
                                failures.append(error)
                        else:
                            failures.append("写争用必须拒绝")
                        finally:
                            completed.set()
                    worker = threading.Thread(target=contend, daemon=True)
                    worker.start()
                    assert completed.wait(10)
                    worker.join(10)
                    assert not worker.is_alive() and failures == []
                    return 1
            db.index_text(Id(), "oldalpha")
            assert completed.is_set() and failures == []
            records = [dict(id=1, text="nexttext", keywords=["nextanchor"]), MappingProxyType(dict(id=2, text="oldbeta"))]
            class RebuildingId:
                def __index__(self):
                    records.clear()
                    return 1
            records[0]["id"] = RebuildingId()
            db.rebuild_text_index(records)
            db.index_text(1, "nexttext")
            db.index_keyword(1, "nextanchor")
            db.build_text_index()
            text_facts(db, True)
            held = db.get(1)
            held_error = invalid(lambda: db.index_text(1, 1), "text", VALIDATION)
            db.flush()
        finally:
            db.close()
        moved = directory.with_name(directory.name + "-moved")
        directory.rename(moved)
        assert held.payload == TEXT["nodes"][0]["payload"] and held_error.target == {"field": "text"}
        for _ in range(2):
            reopened = triviumdb.TriviumDB(str(moved / "database.tdb"), **options)
            try:
                text_facts(reopened, True)
                reopened.flush()
            finally:
                reopened.close()
