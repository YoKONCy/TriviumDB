"""Python 公共 API 跨层契约测试。

覆盖 Prepared TQL、一等 Path/List、四类属性索引、存储诊断和历史入口迁移错误。
"""

import json
import os
import shutil
import tempfile

import triviumdb
from write_boundaries import run_write_boundaries
from index_graph_boundaries import run_index_graph_boundaries
from query_numeric_indexes import run_query_numeric_indexes
from tql_semantics import run_tql_semantics
from tql_boundaries import run_tql_boundaries
from error_contracts import run_error_contract
from kernel_boundaries import run_kernel_boundaries


def run_transaction_contract(root: str) -> None:
    contract_path = os.path.join(os.path.dirname(__file__), "..", "contracts", "transaction_cases.json")
    with open(contract_path, encoding="utf-8") as source:
        contract = json.load(source)
    for dtype in contract["dtypes"]:
        for case in contract["cases"]:
            file = os.path.join(root, f"transaction-{dtype}-mmap-{case['name']}.tdb")
            options = dict(dim=contract["dimension"], dtype=dtype, auto_build_quiver=False)
            db = triviumdb.TriviumDB(file, **options)
            try:
                for node in contract["setup"]:
                    db.insert_with_id(node["id"], node["vector"], node["payload"])
                with open(file + ".wal", "rb") as source:
                    before_wal = source.read()
                tx = db.transaction()
                for operation in case["operations"]:
                    kind = operation["type"]
                    if kind == "insert":
                        tx.insert(operation["vector"], operation["payload"])
                    elif kind == "insertWithId":
                        tx.insert_with_id(operation["id"], operation["vector"], operation["payload"])
                    elif kind == "delete":
                        tx.delete(operation["id"])
                    elif kind == "updatePayload":
                        tx.update_payload(operation["id"], operation["payload"])
                    elif kind == "updateVector":
                        tx.update_vector(operation["id"], operation["vector"])
                    else:
                        raise AssertionError(f"未知事务契约操作：{kind}")
                if "error" in case["expected"]:
                    try:
                        tx.commit()
                    except RuntimeError:
                        pass
                    else:
                        raise AssertionError("非法事务必须整批拒绝")
                    with open(file + ".wal", "rb") as source:
                        assert source.read() == before_wal
                else:
                    assert tx.commit() == case["expected"]["ids"]

                def check(database):
                    assert sorted(database.all_node_ids()) == [node["id"] for node in case["expected"]["nodes"]]
                    for expected in case["expected"]["nodes"]:
                        actual = database.get(expected["id"])
                        assert actual is not None
                        assert actual.id == expected["id"]
                        assert actual.payload == expected["payload"]
                        assert actual.vector == expected["vector"]

                check(db)
                replay_file = file + ".replay"
                shutil.copyfile(file, replay_file)
                shutil.copyfile(file + ".wal", replay_file + ".wal")
                replay = triviumdb.TriviumDB(replay_file, **options)
                try:
                    check(replay)
                finally:
                    replay.close()
                db.close()
                reopened = triviumdb.TriviumDB(file, **options)
                try:
                    check(reopened)
                finally:
                    reopened.close()
            finally:
                db.close()


def run_text_recovery_contract(root: str) -> None:
    source = os.path.join(os.path.dirname(__file__), "..", "fixtures", "v0810_text_mmap")
    for missing in [False, True]:
        target = os.path.join(root, f"text-upgrade-{missing}.tdb")
        for suffix in ["", ".vec", ".flush_ok", ".pld.2", ".text", ".text.meta", ".wal"]:
            if missing and suffix == ".text.meta":
                continue
            shutil.copyfile(os.path.join(source, "sample.tdb" + suffix), target + suffix)
        db = triviumdb.TriviumDB(target, dim=4, auto_build_quiver=False)
        db.insert_with_id(5, [0, 0, 1, 0], {})
        db.flush()
        db.build_text_index()
        assert sorted(row.row["hits"]["id"] for row in db.tql(
            'TEXT BM25 "alpha" TOP 10 AS hits WITH hits RETURN hits')) == [1, 2, 3, 4]
        db.close()
        with open(target + ".text", "wb") as output:
            output.write(b"corrupt")
        db = triviumdb.TriviumDB(target, dim=4, load_text_index=True, auto_build_quiver=False)
        with open(target + ".text", "rb") as original:
            assert original.read() == b"corrupt"
        try:
            db.build_text_index()
        except triviumdb.ImmutableArtifactError:
            pass
        else:
            raise AssertionError("损坏文本应返回结构化校验错误")
        db.rebuild_text_index([{"id": 1, "text": "restoredtoken", "keywords": ["restoredanchor"]}])
        db.close()
        reader = triviumdb.TriviumDB(target, dim=4, load_text_index=True, access_mode="read_only")
        assert reader.node_count() == 5
        assert [row.row["hits"]["id"] for row in reader.tql(
            'TEXT BM25 "restoredtoken" TOP 10 AS hits WITH hits RETURN hits')] == [1]
        assert [row.row["hits"]["id"] for row in reader.tql(
            'TEXT AC "restoredanchor" TOP 10 AS hits WITH hits RETURN hits')] == [1]
        reader.close()


def run_lifecycle_contract(root: str) -> None:
    case_path = os.path.join(os.path.dirname(__file__), "..", "contracts", "lifecycle_cases.json")
    with open(case_path, encoding="utf-8") as source:
        case = json.load(source)
    for dtype in case["dtypes"]:
        runtime = triviumdb.ResourceRuntime(**case["runtime"])
        db = triviumdb.TriviumDB(os.path.join(root, f"lifecycle-{dtype}.tdb"),
            dim=case["dimension"], dtype=dtype, memory_limit_mb=case["database_memory_limit_mb"],
            auto_build_quiver=False, runtime=runtime)
        db.insert_with_id(case["node_id"], case["vector"], case["original_payload"])
        snapshot = os.path.join(root, f"snapshot-{dtype}")
        restored = os.path.join(root, f"restored-{dtype}")
        manifest = db.create_snapshot(snapshot, case["generation_id"])
        assert manifest["node_count"] == 1 and manifest["complete"]
        db.update_payload(case["node_id"], case["later_payload"])
        db.insert_with_id(case["later_node_id"], case["vector"], {})
        try:
            db.create_snapshot(snapshot, case["generation_id"])
        except FileExistsError:
            pass
        else:
            raise AssertionError("不得覆盖已有快照")
        assert triviumdb.verify_snapshot(snapshot, runtime)["node_count"] == 1
        assert triviumdb.restore_snapshot(snapshot, restored, runtime)["generation_id"] == case["generation_id"]
        try:
            triviumdb.restore_snapshot(snapshot, restored, runtime)
        except FileExistsError:
            pass
        else:
            raise AssertionError("不得覆盖已有恢复目标")
        reader = triviumdb.TriviumDB(os.path.join(restored, case["database_file"]),
            dim=case["dimension"], dtype=dtype, access_mode="immutable",
            memory_limit_mb=case["database_memory_limit_mb"], runtime=runtime)
        assert reader.get_payload(case["node_id"]) == case["original_payload"]
        assert case["later_node_id"] not in reader
        assert reader.close_timeout(case["close_timeout_ms"])
        assert triviumdb.verify_snapshot(restored, runtime)["node_count"] == 1
        db.request_shutdown()
        assert db.close_timeout(case["close_timeout_ms"])
        assert db.shutdown_status()["phase"] == "closed"
        assert db.close_timeout(0)
        stats = runtime.stats()
        assert stats["attached_databases"] == 0
        assert stats["active_maintenance_tasks"] == 0
        assert stats["reserved_maintenance_bytes"] == 0
        assert stats["peak_reserved_bytes"] <= stats["memory_limit"]


def run_resource_contract(root: str) -> None:
    source_path = os.path.join(os.path.dirname(__file__), "..", "contracts", "resource_cases.json")
    with open(source_path, encoding="utf-8") as source:
        case = json.load(source)
    mb = 1024 * 1024
    for dtype in case["dtypes"]:
        runtime = triviumdb.ResourceRuntime(**case["runtime"])

        def check(phase):
            stats = runtime.stats()
            for field, expected in case["expected"][phase].items():
                assert stats[field] == expected, (dtype, phase, field, stats)
            assert stats["active_tasks"] == 0
            assert stats["reserved_workspace_bytes"] == 0
            assert stats["peak_worker_threads"] <= case["runtime"]["max_threads"]

        def open_db(name, memory):
            return triviumdb.TriviumDB(
                os.path.join(root, f"resource-{dtype}-{name}.tdb"), dtype=dtype,
                dim=case["database"]["dim"], memory_limit_mb=memory,
                auto_build_quiver=False, runtime=runtime,
            )

        def check_instance(db, phase):
            stats = db.instance_stats()
            expected = case["instances"][phase]
            assert stats["memory_limit_bytes"] == expected["memory_limit_bytes"]
            assert stats["memory"]["reserved_database_bytes"] == expected["reserved_database_bytes"]
            assert stats["memory"]["node_count"] == expected["node_count"]
            assert stats["shutdown"]["phase"] == expected["phase"]
            assert stats["shutdown"]["active_operations"] == 0
            assert stats["shutdown"]["last_failure"] is None
            assert os.path.isabs(stats["database_path"])

        def check_budget(error, phase):
            for key, value in case["budget_errors"][phase].items():
                assert getattr(error, key) == value, (phase, key)

        first = open_db("first", case["database"]["memory_limit_mb"])
        second = open_db("second", case["database"]["memory_limit_mb"])
        check("opened")
        check_instance(first, "opened")
        check_instance(second, "opened")
        try:
            open_db("rejected", case["overbook_mb"])
        except triviumdb.ResourceBudgetError as error:
            check_budget(error, "open")
        else:
            raise AssertionError("超额开库必须返回资源异常")
        assert not os.path.exists(os.path.join(root, f"resource-{dtype}-rejected.tdb"))
        first.set_memory_limit(case["resize_mb"])
        check("resized")
        check_instance(first, "resized")
        try:
            second.set_memory_limit(case["failed_resize_mb"])
        except triviumdb.ResourceBudgetError as error:
            check_budget(error, "resize")
        else:
            raise AssertionError("扩容失败必须保留原配额")
        check("resized")
        check_instance(second, "opened")
        first.set_memory_limit(case["database"]["memory_limit_mb"])
        first.insert_with_id(101, case["vector"], {"owner": "first"})
        second.insert_with_id(202, case["vector"], {"owner": "second"})
        assert first.search_exact(case["vector"], 1)[0].id == 101
        assert second.search_exact(case["vector"], 1)[0].id == 202
        first.enable_auto_compaction(3600)
        assert first.instance_stats()["compaction"]["phase"] == "scheduled"
        first.disable_auto_compaction()
        assert first.instance_stats()["compaction"] is None
        first.close()
        check_instance(first, "closed")
        check("closed_one")
        replacement = open_db("replacement", case["replacement_mb"])
        check("replaced")
        second.close()
        replacement.close()
        check("closed_all")
        assert runtime.stats()["peak_reserved_bytes"] <= case["runtime"]["memory_limit_mb"] * mb
    unlimited = triviumdb.ResourceRuntime().stats()
    for key in ["available_foreground_bytes", "available_maintenance_bytes", "available_task_slots"]:
        assert unlimited[key] is None
    for options in [{"max_threads": 65}, {"max_threads": -1}]:
        try:
            triviumdb.ResourceRuntime(**options)
        except (ValueError, OverflowError, RuntimeError):
            pass
        else:
            raise AssertionError("无效线程额度必须被拒绝")
    try:
        triviumdb.TriviumDB(os.path.join(root, "bad-runtime.tdb"), runtime={})
    except TypeError:
        pass
    else:
        raise AssertionError("运行时参数必须是原生资源句柄")


def run_shared_contract(root: str) -> None:
    contract_path = os.path.join(os.path.dirname(__file__), "..", "contracts", "public_cases.json")
    with open(contract_path, encoding="utf-8") as source:
        contract = json.load(source)
    assert contract["schema_version"] == 1
    path = os.path.join(root, "shared-contract.tdb")
    db = triviumdb.TriviumDB(path, dim=contract["setup"]["dimension"])
    for node in contract["setup"]["nodes"]:
        db.insert_with_id(node["id"], node["vector"], node["payload"])
    for edge in contract["setup"]["edges"]:
        db.link(edge["source"], edge["target"], edge["label"], edge["weight"])
    for case in contract["cases"]:
        expected = case["expected"]
        if case["operation"] == "get_payload":
            payload = db.get_payload(case["node_id"])
            assert payload[expected["field"]] == expected["value"], case["name"]
        elif case["operation"] == "prepared_tql":
            prepared = db.prepare_tql(case["tql"])
            rows = db.execute_prepared_tql(prepared, case.get("parameters", {}))
            assert len(rows) == expected["row_count"], case["name"]
            assert rows[0].row[expected["column"]] == expected["value"], case["name"]
        elif case["operation"] == "tql_path":
            rows = db.tql(case["tql"])
            assert rows[0].row["path"] == expected["path"], case["name"]
        elif case["operation"] == "prepared_missing_parameter":
            prepared = db.prepare_tql(case["tql"])
            try:
                db.execute_prepared_tql(prepared, case.get("parameters", {}))
            except RuntimeError as error:
                assert error.code == expected["error_code"]
                assert error.details == expected["error_details"]
            else:
                raise AssertionError(case["name"])
        elif case["operation"] == "text_lifecycle":
            parameters = case["parameters"]
            db.index_text(1, parameters["old_text"])
            db.index_keyword(1, parameters["keyword"])
            db.index_keyword(1, parameters["keyword"])
            db.index_keyword(2, parameters["keyword"])
            db.build_text_index()
            db.index_text(1, parameters["new_text"])
            for phase in ["before_delete", "after_delete"]:
                if phase == "after_delete":
                    db.delete(1)
                for check in expected[phase]:
                    assert len(db.tql(check["query"])) == check["count"], case["name"]
        elif case["operation"] == "text_rebuild":
            db.rebuild_text_index(case["documents"])
            for check in expected["checks"]:
                assert len(db.tql(check["query"])) == check["count"], case["name"]
        else:
            raise AssertionError(f"未知共享契约操作: {case['operation']}")
    db.close()
    try:
        db.build_text_index()
    except RuntimeError:
        pass
    else:
        raise AssertionError("关闭后的文本构建错误必须透传")
    reader = triviumdb.TriviumDB(path, dim=contract["setup"]["dimension"],
        load_text_index=True, access_mode="read_only")
    checks = next(case for case in contract["cases"] if case["operation"] == "text_rebuild")["expected"]["checks"]
    for check in checks:
        assert len(reader.tql(check["query"])) == check["count"]
    try:
        reader.rebuild_text_index([])
    except triviumdb.ReadOnlyError:
        pass
    else:
        raise AssertionError("只读句柄的文本重建必须返回写入权限错误")
    reader.close()


def run_query_budget_contract(root: str) -> None:
    for dtype in ["f32", "f16", "u64"]:
        runtime = triviumdb.ResourceRuntime(memory_limit_mb=40, maintenance_memory_limit_mb=8,
            max_threads=2, max_active_tasks=1)
        db = triviumdb.TriviumDB(os.path.join(root, f"query-budget-{dtype}.tdb"), dim=3,
            dtype=dtype, memory_limit_mb=8, auto_build_quiver=False, runtime=runtime)
        try:
            db.insert_with_id(1, [1, 2, 3], {})
            db.set_memory_limit(32)
            for operation in [lambda: db.search_exact([1, 2, 3], 1),
                              lambda: db.search_batch([[1, 2, 3]], top_k=1)]:
                try:
                    operation()
                except triviumdb.ResourceBudgetError as error:
                    assert error.resource == "memory_bytes"
                    assert error.requested > 0
                    assert error.used == error.limit == 32 * 1024 * 1024
                else:
                    raise AssertionError("查询工作区必须在预算耗尽时被拒绝")
            assert runtime.stats()["active_tasks"] == 0
            db.set_memory_limit(8)
            assert db.search_batch([[1, 2, 3]], top_k=1)[0][0].id == 1
        finally:
            db.close()


def run_search_boundaries(root: str) -> None:
    file = os.path.join(root, "search-order.tdb")
    options = dict(dim=2, auto_build_quiver=False)
    db = triviumdb.TriviumDB(file, **options)
    for node_id in [1, 2, 3]:
        db.insert_with_id(node_id, [1, 0], {"value": node_id})
    db.delete(1)
    db.insert_with_id(10, [1, 0], {"value": 10})

    def check(database: triviumdb.TriviumDB) -> None:
        hits = database.search_advanced([1, 0], top_k=1, recall_k=1, rerank_k=1,
                                       expand_depth=0, min_score=0, force_brute_force=True)
        assert [hit.id for hit in hits] == [2]

    check(db)
    db.update_payload(2, {"value": 2**64 - 1})
    db.update_payload(3, {"value": 2**64 - 2})
    db.update_payload(10, {"value": 2**53 + 1})
    db.create_ordered_index("value")
    query = "FIND {value: {$exists: true}} RETURN * ORDER BY _.value ASC"
    assert [row.row["_"]["id"] for row in db.tql(query)] == [10, 3, 2]
    db.close()
    reopened = triviumdb.TriviumDB(file, **options)
    try:
        check(reopened)
        assert [row.row["_"]["id"] for row in reopened.tql(query)] == [10, 3, 2]
    finally:
        reopened.close()


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="triviumdb-python-public-api-") as root:
        run_search_boundaries(root)
        run_lifecycle_contract(root)
        run_resource_contract(root)
        run_shared_contract(root)
        run_error_contract(root)
        run_kernel_boundaries(root)
        run_transaction_contract(root)
        run_write_boundaries(root)
        run_index_graph_boundaries(root)
        run_query_numeric_indexes(root)
        run_tql_semantics(root)
        run_tql_boundaries(root)
        run_text_recovery_contract(root)
        run_query_budget_contract(root)
        path = os.path.join(root, "api.tdb")
        db = triviumdb.TriviumDB(path, dim=2)
        db.insert_with_id(1, [1.0, 0.0], {"kind": "a", "score": 1, "tenant": "t", "region": "r"})
        db.insert_with_id(2, [0.9, 0.1], {"kind": "b", "score": 2, "tenant": "t", "region": "r"})
        db.link(1, 2, "next", 1.0)

        stages: list[str] = []

        class FullHook:
            def on_pre_search(self, query, ctx):
                stages.append("pre")
                return query

            def on_custom_recall(self, query, ctx):
                stages.append("custom")
                return None

            def on_post_recall(self, hits, ctx):
                stages.append("post_recall")
                return hits

            def on_pre_graph_expand(self, hits, ctx):
                stages.append("pre_graph")
                return hits

            def on_rerank(self, hits, ctx):
                stages.append("rerank")
                return hits

            def on_post_search(self, hits, ctx):
                stages.append("post")
                return hits

        db.set_hook(FullHook())
        db.search_with_context([1.0, 0.0], top_k=2, expand_depth=1, min_score=0.0)
        assert {"pre", "custom", "post_recall", "pre_graph", "rerank", "post"} <= set(stages)
        db.clear_hook()

        class BrokenHook:
            def on_pre_search(self, query, ctx):
                raise RuntimeError("python-hook-failure")

        db.set_hook(BrokenHook())
        try:
            db.search_with_context([1.0, 0.0], top_k=2)
            raise AssertionError("Hook 异常必须传播")
        except RuntimeError as error:
            assert "python-hook-failure" in str(error)
        db.clear_hook()

        db.create_index("kind")
        db.create_ordered_index("score")
        db.create_composite_index(["tenant", "region"])
        db.create_bitmap_index("region")
        indexes = db.index_info()
        assert len(indexes) == 4
        assert next(item for item in indexes if item["kind"] == "composite")["fields"] == [
            "tenant",
            "region",
        ]

        storage = db.storage_info()
        assert storage["database_format_current"] == 10
        assert storage["property_index_format"] == 8
        assert storage["node_count"] == 2

        vector_prepared = db.prepare_tql(
            "SEARCH VECTOR [$x, $y] TOP 1 RETURN *"
        )
        assert vector_prepared.parameter_names() == ["x", "y"]
        vector_rows = db.execute_prepared_tql(vector_prepared, {"x": 1, "y": 0.0})
        assert vector_rows[0].row["_"].get("id") == 1

        prepared = db.prepare_tql(
            'FIND {kind: "a"} AS seed WITH seed RETURN seed, $bonus + 1 AS score'
        )
        assert prepared.parameter_names() == ["bonus"]
        prepared_rows = db.execute_prepared_tql(prepared, {"bonus": 4})
        assert len(prepared_rows) == 1
        assert prepared_rows[0].row["score"] == 5
        try:
            db.execute_prepared_tql(prepared, {})
        except RuntimeError as error:
            assert error.code == "TDB_MISSING_QUERY_PARAMETER"
            assert error.details == {"parameter": "bonus"}
        else:
            raise AssertionError("缺少 Prepared TQL 参数必须失败")

        rows = db.tql(
            "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed shortest_paths seed TO [2] "
            "LABEL next AS route WITH route RETURN path(route) AS path"
        )
        assert rows[0].row["path"] == [1, 2]

        try:
            db.tql_mut('FIND {kind: "a"} RETURN *')
        except RuntimeError as error:
            assert error.code == "TDB_API_MIGRATION_REQUIRED"
            assert error.details["removed_api"] == "tql_mut(read query)"
        else:
            raise AssertionError("tql_mut 读查询必须失败")
        db.close()


if __name__ == "__main__":
    main()
