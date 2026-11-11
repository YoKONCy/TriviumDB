"""普通写入口的成员诊断、宿主协议与持久化事实契约。"""

from copy import deepcopy
import json
from pathlib import Path

import triviumdb
from read_arguments import node
from write_outcomes import expect_write


def invalid(action, field):
    try:
        action()
    except (TypeError, ValueError, OverflowError, UnicodeError, IndexError) as error:
        assert error.code == "TDB_INVALID_INPUT" and error.schema_version == 1, (field, error)
        assert error.target == {"field": field} and error.stage is None, (field, error.target)
        assert error.write == {"phase": "validation", "outcome": "not_committed"}
        assert set(error.details) == {"reason"} and error.details["reason"]
        return error
    raise AssertionError(f"参数转换必须拒绝 {field}")


def rejected_values(kind, field):
    if kind == "id":
        return [(value, field) for value in ("7", None, [], object(), set(), -1, 1 << 64)]
    if kind == "string":
        return [(value, field) for value in (1, [], object(), "\ud800")]
    if kind == "number":
        return [(value, field) for value in ("1", [], {}, object(), complex(1, 2))]
    if kind == "vector":
        return [(None, field), ("10", field), ([1, "x"], f"/{field}/1"), ([1, object()], f"/{field}/1")]
    if kind == "vectors":
        return [(None, field), (((1, 0), (0, 1)), field), ([[1, 0], None], f"/{field}/1"),
                ([[1, 0], ["x", 1]], f"/{field}/1/0")]
    if kind == "ids":
        return [(None, field), ([7, "x"], f"/{field}/1"), ([7, -1], f"/{field}/1")]
    if kind == "payloads":
        return [(None, field), (({}, {}), field), ([{}, {"a/b~c": [1, object()]}], f"/{field}/1/a~1b~0c/1")]
    assert kind == "json"
    values = [(object(), field), ({1: "键"}, field), ({"\ud800": 1}, field)]
    for value in (object(), float("nan"), float("inf"), 1 << 64, "\ud800"):
        values.append(({"a/b~c": [1, value]}, f"/{field}/a~1b~0c/1"))
    values.append(({"词.行[]\x00😀": object()}, f"/{field}/词.行[]\x00😀"))
    return values


def facts(db):
    return [node(db.get(id)) for id in db.all_node_ids()]


def run_crud_arguments(root):
    fixture = json.loads((Path(__file__).parents[1] / "contracts/crud_arguments.json").read_text(encoding="utf-8"))
    assert len(fixture["operations"]) == 15
    for dtype in ("f32", "f16", "u64"):
        directory = Path(root) / f"crud-arguments-{dtype}"
        file = str(directory / "database.tdb")
        options = dict(dim=2, dtype=dtype, auto_build_quiver=False)
        db = triviumdb.TriviumDB(file, **options)
        try:
            for value in fixture["before"]:
                db.insert_with_id(value["id"], value["vector"], value["payload"])
            db.upsert_edge(7, 11, "route", 0.5, {"part": "前段"})
            assert facts(db) == fixture["before"]
            wal = Path(file + ".wal").read_bytes()
            for operation in fixture["operations"]:
                method = getattr(db, operation["python"])
                for entry in operation["fields"]:
                    for value, field in rejected_values(entry["kind"], entry["field"]):
                        args = deepcopy(operation.get("python_args", operation["args"]))
                        args[entry.get("python_index", entry["index"])] = value
                        invalid(lambda: method(*args), field)
                assert facts(db) == fixture["before"], operation["python"]
                assert Path(file + ".wal").read_bytes() == wal, operation["python"]
            for action, field in (
                (lambda: db.batch_insert([[1, 0]], []), "payloads"),
                (lambda: db.batch_insert_with_ids([], [[1, 0]], [{}]), "ids"),
                (lambda: db.batch_insert_with_ids([19], [[1, 0]], []), "payloads"),
            ):
                invalid(action, field)
            for action in (
                lambda: db.batch_insert([[1, 0], [1]], [{}, {}]),
                lambda: db.batch_insert_with_ids([19, 23], [[1, 0], [1]], [{}, {}]),
            ):
                expect_write(action, "TDB_DIMENSION_MISMATCH", {"expected": 2, "got": 1},
                             {"phase": "validation", "outcome": "not_committed"})
                assert facts(db) == fixture["before"]
                assert Path(file + ".wal").read_bytes() == wal
            if dtype == "u64":
                for value in (-1, 1 << 64, 0.5):
                    invalid(lambda: db.update_vector([1, value], 7), "/vector/1")

            def reject_reentry():
                for action, write in ((lambda: db.get(7), None),
                                      (lambda: db.delete(7), {"phase": "admission", "outcome": "not_committed"})):
                    try:
                        action()
                    except triviumdb.TriviumDBError as error:
                        assert error.code == "TDB_REENTRANT_ACCESS" and error.write == write
                        assert error.target is None and error.details == {"operation": "python_callback"}
                    else:
                        raise AssertionError("参数协议内的同库访问必须拒绝")

            class Id:
                def __index__(self):
                    reject_reentry()
                    return 7

            class Component:
                def __index__(self):
                    reject_reentry()
                    return 1

                def __float__(self):
                    reject_reentry()
                    return 1.0

            class Sequence:
                def __len__(self):
                    reject_reentry()
                    return 1 << 61

                def __getitem__(self, index):
                    return (Component(), 0)[index]

            db.update_vector(Sequence(), Id())

            class FailedHint(Sequence):
                def __len__(self):
                    raise TypeError("长度提示异常沿既有协议忽略")

            db.update_vector(FailedHint(), 7)
            cause = RuntimeError("宿主 cause")
            sentinel = TypeError("宿主参数原异常")
            sentinel.code = "HOST_CONVERSION"
            sentinel.target = {"host": True}
            sentinel.write = {"host": "原字段"}

            class BadId:
                def __index__(self):
                    raise sentinel from cause

            class BadComponent:
                def __index__(self):
                    raise sentinel from cause

                def __float__(self):
                    raise sentinel from cause

            class BadSequence(Sequence):
                def __getitem__(self, index):
                    raise sentinel from cause

            class BadIterator(Sequence):
                def __iter__(self):
                    raise sentinel from cause

            for action in (lambda: db.delete(BadId()), lambda: db.update_vector([BadComponent(), 0], 7),
                           lambda: db.update_vector(BadSequence(), 7), lambda: db.update_vector(BadIterator(), 7)):
                try:
                    action()
                except TypeError as error:
                    assert error is sentinel and error.__cause__ is cause
                    assert error.code == "HOST_CONVERSION" and error.target == {"host": True}
                    assert error.write == {"host": "原字段"} and not hasattr(error, "details")
                else:
                    raise AssertionError("宿主转换异常必须原样传播")
            payloads = [{}, {}]

            class MutatingComponent(Component):
                def mutate(self):
                    if len(payloads) == 2:
                        payloads.append({})
                        vectors.append([1, 1])
                    return 1

                def __index__(self):
                    return self.mutate()

                def __float__(self):
                    return float(self.mutate())

            vectors = [[MutatingComponent(), 0], [0, 1]]
            db.batch_insert_with_ids([19, 23], vectors, payloads)
            assert facts(db) == fixture["before"] + [
                dict(id=19, vector=[1, 0], payload={}, num_edges=0, edges=[]),
                dict(id=23, vector=[0, 1], payload={}, num_edges=0, edges=[]),
            ]
            assert db.delete_many_atomic([19, 23]) == 2
            assert facts(db) == fixture["before"]
            for explicit_ids in (False, True):
                for field in ("vectors", "payloads"):
                    class ShrinkingComponent(Component):
                        def shrink(self):
                            containers[field].clear()
                            return 1

                        def __index__(self):
                            return self.shrink()

                        def __float__(self):
                            return float(self.shrink())

                    containers = dict(vectors=[[ShrinkingComponent(), 0], [0, 1]], payloads=[{}, {}])
                    wal = Path(file + ".wal").read_bytes()
                    def shrinking_batch():
                        if explicit_ids:
                            return db.batch_insert_with_ids([19, 23], containers["vectors"], containers["payloads"])
                        return db.batch_insert(containers["vectors"], containers["payloads"])
                    invalid(shrinking_batch, f"/{field}/1")
                    assert facts(db) == fixture["before"]
                    assert Path(file + ".wal").read_bytes() == wal
            # 成功的协议验证会写入相同值，之后的失败必须保持当下 WAL 字节。
            wal = Path(file + ".wal").read_bytes()
            held_error = invalid(lambda: db.delete_many_atomic([7, "x"]), "/ids/1")
            assert Path(file + ".wal").read_bytes() == wal
            automatic = [db.insert([1, 1], {"临时": 1})]
            automatic.extend(db.batch_insert([[1, 0], [0, 1]], [{"临时": 2}, {"临时": 3}]))
            db.insert_with_id(29, [0, 1], {"临时": 4})
            db.batch_insert_with_ids((19, 23), [(1, 0), (0, 1)], [fixture["after"][2]["payload"], {}])
            db.upsert_with_id(23, (1, 1), {"name": "替换"})
            db.update_payload(7, {"name": "待更新", "count": 1})
            db.patch_payload(7, {"$inc": {"count": 1}})
            db.compare_and_set_payload_field(7, "name", "待更新", "更新")
            db.update_vector((1, 1), 7)
            db.link(7, 11)
            db.unlink(7, 11, "related")
            db.upsert_edge(7, 11, "route", metadata={"part": "新增"})
            db.update_edge(7, 11, "route", 0.25, {"part": "更新"})
            db.delete(29)
            assert db.delete_many_atomic(tuple(automatic)) == 3
            assert facts(db) == fixture["after"]
            held = [db.get(value["id"]) for value in fixture["after"]]
            db.flush()
        finally:
            db.close()
        moved = directory.with_name(directory.name + "-moved")
        directory.rename(moved)
        assert held_error.target == {"field": "/ids/1"}
        assert [node(value) for value in held] == fixture["after"]
        for _ in range(2):
            reopened = triviumdb.TriviumDB(str(moved / "database.tdb"), **options)
            try:
                assert facts(reopened) == fixture["after"]
                reopened.flush()
            finally:
                reopened.close()
