#!/usr/bin/env node
// TriviumDB WebUI V4 组合矩阵：3 视口 × 暗浅 × zh/en × TQL/向量 × 5 状态 = 120 组合/引擎。
// 真实 server + 768 维种子；DOM 状态、横向溢出、关键按钮边界、console/pageerror 全检查。
// 同一轮只启动一个 server；WEBUI_BROWSER=chromium|webkit；失败非零退出。

import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const RUN_ID = `${process.pid}-${Date.now()}`;
const OUT = path.join(ROOT, '.tmp', 'v4-combos', RUN_ID);
const DB_DIR = path.join(ROOT, '.tmp', 'v4-combos', `db-${RUN_ID}`);
const BUILD_TARGET = path.resolve(process.env.CARGO_TARGET_DIR || path.join(ROOT, 'target', 'webui-tests'));
const EXE = path.join(BUILD_TARGET, 'debug', process.platform === 'win32' ? 'triviumdb-server.exe' : 'triviumdb-server');
const BROWSER = process.env.WEBUI_BROWSER || 'chromium';
const VIEWPORTS = [
  { name: '1440', width: 1440, height: 900 },
  { name: '1024', width: 1024, height: 768 },
  { name: '390', width: 390, height: 844 },
];
const THEMES = ['light', 'dark'];
const LANGS = ['zh', 'en'];
const PANES = ['query', 'vector'];
const STATES = ['idle', 'running', 'success', 'zero', 'error'];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const failures = [];
let checks = 0;
const expect = (cond, msg) => { checks += 1; if (!cond) failures.push(msg); };
const log = (m) => console.log(`[v4-combos] ${m}`);

mkdirSync(OUT, { recursive: true });
mkdirSync(DB_DIR, { recursive: true });

function resolveCargo() {
  const exeName = process.platform === 'win32' ? 'cargo.exe' : 'cargo';
  const probe = (cmd) => spawnSync(cmd, ['--version'], { encoding: 'utf8', shell: false });
  if (probe(exeName).status === 0) return exeName;
  const full = path.join(os.homedir(), '.cargo', 'bin', exeName);
  return existsSync(full) && probe(full).status === 0 ? full : null;
}
function findPort(from = 8130, to = 8160) {
  return new Promise((resolve) => {
    const tryPort = (p) => {
      if (p > to) return resolve(null);
      const srv = net.createServer();
      srv.once('error', () => tryPort(p + 1));
      srv.listen(p, '127.0.0.1', () => srv.close(() => resolve(p)));
    };
    tryPort(from);
  });
}
async function loadPlaywright() {
  try { return await import('playwright'); } catch (_) { /* fall through */ }
  const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm';
  const probe = spawnSync(npm, ['root', '-g'], { encoding: 'utf8', shell: process.platform === 'win32' });
  if (probe.status === 0 && probe.stdout) {
    const globalRoot = probe.stdout.trim();
    const candidates = [
      path.join(globalRoot, 'noop.js'),
      path.join(globalRoot, '@playwright', 'cli', 'node_modules', 'noop.js'),
    ];
    try {
      const { createRequire } = await import('node:module');
      for (const base of candidates) {
        try {
          const pw = createRequire(base)('playwright');
          if (pw && pw.chromium) return pw;
        } catch (_) { /* next */ }
      }
    } catch (_) { /* ignore */ }
  }
  return null;
}
function request(port, method, urlPath, body, headers = {}) {
  return new Promise((resolve, reject) => {
    const req = http.request(`http://127.0.0.1:${port}${urlPath}`, { method, headers }, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => {
        const text = Buffer.concat(chunks).toString('utf8');
        let json = null; try { json = JSON.parse(text); } catch (_) { /* keep null */ }
        resolve({ status: res.statusCode, text, json });
      });
    });
    req.on('error', reject);
    if (body) req.write(body);
    req.end();
  });
}

const cargo = resolveCargo();
if (!cargo) { console.error('cargo not found'); process.exit(1); }
const build = spawnSync(cargo, ['build', '-p', 'triviumdb-server', '--target-dir', BUILD_TARGET], { cwd: ROOT, stdio: 'inherit' });
if (build.status !== 0) process.exit(build.status || 1);

const pw = await loadPlaywright();
if (!pw || !pw[BROWSER]) {
  console.error('playwright unavailable or unknown WEBUI_BROWSER');
  process.exit(1);
}
const browserType = pw[BROWSER];
const port = await findPort();
if (!port) { console.error('no free port'); process.exit(1); }

const server = spawn(EXE, ['--dim', '768', '--database', path.join(DB_DIR, 'data.tdb'), '--listen', `127.0.0.1:${port}`], {
  cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true,
});
let serverLog = '';
server.stdout.on('data', (d) => { serverLog += d.toString(); });
server.stderr.on('data', (d) => { serverLog += d.toString(); });
for (let i = 0; i < 100; i += 1) {
  try { const r = await fetch(`http://127.0.0.1:${port}/health/ready`); if (r.ok) break; } catch (_) {}
  await sleep(200);
}

function makeVec(seed) {
  return Array.from({ length: 768 }, (_, i) => Math.sin((i + 1) * (seed + 1) * 0.013) * 0.5 + (i === seed ? 1 : 0));
}
const NODES = [0, 1, 2].map((i) => ({ id: i + 1, vector: makeVec(i), payload: { name: ['Alpha', 'Beta', 'Gamma'][i], type: 'person' } }));
const VEC = NODES[0].vector;
const seed = await request(port, 'POST', '/v1/import/nodes', NODES.map(n => JSON.stringify(n)).join('\n'), {
  'Content-Type': 'application/x-ndjson',
});
if (seed.status !== 200) {
  console.error(`seed failed ${seed.status} ${seed.text}`);
  process.exit(1);
}

const browser = await browserType.launch({ headless: true });
const report = { browser: BROWSER, viewports: VIEWPORTS.map(v => v.name), themes: THEMES, langs: LANGS, panes: PANES, states: STATES, combos: 0, checks: 0, failures };

async function newPage(viewport, theme, lang) {
  const context = await browser.newContext({ viewport: { width: viewport.width, height: viewport.height }, reducedMotion: 'reduce', deviceScaleFactor: 1 });
  await context.addInitScript(([t, l]) => {
    try { localStorage.setItem('tdb_theme', t); localStorage.setItem('tdb_lang', l); } catch (_) {}
  }, [theme, lang]);
  const page = await context.newPage();
  const errors = [];
  const allowHttpErrors = { value: false };
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
  page.on('console', (m) => {
    if (m.type() !== 'error') return;
    const text = m.text();
    if (text.includes('net::ERR_FAILED') && text.includes('Failed to load resource')) return;
    if (allowHttpErrors.value && /Failed to load resource: the server responded with a status of (400|500)/.test(text)) return;
    errors.push(`console: ${text}`);
  });
  await page.route('**/*', (route) => {
    const u = new URL(route.request().url());
    if (u.hostname === '127.0.0.1' || u.hostname === 'localhost') route.continue();
    else route.abort();
  });
  await page.goto(`http://127.0.0.1:${port}/ui`, { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true, null, { timeout: 30000 });
  return { context, page, errors, allowHttpErrors };
}

async function waitResult(page, state) {
  await page.waitForFunction((s) => document.getElementById('resultsContainer')?.dataset.resultState === s, state, { timeout: 15000 });
}
async function waitVector(page, state) {
  await page.waitForFunction((s) => document.getElementById('vlResults')?.dataset.resultState === s, state, { timeout: 15000 });
}

function stateLabel(ctx, pane, state) { return `${BROWSER}/${ctx.viewport}/${ctx.theme}/${ctx.lang}/${pane}/${state}`; }

async function checkCommon(page, ctx, pane, state) {
  const label = stateLabel(ctx, pane, state);
  const overflow = await page.evaluate(() => ({
    doc: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    body: document.body.scrollWidth - document.body.clientWidth,
  }));
  expect(overflow.doc <= 1, `${label}: 页面横向溢出 ${overflow.doc}px`);
  expect(overflow.body <= 1, `${label}: body 横向溢出 ${overflow.body}px`);

  const tab = await page.evaluate(() => {
    const el = document.querySelector('.tabs-bar .tab.active');
    const sc = el?.closest('.tabs-scroll');
    if (!el || !sc) return null;
    const r = el.getBoundingClientRect(), sr = sc.getBoundingClientRect();
    return { left: r.left, right: r.right, boxLeft: sr.left, boxRight: sr.right, fullyVisible: r.left >= sr.left - 1 && r.right <= sr.right + 1 };
  });
  expect(!!tab && tab.fullyVisible, `${label}: 当前主 tab 不完整可见`);

  if (pane === 'query') {
    const run = await page.locator('#runQueryBtn').boundingBox();
    expect(!!run && run.y >= -1 && run.y + run.height <= ctx.height + 1, `${label}: TQL 运行按钮不在视口内`);
  } else {
    const vectorRun = await page.locator('#vlRunBtn').boundingBox();
    expect(!!vectorRun && vectorRun.y >= -1 && vectorRun.y + vectorRun.height <= ctx.height + 1, `${label}: 向量运行按钮不在视口内`);
  }

  const geo = await page.evaluate((p) => {
    const rail = document.getElementById('uspRail')?.getBoundingClientRect();
    const results = document.getElementById(p === 'vector' ? 'vlResults' : 'resultsContainer')?.getBoundingClientRect();
    if (!rail || !results) return null;
    const overlapX = Math.min(rail.right, results.right) - Math.max(rail.left, results.left);
    const overlapY = Math.min(rail.bottom, results.bottom) - Math.max(rail.top, results.top);
    return { overlap: overlapX > 0 && overlapY > 0 };
  }, pane);
  if (geo) expect(!geo.overlap, `${label}: 右侧/底部 rail 压住结果区`);

  if (pane === 'query') {
    expect(await page.getAttribute('#resultsContainer', 'data-result-state') === state, `${label}: 状态标记不符`);
  } else {
    expect(await page.getAttribute('#vlResults', 'data-result-state') === state, `${label}: 状态标记不符`);
  }
}

async function runQueryStates(page, ctx) {
  // idle
  await checkCommon(page, ctx, 'query', 'idle');
  expect(await page.locator('#emptyRunBtn').isVisible(), `${stateLabel(ctx, 'query', 'idle')}: 初始 CTA 不可见`);
  if (ctx.takeShots) await page.screenshot({ path: path.join(ctx.dir, 'query-idle.png') });

  // running (real delayed route)
  await page.route('**/v1/tql', async (route) => { await sleep(600); await route.continue(); });
  await page.fill('#tqlInput', 'FIND {type: "person"} RETURN * LIMIT 25');
  await page.click('#runQueryBtn');
  await waitResult(page, 'running');
  expect(await page.getAttribute('#resultsContainer', 'aria-busy') === 'true', `${stateLabel(ctx, 'query', 'running')}: 未 aria-busy`);
  expect(await page.locator('#gridBody .sk-line').count() >= 5, `${stateLabel(ctx, 'query', 'running')}: 无骨架`);
  await checkCommon(page, ctx, 'query', 'running');
  if (ctx.takeShots) await page.screenshot({ path: path.join(ctx.dir, 'query-running.png') });
  await waitResult(page, 'success');
  await page.unroute('**/v1/tql');

  // success
  expect(await page.locator('#gridBody tr[data-row-index]').count() === 3, `${stateLabel(ctx, 'query', 'success')}: 行数不为 3`);
  await checkCommon(page, ctx, 'query', 'success');
  if (ctx.takeShots) await page.screenshot({ path: path.join(ctx.dir, 'query-success.png') });

  // zero
  await page.fill('#tqlInput', 'FIND {type: "nobody"} RETURN * LIMIT 25');
  await page.click('#runQueryBtn');
  await waitResult(page, 'zero');
  expect(await page.locator('#emptyRunBtn').isVisible(), `${stateLabel(ctx, 'query', 'zero')}: 零结果 CTA 不可见`);
  await checkCommon(page, ctx, 'query', 'zero');

  // error
  await page.fill('#tqlInput', 'FIND {');
  ctx.allowHttpErrors.value = true;
  await page.click('#runQueryBtn');
  await waitResult(page, 'error');
  ctx.allowHttpErrors.value = false;
  expect(await page.locator('#errorRunBtn').isVisible(), `${stateLabel(ctx, 'query', 'error')}: 重试按钮不可见`);
  expect(await page.locator('#gridBody tr[data-row-index]').count() === 0, `${stateLabel(ctx, 'query', 'error')}: 残留旧行`);
  await checkCommon(page, ctx, 'query', 'error');
  if (ctx.takeShots) await page.screenshot({ path: path.join(ctx.dir, 'query-error.png') });
}

async function runVectorStates(page, ctx) {
  await page.click('[data-tab="vectorLabPane"]');
  await page.waitForTimeout(240); // 等 tab-pane 入场动画结束，避免截到 opacity 中间态
  // idle
  expect(await page.locator('#vlEmpty').isVisible(), `${stateLabel(ctx, 'vector', 'idle')}: 空态不可见`);
  await checkCommon(page, ctx, 'vector', 'idle');
  if (ctx.takeShots) await page.screenshot({ path: path.join(ctx.dir, 'vector-idle.png') });

  // loading (real delayed route)
  await page.click('#vlSourceTabJson');
  await page.fill('#vlJsonInput', JSON.stringify(VEC));
  await page.route('**/v1/search/vector*', async (route) => { await sleep(600); await route.continue(); });
  await page.click('#vlRunBtn');
  await waitVector(page, 'loading');
  expect(await page.getAttribute('#vlResults', 'aria-busy') === 'true', `${stateLabel(ctx, 'vector', 'loading')}: 未 aria-busy`);
  expect(await page.locator('#vlResultTable').isHidden(), `${stateLabel(ctx, 'vector', 'loading')}: 表格未隐藏`);
  await checkCommon(page, ctx, 'vector', 'loading');
  await waitVector(page, 'success');
  await page.unroute('**/v1/search/vector*');

  // success
  expect(await page.locator('#vlResultBody tr').count() === 3, `${stateLabel(ctx, 'vector', 'success')}: hits 不为 3`);
  await checkCommon(page, ctx, 'vector', 'success');
  if (ctx.takeShots) await page.screenshot({ path: path.join(ctx.dir, 'vector-success.png') });

  // zero（V4.1 R2：min_score 收进「高级参数」折叠区，先展开）
  await page.evaluate(() => { document.getElementById('vlAdvanced').open = true; });
  await page.fill('#vlMinScore', '2');
  await page.click('#vlRunBtn');
  await waitVector(page, 'zero');
  expect(await page.locator('#vlStateTitle').textContent() !== '', `${stateLabel(ctx, 'vector', 'zero')}: 零结果标题为空`);
  await checkCommon(page, ctx, 'vector', 'zero');

  // error (mock 500, clearly a test interception)
  await page.route('**/v1/search/vector*', (route) => route.fulfill({
    status: 500, contentType: 'application/json',
    body: JSON.stringify({ title: '模拟服务端错误', detail: 'v4 combo mock 500' }),
  }));
  ctx.allowHttpErrors.value = true;
  await page.click('#vlRunBtn');
  await waitVector(page, 'error');
  ctx.allowHttpErrors.value = false;
  expect(await page.locator('#vlSearchError').isVisible(), `${stateLabel(ctx, 'vector', 'error')}: 错误未显示`);
  expect(await page.locator('#vlResultTable').isHidden(), `${stateLabel(ctx, 'vector', 'error')}: 残留结果表`);
  await checkCommon(page, ctx, 'vector', 'error');
  if (ctx.takeShots) await page.screenshot({ path: path.join(ctx.dir, 'vector-error.png') });
  await page.unroute('**/v1/search/vector*');
}

try {
  for (const viewport of VIEWPORTS) {
    for (const theme of THEMES) {
      for (const lang of LANGS) {
        const { context, page, errors, allowHttpErrors } = await newPage(viewport, theme, lang);
        const takeShots = ((viewport.name === '390' || viewport.name === '1440') && theme === 'light' && lang === 'zh')
          || (viewport.name === '1024' && theme === 'dark' && lang === 'en');
        const dir = path.join(OUT, `${viewport.name}-${theme}-${lang}`);
        if (takeShots) mkdirSync(dir, { recursive: true });
        const ctx = { viewport: viewport.name, height: viewport.height, width: viewport.width, theme, lang, dir, takeShots, allowHttpErrors };
        report.combos += PANES.length * STATES.length; // 每个上下文 10 个状态组合
        try {
          await runQueryStates(page, ctx);
          await runVectorStates(page, ctx);
          expect(errors.length === 0, `${BROWSER}/${viewport.name}/${theme}/${lang}: errors ${errors.join('; ')}`);
        } catch (e) {
          failures.push(`${BROWSER}/${viewport.name}/${theme}/${lang}: ${e.message}`);
          try { await page.screenshot({ path: path.join(OUT, `FAIL-${viewport.name}-${theme}-${lang}.png`) }); } catch (_) {}
        }
        await context.close();
      }
    }
  }
} finally {
  try { server.kill(); } catch (_) {}
  await sleep(400);
  try { rmSync(DB_DIR, { recursive: true, force: true }); } catch (e) { report.cleanup = String(e.message); }
  try { await browser.close(); } catch (_) {}
}

report.checks = checks;
report.failures = failures;
report.passed = failures.length === 0;
writeFileSync(path.join(OUT, 'report.json'), JSON.stringify(report, null, 2));
log(`${report.combos} context combos, ${checks} checks, ${failures.length} failure(s), artifacts ${OUT}`);
if (failures.length) {
  for (const f of failures) console.error(`  FAIL ${f}`);
  process.exitCode = 1;
} else {
  log('PASS');
}
