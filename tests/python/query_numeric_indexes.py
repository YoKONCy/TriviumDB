"""Python 安装制品的完整整数、事务最终键和索引构建预算契约。"""
from pathlib import Path
import math
import triviumdb


def run_query_numeric_indexes(root: str) -> None:
    from cognitive_boundaries import run_cognitive_boundaries
    run_cognitive_boundaries(root)
    from search_boundaries import run_search_boundaries
    run_search_boundaries(root)
    file = Path(root) / "query-numeric.tdb"
    db = triviumdb.TriviumDB(str(file), dim=2, auto_build_quiver=False)
    try:
        node_id = 2**64 - 3
        db.insert_with_id(node_id, [1, 0], {"lo": 2**63 + 1, "hi": 2**63 + 2, "tiny": 1e-20})
        row = db.tql("MATCH (n) RETURN n.id AS id, n.lo AS lo, n.hi AS hi")[0].row
        assert row == {"id": node_id, "lo": 2**63 + 1, "hi": 2**63 + 2}
        assert len(db.tql("MATCH (n) WHERE n.id > 0 RETURN n")) == 1
        assert db.tql("MATCH (n) WHERE n.lo == n.hi RETURN n") == []
        assert len(db.tql("MATCH (n) WHERE n.lo < n.hi RETURN n")) == 1
        prepared = db.prepare_tql("MATCH (n) WHERE n.id == $id RETURN n.id AS id")
        assert db.execute_prepared_tql(prepared, {"id": node_id})[0].row == {"id": node_id}
        assert db.tql("MATCH (n) WHERE n.tiny > 0 AND n.tiny == 0.0 RETURN n") == []
        assert db.tql("MATCH (n) RETURN 1.0 / n.tiny AS quotient")[0].row == {"quotient": 1e20}
        assert db.tql("MATCH (n) RETURN n.lo + 0 AS unchanged")[0].row == {"unchanged": 2**63 + 1}
    finally:
        db.close()

    file = Path(root) / "unique-final-keys.tdb"
    db = triviumdb.TriviumDB(str(file), dim=2, auto_build_quiver=False)
    try:
        for node_id, key in [(1, "a"), (2, "b")]:
            db.insert_with_id(node_id, [1, 0], {"tenant": "t", "key": key})
        db.create_unique_composite_index(["tenant", "key"])
        tx = db.transaction()
        tx.update_payload(1, {"tenant": "t", "key": "b"})
        tx.update_payload(1, {"tenant": "t", "key": "c"})
        tx.commit()
        before = Path(str(file) + ".wal").read_bytes()
        try:
            db.insert_with_id(3, [1, 0], {"tenant": "t", "key": "b"})
        except (RuntimeError, ValueError):
            pass
        else:
            raise AssertionError("事务中的临时冲突不能移除其他节点的唯一键所有权")
        assert Path(str(file) + ".wal").read_bytes() == before
        db.close()
        db = triviumdb.TriviumDB(str(file), dim=2, auto_build_quiver=False)
        assert db.get_payload(1) == {"tenant": "t", "key": "c"}
        assert db.get_payload(2) == {"tenant": "t", "key": "b"}
    finally:
        db.close()

    file = Path(root) / "index-ddl-budget.tdb"
    db = triviumdb.TriviumDB(str(file), dim=2, auto_build_quiver=False)
    try:
        for node_id in range(1, 513):
            db.insert_with_id(node_id, [1, 0], {"key": f"{node_id}-" + "x" * 3072})
        before = db.estimated_memory()
        db.set_memory_limit(math.ceil(before / (1024 * 1024)))
        wal = Path(str(file) + ".wal").read_bytes()
        try:
            db.create_index("key")
        except (RuntimeError, ValueError, MemoryError):
            pass
        else:
            raise AssertionError("索引键的新增堆内容超过配额余量时必须拒绝")
        assert db.index_info() == []
        assert db.estimated_memory() == before
        assert Path(str(file) + ".wal").read_bytes() == wal
    finally:
        db.close()
