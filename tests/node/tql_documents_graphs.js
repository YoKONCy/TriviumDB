// 实际npm制品的文档来源、命名图身份与结构化TQL契约。
const assert = require('node:assert/strict')
const path = require('node:path')
const fixture = require('../contracts/tql_documents_graphs_cases.json')

function check(actual, expected) {
  if (typeof expected === 'number') {
    assert.equal(typeof actual, 'number')
    assert.ok(Math.abs(actual - expected) < 1e-6, `${actual} != ${expected}`)
  } else if (Array.isArray(expected)) {
    assert.equal(actual.length, expected.length)
    expected.forEach((value, index) => check(actual[index], value))
  } else if (expected && typeof expected === 'object') {
    assert.deepEqual(Object.keys(actual).sort(), Object.keys(expected).sort())
    for (const key of Object.keys(expected)) check(actual[key], expected[key])
  } else {
    assert.equal(actual, expected)
  }
}

function rows(result) {
  return result.map(input => {
    const row = { ...input }
    for (const key of ['id', 'a', 'b']) if (row[key] != null) row[key] = String(row[key])
    if (row.path) row.path = row.path.map(String)
    return row
  })
}

function runTqlDocumentsGraphs(root, TriviumDB) {
  for (const dtype of ['f32', 'f16', 'u64']) {
    const file = path.join(root, `tql-documents-graphs-${dtype}.tdb`)
    let db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    for (const source of fixture.documents) {
      db.documentRequest(JSON.stringify({ operation: 'put', document: {
        source_id: `notes://${source.id}`, source_revision: 'v1', parser_id: 'plain-v1', content: 'alpha',
        root: { id: source.id, vector: dtype === 'u64' ? [0, 0] : source.root_vector, payload: { active: true } },
        blocks: [{ key: 'body', kind: 'body', start_byte: 0, end_byte: 5,
          node: { id: source.node, vector: dtype === 'u64' ? source.binary_vector : source.vector, payload: { active: true } } }]
      } }))
    }
    db.link(11, 31, 'rel', 50)
    for (const graph of fixture.graphs) db.graphRequest(JSON.stringify({ operation: 'put', graph }))
    for (const testCase of fixture.cases) {
      if (testCase.dtypes.includes(dtype)) check(rows(db.tql(testCase.query)), testCase.expected)
    }
    assert.deepEqual(db.tql("GRAPH {graph_ids:['words']} MATCH (a)-[e:rel]->(b) RETURN e")[0].e,
      { type: 'edge', source: '11', target: '21', label: 'rel', weight: 2, metadata: { owner: 'words' }, graph_id: 'words' })
    const prepared = db.prepareTql("GRAPH {graph_ids:[$graph]} DOCUMENTS {operation:'search',query:{text:$text}} WITH _ AS seed,document AS evidence EXPAND seed [:rel*1..1] AS related RETURN related.id AS id,evidence.document_id AS doc")
    assert.deepEqual(prepared.parameterNames(), ['graph', 'text'])
    check(rows(db.executePreparedTql(prepared, { graph: 'words', text: 'alpha' })), [{ id: '21', doc: '10' }])
    assert.throws(() => db.tql("GRAPH {graph_ids:['missing']} MATCH (n) RETURN n"), error => {
      assert.equal(error.code, 'TDB_GRAPH_NOT_FOUND')
      assert.equal(error.stage, 'plan')
      assert.equal(error.target.query_span.start_byte, 0)
      assert.equal(error.target.query_span.end_byte, "GRAPH {graph_ids:['missing']}".length)
      return true
    })
    if (dtype !== 'u64') {
      for (const residency of ['ssd', 'hot']) {
        db.quiverRequest(JSON.stringify({ operation: 'configure', options: { encoding: 'centered_int4', query_precision: 'fp32', residency, cache_bytes: 8192, io_depth: 1, use_for_ann: false } }))
        const testCase = fixture.cases[0]
        check(rows(db.tql(testCase.query.replace("strategy:'exact'", "strategy:'ann',rerank:'compressed'"))), testCase.expected)
      }
    }
    db.close()
    db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false, accessMode: 'readOnly' })
    check(rows(db.tql(fixture.cases[2].query)), fixture.cases[2].expected)
    db.close()
  }
}

module.exports = { runTqlDocumentsGraphs }
