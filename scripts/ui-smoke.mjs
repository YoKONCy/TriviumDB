#!/usr/bin/env node
// TriviumDB WebUI 冒烟脚本 (ui-smoke)
//
// 目的：为内置 Web Console (crates/triviumdb-server/web/index.html) 提供一条
// 可重复执行的端到端冒烟路径。依赖缺失必须返回失败，不能把未执行的检查算作通过。
//
// 流程：
//   1. cargo build -p triviumdb-server（HTML 经 include_str! 编译内嵌，必须先构建）
//   2. 启动临时 server（探测独立空闲端口，每轮数据库位于独占临时目录）
//   3. 通过 /v1/tql 注入少量种子数据（默认 FIND {type:"person"} 查询才有结果行）
//   4. Playwright 打开 /ui：运行默认查询出结果 → 打开节点抽屉 → 依次切三个标签
//   5. 打开 /ui?selftest=1：断言页内自检套件 0 failed
//   6. 清理：关闭浏览器、杀掉 server 进程、删除本轮独占临时目录中的数据库文件
//
// 用法：node scripts/ui-smoke.mjs
// 环境变量：SMOKE_BROWSER=chromium|firefox|webkit（默认 chromium）

import { spawn, spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { existsSync, mkdirSync, readdirSync, rmdirSync, rmSync } from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const TMP_DIR = path.join(ROOT, '.tmp');
// 每轮独占一个临时目录，不扫描或删除其他轮次的数据库。
const DB_DIR = path.join(TMP_DIR, `smoke-${process.pid}-${Date.now()}`);
const DB_PATH = path.join(DB_DIR, 'data.tdb');
const BROWSER = process.env.SMOKE_BROWSER || 'chromium';
const PORT_CANDIDATES = [8081, 8082, 8083, 8084, 8085];
// 独立构建目录不会替换其他开发服务正在使用的可执行文件。
const BUILD_TARGET = path.resolve(process.env.CARGO_TARGET_DIR || path.join(ROOT, 'target', 'webui-tests'));
const EXE = path.join(BUILD_TARGET, 'debug', process.platform === 'win32' ? 'triviumdb-server.exe' : 'triviumdb-server');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function log(msg) { console.log(`[ui-smoke] ${msg}`); }

// cargo 解析：优先 PATH，回退 ~/.cargo/bin（shell 环境未加载 rustup PATH 时仍可工作）
function resolveCargo() {
  const isWin = process.platform === 'win32';
  const exeName = isWin ? 'cargo.exe' : 'cargo';
  const probe = (cmd) => spawnSync(cmd, ['--version'], { encoding: 'utf8', shell: false });
  if (probe(exeName).status === 0) return exeName;
  const full = path.join(os.homedir(), '.cargo', 'bin', exeName);
  if (existsSync(full) && probe(full).status === 0) return full;
  return null;
}

// ---------- playwright 可用性探测（缺失则失败） ----------
async function loadPlaywright() {
  // 1) 项目本地 node_modules
  try {
    return await import('playwright');
  } catch (_) { /* fallthrough */ }
  // 2) 全局 npm root（npm i -g playwright）
  // 注意：Windows 下 npm 为 .cmd，Node 新版本要求 shell:true 才能启动（否则 EINVAL）
  const npmCmd = process.platform === 'win32' ? 'npm.cmd' : 'npm';
  const probe = spawnSync(npmCmd, ['root', '-g'], {
    encoding: 'utf8',
    shell: process.platform === 'win32',
  });
  if (probe.status === 0 && probe.stdout) {
    const globalRoot = probe.stdout.trim();
    const candidates = [
      path.join(globalRoot, 'noop.js'),
      // playwright-cli (@playwright/cli) 内嵌的 playwright 副本
      path.join(globalRoot, '@playwright', 'cli', 'node_modules', 'noop.js'),
    ];
    for (const base of candidates) {
      try {
        const requireFrom = createRequire(base);
        const pw = requireFrom('playwright');
        if (pw && pw.chromium) return pw;
      } catch (_) { /* try next */ }
    }
  }
  return null;
}

// ---------- 端口探测 ----------
function findFreePort(preferred) {
  return new Promise((resolve) => {
    const tryPort = (idx) => {
      if (idx >= PORT_CANDIDATES.length) { resolve(null); return; }
      const port = PORT_CANDIDATES[idx];
      const srv = net.createServer();
      srv.once('error', () => tryPort(idx + 1));
      srv.listen(port, '127.0.0.1', () => {
        srv.close(() => resolve(port));
      });
    };
    tryPort(preferred ? Math.max(0, PORT_CANDIDATES.indexOf(preferred)) : 0);
  });
}

// ---------- HTTP 助手 ----------
function request(method, urlPath, { body, headers } = {}) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      `http://127.0.0.1:${PORT}${urlPath}`,
      { method, headers: headers || {} },
      (res) => {
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () => {
          const text = Buffer.concat(chunks).toString('utf8');
          let json = null;
          try { json = JSON.parse(text); } catch (_) { /* keep null */ }
          resolve({ status: res.statusCode, json, text });
        });
      }
    );
    req.on('error', reject);
    if (body) req.write(body);
    req.end();
  });
}

async function waitForReady(timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await request('GET', '/health/ready');
      if (res.status === 200 && res.json && res.json.status) return true;
    } catch (_) { /* server not up yet */ }
    await sleep(300);
  }
  return false;
}

// ---------- 清理 ----------
let serverProc = null;
let browser = null;
let cleaned = false;

async function cleanup(exitCode) {
  if (cleaned) return;
  cleaned = true;
  try { if (browser) { await browser.close(); } } catch (_) {}
  if (serverProc && serverProc.exitCode === null) {
    try { serverProc.kill(); } catch (_) {}
    // Windows 下 kill() 是异步的：必须等进程真正退出、释放库文件句柄后才能删除临时库
    await new Promise((resolve) => {
      const timer = setTimeout(resolve, 5000);
      serverProc.once('exit', () => { clearTimeout(timer); resolve(); });
    });
  }
  // 临时库删除：进程退出后句柄可能仍被短暂占用（Windows 文件锁/杀软扫描），重试至多 15s；
  // 即使最终失败也不影响后续轮次（每轮库名唯一）
  let lastRmError = null;
  for (let attempt = 0; attempt < 30; attempt++) {
    try {
      // 只删除本轮独占目录中的直接子文件，覆盖 WAL、Payload 与所有 sidecar。
      if (existsSync(DB_DIR)) {
        for (const file of readdirSync(DB_DIR)) rmSync(path.join(DB_DIR, file), { force: true });
        rmdirSync(DB_DIR);
      }
      lastRmError = null;
      break;
    } catch (err) {
      lastRmError = err;
      await sleep(500);
    }
  }
  if (lastRmError) {
    console.error(`[ui-smoke] 警告：临时库删除失败 (${lastRmError.code || lastRmError.message})，本轮独占目录仍保留，需人工检查`);
  }
  process.exit(exitCode);
}

// ---------- 主流程 ----------
const playwright = await loadPlaywright();
if (!playwright) {
  log('playwright 不可用，无法执行冒烟测试。');
  log('如需启用：npm i -D playwright && npx playwright install chromium');
  process.exit(1);
}

log(`1/6 cargo build -p triviumdb-server (browser=${BROWSER})`);
const cargoCmd = resolveCargo();
if (!cargoCmd) {
  console.error('[ui-smoke] 未找到 cargo（PATH 与 ~/.cargo/bin 均无）。请先安装 Rust 工具链。');
  process.exit(1);
}
const build = spawnSync(cargoCmd, ['build', '-p', 'triviumdb-server', '--target-dir', BUILD_TARGET], {
  cwd: ROOT, stdio: 'inherit',
  shell: false,
});
if (build.status !== 0) {
  console.error('[ui-smoke] cargo build 失败');
  process.exit(1);
}
if (!existsSync(EXE)) {
  console.error(`[ui-smoke] 未找到 server 可执行文件: ${EXE}`);
  process.exit(1);
}

const PORT = await findFreePort();
if (!PORT) {
  console.error('[ui-smoke] 8081-8085 端口均被占用，无法启动临时 server');
  process.exit(1);
}
const BASE = `http://127.0.0.1:${PORT}`;

log(`2/6 启动临时 server: 127.0.0.1:${PORT}, db=${DB_PATH}`);
mkdirSync(TMP_DIR, { recursive: true });
mkdirSync(DB_DIR);
serverProc = spawn(EXE, ['--dim', '4', '--database', DB_PATH, '--listen', `127.0.0.1:${PORT}`], {
  cwd: ROOT,
  // stdout/stderr 均需持续消费：管道缓冲区 (64KB) 一旦写满，server 会阻塞在日志写入上
  // （规模注入会产生大量逐请求日志，不消费就会把整个 server 卡死）
  stdio: ['ignore', 'pipe', 'pipe'],
});
let serverStdout = '';
serverProc.stdout.on('data', (d) => {
  serverStdout += d.toString();
  if (serverStdout.length > 200000) serverStdout = serverStdout.slice(-100000);
});
let serverStderr = '';
serverProc.stderr.on('data', (d) => { serverStderr += d.toString(); });
serverProc.on('exit', (code) => {
  if (!cleaned && code !== 0 && code !== null) {
    console.error(`[ui-smoke] server 提前退出 (code=${code})\n${serverStderr.slice(-2000)}\n${serverStdout.slice(-2000)}`);
    cleanup(1);
  }
});
process.on('SIGINT', () => cleanup(1));
process.on('SIGTERM', () => cleanup(1));

log('3/6 等待 /health/ready 就绪');
if (!(await waitForReady(30000))) {
  console.error('[ui-smoke] server 30s 内未就绪\n' + serverStderr.slice(-2000));
  await cleanup(1);
}

log('3/6 注入种子数据 (2 个 person 节点 + 1 条 KNOWS 边)');
const seed = (query) => request('POST', '/v1/tql', {
  body: JSON.stringify({ query, mutation: true }),
  headers: { 'Content-Type': 'application/json' },
});
const seedRes = await seed('CREATE ({name: "Smoke Alice", type: "person", citations: 10})-[:KNOWS]->({name: "Smoke Bob", type: "person", citations: 20})');
if (seedRes.status !== 200) {
  console.error('[ui-smoke] 种子数据写入失败: ' + seedRes.text);
  await cleanup(1);
}
// 种子生效性检查：唯一全新库中恰好应有 2 个 person
const probeRows = await request('POST', '/v1/tql', {
  body: JSON.stringify({ query: 'FIND {type: "person"} RETURN * LIMIT 100' }),
  headers: { 'Content-Type': 'application/json' },
});
const seededCount = (probeRows.json && probeRows.json.rows ? probeRows.json.rows.length : 0);
if (seededCount !== 2) {
  console.error(`[ui-smoke] 种子数据校验失败（person=${seededCount}，应为 2）`);
  await cleanup(1);
}

let failures = 0;
let page = null;

try {
  log(`4/6 打开 ${BASE}/ui 执行冒烟路径`);
  browser = await playwright[BROWSER].launch();
  page = await browser.newPage();
  const tqlRequests = [];
  page.on('request', (req) => {
    if (req.method() === 'POST' && req.url().includes('/v1/tql')) tqlRequests.push(req.url());
  });
  page.on('pageerror', (err) => {
    console.error('[ui-smoke] 页面 JS 异常: ' + err.message);
  });
  page.on('console', (msg) => {
    if (msg.type() === 'error') console.error('[ui-smoke] 页面 console.error: ' + msg.text().slice(0, 300));
  });
  page.on('requestfailed', (r) => {
    console.error('[ui-smoke] 请求失败: ' + r.url() + ' — ' + (r.failure() && r.failure().errorText));
  });

  await page.goto(`${BASE}/ui`, { waitUntil: 'domcontentloaded', timeout: 20000 });
  // 等待应用初始化完成（DOMContentLoaded 监听器全部挂载后再交互，避免竞态）
  await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true, null, { timeout: 10000 });
  await page.waitForSelector('#runQueryBtn', { timeout: 10000 });

  // 冒烟 1：运行默认查询，表格应出现结果行
  await page.click('#runQueryBtn');
  await page.waitForFunction(
    () => document.querySelectorAll('#gridBody tr').length >= 1
      && document.getElementById('statusRowCount').textContent.includes('返回: 2 行'),
    null, { timeout: 15000 }
  );
  log('  ✓ 默认查询出结果 (2 行)');

  // 冒烟 2：点击节点行打开 360° 抽屉
  await page.click('#gridBody tr.row-clickable');
  await page.waitForSelector('#nodeDrawer.open', { timeout: 5000 });
  const drawerPayload = await page.textContent('#drawerNodePayload');
  if (!drawerPayload.includes('Smoke Alice')) throw new Error('抽屉 Payload 应包含 Smoke Alice');
  await page.click('#uspCollapseBtn');
  // 关闭后 .open class 被移除（元素仍在 DOM），用 hidden 状态等待而非 detached
  await page.waitForSelector('#nodeDrawer.open', { state: 'hidden', timeout: 5000 });
  log('  ✓ 节点 360° 抽屉打开/关闭');

  // 冒烟 2.5：混合搜索种子 —— 查询结果并入图探索
  await page.evaluate(() => document.querySelector('[data-view="graph"]').click());
  await page.waitForSelector('#graphMergeBtn', { state: 'visible', timeout: 5000 });
  await page.click('#graphMergeBtn');
  await page.waitForFunction(() => graphNodes.length >= 2, null, { timeout: 5000 });
  const mergedCount = await page.evaluate(() => graphNodes.length);
  log('  ✓ 查询结果并入图探索 (graphNodes=' + mergedCount + ')');
  await page.evaluate(() => document.querySelector('[data-view="grid"]').click());

  // 冒烟 3：依次切换三个主标签（切换本身不得产生新的查询请求）
  const tqlBeforeTabs = tqlRequests.length;
  for (const tabId of ['graphExplorePane', 'vectorLabPane', 'queryPane']) {
    await page.click(`[data-tab="${tabId}"]`);
    await page.waitForSelector(`#${tabId}.active`, { timeout: 5000 });
  }
  if (tqlRequests.length !== tqlBeforeTabs) {
    throw new Error(`切换主标签不应执行查询，实际新增 ${tqlRequests.length - tqlBeforeTabs} 次`);
  }
  log('  ✓ 三个主标签切换（0 次查询）');

  // 冒烟 4：图规模护栏 —— 注入 700 节点，用 LIMIT 500 的 MATCH 出图验证不冻结：
  // >300 节点应自动暂停力学模拟（O(n²) tick 护栏）并显示提示条
  log('  注入 700 个规模节点 (350 次链式 CREATE)');
  for (let i = 0; i < 350; i++) {
    const res = await seed(`CREATE ({name: "ScaleA${i}", type: "person"})-[:KNOWS]->({name: "ScaleB${i}", type: "person"})`);
    if (res.status !== 200) throw new Error(`规模种子写入失败 (i=${i}): ` + res.text);
  }
  await page.evaluate(() => {
    setEditorValue('MATCH (a)-[]->(b) RETURN a, b LIMIT 500');
  });
  await page.click('#runQueryBtn');
  // 351 行 = 1 条种子 KNOWS 边 + 350 条注入边（MATCH 按边返回行）
  await page.waitForFunction(
    () => document.getElementById('statusRowCount').textContent.includes('返回: 351 行'),
    null, { timeout: 15000 }
  );
  const guard = await page.evaluate(() => ({
    gridRows: document.querySelectorAll('#gridBody tr').length,
    physicsBtn: document.getElementById('togglePhysicsBtn').textContent,
    noticeHidden: document.getElementById('graphScaleNotice').hidden,
    noticeText: document.getElementById('graphScaleNotice').textContent,
  }));
  // 虚拟滑窗（PR-12）：351 行 > 阈值 200 → DOM 只渲染可视窗口（远小于数据量）
  if (guard.gridRows >= 100) throw new Error('351 行应走虚拟滑窗（tbody DOM 行数 < 100），实际 ' + guard.gridRows);
  if (guard.physicsBtn !== '恢复力学') throw new Error('700 节点应自动暂停力学（按钮应为「恢复力学」），实际: ' + guard.physicsBtn);
  if (guard.noticeHidden) throw new Error('图规模提示条应显示');
  if (!guard.noticeText.includes('300')) throw new Error('图规模提示条应说明 300 阈值: ' + guard.noticeText);
  log('  ✓ 图规模护栏生效 (700 节点 → 力学暂停 + 提示条，页面未冻结)');

  // 冒烟 5：虚拟滑窗滚动 —— 滚到中部后窗口行号/内容必须正确（spacer 换算与实测行距一致）
  await page.evaluate(() => {
    const c = document.getElementById('viewGrid');
    c.scrollTop = 3000;
    c.dispatchEvent(new Event('scroll'));
  });
  await page.waitForFunction(
    () => {
      const tr = document.querySelector('#gridBody tr[data-row-index="90"]');
      // 首格是批量选择（复选框），行号在 data-col-key="#" 上
    return tr && (tr.querySelector('td[data-col-key="#"]') || {}).textContent === '91' && tr.textContent.length > 5;
    },
    null, { timeout: 5000 }
  );
  log('  ✓ 虚拟滑窗滚动正确 (351 行窗口化，滚动后行号 91 / 内容一致)');

  // 冒烟 5：WebGL 渲染后端切换（Sigma CDN 按需加载；离线/CDN 不可达时回退 Canvas + toast）
  await page.evaluate(() => document.querySelector('[data-view="graph"]').click());
  await page.waitForSelector('#toggleWebGLBtn', { state: 'visible', timeout: 5000 });
  await page.click('#toggleWebGLBtn');
  await page.waitForFunction(
    () => (typeof graphBackend !== 'undefined' && graphBackend === 'sigma')
      || document.querySelector('#toastStack .toast'),
    null, { timeout: 20000 }
  );
  const webglState = await page.evaluate(() => ({
    backend: graphBackend,
    sigmaVisible: !document.getElementById('graphSigmaContainer').hidden,
    toast: (document.querySelector('#toastStack .toast') || {}).textContent || '',
  }));
  if (webglState.backend === 'sigma') {
    if (!webglState.sigmaVisible) throw new Error('WebGL 模式下 Sigma 容器应显示');
    log('  ✓ WebGL (Sigma) 渲染后端加载成功并接管图视图');
  } else {
    if (!webglState.toast) throw new Error('WebGL 库不可达时应出现回退 toast');
    log('  ✓ WebGL 库不可达，按设计回退 Canvas（toast 提示）');
  }
  await page.click('#toggleWebGLBtn');
  await page.waitForFunction(() => graphBackend === 'canvas', null, { timeout: 5000 });
  log('  ✓ 图渲染后端切换往返正常');

  // 冒烟 5.5：AI 助手（PR-15）—— 入口按钮常显（虚线未配置态），未配置点击
  // 打开抽屉（含未配置横幅）并直达设置模态
  await page.click('#aiChatToggleBtn');
  await page.waitForSelector('#aiSettingsOverlay.open', { timeout: 5000 });
  await page.click('#aiSettingsCloseBtn');
  const aiState = await page.evaluate(() => ({
    drawerOpen: document.getElementById('aiChatDrawer').classList.contains('open'),
    bannerVisible: !document.getElementById('aiNotConfiguredBanner').hidden,
    unconfigured: document.getElementById('aiChatToggleBtn').classList.contains('ai-toggle-unconfigured'),
  }));
  if (!aiState.drawerOpen) throw new Error('未配置点击入口应打开 AI 抽屉');
  if (!aiState.bannerVisible) throw new Error('未配置时抽屉内应显示未配置横幅');
  if (!aiState.unconfigured) throw new Error('未配置态入口按钮应有弱化样式');
  log('  ✓ AI 助手入口常显：未配置点击直达设置，抽屉内含未配置横幅');
  // 关闭 AI 抽屉本体：抽屉停靠右侧，若保持打开会拦截后续右下角按钮（星云模式等）的点击
  await page.click('#uspCollapseBtn');
  await page.waitForFunction(() => !document.getElementById('aiChatDrawer').classList.contains('open'), null, { timeout: 5000 });

  // 冒烟 5.6：探索器内 TQL 弹窗（PR-16）—— 示例填充 → 真实只读查询 → 结果并入图，
  // 全程停留在独立探索标签（探索流不断线）
  await page.click('[data-tab="graphExplorePane"]');
  await page.waitForSelector('#graphExplorePane.active', { timeout: 5000 });
  // 先清空图：前序规模冒烟已把 700 节点载入图，不清空则 merge 新增恒为 0 无法断言
  await page.evaluate(() => renderGraph([]));
  await page.click('#graphTqlBtn');
  await page.waitForSelector('#graphTqlOverlay.open', { timeout: 5000 });
  await page.evaluate(() => {
    const ex = document.getElementById('graphTqlExample');
    ex.value = 'ex-match';
    ex.dispatchEvent(new Event('change', { bubbles: true }));
  });
  const tqlFilled = await page.evaluate(() => document.getElementById('graphTqlInput').value);
  if (!tqlFilled.startsWith('MATCH')) throw new Error('示例应填充 TQL 编辑区');
  await page.click('#graphTqlRunBtn');
  await page.waitForFunction(
    () => document.getElementById('graphTqlStatus').textContent.includes('新增'),
    null, { timeout: 15000 }
  );
  const tqlMerge = await page.evaluate(() => ({
    status: document.getElementById('graphTqlStatus').textContent,
    nodes: graphNodes.length,
    paneActive: document.getElementById('graphExplorePane').classList.contains('active'),
  }));
  if (tqlMerge.nodes === 0) throw new Error('TQL 结果应并入图: ' + tqlMerge.status);
  if (!tqlMerge.paneActive) throw new Error('运行后应停留在独立探索标签');
  log(`  ✓ 探索器内 TQL 并入图 (${tqlMerge.status})，探索流未中断`);
  await page.click('#graphTqlCloseBtn');

  // 冒烟 5.7：星云模式（PR-18）—— 真实 server 702 节点：单查询 MATCH 上限 10000 拉全图，
  // Sigma 专属 + 物理必关 + 静态聚类布局；WebGL 库不可达时星云中止 + toast（与 WebGL 冒烟同双路径）。
  // PR-20：退出改为恢复进入前快照——先摆 2 个种子节点作为快照源
  await page.evaluate(() => {
    // 后续会真实打开详情并请求邻域；快照必须引用库内节点，不能用非法的 s1/s2 ID。
    const ids = ['Smoke Alice', 'Smoke Bob'].map(name => graphNodes.find(n => n.label === name)?.id);
    if (ids.some(id => id == null)) throw new Error('星云快照缺少库内种子节点');
    renderGraph([
      { a: { type: 'node', id: ids[0], payload: { name: 'PreNebula A', type: 'person' }, vector: [] },
        b: { type: 'node', id: ids[1], payload: { name: 'PreNebula B', type: 'person' }, vector: [] } }
    ]);
  });
  // 清空前序步骤残留 toast：避免「任一 toast 存在」的等待条件被旧 toast 瞬间满足
  await page.evaluate(() => { const s = document.getElementById('toastStack'); if (s) s.textContent = ''; });
  await page.click('#nebulaModeBtn');
  await page.waitForFunction(
    () => (typeof nebulaActive !== 'undefined' && nebulaActive === true)
      || Array.from(document.querySelectorAll('#toastStack .toast')).some(t => t.textContent.indexOf('星云') !== -1),
    null, { timeout: 60000 }
  );
  const nebulaState = await page.evaluate(() => ({
    active: typeof nebulaActive !== 'undefined' && nebulaActive === true,
    nodes: graphNodes.length,
    edges: graphEdges.length,
    physicsRunning: simulationRunning,
    layout: graphLayoutMode,
    backend: graphBackend,
    btnText: document.getElementById('nebulaModeBtn').textContent,
    toast: Array.from(document.querySelectorAll('#toastStack .toast')).map(t => t.textContent).join('|'),
  }));
  if (nebulaState.active) {
    if (nebulaState.nodes < 702) throw new Error('星云应合并全部 702 节点，实际 ' + nebulaState.nodes);
    if (nebulaState.physicsRunning) throw new Error('星云应物理关闭');
    if (nebulaState.layout !== 'cluster') throw new Error('星云应为静态聚类布局，实际 ' + nebulaState.layout);
    if (nebulaState.backend !== 'sigma') throw new Error('星云应为 Sigma 后端');
    log(`  ✓ 星云模式进入：${nebulaState.nodes} 节点 / ${nebulaState.edges} 边 / 物理关闭 / 静态聚类 (Sigma)`);
    // 退出星云（PR-20：无 confirm，直接退出并恢复进入前快照）
    await page.click('#nebulaModeBtn');
    await page.waitForFunction(() => nebulaActive === false, null, { timeout: 10000 });
    const exited = await page.evaluate(() => ({
      nodes: graphNodes.length,
      layout: graphLayoutMode,
      backend: graphBackend,
      names: graphNodes.map(n => n.label).join(','),
    }));
    if (exited.layout !== 'force') throw new Error('退出星云后应恢复进入前布局（force）');
    if (exited.nodes !== 2 || exited.names.indexOf('PreNebula A') === -1) {
      throw new Error('退出星云后应恢复进入前快照（2 个 PreNebula 种子节点），实际 ' + exited.nodes + ' [' + exited.names + ']');
    }
    log(`  ✓ 星云模式退出：恢复进入前快照 (${exited.nodes} 节点，力导向)`);
  } else {
    if (!nebulaState.toast || nebulaState.toast.indexOf('星云') === -1) {
      throw new Error('星云失败时应有中止 toast，实际: ' + nebulaState.toast);
    }
    log('  ✓ 星云模式因 WebGL 库不可达中止并 toast（按设计）');
  }

  // 冒烟 5.8：右侧统一面板（PR-19）—— 打开节点 → 详情 tab；收起 → 导航条切过滤器 tab；再收起
  await page.evaluate(() => {
    const n = graphNodes[0];
    openNodeDrawer({ id: n.id, label: n.label, payload: n.payload, vector: [] });
  });
  await page.waitForFunction(() => uspActiveTab === 'detail', null, { timeout: 5000 });
  const upanel = await page.evaluate(() => ({
    panelOpen: document.getElementById('unifiedSidePanel').classList.contains('open'),
    detailVisible: !document.getElementById('uspBodyDetail').hidden,
    shellOpen: document.getElementById('nodeDrawer').classList.contains('open'),
    title: document.getElementById('drawerNodeTitle').textContent,
  }));
  if (!upanel.panelOpen || !upanel.detailVisible || !upanel.shellOpen) {
    throw new Error('统一面板详情 tab 映射失败: ' + JSON.stringify(upanel));
  }
  log(`  ✓ 统一面板：打开节点 → 详情 tab (${upanel.title})`);
  await page.click('#uspCollapseBtn');
  await page.waitForFunction(() => uspActiveTab === null, null, { timeout: 5000 });
  await page.click('#uspRailFilterBtn');
  await page.waitForFunction(() => uspActiveTab === 'filter'
    && !document.getElementById('graphFilterPanel').hidden, null, { timeout: 5000 });
  const filterSummary = await page.evaluate(() => document.getElementById('gfSummary').textContent);
  if (!filterSummary.includes('节点')) throw new Error('过滤器 tab 应渲染统计摘要: ' + filterSummary);
  log(`  ✓ 统一面板：收起 → 导航条切过滤器 tab (${filterSummary})`);
  await page.click('#uspCollapseBtn');
  await page.waitForFunction(() => uspActiveTab === null, null, { timeout: 5000 });
  log('  ✓ 统一面板：收起回导航条');

  // 冒烟 5.9：0.8.8 图探索 API —— k-hop 单请求 /neighbors + 边列举双向 + 路径追踪
  // （种子数据：Smoke Alice -KNOWS-> Smoke Bob）
  // 注意：前序星云步骤的图可能已被快照恢复替换（含非数字 id 的合成节点），
  // 必须先跑一次真实查询把种子节点重新载入，才能命中 /v1/nodes/{id}/edges
  await page.evaluate(() => {
    document.querySelector('[data-tab="queryPane"]').click();
    setEditorValue('FIND {type: "person"} RETURN * LIMIT 25');
  });
  await page.click('#runQueryBtn');
  await page.waitForFunction(() => graphNodes.some(n => n.label === 'Smoke Alice'), null, { timeout: 15000 });
  const aliceId = await page.evaluate(() => {
    const n = graphNodes.find(x => x.label === 'Smoke Alice');
    openNodeDrawer({ id: n.id, label: n.label, payload: n.payload, vector: n.vector || [] });
    return String(n.id);
  });
  await page.waitForFunction(() => document.querySelectorAll('#drawerNodeEdges .inspector-edge-item').length >= 1,
    null, { timeout: 8000 });
  const edgeInfo = await page.evaluate(() => ({
    text: document.getElementById('drawerNodeEdges').textContent,
    count: document.querySelectorAll('#drawerNodeEdges .inspector-edge-item').length,
  }));
  if (edgeInfo.text.indexOf('KNOWS') === -1) throw new Error('抽屉边列表应含 KNOWS（/v1/nodes/{id}/edges）: ' + edgeInfo.text);
  log(`  ✓ 边列举（0.8.8 /edges）：${edgeInfo.count} 条真实边，含 KNOWS`);

  await page.evaluate(() => document.getElementById('expandNeighborhoodBtn').click());
  await page.waitForFunction(() => Array.from(document.querySelectorAll('#toastStack .toast'))
    .some(t => t.textContent.indexOf('邻域') !== -1 || t.textContent.indexOf('已在图中') !== -1), null, { timeout: 15000 });
  log('  ✓ k-hop 邻域（0.8.8 /neighbors 单请求）执行成功');

  const bobId = await page.evaluate(() => {
    const n = graphNodes.find(x => x.label === 'Smoke Bob');
    return n ? String(n.id) : null;
  });
  if (bobId) {
    await page.evaluate((to) => {
      document.getElementById('pathTargetInput').value = to;
      document.getElementById('pathHopsSelect').value = '4';
      document.getElementById('tracePathBtn').click();
    }, bobId);
    await page.waitForFunction(() => document.getElementById('pathHint').textContent.indexOf('路径') !== -1,
      null, { timeout: 15000 });
    const pathState = await page.evaluate(() => ({
      hint: document.getElementById('pathHint').textContent,
      highlighted: pathHighlightEdges.size,
      clearVisible: !document.getElementById('clearPathBtn').hidden,
    }));
    if (pathState.highlighted < 1 || !pathState.clearVisible) {
      throw new Error('路径追踪应高亮路径并显示清除按钮: ' + JSON.stringify(pathState));
    }
    log(`  ✓ 路径追踪（0.8.8 /paths + batch-get）：${pathState.hint}`);
    await page.click('#clearPathBtn');
    const cleared = await page.evaluate(() => pathHighlightEdges.size === 0
      && document.getElementById('clearPathBtn').hidden);
    if (!cleared) throw new Error('清除路径高亮应复位');
    log('  ✓ 路径高亮清除复位');
  } else {
    log('  ⚠ 未找到 Smoke Bob 节点，跳过路径追踪冒烟');
  }
  await page.click('#uspCollapseBtn');

  log(`5/6 打开 ${BASE}/ui?selftest=1 断言自检套件`);
  await page.goto(`${BASE}/ui?selftest=1`, { waitUntil: 'domcontentloaded', timeout: 20000 });
  await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true, null, { timeout: 10000 });
  await page.waitForSelector('#selftestBar', { timeout: 15000 });
  // 边界加固断言含万行网格分块渲染与万元素 JSON 树懒渲染，整体预算放宽到 120s
  await page.waitForFunction(() => window.__selftestDone === true, null, { timeout: 120000 });
  const selftest = await page.evaluate(() => ({
    failed: window.__selftestFailedCount,
    results: window.__selftestResults,
  }));
  for (const item of selftest.results) {
    const mark = item.status === 'pass' ? '✓' : '✗';
    log(`  ${mark} [${item.group}] ${item.name}${item.error ? ' — ' + item.error : ''}`);
  }
  if (selftest.failed !== 0) {
    console.error(`[ui-smoke] 页内自检存在 ${selftest.failed} 条失败`);
    failures += selftest.failed;
  } else {
    log(`  ✓ 页内自检全部通过 (${selftest.results.length} 条)`);
  }
} catch (err) {
  console.error('[ui-smoke] 冒烟失败: ' + (err && err.message ? err.message : String(err)));
  // 失败时转储页面关键状态，便于远程定位
  try {
    if (page) {
      const dump = await page.evaluate(() => ({
        url: location.href,
        appReady: window.__TRIVIUM_APP_READY === true,
        statusRowCount: document.getElementById('statusRowCount')?.textContent,
        statusQueryTime: document.getElementById('statusQueryTime')?.textContent,
        gridRows: document.querySelectorAll('#gridBody tr').length,
        gridHead: document.getElementById('gridBody')?.textContent?.slice(0, 200),
        connText: document.getElementById('connText')?.textContent,
      }));
      console.error('[ui-smoke] 页面状态: ' + JSON.stringify(dump, null, 2));
    }
  } catch (_) { /* page may be gone */ }
  failures += 1;
} finally {
  log('6/6 清理浏览器进程 / server 进程 / 临时库');
  await cleanup(failures === 0 ? 0 : 1);
}
