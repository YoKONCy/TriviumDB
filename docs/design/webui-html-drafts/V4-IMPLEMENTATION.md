# V4 工作区视觉统一：开发接续

用户已认可 V4 设计方向。V4-A/B/C 已在工作分支实施（fixture 数据不作为真实检索或性能证据）。

## 从这里开始

1. 阅读同目录 `V2-MAPPING.md`，尤其 §4 的不可重命名/删除测试锚点。
2. 按 V4-A → V4-B → V4-C 的顺序阅读本文的实施记录与验收口径。
3. 需要对照可交互原型时，使用开发机本地副本（见下节），仓库只保留文档。

## 原型位置（不随仓库分发）

可交互蓝本 `v4-workspace-unification.html`、`v2-hybrid-explorer.html`、`warm-stone-light.html` 等
是带内联样式/脚本的设计原型，按仓库规范（`docs/` 只放文档）不纳入 Git，开发机本地存放在
`.tmp/design-prototypes/`；原型预览脚本同样只存在于本地 `.tmp/`。
原型仅作视觉/结构参考：其中的数据、分数与响应均为 fixture，不构成真实检索或性能证据。

## V4 实施范围

- **V4-A：基础材质与操作角色。** 暗色工作面从旧暖黑统一到冷色家族，浅色保留暖石。新增 action/on-action，成功色只表达成功状态；明确 data-theme 对应的 color-scheme。
- **V4-B：TQL 与向量工作区。** 编辑器、toolbar、结果表格/JSON、向量摘要/配置/结果区统一层级；修正空态字体、按钮强调关系与原生选择列表。
- **V4-C：状态与回归。** 检查未运行、零结果、加载、有结果、错误、选中、禁用、聚焦；审计 token 对图浮层、详情、对话与弹窗的影响。定稿后统一更新受影响的视觉基线。

先审计 token 的继承范围，再改组件；不要仅给截图红框加末尾覆盖规则。
参考稿中的数据、分数与响应均为 fixture，不是真实检索结果。
原型未完整模拟产品编辑器、图算法、表格虚拟化或服务器校验，实施时保留现有行为。

## V3 已完成的产品基点

产品代码基点是 `27175a9`，已经推送：

- `c57f29e`：窄屏保留加载全图/TQL 入图入口。
- `e5d2857`：小图力学、屏幕空间尺寸、命中与缩放边界，新增 `scripts/graph-scale-checks.mjs`。
- `27175a9`：刷新全部 9 张 win32 基线（含浅色 cockpit/dock），修正冒烟测试快照 ID。

V3-B 既有验收：9 节点星型覆盖率 54.76%，最近点对 147.81px；原种子拓扑覆盖率 52.63%，最近距离 211.09px。
normal 标签在 k=1/3/20 均为 11px；pill/边为 10px。
小于 60 节点的力学按规模调节，60+ 保留旧参数；fit 上限 3，手动倍率范围 0.05–20。
新 V4 不应重新修改这些渲染逻辑。

产品既有验证结果（2026-10-25 Windows）：cargo 11 单元 + 29 契约；selftest 136/136；ui-smoke 通过；9 张视觉基线复验最大差异 0.02%。
1009 节点自动暂停力学，可开详情；动效开启时 glow 为 15/6/0。
已知旧问题：动效关闭时 glow 是 18/8/8，V3 未改该函数。密集图标签优先级、自动面板相机跟随、分数命名和 FPS 表述另列，不混入 V4 视觉范围。

## 验证与本机依赖

V4 原型已经过 1440/1024/390 × 暗浅 × zh/en × 两页 × 五状态共 120 组合检查，无页面横向溢出或脚本错误，并完成代表性截图复核。
原型对比度初筛：主按钮最低 4.62:1，次级文字 6.07:1，输入轮廓 3.34:1；不等同于产品最终可访问性验收。

原型侧的 120 组合校验脚本（`v4-reference-checks`）与原型一起只存在于本地 `.tmp/`，不纳入仓库；
产品侧对应的可复跑入口是 `scripts/webui-v4-combos.mjs`（120 组合矩阵）。
这类原型检查需要可解析的 Playwright 包和已安装的 Chromium，复用仓库现有 UI 测试环境。

产品实施后再跑：

```powershell
cargo test -p triviumdb-server
bun scripts/ui-smoke.mjs
bun scripts/visual-baseline.mjs
```

`scripts/graph-scale-checks.mjs` 需另有已重建、正在运行的产品 server（默认 8080，可用 TDB_BASE 指定）。
历史 P1/light/detail 实机脚本位于旧电脑 Temp/pw-env，未纳入此提交；新电脑不能假定存在这些路径，可依据 V4 验收清单补齐同等实机检查。

Windows 上运行中的 triviumdb-server.exe 会锁住构建输出，cargo build/test 前先停止该进程。
产品 HTML 经 include_str! 嵌入，修改后必须重新 cargo build 才能实测新界面。
CARGO_TARGET_DIR 是本机缓存设置，应在新电脑上自行选择，不依赖旧电脑绝对路径。

## 2026-10-26 实施进度

- V4-A：已落地暗色冷材质、浅色暖石、action/on-action、危险操作、color-scheme、原生 listbox option、语法 token 与成功色收口。
- V4-B：TQL 结果五态（idle/running/success/zero/error）、16px 无衬线空态、结果 tab/表头/选中行层级；向量摘要/source tabs/inset 输入/候选列表/参数/运行/结果五态、768 维 JSON 与真实检索。
- V4-C：390 主 tab 内部滚动与当前项自动可见；统一面板 rail 窄屏底部胶囊、工作区净空、面板全宽；教程单列/横向步骤导航/焦点陷阱/Esc；AI 设置两列与焦点/Esc；关键按钮吸底。
- 两项 P2：Chromium 候选失焦选中态（`--active` + 600 字重）与浅色主按钮扫光移除，均由 `scripts/webui-v4-regressions.mjs` 覆盖。
- 验证入口：`scripts/webui-v4-regressions.mjs`、`scripts/webui-v4-combos.mjs`（120 组合/引擎）、`scripts/ui-smoke.mjs`、`scripts/visual-baseline.mjs`。
- 视觉基线：六张因 V4-B/C 有明确依据更新；05/07/08 保持原基线，阈值仍为 1.5%。

## 2026-10-27 预审修复（三项 P2 + 一处分行塌陷）

- 发现 1（图探索顶部遮挡）：`.v2-hud.left` 与 `.v2-cockpit` 原同为 `top:12px`，1440 下 HUD 的 k-hop/路径按钮中心命中查询舱开关。现固定为「HUD 第一行 / 查询舱第二行」，视图工具条留在第一行右侧；`max-width:1280px` 不再把 HUD 下推。701–1280 两行、≤700 三行（工具条 8px / HUD 56px / 查询舱 96px），因为 390 下 HUD 与工具条并排必然相交。
- 同一处顺带修复：统一面板打开时 `calc(100% - 916px)` 在 1024 算出负宽度，查询舱被压成 2px。现改为锚定面板左侧（`calc((100% - 376px)/2)`，与既有 376px 让位基准一致），面板打开时顶部三行让位，1024/1440 均保持 360/560px 可用宽度。
- 发现 2（桌面→手机侧栏）：GSAP 首屏动画在 `aside.sidebar` 留下 `transform: translate(0,0)`，跨到 ≤900 后覆盖 CSS 的 `translateX(-100%)`。现于断点监听里 `killTweensOf` + `clearProps`（transform/opacity/visibility/translate/rotate/scale，同时清理侧栏导航项），不用 `!important` 硬压，reduced-motion 路径不受影响，桌面动画保留。
- 发现 3（390 向量参数遮挡）：`.vl-config` 改为「滚动区 + 真实 footer」。新增 `.vl-config-scroll` 承载配置项滚动，`#vlRunBtn` 退出 `sticky` 成为普通 flex 子项；原 ID、事件、`data-result-state` 不变，桌面视觉不变。
- 回归：`webui-v4-regressions.mjs` 新增三组用例（图探索 hit-test 1440/1024/390 × 暗浅 × 查询舱/侧栏/统一面板；桌面→手机→桌面断点生命周期，真实 GSAP，普通/reduced-motion；390 向量参数标签/输入框遮挡，真实 768 维 server）。GSAP 用 `gsap@3.13.0` 本地固定文件路由，空 stub 不可接受；WebKit 的 ResizeObserver 警告单独记录为既有噪声，其余 pageerror 仍失败。
- 视觉基线：07-light-cockpit / 08-light-dock 因查询舱整体下移 44px（节点随之重排）更新，旧基线留档于 `.tmp/v4-baseline-old/`；其余 7 张不变。
- 本机结果：定向回归 Chromium 532 检查/0 失败、WebKit 532/0；页内自检 136/136（双引擎）；120 组合矩阵 900 检查/0 失败（双引擎）；视觉 9/9。`05-panel-filter` 未改基线，差异 1.43% 在阈值内。

## 2026-10-27 第二轮修复（渲染/后端五项）

- **写入模式 500**：服务端不再把 `mutation=true` 当作强制写路径——先用 `parse_tql_statement` 判定，读语句回落到读查询（`lib.rs` tql）；前端 `isMutation = 勾选 && !isReadOnlyTql(query)`，读语句即使勾选也不会被打成写请求；写语句未勾选时错误页补「请勾选写入模式」提示（zh/en）。
- **HiDPI**：`syncCanvasSize` 位图改为 `CSS 尺寸 × devicePixelRatio`，记录 `__wlsCssW/H/Dpr`；`drawGraphScene` 开头 `setTransform(dpr)`；相机、fit、物理边界、飞行动画、命中全部改用 CSS 逻辑尺寸（新增 `canvasCssSize` 纯读取）。DPR=2 实测位图 2352×1536（CSS 1176×768）。
- **触屏**：两张图画布改用 Pointer Events，新增 `touch-action:none`；单指平移/拖节点、双指捏合缩放（`pinchTransform` 纯函数，中点锚定）；独立探索画布同样支持。
- **长文本**：`.jt-line > span` 允许 `overflow-wrap:anywhere`，长 URL/hash 在抽屉内换行不再被 `overflow:hidden` 裁掉；普通标签模式按 32 字符截断（pill 18、边标签 24，`truncateGraphLabel` 纯函数）；抽屉标题单行省略 + title 全文。
- **`/ui` 传输**：启用 `tower-http` gzip（`CompressionLayer` 最外层），内嵌页面按内容指纹生成弱 ETag，`If-None-Match` 命中返回 304；`no-cache` 保留。
- 回归：定向回归新增 4 组（写入模式只读、DPR=2 位图、触屏平移/捏合、长 payload），合计 547 检查/0 失败（双引擎）；页内自检 136→139；视觉 9/9 不变；`graph-scale` 增加「位图 = CSS×DPR」断言。
- 本机结果：定向回归 Chromium 547/0、WebKit 547/0；selftest 139/139（双引擎）；组合矩阵 900/0（双引擎）；视觉 9/9；`cargo test -p triviumdb-server` 31/0（含 ETag/304、gzip、读语句回落三条新契约）。

## 2026-10-27 第三轮：前端可访问性 / 键盘加固

axe-core 4.11 扫描 26 个状态（1440/390 × 明暗 × 查询/图探索/向量/AI/详情/过滤器/命令面板/教程），修复前有 2 类 critical（aria-required-children / aria-required-parent）、4 类 serious（对比度 / target-size / scrollable-region-focusable / 教程按钮无名），修复后 **0 violations**（含 WCAG 2.2 AA target-size）。

- **ARIA 结构**：`.usp-tabs` 内层新增 `#uspTablist[role=tablist]`，收起按钮移出 tablist；`#uspRail` 恢复 `<nav>` landmark，三个按钮包进 `#uspRailTablist[role=tablist][aria-orientation=vertical]`；`#aiChatDrawer` 由 complementary 改 region；补 `h1`（视觉隐藏）与 `h2` 层级（抽屉/教程标题）。
- **键盘**：图结果表行支持 roving tabindex + ↑↓/Home/End + Enter/Space 打开详情；节点搜索补 `role=combobox`/`aria-expanded`/`aria-activedescendant` 与 ↑↓/Home/End/Enter；主工作台标签、结果视图标签、统一面板标签三组均补 ←/→/Home/End + roving tabindex。
- **对比度**（浅色）：`--text-light` `#6f6a5f` → `#625c51`（5.22:1），`--success` `#4f6b3a` → `#486231`（4.96:1），`--warning` `#8a6d3b` → `#7c6032`（4.95:1）。
- **触控/移动**：过滤器每行最小 32px；结果表滚动容器可聚焦；390 向量结果表改 `table-layout:fixed` + payload 换行，不再横向裁切；rail 在 ≤700 保持底部横排胶囊。
- **其他**：toast 容器补 `role=status/aria-live=polite`；✕ 按钮补「关闭…」可访问名；QuIVer 状态枚举本地化（未预热/预热中/就绪…）；`body` 高度补 `100dvh`。
- 回归：`webui-v4-regressions.mjs` 新增结构/键盘/390 表格三组（547→566 检查）；selftest 139 保持（主标签断言改为 roving 语义）；组合矩阵 900/0、视觉 9/9、graph-scale 通过（双引擎）。

## 2026-10-27 第四轮：交互问题修复

- **运行按钮**：不再在运行中替换 `innerHTML`（原先会丢掉播放图标、改变按钮宽度，造成左跳/闪烁）；改为固定结构 + `is-running` 态，左侧图标原位显示 spinner，`aria-busy` 同步；去掉主按钮 hover 扫光与按压缩放，图标不再做 hover/active 位移；同文案 toast 去重并限制最多 3 条。
- **内置向量示例**：侧栏「向量与 QuIVer」与教程第 4 步不再写死 4 维示例，改为按当前库维度（默认 768）生成完整示例向量；查询舱默认从 4 维 VECTOR 改为 `FIND {type: "person"} RETURN * LIMIT 10`（任何库都有效），输入 `SEARCH VECTOR` 时才显示阈值滑杆；向量 JSON 输入占位与空态文案去掉 4 维误导。
- **查询历史**：点击后写入 `.active`/`aria-current` 并立即聚焦；补 `enableKeyboardActivation`；查询完成后 `renderHistory` 重建仍按 `activeHistoryQuery` 保持高亮。
- **星云背景**：星点由黄金比例序列改为固定种子 PRNG 的 220 颗随机星（大小/亮度/颜色/十字星芒不同），星云从 2 团增至 3 团（青绿/紫罗兰/暖金）+ 四周暗角；确定性生成、按画布尺寸缓存，不引入随机帧抖动。
- **拓扑抖动**：节点光晕去掉按 `auroraPhase` 的正弦呼吸（固定 10/18px），空闲不再以 15fps 重绘；`auroraStep` 只在存在脉冲环时运行，最后一帧擦除残留；重合节点的分离改用确定性角度，不再 `Math.random()`。
- **品牌**：移除侧栏「星尘工作台」副标题（中英字典同步删除）。
- 回归：`webui-v4-regressions.mjs` 新增 G8（运行按钮宽度/图标稳定、历史 active+focus、示例向量维度 >100、查询舱默认 FIND 可运行、toast 去重、无脉冲不启动 aurora），547→575 检查；selftest 139、组合矩阵 900/0、视觉 9/9（无基线更新，差异均 <1.5%）、graph-scale 通过。

## 2026-10-27 第五轮：窄屏搜索 / 底栏统计 / WebGL 取景 / HTTP 边界

五项缺陷均为既有行为，本轮按"先复现、再最小修正、用真实引擎复验"处理：

- **窄屏节点搜索入口**：`≤900px` 原先把 `.node-search-wrap` 整块隐藏，HUD 的「图内搜索」在 `≤1280` 也已隐藏，手机上没有任何可用入口。现新增 `#nodeSearchToggle`（仅 `≤900` 显示），点击把**同一个** `#nodeSearchInput` 展开为 tab 行下方浮层；复用原 combobox 的 ↑↓/Home/End/Enter 与 `aria-activedescendant`，Esc 收起并回焦到按钮，命中结果仍走 `flyToNode`。不新增第二套状态或重复 id。
- **向量底栏统计**：`runVectorSearch` 只更新 `elapsed`，`rows` 仍保留上一次 TQL 的结果（复现：先跑 11 行 TQL，再向量 top_k=3，底部显示"返回 11 行 + 向量耗时"）。现按工作区分桶：`statusState`（TQL）与 `vectorStatusState`（向量）各自保存，`renderStatusBar` 取当前 tab 的桶；向量侧在请求前清空、成功写 `hits.length`、失败保持 `—`，切回 TQL 仍显示其原始行数与耗时。写入的 created/affected 语义未动。
- **WebGL 取景避让**：Sigma 旧 fit 只用 `bbox × 1.35`，完全不知道 HUD/查询舱/布局条/dock/统一面板。现 `graphFitArea` 按画布内**实际浮层矩形**（按"最近画布边"归属顶/底/左/右）求可用区，并预留节点半径与标签（Sigma 标签在右、Canvas 标签在下）；`sigmaFitTarget` 用 Sigma 自己的 `graphToViewport` 量出当前"像素/图单位"，再反算 `ratio` 与相机中心，不改布局算法、坐标、缩放上下限与命中。另修：WebGL 下画布被 Sigma 覆盖、不再逐帧重绘，窗口缩放后 `__wlsCssW/H` 不更新，fit 会用旧尺寸——取景前先 `syncCanvasSize`。
- **If-None-Match 弱比较**：服务端 ETag 为 `W/"…"`，原实现只做整串相等，回传强形式 `"…"` 会错误 200。现按 RFC 9110 §13.1.2 忽略 `W/` 前缀比较 opaque-tag，支持标签列表（含引号内逗号）与 `*`，非法标签不命中；304 补 `Cache-Control: no-cache`，并让压缩谓词跳过 304（此前 gzip 请求的 304 会被包成 20 字节 gzip 帧并带 `Content-Encoding`）。
- **非法 query-string**：`Query<T>` 的 axum 默认拒绝是 `text/plain`（`Failed to deserialize query string: …`）。现 `ApiQuery<T>` 把拒绝映射为既有 `ApiError`（400 / `INVALID_REQUEST` / problem JSON / 双语 detail 指明参数），覆盖 edges/neighbors/paths 与向量 `top_k`；合法参数与原有业务校验（如 depth 1..=3）语义不变。

回归：`webui-v4-regressions.mjs` 675 → 766 检查（Chromium/WebKit 各 766/0），新增 390/900 搜索按钮真实点击与键盘、向量底栏五态分桶、80 节点 Canvas fit 避让（面板开关/窗口缩放）、非法 query-string problem JSON；新增 `webui-graph-fit.mjs`（真实 CDN Sigma，1440/1024/390 × DPR 1/2 × 面板/抽屉 + 后端切换 + 窗口缩放，71 检查/引擎）。页内自检 139、组合矩阵 900/0、视觉 9/9（阈值仍 1.5%，最大差异 0.84%）、graph-scale 通过。HTTP 改动后 Rust 全量门禁需针对最终版本重跑。

## 2026-10-28 第六轮：教程排版 / 节点拖拽 / 侧栏语义 / 工具条自适应

四项均来自实际使用反馈，先复现再改：

- **教程文字排版**：`.tutorial-code-block` 用 `white-space: pre-wrap`，第 1 步的「教学任务」说明块直接写在里面，模板字符串的 10 个缩进空格被当成正文渲染——首行缩进一个字、第二行只剩「数据！」。现在说明块改用 `.tutorial-note`（普通文本排版，`strong` 独立成行）；代码块内容改为紧贴标签书写、多行用真实换行，不再有首尾空行。
- **节点拖拽**：① 独立探索画布的 pointer 逻辑只做了「点击开抽屉 + 空白拖拽平移」，**根本没有节点拖拽**；现在与查询画布一致：命中节点后按 `standaloneTransform` 更新世界坐标，写回 `baseX/baseY`，拖拽途中不弹面板。② WebGL 用的是 Sigma 2.4，它只内建相机拖拽（拖节点实际是平移整张图），且共享坐标每帧回写会把位置弹回去；新增 `setupSigmaNodeDrag()`：document 捕获阶段命中节点即接管（`getNodeAtPosition`），`viewportToGraph` 反算、`mapFn.toWorld` 写回，拖拽期间 `syncSigmaPositions` 跳过该节点；单击仍打开详情（`clickNode` 用 350ms 守卫去重）。③ 顺带修正 **WebGL 与 Canvas 上下镜像**：Sigma 归一化坐标 y 轴朝上，原 `mapFn` 直接映射世界 y（朝下），切换后端整张图上下翻转、拖拽方向也相反；现在 `mapFn` 翻 y 并在 `toWorld` 里反算。
- **侧栏语义**：`[data-snippet]` 原来点击就执行，用户只是看模板却平白多一条查询历史；现在片段只填入编辑器并提示 Ctrl+Enter 运行（命令面板仍保留「执行」语义）。「对象导航」三项改为纯工作区切换（文档 / 图探索 / 向量），不再自动跑查询；空图时给可执行提示。片段分区补一行用途说明。
- **工具条「更多」自适应**：原来固定把导出 PNG / 全屏 / 过滤器 / 命名空间等塞进「更多」，1440 下明明有空间也不显示。现在按优先级尝试铺满：先全部放进工具条，只有 `scrollWidth > clientWidth` 才从末位逐项移入「更多」，一项没溢出就隐藏「更多」按钮。窗口缩放、切换标签、切语言、开关统一面板都会重排；窄屏保留横向滚动兜底。

回归：定向回归 766 → **794** 检查（新增教程排版、Canvas 节点拖拽与单击、侧栏不写历史、工具条三档不溢出且无重复）；`webui-graph-fit.mjs` 71 → **76** 检查（新增 WebGL 拖拽写回、后端往返保持、y 轴朝向一致）。页内自检 139 → **140**（侧栏片段不执行/不写历史）。视觉基线因工具条铺开而更新 9 张 win32 基线，阈值仍 1.5%，更新后差异 ≤0.02%。

## 2026-10-28 第七轮：编辑器生产力 + 结果表格基础操作

对照 Navicat 类客户端补「改/找/拿」三件事里的第一批（不涉及服务端契约）：

- **编辑器行号**：`.editor-container` 左侧新增 46px 行号槽（`#tqlGutter`），`#tqlHighlight` 与 textarea 的 `padding-left` 同步从 16→60，保证叠加层逐字对齐不变。编辑器是软换行，所以行号不能按「一行 21px」堆叠：隐藏测量层 `#tqlMeasure` 把每条逻辑行包成 `<span>`，用 `getClientRects().length` 得到折行后的视觉行数，再按 `rows × 21px` 撑高对应行号；滚动时整槽 `translateY(-scrollTop)` 跟随。
- **执行选中**：`getExecutableQuery()` —— 有选区只跑选区（Ctrl+Enter 与「运行」按钮同源），无选区跑全文；历史/写入模式推断仍按实际执行文本。
- **取消运行**：运行中显示「取消」按钮，`AbortController.abort()` 后进入独立的 `canceled` 结果状态（不是错误、不是空结果），状态栏行数/耗时归零，按钮与 `is-running` 由 `finally` 恢复。主动取消与新查询中止旧查询分离（后者静默）。
- **列排序**：表头（`#` 除外）可点击/Enter 排序，稳定排序、升/降序循环，`aria-sort` 同步；排序只影响当前结果视图，不重发查询；新查询自动重置；流式追加期间明确拒绝（避免追加顺序与排序打架）。排序后重渲染表头会把焦点还给同一列，键盘操作不掉焦点。
- **复制**：点击单元格记录复制目标；`Ctrl/Cmd+C`（焦点不在输入框、无文本选区时）复制单元格；右键菜单提供「复制单元格 / 复制整行（TSV）/ 复制为 JSON」，剪贴板走 `navigator.clipboard` + `execCommand` 兜底，成功/失败都有 toast。
- **CSV 导出**：新增「导出 CSV」，按**当前表格视图**（列顺序 + 排序）生成，UTF-8 BOM、CRLF、逗号/引号/换行按 RFC 4180 转义；无数据时明确提示而不是导出空文件。

回归：定向回归 794 → **808**（新增行号（含软换行高度）、执行选中、取消、表头排序（点击+Enter+aria）、右键复制、CSV 下载事件）；页内自检 140 → **145**（行号视觉行/滚动同步、执行选中、排序三态、CSV 转义、复制三种目标）。视觉 9/9 不变（现有基线不覆盖查询编辑器，差异 ≤0.02%）。rust：fmt 0 / clippy 0 / `cargo test -p triviumdb-server` 47/0。

## 2026-10-28 第八轮：对象树 + 导入向导

补「找不到数据 / 导不进数据」两件事，全部复用既有 HTTP 契约（无服务端改动）：

- **对象树（侧栏「数据对象」）**：把原来只有三个入口的导航补成可浏览的树，四组：
  · **类型**：来源是一次有界采样 `MATCH (a) RETURN a LIMIT 200`，按 `payload.type` 计数；`rowCount` 命中上限时标注「样本 N」，否则标注「N 个节点」——不把样本数伪装成总数。
  · **边标签**：同一次采样里各节点的出边 `label` 计数（同样标注样本口径）。
  · **索引**：`GET /v1/indexes` 的真实字段（kind/fields/entry_count）；删除走 `DELETE /v1/indexes/delete`，因为内嵌 webview 禁用原生 `confirm`，改为「✕ → ✓ 3 秒内二次点击」确认。
  · **向量**：维度 + `GET /v1/indexes/quiver` 状态，行尾 ▶ 触发 `POST /v1/indexes/quiver` 构建。
  交互与侧栏片段一致：点行只**填入** `FIND {type: "…"}` / `MATCH (a)-[:LABEL]->(b)` 并切到查询台，行尾 ▶ 才执行——不悄悄写查询历史。非法标签标识符退回通用 MATCH，类型值里的引号做转义。
- **导入向导（侧栏「导入数据…」）**：模态框，节点/关系两个 tab。
  · 文件或粘贴（文件优先），自动识别 JSONL（首行 `{`）与 CSV（RFC 4180：引号包裹、`""` 转义、引号内换行、CRLF）。
  · CSV 节点：`id` / `vector` 为保留列，其余列写入 payload；`vector` 列存在但解析失败 → 按行报错（不静默补零），只有**空单元格/缺列**才在勾选「缺少向量时用零向量补齐」时按库维度补零；`id` 缺省交给服务端分配。
  · 关系 CSV：`source` / `target` / `label` / `weight`，空 id 必须报错（`Number('') === 0` 会把空值当成节点 0）。
  · 预览前 6 行 + 「识别为 CSV/JSONL：N 行 / M 列」「payload 字段」「解析失败行」；超过服务端 10000 条上限时直接禁用导入按钮。
  · 提交 NDJSON 到 `/v1/import/nodes|edges`，成功显示条数与代次并刷新对象树，失败显示服务端 `detail`。

回归：定向回归 808 → **820**（对象树四组渲染、点行只填不写历史、▶ 执行、索引创建→两次点击删除、节点导入预览→落库→树刷新出新类型、关系导入→`/v1/nodes/{id}/edges` 复核、Esc 关闭）；页内自检 145 → **151**（采样归纳、查询生成转义、树渲染、CSV/JSONL 解析、节点映射与坏向量报错、关系映射）。视觉 9/9（侧栏新增对象树，基线已按新 UI 更新，复验差异 ≤0.02%）。rust：fmt 0 / clippy 0 / `cargo test -p triviumdb-server` 47/0。

## 2026-10-28 第九轮：列宽 / 列显隐 / 单元格编辑与删除

把结果表格从「只读展示」推进到「可整理 + 可改」，写回全部走既有 `/v1/transactions` 与 `/v1/nodes/delete-many`：

- **列宽拖拽**：表头右缘 7px 热区（`#` 列不给手柄），拖动实时改 `<col>` 宽度；拖动前先把当前布局冻结成显式列宽再切 `table-layout: fixed`，避免切换瞬间跳变。宽度只写在 `<col>` 上——`th` 的 width 是 content-box、`col` 是 border-box，两处同写会在重渲染后漂移（实测 645 → 630）；落库值取「最后一次 move 的目标值」而不是 mouseup 时重新量 rect（Playwright 下 mouseup 可能早于最后一帧布局，实测少存一个步长）。列宽跨排序/重渲染保留，新查询保留（属于用户偏好）。
- **列显隐**：工具栏「列」按钮 + 表头右键；`#` 始终显示；隐藏只影响视图——表头、整行复制、CSV 导出都不含隐藏列；新查询重置为全显示（避免「列丢了」的误解）。
- **编辑 payload**：双击节点单元格（或右键「编辑 payload…」）打开 JSON 编辑器，回填该节点 payload；保存时 `POST /v1/transactions {op:"updatePayload"}`。注意服务端是**整体替换**不是合并（已实测），所以编辑器直接编辑完整 payload，并在提示里写明。保存后本地同步行/图/抽屉的节点引用并重渲染，随后刷新对象树。
- **删除节点**：右键「删除该节点…」→ 菜单项变成「再次点击确认删除 #id」→ 4 秒内再点才调 `/v1/nodes/delete-many`。不用原生 `confirm`（内嵌 webview 红线）。删除后移除行、重建图、刷新对象树。
- **交互防误触**：拖列宽结束 250ms 内忽略表头 click（否则「拖宽度」会顺带排序）。

回归：定向回归 820 → **832**（拖宽→排序后保持、隐藏列同时作用于表头与 CSV、双击编辑经 API 复核、右键两步删除后 404）；页内自检 151 → **154**（隐藏列只影响视图、列宽写 col 并切 fixed、节点单元格解析与编辑器回填）。视觉 9/9 不变（基线截图不含查询工具条）、冒烟/组合矩阵通过、rust fmt/clippy/test 0。

## 2026-10-28 第十轮：批量选择与批量操作

结果表格补上多选，写路径复用既有 `/v1/nodes/delete-many`：

- **选择列**：表格最左新增 36px 选择列（表头全选/半选、每行复选框），`#` 仍是行号列。选择按**节点 id** 记录，排序/隐藏列/重渲染后保持；重跑查询时只保留仍存在的 id。非节点行（如聚合结果）复选框禁用。
- **浮动操作条**：有选中行时在结果区底部浮出「已选 N 行 · 导出选中 · 删除选中… · 取消选择」，只作用于当前视图（切到图/JSON 自动隐藏）。
- **导出选中**：`buildGridCsv(rows)` 支持传入行集合，导出的是「选中行 × 当前可见列」；无选中时导全部。
- **批量删除**：两步确认（按钮文案变成「确认删除 N 个节点」，4 秒内再点），一次 `POST /v1/nodes/delete-many`；成功后本地移除这些行、清空选择、重建图、刷新对象树。
- **防误触**：点复选框不会触发「点击行打开抽屉」；`exportCsv` 现在同时兼容「当事件处理器绑定」与「传行数组」两种调用（此前把 click 事件当 rows 会静默不下载——回归里就是靠这一点抓到的）。

回归：定向回归 832 → **843**（全选/半选、导出选中真实下载、批量删除后逐个 404 且未选中节点保留、操作条隐藏）；页内自检 154 → **155**（全选/半选、导出选中 CSV 行数、非节点行禁用、新查询清空）；冒烟脚本的行号断言改为按 `data-col-key` 取（选择列占首格）。视觉 9/9、组合矩阵/冒烟双引擎通过、rust 0。

## 2026-10-28 第十一轮：查询多标签 + 事务模式

- **查询多标签**：查询台顶部新增标签条（＋新建 / 点击切换 / ✕ 关闭 / ←→ 切换 / Delete 关闭 / Ctrl+T 新建）。每个标签独立保存**编辑器文本、写入模式、最近一次结果与执行语句**；切换时保存当前、恢复目标，结果按服务端响应 `data` 重新渲染（不复制 DOM）。只读流式结果与普通 JSON 结果都会写入所属标签，所以切回来不会丢结果；标签标题在运行后自动取语句首行（截断 28 字）。关闭最后一个标签被禁用。排序/隐藏列/批量选择属于「当前结果视图」，切标签时重置。
- **事务模式**（工具栏「事务」）：服务端只有批量事务 `/v1/transactions`、没有交互式会话，因此把事务落在**界面写操作**上——编辑 payload、单删/批量删、导入（节点/关系）在事务模式下不再立即发请求，而是转成 `TransactionOperation` 暂存；标签条右侧出现「事务中 · N 个待提交 / 提交 / 回滚」，提交时一次 `POST /v1/transactions` 原子生效，回滚丢弃并重跑当前查询恢复本地乐观状态。有暂存操作时不允许直接关掉开关（先提交或回滚）。
- **TQL 写语句在事务模式下被明确拦下**：它走的是单语句 `/v1/tql` 且会立即落库，假装被事务保护是错的；提示文案给出替代路径（用界面编辑/删除，或先提交/回滚）。读语句不受影响。

回归：定向回归 843 → **860**（多标签新建/切换/结果恢复/关闭，事务暂存不动库、回滚保留、提交后 404、TQL 写语句被拦）；页内自检 155 → **157**（标签文本与结果快照、事务暂存不请求 + 提交一次请求 + 回滚清空）。冒烟/组合矩阵/视觉 9/9/真实 Sigma 取景 76×2 全通过；Rust 全量门禁：fmt 0、clippy 0、workspace **1537 passed / 0 failed / 13 ignored**、hooks/query/python/nodejs 全 0。

## 2026-10-29 第十二轮：侧栏对象树排版整改 + 查询历史弹层

- **对象树排版**（用户反馈「左边新增的 UI 太丑」）：组头改成「三角 + 标签 + 计数胶囊」，组体加 1px 引线明确层级；行内用 5px 色点区分类型/边标签，名称回到正文字体（原来整行 mono 像原始数据）；空组不再渲染「带计数的假行」，改成一行哑文字（暂无索引 / 暂无边标签）；向量组把「维度 + QuIVer」合并成一个彩色胶囊；导入入口从整宽边框按钮改成树末的虚线幽灵行。次要组（边标签/索引/向量）默认收起，类型保持展开；树整体限高 `min(300px, 38vh)` 内部滚动，避免把「查询历史 / 代码片段」挤出首屏。
- **查询历史改成入口 + 弹层**：侧栏原来的内联历史列表（最多 10 条、点击即执行）改为一个带条数徽标的导航项，点击打开面板。面板里有：搜索框、每条历史全文（保留换行）、元信息胶囊（读/写 · 行数 · 耗时 · 时间）、行内操作 **填入 / 运行 / 复制 / 删除**，以及「清空」。历史元信息存在内存 `historyMeta`（`queryHistory` 仍是字符串数组，兼容旧存储与既有断言）；`填入` 只写编辑器并关闭面板，`运行` 才执行——与侧栏片段同一套语义。

回归：定向回归 860 → **877**（对象树组展开态/空组哑文字/向量胶囊/导入行样式、历史面板条数与元信息、搜索过滤、填入、删除后徽标同步、清空后空态、Esc 关闭、导入行仍可打开向导；旧的内联历史断言改为新弹层语义）。页内自检 157 → **159**。冒烟双引擎、组合矩阵 900×2、视觉 9/9（侧栏改版后基线已更新并复验 ≤0.01%）、Sigma 取景 76×2、图尺度通过；rust fmt/clippy/`cargo test -p triviumdb-server` 全 0。

## 2026-10-30 第十三轮：V4.1 R2 工作区（设计稿 `v4.1-r2.html`）

R2 设计已获用户确认。本轮把「查询 / 图探索 / 向量」收敛成一套主导航，左栏只负责对象、历史与片段，并补齐结果语义、图视觉、跨页上下文和写入审阅。原型只作布局/交互参考：没有把 fixture、固定节点数、示例分数、8/768 维示例、简化 TQL 解析、内存假事务或 DOM 翻译适配器搬进产品。

- **外壳**：`.tabs-bar` 从工作区移到顶栏（`nav.tabs-bar.main-nav`），三个 `.tab` 的 id/role/roving tabindex/data-tab 全部保留；顶栏依次是品牌、主导航、连接状态、`中/EN`、教程/AI/文档/主题。左栏改为「数据浏览器」：`对象 / 历史 / 片段` 三段（`.side-panel` 互斥显示），历史段保留 `#historyNavItem` 入口 + 最近 6 条快捷填入。原侧栏「文档 / 图拓扑 / 向量与 QuIVer」三项重复导航移除，`data-action` 迁到顶栏 tab；「按库维度插入向量示例」保留为片段段显式动作（不再在切工作区时改写编辑器）。品牌从侧栏移到顶栏，`#sidebarRailToggle`、`#sidebarMobileBtn`、`#sidebarScrim`、`#historyNavItem`、`data-snippet` 等锚点保留。
- **侧栏宽度**：默认 232px（`--sidebar-width`），可拖 200–340；`#sidebarResize` 是 `role=separator`，支持真实鼠标拖动、←/→ ±10、Home/End 到边界、双击/Enter 回 232，`aria-valuenow/valuetext` 同步；偏好 `tdb_sidebar_width`。收起仍为 64px rail（`tdb_sidebar_rail`），新增对象/历史/片段三个图标，点击展开并切段。拖宽/收起后重算 Canvas 位图与 Sigma 容器并重新取景；断点往返沿用 `killTweensOf + clearProps`，不被 GSAP inline transform 锁死。
- **双语**：语言开关常驻顶栏（桌面/手机均可见），所有新增文案进入 I18N/I18N_EXT。切换语言重建对象树、查询标签、历史、待提交条、当前节点条、审阅弹层与向量来源卡，但**不**改写编辑器、结果、当前节点或向量输入。
- **结果语义**：节点列渲染身份卡（类型点 + 名称 + 原始 `payload.type` + `#id` + 实际维度 + payload 摘要）；缺失类型显示「(缺少 type)」，无向量显示「无向量」；服务端标量投影 `{type,value}` 在表格里显示可读值与类型小字，JSON/复制/导出仍是原始对象。任意投影/聚合/多节点列/null/嵌套结构继续按原列名与顺序显示。低频导出与列显隐收进「结果操作」菜单（id 与事件不变），批量操作只在选中后出现。对象树类型色改用 `getNodeColor` 的稳定哈希，采样计数统一写「样本 N（上限 200）」。
- **图视觉**：`nodeRadius` 基础 16→8（度数 +5%/级，2 倍封顶），命中半径仍额外 +10；悬停细环、选中环（+5/+9）、邻居细环；选中后无关节点/边/标签降权但仍可见，相关边加亮；标签按「选中/悬停 → 邻居 → 枢纽 → 其余」优先级，36 节点以上按缩放预算裁剪；普通节点默认不发光，只在暗色下给选中/邻居 7/3px 轻光晕（浅色无霓虹、reduced-motion/关闭动效为 0）。Canvas 与 Sigma 同源尺寸/降权；Sigma 选中用金色 + 1.35× 尺寸。`graphFitArea` 继续按实际浮层矩形让位（含统一面板），增量并入新节点且用户未动镜头时自动 fit，侧栏调宽/收起后重新取景。
- **向量与上下文**：新增全局「当前节点条」（`#contextStrip`），从结果表格、图、向量命中、详情打开都会更新；详情/探索关系/查找相似/清除四个动作跨三个工作区。向量页新增当前节点来源卡（`#vlSourceCard` / `#vlUseContextBtn`），缺向量时补一次真实节点读取并如实提示；`top_k` 留在主界面，`min_score` 收进「高级参数」（默认 -1 与合法范围不变）；结果列 = 排名 / 身份（名称·类型·ID·摘要·详情/关系/用作来源）/ 原始 score / payload 原文（≤700 隐藏）；分数不做概率转换；`__vectorSeq` 保证旧响应不覆盖新请求。
- **写入审阅**：`#txModeCheck` 默认开启（文案「审阅」），界面编辑/单删/批量删/导入默认进 `txOperations`，常驻 `#txBar` 显示数量与摘要；「审阅差异」打开 `#txReviewOverlay`，按操作类型展示前后键级差异/删除快照/新增节点/关系，提交仍走既有 `POST /v1/transactions` 原子事务；失败保留暂存与弹层、不自动重试；放弃不写库并重跑当前查询。TQL 写语句不进入队列：队列非空时拦下，队列为空时立即执行并首次提示。payload 编辑器保留整体替换语义，新增实时「未修改/修改/新增/删除」预览。
- **测试**：页内自检 162→167（新增主导航/侧栏三段/当前节点条、宽度 clamp 与键盘、语言保留状态、身份卡与缺失类型、审阅差异与提交）；回归新增 R2 场景脚本 `scripts/webui-v41-r2-checks.mjs`（真实 server，覆盖外壳组合 hit-test、拖宽/键盘/偏好/rail/断点、语言保留、任意结果形状、对象导航、跨工作区节点、密集图视觉/光晕/取景、写入审阅失败与提交、向量异步隔离、axe）。旧的 `.sidebar-brand-copy`、`.sidebar .nav-item[data-action]`、`#vlMinScore` 直接填写等断言按新结构更新，语义保留。

## 保留的约束

- 产品前端仍是 `crates/triviumdb-server/web/index.html`；无前端打包流程，Sigma 继续 CDN 按需加载。
- 不删除/重命名 V2-MAPPING §4 锚点，新增产品文案同时进入 I18N/I18N_EXT 的 zh/en 字典。
- 不改 HTTP 契约、PCA、输入校验、检索分数算法或大图护栏。
- 原型检查与产品检查分开记录，不能把示例结果当作真实性能数据。
- 不调用 GitHub API 创建 PR/评论。PR 由用户全部审核后手动创建。
- 根目录 HANDOFF.md 无关；PR-DRAFT.md、WEBUI-HANDOFF.md 是旧电脑未跟踪素材。跨电脑以本说明和已提交设计稿接续。


## 2026-10-30 整合验收：写入与标签边界修正

- 界面的节点编辑、删除与 CSV/JSONL 导入先校验完整 ID，只接受 `1..=9007199254740991`，不再经 `Number` 静默降精度；完整 Server ID 仍可查看，HTTP/内核的 `u64` 契约不变。
- 事务提交使用操作快照，成功只清除已发送部分；提交期间新增操作保留待下一批，失败后按原顺序重试，并保留新暂存项的乐观视图。
- 切换、新建活动标签或关闭活动标签时取消原标签未完成请求，并使其查询序号失效；JSON/NDJSON 迟到结果都不能写入其他标签。取消客户端等待不回滚已经提交的服务端写入。
- G21 使用真实 Server 和受控响应屏障覆盖这些场景，完整入口与范围见 [测试实践](../../webui-testing.md#写入与查询标签的边界回归)。手动 WebUI 诊断先验 Server 契约，双引擎回归与组合矩阵不再允许失败后继续判绿；该配置保持与嵌入式发布门禁独立。
