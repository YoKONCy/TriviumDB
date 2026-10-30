# nightly WebUI 开发诊断

Server 与 WebUI 处于 nightly 快速迭代阶段，布局、主题、交互和 HTTP 接口随开发调整。开发者按需要选择服务端自测、浏览器交互检查和截图记录。嵌入式 Core、Rust/Python/Node SDK 与 CLI 使用各自的正式验收流程。

WebUI 位于 `crates/triviumdb-server/web/index.html`，由 Server 通过 `include_str!` 内嵌，页面修改后重新构建 Server。当前文档对应 0.9.0 + V4.1 R2 工作区验收。

## 按需运行

| 工具 | 入口 | 用途 |
| --- | --- | --- |
| 服务端契约 | `cargo test -p triviumdb-server` | HTTP、TQL 与图探索 API |
| 页内自检 | `/ui?selftest=1` | `window.__selftestResults` 中的状态、解析、布局、流式与回归断言 |
| 离线浏览器检查 | `node scripts/test_webui.cjs` | 页面自测与离线交互 |
| 浏览器冒烟 | `scripts/ui-smoke.mjs` | 临时真实数据库、种子数据、图探索、AI 模拟、查询与抽屉；Chromium/WebKit 各执行一次 |
| V4 定向回归 | `scripts/webui-v4-regressions.mjs` | 真实 768 维候选/检索；TQL 与向量五态；两项 P2；390 教程/AI/面板焦点与 Esc；图探索顶部 hit-test（1440/1024/390 × 暗浅 × 查询舱/侧栏/统一面板）；桌面→手机断点生命周期（真实 GSAP，普通/reduced-motion）；390 向量参数标签/输入框遮挡；写入模式+只读查询不 500；DPR=2 位图 = CSS×2；触屏单指平移/双指捏合；长 payload 换行；结构/键盘（usp tablist、rail landmark、dock ↑↓/Enter、节点搜索 combobox、主/结果/panel tab ←/→ + roving）；390 结果表不横向溢出且滚动容器可聚焦；运行按钮宽度/图标稳定、历史点击高亮+聚焦、内置示例向量维度、查询舱默认 FIND、toast 去重、空闲不启动 aurora；390/900 搜索按钮真实点击+↑↓/Enter/Esc+焦点恢复；向量底栏与 TQL 分桶（加载/成功/零结果/失败/切回）；800 节点大图 Canvas fit 避让浮层（面板开关+窗口缩放）；非法 query-string 统一 problem JSON；教程说明块排版（非 pre-wrap、代码块无首尾空行）；Canvas 节点真实拖拽改变坐标且拖拽途中不弹面板、单击仍开详情；侧栏片段/导航不写查询历史；图工具条 1440/1024/390 自适应「更多」（不横向溢出、同一按钮不重复出现）；编辑器行号按软换行撑高、执行选中、取消运行（独立 canceled 状态）、表头排序（点击+Enter+aria-sort）、右键复制单元格、CSV 真实下载；对象树四组（类型/边标签/索引/向量）渲染、点行只填不执行、▶ 执行、索引创建与两步删除、节点/关系 CSV 导入落库与树刷新；列宽拖拽（排序后保持）、列显隐（表头与 CSV 同步）、双击编辑 payload（经 /v1/transactions 复核）、右键两步删除（Node 侧 404 复核）；批量选择（全选/半选、导出选中真实下载、批量删除后逐个 404 且未选中节点保留）；查询多标签（新建/切换恢复文本与结果/关闭）；事务模式（暂存不动库、回滚保留、提交后 404、TQL 写语句被拦）；侧栏对象树排版（组展开态/空组哑文字/向量胶囊/导入行）与查询历史弹层（元信息、分组切换、搜索、填入、删除、清空、Esc） |
| WebGL 取景回归（真实 Sigma） | `scripts/webui-graph-fit.mjs` | 真实 CDN 加载 Sigma；1440/1024/390 × DPR 1/2 × 面板/抽屉开关；按钮 fit、后端切换、窗口缩放后节点/标签不压 HUD/查询舱/布局条/dock/统一面板，且不越出画布。Sigma 不可用直接失败，不用 Canvas 冒充 |
| V4 组合矩阵 | `scripts/webui-v4-combos.mjs` | 3 视口 × 暗浅 × zh/en × TQL/向量 × 5 状态 = 120 组合/引擎；状态标记、横向溢出、关键按钮边界、rail/结果区遮挡 |
| V4.1 R2 验收 | `scripts/webui-v41-r2-checks.mjs` | 真实 8 维库；3 视口 × 暗浅 × zh/en 外壳组合与关键控件中心点 hit-test；侧栏真实拖动/键盘/偏好/rail/断点往返；语言切换保留查询·结果·节点·向量输入；标量/聚合/多列/null 结果形状与导出一致性；对象树自定义/缺失类型与样本口径；跨工作区当前节点（表格→向量来源卡→命中动作→图定位）；48 节点图的收敛半径/标签预算/光晕与取景避让；写入审阅暂存/差异/失败保留/提交/放弃；向量旧响应隔离；axe-core 可用时自动扫描 |
| 视觉回归 | `scripts/visual-baseline.mjs` | 固定视口、确定性布局和平台对应 PNG 基线的实际像素比对；`--record-only` / `--compare` / `--update` 显式选择记录、比对或更新 |

浏览器工具的安装和运行：

```bash
npm i --no-save --ignore-scripts playwright@1.64.1 pixelmatch@8.0.0 pngjs@7.0.0 gsap@3.13.0
npx playwright install chromium webkit
cargo build -p triviumdb-server --target-dir target/webui-tests

node scripts/test_webui.cjs
node scripts/ui-smoke.mjs
SMOKE_BROWSER=webkit node scripts/ui-smoke.mjs
WEBUI_BROWSER=chromium node scripts/webui-v4-regressions.mjs
WEBUI_BROWSER=webkit node scripts/webui-v4-regressions.mjs
WEBUI_BROWSER=chromium node scripts/webui-v4-combos.mjs
WEBUI_BROWSER=webkit node scripts/webui-v4-combos.mjs
WEBUI_BROWSER=chromium node scripts/webui-v41-r2-checks.mjs
WEBUI_BROWSER=webkit node scripts/webui-v41-r2-checks.mjs
node scripts/webui-graph-fit.mjs
WEBUI_BROWSER=webkit node scripts/webui-graph-fit.mjs
node scripts/visual-baseline.mjs
```

PowerShell 使用 `$env:SMOKE_BROWSER='webkit'` / `$env:WEBUI_BROWSER='webkit'` 设置引擎，运行后移除该环境变量。脚本默认使用独立构建目录 `target/webui-tests`，也可通过 `CARGO_TARGET_DIR` 指定目录。依赖缺失时脚本返回失败，不能把没有执行的检查算作通过。

V4 回归脚本自建临时 768 维数据库，通过 `/v1/import/nodes` 写入非零向量，真实执行节点来源与 JSON 来源检索；候选列表使用真实节点，不以 fixture 代替。`webui-v4-regressions.mjs` 还覆盖 Chromium 失焦选中态、浅色主按钮无扫光、普通/reduced-motion 的悬停进入/离开采样。

断点用例必须加载真实 GSAP：脚本把页面里的 `https://cdn.jsdelivr.net/npm/gsap@3.13.0/dist/gsap.min.js` 路由到本地固定文件 `node_modules/gsap/dist/gsap.min.js`，其余外网 CDN 仍阻断。缺依赖直接失败，不用空 stub 代替——要验证的正是真实首屏动画留下的 inline transform。hit-test 用例用 `elementFromPoint(中心点)` 判断控件是否被压住，并按矩形相交检查 HUD / 查询舱 / 视图工具条；1024/390 下按既有设计隐藏的辅助入口记为 hidden，不做“必须可见”的伪断言。

过滤口径保持最小：只放过测试自己阻断的外部 CDN `ERR_FAILED`、错误态用例中成对开启的 400/500 资源错误，以及 WebKit 的 `ResizeObserver loop completed with undelivered notifications` 浏览器警告（已用修复前/后两份构建分别复现，确属既有噪声；记录进 `report.json` 的 `benignPageErrors`）。其余 console/pageerror 一律失败。`webui-graph-fit.mjs` 在 WebKit 下还会遇到多轮重建 Sigma 实例导致的 `There are too many active WebGL contexts` 与 `loseContext: context already lost`（Chromium 干净）；只对这两条做精确匹配，记入该脚本报告的 `benign`，不宣称 console 全零。

内嵌页面按内容指纹返回弱 ETag（`If-None-Match` → 304）并启用 gzip；`If-None-Match` 按 RFC 9110 §13.1.2 做弱比较——`W/"tag"`、`"tag"`、标签列表（含引号内逗号）与 `*` 都必须命中 304，不合法标签不命中；304 为空体，带 `ETag` 与 `Cache-Control: no-cache`，且压缩层对 304 不再附加 `Content-Encoding`。契约测试 `webui支持etag304与gzip压缩`、`if_none_match执行弱比较并保留304缓存头` 与 `读语句带mutation标志走读路径而不是500` 钉住行为。查询参数反序列化失败统一返回 problem JSON（`code=INVALID_REQUEST`、双语 `detail` 指明参数名），不再泄漏 axum 的 text/plain；契约测试 `非法查询参数返回统一problem_json而不是text_plain` 覆盖 pagination/direction/depth/hops 与向量 `top_k`。渲染回归额外覆盖：DPR=2 时画布位图必须等于 CSS×DPR；触屏单指平移/双指捏合（合成 PointerEvent）；抽屉长 payload 不横向裁切。

无障碍审计：本地用 `npm i --no-save --ignore-scripts axe-core@4.11.0`，在 1440/390 × 明暗 × 查询/图探索/向量/AI/详情/过滤器/命令面板/教程执行 `axe.run`（WCAG 2.0/2.1/2.2 AA + best-practice）。当前 0 violations。CI 不安装 axe；回归脚本把关键结构/键盘行为固化成断言（tablist 子节点、rail landmark、dock ↑↓/Enter、搜索 combobox、panel tab ←/→、390 结果表适配），避免审计工具成为运行期依赖。


## 写入与查询标签的边界回归

`webui-v4-regressions.mjs` 的 G21 在真实临时 Server 上验证三类边界：

- 用两条相邻的大整数 ID 记录核对编辑、单删和批删在发送或暂存前拒绝，完整节点与向量保持原样；CSV/JSONL 的节点和关系导入同样验证安全整数限制。
- 用受控响应屏障覆盖成功与失败提交：提交期间新增的编辑仍排队，下一次提交/重试后分别读取具体节点核对 Payload，不用固定 sleep 证明完成。
- JSON 与 NDJSON 分别覆盖切换和关闭标签：在真实响应取得后延迟交付，即使响应迟到，当前编辑器、结果、标签快照和按钮状态也不能被旧查询覆盖。切换/关闭会取消当前未完成请求，切回标签不会复活已取消结果。取消终止客户端等待，不承诺回滚已经在服务端提交的写入；写请求中断后须重新查询确认状态。

可在 PowerShell 单独运行这些边界，以便快速复现；默认命令仍执行完整套件：

```powershell
$env:WEBUI_BOUNDARIES_ONLY='1'
node scripts/webui-v4-regressions.mjs
$env:WEBUI_BROWSER='webkit'
node scripts/webui-v4-regressions.mjs
Remove-Item Env:WEBUI_BOUNDARIES_ONLY
Remove-Item Env:WEBUI_BROWSER
```

单独运行报告的 `scope` 为 `write-boundaries`，不能将其检查数量当作完整 V4 或视觉验收。

## 可选的截图比对

设计调整时可以直接记录新界面，需要对照某个参考设计时再显式选择比对或更新：

```bash
node scripts/visual-baseline.mjs --record-only
node scripts/visual-baseline.mjs --compare
node scripts/visual-baseline.mjs --update
node scripts/visual-baseline.mjs --with-sigma
```

参考图按平台放在 `tests/visual/baseline/<platform>/`。截图写入 `.tmp/visual-shots/`，显式比对产生的差异图写入 `.tmp/visual-diff/`。现有参考图用于开发对照，随设计演进维护或替换。

## 手动 workflow

Actions 中的 **Nightly WebUI Diagnostics** 通过 `workflow_dispatch` 手动运行。默认记录截图；`run_smoke` 选择是否执行当前页面的交互自测（页内自检 + ui-smoke + V4 定向回归 + 120 组合矩阵，双引擎），`visual_mode` 选择记录、比对或更新参考图。产物随该次诊断上传，V4 两个套件的日志与报告同样作为 artifact 保留。

手动诊断在 Windows Runner 上先执行 Server 契约，再执行 Chromium/WebKit 的冒烟、V4 回归和组合矩阵。上述步骤不再使用 `continue-on-error`；任何断言失败都会使本次诊断失败。该配置保持与嵌入式发布门禁独立，不代表 Linux 浏览器视觉度量已验收。

常规 PR/push 与嵌入式 beta/stable 发布执行 Core、SDK 和 CLI 验收。Server 使用独立 workflow 与按需自测入口。界面重做时，开发者同步调整相关诊断脚本即可。

脚本创建独立临时数据库，仅清理自身创建的子进程与文件。

workflow 无论成功或失败都会上传截图与新基线；基线变更须说明原因，并实际查看截图，不能通过自动覆盖基线掩盖布局或语义回归。

V4 批次基线说明（2026-10-29）：9 张 win32 基线全部按 V4 表面重录（含新增 `07-light-cockpit`、`08-light-dock`），重录前逐张查看旧图/新图/差异图；重录后连续两次普通比对差异 ≤ 0.05%。阈值仍为 1.5%，未使用 `--record-only` 冒充通过。

## 发布前要求

WebUI 专项验证补充而不替代 `CONTRIBUTING.md` 的全工作区、Clippy、hook 套件和绑定检查。平台限制应在验收记录中如实说明，不得用某台机器的环境限制豁免全项目门禁。完整证据绑定具体提交，候选阶段不打正式 tag；正式 tag 只创建在通过验收的 master 发布提交上。
