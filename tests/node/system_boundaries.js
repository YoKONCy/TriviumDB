'use strict'
const assert = require('node:assert/strict')
const path = require('node:path')
const contract = require('../contracts/system_boundaries.json')

function runSystemBoundaries(root, { TriviumDB, ResourceRuntime }) {
  for (const dtype of contract.dtypes) {
    const runtime = new ResourceRuntime({ memoryLimitMb: contract.runtime_mb, maintenanceMemoryLimitMb: contract.maintenance_mb, maxThreads: 1 })
    const db = new TriviumDB(path.join(root, `system-budget-${dtype}.tdb`), { dim: 2, dtype, runtime, memoryLimitMb: contract.database_mb, autoBuildQuiver: false })
    for (const id of contract.nodes) db.insertWithId(id, [1, 0], { id })
    for (const [source, target, label] of contract.edges) db.link(source, target, label, 1)
    db.setMemoryLimit(contract.blocked_quota_mb)
    assert.equal(runtime.stats().available_foreground_bytes, 0)
    for (const action of [
      () => db.searchExact([1, 0], 1), () => db.searchGraphFirst([1, 0], contract.nodes, 1),
      () => db.neighbors(1, 2), () => db.reachable(1), () => db.reachableDetailed(1), () => db.querySubgraph(1),
      () => db.tql('MATCH (n) RETURN n'), () => db.tqlMut('MATCH (n) SET n.touched == true'),
    ]) {
      assert.throws(action, error => error.code === 'QueueFull' && error.details.resource === 'memory_bytes' && error.details.requested > 0 && error.details.used === error.details.limit)
      assert.equal(runtime.stats().reserved_workspace_bytes, 0)
    }
    db.setMemoryLimit(contract.database_mb)
    assert.deepEqual(db.searchGraphFirst([1, 0], [4, 3, 2, 1, 1], 1)[0].payload, { id: contract.winner_id })
    assert.deepEqual(db.reachable(1).map(hit => hit.targetId), contract.reachable_ids)
    assert.equal(runtime.stats().reserved_workspace_bytes, 0)
    db.close()
    for (const [index, query] of contract.delete_queries.entries()) {
      const file = path.join(root, `system-delete-${dtype}-${index}.tdb`)
      const db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
      for (const id of contract.nodes) db.insertWithId(id, [1, 0], { id })
      for (const [source, target, label] of contract.edges) db.link(source, target, label, 1)
      assert.equal(db.tqlMut(query).affected, 2)
      assert.deepEqual(db.allNodeIds(), contract.remaining_ids)
      assert.deepEqual(db.getEdges(3).map(edge => [edge.targetId, edge.label]), [[4, 'keep']])
      db.close()
      const reader = new TriviumDB(file, { dim: 2, dtype, accessMode: 'readOnly' })
      assert.deepEqual(reader.allNodeIds(), contract.remaining_ids)
      assert.deepEqual(reader.getPayload(3), { id: 3 })
      assert.deepEqual(reader.getEdges(3).map(edge => [edge.targetId, edge.label]), [[4, 'keep']])
      reader.close()
    }
  }
}
module.exports = { runSystemBoundaries }
