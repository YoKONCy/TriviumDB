"""实际绑定中的命名图、共享成员、独立边和修订契约。"""
import json
from pathlib import Path
import triviumdb


def run_named_graphs(root: str) -> None:
    fixture = json.loads((Path(__file__).parent / "../contracts/named_graph_cases.json").read_text(encoding="utf-8"))
    for dtype in ("f32", "f16", "u64"):
        file = str(Path(root) / f"named-graphs-{dtype}.tdb")
        db = triviumdb.TriviumDB(file, dim=2, dtype=dtype, auto_build_quiver=False)
        for node in fixture["nodes"]:
            db.insert_with_id(int(node), [1, 0], {"id": node})

        def request(command: dict) -> object:
            return json.loads(db.graph_request(json.dumps(command, ensure_ascii=False)))

        snapshots = [request({"operation": "put", "graph": graph}) for graph in fixture["graphs"]]
        result = request({"operation": "scope", "scope": {"graph_ids": ["词级图", "事件图"]}})
        assert result["members"] == fixture["expected"]["union_members"]
        assert result["edges"] == fixture["expected"]["union_edges"]
        assert request({"operation": "traverse", "scope": {"graph_ids": ["词级图"]}, "start": fixture["nodes"][0], "max_depth": 2})["members"] == fixture["expected"]["single_members"]
        assert db.get_edges(int(fixture["nodes"][0])) == []
        changed = request({"operation": "edit", "graph_id": "词级图", "expected_revision": snapshots[0]["revision"], "edits": [{"operation": "remove_member", "node_id": fixture["nodes"][1]}]})
        assert changed["members"] == [fixture["nodes"][0]]
        assert changed["edges"] == []
        assert request({"operation": "get", "graph_id": "事件图"}) == snapshots[1]
        try:
            request({"operation": "delete", "graph_id": "词级图", "expected_revision": snapshots[0]["revision"]})
        except triviumdb.TriviumDBError as error:
            assert error.code == "TDB_GRAPH_REVISION_MISMATCH"
            assert error.details == {"graph_id": "词级图", "expected": snapshots[0]["revision"], "actual": changed["revision"]}
        else:
            raise AssertionError("旧修订必须拒绝")
        db.close()
        db = triviumdb.TriviumDB(file, dim=2, dtype=dtype, auto_build_quiver=False)
        assert request({"operation": "get", "graph_id": "词级图"}) == changed
        assert request({"operation": "get", "graph_id": "事件图"}) == snapshots[1]
        assert request({"operation": "delete", "graph_id": "词级图", "expected_revision": changed["revision"]}) == {"deleted": True}
        assert db.node_count() == 3
        db.delete(int(fixture["nodes"][1]))
        remaining = request({"operation": "get", "graph_id": "事件图"})
        assert remaining["members"] == [fixture["nodes"][0], fixture["nodes"][2]]
        assert remaining["edges"] == []
        db.close()
        db = triviumdb.TriviumDB(file, dim=2, dtype=dtype, auto_build_quiver=False)
        assert request({"operation": "get", "graph_id": "词级图"}) is None
        try:
            request({"operation": "get", "graph_id": "词级图", "limits": {"max_return_bytes": 0}})
        except triviumdb.TriviumDBError as error:
            assert error.code == "QueueFull"
            assert error.details["resource"] == "named_graph_return_bytes"
        else:
            raise AssertionError("空值响应同样需要返回字节额度")
        assert request({"operation": "get", "graph_id": "事件图"}) == remaining
        db.close()
