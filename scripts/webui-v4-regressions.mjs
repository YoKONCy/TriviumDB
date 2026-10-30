#!/usr/bin/env node
// TriviumDB WebUI V4 回归：两项 P2 + TQL/向量状态 + 390 浮层。
// 使用真实 server + 独占临时库；默认 Chromium，设置 WEBUI_BROWSER=webkit 跑第二引擎。
// 失败必须非零退出；临时目录每轮独占，只终止本轮创建的 server。

import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const RUN_ID = `${process.pid}-${Date.now()}`;
const OUT = path.join(ROOT, '.tmp', 'v4-regressions', RUN_ID);
const DB_DIR = path.join(ROOT, '.tmp', 'v4-regressions', `db-${RUN_ID}`);
const BUILD_TARGET = path.resolve(process.env.CARGO_TARGET_DIR || path.join(ROOT, 'target', 'webui-tests'));
const EXE = path.join(BUILD_TARGET, 'debug', process.platform === 'win32' ? 'triviumdb-server.exe' : 'triviumdb-server');
const BROWSER = process.env.WEBUI_BROWSER || 'chromium';
const GSAP_URL = 'https://cdn.jsdelivr.net/npm/gsap@3.13.0/dist/gsap.min.js';
const GSAP_FILE = path.join(ROOT, 'node_modules', 'gsap', 'dist', 'gsap.min.js');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const failures = [];
let checks = 0;
const expect = (cond, msg) => { checks += 1; if (!cond) failures.push(msg); };
const log = (m) => console.log(`[v4-regressions] ${m}`);

mkdirSync(OUT, { recursive: true });
mkdirSync(DB_DIR, { recursive: true });

function resolveCargo() {
  const exeName = process.platform === 'win32' ? 'cargo.exe' : 'cargo';
  const probe = (cmd) => spawnSync(cmd, ['--version'], { encoding: 'utf8', shell: false });
  if (probe(exeName).status === 0) return exeName;
  const full = path.join(os.homedir(), '.cargo', 'bin', exeName);
  return existsSync(full) && probe(full).status === 0 ? full : null;
}

function findPort(from = 8095, to = 8125) {
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

// ---------- 对比度（含渐变中最坏白色叠加） ----------
function parseRgb(s) {
  const m = String(s).match(/rgba?\(([^)]+)\)/);
  if (!m) return null;
  const p = m[1].split(',').map((v) => parseFloat(v.trim()));
  return { r: p[0], g: p[1], b: p[2], a: p.length > 3 ? p[3] : 1 };
}
function lum(c) {
  const f = (v) => { const x = v / 255; return x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4; };
  return 0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b);
}
function contrast(a, b) {
  const A = lum(a), B = lum(b);
  return (Math.max(A, B) + 0.05) / (Math.min(A, B) + 0.05);
}
function maxWhiteAlpha(image) {
  if (!image || image === 'none') return 0;
  let max = 0;
  for (const m of image.matchAll(/rgba?\(([^)]+)\)/g)) {
    const p = m[1].split(',').map((v) => parseFloat(v.trim()));
    if (p[0] === 255 && p[1] === 255 && p[2] === 255) max = Math.max(max, p.length > 3 ? p[3] : 1);
  }
  return max;
}
function worstContrast(bg, color, image) {
  const base = parseRgb(bg);
  const fg = parseRgb(color);
  if (!base || !fg) return 0;
  const a = maxWhiteAlpha(image);
  if (!a) return contrast(base, fg);
  const blended = { r: base.r * (1 - a) + 255 * a, g: base.g * (1 - a) + 255 * a, b: base.b * (1 - a) + 255 * a };
  return contrast(blended, fg);
}

// ---------- 构建 ----------
const cargo = resolveCargo();
if (!cargo) {
  console.error('[v4-regressions] cargo not found');
  process.exit(1);
}
log(`building server (${BUILD_TARGET})`);
const build = spawnSync(cargo, ['build', '-p', 'triviumdb-server', '--target-dir', BUILD_TARGET], {
  cwd: ROOT, stdio: 'inherit', encoding: 'utf8',
});
if (build.status !== 0) {
  console.error('[v4-regressions] cargo build failed');
  process.exit(build.status || 1);
}

const pw = await loadPlaywright();
if (!pw) {
  console.error('[v4-regressions] playwright not available; run npm i --no-save playwright@1.64.1');
  process.exit(1);
}
const browserType = pw[BROWSER];
if (!browserType) {
  console.error(`[v4-regressions] unknown WEBUI_BROWSER=${BROWSER}`);
  process.exit(1);
}

const port = await findPort();
if (!port) {
  console.error('[v4-regressions] no free port in 8095-8125');
  process.exit(1);
}
const server = spawn(EXE, ['--dim', '768', '--database', path.join(DB_DIR, 'data.tdb'), '--listen', `127.0.0.1:${port}`], {
  cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true,
});
let serverLog = '';
server.stdout.on('data', (d) => { serverLog += d.toString(); });
server.stderr.on('data', (d) => { serverLog += d.toString(); });

async function waitReady() {
  for (let i = 0; i < 100; i += 1) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}/health/ready`);
      if (r.ok) return;
    } catch (_) { /* retry */ }
    await sleep(200);
  }
  throw new Error('server not ready');
}

function makeVec(seed) {
  return Array.from({ length: 768 }, (_, i) => Math.sin((i + 1) * (seed + 1) * 0.013) * 0.5 + (i === seed ? 1 : 0));
}
const NODES = [0, 1, 2].map((i) => ({
  id: i + 1,
  vector: makeVec(i),
  payload: { name: ['Alpha 768', 'Beta 768', 'Gamma 768'][i], type: 'person' },
}));
const VEC = NODES[0].vector;

// GSAP 必须用固定版本真实文件，不能用空 stub：断点用例验证的正是真实首屏动画留下的 inline transform。
if (!existsSync(GSAP_FILE)) {
  console.error('[v4-regressions] gsap not available; run npm i --no-save gsap@3.13.0');
  process.exit(1);
}
const gsapSource = readFileSync(GSAP_FILE, 'utf8');

const browser = await browserType.launch({ headless: true });
const report = { browser: BROWSER, scope: process.env.WEBUI_BOUNDARIES_ONLY === '1' ? 'write-boundaries' : 'full', port, checks: 0, failures, parts: {}, errors: [], benignPageErrors: [] };
// WebKit 在画布 ResizeObserver 回调里同步改尺寸会抛这条浏览器警告。
// 已分别用修复前（8188 旧构建）与修复后（8189 新构建）复现，属既有噪声；
// 只做精确匹配记录，不放过其它 pageerror。
const BENIGN_PAGEERROR = /^ResizeObserver loop completed with undelivered notifications\.?$/;

async function newPage(theme, viewport, lang = 'zh', motion = 'reduce', realGsap = false, dpr = 1) {
  const context = await browser.newContext({ viewport, reducedMotion: motion, deviceScaleFactor: dpr });
  await context.addInitScript(([t, l]) => {
    try { localStorage.setItem('tdb_theme', t); localStorage.setItem('tdb_lang', l); } catch (_) {}
  }, [theme, lang]);
  const page = await context.newPage();
  const errors = [];
  const allowHttpErrors = { value: false };
  page.on('pageerror', (e) => {
    if (BENIGN_PAGEERROR.test(e.message)) {
      const key = `${BROWSER}/${theme}/${viewport.width}x${viewport.height}`;
      if (!report.benignPageErrors.some((x) => x.key === key)) {
        report.benignPageErrors.push({ key, message: e.message });
      }
      return;
    }
    errors.push(`pageerror: ${e.message}`);
  });
  page.on('console', (m) => {
    if (m.type() !== 'error') return;
    const text = m.text();
    // 只放过本轮脚本主动 abort 的外部 CDN 请求；其余 console.error 一律保留。
    if (text.includes('net::ERR_FAILED') && text.includes('Failed to load resource')) return;
    // 仅在本轮明确制造 400/500 错误态时放过对应资源错误（调用方成对开关）。
    if (allowHttpErrors.value && /Failed to load resource: the server responded with a status of (400|500)/.test(text)) return;
    errors.push(`console: ${text}`);
  });
  await page.route('**/*', (route) => {
    const url = route.request().url();
    const u = new URL(url);
    if (u.hostname === '127.0.0.1' || u.hostname === 'localhost') return route.continue();
    // 仅放行固定版本 GSAP；其余外网 CDN 仍按现有策略阻断。
    if (realGsap && url === GSAP_URL) {
      return route.fulfill({ status: 200, contentType: 'application/javascript', body: gsapSource });
    }
    return route.abort();
  });
  await page.goto(`http://127.0.0.1:${port}/ui`, { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true, null, { timeout: 30000 });
  if (realGsap) {
    await page.waitForFunction(() => !!(window.gsap && window.gsap.timeline), null, { timeout: 15000 });
  }
  return { context, page, errors, allowHttpErrors };
}

async function waitResultState(page, state, timeout = 15000) {
  await page.waitForFunction((s) => document.getElementById('resultsContainer')?.dataset.resultState === s, state, { timeout });
}
async function waitVectorState(page, state, timeout = 15000) {
  await page.waitForFunction((s) => document.getElementById('vlResults')?.dataset.resultState === s, state, { timeout });
}
async function clickRun(page) {
  await page.click('#runQueryBtn');
}
async function waitVectorReady(page) {
  await page.waitForFunction(() => document.querySelectorAll('#vlNodeSelect option').length >= 3, null, { timeout: 15000 });
}

// ---------- 图探索顶部命中探针（页内执行） ----------
function graphTopProbe() {
  const stage = document.getElementById('standaloneGraphShell');
  const stageRect = stage.getBoundingClientRect();
  const rectOf = (el) => {
    const b = el.getBoundingClientRect();
    const cs = getComputedStyle(el);
    return {
      x: b.x, y: b.y, w: b.width, h: b.height, right: b.right, bottom: b.bottom,
      visible: b.width > 0.5 && b.height > 0.5 && cs.display !== 'none' && cs.visibility !== 'hidden',
      stageX: b.x - stageRect.x, stageY: b.y - stageRect.y,
    };
  };
  const hitOf = (el) => {
    const b = el.getBoundingClientRect();
    if (b.width < 1 || b.height < 1) return { visible: false };
    const x = b.x + b.width / 2;
    const y = b.y + b.height / 2;
    if (x < 0 || y < 0 || x > window.innerWidth || y > window.innerHeight) {
      return { visible: true, outOfViewport: true, self: false, id: '', tag: '' };
    }
    const target = document.elementFromPoint(x, y);
    return {
      visible: true,
      id: target ? target.id || '' : '',
      tag: target ? target.tagName : '',
      self: !!target && (target === el || el.contains(target)),
    };
  };
  const panel = document.getElementById('unifiedSidePanel');
  const panelOpen = document.body.classList.contains('panel-open') && !!panel && panel.classList.contains('open');
  const hud = document.querySelector('.v2-hud.left');
  const cockpit = document.getElementById('graphCockpit');
  const toolbar = document.querySelector('#standaloneGraphShell .graph-float-toolbar');
  const probeButton = (el) => {
    const h = hitOf(el);
    return { btnId: el.id, visible: h.visible, hitId: h.id, hitTag: h.tag, self: h.self, outOfViewport: !!h.outOfViewport };
  };
  return {
    viewport: { w: window.innerWidth, h: window.innerHeight },
    panelOpen,
    panel: panelOpen ? rectOf(panel) : null,
    groups: {
      hud: rectOf(hud),
      cockpit: rectOf(cockpit),
      toolbar: rectOf(toolbar),
    },
    hudButtons: [...hud.querySelectorAll('button')].map(probeButton),
    cockpitToggle: probeButton(document.getElementById('graphCockpitToggle')),
    toolbarButtons: [...toolbar.querySelectorAll('button')].map(probeButton),
    sidebarTransform: getComputedStyle(document.querySelector('aside.sidebar')).transform,
  };
}

const overlaps = (a, b) => !!a && !!b && a.visible && b.visible
  && !(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y);

function checkGraphTop(geo, label, state) {
  const entries = [['hud', geo.groups.hud], ['cockpit', geo.groups.cockpit], ['toolbar', geo.groups.toolbar]];
  for (const [name, rect] of entries) {
    for (const [other, otherRect] of entries) {
      if (other <= name) continue;
      expect(!overlaps(rect, otherRect), `${label}: ${state} ${name} 与 ${other} 矩形相交`);
    }
  }
  if (geo.panelOpen) {
    for (const [name, rect] of entries) {
      expect(!overlaps(rect, geo.panel), `${label}: ${state} ${name} 被统一面板压住`);
    }
  }
  const visibleHud = geo.hudButtons.filter((b) => b.visible);
  expect(visibleHud.length >= 2, `${label}: ${state} 可交互 HUD 入口少于 2 个`);
  for (const btn of visibleHud) {
    expect(btn.self, `${label}: ${state} HUD ${btn.btnId} 中心命中 ${btn.hitId || btn.hitTag || 'none'}`);
  }
  // 1440 是发现 1 的原始现场：五个 HUD 入口必须全部可见且可点。
  if (geo.viewport.w >= 1281 && !geo.panelOpen) {
    const ids = visibleHud.map((b) => b.btnId);
    expect(ids.includes('graphHudKhop') && ids.includes('graphHudPath'),
      `${label}: ${state} 宽屏下 k-hop/路径入口不可见 (${ids.join(',')})`);
  }
  expect(geo.cockpitToggle.self, `${label}: ${state} 查询舱开关中心命中 ${geo.cockpitToggle.hitId || geo.cockpitToggle.hitTag || 'none'}`);
  const visibleTools = geo.toolbarButtons.filter((b) => b.visible);
  expect(visibleTools.length >= 4, `${label}: ${state} 视图工具条入口不足 (${visibleTools.length})`);
  for (const btn of visibleTools) {
    expect(btn.self, `${label}: ${state} 工具条 ${btn.btnId} 中心命中 ${btn.hitId || btn.hitTag || 'none'}`);
  }
}

// ---------- 侧栏断点生命周期探针 ----------
async function waitGsapShell(page) {
  await page.waitForFunction(() => !!(window.gsap && window.gsap.timeline), null, { timeout: 15000 });
  await page.waitForFunction(() => {
    const sb = document.querySelector('aside.sidebar');
    if (!sb) return false;
    const inline = sb.getAttribute('style') || '';
    if (!inline.includes('transform')) return false;
    return getComputedStyle(sb).transform === 'matrix(1, 0, 0, 1, 0, 0)';
  }, null, { timeout: 15000 });
  await page.waitForTimeout(300);
}

async function waitSidebarClosed(page) {
  await page.waitForFunction(() => {
    const sidebar = document.querySelector('aside.sidebar');
    return window.matchMedia('(max-width: 900px)').matches && sidebar
      && document.getElementById('sidebarMobileBtn')?.getAttribute('aria-expanded') === 'false'
      && sidebar.getBoundingClientRect().right <= 1;
  }, null, { timeout: 15000 });
}

async function sidebarProbe(page) {
  return page.evaluate(() => {
    const sb = document.querySelector('aside.sidebar');
    const b = sb.getBoundingClientRect();
    const x = window.innerWidth / 2;
    const y = 140;
    const hit = document.elementFromPoint(x, y);
    return {
      rect: { x: b.x, y: b.y, w: b.width, h: b.height, right: b.right, bottom: b.bottom },
      inline: sb.getAttribute('style'),
      transform: getComputedStyle(sb).transform,
      expanded: document.getElementById('sidebarMobileBtn')?.getAttribute('aria-expanded'),
      datasetOpen: document.documentElement.dataset.sidebarOpen ?? '',
      datasetRail: document.documentElement.dataset.rail ?? '',
      scrimVisible: getComputedStyle(document.getElementById('sidebarScrim')).display !== 'none',
      workspaceHit: { id: hit?.id || '', tag: hit?.tagName || '', insideSidebar: !!hit && sb.contains(hit) },
      gsap: !!(window.gsap && window.gsap.timeline),
    };
  });
}

// ---------- 390 向量命中探针 ----------
function vectorHitProbe() {
  const btn = document.getElementById('vlRunBtn');
  const bt = btn.getBoundingClientRect();
  const scrollAncestor = (el) => {
    let p = el.parentElement;
    while (p && p !== document.body) {
      const cs = getComputedStyle(p);
      if (/(auto|scroll)/.test(cs.overflowY) && p.scrollHeight > p.clientHeight + 1) return p;
      p = p.parentElement;
    }
    return null;
  };
  const hitTest = (r, owner) => {
    const x = r.left + r.width / 2;
    const y = r.top + r.height / 2;
    const el = document.elementFromPoint(x, y);
    return {
      id: el?.id || '',
      tag: el?.tagName || '',
      self: !!el && (el === owner || owner.contains(el)),
    };
  };
  const labels = [...document.querySelectorAll('.vl-params .vl-param')].map((lab) => {
    const textNode = lab.firstChild;
    const range = document.createRange();
    range.selectNodeContents(textNode);
    const tr = range.getBoundingClientRect();
    const input = lab.querySelector('input');
    const ir = input.getBoundingClientRect();
    const sc = scrollAncestor(lab);
    const scRect = sc ? sc.getBoundingClientRect() : { top: 0, bottom: window.innerHeight };
    const inView = (r) => r.width > 0 && r.top >= Math.max(0, scRect.top) - 1 && r.bottom <= Math.min(window.innerHeight, scRect.bottom) + 1;
    return {
      name: lab.textContent.replace(/\s+/g, ' ').trim().split(' ')[0],
      textVisible: inView(tr),
      inputVisible: inView(ir),
      textRect: { x: tr.x, y: tr.y, w: tr.width, h: tr.height },
      inputRect: { x: ir.x, y: ir.y, w: ir.width, h: ir.height },
      textHit: hitTest(tr, lab),
      inputHit: hitTest(ir, input),
    };
  });
  const scroller = scrollAncestor(document.querySelector('.vl-params .vl-param'));
  return {
    viewport: { w: window.innerWidth, h: window.innerHeight },
    btn: {
      x: bt.x, y: bt.y, w: bt.width, h: bt.height,
      inViewport: bt.top >= -0.5 && bt.bottom <= window.innerHeight + 0.5 && bt.left >= -0.5 && bt.right <= window.innerWidth + 0.5,
      hit: hitTest(bt, btn),
    },
    scroller: scroller ? { cls: String(scroller.className), scrollTop: scroller.scrollTop, max: scroller.scrollHeight - scroller.clientHeight } : null,
    labels,
    activeId: document.activeElement?.id || '',
  };
}

function checkVectorHit(geo, label, state) {
  expect(geo.btn.inViewport, `${label}: ${state} 运行按钮不在视口内`);
  expect(geo.btn.hit.self, `${label}: ${state} 运行按钮中心命中 ${geo.btn.hit.id || geo.btn.hit.tag}`);
  for (const item of geo.labels) {
    if (item.textVisible) {
      expect(item.textHit.self, `${label}: ${state} 参数 ${item.name} 标签中心命中 ${item.textHit.id || item.textHit.tag}`);
    }
    if (item.inputVisible) {
      expect(item.inputHit.self, `${label}: ${state} 参数 ${item.name} 输入框中心命中 ${item.inputHit.id || item.inputHit.tag}`);
    }
  }
  return geo.labels.filter((l) => l.textVisible || l.inputVisible).length;
}

async function setVectorScroll(page, where) {
  await page.evaluate((w) => {
    const lab = document.querySelector('.vl-params .vl-param');
    let p = lab.parentElement;
    while (p && p !== document.body) {
      const cs = getComputedStyle(p);
      if (/(auto|scroll)/.test(cs.overflowY) && p.scrollHeight > p.clientHeight + 1) break;
      p = p.parentElement;
    }
    if (!p || p === document.body) return;
    p.scrollTop = w === 'bottom' ? p.scrollHeight : 0;
  }, where);
  await page.waitForTimeout(200);
}

try {
  await waitReady();
  const seed = await request(port, 'POST', '/v1/import/nodes', NODES.map(n => JSON.stringify(n)).join('\n'), {
    'Content-Type': 'application/x-ndjson',
  });
  if (seed.status !== 200) throw new Error(`seed failed ${seed.status} ${seed.text}`);
  report.seedRows = NODES.length;

  if (process.env.WEBUI_BOUNDARIES_ONLY !== '1') {
  // ---------- A. P2 listbox + primary button ----------
  for (const theme of ['light', 'dark']) {
    const { context, page, errors } = await newPage(theme, { width: 1440, height: 900 }, 'zh', 'no-preference');
    const dir = path.join(OUT, theme);
    mkdirSync(dir, { recursive: true });
    await page.click('[data-tab="vectorLabPane"]');
    await page.click('#vlLoadCandidatesBtn');
    await waitVectorReady(page);
    const readList = () => page.evaluate(() => {
      const sel = document.getElementById('vlNodeSelect');
      const probe = document.createElement('span');
      probe.style.backgroundColor = 'var(--active)';
      document.body.appendChild(probe);
      const expected = getComputedStyle(probe).backgroundColor;
      probe.remove();
      return {
        value: sel.value,
        index: sel.selectedIndex,
        focused: document.activeElement === sel,
        expected,
        options: [...sel.options].map((o, i) => {
          const cs = getComputedStyle(o);
          return { i, selected: o.selected, disabled: o.disabled, bg: cs.backgroundColor, color: cs.color, weight: cs.fontWeight };
        }),
      };
    });
    const initial = await readList();
    const optPoint = await page.evaluate(() => {
      const o = document.getElementById('vlNodeSelect').options[1];
      const r = o.getBoundingClientRect();
      return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
    });
    await page.mouse.click(optPoint.x, optPoint.y);
    await page.waitForTimeout(150);
    const mouseFocused = await readList();
    await page.click('#vlSelectorQuery');
    await page.waitForTimeout(150);
    const mouseBlurred = await readList();
    await page.locator('#vlNodeSelect').focus();
    await page.keyboard.press('ArrowUp');
    await page.waitForTimeout(150);
    const keyFocused = await readList();
    await page.keyboard.press('Tab');
    await page.waitForTimeout(150);
    const keyBlurred = await readList();
    await page.screenshot({ path: path.join(dir, 'list-keyboard-blurred.png') });

    expect(initial.options.length === 3, `${BROWSER}/${theme}: 候选数 ${initial.options.length} != 3`);
    expect(mouseFocused.value === '1' && mouseBlurred.value === '1', `${BROWSER}/${theme}: 鼠标选中值失焦变化`);
    expect(mouseFocused.index === mouseBlurred.index, `${BROWSER}/${theme}: 鼠标 selectedIndex 失焦变化`);
    expect(mouseBlurred.focused === false, `${BROWSER}/${theme}: 失焦后仍报告 select 聚焦`);
    expect(keyFocused.value === '0' && keyBlurred.value === '0', `${BROWSER}/${theme}: 键盘选中值 Tab 后变化`);
    expect(keyBlurred.index === keyFocused.index, `${BROWSER}/${theme}: 键盘 selectedIndex Tab 后变化`);
    for (const [stateName, state] of [['mouse', mouseBlurred], ['keyboard', keyBlurred]]) {
      const sel = state.options.find(o => o.selected);
      const unsel = state.options.find(o => !o.selected && !o.disabled);
      expect(!!sel && !!unsel, `${BROWSER}/${theme}/${stateName}: 缺少选中/未选中项`);
      if (sel && unsel) {
        expect(sel.bg === state.expected, `${BROWSER}/${theme}/${stateName}: 失焦底色 ${sel.bg} != ${state.expected}`);
        expect(sel.weight === '600', `${BROWSER}/${theme}/${stateName}: 选中字重 ${sel.weight}`);
        expect(sel.bg !== unsel.bg, `${BROWSER}/${theme}/${stateName}: 选中/未选中同色`);
      }
    }

    // 按钮：真实 AI 设置 Save（打开设置自动聚焦 Base URL）
    await page.click('#aiChatToggleBtn');
    await page.waitForTimeout(250);
    if (!await page.evaluate(() => document.getElementById('aiSettingsOverlay').classList.contains('open'))) {
      await page.click('#aiSettingsBtn');
    }
    await page.waitForSelector('#aiSettingsOverlay.open', { timeout: 10000 });
    await page.waitForTimeout(200);
    const sampleButton = (sel) => page.evaluate((s) => {
      const el = document.querySelector(s);
      const cs = getComputedStyle(el);
      return { bg: cs.backgroundColor, color: cs.color, image: cs.backgroundImage, pos: cs.backgroundPosition, transition: cs.transitionDuration, opacity: cs.opacity, disabled: el.disabled, focusVisible: el.matches(':focus-visible'), active: el.matches(':active'), hover: el.matches(':hover') };
    }, sel);
    const btn = '#aiCfgSaveBtn';
    await page.mouse.move(5, 5);
    await page.waitForTimeout(500);
    const normal = await sampleButton(btn);
    const box = await page.locator(btn).boundingBox();
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    const enter = [];
    let t = 0;
    for (const at of [0, 20, 50, 200, 520, 700]) {
      if (at > t) { await sleep(at - t); t = at; }
      enter.push({ at, ...(await sampleButton(btn)) });
    }
    await page.mouse.move(5, 5);
    const leave = [];
    t = 0;
    for (const at of [0, 20, 50, 200, 520, 700]) {
      if (at > t) { await sleep(at - t); t = at; }
      leave.push({ at, ...(await sampleButton(btn)) });
    }
    const hover = enter[enter.length - 1];
    const minEnter = Math.min(...enter.map(s => worstContrast(s.bg, s.color, s.image)));
    const minLeave = Math.min(...leave.map(s => worstContrast(s.bg, s.color, s.image)));
    expect(minEnter >= 4.5, `${BROWSER}/${theme}: 按钮进入最低对比度 ${minEnter.toFixed(2)}`);
    expect(minLeave >= 4.5, `${BROWSER}/${theme}: 按钮离开最低对比度 ${minLeave.toFixed(2)}`);
    expect(hover.hover === true, `${BROWSER}/${theme}: 悬停终态未命中 :hover`);
    expect(enter[0].bg !== hover.bg, `${BROWSER}/${theme}: 按钮进入无颜色过渡`);
    expect(leave[0].bg !== leave[leave.length - 1].bg, `${BROWSER}/${theme}: 按钮离开无颜色过渡`);
    expect(normal.transition.includes('0.14s'), `${BROWSER}/${theme}: 缺少 0.14s 过渡 (${normal.transition})`);
    if (theme === 'light') {
      expect(enter.every(s => s.image === 'none') && leave.every(s => s.image === 'none'), `${BROWSER}/light: 主按钮仍有扫光`);
    } else {
      expect(enter.every(s => s.image.includes('linear-gradient')), `${BROWSER}/dark: 主按钮扫光被误删`);
    }
    await page.screenshot({ path: path.join(dir, 'button-enter.png') });

    // active（阻断 click，避免空表单提交）；focus（移开鼠标）
    await page.evaluate((s) => {
      const el = document.querySelector(s);
      el.addEventListener('click', e => { e.preventDefault(); e.stopImmediatePropagation(); }, { capture: true, once: true });
    }, btn);
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.waitForTimeout(400);
    await page.mouse.down();
    await page.waitForTimeout(40);
    const active = await sampleButton(btn);
    expect(active.active === true, `${BROWSER}/${theme}: 按下时未命中 :active`);
    await page.mouse.up();
    expect(await page.evaluate((s) => document.querySelector(s).matches(':active'), btn) === false, `${BROWSER}/${theme}: active 态残留`);
    await page.mouse.move(5, 5);
    await page.waitForTimeout(500);
    await page.locator(btn).focus();
    await page.keyboard.press('Shift+Tab');
    await page.keyboard.press('Tab');
    await page.waitForTimeout(150);
    const focus = await sampleButton(btn);
    expect(focus.focusVisible === true, `${BROWSER}/${theme}: 主按钮未进入 focus-visible`);
    await page.evaluate((s) => { document.querySelector(s).disabled = true; }, btn);
    await page.waitForTimeout(100);
    const disabled = await sampleButton(btn);
    expect(disabled.opacity === '0.5', `${BROWSER}/${theme}: disabled opacity ${disabled.opacity}`);
    await page.evaluate((s) => { document.querySelector(s).disabled = false; }, btn);
    expect(worstContrast(active.bg, active.color, active.image) >= 4.5, `${BROWSER}/${theme}: active 对比度不足`);
    expect(worstContrast(focus.bg, focus.color, focus.image) >= 4.5, `${BROWSER}/${theme}: focus 对比度不足`);
    await page.screenshot({ path: path.join(dir, 'button-focus.png') });
    report.parts.p2 ??= {};
    report.parts.p2[theme] = { minEnter, minLeave, active: active.bg, focus: focus.bg };
    expect(errors.length === 0, `${BROWSER}/${theme}: P2 errors ${errors.join('; ')}`);
    await context.close();
  }

  // ---------- B. TQL / vector states (1440, both themes) ----------
  for (const theme of ['light', 'dark']) {
    const { context, page, errors, allowHttpErrors } = await newPage(theme, { width: 1440, height: 900 });
    const dir = path.join(OUT, theme);
    mkdirSync(dir, { recursive: true });
    await page.screenshot({ path: path.join(dir, 'tql-idle.png') });
    expect(await page.getAttribute('#resultsContainer', 'data-result-state') === 'idle', `${BROWSER}/${theme}: TQL 初始非 idle`);

    // running via delayed route
    await page.route('**/v1/tql', async (route) => { await sleep(700); await route.continue(); });
    await page.fill('#tqlInput', 'FIND {type: "person"} RETURN * LIMIT 25');
    await clickRun(page);
    await waitResultState(page, 'running', 5000);
    expect(await page.getAttribute('#resultsContainer', 'aria-busy') === 'true', `${BROWSER}/${theme}: running 未 aria-busy`);
    expect(await page.locator('#gridBody .sk-line').count() >= 5, `${BROWSER}/${theme}: running 无骨架`);
    expect(await page.isDisabled('#exportJsonBtn') === true, `${BROWSER}/${theme}: running 导出未禁用`);
    await page.screenshot({ path: path.join(dir, 'tql-running.png') });
    await waitResultState(page, 'success');
    await page.unroute('**/v1/tql');
    expect(await page.locator('#gridBody tr[data-row-index]').count() === 3, `${BROWSER}/${theme}: TQL success 行数`);
    await page.screenshot({ path: path.join(dir, 'tql-success.png') });

    await page.fill('#tqlInput', 'FIND {type: "nobody"} RETURN * LIMIT 25');
    await clickRun(page);
    await waitResultState(page, 'zero');
    const zeroText = await page.textContent('#gridBody');
    expect(zeroText.includes('没有匹配的行'), `${BROWSER}/${theme}: zero 文案缺失`);
    expect(zeroText.includes('检查过滤条件'), `${BROWSER}/${theme}: zero 说明缺失`);
    await page.screenshot({ path: path.join(dir, 'tql-zero.png') });

    await page.fill('#tqlInput', 'FIND {');
    allowHttpErrors.value = true;
    await clickRun(page);
    await waitResultState(page, 'error');
    allowHttpErrors.value = false;
    expect(await page.locator('#gridBody tr[data-row-index]').count() === 0, `${BROWSER}/${theme}: error 残留旧行`);
    expect((await page.textContent('#gridBody')).includes('查询解析失败'), `${BROWSER}/${theme}: error 标题缺失`);
    expect((await page.textContent('#statusRowCount')).includes('—'), `${BROWSER}/${theme}: error 状态栏仍显示行数`);
    await page.screenshot({ path: path.join(dir, 'tql-error.png') });

    // vector: idle
    await page.click('[data-tab="vectorLabPane"]');
    await page.waitForTimeout(240);
    expect(await page.getAttribute('#vlResults', 'data-result-state') === 'idle', `${BROWSER}/${theme}: 向量初始非 idle`);
    expect(await page.locator('#vlEmpty').isVisible(), `${BROWSER}/${theme}: 向量 idle 空态不可见`);
    await page.click('#vlSourceTabJson');
    await page.fill('#vlJsonInput', JSON.stringify(VEC));
    expect((await page.textContent('#vlDimHint')).includes('768'), `${BROWSER}/${theme}: 768 JSON 提示缺失`);
    expect(await page.getAttribute('#vlDimHint', 'class') .then(c => c.includes('vl-hint--ok')), `${BROWSER}/${theme}: JSON 合法提示不是成功色`);
    await page.screenshot({ path: path.join(dir, 'vector-json-768.png') });

    // loading via delayed vector search
    await page.route('**/v1/search/vector*', async (route) => { await sleep(700); await route.continue(); });
    await page.click('#vlRunBtn');
    await waitVectorState(page, 'loading', 5000);
    expect(await page.getAttribute('#vlResults', 'aria-busy') === 'true', `${BROWSER}/${theme}: vector loading 未 aria-busy`);
    expect(await page.locator('#vlResultTable').isHidden(), `${BROWSER}/${theme}: loading 表格未隐藏`);
    await page.screenshot({ path: path.join(dir, 'vector-loading.png') });
    await waitVectorState(page, 'success');
    await page.unroute('**/v1/search/vector*');
    expect(await page.locator('#vlResultBody tr').count() === 3, `${BROWSER}/${theme}: vector hits`);
    await page.screenshot({ path: path.join(dir, 'vector-success.png') });

    await page.evaluate(() => { document.getElementById('vlAdvanced').open = true; });
    await page.fill('#vlMinScore', '2');
    await page.click('#vlRunBtn');
    await waitVectorState(page, 'zero');
    expect(await page.textContent('#vlStateTitle') === '没有命中结果', `${BROWSER}/${theme}: vector zero 标题`);
    await page.screenshot({ path: path.join(dir, 'vector-zero.png') });

    await page.unroute('**/v1/search/vector*').catch(() => {});
    await page.route('**/v1/search/vector*', (route) => route.fulfill({
      status: 500, contentType: 'application/json',
      body: JSON.stringify({ title: '模拟服务端错误', detail: 'v4 regression mock 500' }),
    }));
    allowHttpErrors.value = true;
    await page.click('#vlRunBtn');
    await waitVectorState(page, 'error');
    allowHttpErrors.value = false;
    expect((await page.textContent('#vlSearchError')).includes('模拟服务端错误'), `${BROWSER}/${theme}: vector 500 错误未显示`);
    expect(await page.locator('#vlResultTable').isHidden(), `${BROWSER}/${theme}: vector error 残留表格`);
    await page.screenshot({ path: path.join(dir, 'vector-error.png') });
    await page.unroute('**/v1/search/vector*');

    expect(errors.length === 0, `${BROWSER}/${theme}: states errors ${errors.join('; ')}`);
    await context.close();
  }

  // ---------- C. 390 overlays / tabs / focus / Esc ----------
  for (const theme of ['light', 'dark']) {
    const { context, page, errors } = await newPage(theme, { width: 390, height: 844 });
    const dir = path.join(OUT, `${theme}-390`);
    mkdirSync(dir, { recursive: true });
    const tab = await page.evaluate(() => {
      const el = document.querySelector('.tabs-bar .tab.active');
      const sc = el.closest('.tabs-scroll');
      const r = el.getBoundingClientRect(), sr = sc.getBoundingClientRect();
      return { left: r.left, right: r.right, boxLeft: sr.left, boxRight: sr.right, fullyVisible: r.left >= sr.left - 1 && r.right <= sr.right + 1 };
    });
    expect(tab.fullyVisible, `${BROWSER}/${theme}/390: 初始 tab 不完整可见`);
    await page.click('[data-tab="vectorLabPane"]');
    await page.waitForTimeout(300);
    const vectorTab = await page.evaluate(() => {
      const el = document.querySelector('.tabs-bar .tab.active');
      const sc = el.closest('.tabs-scroll');
      const r = el.getBoundingClientRect(), sr = sc.getBoundingClientRect();
      return { left: r.left, right: r.right, boxLeft: sr.left, boxRight: sr.right, fullyVisible: r.left >= sr.left - 1 && r.right <= sr.right + 1 };
    });
    expect(vectorTab.fullyVisible, `${BROWSER}/${theme}/390: 向量 tab 不完整可见`);
    const runBox = await page.locator('#vlRunBtn').boundingBox();
    expect(runBox && runBox.y + runBox.height <= 844 + 1 && runBox.y >= 0, `${BROWSER}/${theme}/390: 向量运行按钮不在视口内`);
    await page.screenshot({ path: path.join(dir, 'vector-tab.png') });

    await page.click('#openTutorialBtn');
    await page.waitForTimeout(250);
    const tut = await page.evaluate(() => {
      const m = document.querySelector('.tutorial-modal').getBoundingClientRect();
      const nav = document.getElementById('tutorialStepsNav');
      const content = document.getElementById('tutorialContentPane');
      return {
        modal: [m.left, m.right, m.top, m.bottom],
        navVisible: nav.offsetHeight > 0 && nav.getBoundingClientRect().width > 100,
        contentVisible: content.offsetHeight > 100,
        focused: document.activeElement?.classList.contains('tutorial-step-item') || document.activeElement?.id === 'closeTutorialBtn',
      };
    });
    expect(tut.modal[0] >= 0 && tut.modal[1] <= 390, `${BROWSER}/${theme}/390: 教程弹窗越界`);
    expect(tut.navVisible && tut.contentVisible, `${BROWSER}/${theme}/390: 教程导航/正文不可见`);
    expect(tut.focused, `${BROWSER}/${theme}/390: 教程打开未聚焦`);
    await page.screenshot({ path: path.join(dir, 'tutorial.png') });
    await page.keyboard.press('Escape');
    await page.waitForTimeout(200);
    expect(await page.evaluate(() => !document.getElementById('tutorialOverlay').classList.contains('open')), `${BROWSER}/${theme}/390: 教程 Esc 未关闭`);
    expect(await page.evaluate(() => document.activeElement?.id === 'openTutorialBtn'), `${BROWSER}/${theme}/390: 教程关闭未还原焦点`);

    await page.click('#aiChatToggleBtn');
    await page.waitForTimeout(250);
    const ai = await page.evaluate(() => {
      const m = document.querySelector('.ai-settings-modal').getBoundingClientRect();
      return [m.left, m.right, m.top, m.bottom];
    });
    expect(ai[0] >= 0 && ai[1] <= 390, `${BROWSER}/${theme}/390: AI 设置越界`);
    await page.keyboard.press('Escape');
    await page.waitForTimeout(200);
    if (await page.locator('#unifiedSidePanel.open').count()) {
      await page.click('#uspCollapseBtn');
      await page.waitForTimeout(250);
    }
    await page.click('#uspRailFilterBtn');
    await page.waitForTimeout(250);
    const panel = await page.evaluate(() => {
      const p = document.getElementById('unifiedSidePanel').getBoundingClientRect();
      const r = document.getElementById('uspRail').getBoundingClientRect();
      return { panel: [p.left, p.right], rail: [r.left, r.right, r.top, r.bottom], railHidden: getComputedStyle(document.getElementById('uspRail')).opacity === '0' };
    });
    expect(panel.panel[0] >= 0 && panel.panel[1] <= 390, `${BROWSER}/${theme}/390: 面板越界`);
    expect(panel.rail[0] >= 0 && panel.rail[1] <= 390, `${BROWSER}/${theme}/390: rail 越界`);
    expect(panel.railHidden, `${BROWSER}/${theme}/390: 面板打开 rail 未隐藏`);
    await page.screenshot({ path: path.join(dir, 'filter-panel.png') });
    expect(errors.length === 0, `${BROWSER}/${theme}/390: errors ${errors.join('; ')}`);
    await context.close();
  }

  // ---------- D. 图探索顶部 hit-test：1440/1024/390 × 暗浅 ----------
  report.parts.graphTop = {};
  for (const theme of ['light', 'dark']) {
    for (const vp of [
      { name: '1440', width: 1440, height: 900 },
      { name: '1024', width: 1024, height: 768 },
      { name: '390', width: 390, height: 844 },
    ]) {
      const { context, page, errors } = await newPage(theme, vp, 'zh', 'reduce');
      const dir = path.join(OUT, `${theme}-${vp.name}-graph`);
      mkdirSync(dir, { recursive: true });
      await page.click('[data-tab="graphExplorePane"]');
      await page.waitForTimeout(450);

      const states = {};
      const capture = async (state) => {
        const geo = await page.evaluate(graphTopProbe);
        states[state] = geo;
        await page.screenshot({ path: path.join(dir, `${state}.png`) });
        checkGraphTop(geo, `${BROWSER}/${theme}/${vp.name}`, state);
        return geo;
      };
      const label = `${BROWSER}/${theme}/${vp.name}`;

      await capture('initial');

      await page.click('#graphCockpitToggle');
      await page.waitForTimeout(300);
      expect(await page.evaluate(() => document.getElementById('graphCockpit').classList.contains('open')),
        `${label}: 查询舱未展开`);
      await capture('cockpit-open');
      await page.click('#graphCockpitToggle');
      await page.waitForTimeout(250);

      if (vp.width > 900) {
        await page.click('#sidebarRailToggle');
        await page.waitForTimeout(400);
        await capture('sidebar-rail');
        await page.click('#sidebarRailToggle');
        await page.waitForTimeout(400);

        await page.click('#uspRailFilterBtn');
        await page.waitForTimeout(500);
        await capture('panel-open');
        await page.click('#uspCollapseBtn');
        await page.waitForTimeout(400);
      }

      // 手机端统一面板是全屏覆盖层：此时图控件按设计不可点，只记录不判定。
      if (vp.width <= 900) {
        await page.click('#uspRailFilterBtn');
        await page.waitForTimeout(500);
        states['panel-open-skipped'] = await page.evaluate(graphTopProbe);
        await page.screenshot({ path: path.join(dir, 'panel-open.png') });
      }

      report.parts.graphTop[`${theme}-${vp.name}`] = states;
      writeFileSync(path.join(dir, 'hit-test.json'), JSON.stringify(states, null, 2));
      expect(errors.length === 0, `${label}: graph errors ${errors.join('; ')}`);
      await context.close();
    }
  }

  // ---------- E. 桌面→手机断点生命周期（真实 GSAP） ----------
  report.parts.breakpoint = {};
  for (const theme of ['light', 'dark']) {
    const { context, page, errors } = await newPage(theme, { width: 1440, height: 900 }, 'zh', 'no-preference', true);
    const label = `${BROWSER}/${theme}/breakpoint`;
    await waitGsapShell(page);
    const before = await sidebarProbe(page);
    expect(!!before.gsap, `${label}: GSAP 未加载（前提不成立）`);
    expect(/transform/.test(before.inline || ''), `${label}: 首屏动画未留下 inline transform（前提不成立）`);
    expect(before.rect.x >= 0 && before.rect.w > 200, `${label}: 桌面前提不成立 x=${before.rect.x} w=${before.rect.w}`);
    await page.screenshot({ path: path.join(OUT, `${theme}-breakpoint-1440.png`) });

    await page.setViewportSize({ width: 390, height: 844 });
    await waitSidebarClosed(page);
    const small = await sidebarProbe(page);
    report.parts.breakpoint[`${theme}-1440to390`] = small;
    expect(small.expanded === 'false', `${label}: 390 后 aria-expanded=${small.expanded}`);
    expect(small.datasetOpen !== '1', `${label}: 390 后仍标记 sidebar-open`);
    expect(small.rect.right <= 1, `${label}: 关闭态侧栏仍占据屏幕 x=${small.rect.x.toFixed(1)}..${small.rect.right.toFixed(1)}`);
    expect(small.rect.w > 0, `${label}: 390 后侧栏宽度为 0（可能已 display:none，断言失效）`);
    expect(!small.workspaceHit.insideSidebar, `${label}: 工作区中心命中侧栏 (${small.workspaceHit.id || small.workspaceHit.tag})`);
    await page.screenshot({ path: path.join(OUT, `${theme}-breakpoint-390.png`) });

    await page.setViewportSize({ width: 1440, height: 900 });
    await page.waitForTimeout(500);
    const back = await sidebarProbe(page);
    report.parts.breakpoint[`${theme}-390to1440`] = back;
    expect(back.rect.x >= 0 && back.rect.w > 200, `${label}: 回到桌面后侧栏不可见 x=${back.rect.x} w=${back.rect.w}`);
    expect(!!back.gsap, `${label}: 回到桌面后 GSAP 被整体禁用`);

    await page.setViewportSize({ width: 390, height: 844 });
    await page.waitForTimeout(500);
    await page.click('#sidebarMobileBtn');
    await page.waitForTimeout(400);
    const opened = await sidebarProbe(page);
    report.parts.breakpoint[`${theme}-mobile-open`] = opened;
    expect(opened.expanded === 'true', `${label}: 手机端打开后 aria-expanded=${opened.expanded}`);
    expect(opened.rect.x >= -1 && opened.rect.right > 100, `${label}: 手机端打开后侧栏未入屏 x=${opened.rect.x}`);
    expect(opened.scrimVisible, `${label}: 手机端打开后遮罩不可见`);
    await page.screenshot({ path: path.join(OUT, `${theme}-breakpoint-mobile-open.png`) });

    await page.mouse.click(370, 320);
    await page.waitForTimeout(400);
    const closedByScrim = await sidebarProbe(page);
    report.parts.breakpoint[`${theme}-mobile-scrim-close`] = closedByScrim;
    expect(closedByScrim.expanded === 'false', `${label}: 点遮罩后未关闭`);
    expect(closedByScrim.rect.right <= 1, `${label}: 点遮罩后侧栏仍在屏内 right=${closedByScrim.rect.right}`);

    await page.click('#sidebarMobileBtn');
    await page.waitForTimeout(350);
    await page.keyboard.press('Escape');
    await page.waitForTimeout(350);
    const closedByEsc = await sidebarProbe(page);
    report.parts.breakpoint[`${theme}-mobile-esc-close`] = closedByEsc;
    expect(closedByEsc.expanded === 'false', `${label}: Esc 后未关闭`);
    expect(closedByEsc.rect.right <= 1, `${label}: Esc 后侧栏仍在屏内 right=${closedByEsc.rect.right}`);

    await page.setViewportSize({ width: 1440, height: 900 });
    await page.waitForTimeout(400);
    await page.setViewportSize({ width: 390, height: 844 });
    await waitSidebarClosed(page);
    const roundTrip = await sidebarProbe(page);
    report.parts.breakpoint[`${theme}-roundtrip`] = roundTrip;
    expect(roundTrip.expanded === 'false', `${label}: 往返后 aria-expanded=${roundTrip.expanded}`);
    expect(roundTrip.rect.right <= 1, `${label}: 往返后侧栏仍在屏内 right=${roundTrip.rect.right}`);
    expect(errors.length === 0, `${label}: errors ${errors.join('; ')}`);
    await context.close();
  }

  // reduced-motion：没有动画也必须保持状态与视觉一致（防止用清动画掩盖逻辑）。
  {
    const { context, page, errors } = await newPage('dark', { width: 1440, height: 900 }, 'zh', 'reduce', true);
    const label = `${BROWSER}/dark/reduced-motion`;
    await page.waitForFunction(() => !!(window.gsap && window.gsap.timeline), null, { timeout: 15000 });
    await page.waitForTimeout(500);
    const before = await sidebarProbe(page);
    report.parts.breakpoint['reduced-1440'] = before;
    expect(!/transform/.test(before.inline || ''), `${label}: reduced-motion 下不应有 GSAP inline transform (${before.inline})`);
    await page.setViewportSize({ width: 390, height: 844 });
    await waitSidebarClosed(page);
    const small = await sidebarProbe(page);
    report.parts.breakpoint['reduced-390'] = small;
    expect(small.expanded === 'false', `${label}: 390 后 aria-expanded=${small.expanded}`);
    expect(small.rect.right <= 1, `${label}: reduced-motion 下侧栏未移出屏幕 right=${small.rect.right}`);
    expect(!small.workspaceHit.insideSidebar, `${label}: reduced-motion 下工作区中心命中侧栏`);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.waitForTimeout(450);
    const back = await sidebarProbe(page);
    expect(back.rect.x >= 0 && back.rect.w > 200, `${label}: 回到桌面后侧栏不可见`);
    expect(errors.length === 0, `${label}: errors ${errors.join('; ')}`);
    await context.close();
  }

  // ---------- F. 390 向量参数遮挡（真实 768 维 server） ----------
  report.parts.vectorHit = {};
  for (const theme of ['light', 'dark']) {
    const { context, page, errors } = await newPage(theme, { width: 390, height: 844 }, 'zh', 'reduce');
    const label = `${BROWSER}/${theme}/390-vector`;
    const dir = path.join(OUT, `${theme}-390-vector`);
    mkdirSync(dir, { recursive: true });
    await page.click('[data-tab="vectorLabPane"]');
    await page.waitForTimeout(350);

    const states = {};
    let checkedVisible = 0;
    const capture = async (state) => {
      const geo = await page.evaluate(vectorHitProbe);
      states[state] = geo;
      checkedVisible += checkVectorHit(geo, label, state);
      await page.screenshot({ path: path.join(dir, `${state}.png`) });
      return geo;
    };

    await capture('initial');
    await page.click('#vlLoadCandidatesBtn');
    await waitVectorReady(page);
    await page.waitForTimeout(250);
    await capture('candidates-loaded');

    await page.click('#vlSourceTabJson');
    await page.fill('#vlJsonInput', JSON.stringify(VEC));
    await page.waitForTimeout(200);
    await capture('json-input');

    await setVectorScroll(page, 'top');
    await capture('scroll-top');
    await setVectorScroll(page, 'bottom');
    const bottom = await capture('scroll-bottom');
    expect(bottom.labels.some((l) => l.textVisible), `${label}: 滚到底部后参数标签仍不可见（用例失效）`);

    await page.locator('#vlTopK').focus();
    await page.waitForTimeout(250);
    const focused = await capture('keyboard-focus');
    expect(focused.activeId === 'vlTopK', `${label}: 键盘聚焦未落在 top_k (${focused.activeId})`);
    expect(checkedVisible >= 4, `${label}: 参数可见性采样不足 (${checkedVisible})`);

    report.parts.vectorHit[theme] = states;
    writeFileSync(path.join(dir, 'hit-test.json'), JSON.stringify(states, null, 2));
    expect(errors.length === 0, `${label}: errors ${errors.join('; ')}`);
    await context.close();
  }

// ---------- G. 预审修复回归：写入模式 / HiDPI / 触屏 / 长文本 ----------
report.parts.fixAudit = {};
{
  // G1. 写入模式勾选后执行只读查询：必须是 200 + 结果行，不能是 500「API 已移除」
  {
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
    await page.check('#mutationCheck');
    await page.click('#runQueryBtn');
    await waitResultState(page, 'success', 15000).catch(() => {});
    const state = await page.evaluate(() => ({
      state: document.getElementById('resultsContainer')?.dataset.resultState,
      title: document.querySelector('.result-empty-title')?.textContent?.trim() || '',
      detail: document.querySelector('.result-error-detail')?.textContent?.trim() || '',
      rows: document.querySelectorAll('#gridBody tr').length,
    }));
    expect(state.state === 'success', `写入模式下只读查询应成功，实际 ${JSON.stringify(state)}`);
    expect(state.rows > 0, `写入模式下只读查询应有结果行，实际 ${state.rows}`);
    expect(!/API 已移除|Internal server error|500/.test(state.detail + state.title),
      `写入模式下只读查询不应出现 500 文案：${state.detail}`);
    expect(errors.length === 0, `写入模式回归 console/pageerror: ${errors.join('; ')}`);
    report.parts.fixAudit.writeMode = state;
    await context.close();
  }

  // G2. HiDPI：DPR=2 时画布位图必须放大到 CSS×2
  {
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 }, 'zh', 'reduce', false, 2);
    await page.click('[data-tab="graphExplorePane"]');
    await page.evaluate(() => {
      renderGraph([{ a: { id: '1', payload: { name: 'HiDPI', type: 'person' }, vector: [], edges: [] } }]);
      redrawGraph();
    });
    await sleep(300);
    const info = await page.evaluate(() => {
      const c = document.getElementById('standaloneGraphCanvas');
      const r = c.getBoundingClientRect();
      return { dpr: devicePixelRatio, css: [Math.round(r.width), Math.round(r.height)], bitmap: [c.width, c.height] };
    });
    expect(info.dpr === 2, `DPR 应模拟为 2，实际 ${info.dpr}`);
    expect(info.bitmap[0] === info.css[0] * 2 && info.bitmap[1] === info.css[1] * 2,
      `位图应为 CSS×DPR，实际 ${JSON.stringify(info)}`);
    expect(errors.length === 0, `HiDPI 回归 console/pageerror: ${errors.join('; ')}`);
    report.parts.fixAudit.hidpi = info;
    await context.close();
  }

  // G3. 触屏：单指平移 + 双指捏合（PointerEvent 合成，验证事件接线与变换数学）
  {
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
    await page.click('[data-tab="graphExplorePane"]');
    await page.click('#loadWholeGraphBtn');
    await sleep(800);
    const gesture = await page.evaluate(() => {
      const c = document.getElementById('standaloneGraphCanvas');
      const r = c.getBoundingClientRect();
      const fire = (type, id, x, y) => c.dispatchEvent(new PointerEvent(type, {
        pointerId: id, pointerType: 'touch', clientX: x, clientY: y,
        bubbles: true, cancelable: true, isPrimary: id === 1,
      }));
      const panBefore = { x: standaloneTransform.x, y: standaloneTransform.y, k: standaloneTransform.k };
      fire('pointerdown', 1, r.left + 300, r.top + 300);
      fire('pointermove', 1, r.left + 360, r.top + 330);
      fire('pointerup', 1, r.left + 360, r.top + 330);
      const panAfter = { x: standaloneTransform.x, y: standaloneTransform.y, k: standaloneTransform.k };

      const pinchBefore = { x: standaloneTransform.x, y: standaloneTransform.y, k: standaloneTransform.k };
      fire('pointerdown', 1, r.left + 300, r.top + 300);
      fire('pointerdown', 2, r.left + 400, r.top + 300);
      fire('pointermove', 1, r.left + 250, r.top + 300);
      fire('pointermove', 2, r.left + 450, r.top + 300);
      fire('pointerup', 1, r.left + 250, r.top + 300);
      fire('pointerup', 2, r.left + 450, r.top + 300);
      const pinchAfter = { x: standaloneTransform.x, y: standaloneTransform.y, k: standaloneTransform.k };
      return { panBefore, panAfter, pinchBefore, pinchAfter };
    });
    const panDx = gesture.panAfter.x - gesture.panBefore.x;
    const panDy = gesture.panAfter.y - gesture.panBefore.y;
    expect(Math.abs(panDx - 60) < 2 && Math.abs(panDy - 30) < 2,
      `单指拖拽应平移 (+60,+30)，实际 (${panDx.toFixed(1)},${panDy.toFixed(1)})`);
    expect(Math.abs(gesture.panAfter.k - gesture.panBefore.k) < 1e-6, '单指拖拽不应改变缩放');
    const pinchRatio = gesture.pinchAfter.k / gesture.pinchBefore.k;
    expect(Math.abs(pinchRatio - 2) < 0.05, `双指距离翻倍应缩放约 2 倍，实际 ${pinchRatio.toFixed(3)}`);
    expect(errors.length === 0, `触屏回归 console/pageerror: ${errors.join('; ')}`);
    report.parts.fixAudit.touch = gesture;
    await context.close();
  }

  // G4. 长 payload：抽屉内必须换行且不产生横向裁切
  {
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
    const wrap = await page.evaluate(() => {
      renderGraph([{ a: { id: '888', payload: { name: 'A'.repeat(400), hash: 'f'.repeat(600) }, vector: [], edges: [] } }]);
      const node = graphNodes.find((n) => String(n.id) === '888');
      if (node) openNodeDrawer(node);
      const payload = document.getElementById('drawerNodePayload');
      return {
        scrollWidth: payload.scrollWidth,
        clientWidth: payload.clientWidth,
        overflowWrap: getComputedStyle(payload.querySelector('.jt-val-string') || payload).overflowWrap,
        docOverflow: document.documentElement.scrollWidth > innerWidth + 1,
      };
    });
    expect(wrap.scrollWidth <= wrap.clientWidth + 2,
      `长 payload 不应横向裁切：scrollWidth=${wrap.scrollWidth} clientWidth=${wrap.clientWidth}`);
    expect(wrap.overflowWrap !== 'normal', `长 payload 应允许断行，实际 ${wrap.overflowWrap}`);
    expect(wrap.docOverflow === false, '长 payload 不应撑破页面');
    expect(errors.length === 0, `长文本回归 console/pageerror: ${errors.join('; ')}`);
    report.parts.fixAudit.longText = wrap;
    await context.close();
  }

  // G5. 结构/无障碍：tablist 子节点、rail landmark、教程按钮名、h1、toast 播报、tab 方向键
  {
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
    const a11y = await page.evaluate(() => {
      const tabs = document.querySelectorAll('#uspTablist > .usp-tab[role="tab"]');
      const collapseOutside = !document.querySelector('#uspTablist #uspCollapseBtn');
      const rail = document.getElementById('uspRail');
      const railList = document.getElementById('uspRailTablist');
      const tutorialAria = document.getElementById('openTutorialBtn')?.getAttribute('aria-label') || '';
      const h1 = document.querySelector('h1');
      return {
        tabs: tabs.length,
        collapseOutside,
        railTag: rail?.tagName,
        railListRole: railList?.getAttribute('role'),
        tutorialAria,
        h1Text: h1?.textContent?.trim() || '',
      };
    });
    expect(a11y.tabs === 3 && a11y.collapseOutside, `usp-tablist 应只含 3 个 tab 且收起按钮在外：${JSON.stringify(a11y)}`);
    expect(a11y.railTag === 'NAV' && a11y.railListRole === 'tablist', `rail 应保留 nav landmark 且内层为 tablist：${JSON.stringify(a11y)}`);
    expect(a11y.tutorialAria.length > 0, '教程按钮应有 aria-label（390 图标态可访问名）');
    expect(a11y.h1Text.length > 0, '页面应有 h1（可视觉隐藏）');

    await page.evaluate(() => showToast('a11y check'));
    await sleep(150);
    const toast = await page.evaluate(() => {
      const stack = document.getElementById('toastStack');
      return { role: stack?.getAttribute('role'), live: stack?.getAttribute('aria-live') };
    });
    expect(toast.role === 'status' && toast.live === 'polite', `toast 应可被读屏播报：${JSON.stringify(toast)}`);

    const tabKey = await page.evaluate(() => {
      const ai = document.getElementById('uspTabAi');
      ai.click();
      ai.focus();
      document.getElementById('uspTablist').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true, cancelable: true }));
      return {
        active: document.querySelector('.usp-tab.active')?.dataset.uspTab,
        focused: document.activeElement?.dataset?.uspTab,
        tabindexes: [...document.querySelectorAll('#uspTablist > .usp-tab')].map((t) => t.tabIndex),
      };
    });
    expect(tabKey.active === 'detail' && tabKey.focused === 'detail', `←/→ 应切换 panel tab：${JSON.stringify(tabKey)}`);
    expect(tabKey.tabindexes.filter((v) => v === 0).length === 1, `roving tabindex 应只有 1 个 0：${JSON.stringify(tabKey.tabindexes)}`);

    const mainTab = await page.evaluate(() => {
      const q = document.querySelector('[data-tab="queryPane"]');
      q.click();
      q.focus();
      q.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true, cancelable: true }));
      return {
        active: document.querySelector('.tab.active')?.dataset.tab,
        focused: document.activeElement?.dataset?.tab,
        tabindexes: [...document.querySelectorAll('.tabs-bar .tab')].map((t) => t.tabIndex),
      };
    });
    expect(mainTab.active === 'graphExplorePane' && mainTab.focused === 'graphExplorePane',
      `主标签 ←/→ 应切换：${JSON.stringify(mainTab)}`);
    expect(mainTab.tabindexes.filter((v) => v === 0).length === 1,
      `主标签 roving tabindex 应只有 1 个 0：${JSON.stringify(mainTab.tabindexes)}`);
    expect(errors.length === 0, `结构/a11y 回归 console/pageerror: ${errors.join('; ')}`);
    report.parts.fixAudit.a11y = a11y;
    await context.close();
  }

  // G6. 图结果键盘：dock 行 ↑↓ + Enter 打开详情；节点搜索 ↑↓ + Enter
  {
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
    await page.click('[data-tab="graphExplorePane"]');
    await page.evaluate(() => {
      renderGraph([
        { a: { id: '1', payload: { name: 'Alpha 768', type: 'person' }, vector: [], edges: [] } },
        { a: { id: '2', payload: { name: 'Beta 768', type: 'person' }, vector: [], edges: [] } },
        { a: { id: '3', payload: { name: 'Gamma 768', type: 'person' }, vector: [], edges: [] } },
      ]);
      redrawGraph();
    });
    await sleep(600);
    const dock = await page.evaluate(async () => {
      const rows = [...document.querySelectorAll('#graphDockBody tr[data-node-id]')];
      if (!rows.length) return { rows: 0 };
      rows[0].focus();
      const first = document.activeElement?.dataset?.nodeId || '';
      rows[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true }));
      const second = document.activeElement?.dataset?.nodeId || '';
      document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
      await new Promise((r) => setTimeout(r, 400));
      return {
        rows: rows.length, first, second,
        open: !document.getElementById('uspBodyDetail')?.hidden,
        title: document.getElementById('drawerNodeTitle')?.textContent?.trim() || '',
      };
    });
    expect(dock.rows > 1 && dock.first !== dock.second, `dock ↑↓ 应移动 roving 焦点：${JSON.stringify(dock)}`);
    expect(dock.open && dock.title.includes('#'), `dock Enter 应打开节点详情：${JSON.stringify(dock)}`);

    await page.click('[data-tab="graphExplorePane"]');
    await page.click('#graphHudSearch');
    await page.keyboard.type('a'); // Alpha/Beta/Gamma 都命中
    await sleep(350);
    const s1 = await page.evaluate(() => ({
      expanded: document.getElementById('nodeSearchInput')?.getAttribute('aria-expanded'),
      activeDesc: document.getElementById('nodeSearchInput')?.getAttribute('aria-activedescendant') || '',
      options: document.querySelectorAll('#nodeSearchResults [role="option"]').length,
    }));
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('Enter');
    await sleep(500);
    const s2 = await page.evaluate(() => ({
      open: !document.getElementById('uspBodyDetail')?.hidden,
      expanded: document.getElementById('nodeSearchInput')?.getAttribute('aria-expanded'),
    }));
    expect(s1.expanded === 'true' && s1.options > 0 && s1.activeDesc.startsWith('nodeSearchOpt-'),
      `节点搜索应暴露 combobox 状态：${JSON.stringify(s1)}`);
    expect(s2.open && s2.expanded === 'false', `节点搜索 Enter 应打开详情并收起：${JSON.stringify(s2)}`);
    expect(errors.length === 0, `键盘回归 console/pageerror: ${errors.join('; ')}`);
    report.parts.fixAudit.keyboard = { dock, search: { s1, s2 } };
    await context.close();
  }

  // G7. 390 向量结果表：不横向溢出、滚动容器可聚焦
  {
    const { context, page, errors } = await newPage('light', { width: 390, height: 844 });
    await page.click('[data-tab="vectorLabPane"]');
    await page.click('#vlLoadCandidatesBtn');
    await sleep(900);
    await page.selectOption('#vlNodeSelect', { index: 1 }).catch(() => {});
    await page.click('#vlRunBtn');
    await sleep(1300);
    const fit = await page.evaluate(() => {
      const table = document.getElementById('vlResultTable');
      const wrap = document.querySelector('.vl-table-scroll');
      return {
        table: [table?.clientWidth || 0, table?.scrollWidth || 0],
        wrap: [wrap?.clientWidth || 0, wrap?.scrollWidth || 0],
        tab: wrap?.tabIndex,
      };
    });
    expect(fit.table[0] > 0 && fit.table[1] <= fit.table[0] + 2, `390 结果表不应横向溢出：${JSON.stringify(fit)}`);
    expect(fit.wrap[1] <= fit.wrap[0] + 2, `390 结果滚动容器不应横向溢出：${JSON.stringify(fit)}`);
    expect(fit.tab === 0, '结果滚动容器应可键盘聚焦');
    expect(errors.length === 0, `390 向量回归 console/pageerror: ${errors.join('; ')}`);
    report.parts.fixAudit.vectorTable390 = fit;
    await context.close();
  }

  // G8. 运行按钮稳定/历史高亮/示例向量维度/查询舱默认/空闲动效
  {
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
    const brand = await page.evaluate(() => ({
      // V4.1 R2：品牌搬到顶栏（侧栏专注对象/历史/片段）
      text: document.querySelector('.header-brand-copy strong')?.textContent.replace(/\s+/g, ' ').trim(),
      hasSub: document.body.textContent.includes('星尘工作台'),
    }));
    expect(brand.text === 'TriviumDB' && !brand.hasSub, `品牌只保留 TriviumDB：${JSON.stringify(brand)}`);

    const btn = await page.evaluate(async () => {
      const b = document.getElementById('runQueryBtn');
      const before = { w: +b.getBoundingClientRect().width.toFixed(1), svg: b.querySelectorAll('svg').length };
      const orig = window.fetch;
      window.fetch = async (...a) => { const r = await orig(...a); await new Promise((x) => setTimeout(x, 700)); return r; };
      b.click();
      await new Promise((x) => setTimeout(x, 200));
      const during = {
        w: +b.getBoundingClientRect().width.toFixed(1), svg: b.querySelectorAll('svg').length,
        busy: b.classList.contains('is-running'), label: b.textContent.trim(),
      };
      await new Promise((x) => setTimeout(x, 1100));
      window.fetch = orig;
      const after = { w: +b.getBoundingClientRect().width.toFixed(1), svg: b.querySelectorAll('svg').length, busy: b.classList.contains('is-running') };
      return { before, during, after };
    });
    expect(btn.during.w === btn.before.w && btn.after.w === btn.before.w,
      `运行中按钮宽度不得变化：${JSON.stringify(btn)}`);
    expect(btn.during.svg === 1 && btn.during.busy && !btn.after.busy,
      `运行中保留图标并切 busy 态：${JSON.stringify(btn)}`);

    // 历史已从侧栏内联列表改为弹层：点击侧栏入口打开面板，点条目「运行」才执行
    const hist = await page.evaluate(async () => {
      queryHistory = ['FIND {type:"person"} RETURN * LIMIT 5', 'MATCH (a)-[]->(b) RETURN a, b LIMIT 5'];
      renderHistory();
      document.getElementById('historyNavItem').click();
      await new Promise((r) => setTimeout(r, 250));
      const overlayOpen = !document.getElementById('historyOverlay').hidden;
      const items = document.querySelectorAll('#historyList .history-item').length;
      const first = document.querySelector('#historyList .history-query')?.textContent || '';
      const runBtn = document.querySelector('#historyList .history-item .history-actions button[data-history-act="run"]');
      if (!runBtn) return { overlayOpen, items, first, closed: false, editor: '', history: 0 };
      runBtn.click();
      await new Promise((r) => setTimeout(r, 700));
      return {
        overlayOpen,
        items,
        first,
        closed: document.getElementById('historyOverlay').hidden,
        editor: document.getElementById('tqlInput').value,
        history: queryHistory.length,
      };
    });
    expect(hist.overlayOpen === true && hist.items === 2 && hist.first.length > 0,
      `点击侧栏「查询历史」应打开面板并列出条目：${JSON.stringify(hist)}`);
    expect(hist.closed === true && hist.editor.startsWith('FIND'),
      `面板里点「运行」应关闭面板并把该语句写回编辑器：${JSON.stringify(hist)}`);

    const vec = await page.evaluate(async () => {
      // V4.1 R2：导航只切工作区；「按库维度生成向量示例」保留在侧栏片段段
      document.querySelector('.side-tab[data-side-tab="snippets"]').click();
      document.querySelector('[data-action="insert-vector-sample"]').click();
      await new Promise((r) => setTimeout(r, 900));
      const q = document.getElementById('tqlInput').value;
      const m = /\[([^\]]*)\]/.exec(q);
      return { dim: m ? m[1].split(',').filter((s) => s.trim()).length : 0, head: q.slice(0, 24) };
    });
    expect(vec.dim > 100, `内置向量示例必须按库维度生成，不应是 4 维：${JSON.stringify(vec)}`);

    const cockpit = await page.evaluate(async () => {
      document.querySelector('[data-tab="graphExplorePane"]').click();
      await new Promise((r) => setTimeout(r, 400));
      const q = document.getElementById('graphCockpitQuery').value;
      document.getElementById('graphCockpitRun').click();
      await new Promise((r) => setTimeout(r, 1200));
      return {
        q,
        label: document.getElementById('graphCockpitToggle')?.textContent?.trim(),
        hits: document.getElementById('graphCockpitHits')?.textContent?.replace(/\s+/g, ' ').trim() || '',
      };
    });
    expect(/^FIND/.test(cockpit.q) && /结果|rows/.test(cockpit.hits),
      `查询舱默认 FIND 应可直接运行并回填摘要：${JSON.stringify(cockpit)}`);

    const misc = await page.evaluate(async () => {
      for (let i = 0; i < 4; i++) showToast('dedupe-check');
      const duplicates = [...document.querySelectorAll('#toastStack .toast')].filter((t) => t.textContent === 'dedupe-check').length;
      __graphPulses = [];
      simulationRafId = null;
      simulationRunning = false;
      const a0 = auroraRafId;
      startGraphAuroraMotion();
      await new Promise((r) => setTimeout(r, 120));
      return { duplicates, a0, a1: auroraRafId };
    });
    expect(misc.duplicates === 1, `同文案 toast 应去重：${JSON.stringify(misc)}`);
    expect(misc.a1 === null, `无脉冲时不应启动空闲 aurora 重绘：${JSON.stringify(misc)}`);
    expect(errors.length === 0, `运行按钮/历史/示例回归 console/pageerror: ${errors.join('; ')}`);
    report.parts.fixAudit.uiPolish = { brand, btn, hist, vec, cockpit, misc };
    await context.close();
  }
}


  // G9. Canvas 的固有位图尺寸不能撑开 Flex 图容器（DPR/窗口缩放后也必须在工作区内）。
  report.parts.canvasResize = [];
  for (const dpr of [1, 2]) {
    for (const theme of ['dark', 'light']) {
      const context = await browser.newContext({
        viewport: { width: 1440, height: 900 }, deviceScaleFactor: dpr, reducedMotion: 'reduce',
      });
      const page = await context.newPage();
      const pageErrors = [];
      page.on('pageerror', error => pageErrors.push(error.message));
      try {
        await context.addInitScript(t => localStorage.setItem('tdb_theme', t), theme);
        await page.route('**/*', route => {
          const host = new URL(route.request().url()).hostname;
          return host === '127.0.0.1' || host === 'localhost' ? route.continue() : route.abort();
        });
        await page.goto(`http://127.0.0.1:${port}/ui`);
        await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true);
        for (const pane of ['graphExplorePane', 'queryPane']) {
          await page.click(`[data-tab="${pane}"]`);
          if (pane === 'queryPane') await page.click('[data-view="graph"]');
          const ids = pane === 'queryPane'
            ? ['graphCanvas', 'resultsContainer']
            : ['standaloneGraphCanvas', 'graphExplorePane'];
          for (const viewport of [
            { width: 1440, height: 900 }, { width: 1440, height: 560 },
            { width: 390, height: 844 }, { width: 1440, height: 900 },
          ]) {
            await page.setViewportSize(viewport);
            // resize / dvh 重算 / ResizeObserver 分属不同帧，不能把旧视口的正确尺寸当成已完成。
            await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
            await page.waitForFunction(([canvasId, areaId, width, height]) => {
              const c = document.getElementById(canvasId), a = document.getElementById(areaId);
              const r = c.getBoundingClientRect(), ar = a.getBoundingClientRect();
              return innerWidth === width && innerHeight === height
                && ar.bottom <= innerHeight + 1 && r.height > 0 && r.bottom <= ar.bottom + 1
                && Math.abs(c.width - Math.round(c.clientWidth * devicePixelRatio)) <= 1
                && Math.abs(c.height - Math.round(c.clientHeight * devicePixelRatio)) <= 1;
            }, [...ids, viewport.width, viewport.height], { timeout: 2500 }).catch(() => {});
            const geometry = await page.evaluate(([canvasId, areaId]) => {
              const c = document.getElementById(canvasId), a = document.getElementById(areaId);
              const r = c.getBoundingClientRect(), ar = a.getBoundingClientRect();
              return {
                css: [c.clientWidth, c.clientHeight], bitmap: [c.width, c.height], dpr: devicePixelRatio,
                vertical: r.height > 0 && r.top >= ar.top - 1 && r.bottom <= ar.bottom + 1 && ar.bottom <= innerHeight + 1,
                horizontal: r.width > 0 && r.left >= ar.left - 1 && r.right <= ar.right + 1,
                area: { top: ar.top, bottom: ar.bottom }, canvas: { top: r.top, bottom: r.bottom },
              };
            }, ids);
            const label = `${BROWSER}/${theme}/DPR${dpr}/${pane}/${viewport.width}x${viewport.height}`;
            expect(geometry.vertical, `${label}: 画布超出工作区高度 ${JSON.stringify(geometry)}`);
            expect(geometry.horizontal, `${label}: 画布超出工作区宽度 ${JSON.stringify(geometry)}`);
            expect(geometry.bitmap.every((value, i) => Math.abs(value - Math.round(geometry.css[i] * geometry.dpr)) <= 1),
              `${label}: 位图与 CSS × DPR 不一致 ${JSON.stringify(geometry)}`);
            report.parts.canvasResize.push({ label, ...geometry });
            if (!geometry.vertical || !geometry.horizontal) {
              await page.screenshot({ path: path.join(OUT, `FAIL-resize-${theme}-${dpr}-${pane}-${viewport.width}-${viewport.height}.png`) });
            }
          }
        }
        expect(pageErrors.length === 0, `${BROWSER}/${theme}/DPR${dpr}: ${pageErrors.join('; ')}`);
      } finally {
        await context.close();
      }
    }
  }
  // G10. 窄屏节点搜索入口：390/900 由图标按钮展开同一输入框；1024/1440 仍为内联输入。
  // 断言真实点击命中、键盘（↑↓/Enter/Esc）、焦点恢复与节点定位，而不是只看 display。
  report.parts.mobileSearch = [];
  {
    const configs = [
      { viewport: { width: 390, height: 844 }, theme: 'dark', lang: 'zh' },
      { viewport: { width: 390, height: 844 }, theme: 'light', lang: 'en' },
      { viewport: { width: 900, height: 900 }, theme: 'dark', lang: 'en' },
      { viewport: { width: 900, height: 900 }, theme: 'light', lang: 'zh' },
      { viewport: { width: 1024, height: 768 }, theme: 'dark', lang: 'zh' },
      { viewport: { width: 1440, height: 900 }, theme: 'light', lang: 'en' },
    ];
    for (const cfg of configs) {
      const { context, page, errors } = await newPage(cfg.theme, cfg.viewport, cfg.lang);
      const label = `搜索入口/${BROWSER}/${cfg.viewport.width}/${cfg.theme}/${cfg.lang}`;
      await page.evaluate(() => {
        renderGraph([
          { a: { id: '1', payload: { name: 'Alpha 768', type: 'person' }, vector: [], edges: [] } },
          { a: { id: '2', payload: { name: 'Beta 768', type: 'person' }, vector: [], edges: [] } },
          { a: { id: '3', payload: { name: 'Gamma 768', type: 'person' }, vector: [], edges: [] } },
        ]);
        redrawGraph();
      });
      const base = await page.evaluate(() => ({
        toggle: getComputedStyle(document.getElementById('nodeSearchToggle')).display,
        wrap: getComputedStyle(document.getElementById('nodeSearchWrap')).display,
        inputs: document.querySelectorAll('#nodeSearchInput').length,
        expanded: document.getElementById('nodeSearchToggle').getAttribute('aria-expanded'),
        lang: document.documentElement.getAttribute('lang') || '',
        placeholder: document.getElementById('nodeSearchInput').placeholder,
      }));
      expect(base.inputs === 1, `${label}: 搜索输入框必须唯一，实际 ${base.inputs}`);
      if (cfg.viewport.width <= 900) {
        expect(base.toggle !== 'none' && base.wrap === 'none',
          `${label}: 窄屏应有搜索按钮且输入板默认收起 ${JSON.stringify(base)}`);
        await page.click('#nodeSearchToggle');
        await sleep(200);
        const opened = await page.evaluate(() => {
          const toggle = document.getElementById('nodeSearchToggle');
          const input = document.getElementById('nodeSearchInput');
          const b = toggle.getBoundingClientRect();
          const hit = document.elementFromPoint(b.left + b.width / 2, b.top + b.height / 2);
          return {
            wrap: getComputedStyle(document.getElementById('nodeSearchWrap')).display,
            focused: document.activeElement?.id,
            expanded: toggle.getAttribute('aria-expanded'),
            toggleHit: !!hit && (hit === toggle || toggle.contains(hit)),
            inputVisible: input.getBoundingClientRect().height > 0,
          };
        });
        expect(opened.wrap === 'block' && opened.inputVisible,
          `${label}: 点击后搜索板应展开 ${JSON.stringify(opened)}`);
        expect(opened.focused === 'nodeSearchInput' && opened.expanded === 'true',
          `${label}: 展开后应聚焦输入框并同步 aria-expanded ${JSON.stringify(opened)}`);
        expect(opened.toggleHit, `${label}: 搜索按钮应真实可点`);
        await page.keyboard.type('a');
        await sleep(350);
        const typed = await page.evaluate(() => {
          const input = document.getElementById('nodeSearchInput');
          const ib = input.getBoundingClientRect();
          const inputHit = document.elementFromPoint(ib.left + ib.width / 2, ib.top + ib.height / 2);
          const opt = document.querySelector('#nodeSearchResults [role="option"]');
          const ob = opt?.getBoundingClientRect();
          const optHit = ob ? document.elementFromPoint(ob.left + ob.width / 2, ob.top + ob.height / 2) : null;
          return {
            options: document.querySelectorAll('#nodeSearchResults [role="option"]').length,
            expanded: input.getAttribute('aria-expanded'),
            activeDesc: input.getAttribute('aria-activedescendant') || '',
            inputSelf: !!inputHit && (inputHit === input || input.contains(inputHit)),
            optionSelf: !!optHit && !!opt && (optHit === opt || opt.contains(optHit)),
            optionInCanvas: !!ob && ob.top >= 0 && ob.bottom <= innerHeight,
          };
        });
        expect(typed.options > 0 && typed.expanded === 'true' && typed.activeDesc.startsWith('nodeSearchOpt-'),
          `${label}: combobox 应列出候选项 ${JSON.stringify(typed)}`);
        expect(typed.inputSelf && typed.optionSelf && typed.optionInCanvas,
          `${label}: 输入框/结果项必须真正命中且不被裁切 ${JSON.stringify(typed)}`);
        await page.keyboard.press('ArrowDown');
        await page.keyboard.press('Enter');
        await sleep(600);
        const chosen = await page.evaluate(() => ({
          wrap: getComputedStyle(document.getElementById('nodeSearchWrap')).display,
          focused: document.activeElement?.id,
          expanded: document.getElementById('nodeSearchToggle').getAttribute('aria-expanded'),
          detailOpen: !document.getElementById('uspBodyDetail')?.hidden,
          title: document.getElementById('drawerNodeTitle')?.textContent?.trim() || '',
        }));
        expect(chosen.wrap === 'none' && chosen.expanded === 'false',
          `${label}: Enter 选择后搜索板应收起 ${JSON.stringify(chosen)}`);
        expect(chosen.detailOpen && chosen.title.includes('#'),
          `${label}: Enter 应定位并打开节点详情 ${JSON.stringify(chosen)}`);
        expect(chosen.focused === 'nodeSearchToggle',
          `${label}: 选择后焦点应回到搜索按钮 ${JSON.stringify(chosen)}`);
        // 390 下详情面板全宽覆盖 tab 行，先关掉再验证 Esc
        await page.keyboard.press('Escape');
        await sleep(450);
        await page.click('#nodeSearchToggle');
        await sleep(200);
        await page.keyboard.press('Escape');
        await sleep(250);
        const escaped = await page.evaluate(() => ({
          wrap: getComputedStyle(document.getElementById('nodeSearchWrap')).display,
          focused: document.activeElement?.id,
          expanded: document.getElementById('nodeSearchToggle').getAttribute('aria-expanded'),
          inputExpanded: document.getElementById('nodeSearchInput').getAttribute('aria-expanded'),
        }));
        expect(escaped.wrap === 'none' && escaped.expanded === 'false' && escaped.inputExpanded === 'false',
          `${label}: Esc 应收起搜索板与结果 ${JSON.stringify(escaped)}`);
        expect(escaped.focused === 'nodeSearchToggle',
          `${label}: Esc 后焦点应回到搜索按钮 ${JSON.stringify(escaped)}`);
      } else {
        expect(base.toggle === 'none' && base.wrap !== 'none',
          `${label}: 桌面应保留内联输入框且隐藏搜索按钮 ${JSON.stringify(base)}`);
        await page.click('#nodeSearchInput');
        await page.keyboard.type('a');
        await sleep(350);
        const desk = await page.evaluate(() => ({
          options: document.querySelectorAll('#nodeSearchResults [role="option"]').length,
          expanded: document.getElementById('nodeSearchInput').getAttribute('aria-expanded'),
        }));
        expect(desk.options > 0 && desk.expanded === 'true',
          `${label}: 桌面内联搜索仍应可用 ${JSON.stringify(desk)}`);
      }
      expect(errors.length === 0, `${label}: console/pageerror ${errors.join('; ')}`);
      report.parts.mobileSearch.push({ label, base, errors });
      await context.close();
    }
  }

  // G11. 向量工作区底栏统计：与 TQL 分桶，加载/成功/零结果/失败/切换都不互相污染。
  report.parts.statusScope = {};
  {
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
    await page.evaluate(async () => {
      for (let i = 0; i < 11; i += 1) {
        await fetch('/v1/tql', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ query: `CREATE ({name: "Status-${i}", type: "statusprobe"})`, mutation: true }),
        });
      }
    });
    await page.fill('#tqlInput', 'FIND {type: "statusprobe"} RETURN * LIMIT 50');
    await page.click('#runQueryBtn');
    await waitResultState(page, 'success');
    const tql = await page.evaluate(() => ({
      rows: document.getElementById('statusRowCount').textContent,
      time: document.getElementById('statusQueryTime').textContent,
      affected: document.getElementById('statusAffected').textContent,
    }));
    expect(/返回: 11 行/.test(tql.rows), `TQL 底栏应显示 11 行：${JSON.stringify(tql)}`);
    expect(/耗时: [\d.]+ ms/.test(tql.time), `TQL 底栏应显示耗时：${JSON.stringify(tql)}`);
    expect(tql.affected.includes('只读'), `TQL 读查询影响应为只读：${JSON.stringify(tql)}`);

    await page.click('[data-tab="vectorLabPane"]');
    const idle = await page.evaluate(() => ({
      rows: document.getElementById('statusRowCount').textContent,
      time: document.getElementById('statusQueryTime').textContent,
    }));
    expect(idle.rows.includes('—') && idle.time.includes('-'),
      `向量页未检索时应显示空统计而不是沿用 TQL：${JSON.stringify(idle)}`);

    await page.click('#vlLoadCandidatesBtn');
    await waitVectorReady(page);
    await page.selectOption('#vlNodeSelect', { index: 1 });
    await page.fill('#vlTopK', '3');
    // 真实请求延迟 700ms：只为观察加载态，响应体仍来自服务端
    await page.route('**/v1/search/vector**', async (route) => {
      await sleep(700);
      await route.continue();
    });
    await page.click('#vlRunBtn');
    await sleep(250);
    const loading = await page.evaluate(() => ({
      rows: document.getElementById('statusRowCount').textContent,
      time: document.getElementById('statusQueryTime').textContent,
      state: document.getElementById('vlResults').dataset.resultState,
    }));
    await waitVectorState(page, 'success');
    await page.unroute('**/v1/search/vector**');
    const success = await page.evaluate(() => ({
      rows: document.getElementById('statusRowCount').textContent,
      time: document.getElementById('statusQueryTime').textContent,
      affected: document.getElementById('statusAffected').textContent,
      hits: document.querySelectorAll('#vlResultBody tr').length,
    }));
    expect(loading.state === 'loading' && loading.rows.includes('—') && loading.time.includes('-'),
      `向量加载中底栏应为空统计：${JSON.stringify(loading)}`);
    expect(success.hits === 3 && /返回: 3 行/.test(success.rows),
      `向量成功应显示 3 条命中：${JSON.stringify(success)}`);
    expect(/耗时: [\d.]+ ms/.test(success.time), `向量成功应显示检索耗时：${JSON.stringify(success)}`);

    // min_score 高于相似度上限（1）以保证零命中，不依赖本库向量的具体分数
    await page.evaluate(() => { document.getElementById('vlAdvanced').open = true; });
    await page.fill('#vlMinScore', '2');
    await page.click('#vlRunBtn');
    await waitVectorState(page, 'zero');
    const zero = await page.evaluate(() => ({
      rows: document.getElementById('statusRowCount').textContent,
      time: document.getElementById('statusQueryTime').textContent,
    }));
    expect(/返回: 0 行/.test(zero.rows), `向量零结果应显示 0 行：${JSON.stringify(zero)}`);
    expect(/耗时: [\d.]+ ms/.test(zero.time), `向量零结果仍应显示耗时：${JSON.stringify(zero)}`);

    await page.click('#vlSourceTabJson');
    await sleep(200);
    await page.fill('#vlJsonInput', '[1,2,3]');
    await page.click('#vlRunBtn');
    await waitVectorState(page, 'error');
    const failed = await page.evaluate(() => ({
      rows: document.getElementById('statusRowCount').textContent,
      affected: document.getElementById('statusAffected').textContent,
      error: document.getElementById('vlSearchError').textContent.slice(0, 60),
    }));
    expect(failed.rows.includes('—'), `向量失败不得沿用旧行数：${JSON.stringify(failed)}`);
    expect(failed.error.includes('维度'), `向量失败应显示可读原因：${JSON.stringify(failed)}`);

    await page.click('[data-tab="queryPane"]');
    const back = await page.evaluate(() => ({
      rows: document.getElementById('statusRowCount').textContent,
      time: document.getElementById('statusQueryTime').textContent,
    }));
    expect(back.rows === tql.rows, `切回 TQL 应恢复其行数：${JSON.stringify({ back, tql })}`);
    expect(back.time === tql.time, `切回 TQL 不应显示向量耗时：${JSON.stringify({ back, tql })}`);
    expect(errors.length === 0, `底栏统计 console/pageerror: ${errors.join('; ')}`);
    report.parts.statusScope = { tql, idle, loading, success, zero, failed, back };
    await context.close();
  }

  // G13. 非法 query-string：统一 problem JSON（不是 text/plain），且前端能把 detail 呈现为可读错误。
  report.parts.queryError = {};
  {
    const { context, page, errors, allowHttpErrors } = await newPage('light', { width: 1440, height: 900 });
    allowHttpErrors.value = true;
    const api = await page.evaluate(async () => {
      const out = {};
      for (const path of [
        '/v1/nodes/1/edges?limit=oops',
        '/v1/nodes/1/neighbors?depth=oops',
        '/v1/nodes/1/paths/to/1?maxHops=oops',
      ]) {
        const res = await fetch(path);
        const text = await res.text();
        let body = null;
        try { body = JSON.parse(text); } catch (_) { /* text/plain */ }
        out[path] = {
          status: res.status,
          type: res.headers.get('content-type') || '',
          code: body?.code,
          detail: body?.detail,
          textPrefix: body ? '' : text.slice(0, 60),
        };
      }
      renderGraph([{ a: { id: '1', payload: { name: 'Alpha', type: 'person' }, vector: [], edges: [] } }]);
      let thrown = '';
      try {
        await expandNeighborhoodIntoGraph('1', 'oops');
      } catch (e) {
        thrown = e.message;
      }
      return { out, thrown };
    });
    for (const [path, item] of Object.entries(api.out)) {
      expect(item.status === 400 && item.type.includes('application/json') && item.code === 'INVALID_REQUEST',
        `非法查询参数应返回统一 JSON 错误：${path} ${JSON.stringify(item)}`);
      expect((item.detail || '').includes('查询参数无效'), `错误 detail 应可定位参数：${path} ${JSON.stringify(item)}`);
    }
    // 前端消费的就是同一个 detail（expandNeighborhoodIntoGraph 抛出的就是用户看到的提示文本）
    expect(api.thrown.includes('查询参数无效'), `前端错误提示应可读：${api.thrown}`);
    allowHttpErrors.value = false;
    expect(errors.length === 0, `查询参数错误 console/pageerror: ${errors.join('; ')}`);
    report.parts.queryError = api;
    await context.close();
  }

  // G12. Canvas 取景避让：80 节点大图 fit 后节点/标签不得压在浮层下；面板开关与窗口缩放后仍成立。
  report.parts.fitInsets = [];
  {
    const cases = [
      { width: 1440, height: 900, panel: false, theme: 'dark' },
      { width: 1440, height: 900, panel: true, theme: 'light' },
      { width: 390, height: 844, panel: false, theme: 'dark' },
    ];
    for (const cfg of cases) {
      const { context, page, errors } = await newPage(cfg.theme, { width: cfg.width, height: cfg.height });
      const label = `取景/${BROWSER}/${cfg.width}x${cfg.height}/panel=${cfg.panel}/${cfg.theme}`;
      await page.click('[data-tab="graphExplorePane"]');
      await sleep(400);
      await page.evaluate(() => {
        const nodes = Array.from({ length: 80 }, (_, i) => ({
          type: 'node', id: String(i + 1),
          payload: { name: `Node-${i}`, type: ['person', 'paper', 'topic'][i % 3] }, vector: [], edges: [],
        }));
        const rows = [];
        for (let i = 1; i < nodes.length; i += 1) {
          rows.push({ a: nodes[i - 1], b: nodes[i], relation: { type: 'edge', source: nodes[i - 1].id, target: nodes[i].id, label: 'KNOWS' } });
        }
        collapseUnifiedPanel();
        renderGraph(rows);
      });
      await page.waitForFunction(() => simulationRafId === null, null, { timeout: 25000 });
      await page.evaluate(() => setGraphLayoutMode('radial'));
      await sleep(500);
      if (cfg.panel) {
        await page.click('#uspRailDetailBtn');
        await sleep(400);
      }
      await page.click('#gftFitBtnStandalone');
      await sleep(450);
      const probe = async () => page.evaluate(() => {
        const canvas = document.getElementById('standaloneGraphCanvas');
        const rect = canvas.getBoundingClientRect();
        const overlays = [...document.querySelectorAll('#standaloneGraphShell .v2-hud, #standaloneGraphShell .v2-cockpit, #standaloneGraphShell .graph-float-toolbar, #standaloneGraphShell .v2-graph-controls, #standaloneGraphShell .v2-dock, #unifiedSidePanel, .usp-rail')]
          .map((el) => {
            const b = el.getBoundingClientRect();
            return { id: el.id || el.className, l: b.left - rect.left, t: b.top - rect.top, r: b.right - rect.left, b: b.bottom - rect.top, w: b.width, h: b.height };
          })
          .filter((o) => o.w > 1 && o.h > 1 && o.r > 0 && o.b > 0 && o.l < rect.width && o.t < rect.height);
        const hit = (a, b) => a.l < b.r && a.r > b.l && a.t < b.b && a.b > b.t;
        // 标签宽用真实字体测量，避免固定 ±46 把短标签高估成遮挡
        const measureCtx = canvas.getContext('2d');
        measureCtx.save();
        measureCtx.setTransform(1, 0, 0, 1, 0, 0);
        measureCtx.font = `11px ${CANVAS_FONT_FAMILY}`;
        const labelHalf = (node) => Math.min(110, measureCtx.measureText(truncateGraphLabel(node.label, GRAPH_NORMAL_MAX_CHARS)).width / 2 + 2);
        const nodes = graphNodes.filter(isNodeVisible).map((n) => {
          const x = n.x * standaloneTransform.k + standaloneTransform.x;
          const y = n.y * standaloneTransform.k + standaloneTransform.y;
          const r = nodeRadius(n);
          const half = labelHalf(n);
          return { label: n.label, x, y,
            box: { l: x - r, r: x + r, t: y - r, b: y + r },
            labelBox: { l: x - half, r: x + half, t: y + r + 2, b: y + r + 18 } };
        });
        measureCtx.restore();
        const collisions = [];
        for (const n of nodes) {
          for (const o of overlays) {
            if (hit(n.box, o)) collisions.push(['node', n.label, o.id]);
            if (hit(n.labelBox, o)) collisions.push(['label', n.label, o.id]);
          }
        }
        return {
          canvas: [rect.width, rect.height],
          nodes: nodes.length,
          collisions,
          out: nodes.filter((n) => n.box.l < -1 || n.box.r > rect.width + 1 || n.box.t < -1 || n.box.b > rect.height + 1).length,
          panelOpen: document.body.classList.contains('panel-open'),
        };
      });
      const before = await probe();
      expect(before.nodes === 80 && before.collisions.length === 0 && before.out === 0,
        `${label}: fit 后节点/标签不得压住浮层 ${JSON.stringify(before)}`);
      // 窗口缩放后重新 fit 仍成立（不依赖旧的可用区缓存）
      if (cfg.width === 1440) {
        await page.setViewportSize({ width: 1024, height: 768 });
        await sleep(500);
        await page.click('#gftFitBtnStandalone');
        await sleep(500);
        const resized = await probe();
        expect(resized.collisions.length === 0 && resized.out === 0,
          `${label}: 缩到 1024 再 fit 仍不得遮挡 ${JSON.stringify(resized)}`);
      }
      // 面板开→关：fit 后可用区应回到无面板状态
      if (cfg.panel) {
        await page.keyboard.press('Escape');
        await sleep(400);
        await page.click('#gftFitBtnStandalone');
        await sleep(450);
        const closed = await probe();
        expect(closed.panelOpen === false && closed.collisions.length === 0 && closed.out === 0,
          `${label}: 关闭面板后 fit 仍不得遮挡 ${JSON.stringify(closed)}`);
      }
      expect(errors.length === 0, `${label}: console/pageerror ${errors.join('; ')}`);
      report.parts.fitInsets.push({ label, before, errors });
      await context.close();
    }
  }
  // G14. 体验四项：教程排版 / 节点拖拽 / 侧栏只填入 / 工具条自适应「更多」。
  report.parts.ux4 = {};
  {
    const { context, page, errors } = await newPage('dark', { width: 1440, height: 900 });

    // (a) 教程：说明块必须是普通文本排版——pre-wrap 会把源码缩进渲染成行首空格与空行。
    await page.click('#openTutorialBtn');
    await sleep(300);
    const tut = await page.evaluate(() => {
      const pane = document.getElementById('tutorialContentPane');
      const note = pane.querySelector('.tutorial-note');
      const info = {
        note: !!note,
        noteWhiteSpace: note ? getComputedStyle(note).whiteSpace : '',
        noteLeadingSpace: note ? /^\s/.test(note.textContent) : true,
        codeCountStep1: pane.querySelectorAll('.tutorial-code-block').length,
      };
      document.querySelectorAll('#tutorialStepsNav .tutorial-step-item')[1].click();
      const code = pane.querySelector('.tutorial-code-block');
      return {
        ...info,
        codeText: code ? code.textContent : '',
        codeStartsWithBlank: code ? /^\s/.test(code.textContent) : true,
        codeHasTrailingBlankLine: code ? /\s$/.test(code.textContent) : true,
      };
    });
    expect(tut.note === true, '教程应使用 tutorial-note 说明块');
    expect(tut.noteWhiteSpace === 'normal', `说明块不应是 pre-wrap（实际 ${tut.noteWhiteSpace}）`);
    expect(tut.noteLeadingSpace === false, '说明块正文不应以空白字符开头');
    expect(tut.codeCountStep1 === 0, '第 1 步的说明不应伪装成代码块');
    expect(tut.codeStartsWithBlank === false && tut.codeText.startsWith('FIND '),
      `代码块首行不应有空行/缩进：${JSON.stringify(tut.codeText.slice(0, 24))}`);
    expect(tut.codeHasTrailingBlankLine === false, '代码块结尾不应有空白行');
    await page.click('#closeTutorialBtn');
    await sleep(200);

    // (b) 侧栏：片段只填入、导航只切工作区，都不写查询历史。
    const sideBefore = await page.evaluate(() => ({
      hist: queryHistory.length,
      editor: document.getElementById('tqlInput').value,
    }));
    // V4.1 R2：片段收进侧栏「片段」段；切段后点击
    await page.click('.side-tab[data-side-tab="snippets"]');
    await sleep(200);
    await page.click('.sidebar .nav-item[data-snippet]');
    await sleep(400);
    const sideAfter = await page.evaluate(() => ({
      hist: queryHistory.length,
      editor: document.getElementById('tqlInput').value,
      tab: document.querySelector('.tabs-bar .tab.active')?.dataset.tab,
    }));
    expect(sideAfter.hist === sideBefore.hist, '点击片段不应新增查询历史');
    expect(sideAfter.editor === 'FIND {type: "person"} RETURN * LIMIT 20',
      `片段应填入编辑器（实际 ${JSON.stringify(sideAfter.editor)}）`);
    expect(sideAfter.tab === 'queryPane', '片段应切回查询工作台');
    // V4.1 R2：主导航是顶栏 tab（data-action 保留在 tab 上）
    await page.click('[data-action="quick-match"]');
    await sleep(400);
    const navAfter = await page.evaluate(() => ({
      hist: queryHistory.length,
      tab: document.querySelector('.tabs-bar .tab.active')?.dataset.tab,
    }));
    expect(navAfter.hist === sideAfter.hist, '对象导航不应替用户执行查询');
    expect(navAfter.tab === 'graphExplorePane', '对象导航应切到图探索工作区');

    // (c) 节点拖拽：拖动改变世界坐标且不弹面板；单击仍打开详情。
    await page.evaluate(() => {
      const nodes = Array.from({ length: 6 }, (_, i) => ({
        type: 'node', id: String(i + 1),
        payload: { name: `Drag-${i + 1}`, type: 'person' }, vector: [], edges: [],
      }));
      const rows = [];
      for (let i = 1; i < nodes.length; i += 1) {
        rows.push({
          a: nodes[i - 1], b: nodes[i],
          relation: { type: 'edge', source: nodes[i - 1].id, target: nodes[i].id, label: 'KNOWS' },
        });
      }
      collapseUnifiedPanel();
      renderGraph(rows);
    });
    await page.waitForFunction(() => simulationRafId === null, null, { timeout: 15000 });
    await page.evaluate(() => setGraphLayoutMode('radial'));
    await sleep(500);
    await page.click('#gftFitBtnStandalone');
    await sleep(500);
    const dragBefore = await page.evaluate(() => {
      const n = graphNodes.find((x) => isNodeVisible(x));
      const rect = document.getElementById('standaloneGraphCanvas').getBoundingClientRect();
      return {
        id: String(n.id), x: n.x, y: n.y,
        screen: {
          x: rect.left + n.x * standaloneTransform.k + standaloneTransform.x,
          y: rect.top + n.y * standaloneTransform.k + standaloneTransform.y,
        },
        k: standaloneTransform.k,
      };
    });
    await page.mouse.move(dragBefore.screen.x, dragBefore.screen.y);
    await page.mouse.down();
    for (let i = 1; i <= 8; i += 1) {
      await page.mouse.move(dragBefore.screen.x + i * 6, dragBefore.screen.y + i * 4);
    }
    const midDrag = await page.evaluate(() => ({ panelOpen: document.body.classList.contains('panel-open') }));
    await page.mouse.up();
    await sleep(300);
    const dragAfter = await page.evaluate((id) => {
      const n = graphNodes.find((x) => String(x.id) === id);
      return { x: n.x, y: n.y, panelOpen: document.body.classList.contains('panel-open') };
    }, dragBefore.id);
    expect(midDrag.panelOpen === false, '拖拽途中不应弹出节点详情（会挤动画布）');
    expect(dragAfter.panelOpen === false, '拖拽结束不应被当作点击');
    const dx = dragAfter.x - dragBefore.x;
    const dy = dragAfter.y - dragBefore.y;
    expect(dx > 20 && dy > 10, `拖动应真实改变节点世界坐标：dx=${dx.toFixed(1)} dy=${dy.toFixed(1)}（k=${dragBefore.k.toFixed(2)}）`);
    const clickPoint = await page.evaluate((id) => {
      const n = graphNodes.find((x) => String(x.id) === id);
      const rect = document.getElementById('standaloneGraphCanvas').getBoundingClientRect();
      return {
        x: rect.left + n.x * standaloneTransform.k + standaloneTransform.x,
        y: rect.top + n.y * standaloneTransform.k + standaloneTransform.y,
      };
    }, dragBefore.id);
    await page.mouse.click(clickPoint.x, clickPoint.y);
    await sleep(350);
    expect(await page.evaluate(() => document.body.classList.contains('panel-open')),
      '单击节点应打开详情面板');
    expect(await page.evaluate(() => document.getElementById('uspTabDetail').classList.contains('active')),
      '单击节点应激活详情 tab');
    await page.evaluate(() => collapseUnifiedPanel());
    await sleep(300);

    // (d) 工具条：放得下就全铺开（不显示「更多」），放不下才收进菜单，且不横向溢出。
    const toolbar = [];
    for (const width of [1440, 1024, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await sleep(700);
      const t = await page.evaluate(() => {
        const bar = document.querySelector('#standaloneGraphShell .v2-graph-controls');
        const more = document.getElementById('graphMoreBtn');
        const menu = document.getElementById('graphMoreMenu');
        const inBar = [...bar.querySelectorAll('.btn')].filter((b) => b.id !== 'graphMoreBtn').map((b) => b.id);
        const inMenu = [...menu.querySelectorAll('.btn')].map((b) => b.id);
        return {
          inBar, inMenu, moreHidden: more.hidden,
          overflow: bar.scrollWidth > bar.clientWidth + 1,
          dup: inBar.filter((id) => inMenu.includes(id)),
        };
      });
      expect(t.overflow === false, `${width}px: 工具条不应横向溢出（应改收进「更多」）`);
      expect(t.dup.length === 0, `${width}px: 按钮不得同时出现在工具条与「更多」：${t.dup}`);
      expect(t.moreHidden === (t.inMenu.length === 0),
        `${width}px: 「更多」只在确有溢出项时显示（moreHidden=${t.moreHidden}, menu=${t.inMenu.length}）`);
      if (width === 1440) {
        expect(t.inBar.length >= 10, `1440px: 放得下时应尽量铺开，实际 ${t.inBar.length} 项`);
      }
      if (width === 390) {
        expect(t.inMenu.length > 0 && t.inBar.length <= 6,
          `390px: 溢出项应进入「更多」，实际 bar=${t.inBar.length} menu=${t.inMenu.length}`);
      }
      toolbar.push({ width, ...t });
    }
    report.parts.ux4 = { tutorial: tut, sidebar: { sideBefore, sideAfter, navAfter }, drag: { dragBefore, dragAfter, dx, dy }, toolbar };
    expect(errors.length === 0, `UX4 console/pageerror: ${errors.join('; ')}`);
    await context.close();
  }
  // G15. 编辑器行号/执行选中/取消 + 结果表格排序/复制/CSV（真实浏览器行为）。
  report.parts.editorGrid = {};
  {
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
    await page.click('[data-tab="queryPane"]');
    await sleep(300);

    // 行号：3 条逻辑行，其中一条软换行占多行
    const gutter = await page.evaluate(() => {
      const area = document.getElementById('tqlInput');
      const long = 'MATCH (a)-[:KNOWS]->(b) RETURN a, b WHERE a.name == "' + 'x'.repeat(520) + '"';
      area.value = `FIND {type: "person"} RETURN * LIMIT 5\n${long}\n// tail`;
      area.dispatchEvent(new Event('input', { bubbles: true }));
      const lns = [...document.querySelectorAll('#tqlGutterInner .tql-ln')];
      return {
        count: lns.length,
        texts: lns.map((el) => el.textContent).join(','),
        heights: lns.map((el) => Math.round(el.getBoundingClientRect().height)),
      };
    });
    expect(gutter.count === 3 && gutter.texts === '1,2,3', `行号数量/文本应连续：${JSON.stringify(gutter)}`);
    expect(gutter.heights[1] > gutter.heights[0] * 2, `软换行行应占多行高度：${JSON.stringify(gutter.heights)}`);

    // 执行选中：选中第一行后运行，历史记录应是选中片段而不是全文
    const runSelection = await page.evaluate(async () => {
      const area = document.getElementById('tqlInput');
      area.value = 'FIND {type: "person"} RETURN * LIMIT 2\nFIND {type: "paper"} RETURN * LIMIT 2';
      area.dispatchEvent(new Event('input', { bubbles: true }));
      area.focus();
      area.setSelectionRange(0, 38);
      const expected = area.value.slice(0, 38).trim();
      queryHistory = [];
      renderHistory();
      executeCurrentQuery();
      await new Promise((r) => { let n = 0; const t = setInterval(() => { n += 1; if (__activeQueryCount === 0 || n > 40) { clearInterval(t); r(); } }, 100); });
      return { expected, history: queryHistory[0], selectedLength: expected.length };
    });
    expect(runSelection.history === runSelection.expected,
      `Ctrl+Enter 有选区时应只执行选中片段：${JSON.stringify(runSelection)}`);

    // 取消：stub fetch 延迟，点运行后点取消
    const cancel = await page.evaluate(async () => {
      const area = document.getElementById('tqlInput');
      area.value = 'FIND {type: "person"} RETURN * LIMIT 3';
      area.dispatchEvent(new Event('input', { bubbles: true }));
      area.setSelectionRange(0, 0);
      const realFetch = window.fetch;
      window.fetch = (url, opts) => new Promise((resolve, reject) => {
        const timer = setTimeout(() => { window.fetch = realFetch; resolve(realFetch(url, opts)); }, 5000);
        if (opts && opts.signal) {
          opts.signal.addEventListener('abort', () => {
            clearTimeout(timer);
            reject(Object.assign(new Error('aborted'), { name: 'AbortError' }));
          });
        }
      });
      executeCurrentQuery();
      await new Promise((r) => setTimeout(r, 350));
      const running = {
        cancelVisible: !document.getElementById('cancelQueryBtn').hidden,
        state: document.getElementById('resultsContainer').dataset.resultState,
      };
      document.getElementById('cancelQueryBtn').click();
      await new Promise((r) => setTimeout(r, 700));
      window.fetch = realFetch;
      return {
        running,
        state: document.getElementById('resultsContainer').dataset.resultState,
        cancelHidden: document.getElementById('cancelQueryBtn').hidden,
        runDisabled: document.getElementById('runQueryBtn').disabled,
        title: document.querySelector('#gridBody .result-empty-title')?.textContent || '',
      };
    });
    expect(cancel.running.cancelVisible === true, `运行中应显示取消按钮：${JSON.stringify(cancel)}`);
    expect(cancel.state === 'canceled' && cancel.cancelHidden === true && cancel.runDisabled === false,
      `取消后应回到可运行状态：${JSON.stringify(cancel)}`);
    expect(cancel.title.length > 0, '取消后应显示明确状态文案');

    // 排序 + 复制 + CSV：用合成结果避免依赖库内容
    await page.evaluate(() => {
      collapseUnifiedPanel();
      renderGrid([
        { type: 'node', id: '3', rank: 30 },
        { type: 'node', id: '1', rank: 10 },
        { type: 'node', id: '2', rank: 20 },
      ]);
      window.__copied = [];
      try {
        Object.defineProperty(navigator, 'clipboard', {
          configurable: true,
          value: { writeText: (text) => { window.__copied.push(text); return Promise.resolve(); } },
        });
      } catch (_) { /* 环境不允许覆盖 */ }
    });
    const order = () => page.evaluate(() => [...document.querySelectorAll('#gridBody tr[data-row-index]')]
      .map((tr) => tr.lastElementChild?.textContent).join(','));
    await page.click('#gridHead th[data-column-key="rank"]');
    await sleep(250);
    expect(await order() === '10,20,30', `点击表头应升序：${await order()}`);
    expect(await page.getAttribute('#gridHead th[data-column-key="rank"]', 'aria-sort') === 'ascending', '升序 aria-sort 缺失');
    await page.click('#gridHead th[data-column-key="rank"]');
    await sleep(250);
    expect(await order() === '30,20,10', `再点应降序：${await order()}`);
    // 键盘排序（Enter）同样生效
    await page.focus('#gridHead th[data-column-key="rank"]');
    await page.keyboard.press('Enter');
    await sleep(250);
    expect(await order() === '10,20,30', `Enter 应回到升序：${await order()}`);

    // 右键菜单复制单元格
    await page.click('#gridBody tr[data-row-index="0"] td[data-col-key="rank"]', { button: 'right' });
    await sleep(250);
    const menuOpen = await page.evaluate(() => !document.getElementById('gridCtxMenu').hidden);
    expect(menuOpen === true, '右键应打开结果表格菜单');
    await page.click('#gridCtxMenu button[data-ctx-act="cell"]');
    await sleep(200);
    const copied = await page.evaluate(() => window.__copied);
    expect(copied[0] === '10', `复制单元格内容应为 10，实际 ${JSON.stringify(copied)}`);

    // CSV：真实下载事件 + 文件名（V4.1 R2 起导出收进「结果操作」菜单）
    await page.click('#resultOpsBtn');
    await sleep(150);
    const [download] = await Promise.all([
      page.waitForEvent('download', { timeout: 8000 }),
      page.click('#exportCsvBtn'),
    ]);
    expect(/\.csv$/.test(download.suggestedFilename()), `CSV 文件名应以 .csv 结尾：${download.suggestedFilename()}`);

    report.parts.editorGrid = { gutter, runSelection, cancel, copied };
    expect(errors.length === 0, `编辑器/表格 console/pageerror: ${errors.join('; ')}`);
    await context.close();
  }
  // G16. 对象树（类型/边标签/索引/向量）+ 导入向导（CSV 节点与关系）。
  report.parts.objectTree = {};
  {
    const { context, page, errors } = await newPage('dark', { width: 1440, height: 900 });
    await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true);
    // V4.1 R2：导入默认进入审阅队列；本场景验证「关闭审阅后立即导入」的既有路径
    await page.evaluate(() => {
      const c = document.getElementById('txModeCheck');
      c.checked = false;
      c.dispatchEvent(new Event('change', { bubbles: true }));
    });
    // 先保证库里一定有数据（空库时对象树只显示空态，无法验证分组）
    await page.evaluate(async () => {
      const rows = [
        { id: 8701, vector: new Array(768).fill(0), payload: { name: 'Seed A', type: 'seedtype' } },
        { id: 8702, vector: new Array(768).fill(0), payload: { name: 'Seed B', type: 'seedtype' } },
      ];
      await fetch('/v1/import/nodes', {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-ndjson' },
        body: rows.map((r) => JSON.stringify(r)).join('\n'),
      });
      await loadObjectTree();
    });
    // 树是异步加载的：等四组出现（数据来自 MATCH 采样 + /v1/indexes）
    await page.waitForFunction(() => document.querySelectorAll('#objectTree .ot-group').length === 4,
      null, { timeout: 15000 });
    const tree = await page.evaluate(() => ({
      groups: [...document.querySelectorAll('#objectTree .ot-group')].map((g) => g.querySelector('summary span')?.textContent),
      types: [...document.querySelectorAll('#objectTree .ot-row[data-ot-kind="type"]')].map((r) => r.querySelector('.ot-row-name')?.textContent),
      labels: [...document.querySelectorAll('#objectTree .ot-row[data-ot-kind="label"]')].map((r) => r.querySelector('.ot-row-name')?.textContent),
    }));
    expect(tree.groups.length === 4, `对象树应有四组：${JSON.stringify(tree.groups)}`);
    expect(tree.types.length > 0 && tree.types[0] !== '(未标注)', `应归纳出节点类型：${JSON.stringify(tree.types)}`);

    // 点类型行：只填入，不执行、不写历史
    const clickFill = await page.evaluate(async () => {
      const row = document.querySelector('#objectTree .ot-row[data-ot-kind="type"]');
      const name = row.querySelector('.ot-row-name').textContent;
      const before = queryHistory.length;
      row.click();
      await new Promise((r) => setTimeout(r, 250));
      return {
        name,
        editor: document.getElementById('tqlInput').value,
        tab: document.querySelector('.tabs-bar .tab.active')?.dataset.tab,
        added: queryHistory.length - before,
      };
    });
    expect(clickFill.tab === 'queryPane' && clickFill.added === 0,
      `点对象应只填入并切到查询台：${JSON.stringify(clickFill)}`);
    expect(clickFill.editor.startsWith('FIND {type: "') && clickFill.editor.includes(clickFill.name),
      `应生成该类型的 FIND：${JSON.stringify(clickFill)}`);

    // 行尾 ▶：填入并执行
    await page.evaluate(() => document.querySelector('#objectTree .ot-row[data-ot-kind="type"] .ot-run').click());
    await page.waitForFunction(() => document.getElementById('resultsContainer').dataset.resultState === 'success',
      null, { timeout: 10000 });
    const runRows = await page.evaluate(() => document.querySelectorAll('#gridBody tr[data-row-index]').length);
    expect(runRows > 0, `▶ 应执行并回填结果，实际 ${runRows} 行`);

    // 索引：创建 → 刷新出现 → 两次点击删除
    await page.evaluate(async () => {
      await fetch('/v1/indexes', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ kind: 'hash', fields: ['name'] }),
      });
      await loadObjectTree();
    });
    await page.waitForFunction(() => document.querySelector('#objectTree [data-ot-kind="index"]'), null, { timeout: 8000 });
    const indexRow = await page.evaluate(() => {
      const row = document.querySelector('#objectTree [data-ot-kind="index"]');
      return { name: row.querySelector('.ot-row-name').textContent, danger: !!row.querySelector('.ot-danger') };
    });
    expect(indexRow.danger === true && /hash/.test(indexRow.name), `索引行应可删除：${JSON.stringify(indexRow)}`);
    // 索引组默认收起：先展开（真实用户路径），再点删除
    await page.evaluate(() => {
      const groups = [...document.querySelectorAll('#objectTree .ot-group')];
      const target = groups.find((g) => /索引|Index/.test(g.querySelector('summary')?.textContent || ''));
      if (target && !target.open) target.querySelector('summary')?.click();
    });
    await sleep(200);
    // 第一次点只进入确认态，第二次才真正删除（内嵌 webview 不用原生 confirm）
    await page.click('#objectTree [data-ot-kind="index"] .ot-danger');
    await sleep(150);
    const stillThere = await page.evaluate(() => !!document.querySelector('#objectTree [data-ot-kind="index"]'));
    expect(stillThere === true, '第一次点击不应直接删除');
    await page.click('#objectTree [data-ot-kind="index"] .ot-danger');
    await page.waitForFunction(() => !document.querySelector('#objectTree [data-ot-kind="index"]'), null, { timeout: 8000 });

    // 导入：节点 CSV
    await page.click('#openImportBtn');
    await page.waitForFunction(() => !document.getElementById('importOverlay').hidden);
    await page.fill('#importPaste', 'id,name,type\n8801,Import A,imptype\n8802,Import B,imptype\n');
    await page.waitForFunction(() => !document.getElementById('importRunBtn').disabled, null, { timeout: 8000 });
    const preview = await page.evaluate(() => ({
      summary: document.getElementById('importSummary').textContent,
      rows: document.querySelectorAll('#importPreview tbody tr').length,
      first: [...document.querySelectorAll('#importPreview tbody tr td')].map((td) => td.textContent).slice(0, 4),
    }));
    expect(preview.rows === 2 && /imptype/.test(preview.first.join(' ')),
      `导入预览应展示解析结果：${JSON.stringify(preview)}`);
    await page.click('#importRunBtn');
    await page.waitForFunction(() => document.getElementById('importStatus').className.includes('is-ok'), null, { timeout: 15000 });
    const importedText = await page.textContent('#importStatus');
    expect(/2|两/.test(importedText), `导入结果应报条数：${importedText}`);

    // 导入：关系 CSV（切到关系 tab）
    await page.click('#importTabEdges');
    await page.fill('#importPaste', 'source,target,label,weight\n8801,8802,IMPORTED,1.5\n');
    await page.waitForFunction(() => !document.getElementById('importRunBtn').disabled, null, { timeout: 8000 });
    await page.click('#importRunBtn');
    await page.waitForFunction(() => document.getElementById('importStatus').className.includes('is-ok'), null, { timeout: 15000 });
    await page.keyboard.press('Escape');
    await sleep(300);
    const closed = await page.evaluate(() => document.getElementById('importOverlay').hidden);
    expect(closed === true, 'Esc 应关闭导入向导');

    // 树刷新后应出现新类型；边用 API 复核（采样只有前 200 个节点，不能保证覆盖到 8801）
    await page.waitForFunction(() => [...document.querySelectorAll('#objectTree .ot-row[data-ot-kind="type"]')]
      .some((r) => r.querySelector('.ot-row-name').textContent === 'imptype'), null, { timeout: 15000 });
    const labels = await page.evaluate(() => [...document.querySelectorAll('#objectTree .ot-row[data-ot-kind="label"]')]
      .map((r) => r.querySelector('.ot-row-name').textContent));
    const edgeProbe = await page.evaluate(async () => {
      const res = await fetch('/v1/nodes/8801/edges?direction=out');
      const body = await res.json().catch(() => ({}));
      return { ok: res.ok, labels: (body.edges || []).map((e) => e.label) };
    });
    expect(edgeProbe.ok && edgeProbe.labels.includes('IMPORTED'),
      `导入的关系应真实落库：${JSON.stringify(edgeProbe)}`);

    report.parts.objectTree = { tree, clickFill, runRows, indexRow, preview, importedText, labels };
    expect(errors.length === 0, `对象树/导入 console/pageerror: ${errors.join('; ')}`);
    await context.close();
  }
  // G17. 列宽拖拽 / 列显隐 / 双击编辑 payload / 右键删除（真实写回 /v1/transactions 与 delete-many）。
  report.parts.gridEditing = {};
  {
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
    await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true);
    // V4.1 R2：界面编辑默认进入审阅队列；本场景验证「关闭审阅后立即写库」的既有路径
    await page.evaluate(() => {
      const c = document.getElementById('txModeCheck');
      c.checked = false;
      c.dispatchEvent(new Event('change', { bubbles: true }));
    });
    // 造一条边，保证两列（a / b）都在
    await page.evaluate(async () => {
      const rows = [
        { id: 8901, vector: new Array(768).fill(0), payload: { name: 'Edit Me', type: 'edittest' } },
        { id: 8902, vector: new Array(768).fill(0), payload: { name: 'Edit Peer', type: 'edittest' } },
      ];
      await fetch('/v1/import/nodes', {
        method: 'POST', headers: { 'Content-Type': 'application/x-ndjson' },
        body: rows.map((r) => JSON.stringify(r)).join('\n'),
      });
      await fetch('/v1/import/edges', {
        method: 'POST', headers: { 'Content-Type': 'application/x-ndjson' },
        body: JSON.stringify({ source: 8901, target: 8902, label: 'EDGE_TEST', weight: 1 }),
      });
    });
    await page.evaluate(() => {
      document.querySelector('[data-tab="queryPane"]').click();
      setEditorValue('MATCH (a)-[]->(b) RETURN a, b LIMIT 20');
      executeCurrentQuery();
    });
    await page.waitForFunction(() => document.getElementById('resultsContainer').dataset.resultState === 'success',
      null, { timeout: 15000 });
    await sleep(400);

    const widthOf = (key) => page.evaluate((k) => {
      const th = document.querySelector(`#gridHead th[data-column-key="${k}"]`);
      return th ? Math.round(th.getBoundingClientRect().width) : 0;
    }, key);
    const beforeWidth = await widthOf('a');
    const handle = await page.$('#gridHead th[data-column-key="a"] .grid-col-resizer');
    const box = await handle.boundingBox();
    await page.mouse.move(box.x + 3, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + 3 + 110, box.y + box.height / 2, { steps: 8 });
    await page.mouse.up();
    await sleep(300);
    const afterWidth = await widthOf('a');
    expect(afterWidth - beforeWidth > 60, `拖拽应加宽列：${beforeWidth} → ${afterWidth}`);
    // 排序后宽度保持（不因重渲染漂移）
    await page.click('#gridHead th[data-column-key="a"]');
    await sleep(350);
    const widthAfterSort = await widthOf('a');
    expect(Math.abs(widthAfterSort - afterWidth) <= 2,
      `排序后列宽应保持：${afterWidth} → ${widthAfterSort}`);
    expect(await page.getAttribute('#gridHead th[data-column-key="a"]', 'aria-sort') === 'ascending',
      '拖列宽后点击表头仍应正常排序');

    // 列显隐（V4.1 R2：先打开「结果操作」菜单，再点「列」）
    await page.click('#resultOpsBtn');
    await sleep(150);
    await page.click('#gridColumnsBtn');
    await sleep(250);
    expect(await page.evaluate(() => !document.getElementById('gridColMenu').hidden), '列面板应打开');
    await page.evaluate(() => {
      const input = document.querySelector('#gridColMenu input[data-col-key="b"]');
      input.checked = false;
      input.dispatchEvent(new Event('change', { bubbles: true }));
    });
    await sleep(300);
    const hidden = await page.evaluate(() => ({
      headers: [...document.querySelectorAll('#gridHead th[data-column-key]')].map((th) => th.dataset.columnKey).join(','),
      csv: buildGridCsv().split('\r\n')[0],
    }));
    expect(hidden.headers === '#,a' && !hidden.csv.includes(',b'),
      `隐藏列应同时作用于表头与 CSV：${JSON.stringify(hidden)}`);
    await page.evaluate(() => setGridColumnHidden('b', false));
    await sleep(250);

    // 双击编辑 payload
    const targetId = await page.evaluate(() => {
      const row = gridDataRows.find((r) => r.a && String(r.a.id) === '8901');
      return row ? String(row.a.id) : '';
    });
    expect(targetId === '8901', '样例行应存在于结果中');
    const cellIndex = await page.evaluate(() => gridDataRows.findIndex((r) => r.a && String(r.a.id) === '8901'));
    await page.dblclick(`#gridBody tr[data-row-index="${cellIndex}"] td[data-col-key="a"]`);
    await sleep(400);
    expect(await page.evaluate(() => !document.getElementById('payloadEditor').hidden), '双击应打开 payload 编辑器');
    await page.fill('#payloadEditorText', '{"name":"Edited In Grid","type":"edittest","marker":"g17"}');
    await page.click('#payloadEditorSave');
    await page.waitForFunction(() => document.getElementById('payloadEditor').hidden, null, { timeout: 10000 });
    const saved = await page.evaluate(async () => {
      const res = await fetch('/v1/nodes/8901');
      const body = await res.json();
      return body.node && body.node.payload;
    });
    expect(saved && saved.marker === 'g17' && saved.name === 'Edited In Grid',
      `保存应真实写回：${JSON.stringify(saved)}`);

    // 右键删除（两步）
    await page.click(`#gridBody tr[data-row-index="${cellIndex}"] td[data-col-key="a"]`, { button: 'right' });
    await sleep(300);
    const menuItems = await page.evaluate(() => [...document.querySelectorAll('#gridCtxMenu button')].map((b) => b.textContent));
    expect(menuItems.some((x) => x.includes('payload') || x.includes('payload')) && menuItems.some((x) => x.includes('删除')),
      `节点单元格右键应有编辑/删除：${JSON.stringify(menuItems)}`);
    await page.click('#gridCtxMenu button[data-ctx-act="delete"]');
    await sleep(250);
    const armedLabel = await page.textContent('#gridCtxMenu button[data-ctx-act="delete"]');
    expect(/8901/.test(armedLabel), `第一次点击应进入确认态：${armedLabel}`);
    await page.click('#gridCtxMenu button[data-ctx-act="delete"]');
    await sleep(900);
    // 从 Node 侧复核（页面内 fetch 404 会写一条 console error，掩盖真实噪声）
    const afterDelete = (await fetch(`http://127.0.0.1:${port}/v1/nodes/8901`)).status;
    expect(afterDelete === 404, `删除后节点应不存在：HTTP ${afterDelete}`);

    report.parts.gridEditing = { beforeWidth, afterWidth, widthAfterSort, hidden, saved, menuItems, afterDelete };
    expect(errors.length === 0, `列宽/编辑/删除 console/pageerror: ${errors.join('; ')}`);
    await context.close();
  }
  // G18. 批量选择：全选/半选、导出选中、批量删除（一次 delete-many）。
  report.parts.batchSelection = {};
  {
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
    await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true);
    // V4.1 R2：本场景验证「关闭审阅后立即批量删除」的既有路径
    await page.evaluate(() => {
      const c = document.getElementById('txModeCheck');
      c.checked = false;
      c.dispatchEvent(new Event('change', { bubbles: true }));
    });
    await page.evaluate(async () => {
      const rows = [8903, 8904, 8905].map((id) => ({
        id, vector: new Array(768).fill(0), payload: { name: `Batch ${id}`, type: 'batchtest' },
      }));
      await fetch('/v1/import/nodes', {
        method: 'POST', headers: { 'Content-Type': 'application/x-ndjson' },
        body: rows.map((r) => JSON.stringify(r)).join('\n'),
      });
      document.querySelector('[data-tab="queryPane"]').click();
      document.querySelector('[data-view="grid"]').click();
      setEditorValue('FIND {type: "batchtest"} RETURN * LIMIT 20');
      executeCurrentQuery();
    });
    await page.waitForFunction(() => document.getElementById('resultsContainer').dataset.resultState === 'success',
      null, { timeout: 15000 });
    await sleep(400);
    const rowCount = await page.evaluate(() => document.querySelectorAll('#gridBody tr[data-row-index]').length);
    expect(rowCount === 3, `应有 3 行 batchtest 结果，实际 ${rowCount}`);

    // 全选 → 操作条出现
    await page.click('#gridHead .grid-sel-th input');
    await sleep(250);
    const selectedAll = await page.evaluate(() => ({
      size: gridSelection.size,
      barHidden: document.getElementById('gridSelectionBar').hidden,
      count: document.getElementById('gridSelectionCount').textContent,
      allChecked: [...document.querySelectorAll('#gridBody input.grid-sel-input')].every((i) => i.checked),
    }));
    expect(selectedAll.size === 3 && selectedAll.barHidden === false && selectedAll.allChecked === true,
      `全选应选中 3 行并显示操作条：${JSON.stringify(selectedAll)}`);
    expect(/3/.test(selectedAll.count), `计数文案应含 3：${selectedAll.count}`);

    // 取消一行 → 半选
    await page.evaluate(() => {
      const input = document.querySelector('#gridBody tr[data-row-index="0"] input.grid-sel-input');
      input.checked = false;
      input.dispatchEvent(new Event('change', { bubbles: true }));
    });
    await sleep(200);
    const partial = await page.evaluate(() => ({
      size: gridSelection.size,
      indeterminate: document.querySelector('#gridHead .grid-sel-th input').indeterminate,
    }));
    expect(partial.size === 2 && partial.indeterminate === true,
      `取消一行后应半选：${JSON.stringify(partial)}`);

    // 导出选中：真实下载 + 内容只含 2 行
    const csvText = await page.evaluate(() => buildGridCsv(selectedGridRows()));
    expect(csvText.trim().split('\r\n').length === 3, `导出选中应 2 行数据：${csvText.split('\r\n').length}`);
    const [download] = await Promise.all([
      page.waitForEvent('download', { timeout: 8000 }),
      page.click('#gridSelExportBtn'),
    ]);
    expect(/\.csv$/.test(download.suggestedFilename()), `导出选中文件名应 .csv：${download.suggestedFilename()}`);

    // 批量删除（两步），删掉选中的 2 个
    const selectedIds = await page.evaluate(() => [...gridSelection]);
    await page.evaluate(() => {
      window.__batchConfirmClicks = [];
      document.getElementById('gridSelDeleteBtn').addEventListener('click', () => {
        queueMicrotask(() => {
          const button = document.getElementById('gridSelDeleteBtn');
          window.__batchConfirmClicks.push({ armed: button.dataset.armed, text: button.textContent, time: performance.now() });
        });
      });
    });
    const deletion = page.waitForResponse(response => response.url().endsWith('/v1/nodes/delete-many') && response.request().method() === 'POST');
    // 同一次真实指针输入完成两次点击，避免自动化 IPC 延迟耗尽四秒确认窗口。
    await page.click('#gridSelDeleteBtn', { clickCount: 2, delay: 100 });
    const deleteResponse = await deletion;
    const confirmClicks = await page.evaluate(() => window.__batchConfirmClicks);
    expect(confirmClicks.length === 2 && confirmClicks[0].armed === '1' && /2/.test(confirmClicks[0].text)
      && confirmClicks[1].time - confirmClicks[0].time < 4000, `必须实际经过四秒内的两步确认：${JSON.stringify(confirmClicks)}`);
    expect(deleteResponse.status() === 200, `批删请求必须成功：${deleteResponse.status()}`);
    await page.waitForFunction(() => gridSelection.size === 0 && document.getElementById('gridSelectionBar').hidden, null, { timeout: 15000 });
    const statuses = [];
    for (const id of selectedIds) {
      statuses.push((await fetch(`http://127.0.0.1:${port}/v1/nodes/${id}`)).status);
    }
    // 未选中的那个必须保留（选择集合取决于排序，不能写死某个 id）
    const survived = [8903, 8904, 8905].filter((id) => !selectedIds.includes(String(id)));
    const survivedStatus = [];
    for (const id of survived) {
      survivedStatus.push((await fetch(`http://127.0.0.1:${port}/v1/nodes/${id}`)).status);
    }
    expect(statuses.every((s) => s === 404), `选中节点应被删除：${JSON.stringify({ selectedIds, statuses })}`);
    expect(survivedStatus.every((s) => s === 200),
      `未选中的节点必须保留：${JSON.stringify({ survived, survivedStatus })}`);
    const afterDelete = await page.evaluate(() => ({
      rows: document.querySelectorAll('#gridBody tr[data-row-index]').length,
      size: gridSelection.size,
      barHidden: document.getElementById('gridSelectionBar').hidden,
    }));
    expect(afterDelete.size === 0 && afterDelete.barHidden === true && afterDelete.rows === 1,
      `删除后应只剩 1 行、清空选择并收起操作条：${JSON.stringify(afterDelete)}`);

    report.parts.batchSelection = { rowCount, selectedAll, partial, selectedIds, statuses, afterDelete };
    expect(errors.length === 0, `批量选择 console/pageerror: ${errors.join('; ')}`);
    await context.close();
  }
  // G19. 查询多标签 + 事务模式（真实写回：暂存不动库、提交原子生效、回滚丢弃）。
  report.parts.tabsTx = {};
  {
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
    await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true);
    const idA = 97000 + (Date.now() % 900) * 2;
    const idB = idA + 1;
    await page.evaluate(async ([a, b]) => {
      const rows = [
        { id: a, vector: new Array(768).fill(0), payload: { name: 'TabTx A', type: 'tabtxtest' } },
        { id: b, vector: new Array(768).fill(0), payload: { name: 'TabTx B', type: 'tabtxtest' } },
      ];
      await fetch('/v1/import/nodes', {
        method: 'POST', headers: { 'Content-Type': 'application/x-ndjson' },
        body: rows.map((r) => JSON.stringify(r)).join('\n'),
      });
      document.querySelector('[data-tab="queryPane"]').click();
    }, [idA, idB]);
    await sleep(300);

    // 标签 1 跑查询
    await page.evaluate(() => {
      setEditorValue('FIND {type: "tabtxtest"} RETURN * LIMIT 10');
      executeCurrentQuery();
    });
    await page.waitForFunction(() => document.getElementById('resultsContainer').dataset.resultState === 'success',
      null, { timeout: 15000 });
    await sleep(300);
    const tab1 = await page.evaluate(() => ({
      tabs: document.querySelectorAll('#queryTabsList .query-tab').length,
      text: document.getElementById('tqlInput').value,
      rows: document.querySelectorAll('#gridBody tr[data-row-index]').length,
      title: document.querySelector('#queryTabsList .query-tab.active .query-tab-title')?.textContent,
    }));
    expect(tab1.tabs === 1 && tab1.rows >= 2, `首个标签应有结果：${JSON.stringify(tab1)}`);
    expect(tab1.title && tab1.title.length > 0, `标签标题应来自查询：${JSON.stringify(tab1)}`);

    // 新建标签 → 独立文本与结果
    await page.click('#queryTabAddBtn');
    await sleep(250);
    await page.evaluate(() => {
      setEditorValue('FIND {type: "person"} RETURN * LIMIT 3');
      executeCurrentQuery();
    });
    await page.waitForFunction(() => document.getElementById('resultsContainer').dataset.resultState !== 'running',
      null, { timeout: 15000 }).catch(() => {});
    await sleep(600);
    const tab2 = await page.evaluate(() => ({
      tabs: document.querySelectorAll('#queryTabsList .query-tab').length,
      text: document.getElementById('tqlInput').value,
      state: document.getElementById('resultsContainer').dataset.resultState,
    }));
    expect(tab2.tabs === 2 && tab2.text.includes('person'), `第二个标签应独立：${JSON.stringify(tab2)}`);

    // 切回标签 1：文本/结果恢复
    const firstTabId = await page.evaluate(() => document.querySelectorAll('#queryTabsList .query-tab')[0].dataset.queryTabId);
    await page.click(`[data-query-tab-id="${firstTabId}"]`);
    await sleep(500);
    const back1 = await page.evaluate(() => ({
      text: document.getElementById('tqlInput').value,
      state: document.getElementById('resultsContainer').dataset.resultState,
      rows: document.querySelectorAll('#gridBody tr[data-row-index]').length,
    }));
    expect(back1.text === tab1.text && back1.state === 'success' && back1.rows === tab1.rows,
      `切回标签 1 应恢复文本与结果：${JSON.stringify({ back1, tab1 })}`);

    // 切回标签 2：结果也恢复
    const secondTabId = await page.evaluate(() => document.querySelectorAll('#queryTabsList .query-tab')[1].dataset.queryTabId);
    await page.click(`[data-query-tab-id="${secondTabId}"]`);
    await sleep(600);
    const back2 = await page.evaluate(() => ({
      text: document.getElementById('tqlInput').value,
      state: document.getElementById('resultsContainer').dataset.resultState,
    }));
    expect(back2.text.includes('person') && back2.state === 'success',
      `切回标签 2 应恢复：${JSON.stringify(back2)}`);

    // 事务模式：暂存删除不动库
    await page.evaluate(() => {
      const c = document.getElementById('txModeCheck');
      c.checked = true;
      c.dispatchEvent(new Event('change', { bubbles: true }));
      setEditorValue('FIND {type: "tabtxtest"} RETURN * LIMIT 10');
      executeCurrentQuery();
    });
    await page.waitForFunction(() => document.getElementById('resultsContainer').dataset.resultState === 'success',
      null, { timeout: 15000 });
    await sleep(400);
    const txStaged = await page.evaluate(async (a) => {
      const idx = gridDataRows.findIndex((r) => r._ && String(r._.id) === String(a));
      const node = gridDataRows[idx] && gridDataRows[idx]._;
      if (!node) return { ok: false };
      await deleteGridNode(node);
      return {
        ok: true,
        ops: txOperations.length,
        barHidden: document.getElementById('txBar').hidden,
        count: document.getElementById('txCount').textContent,
      };
    }, idA);
    expect(txStaged.ok && txStaged.ops === 1 && txStaged.barHidden === false,
      `删除应被暂存并显示事务条：${JSON.stringify(txStaged)}`);
    expect((await fetch(`http://127.0.0.1:${port}/v1/nodes/${idA}`)).status === 200,
      '暂存阶段服务端不应删除');

    // 回滚：丢弃，库里仍有
    await page.click('#txRollbackBtn');
    await sleep(1200);
    const afterRollback = await page.evaluate(() => ({ ops: txOperations.length }));
    expect(afterRollback.ops === 0, '回滚应清空暂存区');
    expect((await fetch(`http://127.0.0.1:${port}/v1/nodes/${idA}`)).status === 200,
      '回滚后节点必须仍在');

    // 再暂存 + 提交：原子生效
    const txStaged2 = await page.evaluate(async (a) => {
      const idx = gridDataRows.findIndex((r) => r._ && String(r._.id) === String(a));
      const node = gridDataRows[idx] && gridDataRows[idx]._;
      if (!node) return { ok: false };
      await deleteGridNode(node);
      return { ok: true, ops: txOperations.length };
    }, idA);
    expect(txStaged2.ops === 1, '再次暂存应成功');
    await page.click('#txCommitBtn');
    await sleep(1500);
    const afterCommit = await page.evaluate(() => ({
      ops: txOperations.length,
      toasts: [...document.querySelectorAll('#toastStack .toast')].map((x) => x.textContent),
    }));
    expect(afterCommit.ops === 0, '提交后暂存区应清空');
    expect(afterCommit.toasts.some((x) => x.includes('提交') || x.includes('Committed')),
      `应提示提交成功：${JSON.stringify(afterCommit.toasts)}`);
    expect((await fetch(`http://127.0.0.1:${port}/v1/nodes/${idA}`)).status === 404,
      '提交后节点应被删除');

    // TQL 写语句：有未提交的界面操作时必须被拦下（不发请求）
    const blockedWrites = [];
    const observeBlockedWrite = request => {
      if (request.method() === 'POST' && request.url().endsWith('/v1/tql')
          && request.postDataJSON()?.query === 'CREATE (n:person {name: "tx-blocked"})') blockedWrites.push(request.url());
    };
    page.on('request', observeBlockedWrite);
    const blocked = await page.evaluate(async () => {
      const fake = { type: 'node', id: '999777', payload: { name: 'tx-blocked-seed' }, vector: [], edges: [] };
      renderGrid([{ _: fake }]);
      await deleteGridNode(fake);
      const opsBefore = txOperations.length;
      const mut = document.getElementById('mutationCheck');
      mut.checked = true;
      mut.dispatchEvent(new Event('change', { bubbles: true }));
      setEditorValue('CREATE (n:person {name: "tx-blocked"})');
      await executeCurrentQuery();
      return { opsBefore, toast: [...document.querySelectorAll('#toastStack .toast')].map((x) => x.textContent).slice(-1)[0] || '' };
    });
    page.off('request', observeBlockedWrite);
    expect(blocked.opsBefore >= 1, `前置条件：应先有一个暂存的界面操作 ${JSON.stringify(blocked)}`);
    expect(blockedWrites.length === 0, `事务模式下 TQL 写语句不应发请求：${JSON.stringify(blockedWrites)}`);
    expect(/事务|审阅|transaction|写入/.test(blocked.toast), `应给出明确提示：${JSON.stringify(blocked)}`);
    await page.evaluate(() => { txOperations = []; syncTxBar(); renderGrid([]); });

    // 关闭标签 2 回到一个标签
    const beforeClose = await page.evaluate(() => document.querySelectorAll('#queryTabsList .query-tab').length);
    await page.click(`[data-query-tab-close="${secondTabId}"]`);
    await sleep(300);
    const afterClose = await page.evaluate(() => document.querySelectorAll('#queryTabsList .query-tab').length);
    expect(afterClose === beforeClose - 1, `关闭标签应减少一个：${beforeClose} → ${afterClose}`);

    report.parts.tabsTx = { tab1, tab2, back1, back2, txStaged, afterRollback, txStaged2, afterCommit, blocked, beforeClose, afterClose };
    expect(errors.length === 0, `标签/事务 console/pageerror: ${errors.join('; ')}`);
    await context.close();
  }
  // G20. 侧栏对象树排版 + 查询历史弹层（真实点击、搜索、填入、删除、清空）。
  report.parts.sidebarHistory = {};
  {
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
    await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true);
    // 造两条真实历史
    for (const q of ['FIND {type: "person"} RETURN * LIMIT 3', 'MATCH (a)-[]->(b) RETURN a, b LIMIT 3']) {
      await page.evaluate((query) => {
        document.querySelector('[data-tab="queryPane"]').click();
        setEditorValue(query);
        executeCurrentQuery();
      }, q);
      await page.waitForFunction(() => document.getElementById('resultsContainer').dataset.resultState !== 'running',
        null, { timeout: 15000 }).catch(() => {});
      await sleep(700);
    }
    await page.waitForFunction(() => document.querySelectorAll('#objectTree .ot-group').length === 4, null, { timeout: 15000 });

    // 对象树排版
    const tree = await page.evaluate(() => {
      const groups = [...document.querySelectorAll('#objectTree .ot-group')];
      return {
        open: groups.map((g) => g.open),
        hasEmptyText: !!groups[2].querySelector('.ot-empty'),
        fakeRow: !!groups[2].querySelector('[data-ot-kind="index-empty"]'),
        importClass: document.getElementById('openImportBtn')?.className || '',
        chip: !!groups[3].querySelector('.ot-chip'),
        badge: document.getElementById('historyCountBadge')?.textContent || '',
      };
    });
    expect(tree.open[0] === true && tree.open.slice(1).every((v) => v === false),
      `类型展开、其余收起：${JSON.stringify(tree.open)}`);
    expect(tree.hasEmptyText && !tree.fakeRow, `空组应是哑文字而不是假行：${JSON.stringify(tree)}`);
    expect(/ot-row/.test(tree.importClass) && /ot-import/.test(tree.importClass),
      `导入入口应是树内行样式：${tree.importClass}`);
    expect(tree.chip === true, '向量行应有胶囊');
    expect(Number(tree.badge) >= 2, `历史徽标应不少于 2：${tree.badge}`);

    // 打开历史弹层（V4.1 R2：历史入口在侧栏「历史」段，先切段）
    await page.click('.side-tab[data-side-tab="history"]');
    await sleep(200);
    await page.click('#historyNavItem');
    await page.waitForFunction(() => !document.getElementById('historyOverlay').hidden, null, { timeout: 5000 });
    await sleep(300);
    const panel = await page.evaluate(() => ({
      items: document.querySelectorAll('#historyList .history-item').length,
      firstQuery: document.querySelector('#historyList .history-query')?.textContent || '',
      firstMeta: document.querySelector('#historyList .history-meta')?.textContent || '',
      // 分组视图后 .history-item 位于 .history-group 内，:first-child 不再成立；
      // 取「第一条历史」的行内操作，语义不变（不依赖兄弟节点位置）
      actions: [...(document.querySelector('#historyList .history-item .history-actions')?.querySelectorAll('button') || [])]
        .map((b) => b.dataset.historyAct),
    }));
    expect(panel.items >= 2, `历史面板应有 ≥2 条：${JSON.stringify(panel)}`);
    expect(/ms/.test(panel.firstMeta) && /\d/.test(panel.firstMeta), `元信息应含耗时：${panel.firstMeta}`);
    expect(panel.actions.join(',') === 'fill,run,copy,remove', `条目应有四个操作：${panel.actions}`);

    // 搜索
    await page.fill('#historySearch', 'MATCH');
    await sleep(300);
    const filtered = await page.evaluate(() => document.querySelectorAll('#historyList .history-item').length);
    expect(filtered === 1, `搜索 MATCH 应只剩 1 条：${filtered}`);
    await page.fill('#historySearch', '');
    await sleep(200);

    // 填入
    await page.click('#historyList .history-item .history-actions button[data-history-act="fill"]');
    await sleep(350);
    const afterFill = await page.evaluate(() => ({
      editor: document.getElementById('tqlInput').value,
      closed: document.getElementById('historyOverlay').hidden,
    }));
    expect(afterFill.closed === true && afterFill.editor.length > 0,
      `填入应写回编辑器并关闭面板：${JSON.stringify(afterFill)}`);

    // 删除一条
    await page.click('#historyNavItem');
    await page.waitForFunction(() => !document.getElementById('historyOverlay').hidden, null, { timeout: 5000 });
    await sleep(250);
    const beforeRemove = await page.evaluate(() => document.querySelectorAll('#historyList .history-item').length);
    // 同上：分组后首条历史不再是 :first-child，直接按行取删除按钮
    await page.evaluate(() => {
      document.querySelector('#historyList .history-item .history-actions button[data-history-act="remove"]')?.click();
    });
    await sleep(300);
    const afterRemove = await page.evaluate(() => ({
      items: document.querySelectorAll('#historyList .history-item').length,
      badge: document.getElementById('historyCountBadge').textContent,
    }));
    expect(afterRemove.items === beforeRemove - 1, `删除应减少一条：${beforeRemove} → ${afterRemove.items}`);
    expect(Number(afterRemove.badge) === afterRemove.items, `徽标应与条数一致：${JSON.stringify(afterRemove)}`);

    // 清空
    await page.click('#historyClearBtn');
    await sleep(300);
    const cleared = await page.evaluate(() => ({
      empty: !!document.querySelector('#historyList .history-empty'),
      badge: document.getElementById('historyCountBadge').textContent,
    }));
    expect(cleared.empty === true && cleared.badge === '0', `清空后应显示空态：${JSON.stringify(cleared)}`);
    await page.keyboard.press('Escape');
    await sleep(200);
    expect(await page.evaluate(() => document.getElementById('historyOverlay').hidden), 'Esc 应关闭历史面板');

    // 导入入口仍可用
    await page.click('#openImportBtn');
    await sleep(300);
    expect(await page.evaluate(() => !document.getElementById('importOverlay').hidden), '树内导入行仍应打开导入向导');
    await page.keyboard.press('Escape');

    report.parts.sidebarHistory = { tree, panel, filtered, afterFill, beforeRemove, afterRemove, cleared };
    expect(errors.length === 0, `侧栏/历史 console/pageerror: ${errors.join('; ')}`);
    await context.close();
  }

  }
  // G21. 写入与标签生命周期边界：真实服务核对记录；屏障控制响应交错，不使用固定等待证明完成。
  report.parts.writeBoundaries = {};
  {
    const vectorJson = JSON.stringify(VEC);
    const seeded = await request(port, 'POST', '/v1/import/nodes', [
      `{"id":9007199254740992,"vector":${vectorJson},"payload":{"name":"相邻记录"}}`,
      `{"id":9007199254740993,"vector":${vectorJson},"payload":{"name":"选中记录"}}`,
    ].join('\n'), { 'Content-Type': 'application/x-ndjson' });
    expect(seeded.status === 200, `大 ID 夹具必须创建成功：${seeded.text}`);
    const before = [];
    for (const id of ['9007199254740992', '9007199254740993']) {
      const response = await request(port, 'GET', `/v1/nodes/${id}`);
      if (response.status !== 200 || !response.json?.node) throw new Error(`缺少大 ID 夹具：${id}`);
      before.push(response.json.node);
    }
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
    const writes = [];
    page.on('request', r => { if (r.method() === 'POST' && /\/v1\/(transactions|nodes\/delete-many|import\/)/.test(r.url())) writes.push(r.url()); });
    const rejected = await page.evaluate(async ({ node, vector }) => {
      document.querySelector('[data-tab="queryPane"]').click();
      const check = document.getElementById('txModeCheck');
      const snapshots = [];
      for (const staged of [false, true]) {
        check.checked = staged;
        txOperations = [];
        renderGrid([{ a: node }]);
        await deleteGridNode(node);
        gridSelection = new Set(['1', node.id]);
        await deleteSelectedGridNodes();
        openPayloadEditor(node);
        document.getElementById('payloadEditorText').value = '{"name":"不得写入"}';
        await savePayloadEditor();
        snapshots.push({ staged, pending: txOperations.length, editorOpen: !document.getElementById('payloadEditor').hidden, error: document.getElementById('payloadEditorStatus').textContent });
        closePayloadEditor();
      }
      check.checked = false;
      const textId = '9007199254740993';
      const invalid = [
        buildNodeImport(`id,vector\n${textId},"[1,0]"`, 'csv'),
        buildNodeImport(JSON.stringify({ id: textId, vector: [1, 0], payload: {} }), 'jsonl'),
        buildNodeImport(`{"id":9007199254740993,"vector":[1,0],"payload":{}}`, 'jsonl'),
        buildEdgeImport(`source,target,label\n1,${textId},关系`, 'csv'),
        buildEdgeImport(JSON.stringify({ source: textId, target: 1, label: '关系' }), 'jsonl'),
      ];
      return { snapshots, invalid: invalid.map(x => ({ records: x.records.length, errors: x.errors.length })), safeMax: parseWriteNodeId('9007199254740991') };
    }, { node: before[1], vector: VEC });
    expect(writes.length === 0, `不安全 ID 应在请求前拒绝：${writes.join(',')}`);
    expect(rejected.safeMax === 9007199254740991, '最大安全 ID 必须保持原值');
    for (const snapshot of rejected.snapshots) {
      expect(snapshot.pending === 0 && snapshot.editorOpen && /安全整数|safe integers/.test(snapshot.error), `单删/批删/编辑拒绝后不得暂存或关闭编辑器：${JSON.stringify(snapshot)}`);
    }
    expect(rejected.invalid.every(x => x.records === 0 && x.errors > 0), `CSV/JSONL 的节点和关系 ID 必须校验：${JSON.stringify(rejected.invalid)}`);
    for (let i = 0; i < before.length; i++) {
      const response = await request(port, 'GET', `/v1/nodes/${before[i].id}`);
      expect(response.status === 200 && JSON.stringify(response.json.node) === JSON.stringify(before[i]), `拒绝后完整节点必须保持：${before[i].id}`);
    }
    report.parts.writeBoundaries.ids = { rejected, writes };
    expect(errors.length === 0, `大 ID console/pageerror: ${errors.join('; ')}`);
    await context.close();
  }
  for (const failFirst of [false, true]) {
    const ids = failFirst ? [970013, 970014] : [970011, 970012];
    const seeded = await request(port, 'POST', '/v1/import/nodes', ids.map(id => JSON.stringify({ id, vector: VEC, payload: { name: '原值' } })).join('\n'), { 'Content-Type': 'application/x-ndjson' });
    if (seeded.status !== 200) throw new Error('提交交错夹具创建失败');
    const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
    const nodes = [];
    for (const id of ids) {
      const response = await request(port, 'GET', `/v1/nodes/${id}`);
      if (response.status !== 200 || !response.json?.node) throw new Error(`提交夹具缺失：${id}`);
      nodes.push(response.json.node);
    }
    // 外部 HTTP 边界只控制第一次响应：成功时发送真实事务，失败时模拟 500 且不转发。
    await page.evaluate(fail => {
      const original = window.fetch;
      let first = true;
      window.__boundaryHeld = false;
      window.fetch = async (url, init) => {
        if (String(url).endsWith('/v1/transactions') && first) {
          first = false;
          window.__boundaryWire = JSON.parse(init.body);
          const response = fail ? new Response(JSON.stringify({ detail: '受控提交失败' }), { status: 500, headers: { 'Content-Type': 'application/json' } }) : await original(url, init);
          window.__boundaryHeld = true;
          await new Promise(resolve => { window.__boundaryRelease = resolve; });
          return response;
        }
        return original(url, init);
      };
      document.getElementById('txModeCheck').checked = true;
    }, failFirst);
    for (let i = 0; i < nodes.length; i++) {
      await page.evaluate(node => openPayloadEditor(node), nodes[i]);
      await page.fill('#payloadEditorText', JSON.stringify({ name: i === 0 ? '第一笔' : '第二笔' }));
      await page.click('#payloadEditorSave');
      if (i === 0) {
        await page.click('#txCommitBtn');
        await page.waitForFunction(() => window.__boundaryHeld === true);
      }
    }
    const during = await page.evaluate(() => ({ pending: txOperations.length, wire: window.__boundaryWire }));
    expect(during.pending === 2 && during.wire.operations.length === 1 && during.wire.operations[0].id === ids[0], `快照只含第一笔，新操作仍应排队：${JSON.stringify(during)}`);
    await page.evaluate(() => window.__boundaryRelease());
    await page.waitForFunction(() => txCommitting === false);
    const after = await page.evaluate(() => txOperations.map(op => ({ id: op.id, payload: op.payload })));
    expect(after.length === (failFirst ? 2 : 1) && after.at(-1).id === ids[1] && after.at(-1).payload.name === '第二笔', `提交响应不得丢失新暂存项：${JSON.stringify(after)}`);
    const secondBeforeRetry = await request(port, 'GET', `/v1/nodes/${ids[1]}`);
    expect(secondBeforeRetry.json.node.payload.name === '原值', '尚未发送的第二笔不能提前落库');
    await page.click('#txCommitBtn');
    await page.waitForFunction(() => !txCommitting && txOperations.length === 0);
    for (let i = 0; i < ids.length; i++) {
      const response = await request(port, 'GET', `/v1/nodes/${ids[i]}`);
      expect(response.status === 200 && response.json.node.payload.name === (i === 0 ? '第一笔' : '第二笔') && response.json.node.id === String(ids[i]), `再次提交后事实必须完整：${ids[i]}`);
    }
    report.parts.writeBoundaries[failFirst ? 'failedCommit' : 'successfulCommit'] = { during, after };
    expect(errors.length === 0, `提交交错 console/pageerror: ${errors.join('; ')}`);
    await context.close();
  }
  // 响应已从真实服务取得，再由受控屏障延迟交付；即使取消未阻止响应返回，也必须拒绝迟到结果。
  report.parts.tabOwnership = [];
  for (const format of ['json', 'ndjson']) {
    for (const action of ['switch', 'close', 'add']) {
      const { context, page, errors } = await newPage('light', { width: 1440, height: 900 });
      const before = await page.evaluate(({ format }) => {
        const origin = activeQueryTabId;
        const other = addQueryTab('FIND {type: "另一个标签"} RETURN *', false);
        const original = window.fetch;
        const query = 'MATCH (a) RETURN a LIMIT 1';
        window.__boundaryHeld = false;
        window.fetch = async (url, init) => {
          if (String(url).endsWith('/v1/tql') && JSON.parse(init.body).query === query) {
            const headers = { ...init.headers, Accept: format === 'json' ? 'application/json' : 'application/x-ndjson' };
            const response = await original(url, { ...init, headers });
            window.__boundaryContentType = response.headers.get('Content-Type');
            window.__boundaryHeld = true;
            await new Promise(resolve => { window.__boundaryRelease = resolve; });
            return response;
          }
          return original(url, init);
        };
        setEditorValue(query);
        document.getElementById('tqlInput').setSelectionRange(query.length, query.length);
        window.__boundaryRun = executeCurrentQuery();
        return { origin, other: other.id };
      }, { format });
      await page.waitForFunction(() => window.__boundaryHeld === true);
      const activated = await page.evaluate(({ action, origin, other }) => {
        let active = other;
        if (action === 'close') closeQueryTab(origin);
        else if (action === 'add') active = addQueryTab('FIND {type: \"另一个标签\"} RETURN *', true).id;
        else switchQueryTab(other);
        window.__boundaryRelease();
        return active;
      }, { action, ...before });
      await page.evaluate(() => window.__boundaryRun);
      const after = await page.evaluate(() => ({ active: activeQueryTabId, editor: document.getElementById('tqlInput').value, state: document.getElementById('resultsContainer').dataset.resultState, busy: document.getElementById('runQueryBtn').disabled, tabs: queryTabList.map(tab => ({ id: tab.id, query: tab.query, hasResult: Boolean(tab.result) })), contentType: window.__boundaryContentType }));
      expect(after.contentType.includes(format === 'json' ? 'application/json' : 'application/x-ndjson'), `必须实际执行 ${format} 响应分支：${after.contentType}`);
      expect(after.active === activated && after.editor === 'FIND {type: "另一个标签"} RETURN *' && after.state === 'empty' && !after.busy, `${format}/${action} 不得污染活动标签或遗留运行态：${JSON.stringify(after)}`);
      expect(after.tabs.every(tab => !tab.hasResult && tab.query === ''), `${format}/${action} 迟到结果不得保存到其他标签：${JSON.stringify(after.tabs)}`);
      expect(after.tabs.length === (action === 'close' ? 1 : action === 'add' ? 3 : 2), `${format}/${action} 标签集合不正确`);
      report.parts.tabOwnership.push({ format, action, after });
      expect(errors.length === 0, `${format}/${action} console/pageerror: ${errors.join('; ')}`);
      await context.close();
    }
  }

} catch (e) {
  failures.push(`fatal: ${e.message}`);
  console.error(e);
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
log(`${checks} checks, ${failures.length} failure(s), artifacts ${OUT}`);
if (failures.length) {
  for (const f of failures) console.error(`  FAIL ${f}`);
  process.exitCode = 1;
} else {
  log('PASS');
}
