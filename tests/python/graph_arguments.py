"""图读取实参的字段、回调准入和实际文件资源契约。"""

import json
import threading
from pathlib import Path
import triviumdb
from query_arguments import check_input_error


def run_graph_arguments(root):
    case = json.loads((Path(__file__).parents[1] / "contracts/graph_arguments.json").read_text(encoding="utf-8"))
    paths = [{**row, "steps": [{key: value for key, value in step.items()
                              if key not in ("edge_source", "edge_target")} for step in row["steps"]]}
             for row in case["paths"]]
    subgraph = dict(nodes=[{"id": node["id"], "payload": node["payload"]} for node in case["nodes"]],
                    edges=case["edges"], visited_nodes=3, traversed_edges=2, truncated=False)
    for dtype in ("f32", "f16", "u64"):
        directory = Path(root) / f"graph-arguments-{dtype}"
        directory.mkdir()
        file = str(directory / "database.tdb")
        options = dict(dim=2, dtype=dtype, auto_build_quiver=False)
        db = triviumdb.TriviumDB(file, **options)
        try:
            for node in case["nodes"]:
                db.insert_with_id(node["id"], node["vector"], node["payload"])
            for edge in case["edges"]:
                db.upsert_edge(edge["source_id"], edge["target_id"], edge["label"], edge["weight"], edge["metadata"])
            before = Path(file + ".wal").read_bytes()
            # 数量回调采用公开默认预算，深度二覆盖完整三节点链。
            valid_numbers = dict(id=7, depth=2, min_depth=1, max_depth=2,
                                 max_visited_nodes=10000, max_results=10000, max_edges=50000)

            def reject_writes(expected_code="TDB_REENTRANT_ACCESS"):
                for action in (
                    lambda: db.insert([1, 0], {}),
                    lambda: db.insert_with_id(17, [1, 0], {}),
                    lambda: db.upsert_with_id(7, [1, 0], {}),
                    lambda: db.batch_insert([[1, 0]], [{}]),
                    lambda: db.batch_insert_with_ids([17], [[1, 0]], [{}]),
                    lambda: db.delete(7),
                    lambda: db.delete_many_atomic([7]),
                    lambda: db.update_payload(7, {}),
                    lambda: db.patch_payload(7, {"$set": {"name": "变化"}}),
                    lambda: db.update_vector([0, 1], 7),
                    lambda: db.compare_and_set_payload_field(7, "name", "源", "变化"),
                    lambda: db.link(7, 13),
                    lambda: db.upsert_edge(7, 13, "route", 1, {}),
                    lambda: db.update_edge(7, 11, "route", weight=1),
                    lambda: db.unlink(7, 11, "route"),
                ):
                    try:
                        action()
                    except triviumdb.TriviumDBError as error:
                        assert error.schema_version == 1 and error.stage is None and error.target is None
                        assert error.code == expected_code
                        assert error.details == ({"resource": "python_database"} if expected_code == "TDB_BUSY"
                                                 else {"operation": "python_callback"})
                        assert error.write == {"phase": "admission", "outcome": "not_committed"}
                    else:
                        raise AssertionError("图参数回调中的同库写入必须拒绝")

            def verify(name, result, both=False):
                if name == "neighbors":
                    assert result == case["neighbors"]
                elif name == "reachable":
                    actual = [dict(source_id=row.source_id, target_id=row.target_id, depth=row.depth,
                                   path=row.path, steps=[{"from": step.from_id, "to": step.to_id,
                                   "label": step.label, "weight": step.weight, "metadata": step.metadata}
                                   for step in row.steps]) for row in result]
                    assert actual == paths
                elif name == "reachable_detailed":
                    assert result == dict(results=case["paths"], **case["stats_both" if both else "stats"])
                else:
                    assert name == "query_subgraph"
                    assert result == (subgraph | {"traversed_edges": 3} if both else subgraph)

            for name in ("neighbors", "reachable", "reachable_detailed", "query_subgraph"):
                method = getattr(db, name)
                base = dict(id=7, depth=2, labels=["route"]) if name == "neighbors" else dict(
                    id=7, max_depth=2, labels=["route"])
                fields = ("id", "depth") if name == "neighbors" else (
                    "id", "min_depth", "max_depth", "max_visited_nodes", "max_results", "max_edges")
                for field in fields:
                    for value, kind in ((None, TypeError), ("1", TypeError), (1.5, TypeError),
                                        ([], TypeError), ({}, TypeError), (-1, OverflowError),
                                        (1 << 100, OverflowError)):
                        try:
                            method(**(base | {field: value}))
                        except kind as error:
                            check_input_error(error, field)
                        else:
                            raise AssertionError(f"图参数必须拒绝 {name}/{field}/{value!r}")
                    calls = []
                    class Index:
                        def __index__(self):
                            reject_writes()
                            calls.append(True)
                            return valid_numbers[field]
                    verify(name, method(**(base | {field: Index()})))
                    assert calls == [True]
                    cause = RuntimeError("图整数转换原因")
                    sentinel = TypeError("图整数转换失败")
                    sentinel.__cause__ = cause
                    class BrokenIndex:
                        def __index__(self):
                            raise sentinel
                    try:
                        method(**(base | {field: BrokenIndex()}))
                    except TypeError as error:
                        assert error is sentinel and error.__cause__ is cause
                        assert not hasattr(error, "code")
                    else:
                        raise AssertionError("必须保留图参数宿主异常")
                calls = []
                class Labels:
                    def __len__(self):
                        reject_writes()
                        calls.append("len")
                        return 1
                    def __getitem__(self, index):
                        if index == 0:
                            reject_writes()
                            calls.append("item")
                            return "route"
                        raise IndexError
                verify(name, method(**(base | {"labels": Labels()})))
                assert "len" in calls and "item" in calls
                for value, field in (("route", "labels"), (["route", 1], "/labels/1"),
                                     (["\ud800"], "/labels/0")):
                    try:
                        method(**(base | {"labels": value}))
                    except (TypeError, UnicodeError) as error:
                        check_input_error(error, field)
                    else:
                        raise AssertionError(f"图标签必须拒绝 {field}")
                class HugeHint(Labels):
                    def __len__(self):
                        return 1 << 61
                class FailedHint(Labels):
                    def __len__(self):
                        raise TypeError("长度提示沿现有策略忽略")
                verify(name, method(**(base | {"labels": HugeHint()})))
                verify(name, method(**(base | {"labels": FailedHint()})))
                for value, field in ((1, "labels"), ("route", "labels"), ([1], "/labels/0"), ({"route": 1}, "labels")):
                    try:
                        method(**(base | {"labels": value}))
                    except TypeError as error:
                        check_input_error(error, field)
                    else:
                        raise AssertionError("标签必须为字符串序列")
                cause = RuntimeError("图标签序列原因")
                sentinel = TypeError("图标签序列失败")
                sentinel.__cause__ = cause
                class BrokenLabels:
                    def __len__(self):
                        return 1
                    def __getitem__(self, index):
                        raise sentinel
                try:
                    method(**(base | {"labels": BrokenLabels()}))
                except TypeError as error:
                    assert error is sentinel and error.__cause__ is cause
                    assert not hasattr(error, "code")
                else:
                    raise AssertionError("必须保留标签序列原异常")
                if name != "neighbors":
                    for value, kind in ((None, TypeError), (1, TypeError), ([], TypeError),
                                        ("sideways", ValueError), ("\ud800", UnicodeEncodeError)):
                        try:
                            method(**(base | {"direction": value}))
                        except kind as error:
                            check_input_error(error, "direction")
                        else:
                            raise AssertionError("图方向必须符合枚举契约")
                    # 双向展开在中间节点还扫描回源边，扫描边数为三。
                    verify(name, method(**(base | {"direction": "both"})), both=True)
                verify(name, method(**base))
            completed = threading.Event()
            failures = []
            def competing_writer():
                try:
                    reject_writes("TDB_BUSY")
                except BaseException as error:
                    failures.append(error)
                finally:
                    completed.set()
            class ThreadLabels:
                def __len__(self):
                    return 1
                def __getitem__(self, index):
                    if index != 0:
                        raise IndexError
                    worker = threading.Thread(target=competing_writer, daemon=True)
                    worker.start()
                    # 等待真实争用请求结束，超时仅用于终止测试挂起。
                    assert completed.wait(10), "争用线程必须完成"
                    worker.join(10)
                    assert not worker.is_alive() and failures == []
                    return "route"
            verify("query_subgraph", db.query_subgraph(7, max_depth=2, labels=ThreadLabels()))
            assert completed.is_set() and failures == []
            assert db.neighbors(7) == [11]
            defaults = db.reachable(7)
            assert [(row.source_id, row.target_id, row.depth, row.path,
                     [(step.from_id, step.to_id, step.label, step.weight, step.metadata)
                      for step in row.steps]) for row in defaults] == [
                (7, 11, 1, [7, 11], [(7, 11, "route", 0.5, {"part": "前段"})])]
            assert db.reachable_detailed(7) == dict(results=case["paths"][:1], visited_nodes=2,
                traversed_edges=1, peak_frontier_size=1, depth_reached=1, truncated=False)
            assert db.query_subgraph(7) == dict(nodes=subgraph["nodes"][:2], edges=case["edges"][:1],
                visited_nodes=2, traversed_edges=1, truncated=False)
            assert Path(file + ".wal").read_bytes() == before
            db.update_vector([0, 1], 7)
        finally:
            db.close()
        moved = directory.with_name(directory.name + "-moved")
        assert directory.is_absolute() and moved.parent == directory.parent
        directory.rename(moved)
        try:
            db.neighbors(7)
        except triviumdb.TriviumDBError as error:
            assert error.code == "TDB_DATABASE_CLOSED"
        else:
            raise AssertionError("旧图对象必须保持关闭状态")
        reopened = triviumdb.TriviumDB(str(moved / "database.tdb"), **options)
        try:
            assert reopened.all_node_ids() == [7, 11, 13]
            for expected in case["nodes"]:
                node = reopened.get(expected["id"])
                assert node.vector == ([0, 1] if node.id == 7 else expected["vector"])
                assert node.payload == expected["payload"]
                assert [(edge.target_id, edge.label, edge.weight, edge.metadata) for edge in node.edges] == [
                    (edge["target_id"], edge["label"], edge["weight"], edge["metadata"])
                    for edge in case["edges"] if edge["source_id"] == node.id]
            assert reopened.query_subgraph(7, max_depth=2, labels=["route"]) == subgraph
            assert reopened.compare_and_set_payload_field(7, "name", "源", "已更新") is None
            assert reopened.get_payload(7) == {"name": "已更新"}
            assert reopened.delete_many_atomic([11, 13]) == 2
            assert reopened.all_node_ids() == [7]
            assert reopened.get_edges(7) == []
        finally:
            reopened.close()
        final = triviumdb.TriviumDB(str(moved / "database.tdb"), **options)
        try:
            assert final.all_node_ids() == [7]
            node = final.get(7)
            assert node.vector == [0, 1] and node.payload == {"name": "已更新"} and node.edges == []
        finally:
            final.close()
