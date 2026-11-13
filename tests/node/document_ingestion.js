// 实际制品的解析、证据回映射、批次确认和持久化契约。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const { createHash } = require('node:crypto')

function runIngestion(root, TriviumDB) {
  const fixture = JSON.parse(fs.readFileSync(path.join(__dirname, '../contracts/document_ingestion_cases.json'), 'utf8'))
  for (const dtype of ['f32', 'f16', 'u64']) {
    const file = path.join(root, `ingestion-${dtype}.tdb`)
    let db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    const request = command => JSON.parse(db.documentRequest(JSON.stringify(command)))
    for (const item of fixture.formats) {
      const input = { source_id: item.name, source_revision: 'file-1', content: item.raw, format: item.format ?? 'auto', media_type: item.media_type ?? null }
      const parsed = request({ operation: 'parse', input })
      assert.equal(parsed.text, item.text)
      assert.equal(parsed.source.content_hash, `sha256:${createHash('sha256').update(item.raw).digest('hex')}`)
      const rootNode = { id: fixture.document_id, vector: [1, 0], payload: { kind: item.name } }
      const snapshot = request({ operation: 'ingest', input, root: rootNode })
      assert.equal(snapshot.content, item.text)
      assert.deepEqual(snapshot.blocks, parsed.blocks)
      const evidence = request({ operation: 'resolve', reference: { document_id: fixture.document_id, revision: snapshot.revision, start_byte: 0, end_byte: Buffer.byteLength(item.text) } })
      assert.equal(evidence.text, item.text)
      assert(evidence.source_spans.length > 0)
      for (const span of evidence.source_spans) assert.equal(span.text, Buffer.from(item.raw).subarray(span.source_start, span.source_end).toString())
      db.close()
      db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
      assert.deepEqual(request({ operation: 'get', document_id: fixture.document_id }), snapshot)
      request({ operation: 'delete', document_id: fixture.document_id, expected_revision: snapshot.revision })
    }
    if (dtype === 'f16') {
      assert.throws(() => request({ operation: 'ingest', input: { source_id: 'bad.txt', source_revision: '1', content: 'bad' }, root: { id: fixture.document_id, vector: [65536, 0] } }), error => {
        assert.equal(error.code, 'TDB_INVALID_VECTOR')
        assert.deepEqual(error.target, { field: '/root/vector/0' })
        assert.equal(error.write.outcome, 'not_committed')
        return true
      })
      assert.deepEqual(request({ operation: 'list_uploads' }), [])
    }
    const begin = { operation: 'begin_upload', upload: { upload_id: fixture.upload_id, source_id: 'stream.txt', source_revision: '2', parser_id: 'host-v1', content_hash: `sha256:${createHash('sha256').update(fixture.stream_text).digest('hex')}`, root: { id: fixture.document_id, vector: [1, 0] } } }
    const status = request(begin)
    assert.equal(status.state, 'pending')
    assert.deepEqual(request(begin), status)
    const batch = { operation: 'append_upload', upload_id: fixture.upload_id, batch: { sequence: 0, content: 'new ', blocks: [{ key: 'p', kind: 'paragraph', start_byte: 0, end_byte: 7, node: { id: fixture.block_id, vector: [0, 1], payload: { key: 'p' } } }] } }
    const ack = request(batch)
    assert.equal(ack.next_sequence, 1)
    assert.deepEqual(request(batch), ack)
    assert.equal(db.nodeCount(), 0)
    db.close()
    db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    assert.deepEqual(request({ operation: 'upload_status', upload_id: fixture.upload_id }), ack)
    request({ operation: 'append_upload', upload_id: fixture.upload_id, batch: { sequence: 1, content: '猫' } })
    const published = request({ operation: 'publish_upload', upload_id: fixture.upload_id })
    assert.equal(published.content, fixture.stream_text)
    assert.equal(db.nodeCount(), 2)
    const result = request({ operation: 'search_vector', query: {}, vector: [0, 1], options: { strategy: 'exact' } })
    assert.equal(result.groups.length, 1)
    assert.equal(result.groups[0].document_id, fixture.document_id)
    assert.equal(result.groups[0].hits.length, 1)
    assert.equal(result.groups[0].hits[0].node_id, fixture.block_id)
    assert.equal(result.groups[0].hits[0].score, dtype === 'u64' ? 128 : 1)
    assert.equal(result.groups[0].hits[0].evidence.text, fixture.stream_text)
    assert.deepEqual(request({ operation: 'publish_upload', upload_id: fixture.upload_id }), published)
    assert.throws(() => request({ ...batch, batch: { ...batch.batch, content: 'changed' } }), error => error.code === 'TDB_DOCUMENT_UPLOAD_CONFLICT')
    db.close()
    db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false, accessMode: 'readOnly' })
    assert.deepEqual(request({ operation: 'get', document_id: fixture.document_id }), published)
    assert.equal(request({ operation: 'upload_status', upload_id: fixture.upload_id }).visible, true)
    assert.throws(() => request({ operation: 'abort_upload', upload_id: fixture.upload_id }), error => error.code === 'TDB_READ_ONLY')
    db.close()
  }
}

module.exports = { runIngestion }
