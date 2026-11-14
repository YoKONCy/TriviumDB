// 实际npm包的压缩文档分组、评分精度与来源证据契约。
const assert = require('node:assert/strict')
const path = require('node:path')
const fixture = require('../contracts/document_compressed_cases.json')

function runCompressed(root, TriviumDB) {
  for (const dtype of ['f32', 'f16']) {
    const db = new TriviumDB(path.join(root, `document-compressed-${dtype}.tdb`), { dim: fixture.dimension, dtype, autoBuildQuiver: false })
    const direction = value => [...Array(fixture.dimension - 1).fill(0), value]
    const request = command => JSON.parse(db.documentRequest(JSON.stringify(command)))
    const snapshots = new Map()
    for (const row of fixture.documents) {
      const snapshot = request({ operation: 'put', document: {
        source_id: `notes://${row.id}`, source_revision: 'v1', parser_id: 'plain-v1', content: 'alpha',
        root: { id: row.id, vector: direction(row.root_direction), payload: { vault: row.vault } },
        blocks: [{ key: 'body', kind: 'body', start_byte: 0, end_byte: 5, node: {
          id: row.node_id, vector: direction(row.direction), payload: { tags: ['evidence'], state: 'ready' }
        } }]
      } })
      snapshots.set(row.id, snapshot)
    }
    const vector = direction(1)
    vector[0] = 0.1
    for (const encoding of fixture.encodings) {
      for (const precision of fixture.precisions) {
        for (const residency of ['ssd', 'hot']) {
          db.quiverRequest(JSON.stringify({ operation: 'configure', options: {
            encoding, query_precision: precision, residency, cache_bytes: 0,
            io_depth: 1, io_mode: 'auto', use_for_ann: false
          } }))
          for (const testCase of fixture.cases) {
            const options = { rerank: 'compressed', initial_candidates: 1, candidate_limit: 64, ef_search: 64 }
            const result = request({ operation: testCase.operation, query: { max_documents: 3, hits_per_document: 1, ...testCase.query }, vector,
              options: testCase.operation === 'search_hybrid' ? { vector: options } : options })
            assert.deepEqual(result.groups.map(group => group.document_id), testCase.documents)
            assert.deepEqual(result.groups.map(group => group.hits.map(hit => hit.node_id)), testCase.nodes.map(node => [node]))
            assert.equal(result.retrieval.rerank, 'compressed')
            assert.equal(result.retrieval.strategy, 'ann')
            assert.equal(result.retrieval.approximate, true)
            assert.deepEqual(result.retrieval.compression, { encoding, query_precision: precision, residency })
            assert.equal(result.retrieval.stop, testCase.stop)
            for (const group of result.groups) {
              const snapshot = snapshots.get(group.document_id)
              const hit = group.hits[0]
              const expected = fixture.positive_scores[dtype][precision] * fixture.signs[hit.node_id]
              assert.ok(Math.abs(hit.scores.vector_score - expected) < 1e-6)
              assert.equal(hit.evidence.text, 'alpha')
              assert.deepEqual(hit.evidence.reference, { document_id: group.document_id, revision: snapshot.revision, start_byte: 0, end_byte: 5 })
              assert.equal(hit.evidence.source_id, snapshot.source_id)
              assert.equal(hit.evidence.source_revision, 'v1')
              if (testCase.operation === 'search_hybrid') {
                const dense = fixture.dense_ranks[hit.node_id]
                const text = fixture.text_ranks[hit.node_id]
                assert.equal(hit.scores.vector_rank, dense)
                assert.equal(hit.scores.text_rank, text)
                assert.ok(Math.abs(hit.scores.text_score - Math.log(8 / 7)) < 1e-6)
                assert.ok(Math.abs(hit.score - (60 / (60 + dense) + 60 / (60 + text))) < 1e-6)
              } else {
                assert.ok(Math.abs(hit.score - expected) < 1e-6)
                assert.equal(hit.scores.text_score, null)
              }
              assert.equal(group.score, hit.score)
            }
          }
        }
      }
    }
    assert.throws(() => request({ operation: 'search_hybrid', query: { text: 'alpha' }, vector,
      options: { vector: { strategy: 'exact', rerank: 'compressed' } } }), error => {
      assert.equal(error.code, 'TDB_INVALID_INPUT')
      assert.deepEqual(error.target, { field: 'options.vector.rerank' })
      return true
    })
    db.close()
  }
}

module.exports = { runCompressed }
