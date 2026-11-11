"""实际 wheel 的维护准入、拒绝后完整事实及再次持久化。"""

import json
from pathlib import Path
import triviumdb

CONTRACT = json.loads((Path(__file__).parents[1] / "contracts/index_maintenance_cases.json").read_text(encoding="utf-8"))


def execute(db, case):
    actions = {
        "index_text": lambda: db.index_text(case["id"], case["text"]),
        "index_keyword": lambda: db.index_keyword(case["id"], case["keyword"]),
        "build_text_index": db.build_text_index,
        "rebuild_text_index": lambda: db.rebuild_text_index(CONTRACT["rebuilt_documents"]),
        "create_index": lambda: db.create_index(case["field"]),
        "create_composite_index": lambda: db.create_composite_index(case["fields"]),
        "create_bitmap_index": lambda: db.create_bitmap_index(case["field"]),
        "create_ordered_index": lambda: db.create_ordered_index(case["field"]),
        "create_ngram_index": lambda: db.create_ngram_index(case["field"]),
        "create_unique_index": lambda: db.create_unique_index(case["field"]),
        "create_unique_composite_index": lambda: db.create_unique_composite_index(case["fields"]),
        "drop_index": lambda: db.drop_index(case["field"]),
        "drop_composite_index": lambda: db.drop_composite_index(case["fields"]),
        "drop_bitmap_index": lambda: db.drop_bitmap_index(case["field"]),
        "drop_ordered_index": lambda: db.drop_ordered_index(case["field"]),
        "drop_ngram_index": lambda: db.drop_ngram_index(case["field"]),
        "flush": db.flush,
        "compact": db.compact,
        "set_sync_mode": lambda: db.set_sync_mode(case["mode"]),
        "reserve_nodes": lambda: db.reserve_nodes(case["additional"]),
        "repair_graph_indexes": db.repair_graph_indexes,
        "set_auto_build_quiver": lambda: db.set_auto_build_quiver(case["enabled"]),
        "enable_auto_compaction": lambda: db.enable_auto_compaction(case["interval_secs"]),
    }
    return actions[case["api"]]()


def facts(db):
    nodes = []
    for id in sorted(db.all_node_ids()):
        node = db.get(id)
        nodes.append({"id": id, "vector": node.vector, "payload": node.payload,
                      "edges": [{"target": edge.target_id, "label": edge.label, "weight": edge.weight,
                                 "metadata": edge.metadata} for edge in node.edges]})
    queries = [sorted(row.row["hits"]["id"] for row in db.tql(
        f'TEXT {query["mode"]} "{query["term"]}" TOP 10 AS hits WITH hits RETURN hits'))
        for query in CONTRACT["queries"]]
    indexes = [{key: index[key] for key in ("kind", "fields", "unique")} for index in db.index_info()]
    return {"nodes": nodes, "queries": queries, "indexes": indexes}


def expected(after, indexes=()):
    return {"nodes": CONTRACT["nodes"], "queries": [query["after" if after else "before"] for query in CONTRACT["queries"]],
            "indexes": list(indexes)}


def check(action, code, details, write, reason=False, target=None):
    try:
        action()
    except (triviumdb.TriviumDBError, triviumdb.InvalidInputError) as error:
        assert error.schema_version == 1
        assert error.code == code, (code, error.code)
        if reason:
            assert list(error.details) == ["reason"]
            assert isinstance(error.details["reason"], str) and error.details["reason"]
        else:
            assert error.details == details
        assert error.write == write
        assert error.target == target
    else:
        raise AssertionError("维护请求必须返回明确拒绝")


def check_management_extraction(db):
    for field, action, valid in (("additional", db.reserve_nodes, 1), ("mb", db.set_memory_limit, 1),
                                 ("interval_secs", db.enable_auto_compaction, 7200)):
        for value, exception in [(None, TypeError), ("1", TypeError), (1.5, TypeError),
                                 ({}, TypeError), ([], TypeError), (-1, OverflowError),
                                 (1 << 100, OverflowError)]:
            try:
                action(value)
            except exception as error:
                assert error.schema_version == 1
                assert error.code == "TDB_INVALID_INPUT"
                assert error.target == {"field": field}
                assert error.stage is None
                assert error.write == CONTRACT["validation_write"]
            else:
                raise AssertionError(f"必须拒绝管理参数 {field}={value!r}")
        calls = []
        class Index:
            def __index__(self):
                check(lambda: db.reserve_nodes(1), "TDB_REENTRANT_ACCESS",
                      {"operation": "python_callback"}, CONTRACT["admission"])
                calls.append(True)
                return valid
        action(Index())
        assert calls == [True]
        cause = RuntimeError("宿主转换原因")
        sentinel = TypeError("宿主整数转换失败")
        sentinel.__cause__ = cause
        class BrokenIndex:
            def __index__(self):
                raise sentinel
        try:
            action(BrokenIndex())
        except TypeError as error:
            assert error is sentinel and error.__cause__ is cause
            assert not hasattr(error, "code")
        else:
            raise AssertionError("必须保留宿主整数协议异常")
        action(valid)
    db.disable_auto_compaction()
    for value in (0, 1, None, "false", [], {}):
        try:
            db.set_auto_build_quiver(value)
        except TypeError as error:
            assert error.code == "TDB_INVALID_INPUT"
            assert error.target == {"field": "enabled"}
            assert error.write == CONTRACT["validation_write"]
        else:
            raise AssertionError("布尔参数必须保持严格提取")
    db.set_auto_build_quiver(True)
    db.set_auto_build_quiver(False)
    for field, action in (("mode", db.set_sync_mode), ("generation_id", db.publish_generation_manifest)):
        for value, exception in ((None, TypeError), (0, TypeError), (True, TypeError),
                                 (b"normal", TypeError), ([], TypeError), ({}, TypeError),
                                 ("\ud800", UnicodeEncodeError)):
            try:
                action(value)
            except exception as error:
                assert error.schema_version == 1
                assert error.code == "TDB_INVALID_INPUT"
                assert error.target == {"field": field}
                assert error.stage is None
                assert error.write == CONTRACT["validation_write"]
            else:
                raise AssertionError(f"必须拒绝非字符串参数 {field}")
    class Text(str):
        def __str__(self):
            raise AssertionError("字符串子类按原始字符提取")
    db.set_sync_mode(Text("normal"))
    db.set_memory_limit()
    assert db.instance_stats()["memory_limit_bytes"] == 0
    db.set_memory_limit(1)
    db.enable_auto_compaction()
    db.disable_auto_compaction()


def run_index_maintenance(root):
    # 显式构建、运行期模式和任务捕获属于 Rust 入口，绑定验证已公开的维护操作。
    operations = [case for case in CONTRACT["operations"] if not case.get("rust_only")]
    for dtype in ("f32", "f16", "u64"):
        # Python 当前构造接口使用默认 Mmap，Rom 由 Rust 与 Node 实际入口验证。
        file = str(Path(root) / f"maintenance-{dtype}.tdb")
        options = dict(dim=2, dtype=dtype, load_text_index=True, auto_build_quiver=False, memory_limit_mb=1)
        db = triviumdb.TriviumDB(file, **options)
        try:
            for node in CONTRACT["nodes"]:
                vector = [int(component) for component in node["vector"]]
                db.insert_with_id(node["id"], vector, node["payload"])
            db.link(1, 2, "related", 1)
            db.index_text(1, "oldalpha")
            db.index_text(2, "oldbeta")
            db.index_keyword(1, "oldanchor")
            db.publish_generation_manifest("maintenance-seed")
            before = Path(file + ".wal").read_bytes()
            for case in CONTRACT["validation"]:
                check(lambda: execute(db, case), case["code"], case.get("details"), CONTRACT["validation_write"], case.get("reason"), case.get("target"))
                assert Path(file + ".wal").read_bytes() == before
                assert facts(db) == expected(False)
            for api in ("index_text", "index_keyword"):
                case = {"api": api, "id": 1, "text": "x" * 100000, "keyword": "x" * 100000}
                try:
                    execute(db, case)
                except triviumdb.CapacityError as error:
                    assert error.code == "TDB_CAPACITY_RESERVATION_REJECTED"
                    assert error.write == CONTRACT["validation_write"]
                else:
                    raise AssertionError("文本构建峰值必须经过预算准入")
            check(lambda: db.set_sync_mode("unsupported"), "TDB_INVALID_INPUT", None,
                  CONTRACT["validation_write"], True, {"field": "mode"})
            import sys
            try:
                db.set_memory_limit(sys.maxsize)
            except OverflowError as error:
                assert error.code == "TDB_INVALID_INPUT"
                assert error.target == {"field": "mb"}
                assert error.write == CONTRACT["validation_write"]
            else:
                raise AssertionError("MiB 换算溢出必须保留明确拒绝")
            assert Path(file + ".wal").read_bytes() == before
            check_management_extraction(db)
            assert Path(file + ".wal").read_bytes() == before
            assert facts(db) == expected(False)
            callbacks = []
            class Hook:
                def on_pre_search(self, vector, context):
                    for action in (lambda: db.set_sync_mode("full"), lambda: db.reserve_nodes(1),
                                   lambda: db.set_memory_limit(1), db.repair_graph_indexes,
                                   lambda: db.set_auto_build_quiver(False), lambda: db.enable_auto_compaction(7200),
                                   lambda: db.publish_generation_manifest("reentrant")):
                        check(action, "TDB_REENTRANT_ACCESS", {"operation": "python_callback"}, CONTRACT["admission"])
                    callbacks.append(True)
                    return vector
            db.set_hook(Hook())
            try:
                hits = db.search([1, 0], top_k=1)
                assert [(hit.id, hit.payload) for hit in hits] == [(1, CONTRACT["nodes"][0]["payload"])]
                assert callbacks == [True]
            finally:
                db.clear_hook()
            assert facts(db) == expected(False)
            db.close()
            for case in operations:
                check(lambda: execute(db, case), "TDB_DATABASE_CLOSED", {}, CONTRACT["admission"])
            check(lambda: db.set_memory_limit(0), "TDB_DATABASE_CLOSED", {}, CONTRACT["admission"])
            check(lambda: db.publish_generation_manifest("closed"), "TDB_DATABASE_CLOSED", {}, CONTRACT["admission"])
        finally:
            db.close()
        for access in ("read_only", "immutable"):
            reader = triviumdb.TriviumDB(file, **dict(options, access_mode=access))
            try:
                for case in operations:
                    check(lambda: execute(reader, case), "TDB_READ_ONLY", {"operation": case["api"]}, CONTRACT["admission"])
                reader.set_memory_limit(1)
                check(lambda: reader.publish_generation_manifest("read-only"), "TDB_READ_ONLY",
                      {"operation": "publish_generation_manifest"}, CONTRACT["admission"])
                assert facts(reader) == expected(False)
            finally:
                reader.close()
        writer = triviumdb.TriviumDB(file, **options)
        indexes = [{"kind": "hash", "fields": ["tag"], "unique": True}]
        try:
            writer.rebuild_text_index(CONTRACT["rebuilt_documents"])
            writer.create_unique_index("tag")
            assert facts(writer) == expected(True, indexes)
        finally:
            writer.close()
        reopened = triviumdb.TriviumDB(file, **options)
        try:
            assert facts(reopened) == expected(True, indexes)
        finally:
            reopened.close()
