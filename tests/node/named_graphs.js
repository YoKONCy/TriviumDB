// 实际原生绑定的命名图与独立边契约，预期来自共享样例。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')

function runNamedGraphs(root, { TriviumDB } = require('../..')) {
  const fixture = JSON.parse(fs.readFileSync(path.join(__dirname, '../contracts/named_graph_cases.json'), 'utf8'))
  for (const dtype of ['f32', 'f16', 'u64']) {
    const file = path.join(root, `named-graphs-${dtype}.tdb`)
    let db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    // 文档JSON入口提供完整u64节点身份，普通Node数值API沿既有安全整数契约。
    const document = {
      source_id: `fixture://${dtype}`, source_revision: 'v1', parser_id: 'plain-v1', content: '',
      root: { id: fixture.nodes[0], vector: [1, 0] },
      blocks: fixture.nodes.slice(1).map((id, index) => ({ key: String(index), kind: 'body', start_byte: 0, end_byte: 0, node: { id, vector: [1, 0] } })),
    }
    const documentSnapshot = JSON.parse(db.documentRequest(JSON.stringify({ operation: 'put', document })))
    const request = command => JSON.parse(db.graphRequest(JSON.stringify(command)))
    const snapshots = fixture.graphs.map(graph => request({ operation: 'put', graph }))
    const result = request({ operation: 'scope', scope: { graph_ids: ['词级图', '事件图'] } })
    assert.deepEqual(result.members, fixture.expected.union_members)
    assert.deepEqual(result.edges, fixture.expected.union_edges)
    assert.deepEqual(request({ operation: 'traverse', scope: { graph_ids: ['词级图'] }, start: fixture.nodes[0], max_depth: 2 }).members, fixture.expected.single_members)
    const defaultGraph = request({ operation: 'scope', scope: { include_default: true } })
    assert.equal(defaultGraph.edges.length, 2)
    assert.ok(defaultGraph.edges.every(edge => edge.graph_id === null && edge.label === 'tdb.document.contains'))
    const changed = request({ operation: 'edit', graph_id: '词级图', expected_revision: snapshots[0].revision, edits: [{ operation: 'remove_member', node_id: fixture.nodes[1] }] })
    assert.deepEqual(changed.members, [fixture.nodes[0]])
    assert.deepEqual(changed.edges, [])
    assert.deepEqual(request({ operation: 'get', graph_id: '事件图' }), snapshots[1])
    assert.throws(() => request({ operation: 'delete', graph_id: '词级图', expected_revision: snapshots[0].revision }), error => {
      assert.equal(error.code, 'TDB_GRAPH_REVISION_MISMATCH')
      assert.deepEqual(error.details, { graph_id: '词级图', expected: snapshots[0].revision, actual: changed.revision })
      return true
    })
    db.close()
    db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    assert.deepEqual(request({ operation: 'get', graph_id: '词级图' }), changed)
    assert.deepEqual(request({ operation: 'get', graph_id: '事件图' }), snapshots[1])
    assert.deepEqual(request({ operation: 'delete', graph_id: '词级图', expected_revision: changed.revision }), { deleted: true })
    assert.equal(db.nodeCount(), 3)
    const revisedDocument = { ...document, source_revision: 'v2', blocks: document.blocks.slice(1) }
    db.documentRequest(JSON.stringify({ operation: 'put', document: revisedDocument, expected_revision: documentSnapshot.revision }))
    const remaining = request({ operation: 'get', graph_id: '事件图' })
    assert.deepEqual(remaining.members, [fixture.nodes[0], fixture.nodes[2]])
    assert.deepEqual(remaining.edges, [])
    db.close()
    db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    assert.equal(request({ operation: 'get', graph_id: '词级图' }), null)
    assert.throws(() => request({ operation: 'get', graph_id: '词级图', limits: { max_return_bytes: 0 } }), error => {
      assert.equal(error.code, 'QueueFull')
      assert.equal(error.details.resource, 'named_graph_return_bytes')
      return true
    })
    assert.deepEqual(request({ operation: 'get', graph_id: '事件图' }), remaining)
    db.close()
  }
}

module.exports = { runNamedGraphs }
