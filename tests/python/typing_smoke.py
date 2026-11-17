from typing import Any
from triviumdb import EdgeDirection, ResourceRuntime, restore_snapshot, verify_snapshot, SearchHit, Transaction, TriviumDB

direction: EdgeDirection = "both"
runtime = ResourceRuntime(memory_limit_mb=64, max_threads=2, max_active_tasks=4)
stats: dict[str, Any] = runtime.stats()
db = TriviumDB("typing.tdb", dim=2, dtype="f32", access_mode="read_write",
               memory_limit_mb=8, runtime=runtime)
hits: list[SearchHit] = db.search_advanced(
    [1.0, 0.0],
    max_edges_per_node=10,
    min_edge_weight=0.25,
    edge_direction=direction,
    exact=True,
)
exact_hits: list[SearchHit] = db.search([1.0, 0.0], exact=True)
exact_batch: list[list[SearchHit]] = db.search_batch([[1.0, 0.0]], exact=True)
transaction: Transaction = db.transaction()
transaction.rollback()
state: dict[str, Any] = db.shutdown_status()
instance: dict[str, Any] = db.instance_stats()
manifest: dict[str, Any] = db.create_snapshot("typing-snapshot", "typing-generation")
verified: dict[str, Any] = verify_snapshot("typing-snapshot", runtime)
restored: dict[str, Any] = restore_snapshot("typing-snapshot", "typing-restored", runtime)
closed: bool = db.close_timeout(1000)
db.close()
