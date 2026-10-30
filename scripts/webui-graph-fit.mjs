#!/usr/bin/env node
// WebGL（真实 Sigma，CDN）取景回归：适合窗口后节点/标签不得被 HUD、查询舱、
// 布局条、结果抽屉、统一面板或右侧 rail 遮挡；也不得越出画布。
// 依赖真实 CDN：Sigma 加载失败直接失败（exit 1），不回退 Canvas 冒充通过。
// 用法：node scripts/webui-graph-fit.mjs   （WEBUI_BROWSER=chromium|webkit）
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const RUN_ID = `${process.pid}-${Date.now()}`;
const OUT = path.join(ROOT, '.tmp', 'graph-fit', RUN_ID);
const DB_DIR = path.join(ROOT, '.tmp', 'graph-fit', `db-${RUN_ID}`);
const BUILD_TARGET = path.resolve(process.env.CARGO_TARGET_DIR || path.join(ROOT, 'target', 'webui-tests'));
const EXE = path.join(BUILD_TARGET, 'debug', process.platform === 'win32' ? 'triviumdb-server.exe' : 'triviumdb-server');
const BROWSER = process.env.WEBUI_BROWSER || 'chromium';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const failures = [];
let checks = 0;
const expect = (cond, msg) => { checks += 1; if (!cond) failures.push(msg); };
const log = (m) => console.log(`[graph-fit] ${m}`);

mkdirSync(OUT, { recursive: true });
mkdirSync(DB_DIR, { recursive: true });

function resolveCargo() {
  const exeName = process.platform === 'win32' ? 'cargo.exe' : 'cargo';
  const probe = (cmd) => spawnSync(cmd, ['--version'], { encoding: 'utf8', shell: false });
  if (probe(exeName).status === 0) return exeName;
  const full = path.join(os.homedir(), '.cargo', 'bin', exeName);
  return existsSync(full) && probe(full).status === 0 ? full : null;
}

function findPort(from = 8150, to = 8185) {
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
    const { createRequire } = await import('node:module');
    for (const base of [
      path.join(probe.stdout.trim(), 'noop.js'),
      path.join(probe.stdout.trim(), '@playwright', 'cli', 'node_modules', 'noop.js'),
    ]) {
      try {
        const pc = createRequire(base)('playwright');
        if (pc && pc.chromium) return pc;
      } catch (_) { /* next */ }
    }
  }
  return null;
}

const cargo = resolveCargo();
if (!cargo) { console.error('[graph-fit] cargo not found'); process.exit(1); }
log(`building server (${BUILD_TARGET})`);
const build = spawnSync(cargo, ['build', '-p', 'triviumdb-server', '--target-dir', BUILD_TARGET], {
  cwd: ROOT, stdio: 'inherit', encoding: 'utf8',
});
if (build.status !== 0) { console.error('[graph-fit] cargo build failed'); process.exit(build.status || 1); }

const pw = await loadPlaywright();
if (!pw) { console.error('[graph-fit] playwright not available'); process.exit(1); }
const browserType = pw[BROWSER];
if (!browserType) { console.error(`[graph-fit] unknown WEBUI_BROWSER=${BROWSER}`); process.exit(1); }

const port = await findPort();
if (!port) { console.error('[graph-fit] no free port in 8150-8185'); process.exit(1); }
const server = spawn(EXE, ['--dim', '768', '--database', path.join(DB_DIR, 'data.tdb'), '--listen', `127.0.0.1:${port}`], {
  cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true,
});
let serverLog = '';
server.stdout.on('data', (d) => { serverLog += d.toString(); });
server.stderr.on('data', (d) => { serverLog += d.toString(); });

async function waitReady() {
  for (let i = 0; i < 120; i += 1) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}/health/ready`);
      if (r.ok) return;
    } catch (_) { /* retry */ }
    await sleep(250);
  }
  throw new Error('server not ready');
}

const SEED = [
  'CREATE ({name: "Alice", type: "person", citations: 150})-[:AUTHORED]->({title: "TriviumDB Architecture", type: "paper"})-[:COVERS]->({name: "Tri-Model DB", type: "topic"})',
  'CREATE ({name: "Bob", type: "person", citations: 45})-[:AUTHORED]->({title: "QuIVer Vector Engine", type: "paper"})-[:COVERS]->({name: "Tri-Model DB", type: "topic"})',
  'CREATE ({name: "Carol", type: "person", citations: 210})',
];

// 页面内取景几何：用 Sigma 自己的 graphToViewport + cameraSizeRatio 换算真实屏幕尺寸
function fitProbe() {
  const canvas = document.getElementById('standaloneGraphCanvas');
  const rect = canvas.getBoundingClientRect();
  const inst = sigmaInstances.standalone;
  if (!inst || !inst.sigma || inst.sigma.killed) return { error: 'sigma-instance-missing' };
  const cam = inst.sigma.getCamera().getState();
  const sizeRatio = Math.sqrt(cam.ratio || 1);
  const labelSize = Number(inst.sigma.getSetting && inst.sigma.getSetting('labelSize')) || 12;
  const overlays = [...document.querySelectorAll('#standaloneGraphShell .v2-hud, #standaloneGraphShell .v2-cockpit, #standaloneGraphShell .graph-float-toolbar, #standaloneGraphShell .v2-graph-controls, #standaloneGraphShell .v2-dock, #unifiedSidePanel, .usp-rail')]
    .map((el) => {
      const b = el.getBoundingClientRect();
      const cs = getComputedStyle(el);
      return {
        id: el.id || el.className,
        l: b.left - rect.left, t: b.top - rect.top, r: b.right - rect.left, b: b.bottom - rect.top,
        w: b.width, h: b.height, visible: cs.visibility !== 'hidden' && cs.opacity !== '0',
      };
    })
    .filter((o) => o.visible && o.w > 1 && o.h > 1 && o.r > 0 && o.b > 0 && o.l < rect.width && o.t < rect.height);
  const hit = (a, b) => a.l < b.r && a.r > b.l && a.t < b.b && a.b > b.t;
  const nodes = graphNodes.filter(isNodeVisible).map((n) => {
    const v = inst.sigma.graphToViewport(inst.mapFn(n));
    const r = nodeRadius(n) / sizeRatio;
    const half = Math.min(140, String(n.label || '').length * 0.31 * labelSize);
    return {
      label: n.label, x: v.x, y: v.y, r,
      box: { l: v.x - r, r: v.x + r, t: v.y - r, b: v.y + r },
      labelBox: { l: v.x + r + 2, r: v.x + r + 2 + half, t: v.y - labelSize / 2, b: v.y + labelSize / 2 },
    };
  });
  const collisions = [];
  for (const n of nodes) {
    for (const o of overlays) {
      if (hit(n.box, o)) collisions.push(['node', n.label, o.id]);
      if (hit(n.labelBox, o)) collisions.push(['label', n.label, o.id]);
    }
  }
  return {
    canvas: [rect.width, rect.height],
    backend: graphBackend,
    nodeCount: nodes.length,
    camera: { x: cam.x, y: cam.y, ratio: cam.ratio },
    overlays: overlays.map((o) => ({ id: o.id, l: Math.round(o.l), t: Math.round(o.t), r: Math.round(o.r), b: Math.round(o.b) })),
    collisions,
    outOfCanvas: nodes.filter((n) => n.box.l < -1 || n.box.r > rect.width + 1 || n.box.t < -1 || n.box.b > rect.height + 1).map((n) => n.label),
  };
}

const browser = await browserType.launch({ headless: true });
const report = { browser: BROWSER, port, checks: 0, failures, cases: [], sigma: 'unknown' };
const CONFIGS = [
  { width: 1440, height: 900, dpr: 1, panel: false, dock: false, resize: true },
  { width: 1440, height: 900, dpr: 2, panel: false, dock: false, resize: false },
  { width: 1440, height: 900, dpr: 1, panel: true, dock: false, resize: false },
  { width: 1440, height: 900, dpr: 1, panel: false, dock: true, resize: false },
  { width: 1024, height: 768, dpr: 1, panel: false, dock: false, resize: false },
  { width: 390, height: 844, dpr: 1, panel: false, dock: false, resize: false },
];

try {
  await waitReady();
  for (const statement of SEED) {
    const res = await fetch(`http://127.0.0.1:${port}/v1/tql`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ query: statement, mutation: true }),
    });
    if (!res.ok) throw new Error(`seed failed: ${res.status} ${await res.text()}`);
  }

  for (const cfg of CONFIGS) {
    const label = `${BROWSER}/${cfg.width}x${cfg.height}/DPR${cfg.dpr}/panel=${cfg.panel}/dock=${cfg.dock}`;
    const context = await browser.newContext({
      viewport: { width: cfg.width, height: cfg.height }, deviceScaleFactor: cfg.dpr, reducedMotion: 'reduce',
    });
    const page = await context.newPage();
    const errors = [];
    const benign = [];
    page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
    page.on('console', (m) => {
      if (m.type() !== 'error') return;
      const text = m.text();
      // WebKit 画布 ResizeObserver 噪声：单独记录，不当作通过依据
      if (/ResizeObserver loop completed with undelivered notifications/.test(text)) {
        benign.push(text);
        return;
      }
      // WebKit 多次重建 Sigma 实例会触发 16 个 WebGL context 上限告警（Chromium 干净）；
      // 只对 WebKit 精确匹配这两条，单独记录，不宣称 console 全零。
      if (BROWSER === 'webkit'
        && /^(There are too many active WebGL contexts on this page, the oldest context will be lost\.|WebGL: INVALID_OPERATION: loseContext: context already lost)$/.test(text)) {
        benign.push(text);
        return;
      }
      errors.push(`console: ${text}`);
    });
    try {
      await page.goto(`http://127.0.0.1:${port}/ui`, { waitUntil: 'domcontentloaded' });
      await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true, null, { timeout: 30000 });
      await page.click('[data-tab="graphExplorePane"]');
      await page.click('#loadWholeGraphBtn');
      await page.waitForFunction(
        () => document.getElementById('graphTqlStatus')?.textContent.includes('结果'),
        null, { timeout: 15000 },
      );
      await sleep(1200);

      // 真实 Sigma（CDN）；失败时不回退 Canvas 冒充通过
      await page.click('#toggleWebGLBtnStandalone');
      const loaded = await page.waitForFunction(
        () => graphBackend === 'sigma' && !!sigmaInstances.standalone && !!sigmaInstances.standalone.sigma,
        null, { timeout: 45000 },
      ).then(() => true).catch(() => false);
      if (!loaded) {
        const toast = await page.evaluate(() => [...document.querySelectorAll('#toastStack .toast')].map((x) => x.textContent).join(' | '));
        report.sigma = 'unavailable';
        failures.push(`${label}: SIGMA_UNAVAILABLE（真实 CDN 依赖不可用，不以 Canvas 冒充）toast=${toast}`);
        report.cases.push({ label, sigmaUnavailable: true, toast, errors, benign: [...new Set(benign)] });
        await context.close();
        continue;
      }
      report.sigma = 'ready';

      const fitted = async (tag) => {
        await page.click('#gftFitBtnStandalone');
        await sleep(800);
        const probe = await page.evaluate(fitProbe);
        expect(probe.backend === 'sigma', `${label}/${tag}: 必须仍在 WebGL 后端（实际 ${probe.backend}）`);
        expect(probe.nodeCount >= 5, `${label}/${tag}: 图内应有真实节点（实际 ${probe.nodeCount}）`);
        expect(probe.collisions.length === 0,
          `${label}/${tag}: 节点/标签被浮层遮挡 ${JSON.stringify((probe.collisions || []).slice(0, 6))}`);
        expect((probe.outOfCanvas || []).length === 0,
          `${label}/${tag}: 节点越出画布 ${JSON.stringify((probe.outOfCanvas || []).slice(0, 6))}`);
        return probe;
      };

      await page.evaluate(() => setGraphLayoutMode('radial'));
      await sleep(700);
      if (cfg.panel) { await page.click('#uspRailDetailBtn'); await sleep(500); }
      if (cfg.dock) { await page.click('#graphDockToggle'); await sleep(500); }
      const afterFit = await fitted('fit');

      // 只在首个配置做一次：WebGL 节点拖拽 + 坐标系朝向（Canvas 与 WebGL 不能上下镜像）
      if (cfg.resize) {
        const axis = await page.evaluate(() => {
          const inst = sigmaInstances.standalone;
          const pts = graphNodes.filter(isNodeVisible).map((n) => {
            const v = inst.sigma.graphToViewport(inst.mapFn(n));
            return { wy: n.y, sy: v.y };
          });
          let inversions = 0;
          for (let i = 0; i < pts.length; i += 1) {
            for (let j = i + 1; j < pts.length; j += 1) {
              if (Math.abs(pts[i].wy - pts[j].wy) > 5
                && (pts[i].wy - pts[j].wy) * (pts[i].sy - pts[j].sy) < 0) inversions += 1;
            }
          }
          return { nodes: pts.length, inversions };
        });
        expect(axis.nodes >= 5 && axis.inversions === 0,
          `WebGL 与 Canvas 的 y 轴方向必须一致（世界 y 越大屏幕 y 越大），实测反序对 ${axis.inversions}/${axis.nodes}`);
        report.axis = axis;

        const dragBefore = await page.evaluate(() => {
          const inst = sigmaInstances.standalone;
          const n = graphNodes.find((x) => isNodeVisible(x));
          const rect = document.getElementById('standaloneSigmaContainer').getBoundingClientRect();
          const v = inst.sigma.graphToViewport(inst.mapFn(n));
          return { id: String(n.id), x: n.x, y: n.y, sx: rect.left + v.x, sy: rect.top + v.y };
        });
        await page.mouse.move(dragBefore.sx, dragBefore.sy);
        await page.mouse.down();
        for (let i = 1; i <= 8; i += 1) {
          await page.mouse.move(dragBefore.sx + i * 6, dragBefore.sy - i * 4);
        }
        const dragMid = await page.evaluate(() => document.body.classList.contains('panel-open'));
        await page.mouse.up();
        await sleep(350);
        const dragAfter = await page.evaluate((id) => {
          const n = graphNodes.find((x) => String(x.id) === id);
          return { x: n.x, y: n.y, panel: document.body.classList.contains('panel-open') };
        }, dragBefore.id);
        expect(dragMid === false, 'WebGL 拖拽途中不应弹出详情面板');
        expect(dragAfter.panel === false, 'WebGL 拖拽结束不应被当作点击');
        expect(dragAfter.x - dragBefore.x > 10 && dragBefore.y - dragAfter.y > 5,
          `WebGL 拖拽应写回共享坐标且方向与指针一致：dx=${(dragAfter.x - dragBefore.x).toFixed(1)} dyUp=${(dragBefore.y - dragAfter.y).toFixed(1)}`);
        report.drag = { dragBefore, dragAfter };
        // 拖拽后切 Canvas 再切回 WebGL：位置不回弹
        const pinned = await page.evaluate((id) => {
          const n = graphNodes.find((x) => String(x.id) === id);
          return { x: n.x, y: n.y };
        }, dragBefore.id);
        await page.click('#toggleWebGLBtnStandalone');
        await page.waitForFunction(() => graphBackend === 'canvas', null, { timeout: 10000 });
        await sleep(300);
        await page.click('#toggleWebGLBtnStandalone');
        await page.waitForFunction(
          () => graphBackend === 'sigma' && !!sigmaInstances.standalone && !sigmaInstances.standalone.sigma.killed,
          null, { timeout: 45000 },
        );
        await sleep(500);
        const kept = await page.evaluate((id) => {
          const n = graphNodes.find((x) => String(x.id) === id);
          return { x: n.x, y: n.y };
        }, dragBefore.id);
        expect(Math.abs(kept.x - pinned.x) < 0.5 && Math.abs(kept.y - pinned.y) < 0.5,
          `后端往返后拖拽落点应保持：${JSON.stringify({ pinned, kept })}`);
      }

      // 后端切换：WebGL → Canvas → WebGL，再点适合窗口
      await page.click('#toggleWebGLBtnStandalone');
      await page.waitForFunction(() => graphBackend === 'canvas', null, { timeout: 10000 });
      await page.click('#toggleWebGLBtnStandalone');
      await page.waitForFunction(
        () => graphBackend === 'sigma' && !!sigmaInstances.standalone && !sigmaInstances.standalone.sigma.killed,
        null, { timeout: 45000 },
      );
      const afterSwitch = await fitted('backend-switch');

      // 窗口缩放：1440 → 1024 → 390 → 1440，每档重新 fit
      let afterResize = null;
      if (cfg.resize) {
        for (const size of [{ width: 1024, height: 768 }, { width: 390, height: 844 }, { width: 1440, height: 900 }]) {
          await page.setViewportSize(size);
          await sleep(700);
          afterResize = await fitted(`resize-${size.width}`);
        }
      }

      // 面板开→关：关闭后再 fit，可用区应回到无面板状态
      let afterPanelClose = null;
      if (cfg.panel) {
        await page.keyboard.press('Escape');
        await sleep(500);
        const closed = await page.evaluate(() => !document.body.classList.contains('panel-open'));
        expect(closed, `${label}: Esc 应关闭统一面板`);
        afterPanelClose = await fitted('panel-close');
      }

      expect(errors.length === 0, `${label}: console/pageerror ${errors.join('; ')}`);
      report.cases.push({ label, afterFit, afterSwitch, afterResize, afterPanelClose, errors, benign: [...new Set(benign)] });
      await page.screenshot({ path: path.join(OUT, `fit-${cfg.width}x${cfg.height}-dpr${cfg.dpr}-panel${cfg.panel}-dock${cfg.dock}.png`) });
    } finally {
      await context.close();
    }
  }
} catch (e) {
  failures.push(`fatal: ${e.message}`);
  console.error(e);
} finally {
  try { server.kill(); } catch (_) { /* ignore */ }
  await sleep(400);
  try { rmSync(DB_DIR, { recursive: true, force: true }); } catch (e) { report.cleanup = String(e.message); }
  try { await browser.close(); } catch (_) { /* ignore */ }
}

report.checks = checks;
report.failures = failures;
report.passed = failures.length === 0;
report.serverLogTail = serverLog.split('\n').slice(-20).join('\n');
writeFileSync(path.join(OUT, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
log(`${checks} checks, ${failures.length} failure(s), sigma=${report.sigma}, artifacts ${OUT}`);
if (failures.length) {
  for (const f of failures) console.error(`  FAIL ${f}`);
  process.exitCode = 1;
} else {
  log('PASS');
}
