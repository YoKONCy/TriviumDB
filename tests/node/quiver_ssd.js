// SDK共享独立方向样例，核对完整分数、ID、驻留切换、重开与拒绝契约。
const assert = require('node:assert/strict')
const path = require('node:path')
const fixture = require('../contracts/quiver_ssd_cases.json')

function runQuiverSsd(root, { TriviumDB } = require('../..')) {
  for (const dtype of ['f32', 'f16']) {
    const file = path.join(root, `quiver-ssd-${dtype}.tdb`)
    let db = new TriviumDB(file, { dim: fixture.dimension, dtype, autoBuildQuiver: false })
    const vector = direction => [...Array(fixture.dimension - 1).fill(0), direction]
    const request = command => JSON.parse(db.quiverRequest(JSON.stringify(command)))
    for (const node of fixture.nodes) {
      db.documentRequest(JSON.stringify({ operation: 'put', document: {
        source_id: `fixture://${node.id}`, source_revision: 'v1', parser_id: 'plain-utf8-v1', content: '',
        root: { id: node.id, vector: vector(node.direction), payload: { id: node.id } }, blocks: []
      } }))
    }
    const stats = request({ operation: 'configure', options: fixture.options })
    assert.equal(stats.metadata.dimension, fixture.dimension)
    assert.equal(stats.residency, 'ssd')
    const search = { ...fixture.search, vector: vector(1) }
    assert.deepEqual(request(search), { hits: fixture.expected_hits })
    const first = request({ operation: 'stats' })
    assert.ok(first.physical_read_calls > 0)
    assert.ok(first.physical_read_bytes >= first.cold_read_bytes)
    if (first.io_mode === 'direct') {
      assert.ok(first.io_alignment_bytes > 0 && first.io_alignment_bytes <= 65536)
      assert.ok(first.io_memory_alignment_bytes > 0 && first.io_memory_alignment_bytes <= 65536)
      assert.equal(typeof first.io_alignment_detected, 'boolean')
      assert.ok(first.io_window_bytes > 0 && first.io_window_bytes <= 65536)
    } else {
      assert.equal(first.io_alignment_bytes, 0)
      assert.equal(first.io_window_bytes, 0)
    }
    assert.deepEqual(request(search), { hits: fixture.expected_hits })
    const cached = request({ operation: 'stats' })
    assert.equal(cached.cold_read_calls, first.cold_read_calls)
    assert.equal(cached.physical_read_calls, first.physical_read_calls)
    assert.equal(cached.physical_read_bytes, first.physical_read_bytes)
    request({ operation: 'configure', options: { ...fixture.options, cache_bytes: 0 } })
    assert.deepEqual(request(search), { hits: fixture.expected_hits })
    const windowSearch = { ...fixture.window_search, vector: vector(-1) }
    assert.deepEqual(request(windowSearch), { hits: fixture.expected_window_hits })
    const beforeWindow = request({ operation: 'stats' })
    assert.deepEqual(request(windowSearch), { hits: fixture.expected_window_hits })
    assert.deepEqual(request(windowSearch), { hits: fixture.expected_window_hits })
    const reused = request({ operation: 'stats' })
    assert.equal(reused.cache_limit_bytes, 0)
    assert.equal(reused.cache_used_bytes, 0)
    assert.equal(reused.cold_read_calls, beforeWindow.cold_read_calls + 2)
    if (reused.io_mode === 'direct') {
      assert.equal(reused.physical_read_calls, beforeWindow.physical_read_calls)
      assert.equal(reused.physical_read_bytes, beforeWindow.physical_read_bytes)
      assert.equal(reused.window_reuse_hits, beforeWindow.window_reuse_hits + 2)
      assert.equal(reused.window_reuse_bytes, beforeWindow.window_reuse_bytes + 2 * reused.metadata.record_bytes)
    }
    request({ operation: 'configure', options: { ...fixture.options, residency: 'hot' } })
    const hot = request({ operation: 'stats' })
    assert.equal(hot.io_active, false)
    assert.equal(hot.io_window_bytes, 0)
    assert.equal(hot.io_alignment_bytes, 0)
    assert.deepEqual(request(search), { hits: fixture.expected_hits })
    assert.equal(request({ operation: 'stats' }).cold_read_calls, hot.cold_read_calls)
    for (const encoding of fixture.encodings) {
      for (const precision of fixture.query_precisions) {
        for (const residency of ['ssd', 'hot']) {
          const configured = request({ operation: 'configure', options: {
            ...fixture.options, encoding, query_precision: precision,
            residency, cache_bytes: 0, use_for_ann: true
          } })
          assert.equal(configured.metadata.encoding, encoding)
          assert.equal(configured.query_precision, precision)
          assert.equal(configured.residency, residency)
          assert.equal(configured.routing_shared, true)
          assert.deepEqual(request(search), { hits: fixture.expected_hits })
        }
      }
    }
    request({ operation: 'configure', options: fixture.options })
    assert.throws(() => request({ operation: 'configure', options: { query_precision: 'fp16' } }),
      error => error.code === 'TDB_INVALID_INPUT')
    assert.throws(() => request({ operation: 'configure', options: { io_depth: 0 } }), error => error.code === 'TDB_INVALID_INPUT')
    assert.throws(() => request({ operation: 'stats', unexpected: true }), error => error.code === 'TDB_INVALID_INPUT')
    db.close()
    db = new TriviumDB(file, { dim: fixture.dimension, dtype, accessMode: 'readOnly', autoBuildQuiver: false })
    request({ operation: 'configure', options: fixture.options })
    assert.deepEqual(request(search), { hits: fixture.expected_hits })
    assert.equal(request({ operation: 'close' }), null)
    assert.equal(request({ operation: 'stats' }), null)
    db.close()
    assert.throws(() => request({ operation: 'stats' }), error => error.code === 'TDB_DATABASE_CLOSED')
  }
}
module.exports = { runQuiverSsd }
