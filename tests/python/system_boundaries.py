"""实际 wheel 的共享内存准入和关联集合删除契约。"""
import json
from pathlib import Path
import triviumdb


def run_system_boundaries(root):
    contract = json.loads((Path(__file__).parents[1] / "contracts/system_boundaries.json").read_text(encoding="utf-8"))
    for dtype in contract["dtypes"]:
        runtime = triviumdb.ResourceRuntime(memory_limit_mb=contract["runtime_mb"], maintenance_memory_limit_mb=contract["maintenance_mb"], max_threads=1)
        db = triviumdb.TriviumDB(str(Path(root) / f"system-budget-{dtype}.tdb"), dim=2, dtype=dtype, runtime=runtime, memory_limit_mb=contract["database_mb"], auto_build_quiver=False)
        for id in contract["nodes"]:
            db.insert_with_id(id, [1, 0], {"id": id})
        for source, target, label in contract["edges"]:
            db.link(source, target, label, 1)
        db.set_memory_limit(contract["blocked_quota_mb"])
        assert runtime.stats()["available_foreground_bytes"] == 0
        for action in [lambda: db.search_exact([1, 0], 1), lambda: db.search_graph_first([1, 0], contract["nodes"], 1), lambda: db.neighbors(1, 2), lambda: db.reachable(1), lambda: db.reachable_detailed(1), lambda: db.query_subgraph(1), lambda: db.tql("MATCH (n) RETURN n"), lambda: db.tql_mut("MATCH (n) SET n.touched == true")]:
            try:
                action()
            except triviumdb.ResourceBudgetError as error:
                assert error.code == "QueueFull"
                assert error.details["resource"] == "memory_bytes"
                assert error.details["requested"] > 0
                assert error.details["used"] == error.details["limit"]
            else:
                raise AssertionError("共享前台余额为零时必须拒绝工作区分配")
            assert runtime.stats()["reserved_workspace_bytes"] == 0
        db.set_memory_limit(contract["database_mb"])
        assert db.search_graph_first([1, 0], [4, 3, 2, 1, 1], 1)[0].payload == {"id": contract["winner_id"]}
        assert [hit.target_id for hit in db.reachable(1)] == contract["reachable_ids"]
        assert runtime.stats()["reserved_workspace_bytes"] == 0
        db.close()
        for index, query in enumerate(contract["delete_queries"]):
            file = str(Path(root) / f"system-delete-{dtype}-{index}.tdb")
            db = triviumdb.TriviumDB(file, dim=2, dtype=dtype, auto_build_quiver=False)
            for id in contract["nodes"]:
                db.insert_with_id(id, [1, 0], {"id": id})
            for source, target, label in contract["edges"]:
                db.link(source, target, label, 1)
            assert db.tql_mut(query)["affected"] == 2
            assert db.all_node_ids() == contract["remaining_ids"]
            assert [(edge.target_id, edge.label) for edge in db.get_edges(3)] == [(4, "keep")]
            db.close()
            reader = triviumdb.TriviumDB(file, dim=2, dtype=dtype, access_mode="read_only")
            assert reader.all_node_ids() == contract["remaining_ids"]
            assert reader.get_payload(3) == {"id": 3}
            assert [(edge.target_id, edge.label) for edge in reader.get_edges(3)] == [(4, "keep")]
            reader.close()
