"""安装制品的索引覆盖、完整整数和图持久化边界。"""
from pathlib import Path
import triviumdb


def run_index_graph_boundaries(root: str) -> None:
    file = Path(root) / "index-graph.tdb"
    options = {"dim": 2, "auto_build_quiver": False}
    db = triviumdb.TriviumDB(str(file), **options)
    try:
        records = [{"tenant": "a", "v": 1}, {"tenant": "a", "v": True},
                   {"tenant": "a", "v": None}, {"tenant": "a"},
                   {"tenant": "a", "v": [1]}, {"tenant": "a", "v": {"x": 1}}]
        for node_id, payload in enumerate(records, 1):
            db.insert_with_id(node_id, [1, 0], payload)
        db.create_ordered_index("v")
        db.create_composite_index(["tenant", "v"])

        def check(database):
            for query in ['FIND {tenant: "a"} RETURN *',
                          'FIND {tenant: "a"} RETURN * ORDER BY _.v ASC']:
                assert sorted(row.row["_"]["id"] for row in database.tql(query)) == [1, 2, 3, 4, 5, 6]

        check(db)
        db.create_unique_index("email")
        db.drop_index("email")
        assert db.list_unique_indexes() == []
        db.update_payload(1, {**records[0], "email": "same"})
        db.update_payload(2, {**records[1], "email": "same"})
        db.link(1, 2, "first", 1)
        db.link(1, 2, "second", 2)
        db.unlink(1, 2, "first")
        db.unlink(1, 2)
        assert db.validate_graph()["valid"]
        before = Path(str(file) + ".wal").read_bytes()
        try:
            db.link(1, 2, "x" * 65536, 1)
        except (ValueError, RuntimeError):
            pass
        else:
            raise AssertionError("超长标签必须在 WAL 前拒绝")
        assert Path(str(file) + ".wal").read_bytes() == before
        db.close()
        db = triviumdb.TriviumDB(str(file), **options)
        check(db)
        assert db.get_payload(1)["email"] == db.get_payload(2)["email"] == "same"
        assert db.validate_graph()["valid"]
    finally:
        db.close()

    file = Path(root) / "composite-exact.tdb"
    db = triviumdb.TriviumDB(str(file), **options)
    try:
        db.create_unique_composite_index(["tenant", "v"])
        for node_id, value in enumerate([2**53, 2**53 + 1, 2**64 - 1], 1):
            db.insert_with_id(node_id, [1, 0], {"tenant": "a", "v": value})
        db.close()
        db = triviumdb.TriviumDB(str(file), **options)
        for node_id, value in enumerate([2**53, 2**53 + 1, 2**64 - 1], 1):
            assert db.get_payload(node_id) == {"tenant": "a", "v": value}
    finally:
        db.close()

    file = Path(root) / "full-ids.tdb"
    sample = Path(__file__).parent.parent / "fixtures/numeric_ids/wal_v3.bin"
    triviumdb.TriviumDB(str(file), dim=2, auto_build_quiver=False).close()
    Path(str(file) + ".wal").write_bytes(sample.read_bytes())
    db = triviumdb.TriviumDB(str(file), dim=2, auto_build_quiver=False)
    try:
        expected = [1, 2**53 - 1, 2**53, 2**53 + 1]
        assert sorted(db.all_node_ids()) == expected
        assert sorted(hit.id for hit in db.search_exact([1, 0], 4)) == expected
        for node_id in expected:
            assert db.get(node_id).id == node_id
            assert db.get_payload(node_id) == {"group": "fixture", "exact_id": str(node_id)}
    finally:
        db.close()
