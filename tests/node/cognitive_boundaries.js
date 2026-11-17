'use strict'

// 实际安装制品应完整传递大数主题权重、图指标与去重后的多样化结果。
const assert = require('node:assert/strict')
const path = require('node:path')

module.exports = function cognitiveBoundaries(root, TriviumDB) {
  for (const storageMode of ['rom', 'mmap']) {
    const db = new TriviumDB(path.join(root, `cognitive-${storageMode}.tdb`), {
      dim: 2, storageMode, autoBuildQuiver: false,
    })
    try {
      for (const [id, vector] of [[1, [1e30, 0]], [2, [0, 1e30]], [3, [1e30, 1e30]]]) {
        db.insertWithId(id, vector, {})
        db.indexKeyword(id, 'alpha')
      }
      const weight = Math.fround(1e-40)
      db.link(1, 2, 'rel', weight)
      const prefix = 'SEARCH VECTOR [1, 0] TOP 3 AS seed WITH seed'
      const topics = db.tql(`${prefix} TOPICS seed K 2 ITERATIONS 8 AS ranked WITH ranked RETURN ranked.id AS id, topic(ranked) AS topic, topic_score(ranked) AS score ORDER BY id`)
      assert.deepEqual(topics.map(row => row.id), [1, 2, 3])
      for (const row of topics) {
        assert.ok([0, 1].includes(row.topic))
        assert.ok(Number.isFinite(row.score) && row.score > 0)
      }
      const harmonic = db.tql(`${prefix} HARMONIC_CENTRALITY seed LABEL rel AS ranked WITH ranked RETURN ranked.id AS id, harmonic_centrality(ranked) AS score, graph_score(ranked) AS graph ORDER BY id`)
      assert.deepEqual(harmonic.map(row => row.id), [1, 2, 3])
      for (const [index, expected] of [1 / weight, 0, 0].entries()) {
        // IEEE 正负零具有相同的数值含义。
        assert.ok(harmonic[index].score === expected)
        assert.ok(harmonic[index].graph === expected)
      }
      const diverse = db.tql(`TEXT AC "${'alpha '.repeat(100)}" TOP 3 AS hits WITH hits DIVERSIFY hits TOP 2 QUALITY_WEIGHT 10 AS ranked WITH ranked RETURN ranked.id AS id, text_score(ranked) AS quality, diversity_score(ranked) AS rank ORDER BY rank DESC`)
      assert.deepEqual(diverse, [{ id: 1, quality: 100, rank: 1 }, { id: 2, quality: 100, rank: 0.5 }])
    } finally {
      db.close()
    }
  }
}
