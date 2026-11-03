#!/usr/bin/env node
// V4.1 R2 产品验收：统一外壳 / 双语 / 侧栏 / 结果形状 / 图视觉 / 向量上下文 / 写入审阅。
//
// 与既有套件的关系：
//   - webui-v4-combos.mjs 负责 120 组合的状态矩阵（本脚本按语义补 R2 场景，不重复截图矩阵）；
//   - webui-v4-regressions.mjs 负责编辑器/表格/图交互等既有回归；
//   - 本脚本覆盖 R2 新增的交互边界，并要求真实 server + 真实数据。
//
// 用法：
//   node scripts/webui-v41-r2-checks.mjs                 # 自动构建并启动临时 server
//   set TDB_BASE=http://127.0.0.1:8091 && node ...       # 复用已有 server（不再启动/清理）
//   set WEBUI_BROWSER=webkit && node ...                 # 第二引擎

import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const RUN_ID = `${process.pid}-${Date.now()}`;
const OUT = path.join(ROOT, '.tmp', 'v41-r2-checks', RUN_ID);
const DB_DIR = path.join(OUT, 'db');
const BUILD_TARGET = path.resolve(process.env.CARGO_TARGET_DIR || path.join(ROOT, 'target', 'webui-tests'));
const EXE = path.join(BUILD_TARGET, 'debug', process.platform === 'win32' ? 'triviumdb-server.exe' : 'triviumdb-server');
const BROWSER = process.env.WEBUI_BROWSER || 'chromium';
const EXTERNAL_BASE = process.env.TDB_BASE || '';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const failures = [];
const checks = [];
let PORT = 0;
let BASE = EXTERNAL_BASE;
let serverProc = null;

const expect = (cond, msg) => {
  checks.push({ msg, ok: !!cond });
  if (!cond) failures.push(msg);
};
const log = (m) => console.log(`[v41-r2] ${m}`);

function resolveCargo() {
  const name = process.platform === 'win32' ? 'cargo.exe' : 'cargo';
  const probe = spawnSync(name, ['--version'], { encoding: 'utf8' });
  if (probe.status === 0) return name;
  const full = path.join(os.homedir(), '.cargo', 'bin', name);
  return existsSync(full) ? full : null;
}

function findPort(from = 8170, to = 8199) {
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

function request(method, urlPath, { body, headers } = {}) {
  return new Promise((resolve, reject) => {
    const req = http.request(`${BASE}${urlPath}`, { method, headers: headers || {} }, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => {
        const text = Buffer.concat(chunks).toString('utf8');
        let json = null;
        try { json = JSON.parse(text); } catch (_) { /* not json */ }
        resolve({ status: res.statusCode, json, text });
      });
    });
    req.on('error', reject);
    if (body) req.write(body);
    req.end();
  });
}

async function waitReady(timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await request('GET', '/health/ready');
      if (res.status === 200) return true;
    } catch (_) { /* not up */ }
    await sleep(250);
  }
  return false;
}

async function startServer() {
  if (EXTERNAL_BASE) {
    BASE = EXTERNAL_BASE.replace(/\/$/, '');
    log(`using external server ${BASE}`);
    return;
  }
  mkdirSync(DB_DIR, { recursive: true });
  const cargo = resolveCargo();
  if (!cargo) throw new Error('cargo 不可用：无法构建 server');
  log(`building server (${BUILD_TARGET})`);
  const build = spawnSync(cargo, ['build', '-p', 'triviumdb-server', '--target-dir', BUILD_TARGET], {
    cwd: ROOT, stdio: 'inherit', shell: false,
  });
  if (build.status !== 0) throw new Error('cargo build failed');
  if (!existsSync(EXE)) throw new Error(`server 可执行文件不存在: ${EXE}`);
  PORT = await findPort();
  if (!PORT) throw new Error('找不到空闲端口');
  BASE = `http://127.0.0.1:${PORT}`;
  serverProc = spawn(EXE, [
    '--database', path.join(DB_DIR, 'data.tdb'),
    '--listen', `127.0.0.1:${PORT}`,
    '--dim', '8',
  ], { cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'] });
  // 必须同时消费 stdout/stderr：否则日志写满管道后 server 会卡死
  serverProc.stdout.on('data', () => {});
  serverProc.stderr.on('data', () => {});
  if (!await waitReady()) throw new Error('server 未在 30s 内就绪');
  log(`server ready at ${BASE}`);
}

async function seed() {
  const types = ['person', 'paper', 'topic', 'service', 'trace', 'event', 'host'];
  const nodes = [];
  for (let i = 0; i < 48; i += 1) {
    const id = 1001 + i;
    const payload = {
      name: i === 5 ? '<img src=x onerror="window.__seedXss=1">' : `R2 Node ${i}`,
      citations: (i * 7) % 53,
      tags: ['seed', 'r2'],
      nested: { depth: 2, ratio: i / 48 },
      note: i % 6 === 0 ? null : '长文本用于验证换行与截断：' + 'x'.repeat(i % 5 === 0 ? 100 : 6),
    };
    if (i !== 47) payload.type = types[i % types.length];
    nodes.push({
      id,
      vector: Array.from({ length: 8 }, (_, d) => Math.round(Math.sin(id * 0.7 + d * 1.3) * 1000) / 1000),
      payload,
    });
  }
  const labels = ['KNOWS', 'AUTHORED', 'DEPENDS_ON', 'OBSERVED', 'ROUTES_TO'];
  const edges = [];
  for (let i = 0; i < 88; i += 1) {
    const s = 1001 + (i % 48);
    const t = 1001 + ((i * 7 + 3) % 48);
    if (s !== t) edges.push({ source: s, target: t, label: labels[i % labels.length], weight: 1 });
  }
  const res1 = await request('POST', '/v1/import/nodes', {
    body: nodes.map((n) => JSON.stringify(n)).join('\n'),
    headers: { 'Content-Type': 'application/x-ndjson' },
  });
  const res2 = await request('POST', '/v1/import/edges', {
    body: edges.map((e) => JSON.stringify(e)).join('\n'),
    headers: { 'Content-Type': 'application/x-ndjson' },
  });
  if (res1.status !== 200 || res2.status !== 200) throw new Error(`seed failed ${res1.status}/${res2.status}`);
  log(`seeded ${nodes.length} nodes / ${edges.length} edges`);
}

async function loadPlaywright() {
  try { return await import('playwright'); } catch (_) { /* fall through */ }
  const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm';
  const probe = spawnSync(npm, ['root', '-g'], { encoding: 'utf8', shell: process.platform === 'win32' });
  if (probe.status === 0 && probe.stdout) {
    const root = probe.stdout.trim();
    const { createRequire } = await import('node:module');
    for (const base of [path.join(root, 'noop.js'), path.join(root, '@playwright', 'cli', 'node_modules', 'noop.js')]) {
      try {
        const pw = createRequire(base)('playwright');
        if (pw && pw.chromium) return pw;
      } catch (_) { /* next */ }
    }
  }
  return null;
}

const pw = await loadPlaywright();
if (!pw) {
  console.error('[v41-r2] playwright 不可用');
  process.exit(1);
}
const engine = pw[BROWSER] || pw.chromium;
if (!engine) {
  console.error(`[v41-r2] 未知浏览器 ${BROWSER}`);
  process.exit(1);
}

mkdirSync(OUT, { recursive: true });
const browser = await engine.launch({ headless: true });
let shotIndex = 0;
const shot = async (page, name) => {
  shotIndex += 1;
  await page.screenshot({ path: path.join(OUT, `${String(shotIndex).padStart(2, '0')}-${name}.png`) });
};

async function newPage({ width = 1440, height = 900, theme = 'dark', lang = 'zh', reducedMotion = 'no-preference' } = {}) {
  const context = await browser.newContext({ viewport: { width, height }, reducedMotion });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(`console: ${m.text()}`);
  });
  await page.addInitScript(({ theme, lang }) => {
    try {
      localStorage.setItem('tdb_theme', theme);
      localStorage.setItem('tdb_lang', lang);
    } catch (_) { /* ignore */ }
  }, { theme, lang });
  await page.goto(`${BASE}/ui`, { waitUntil: 'load' });
  await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true, null, { timeout: 20000 });
  // 首屏 GSAP timeline 约 0.9s（autoAlpha 期间元素 visibility:hidden）：等动画结束再做 hit-test
  await page.waitForTimeout(1000);
  return { context, page, errors };
}

async function runQuery(page, query) {
  await page.evaluate((q) => {
    document.querySelector('[data-tab="queryPane"]').click();
    document.querySelector('[data-view="grid"]').click();
    setEditorValue(q);
    executeCurrentQuery();
  }, query);
  await page.waitForFunction(() => document.getElementById('resultsContainer').dataset.resultState === 'success',
    null, { timeout: 20000 });
  await page.waitForTimeout(200);
}

try {
  await startServer();
  // 外部 server 假定已由调用方准备好数据（避免重复导入固定 id）
  if (!EXTERNAL_BASE) await seed();

  // ---------- S1. 外壳组合：3 视口 × 暗浅 × zh/en ----------
  for (const viewport of [{ w: 1440, h: 900 }, { w: 1024, h: 768 }, { w: 390, h: 844 }]) {
    for (const theme of ['dark', 'light']) {
      for (const lang of ['zh', 'en']) {
        const { context, page, errors } = await newPage({ width: viewport.w, height: viewport.h, theme, lang });
        const label = `${viewport.w}/${theme}/${lang}`;
        const shell = await page.evaluate(() => {
          const rect = (el) => {
            const b = el.getBoundingClientRect();
            return { left: b.left, top: b.top, right: b.right, bottom: b.bottom, w: b.width, h: b.height };
          };
          const visible = (el) => {
            if (!el) return false;
            const s = getComputedStyle(el);
            return s.display !== 'none' && s.visibility !== 'hidden' && el.getBoundingClientRect().width > 0;
          };
          const hit = (el) => {
            if (!visible(el)) return { ok: false, reason: 'invisible' };
            const b = el.getBoundingClientRect();
            const cx = b.left + b.width / 2;
            const cy = b.top + b.height / 2;
            if (cx < 0 || cy < 0 || cx > innerWidth || cy > innerHeight) return { ok: false, reason: 'offscreen' };
            const top = document.elementFromPoint(cx, cy);
            return { ok: !!top && (el === top || el.contains(top) || top.contains(el)), reason: top ? (top.id || top.className) : 'none' };
          };
          const targets = {
            brand: document.querySelector('.header-brand'),
            navQuery: document.getElementById('tab-queryPane'),
            navGraph: document.getElementById('tab-graphExplorePane'),
            navVector: document.getElementById('tab-vectorLabPane'),
            langZh: document.querySelector('.lang-btn[data-lang="zh"]'),
            langEn: document.querySelector('.lang-btn[data-lang="en"]'),
            theme: document.getElementById('themeToggleBtn'),
            run: document.getElementById('runQueryBtn'),
          };
          // 桌面才检查侧栏段按钮；≤900 侧栏是抽屉，按钮不可见是预期
          if (innerWidth > 900) {
            targets.sideObjects = document.querySelector('.side-tab[data-side-tab="objects"]');
            targets.sideHistory = document.querySelector('.side-tab[data-side-tab="history"]');
          }
          const hits = {};
          Object.keys(targets).forEach((key) => { hits[key] = hit(targets[key]); });
          return {
            doc: [document.documentElement.scrollWidth, document.documentElement.clientWidth],
            body: [document.body.scrollWidth, document.body.clientWidth],
            header: rect(document.querySelector('header.app-header')),
            navCount: document.querySelectorAll('.tabs-bar .tab').length,
            langCount: document.querySelectorAll('.lang-btn').length,
            hits,
            docLang: document.documentElement.lang,
          };
        });
        expect(shell.doc[0] <= shell.doc[1] + 1, `${label}: 页面不应横向溢出 ${shell.doc}`);
        expect(shell.body[0] <= shell.body[1] + 1, `${label}: body 不应横向溢出 ${shell.body}`);
        expect(shell.navCount === 3, `${label}: 主导航应为 3 项`);
        expect(shell.langCount === 2, `${label}: 语言切换应常驻 2 个按钮`);
        expect(shell.docLang === (lang === 'en' ? 'en' : 'zh-CN'), `${label}: html lang 应跟随界面语言`);
        Object.entries(shell.hits).forEach(([key, hit]) => {
          expect(hit.ok, `${label}: 控件 ${key} 中心点不可命中 (${hit.reason})`);
        });
        await shot(page, `shell-${viewport.w}-${theme}-${lang}`);
        expect(errors.length === 0, `${label}: console/pageerror 非空 ${errors.join('; ')}`);
        await context.close();
      }
    }
  }

  // ---------- S2. 侧栏拖宽 / 键盘 / 偏好 / rail / 断点往返 ----------
  {
    const { context, page, errors } = await newPage({ width: 1440, height: 900 });
    const widthOf = () => page.evaluate(() => document.querySelector('aside.sidebar').getBoundingClientRect().width);
    const before = await widthOf();
    const handle = await page.locator('#sidebarResize').boundingBox();
    await page.mouse.move(handle.x + handle.width / 2, handle.y + 200);
    await page.mouse.down();
    await page.mouse.move(handle.x + handle.width / 2 + 70, handle.y + 200, { steps: 8 });
    await page.mouse.up();
    const dragged = await widthOf();
    expect(Math.abs(before - 232) < 1, `侧栏默认宽度应为 232，实际 ${before}`);
    expect(Math.abs(dragged - 302) < 4, `拖拽 +70 后应约 302，实际 ${dragged}`);
    await page.reload({ waitUntil: 'load' });
    await page.waitForFunction(() => window.__TRIVIUM_APP_READY === true);
    await page.waitForTimeout(1100);   // GSAP 首屏 autoAlpha 结束前 focus 会落在 hidden 元素上
    expect(Math.abs((await widthOf()) - dragged) < 2, '刷新后应恢复拖拽宽度');

    await page.focus('#sidebarResize');
    expect(await page.evaluate(() => document.activeElement.id) === 'sidebarResize', '调宽分隔线应可聚焦');
    await page.keyboard.press('Home');
    await page.waitForTimeout(260);   // 宽度有 180ms 过渡
    expect(Math.abs((await widthOf()) - 200) < 1, 'Home 应到 200');
    await page.keyboard.press('End');
    await page.waitForTimeout(260);
    expect(Math.abs((await widthOf()) - 340) < 1, 'End 应到 340');
    await page.keyboard.press('ArrowLeft');
    await page.waitForTimeout(260);
    expect(Math.abs((await widthOf()) - 330) < 1, '← 应减 10');
    await page.evaluate(() => applySidebarWidth(232, true));
    await page.waitForTimeout(260);

    await page.click('#sidebarRailToggle');
    await page.waitForTimeout(250);
    expect(Math.abs((await widthOf()) - 64) < 1, 'rail 应收起到 64');
    await page.keyboard.press('Control+b');
    await page.waitForTimeout(250);
    expect(Math.abs((await widthOf()) - 232) < 1, 'Ctrl+B 应展开回 232');

    // 桌面 → 手机 → 桌面：GSAP inline transform 不能锁死抽屉
    await page.setViewportSize({ width: 390, height: 844 });
    await page.waitForTimeout(350);
    const mobile = await page.evaluate(() => {
      const sb = document.querySelector('aside.sidebar');
      return { transform: getComputedStyle(sb).transform, width: sb.getBoundingClientRect().width, left: sb.getBoundingClientRect().left };
    });
    expect(mobile.left + mobile.width <= 2, `390 侧栏应完全移出视口，实际 left=${mobile.left}`);
    await page.click('#sidebarMobileBtn');
    await page.waitForTimeout(300);
    const opened = await page.evaluate(() => document.querySelector('aside.sidebar').getBoundingClientRect().left);
    expect(Math.abs(opened) < 2, '390 抽屉应可打开');
    // 遮罩中心会落在抽屉内部，直接点右侧空白（也验证遮罩命中）
    await page.mouse.click(370, 420);
    await page.waitForTimeout(250);
    expect(await page.evaluate(() => document.documentElement.dataset.sidebarOpen !== '1'), '点遮罩应关闭抽屉');
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.waitForTimeout(350);
    const back = await page.evaluate(() => ({
      left: document.querySelector('aside.sidebar').getBoundingClientRect().left,
      width: document.querySelector('aside.sidebar').getBoundingClientRect().width,
      transform: getComputedStyle(document.querySelector('aside.sidebar')).transform,
    }));
    expect(Math.abs(back.width - 232) < 2 && back.left < 2, `桌面恢复后侧栏应为 232 且在视口内：${JSON.stringify(back)}`);
    expect(errors.length === 0, `侧栏场景 console/pageerror：${errors.join('; ')}`);
    await shot(page, 'sidebar-desktop-restored');
    await context.close();
  }

  // ---------- S3. 语言切换保留状态 ----------
  {
    const { context, page, errors } = await newPage({ width: 1440, height: 900 });
    await runQuery(page, 'FIND {type: "person"} RETURN * LIMIT 3');
    await page.click('#gridBody tr[data-row-index="0"] .entity-name');
    await page.waitForTimeout(400);
    const before = await page.evaluate(() => ({
      editor: document.getElementById('tqlInput').value,
      rows: document.querySelectorAll('#gridBody tr[data-row-index]').length,
      node: document.getElementById('contextName').textContent,
      activeTab: document.querySelector('.tabs-bar .tab.active').dataset.tab,
      lang: document.documentElement.lang,
    }));
    await page.click('.lang-btn[data-lang="en"]');
    await page.waitForTimeout(500);
    const after = await page.evaluate(() => ({
      editor: document.getElementById('tqlInput').value,
      rows: document.querySelectorAll('#gridBody tr[data-row-index]').length,
      node: document.getElementById('contextName').textContent,
      activeTab: document.querySelector('.tabs-bar .tab.active').dataset.tab,
      lang: document.documentElement.lang,
      tabLabel: document.getElementById('tab-queryPane').textContent,
    }));
    expect(after.lang === 'en', '语言应切到 en');
    expect(after.editor === before.editor, '切换语言不得改写查询文本');
    expect(after.rows === before.rows, '切换语言不得清空结果');
    expect(after.node === before.node, '切换语言不得丢失当前节点');
    expect(after.activeTab === before.activeTab, '切换语言不得切走工作区');
    expect(after.tabLabel.includes('TQL Console'), '导航文案应切英文');
    await page.click('.lang-btn[data-lang="zh"]');
    await page.waitForTimeout(300);
    expect(await page.evaluate(() => document.documentElement.lang) === 'zh-CN', '应能切回中文');
    expect(errors.length === 0, `语言场景 console/pageerror：${errors.join('; ')}`);
    await context.close();
  }

  // ---------- S4. 任意 TQL 结果形状 + 导出一致性 ----------
  {
    const { context, page, errors } = await newPage({ width: 1440, height: 900 });
    const shapes = [
      { name: 'scalar', q: 'FIND {type: "person"} RETURN a.name AS name, a.citations AS citations LIMIT 3', cols: 2 },
      { name: 'aggregate', q: 'FIND {type: "person"} RETURN count(*) AS n', cols: 1 },
      { name: 'multi-node', q: 'MATCH (a)-[]->(b) RETURN a, b LIMIT 5', cols: 2 },
      { name: 'null', q: 'FIND {type: "person"} RETURN a.note AS note LIMIT 3', cols: 1 },
    ];
    for (const shape of shapes) {
      await runQuery(page, shape.q);
      const info = await page.evaluate(() => ({
        cols: [...document.querySelectorAll('#gridHead th[data-column-key]')].map((th) => th.dataset.columnKey),
        rows: document.querySelectorAll('#gridBody tr[data-row-index]').length,
        text: document.getElementById('gridBody').textContent,
        json: document.getElementById('rawJsonOutput').textContent,
      }));
      expect(info.cols.length >= shape.cols, `${shape.name}: 列数应保留 ${shape.cols}，实际 ${info.cols}`);
      expect(info.rows > 0, `${shape.name}: 应返回行`);
      expect(info.json.length > 0, `${shape.name}: JSON 视图应有真实数据`);
    }
    // 导出用原始值：节点列仍是 JSON，不是身份卡文本
    const csv = await page.evaluate(() => buildGridCsv());
    expect(csv.includes('{') || csv.includes('null'), 'CSV 应包含原始值（节点为 JSON）');
    expect(errors.length === 0, `结果形状 console/pageerror：${errors.join('; ')}`);
    await shot(page, 'results-multi-node');
    await context.close();
  }

  // ---------- S5. 对象导航：真实类型 / 自定义 / 缺失 / 样本口径 ----------
  {
    const { context, page, errors } = await newPage({ width: 1440, height: 900 });
    await page.waitForFunction(() => document.querySelectorAll('#objectTree .ot-group').length === 4, null, { timeout: 20000 });
    const tree = await page.evaluate(() => ({
      types: [...document.querySelectorAll('#objectTree .ot-group[data-ot-group="ot.types"] .ot-row')].map((r) => r.dataset.otValue),
      note: document.querySelector('#objectTree .ot-group[data-ot-group="ot.types"] .ot-note')?.textContent
        || document.querySelector('#objectTree .ot-group[data-ot-group="ot.types"] .ot-group-note')?.textContent
        || document.querySelector('#objectTree .ot-group[data-ot-group="ot.types"]')?.textContent,
      dots: [...document.querySelectorAll('#objectTree .ot-group[data-ot-group="ot.types"] .ot-dot')].map((d) => d.style.background || ''),
    }));
    expect(tree.types.includes('person') && tree.types.includes('service'), `对象树应包含真实类型：${tree.types}`);
    expect(tree.types.some((t) => t.includes('未标注') || t.includes('unlabeled')), `缺失类型应可发现：${tree.types}`);
    expect(/样本|sample/.test(tree.note || ''), `计数必须说明是样本：${tree.note}`);
    expect(new Set(tree.dots).size > 1, '不同类型应有稳定色点');

    const filled = await page.evaluate(() => {
      const row = document.querySelector('#objectTree .ot-row[data-ot-kind="type"]');
      const value = row.dataset.otValue;
      row.click();
      return { value, editor: document.getElementById('tqlInput').value, tab: document.querySelector('.tabs-bar .tab.active').dataset.tab };
    });
    expect(filled.editor.includes(filled.value), '点类型只应填入查询');
    expect(filled.tab === 'queryPane', '点类型应切回查询台');
    expect(errors.length === 0, `对象导航 console/pageerror：${errors.join('; ')}`);
    await context.close();
  }

  // ---------- S6. 跨工作区节点上下文 ----------
  {
    const { context, page, errors } = await newPage({ width: 1440, height: 900 });
    await runQuery(page, 'FIND {type: "person"} RETURN * LIMIT 5');
    const nodeId = await page.evaluate(() => {
      const row = gridDataRows[0];
      const node = Object.values(row).find((v) => v && v.type === 'node');
      return String(node.id);
    });
    await page.click('#gridBody tr[data-row-index="0"] .entity-name');
    await page.waitForTimeout(400);
    expect((await page.textContent('#contextMeta')).includes(`#${nodeId}`), '当前节点条应显示选中的 id');
    await page.click('[data-tab="vectorLabPane"]');
    await page.waitForTimeout(300);
    const sourceCard = await page.evaluate(() => ({
      hidden: document.getElementById('vlSourceCard').hidden,
      name: document.getElementById('vlSourceCardName').textContent,
      dim: document.getElementById('vlSourceCardDim').textContent,
    }));
    expect(!sourceCard.hidden && sourceCard.dim.includes('8'), `向量来源卡应带入当前节点：${JSON.stringify(sourceCard)}`);
    await page.click('#vlUseContextBtn');
    await page.waitForTimeout(500);
    await page.click('#vlRunBtn');
    await page.waitForFunction(() => document.getElementById('vlResults').dataset.resultState === 'success', null, { timeout: 20000 });
    const hits = await page.evaluate(() => [...document.querySelectorAll('#vlResultBody tr')].map((tr) => ({
      id: tr.querySelector('.entity-meta span:nth-child(2)')?.textContent || '',
      name: tr.querySelector('.vl-hit-name')?.textContent || '',
      buttons: [...tr.querySelectorAll('.vl-hit-actions button')].map((b) => b.textContent),
    })));
    expect(hits.length > 0, '向量检索应返回真实命中');
    expect(hits.every((h) => h.buttons.length === 3), `每条命中应有 详情/关系/用作来源：${JSON.stringify(hits[0])}`);
    await page.click('[data-tab="graphExplorePane"]');
    await page.waitForTimeout(300);
    expect(!(await page.evaluate(() => document.getElementById('contextStrip').hidden)), '切到图探索后当前节点条仍应可见');
    await page.click('#contextGraphBtn');
    await page.waitForTimeout(1200);
    const graphHas = await page.evaluate((id) => graphNodes.some((n) => String(n.id) === String(id)), nodeId);
    expect(graphHas, '「探索关系」应把当前节点加入图并定位');
    expect(errors.length === 0, `跨工作区 console/pageerror：${errors.join('; ')}`);
    await shot(page, 'context-graph');
    await context.close();
  }

  // ---------- S7. 密集图视觉角色 / 光晕 / 取景避让（独立探索视图，含查询舱/抽屉） ----------
  {
    const { context, page, errors } = await newPage({ width: 1440, height: 900, theme: 'dark' });
    await page.click('[data-tab="graphExplorePane"]');
    await page.waitForTimeout(300);
    const dense = await page.evaluate(() => {
      const names = Array.from({ length: 48 }, (_, i) => `Dense ${i}`);
      const nodes = names.map((name, i) => ({
        type: 'node', id: String(3000 + i),
        payload: { name, type: ['person', 'paper', 'topic'][i % 3] },
        vector: [], edges: [],
      }));
      const rows = [];
      for (let i = 0; i < 48; i += 1) {
        const s = (i * 7 + 3) % 48;
        if (s === i) continue;   // 不要自环：邻居集不含自身
        rows.push({
          a: nodes[s], b: nodes[i],
          relation: { type: 'edge', source: nodes[s].id, target: nodes[i].id, label: 'LINK', weight: 1 },
        });
      }
      collapseUnifiedPanel();
      graphLayoutMode = 'force'; simulationRunning = false; physicsLockedByScale = false;
      renderGraph(rows);
      selectedNode = null; hoveredNode = null;
      const radius = nodeRadius({ degree: 0 });
      const focusNode = graphNodes.find((n) => (n.degree || 0) > 0) || graphNodes[0];
      const glowNormal = graphAuroraGlow(focusNode);
      selectedNode = focusNode;
      graphAuroraNeighbors = graphNeighborIds(selectedNode);
      const glowSelected = graphAuroraGlow(focusNode);
      const neighbor = graphNodes.find((n) => graphAuroraNeighbors.has(String(n.id)));
      const neighborGlow = neighbor ? graphAuroraGlow(neighbor) : -1;
      fitGraphView('standalone', false);
      const area = graphFitArea('standalone', 'canvas');
      const canvas = document.getElementById('standaloneGraphCanvas').getBoundingClientRect();
      const cockpit = document.querySelector('#standaloneGraphShell .v2-cockpit').getBoundingClientRect();
      const dock = document.getElementById('graphDock').getBoundingClientRect();
      const radiusScaled = nodeRadius({ degree: 3 });
      return {
        radius, glowNormal, glowSelected, neighborGlow,
        area, canvas: { w: canvas.width, h: canvas.height },
        cockpitBottom: cockpit.bottom - canvas.top,
        dockTop: dock.top - canvas.top,
        radiusScaled,
      };
    });
    // 用 canvas 实际绘制统计标签数（drawGraphScene 的 fillText 计数）
    const labelStats = await page.evaluate(() => {
      const canvas = document.getElementById('standaloneGraphCanvas');
      const ctx = canvas.getContext('2d');
      const original = ctx.fillText;
      const names = new Set(graphNodes.map((n) => n.label));
      let nodeLabels = 0;
      ctx.fillText = function (text, ...rest) {
        if (names.has(text)) nodeLabels += 1;
        return original.call(this, text, ...rest);
      };
      try { drawGraphScene(ctx, standaloneTransform, canvas); } finally { ctx.fillText = original; }
      return { nodeLabels, total: graphNodes.length };
    });
    expect(dense.radius < 10, `普通节点应是较小色点（半径<10），实际 ${dense.radius}`);
    expect(dense.radiusScaled > dense.radius, '度数-半径缩放仍应保留');
    expect(dense.glowNormal === 0, `无选中时普通节点不应持续发光，实际 ${dense.glowNormal}`);
    expect(dense.glowSelected > 0, '暗色下选中节点应有轻光晕');
    expect(dense.neighborGlow > 0 && dense.neighborGlow < dense.glowSelected, '暗色下邻居应有更弱光晕');
    expect(labelStats.nodeLabels < labelStats.total, `密集图标签应按上下文裁剪：${labelStats.nodeLabels}/${labelStats.total}`);
    expect(dense.area.y >= dense.cockpitBottom - 1, `取景顶部应避开查询舱：${dense.area.y} vs ${dense.cockpitBottom}`);
    expect(dense.area.y + dense.area.height <= dense.dockTop + 1, `取景底部应避开结果抽屉：${dense.area.y + dense.area.height} vs ${dense.dockTop}`);
    expect(dense.area.x >= 0 && dense.area.x + dense.area.width <= dense.canvas.w + 1, '取景不应超出画布');
    expect(errors.length === 0, `密集图 console/pageerror：${errors.join('; ')}`);
    await shot(page, 'graph-dense-selected');

    // reduced-motion：仍可交互、不启动无意义动画
    const { context: rmCtx, page: rmPage } = await newPage({ width: 1024, height: 768, reducedMotion: 'reduce' });
    await rmPage.evaluate(() => {
      document.querySelector('[data-tab="graphExplorePane"]').click();
      renderGraph([{ a: { type: 'node', id: '1', payload: { name: 'A', type: 'person' }, vector: [], edges: [] } }]);
    });
    const rm = await rmPage.evaluate(() => ({
      motion: isGraphMotionEnabled(),
      glow: (() => { selectedNode = graphNodes[0]; graphAuroraNeighbors = graphNeighborIds(selectedNode); return graphAuroraGlow(graphNodes[0]); })(),
    }));
    expect(rm.motion === false, 'reduced-motion 下动效应关闭');
    expect(rm.glow === 0, 'reduced-motion 下不应有光晕动画');
    await rmCtx.close();
    await context.close();
  }

  // ---------- S8. 写入审阅：暂存 / 差异 / 提交 / 失败保留 / 放弃 ----------
  {
    const { context, page, errors } = await newPage({ width: 1440, height: 900 });
    await runQuery(page, 'FIND {type: "person"} RETURN * LIMIT 3');
    const targetId = await page.evaluate(() => String(Object.values(gridDataRows[0]).find((v) => v && v.type === 'node').id));
    // 右键节点单元格 → 编辑 payload（比双击更稳：单击会先打开详情面板）
    await page.click('#gridBody tr[data-row-index="0"] .entity-cell', { button: 'right' });
    await page.waitForTimeout(300);
    await page.click('#gridCtxMenu button[data-ctx-act="edit"]');
    await page.waitForTimeout(300);
    const original = await page.evaluate(async (id) => (await (await fetch(`/v1/nodes/${id}`)).json()).node.payload, targetId);
    await page.fill('#payloadEditorText', JSON.stringify({ ...original, name: 'R2 Staged', marker: 'r2' }, null, 2));
    await page.click('#payloadEditorSave');
    await page.waitForFunction(() => document.getElementById('payloadEditor').hidden === true);
    await page.waitForTimeout(300);
    const staged = await page.evaluate(() => ({
      count: txOperations.length,
      bar: !document.getElementById('txBar').hidden,
      label: document.getElementById('txCount').textContent,
      summary: document.getElementById('txSummary').textContent,
    }));
    expect(staged.count === 1 && staged.bar, `保存应进入暂存队列：${JSON.stringify(staged)}`);
    expect(staged.label.includes('1'), '待提交条应显示数量');
    const serverBefore = await page.evaluate(async (id) => (await (await fetch(`/v1/nodes/${id}`)).json()).node.payload.marker, targetId);
    expect(serverBefore !== 'r2', '暂存不应写库');

    await page.click('#txReviewBtn');
    await page.waitForTimeout(200);
    const review = await page.evaluate(() => ({
      open: !document.getElementById('txReviewOverlay').hidden,
      text: document.getElementById('txReviewList').textContent,
    }));
    expect(review.open && review.text.includes('marker') && review.text.includes('R2 Staged'),
      `审阅应展示对象与差异：${review.text.slice(0, 120)}`);

    // 失败路径：拦截事务接口 → 暂存必须保留
    await page.route('**/v1/transactions', (route) => route.fulfill({
      status: 500, contentType: 'application/problem+json',
      body: JSON.stringify({ title: 'Forced failure', detail: 'r2 failure path', status: 500 }),
    }));
    await page.click('#txReviewCommitBtn');
    await page.waitForTimeout(800);
    const afterFail = await page.evaluate(() => ({
      count: txOperations.length,
      open: !document.getElementById('txReviewOverlay').hidden,
      server: null,
    }));
    expect(afterFail.count === 1, '提交失败必须保留暂存操作');
    expect(afterFail.open, '提交失败应保留审阅弹层');
    await page.unroute('**/v1/transactions');

    // 成功提交 → 真实写库
    await page.click('#txReviewCommitBtn');
    await page.waitForFunction(() => txOperations.length === 0, null, { timeout: 15000 });
    await page.waitForTimeout(500);
    const serverAfter = await page.evaluate(async (id) => (await (await fetch(`/v1/nodes/${id}`)).json()).node.payload, targetId);
    expect(serverAfter.marker === 'r2', `提交后应真实写库：${JSON.stringify(serverAfter)}`);
    expect(await page.evaluate(() => document.getElementById('txBar').hidden), '提交后待提交条应收起');

    // 放弃路径（同样走右键菜单）
    await page.click('#gridBody tr[data-row-index="0"] .entity-cell', { button: 'right' });
    await page.waitForTimeout(300);
    await page.click('#gridCtxMenu button[data-ctx-act="edit"]');
    await page.fill('#payloadEditorText', JSON.stringify({ ...serverAfter, name: 'R2 Discard' }, null, 2));
    await page.click('#payloadEditorSave');
    await page.waitForFunction(() => document.getElementById('payloadEditor').hidden === true);
    await page.click('#txRollbackBtn');
    await page.waitForTimeout(300);
    expect(await page.evaluate(() => txOperations.length === 0), '放弃应清空暂存');
    const serverDiscard = await page.evaluate(async (id) => (await (await fetch(`/v1/nodes/${id}`)).json()).node.payload.name, targetId);
    expect(serverDiscard !== 'R2 Discard', '放弃不应写库');
    // 本场景故意注入一次 500（失败路径），只放过这一条资源错误
    const unexpected = errors.filter((e) => !/status of 500/.test(e));
    expect(unexpected.length === 0, `写入审阅 console/pageerror：${unexpected.join('; ')}`);
    await shot(page, 'write-review');
    await context.close();
  }

  // ---------- S9. 向量异步隔离：旧响应不得覆盖新请求 ----------
  {
    const { context, page, errors } = await newPage({ width: 1440, height: 900 });
    await page.click('[data-tab="vectorLabPane"]');
    await page.evaluate(() => {
      document.getElementById('vlSourceTabJson').click();
      document.getElementById('vlJsonInput').value = '[0.1,0.2,0.3,0.4,0.5,0.6,0.7,0.8]';
      updateVectorJsonPreview();
    });
    let releaseFirst;
    let call = 0;
    await page.route('**/v1/search/vector*', async (route) => {
      call += 1;
      const current = call;
      if (current === 1) {
        await new Promise((resolve) => { releaseFirst = resolve; });
        return route.fulfill({
          status: 200, contentType: 'application/json',
          body: JSON.stringify({ hits: [
            { id: '1001', score: 9.1, payload: { name: 'Old A', type: 'person' } },
            { id: '1002', score: 9.0, payload: { name: 'Old B', type: 'person' } },
            { id: '1003', score: 8.9, payload: { name: 'Old C', type: 'person' } },
          ] }),
        });
      }
      return route.fulfill({
        status: 200, contentType: 'application/json',
        body: JSON.stringify({ hits: [{ id: '1048', score: 1.5, payload: { name: 'New Hit', type: 'host' } }] }),
      });
    });
    await page.click('#vlRunBtn');
    await page.waitForTimeout(250);
    // 第一次请求未决时按钮是 disabled；直接从同一条代码路径发起第二次请求
    await page.evaluate(() => runVectorSearch());
    await page.waitForFunction(() => document.querySelectorAll('#vlResultBody tr').length === 1, null, { timeout: 10000 });
    if (releaseFirst) releaseFirst();
    await page.waitForTimeout(700);
    const rowsAfter = await page.$$eval('#vlResultBody tr', (rows) => rows.map((r) => r.querySelector('.vl-hit-name')?.textContent));
    expect(rowsAfter.length === 1 && rowsAfter[0] === 'New Hit',
      `旧响应不得覆盖新结果：${JSON.stringify(rowsAfter)}`);
    const status = await page.evaluate(() => ({
      rows: document.getElementById('statusRowCount').textContent,
      elapsed: document.getElementById('statusQueryTime').textContent,
      tab: document.querySelector('.tabs-bar .tab.active').dataset.tab,
    }));
    expect(status.tab === 'vectorLabPane', '异步返回不应切走工作区');
    expect(errors.length === 0, `向量异步 console/pageerror：${errors.join('; ')}`);
    await context.close();
  }

  // ---------- S10. 可访问性（axe-core 可用时） ----------
  {
    let axePath = null;
    try {
      const require = (await import('node:module')).createRequire(path.join(ROOT, 'noop.js'));
      axePath = require.resolve('axe-core');
    } catch (_) { axePath = null; }
    if (!axePath) {
      log('axe-core 未安装，跳过自动 a11y 扫描（不记为通过）');
    } else {
      const axeReport = {};
      for (const cfg of [{ w: 1440, h: 900, theme: 'dark' }, { w: 390, h: 844, theme: 'light' }]) {
        const { context, page, errors } = await newPage({ width: cfg.w, height: cfg.h, theme: cfg.theme });
        await page.addScriptTag({ path: axePath });
        const prefix = `${cfg.w}/${cfg.theme}`;
        const scan = async (label) => {
          const result = await page.evaluate(async () => {
            const r = await window.axe.run(document, {
              runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag22aa'] },
            });
            return r.violations.map((v) => ({
              id: v.id, impact: v.impact, nodes: v.nodes.length,
              targets: v.nodes.slice(0, 8).map((n) => n.target),
              summaries: v.nodes.slice(0, 3).map((n) => (n.failureSummary || '').slice(0, 220)),
            }));
          });
          axeReport[`${prefix}/${label}`] = result;
          const serious = result.filter((v) => v.impact === 'critical' || v.impact === 'serious');
          expect(serious.length === 0, `${prefix}/${label}: axe 严重违规 ${JSON.stringify(serious)}`);
        };
        await runQuery(page, 'FIND {type: "person"} RETURN * LIMIT 3');
        await scan('主工作区');
        await page.click('#gridBody tr[data-row-index="0"] .entity-name');
        await page.waitForTimeout(500);
        await scan('节点详情');
        await page.evaluate(() => collapseUnifiedPanel());
        await page.click('#openTutorialBtn');
        await page.waitForTimeout(350);
        await scan('教程');
        await page.keyboard.press('Escape');
        await page.waitForTimeout(250);
        await page.evaluate(() => openPalette());
        await page.waitForTimeout(300);
        await scan('命令面板');
        await page.evaluate(() => closePalette());
        await page.evaluate(() => openAiSettings());
        await page.waitForTimeout(300);
        await scan('AI 设置');
        await page.evaluate(() => closeAiSettings());
        await page.evaluate(() => openTxReview());
        await page.waitForTimeout(250);
        await scan('写入审阅');
        await page.evaluate(() => closeTxReview());
        expect(errors.length === 0, `${prefix}: a11y 场景 console/pageerror ${errors.join('; ')}`);
        await context.close();
      }
      writeFileSync(path.join(OUT, 'axe.json'), JSON.stringify(axeReport, null, 2));
    }
  }

  await browser.close();
  writeFileSync(path.join(OUT, 'report.json'), JSON.stringify({ checks, failures, base: BASE, browser: BROWSER }, null, 2));
  log(`${checks.length} checks, ${failures.length} failure(s), artifacts ${OUT}`);
  if (failures.length) {
    failures.forEach((f) => console.error(`FAIL ${f}`));
    process.exitCode = 1;
  }
} catch (err) {
  console.error(`[v41-r2] fatal: ${err && err.stack ? err.stack : err}`);
  process.exitCode = 1;
} finally {
  try { await browser.close(); } catch (_) {}
  if (serverProc) {
    try { serverProc.kill(); } catch (_) {}
    await sleep(600);
  }
  if (!EXTERNAL_BASE && existsSync(DB_DIR)) {
    try { rmSync(DB_DIR, { recursive: true, force: true }); } catch (_) {}
  }
}
