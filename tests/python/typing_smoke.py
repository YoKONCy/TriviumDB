from triviumdb import EdgeDirection, SearchHit, Transaction, TriviumDB

direction: EdgeDirection = "both"
db = TriviumDB("typing.tdb", dim=2, dtype="f32", access_mode="read_write")
hits: list[SearchHit] = db.search_advanced(
    [1.0, 0.0],
    max_edges_per_node=10,
    min_edge_weight=0.25,
    edge_direction=direction,
)
transaction: Transaction = db.transaction()
transaction.rollback()
db.close()
