// 文档协议复用共享独立样例，核对完整来源、证据与跨重开修订。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')

function runDocuments(root, { TriviumDB } = require('../..')) {
  require('./document_ingestion').runIngestion(root, TriviumDB)
  runRetrieval(root, TriviumDB)
  runPositions(root, TriviumDB)
  runFields(root, TriviumDB)
  runVectors(root, TriviumDB)
  const fixture = JSON.parse(fs.readFileSync(path.join(__dirname, '../contracts/document_cases.json'), 'utf8'))
  for (const dtype of ['f32', 'f16', 'u64']) {
    const file = path.join(root, `documents-${dtype}.tdb`)
    let db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    const request = command => JSON.parse(db.documentRequest(JSON.stringify(command)))
    const snapshot = request({ operation: 'put', document: fixture.document })
    assert.equal(snapshot.document_id, '9007199254740993')
    assert.equal(snapshot.source_id, 'vault://notes/cat.md')
    assert.equal(snapshot.source_revision, 'file-v1')
    assert.equal(snapshot.content, fixture.document.content)
    assert.equal(snapshot.content_hash, fixture.expected.content_hash)
    assert.deepEqual(snapshot.blocks.map(block => block.node_id), [null, '9007199254740995', '9007199254740997', '9007199254740999'])
    const reference = { document_id: snapshot.document_id, revision: snapshot.revision, start_byte: 10, end_byte: 23 }
    const evidence = request({ operation: 'resolve', reference })
    assert.deepEqual(Object.fromEntries(Object.keys(fixture.expected.evidence).map(key => [key, evidence[key]])), fixture.expected.evidence)
    assert.deepEqual(evidence.reference, reference)
    assert.throws(() => request({ operation: 'resolve', reference: { ...reference, start_byte: 11 } }), error => {
      assert.equal(error.code, 'TDB_INVALID_INPUT')
      assert.deepEqual(error.target, { field: 'reference' })
      return true
    })
    db.close()
    db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    assert.deepEqual(request({ operation: 'get', document_id: snapshot.document_id }), snapshot)
    const changed = structuredClone(fixture.document)
    changed.source_revision = 'file-v2'
    const updated = request({ operation: 'put', document: changed, expected_revision: snapshot.revision })
    assert.equal(updated.source_revision, 'file-v2')
    assert.notEqual(updated.revision, snapshot.revision)
    assert.throws(() => request({ operation: 'resolve', reference }), error => {
      assert.equal(error.code, 'TDB_DOCUMENT_REVISION_MISMATCH')
      assert.deepEqual(error.details, { document_id: snapshot.document_id, expected: snapshot.revision, actual: updated.revision })
      return true
    })
    assert.deepEqual(request({ operation: 'delete', document_id: snapshot.document_id, expected_revision: updated.revision }), { deleted: true })
    db.close()
    db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    assert.equal(request({ operation: 'get', document_id: snapshot.document_id }), null)
    assert.equal(db.nodeCount(), 0)
    db.close()
  }
}

function runRetrieval(root, TriviumDB) {
  const fixture = JSON.parse(fs.readFileSync(path.join(__dirname, '../contracts/document_retrieval_cases.json'), 'utf8'))
  for (const dtype of ['f32', 'f16', 'u64']) {
    const file = path.join(root, `document-retrieval-${dtype}.tdb`)
    let db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    const request = command => JSON.parse(db.documentRequest(JSON.stringify(command)))
    const snapshots = fixture.documents.map(document => request({ operation: 'put', document }))
    const result = request({ operation: 'search', query: fixture.query })
    for (const key of ['corpus_documents', 'corpus_units', 'matched_units']) assert.equal(result[key], fixture.expected[key])
    assert.deepEqual(result.groups.map(group => group.document_id), fixture.expected.document_ids)
    assert.deepEqual(result.groups.map(group => group.hits[0].node_id), fixture.expected.node_ids)
    assert.deepEqual(result.groups.map(group => group.hits[0].evidence.text), fixture.expected.texts)
    for (const [index, group] of result.groups.entries()) {
      assert.equal(group.revision, snapshots[index].revision)
      assert.equal(group.source_id, snapshots[index].source_id)
      const hit = group.hits[0]
      const text = fixture.expected.texts[index]
      assert.deepEqual(hit.evidence, {
        reference: { document_id: fixture.expected.document_ids[index], revision: snapshots[index].revision, start_byte: 0, end_byte: text.length },
        source_id: fixture.documents[index].source_id, source_revision: 'v1', parser_id: 'plain-v1',
        content_hash: fixture.expected.content_hashes[index], text, line: 1, column: 1, end_line: 1, end_column: text.length + 1,
      })
      assert.equal(group.score, hit.score)
      assert.ok(group.score > 0)
    }
    assert.deepEqual(request({ operation: 'search', query: { ...fixture.query, use_cache: false } }), result)
    assert.deepEqual(request({ operation: 'clear_index' }), { cleared: true })
    assert.deepEqual(request({ operation: 'search', query: fixture.query }), result)
    const contextQuery = { seeds: [{ reference: result.groups[0].hits[0].evidence.reference, block_key: 'first' }], before: 0, after: 1 }
    const context = request({ operation: 'context', query: contextQuery })
    assert.equal(context.length, 1)
    assert.deepEqual(context[0].block_keys, ['second', 'first'])
    assert.equal(context[0].windows.length, 1)
    assert.equal(context[0].windows[0].text, 'alpha alpha\nalpha alpha')
    assert.deepEqual(context[0].windows[0].reference, { ...contextQuery.seeds[0].reference, end_byte: 23 })
    for (const [query, code] of [
      [{ ...fixture.query, limits: { max_candidates: 1 } }, 'QueueFull'],
      [{ ...fixture.query, expected_revisions: { '100': 'stale' } }, 'TDB_DOCUMENT_REVISION_MISMATCH'],
    ]) {
      assert.throws(() => request({ operation: 'search', query }), error => {
        assert.equal(error.code, code)
        assert.equal(error.write, undefined)
        return true
      })
    }
    db.close()
    db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    assert.deepEqual(request({ operation: 'search', query: fixture.query }), result)
    assert.deepEqual(request({ operation: 'context', query: contextQuery }), context)
    assert.deepEqual(request({ operation: 'delete', document_id: '100', expected_revision: snapshots[0].revision }), { deleted: true })
    assert.deepEqual(request({ operation: 'search', query: fixture.query }).groups.map(group => group.document_id), ['200'])
    db.close()
  }
}

function runPositions(root, TriviumDB) {
  const fixture = JSON.parse(fs.readFileSync(path.join(__dirname, '../contracts/document_positions_cases.json'), 'utf8'))
  for (const dtype of ['f32', 'f16', 'u64']) {
    const file = path.join(root, `document-positions-${dtype}.tdb`)
    let db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    const request = command => JSON.parse(db.documentRequest(JSON.stringify(command)))
    const snapshots = fixture.documents.map(document => request({ operation: 'put', document }))
    const results = fixture.cases.map(test => {
      const query = { level: 'document', include_matches: true, max_documents: 10, hits_per_document: 10, ...test.query }
      const result = request({ operation: 'search', query })
      assert.equal(result.analyzer_id, fixture.analyzer_id)
      assert.equal(result.groups.length, 1, test.name)
      const group = result.groups[0]
      assert.equal(group.document_id, query.document_ids[0])
      assert.equal(group.hits.length, 1)
      const hit = group.hits[0]
      assert.equal(hit.node_id, test.node_id)
      const index = fixture.documents.findIndex(document => document.root.id === group.document_id)
      assert.ok(index >= 0)
      assert.deepEqual(hit.matches, test.matches.map(match => ({
        evidence: {
          reference: { document_id: group.document_id, revision: snapshots[index].revision, start_byte: match.start_byte, end_byte: match.end_byte },
          source_id: fixture.documents[index].source_id, source_revision: 'v1', parser_id: 'plain-v1', content_hash: fixture.hashes[index],
          text: match.text, line: match.line, column: match.column, end_line: match.end_line, end_column: match.end_column,
        }, terms: match.terms,
      })), test.name)
      assert.deepEqual(request({ operation: 'search', query: { ...query, use_cache: false } }), result)
      return result
    })
    const context = request({ operation: 'context', query: {
      seeds: [{ reference: results[0].groups[0].hits[0].matches[1].evidence.reference, block_key: 'second' }], before: 0, after: 0,
    } })
    assert.equal(context[0].windows[0].text, 'alpha beta beta')
    const base = { level: 'document', text: 'alpha beta', match_mode: 'phrase', include_matches: true }
    for (const [extra, code, field] of [
      [{ analyzer_id: 'unknown' }, 'TDB_INVALID_INPUT', 'query.analyzer_id'],
      [{ max_gap: 1 }, 'TDB_INVALID_INPUT', 'query.max_gap'],
      [{ limits: { max_matches: 1 } }, 'QueueFull', null],
    ]) {
      assert.throws(() => request({ operation: 'search', query: { ...base, ...extra } }), error => {
        assert.equal(error.code, code)
        assert.equal(error.write, undefined)
        if (field === null) assert.equal(error.details.resource, 'document_matches')
        else assert.deepEqual(error.target, { field })
        return true
      })
    }
    db.close()
    db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    for (const [index, test] of fixture.cases.entries()) {
      const query = { level: 'document', include_matches: true, max_documents: 10, hits_per_document: 10, ...test.query }
      assert.deepEqual(request({ operation: 'search', query }), results[index])
    }
    const changed = structuredClone(fixture.documents[0])
    changed.source_revision = 'v2'
    changed.content = changed.content.replaceAll('Alpha', 'Delta').replaceAll('alpha', 'delta')
    const updated = request({ operation: 'put', document: changed, expected_revision: snapshots[0].revision })
    assert.deepEqual(request({ operation: 'search', query: base }).groups, [])
    const current = request({ operation: 'search', query: { ...base, text: 'delta beta' } })
    assert.equal(current.groups[0].revision, updated.revision)
    assert.deepEqual(current.groups[0].hits[0].matches.map(match => match.evidence.text), ['Delta, beta', 'delta beta'])
    assert.deepEqual(request({ operation: 'delete', document_id: '1000', expected_revision: updated.revision }), { deleted: true })
    assert.deepEqual(request({ operation: 'search', query: { ...base, text: 'delta beta' } }).groups, [])
    db.close()
  }
}

function runFields(root, TriviumDB) {
  const fixtureText = fs.readFileSync(path.join(__dirname, '../contracts/document_fields_cases.json'), 'utf8')
  const fixture = JSON.parse(fixtureText)
  // 样例首篇的9007199254740993通过原始JSON传入，保留与第二篇的整数区别。
  const encodeDocument = document => {
    const encoded = JSON.stringify(document)
    return document.root.id === '10' ? encoded.replace('"identity":[9007199254740992]', '"identity":[9007199254740993]') : encoded
  }
  for (const dtype of ['f32', 'f16', 'u64']) {
    const file = path.join(root, `document-fields-${dtype}.tdb`)
    let db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    const request = command => JSON.parse(db.documentRequest(JSON.stringify(command)))
    const documents = fixture.documents.map(encodeDocument)
    const snapshots = documents.map(document => JSON.parse(db.documentRequest(`{"operation":"put","document":${document}}`)))
    const search = (testCase, useCache = true) => {
      const base = JSON.stringify({ ...fixture.query, use_cache: useCache, ...testCase.query })
      const query = testCase.query_json ? base.slice(0, -1) + ',' + testCase.query_json.slice(1) : base
      return JSON.parse(db.documentRequest(`{"operation":"search","query":${query}}`))
    }
    const check = testCase => {
      const result = search(testCase)
      assert.deepEqual(result.groups.map(group => group.document_id), testCase.documents, testCase.name)
      assert.deepEqual(result.groups.map(group => group.hits.map(hit => hit.node_id)), testCase.hits, testCase.name)
      assert.equal(result.matched_units, testCase.hits.reduce((count, hits) => count + hits.length, 0))
      assert.equal(result.corpus_documents, 4)
      assert.equal(result.corpus_units, 7)
      if (testCase.name !== 'unknown-field') {
        for (const [index, field] of result.field_corpora.entries()) {
          const expected = fixture.field_corpora[index]
          assert.equal(field.kind, expected.kind)
          assert.equal(field.weight, expected.weight)
          assert.equal(field.corpus_units, expected.corpus_units)
          assert.ok(Math.abs(field.average_length - expected.average_length) < 1e-6)
        }
      }
      for (const group of result.groups) {
        const snapshot = snapshots.find(row => row.document_id === group.document_id)
        const document = fixture.documents.find(row => row.root.id === group.document_id)
        assert.equal(group.revision, snapshot.revision)
        assert.equal(group.source_id, document.source_id)
        assert.equal(group.source_revision, 'v1')
        assert.equal(group.score, group.hits[0].score)
        for (const hit of group.hits) {
          const block = document.blocks.find(row => row.node.id === hit.node_id)
          assert.equal(hit.kind, block.kind)
          assert.equal(hit.block_key, block.key)
          assert.deepEqual(hit.evidence.reference, { document_id: group.document_id, revision: snapshot.revision, start_byte: block.start_byte, end_byte: block.end_byte })
          assert.equal(hit.evidence.text, Buffer.from(document.content).subarray(block.start_byte, block.end_byte).toString('utf8'))
          if (!testCase.query?.include_matches) assert.ok(Math.abs(hit.score - fixture.scores[hit.node_id]) < 1e-6)
          else assert.deepEqual(hit.matches[0].terms, [{ term: 'alpha', position: 0, start_byte: 0, end_byte: 5 }, { term: 'beta', position: 1, start_byte: 6, end_byte: 10 }])
        }
      }
      assert.deepEqual(search(testCase, false), result)
      return result
    }
    for (const testCase of fixture.cases) check(testCase)
    for (const invalid of fixture.invalid) {
      assert.throws(() => search(invalid), error => {
        assert.equal(error.code, 'TDB_INVALID_INPUT')
        assert.deepEqual(error.target, { field: invalid.field })
        assert.equal(error.write, undefined)
        return true
      })
    }
    const selected = fixture.cases.find(row => row.name === 'scope-intersection')
    const retained = check(selected)
    db.close()
    db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    assert.deepEqual(check(selected), retained)
    const changed = structuredClone(fixture.documents[0])
    changed.source_revision = 'v2'
    changed.root.payload.tags = ['new']
    const updated = JSON.parse(db.documentRequest(`{"operation":"put","document":${encodeDocument(changed)},"expected_revision":${JSON.stringify(snapshots[0].revision)}}`))
    assert.deepEqual(search(selected).groups, [])
    const current = request({ operation: 'search', query: { ...fixture.query, members: [{ scope: 'document', field: 'tags', mode: 'any', values: ['new'] }] } })
    assert.deepEqual(current.groups.map(group => group.hits.map(hit => hit.node_id)), [['11', '12']])
    assert.equal(current.groups[0].revision, updated.revision)
    assert.deepEqual(request({ operation: 'delete', document_id: '10', expected_revision: updated.revision }), { deleted: true })
    db.close()
    db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    assert.deepEqual(search(selected).groups, [])
    db.close()
  }
}

function runVectors(root, TriviumDB) {
  const fixture = JSON.parse(fs.readFileSync(path.join(__dirname, '../contracts/document_vector_cases.json'), 'utf8'))
  const source = JSON.parse(fs.readFileSync(path.join(__dirname, '../contracts', fixture.documents_fixture), 'utf8'))
  for (const dtype of ['f32', 'f16', 'u64']) {
    const file = path.join(root, `document-vectors-${dtype}.tdb`)
    let db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    const request = command => JSON.parse(db.documentRequest(JSON.stringify(command)))
    const snapshots = source.documents.map(document => request({ operation: 'put', document }))
    const search = (testCase, useCache = true) => request({ operation: testCase.operation, query: { hits_per_document: 10, use_cache: useCache, ...testCase.query }, vector: fixture.vector, options: testCase.options ?? (testCase.operation === 'search_vector' ? { strategy: 'exact' } : { vector: { strategy: 'exact' } }) })
    const check = testCase => {
      const result = search(testCase)
      assert.deepEqual(result.groups.map(group => group.document_id), testCase.documents, testCase.name)
      assert.deepEqual(result.groups.map(group => group.hits.map(hit => hit.node_id)), testCase.hits, testCase.name)
      assert.equal(result.matched_units, testCase.hits.reduce((count, hits) => count + hits.length, 0))
      assert.deepEqual(result.retrieval, { hybrid: testCase.operation === 'search_hybrid', strategy: 'exact', approximate: false, eligible_units: testCase.eligible, vector_candidates: testCase.vector_candidates ?? testCase.eligible, vector_scores: testCase.eligible, ann_visits: 0, ann_rounds: 0, stop: 'exact_complete' })
      for (const group of result.groups) {
        const snapshot = snapshots.find(row => row.document_id === group.document_id)
        const document = source.documents.find(row => row.root.id === group.document_id)
        assert.equal(group.revision, snapshot.revision)
        assert.equal(group.source_id, document.source_id)
        assert.equal(group.source_revision, 'v1')
        assert.equal(group.score, group.hits[0].score)
        for (const hit of group.hits) {
          const block = testCase.query.level === 'document' ? { key: null, kind: 'document', start_byte: 0, end_byte: Buffer.byteLength(document.content) } : document.blocks.find(row => row.node.id === hit.node_id)
          assert.equal(hit.kind, block.kind)
          assert.equal(hit.block_key, block.key)
          assert.deepEqual(hit.evidence.reference, { document_id: group.document_id, revision: snapshot.revision, start_byte: block.start_byte, end_byte: block.end_byte })
          assert.equal(hit.evidence.text, Buffer.from(document.content).subarray(block.start_byte, block.end_byte).toString('utf8'))
          const title = ['title', 'document'].includes(hit.kind)
          if (testCase.name === 'hybrid-sparse-only') {
            assert.equal(hit.scores.vector_score, null)
            assert.equal(hit.scores.vector_rank, null)
            const rank = fixture.text_field_order.indexOf(hit.node_id) + 1
            assert.equal(hit.scores.text_rank, rank)
            assert.ok(Math.abs(hit.score - 120 / (60 + rank)) < 1e-6)
          } else assert.equal(hit.scores.vector_score, dtype === 'u64' ? (title ? 128 : 126) : (title ? 1 : 0))
          if (testCase.name === 'hybrid-fields') {
            assert.ok(Math.abs(hit.score - fixture.fused_scores[hit.node_id]) < 1e-6)
            assert.equal(hit.scores.vector_rank, fixture.dense_block_order.indexOf(hit.node_id) + 1)
            const textRank = fixture.text_field_order.indexOf(hit.node_id)
            assert.equal(hit.scores.text_rank, textRank < 0 ? null : textRank + 1)
            if (textRank < 0) assert.equal(hit.scores.text_score, null)
            else assert.ok(Math.abs(hit.scores.text_score - source.scores[hit.node_id]) < 1e-6)
          }
          if (testCase.name === 'hybrid-phrase-constraint') {
            assert.ok(Math.abs(hit.score - 120 / 61) < 1e-6)
            assert.equal(hit.matches.length, 1)
            assert.equal(hit.matches[0].evidence.text, 'alpha beta')
            assert.deepEqual(hit.matches[0].terms, [{ term: 'alpha', position: 0, start_byte: 0, end_byte: 5 }, { term: 'beta', position: 1, start_byte: 6, end_byte: 10 }])
          } else assert.equal(hit.matches, undefined)
          if (testCase.operation === 'search_vector' || testCase.name === 'hybrid-dense-only') {
            assert.equal(hit.score, hit.scores.vector_score)
            assert.equal(hit.scores.text_score, null)
          }
        }
      }
      assert.deepEqual(search(testCase, false), result)
      return result
    }
    for (const testCase of fixture.cases) check(testCase)
    for (const invalid of fixture.invalid) assert.throws(() => search(invalid), error => {
      assert.equal(error.code, 'TDB_INVALID_INPUT')
      assert.deepEqual(error.target, { field: invalid.field })
      assert.equal(error.write, undefined)
      return true
    })
    assert.throws(() => request({ operation: 'search_vector', query: {}, vector: [1], options: {} }), error => {
      assert.equal(error.code, 'TDB_DIMENSION_MISMATCH')
      assert.deepEqual(error.target, { field: 'vector' })
      return true
    })
    const retained = check(fixture.cases[5])
    db.close()
    db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    assert.deepEqual(check(fixture.cases[5]), retained)
    const changed = structuredClone(source.documents[0])
    changed.source_revision = 'v2'
    changed.blocks[1].node.vector = [1, 0]
    const updated = request({ operation: 'put', document: changed, expected_revision: snapshots[0].revision })
    const current = request({ operation: 'search_vector', query: { document_ids: ['10'], kinds: ['body'] }, vector: [1, 0] })
    assert.equal(current.groups[0].hits[0].node_id, '12')
    assert.equal(current.groups[0].hits[0].score, dtype === 'u64' ? 128 : 1)
    assert.equal(current.groups[0].revision, updated.revision)
    assert.equal(current.retrieval.strategy, 'exact')
    request({ operation: 'delete', document_id: '10', expected_revision: updated.revision })
    db.close()
    db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    assert.deepEqual(request({ operation: 'search_vector', query: { document_ids: ['10'] }, vector: [1, 0] }).groups, [])
    db.close()
  }
  const db = new TriviumDB(path.join(root, 'document-vectors-full-u64.tdb'), { dim: 2, dtype: 'u64', autoBuildQuiver: false })
  db.documentRequest(`{"operation":"put","document":{"source_id":"bits://full","source_revision":"v1","parser_id":"plain-v1","content":"","root":{"id":"10","vector":[0,0]},"blocks":[
    {"key":"exact","kind":"body","start_byte":0,"end_byte":0,"node":{"id":"11","vector":[18446744073709551615,1]}},
    {"key":"near","kind":"body","start_byte":0,"end_byte":0,"node":{"id":"12","vector":[18446744073709551614,1]}}
  ]}}`)
  const full = JSON.parse(db.documentRequest('{"operation":"search_vector","query":{"hits_per_document":2},"vector":[18446744073709551615,1]}'))
  assert.equal(full.corpus_units, 0)
  assert.equal(full.retrieval.eligible_units, 2)
  assert.deepEqual(full.groups[0].hits.map(hit => [hit.node_id, hit.score, hit.evidence.text]), [['11', 128, ''], ['12', 127, '']])
  db.close()
}

module.exports = { runDocuments }
