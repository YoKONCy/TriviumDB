#!/usr/bin/env node
// V3-B 几何/交互回归。先 build 并启动本地 server，再 bun scripts/graph-scale-checks.mjs。
// 不写数据库；图 fixture 只进入浏览器内存。TDB_BASE 可覆盖默认 8080。
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = path.join(root, '.tmp', 'graph-scale');
const base = process.env.TDB_BASE || 'http://127.0.0.1:8080';
mkdirSync(output, { recursive: true });
const browser = await chromium.launch();
const results = [];
const names = ['Alice', 'Bob', 'Carol', 'Dave', 'Eve', 'Frank', 'Grace', 'Heidi', 'Ivan'];
const makeRows = shape => {
  const nodes = names.map((name, i) => ({ type: 'node', id: String(i + 1),
    payload: { name, type: ['person', 'paper', 'topic'][i % 3] }, vector: [], edges: [] }));
  const parents = shape === 'star' ? [0, 0, 0, 0, 0, 0, 0, 0] : [0, 0, 0, 1, 2, 3, 4, 5];
  return parents.map((from, i) => ({ a: nodes[from], b: nodes[i + 1],
    relation: { type: 'edge', source: nodes[from].id, target: nodes[i + 1].id,
      label: ['KNOWS', 'WORKS_WITH', 'MENTORS'][i % 3] } }));
};
try {
  for (const shape of ['star', 'seed-tree']) {
    for (const reducedMotion of ['reduce', 'no-preference']) {
      const page = await browser.newPage({ viewport: { width: 1600, height: 951 }, reducedMotion });
      const errors = [];
      page.on('pageerror', error => errors.push(error.message));
      page.on('console', msg => { if (msg.type() === 'error') errors.push(msg.text()); });
      await page.goto(base + '/ui');
      await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true);
      await page.click('[data-tab="graphExplorePane"]');
      const samples = [];
      for (let pass = 0; pass < 2; pass++) {
        await page.evaluate(rows => {
          collapseUnifiedPanel();
          graphLayoutMode = 'force'; simulationRunning = true; physicsLockedByScale = false;
          renderGraph(rows);
        }, makeRows(shape));
        await page.waitForFunction(() => simulationRafId === null, null, { polling: 100, timeout: 20000 });
        samples.push(await page.evaluate(() => {
          const c = document.getElementById('standaloneGraphCanvas');
          const bbox = computeGraphBBox(graphNodes, 0), k = standaloneTransform.k;
          const pairs = graphNodes.flatMap((n, i) => graphNodes.slice(i + 1).map(m => Math.hypot(n.x - m.x, n.y - m.y) * k));
          const edges = graphEdges.map(e => {
            const a = nodeMap.get(e.source), b = nodeMap.get(e.target);
            return Math.hypot(a.x - b.x, a.y - b.y) * k;
          });
          const q = document.getElementById('graphCanvas');
          return { canvas: [c.clientWidth, c.clientHeight], hiddenQuery: [q.clientWidth, q.clientHeight],
            bitmap: [c.width, c.height], dpr: devicePixelRatio, k,
            bbox: [bbox.maxX - bbox.minX, bbox.maxY - bbox.minY],
            coverage: (bbox.maxX - bbox.minX) * (bbox.maxY - bbox.minY) * k * k / (c.clientWidth * c.clientHeight),
            minEdge: Math.min(...edges), minPair: Math.min(...pairs),
            positions: graphNodes.map(n => [n.x, n.y]) };
        }));
      }
      // V4.1 R2：侧栏 232（原 264）、工作区头 74、常驻当前节点条 42
      assert.deepEqual(samples[0].canvas, [1368, 751], '蓝本舞台尺寸必须精确匹配');
      assert.deepEqual(
        samples[0].bitmap,
        samples[0].canvas.map(v => Math.round(v * samples[0].dpr)),
        '位图必须是 CSS 尺寸 × DPR（HiDPI 清晰度）'
      );
      assert.deepEqual(samples[0].positions, samples[1].positions, '重复初始化必须确定');
      // V4.1 R2：舞台 1336×819（1.63）→ 1368×751（1.82）；seed-tree 由宽度受限，
      // 纵向留白增加，覆盖率基线 0.45 → 0.40（仍要求铺满大部分舞台，不做空图放行）
      assert(samples[0].coverage >= 0.40, `覆盖率不足：${samples[0].coverage}`);
      assert(samples[0].minEdge >= 90 && samples[0].minPair >= 90, '屏幕中心距不足 90px');
      // 访问查询画布后返回，不能再因隐藏画布缓存尺寸改变力学结果。
      await page.click('[data-tab="queryPane"]');
      await page.click('[data-view="graph"]');
      await page.click('[data-tab="graphExplorePane"]');
      await page.evaluate(rows => renderGraph(rows), makeRows(shape));
      await page.waitForFunction(() => simulationRafId === null, null, { polling: 100, timeout: 20000 });
      const revisited = await page.evaluate(() => graphNodes.map(n => [n.x, n.y]));
      assert.deepEqual(samples[0].positions, revisited, '进入查询页的顺序不能改变独立图布局');
      for (const theme of ['dark', 'light']) {
        await page.evaluate(theme => applyThemeAttributes(theme), theme);
        const typography = await page.evaluate(() => {
          const canvas = document.getElementById('standaloneGraphCanvas'), ctx = canvas.getContext('2d');
          const original = ctx.fillText, labels = [], nodeNames = new Set(graphNodes.map(n => n.label));
          ctx.fillText = function(text, x, y, ...rest) {
            if (nodeNames.has(text)) {
              const m = this.getTransform(), metrics = this.measureText(text);
              const k = Math.hypot(m.a, m.b), sx = m.a * x + m.c * y + m.e, sy = m.b * x + m.d * y + m.f;
              labels.push({ text, size: parseFloat(this.font) * k,
                left: sx - metrics.actualBoundingBoxLeft * k, right: sx + metrics.actualBoundingBoxRight * k,
                top: sy - metrics.actualBoundingBoxAscent * k, bottom: sy + metrics.actualBoundingBoxDescent * k });
            }
            return original.call(this, text, x, y, ...rest);
          };
          try { drawGraphScene(ctx, standaloneTransform, canvas); } finally { ctx.fillText = original; }
          const intersect = (a, b) => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
          const origin = canvas.getBoundingClientRect();
          const overlays = Array.from(canvas.parentElement.querySelectorAll('.v2-hud, .v2-cockpit, .graph-float-toolbar, .graph-controls, .v2-dock'))
            .map(el => { const b = el.getBoundingClientRect(); return { id: el.id || el.className, width: b.width, height: b.height,
              left: b.left - origin.left, right: b.right - origin.left, top: b.top - origin.top, bottom: b.bottom - origin.top }; })
            .filter(b => b.width && b.height);
          const bodies = graphNodes.map(n => {
            const x = n.x * standaloneTransform.k + standaloneTransform.x, y = n.y * standaloneTransform.k + standaloneTransform.y;
            const r = nodeRadius(n) + (n === selectedNode ? 10 : 0);
            return { text: n.label, left: x-r, right: x+r, top: y-r, bottom: y+r };
          });
          return { labels, overlaps: labels.flatMap((a, i) => labels.slice(i + 1).filter(b => intersect(a,b)).map(b => [a.text, b.text])),
            overlayCollisions: labels.concat(bodies).flatMap(a => overlays.filter(b => intersect(a,b)).map(b => [a.text, b.id])) };
        });
        assert.equal(typography.labels.length, 9);
        assert(typography.labels.every(l => Math.abs(l.size - 11) < 0.02));
        assert.deepEqual(typography.overlaps, [], '节点标签不得重叠');
        assert.deepEqual(typography.overlayCollisions, [], '节点与标签不得被舞台浮层遮挡');
        if (reducedMotion === 'reduce') await page.screenshot({ path: path.join(output, `${shape}-${theme}.png`) });
      }
      assert.deepEqual(errors, [], 'console/pageerror 必须为空');
      const { positions, ...metrics } = samples[0];
      results.push({ shape, reducedMotion, ...metrics });
      console.log(JSON.stringify(results.at(-1)));
      await page.close();
    }
  }
  // 两个实际滚轮入口都遵守按钮已有的倍率上限/下限。
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, reducedMotion: 'reduce' });
  await page.goto(base + '/ui');
  await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true);
  const zoom = await page.evaluate(() => {
    const out = [];
    for (const [id, camera] of [['graphCanvas', transform], ['standaloneGraphCanvas', standaloneTransform]]) {
      const canvas = document.getElementById(id);
      camera.k = 20; canvas.dispatchEvent(new WheelEvent('wheel', { deltaY: -1, cancelable: true }));
      out.push(camera.k);
      camera.k = 0.05; canvas.dispatchEvent(new WheelEvent('wheel', { deltaY: 1, cancelable: true }));
      out.push(camera.k);
    }
    return out;
  });
  assert.deepEqual(zoom, [20, 0.05, 20, 0.05]);
  await page.close();
  writeFileSync(path.join(output, 'metrics.json'), JSON.stringify(results, null, 2) + '\n');
  console.log('[graph-scale] 几何、重复初始化、两主题标签与两画布缩放全部通过');
} finally {
  await browser.close();
}
