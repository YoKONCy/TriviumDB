"""真实 wheel 的 TQL 规划、执行来源和拒绝后完整事实。"""

import json
from pathlib import Path

import triviumdb


def run_query_sources(root):
    cases = json.loads((Path(__file__).parents[1] / "contracts/query_execution_diagnostics.json").read_text(encoding="utf-8"))["cases"]
    for dtype in ("f32", "f16", "u64"):
        file = Path(root) / f"query-sources-{dtype}.tdb"
        options = dict(dim=2, dtype=dtype, auto_build_quiver=False,
                       max_query_rows=1, row_overflow="throw")
        db = triviumdb.TriviumDB(str(file), **options)
        try:
            db.insert_with_id(7, [1, 0], {"kind": "alpha"})
            db.insert_with_id(8, [0, 1], {"kind": "beta"})
            db.create_unique_index("kind")
            session = triviumdb.ConsoleSession('{"readOnly":false}')
            before = Path(str(file) + ".wal").read_bytes()
            for case in cases:
                actions = []
                if case["mutation"]:
                    actions.append(lambda: db.tql_mut(case["query"]))
                else:
                    actions.append(lambda: db.tql(case["query"]))
                    if case["prepare_error"]:
                        actions.append(lambda: db.prepare_tql(case["query"]))
                    else:
                        prepared = db.prepare_tql(case["query"])
                        actions.append(lambda: db.execute_prepared_tql(prepared, {}))
                request = json.dumps({"version": 1, "requestId": case["name"], "command": {
                    "operation": "mutate" if case["mutation"] else "query", "query": case["query"],
                }})
                actions.append(lambda: db.console_request(session, request))
                for action in actions:
                    try:
                        action()
                    except Exception as error:
                        assert error.schema_version == 1
                        assert error.code == case["code"], (case, error)
                        assert error.stage == case["stage"], (case, error)
                        assert error.target == {"query_span": case["span"]}, (case, error.target)
                        assert error.write == case.get("write"), (case, error.write)
                        assert isinstance(error.details, dict) and error.details
                    else:
                        raise AssertionError(f"必须拒绝该查询 {case['name']}")
                assert Path(str(file) + ".wal").read_bytes() == before, case
            prepared = db.prepare_tql("FIND {kind: 'alpha'} RETURN $x AS value")
            assert [row.row for row in db.execute_prepared_tql(prepared, {"x": 42})] == [{"value": 42}]
        finally:
            db.close()
        reopened = triviumdb.TriviumDB(str(file), **options)
        try:
            assert reopened.all_node_ids() == [7, 8]
            for node_id, vector, kind in ((7, [1, 0], "alpha"), (8, [0, 1], "beta")):
                node = reopened.get(node_id)
                assert node.vector == vector
                assert node.payload == {"kind": kind}
                assert node.edges == []
        finally:
            reopened.close()
