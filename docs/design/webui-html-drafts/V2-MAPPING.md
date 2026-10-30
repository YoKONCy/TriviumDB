# V2 蓝本映射表

- 蓝本：`v2-hybrid-explorer.html`（可交互原型不随仓库分发，本地保留在 `.tmp/design-prototypes/`）
- 预览：本地原型预览脚本（`.tmp/` 内，未入库）
- 当前产品分支：`feature/webui-astrmira-theme`
- 当前产品代码 HEAD：`84baa23`
- 原则：V2 是唯一视觉/结构蓝本；不按截图临时发挥；保留既有 DOM id 与测试锚点
- 浅色 C 风格：V2 验收完成后再单独立项，本阶段不新增 C 切换入口

## 1. 区域映射

| V2 区域 | 当前实现 | 动作 | 风险/约束 |
|---|---|---|---|
| 左侧 A 侧栏 | `aside.sidebar`、`.sidebar-brand`、`.nav-item`、`.sidebar-content`、`.sidebar-footer` | 保留结构，按 V2 调整宽度、间距、选中态、分组 | 保留 `data-action`、`sidebarRailToggle`、`sidebarMobileBtn`、`sidebarScrim` |
| 侧栏品牌 | `.sidebar-brand`、`.brand-mark`、`.sidebar-brand-copy` | 保留，按 V2 微调 30px 标记 / 15px 字标 / 9px 副标题 | 不恢复顶栏品牌，避免重复 |
| 侧栏选中态 | `.nav-item.active` | 改为 B 版：3px 青色左条 + 青紫渐变胶囊 + 轻微青 glow | 用户明确保留该效果 |
| 顶栏 | `header.app-header`、`.header-status`、`#connStatus`、`#connText`、`#headerGeneration`、`.header-actions` | 重排为「视图名 + 状态 + 操作」 | 保留连接状态和 generation 逻辑 |
| 主图舞台 | `#viewGraph`、`#graphCanvas`、`#standaloneGraphCanvas`、`.graph-sigma-container` | 保留双后端容器，按 V2 设置深空舞台 | 不直接粘 V2 SVG；Canvas/Sigma 分别实现 |
| 左上 HUD | 现有图工具条 / k-hop / 路径入口 | 拆成图内搜索、k-hop、路径追踪 | 保留原按钮 id 与事件 |
| 顶部查询舱 | 无 | 新增 `#graphCockpit`、`#graphCockpitToggle`、`#graphCockpitQuery`、`#graphSimThreshold`、`#graphCockpitRun`、`#graphHitList` | 复用现有 `runQuery` / TQL 输入，不新造查询语法 |
| 右上 HUD | `.graph-float-toolbar` | 放大 / 缩小 / 适应窗口 / 星云模式 / 动效开关 | 保留 `gftZoomInBtn` 等既有 id |
| 底部布局条 | `.graph-controls` 12/14 个按钮 | 只留后端 + 4 布局，总宽 ≤520px | 其余按钮迁右上或「更多」；保留 `layoutForceBtn` 等 id |
| 底部结果抽屉 | `#viewGrid`、`#viewJson`、`.results-tabs` | 新增 `#graphResultDock` 包裹层，表格/JSON 复用现有渲染 | 不移动既有 id；先做结构，再接线 |
| 右侧详情 | `#unifiedSidePanel`、`#uspRail`、`#uspTabDetail`、`#uspTabFilter`、`#uspTabAi`、`#uspCollapseBtn` | 重构四卡片：Payload / Vector / Edges / 投影 | 保留三 tab 与 open/close class 语义 |
| 节点详情内容 | `#drawerNodePayload`、`#drawerNodeVector`、`#drawerNodeEdges`、`#drawerMiniMap` | 加 3px 三模色条、粘性折叠头、Payload 8 行 | 保留现有数据渲染函数 |
| 状态栏 | `footer.app-status-bar`、`#status*` | 保留现有字段顺序 | 不改服务端字段含义 |
| 星尘 | `.starfield`、`#starfield` | 保留 240 星点 + 极慢漂移 | 浅色阶段前不做 C 星空 |
| 动效开关 | 无 | 新增 `#graphMotionToggle` + `localStorage.tdb_graph_motion` | 不新增 C 主题切换入口 |

## 2. 选中态规范（B 版，用户确认保留）

```css
.nav-item.active {
  color: #fff;
  font-weight: 650;
  border-radius: 12px;
  background: linear-gradient(100deg,
    rgba(99, 242, 231, 0.22) 0%,
    rgba(167, 139, 250, 0.24) 78%,
    rgba(99, 242, 231, 0.10) 100%);
  box-shadow:
    inset 3px 0 0 #63f2e7,
    0 0 18px rgba(99, 242, 231, 0.10);
}
```

- 左侧 3px 青色条必须圆角贴合胶囊；
- 渐变由青到紫，不要改成纯色；
- 图标和文字同时提亮为 `#fff`；
- 不加外边框，避免看起来像按钮；
- hover 不应覆盖 active 背景。

## 3. 新 DOM 命名

| 新 id | 用途 | 实现名（2026-10-23 核对） |
|---|---|---|
| `graphCockpit` | 顶部查询舱外壳 | 同名 |
| `graphCockpitToggle` | 展开/收起 | 同名 |
| `graphCockpitQuery` | 查询摘要/编辑器 | 同名 |
| `graphSimThreshold` | 相似度阈值 | **`graphCockpitThreshold`**（+ `graphCockpitThresholdValue`） |
| `graphCockpitRun` | 运行按钮 | 同名 |
| `graphHitList` | 查询命中列表 | **`graphCockpitHits`** |
| `graphResultDock` | 底部结果抽屉 | **`graphDock`**（+ `graphDockCount`） |
| `graphDockToggle` | 抽屉展开/收起 | 同名 |
| `graphDockGrid` | 表格 tab | `graphDockGrid` / tab 按钮 `graphDockGridTab` |
| `graphDockJson` | JSON tab | `graphDockJson` / tab 按钮 `graphDockJsonTab` |
| `graphMotionToggle` | 图动效开关 | **`graphMotionBtn`**（持久化键仍为 `tdb_graph_motion`） |
| `graphHudSearch` | 图内搜索 | 同名 |
| `graphHudKhop` | k-hop 入口 | 同名 |
| `graphHudPath` | 路径追踪 | 同名 |

新 class 统一前缀：`g-`（graph overlay）、`v2-`（布局辅助），避免与既有样式冲突。

## 4. 保持不变的测试锚点

以下不得重命名或删除：

- `#tqlInput`、`#runQueryBtn`、`#formatTqlBtn`、`#clearEditorBtn`、`#exportJsonBtn`
- `#viewGrid`、`#viewGraph`、`#viewJson`、`#graphCanvas`、`#standaloneGraphCanvas`
- `#layoutForceBtn`、`#layoutClusterBtn`、`#layoutVectorBtn`、`#layoutRadialBtn`
- `#unifiedSidePanel`、`#uspRail`、`#uspTabDetail`、`#uspTabFilter`、`#uspTabAi`
- `#drawerNodePayload`、`#drawerNodeVector`、`#drawerNodeEdges`、`#drawerMiniMap`
- `#sidebarRailToggle`、`#sidebarMobileBtn`、`#sidebarScrim`
- `#connText`、`#headerGeneration`、`#status*`
- `#graphFilterPanel`、`#graphLegend`、`#toggleWebGLBtn`

如需新增包裹层，只包裹，不替换原 id；selftest/ui-smoke 先跑再调整断言。

## 5. 实施顺序（Phase 0 → 6）

1. **Phase 0**：冻结 V2 + 本映射表；不改产品代码。
2. **Phase 1**：顶栏 + 侧栏 + 选中态 + token；先静态对齐。
3. **Phase 2**：图舞台几何 + HUD + 查询舱/抽屉骨架。
4. **Phase 3**：Canvas 图视觉 → Sigma 对齐（最高风险，单独小步）。
5. **Phase 4**：右侧详情四卡片 + 图/表/详情选择联动。
6. **Phase 5**：390/1024/1440、i18n、性能降级、动效开关。
7. **Phase 6**：selftest / ui-smoke / 视觉基线 / 单 commit。

## 6. 验收门禁

- 每阶段 `cargo test -p triviumdb-server` + `node scripts/ui-smoke.mjs` 全绿；
- 1440 / 1024 / 390 无横向溢出；
- console 0 error；
- 新增文案 zh/en 双字典；
- 动效关闭后图节点 0 漂移；
- 大图 1k 节点可交互，glow 按规模降级；
- 视觉基线按 V2 重建，不沿用旧截图。

## 7. 浅色 C 风格（✅ 2026-10-25 落地，蓝本 `warm-stone-light.html`，本地原型在 `.tmp/design-prototypes/`）

- 暖石底：`#f3efe6` 底 / `#fbf9f4` 面板 / `#eee9dd` 壳层；实线边 `#ddd5c4` / `#c8c0ad`；文字 `#242018` 三级灰阶
- 图边线 `#5f523b`（既有）、34px 暖石网格舞台（Canvas `drawSpaceBackground` 与 Sigma 容器 CSS 网格一致）
- 文字 Halo：面板色描边（11px 字号 3.4px，蓝本 4px@11.5px 同比例）
- 浅色三模色（足饱和）：person 青 `#4d7c78` / paper 赭 `#8c764e` / topic 紫 `#645680` / user 金 `#9e7f47`，扩展盘 6 色同调；选中环金 `#9e7f47`
- 不新增切换按钮：沿用既有主题按钮；selftest 断言「浅色 C 暖石——足饱和三模色与暖石底 token」（总数 128）

## 8. 实施进度（2026-10-24 核对）

| Phase | 状态 | 说明 |
|---|---|---|
| 0 冻结蓝本 | ✅ | 本目录 6 份 HTML + 本映射表 |
| 1 壳层 / 侧栏 / token | ✅ | 星尘侧栏、B 版选中态、暗浅 token 已落地 |
| 2 图舞台 / HUD / 查询舱 / 抽屉骨架 | ✅ | id 以上表「实现名」列为准 |
| 3 Canvas 视觉 → Sigma 对齐 | 🟡 | Canvas：未知类型哈希配色、边可见度、光晕、首屏自动适配已调；Sigma：节点尺寸/选中尺寸/边宽已与 Canvas 对齐并加切换后 fit。**已知差异**：Sigma WebGL 单色边无法复刻 Canvas 源→目标渐变、无节点描边程序 |
| 4 详情四卡 | ✅ | Payload / Vector / Edges / **投影**（PCA 3D：`POST /v1/nodes/batch-get` 一次取自身 + 1 跳邻居，固定轴测投影，点击投影点可跳转节点；无向量时显示空态） |
| 5 响应式 / i18n / 降级 / 动效开关 | ✅ | i18n、断点（1280/1100/900/700）、动效开关（`graphMotionBtn`）已完成；2026-10-24 实机复核：1440/1024/390 无横向溢出（390 修复工具栏提示逐字竖排缺陷）、prefers-reduced-motion 默认关闭+按钮禁用+不误写持久化、动效关闭 0 漂移、1k 节点 glow 按规模降级（选中 15/枢纽 6/普通 0）且可交互（力学护栏生效，重绘 20ms） |
| 6 selftest / ui-smoke / 视觉基线 | ✅ | 2026-10-24 全绿：页内 selftest **127/127**；`cargo test -p triviumdb-server` 11 单元 + 29 契约；`node scripts/ui-smoke.mjs` 全流程通过（Playwright）；视觉基线按 V2 重建（`visual-baseline.mjs --update`，复验差异 ≤0.02%） |

### V2 细节收敛（2026-10-25 第三轮，对照 v2-hybrid-explorer 蓝本逐项补齐）

- **查询舱 KNN 模式**：`SEARCH VECTOR [...] TOP k` 查询改走 `POST /v1/search/vector`（二进制 LE f32，`application/vnd.triviumdb.vector+f32`）取回带分数命中；引擎分数为内部融合分（非 0~1 余弦），UI 按本批最高分归一为相对相似度；阈值滑杆（0.70~0.95）做客户端过滤（`graph.cockpit.filtered` 空态文案 zh/en）
- **命中列表**：`graphCockpitHits` 空态 → 真实命中（ID + 青色相似度 + 类型 chip，点击跳转节点）；TQL 运行后显示结果摘要
- **结果抽屉 6 列**：`# / 节点 ID / 类型（getNodeColor 彩色胶囊）/ 关联边（最高频边标签）/ Payload 摘要 / 相似度（青→金渐变条 + 数值，来自 _sim）`；表头 zh/en 双字典（`graph.dock.index/edges/sim`）
- **脉冲扩散环**：运行（KNN/TQL 新增>0）与路径追踪成功后从锚点扩出 r18→220 青色环（1.05s ease-out）；动效关闭或 reduced-motion 不触发（不改坐标，0 漂移门禁不受影响）
- **边标签沿边旋转**：atan2 角度 + 倒置翻转 180°，Halo 保留
- **查询舱语法高亮**：textarea 透明字 + `graphCockpitQueryHl` overlay（复用 `highlightTql`，tk-* 类配色）；预览栏实时同步当前查询
- **聚类星云虚线环**：聚类布局每簇（≥3 节点）画青色外环+紫色内环虚线椭圆+柔填充（暗/浅两套色）；分组标记 `_cluster` 与摆位同源（社区→类型回退），切出聚类即清除
- selftest 新增 4 组断言（**127→132**）：KNN 解析/命中/相似度列、脉冲时长与动效门禁、高亮 overlay 与预览同步、聚类分组标记
