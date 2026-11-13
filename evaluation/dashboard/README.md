# 实验数据看板

直接打开 [index.html](index.html)，或将整个 `evaluation/` 目录放到静态网站上。页面使用本地 CSS、经典 JavaScript 和 Canvas 2D，支持离线浏览。

## 浏览入口

- **方法对比**默认显示 QuIVer 与 BBQ。选择数据集、线程和召回档后，可展开各方法的构建配置与查询点，最多加入四个方法。四项优势指标比较 A/B，图表包含全部已选择方法。
- **全量记录**可按数据集、方法、方法类别、线程、召回和参数检索。列组包括吞吐、延迟、构建与内存，以及全部指标，当前筛选结果可导出 CSV。
- **QuIVer 全库**汇集 QuIVer 在 21 个数据集上的记录。
- **覆盖与来源**显示观测数量、指标覆盖及方法与数据集的交叉表，点击数量可跳转到对应记录。

图表支持选点、缩放、平移、图例显隐和 PNG 导出。测量口径、逐轮数据、组件账与证据身份放在备注抽屉中。

## 语言

右上角可切换简体中文与 English。首次打开按浏览器语言选择，手动选择保存在本地。视图链接包含语言设置。数字使用对应的地区格式，英文数量文案支持单复数。

表头、筛选项、图表、方法名称、导出列名和备注随语言切换。检索同时匹配中英方法名称与参数说明。方法身份、查询参数和证据 SHA 使用记录中的原值，界面翻译保存在独立语言包中。

中文原文用作稳定的消息键，英文词典位于 `locales/en/`。新增翻译时，插值占位符名称应与原文一致。静态 HTML 使用 `data-i18n` 和属性标记，动态文案调用 `t()`，记录对象通过 `localize()` 生成展示内容。

## 文件职责

| 文件 | 职责 |
| --- | --- |
| `index.html` | 页面结构、导航和控件 |
| `assets/css/tokens.css` | 字体、颜色、主题和基础规则 |
| `assets/css/layout.css` | 页面、面板和响应式结构 |
| `assets/css/components.css` | 比较卡片、表格、图表控件和备注抽屉 |
| `assets/js/core.js` | 数据入口、共享状态、索引和格式化 |
| `assets/js/i18n.js` | 语言选择、词典、插值、单复数与本地化格式 |
| `locales/en/ui.js` | 导航、控件、表格、图表和操作文案 |
| `locales/en/record-labels.js` | 方法、类别与记录字段名称 |
| `locales/en/measurement-notes.js` | 构建、计时、内存与查询记录的英文备注 |
| `locales/en/archive-notes.js` | 汇总与历史记录的英文说明 |
| `assets/js/model.js` | 观测映射、选点、筛选与优势计算 |
| `assets/js/battle.js` | 方法配置、A/B 比较和实测召回配对 |
| `assets/js/tables.js` | 列定义、排序、分页和 CSV 导出 |
| `assets/js/charts.js` | Canvas 2D 绘图、点选择和视域 |
| `assets/js/coverage.js` | 覆盖和来源汇总 |
| `assets/js/notes.js` | 指标说明与逐条证据 |
| `assets/js/app.js` | 初始化、跨模块事件和导航 |
| `data/catalog.js` | 元数据、数据集及原始记录顺序 |
| `data/configurations/Dxx.js` | 每个数据集的构建配置 |
| `data/observations/Dxx/*.js` | 按数据集与构建配置拆分的观测曲线 |
| `data/annotations/Dxx.js` | 各数据集去重后的备注与证据对象 |
| `data/reference-notes.js` | 源记录的全局说明 |
| `data/historical-bq.js` | 历史 BQ 观测 |
| `data/assemble.js` | 恢复完整观测结构及原始顺序 |
| `tools/update_data.py` | 生成分片并更新 HTML 中的加载清单 |

样式和交互代码可直接修改后刷新。脚本按 `index.html` 中的 `defer` 顺序加载，共享命名空间为 `window.QuiverDashboard`。

## 更新测量数据

权威数据源为 [`../results/benchmark-snapshot.json`](../results/benchmark-snapshot.json)。数据按数据集与构建配置拆分，备注和证据以引用去重。装配时恢复完整内容与原始顺序。在仓库根目录运行以下命令更新离线副本和加载清单。

```bash
python evaluation/dashboard/tools/update_data.py
```

随发布文件更新 `../provenance/manifest.json` 的文件清单与 SHA-256 后，可运行数据身份核对。

```bash
python evaluation/tools/verify_data.py
```

核对程序检查文件哈希，以及 JSON、CSV 和离线数据副本的逐项一致性。
