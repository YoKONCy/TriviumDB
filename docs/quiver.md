# QuIVer 向量索引

> 本文介绍 QuIVer 的图导航、原始向量精排、可选压缩重排、冷热驻留、构建维护与资源计量。开发基线为 `0.9.1-beta.1`，新增能力见[未发布变更](changes/unreleased.md)，冻结研究资料见 [ICDE 复现归档](../evaluation/README.md)。

---

## 目录

- [概述](#概述)
- [自动构建阈值](#自动构建阈值)
- [默认查询路径](#默认查询路径)
- [默认、显式与兼容模式](#默认显式与兼容模式)
- [增量维护、重载与内存](#增量维护重载与内存)
- [SSD 冷向量编码基底](#ssd-冷向量编码基底)
- [SSD 索引制品与 Reader](#ssd-索引制品与-reader)
- [性能评估](#性能评估)
- [检查与复现入口](#检查与复现入口)
- [0.9.0-beta.3 检索与计量](#090-beta3-检索与计量)
- [索引事实绑定与 BQ 边界](#索引事实绑定与-bq-边界)

---

## 概述

QuIVer 是 TriviumDB 的近似最近邻（ANN）图索引，使用紧凑量化表示完成图导航，再从原始向量计算余弦相似度。SM-L2 导航从 `0.8.9` 起默认启用，`0.8.10` 增加构图连通性与增量维护保护，详见 [版本变更](changes/0.8.10.md)。

---

## 自动构建阈值

从 `0.9.0-beta.2` 起按 `ceil(666,667 / dim)` 计算自动构建节点阈值，并限制在 209–834 节点，约为此前阈值的十二分之一。384/768 维为 834，1024 维为 652，1536 维为 435，2048 维为 326，3072 维为 218。达到阈值后，在下一次符合条件的查询准备或显式预热中构建；普通查询与连续向量视图入口共享同一判断。

`auto_build_quiver=false` 关闭自动构建；高于 3072 维继续使用精确 BruteForce。ANN 查询通过紧凑导航缩小原始向量精排范围，实例容量还需计入索引常驻结构、首次构建工作区与向量页驻留。

---

## 默认查询路径

数据库的 `f32/f16` 浮点向量查询采用：**原 BQ 快速构图 → 签名多样性保护与入口可达性修复 → 无训练 SM-L2 导航 → 有界原始向量余弦精排 → 按需读取 Payload**。近邻选择沿用原 QuIVer 的 BQ 路径；最终剪枝出现同签名近邻挤占时，利用既有容量保留其他签名的候选。随后仅给不可达区域补边。没有签名碰撞且原本全可达的图，其拓扑保持不变。

`u64` 用于 SimHash 等二进制指纹，查询按原始整数位模式执行精确汉明检索，分数为各分量的匹配位数之和。自动查询、批量查询和重开保持相同度量；加载时跳过历史 `u64` QuIVer sidecar，只读打开保留原文件。显式 QuIVer 构建适用于 `f32/f16`，对 `u64` 返回 `TDB_INVALID_INPUT`。

| 环节 | 当前行为 |
| --- | --- |
| 构图 | 使用低位 BQ 编码和批量并行图构建路径，保留签名多样性并保护入口可达性 |
| 表示 | 两个位平面保存符号与强弱分组，并保存每向量弱幅值、幅值差与重建范数等紧凑元数据 |
| 导航 | 结合位计数和幅值运算，计算重建向量的平方 L2 评分 |
| 精排 | 默认查询按原始向量余弦相似度返回分数 |
| 训练 | 默认路径无需查询样本校正、锚点拟合、码本或旋转训练；每向量幅值由该向量自身计算 |
| 执行 | 768 维保留专用实现，其他合法维度使用通用实现；运行时选择可用 SIMD，保留标量和尾部处理 |
| 维度 | 支持 `1..=3072`，包括非 64 倍数；更高维数据库保持精确 BruteForce 路由 |

概念上，每维重建值为 `符号 × (弱幅值 + 强位 × 幅值差)`；SM-L2 比较重建向量之间的平方距离。最终结果由原始向量余弦精排产生，召回率随近似候选的覆盖范围变化。

底层 `QuIVerSearchConfig` 的有效候选宽度至少覆盖 `top_k`，并受节点数限制。未显式指定 `rerank_limit` 时默认预算为 64，实际预算按 `min(有效候选宽度, max(top_k, 指定预算或64))` 计算。上层管线显式传入的预算继续生效；过滤拒绝和墓碑不占成功精排名额。低 `ef` 会收紧有效精排预算，普通默认查询继续使用 SM-L2。

---

## 默认、显式与兼容模式

候选按近似距离进入精排名额，截断处的同分候选按外部 NodeId 升序选择，最终结果按精确分数降序、外部 NodeId 升序排列；顺序适用于实际搜索到的候选集合。默认 SM 与公开 BQ 精排路径采用同一结果同分规则。直接调用 `QuIVer::search`、`search_with_scorer`、`search_dual_queue` 时，维度错误、非有限查询及 `top_k=0` 返回空结果；冷向量回调中的维度错误和非有限分量会使对应候选被拒绝。

| 模式 | 启用与度量 | 边界 |
| --- | --- | --- |
| 默认无训练 SM-L2 | 数据库构图和加载后自动准备；最终余弦精排 | Python、Node.js 和 Rust 普通查询共用 Core，无需手动调用启用函数 |
| 显式校正 SM | Rust 显式安装带校正系数和锚点的模型 | 需要显式安装校正模型；默认查询不执行模型拟合 |
| 显式原生 L2 | Rust `enable_quiver_sm_native_l2()`；保留原始幅值与合法零向量，返回负平方欧氏距离 | 适用于显式原生 L2 查询；普通数据库查询继续使用余弦度量，召回率需按业务数据验证 |
| 自定义导航 | 使用 `NavigationScorer` 的 TSNG 等入口 | 按所选 scorer 计算导航得分 |
| 独立旧索引兼容 | 仅从字节加载、尚未绑定原始向量的 `QuIVer` 对象 | 可保留旧 BQ 路径；数据库加载流程会补齐默认 SM 状态 |

显式覆盖在图或向量变化后失效，普通查询恢复默认 SM-L2。显式校正锚点被过滤屏蔽时，可以采用无锚点默认 SM。

---

## 增量维护、重载与内存

- 构图时准备默认 SM 状态，插入时追加新节点的紧凑行。墓碑可以参与导航，但不进入结果，也不能将已复用 slot 中的新向量当作旧节点向量。
- 构图和加载时派生从当前入口覆盖全图的导航树，每节点保存一个 `u32` 父节点，计入索引热内存。增量剪枝保留树边，入口升层时连接旧树；补边使用既有邻接表容量，不自动扩大存储步长。必要时出度可高于常规 `m0`，但不超过原有 `2*m0` 容量；无碰撞且全可达的正常图不额外加环。
- 入口全可达表示图拓扑可遍历全库；实际候选覆盖范围由 `ef` 和精排预算共同决定。近重复、量化签名碰撞和低内在维度数据需独立验证召回与性能。
- 旧索引通过重新构建应用本次修复，不提供加载时补边或独立的旧图修复入口。加载发现入口不可达节点时返回需要重建的错误，数据库沿用现有缺失/无效索引处理流程：通常在首次查询时重建，严格索引策略则返回错误。
- `.quiver` v2 在原图与 BQ 数据后保存默认 SM 的位平面、幅值、重建范数和精算归一化比例。路由快照记录维度、节点数、行宽和 CRC，加载时先检查完整几何与 CRC，再分配紧凑行。默认 SM 使用已有图邻接表。
- v1 索引及 v2 中空的路由快照从实际向量池按 slot 流式恢复默认 SM 状态。显式 `flush()` 将当前索引保存为 v2。墓碑沿用历史恢复的零向量导航状态；显式校正和原生 L2 模式通过各自安装入口启用。
- 同一次 Mmap 打开中，文件组已经验证的主文件与向量 CRC 传递给 QuIVer 事实绑定，完整文件校验在各次打开之间独立执行。加载扫描结束后回收干净向量页：Unix 提交释放建议，Windows 从原文件句柄重建同代冷映射，后续精排按需触及向量。启动峰值与加载结束后的驻留工作集分别采集。
- 默认准备过程不保留第二份全库 FP32 向量。显式模式的安装路径可能使用全量临时 FP32 缓冲，安装峰值与稳定驻留应分别计量；显式原生 L2 的度量选择不由旧图文件自动恢复。
- 热内存包括索引 BQ、默认/显式 SM 状态、图、ID/slot 映射、派生缓存和每线程工作区。
- 原始向量及 Payload 冷层、映射文件、RSS/PSS、OS PageCache 和安装临时峰值分别报告。SM 拥有内存计入索引及内核预算；进程总开销和 OS PageCache 单独测量。

> **兼容与容量提醒**　v2 的默认路由快照占用 `28 + 索引节点数（含墓碑） × (16 × ceil(dim/64) + 20)` 字节，外层另有 8 字节长度。例如一百万个 384 维索引节点增加约 116 MB 磁盘数据。打开所需的保守预算准入按索引文件大小计算。仅支持 v1 的旧内核按自身缺失索引策略处理 v2；需要降级时由目标版本重建派生索引。主数据及索引的完整文件组备份遵循对应版本的发布与恢复契约。

---

## SSD 冷向量编码基底

### 可选压缩重排与查询精度

库编码、重排查询精度和驻留策略分别配置。`RerankEncoding` 提供 `int8`、`centered_int8`、`centered_int4`，`QueryPrecision` 提供 `fp32`、`int8`、`int4`。`SsdRuntimeOptions::accuracy()` 选择中心化 INT8 与 FP32 查询，`compact()` 选择中心化 INT4 与 FP32 查询。两种预设均启用 `use_for_ann`，驻留策略通过 `residency` 选择 `ssd` 或 `hot`。

中心化编码先稳定归一化库向量，减去制品共享的 FP32 均值，再逐 128 维分组量化。INT8 采用 `[-127,127]`，INT4 采用 `[0,15]`，两码压紧到一个字节，尾部高半字节填零。分组参数为 FP32 scale 与 bias，舍入采用 ties-even。每行记录完整重建向量的逆范数，原始零向量使用零因子。均值按当前索引内部行序等距抽样至多 32768 行，跳过墓碑，以归一化后的 FP32 分量进行 FP64 累加，最终保存 FP32 均值。均值固定于本次发布代际。

独立宿主可通过 `SsdQuiver::publish_with_center` 传入冻结的中心，以复现指定标定结果。中心化编码要求中心长度等于维度，所有分量有限且位于 `[-1,1]`。传入 `None` 使用上述抽样规则。

每条查询按所选精度准备一次范数、分组和与中心点积。FP32 查询保留输入表示并以 FP64 累加；INT8 和 INT4 查询分别使用全局对称范围 `[-127,127]`、`[-7,7]`，评分使用该整数表示的范数和中心项。完整组在支持 AVX2 的 x86_64 上使用向量化核，尾组与其他平台使用标量核。图导航继续使用原查询的现有 SM-L2 表示。

Hot 驻留保存压缩记录与共享均值，评分按组解包。SSD 驻留使用同一编码和评分逻辑，通过 `cache_bytes` 控制 Ready 行缓存。数据库入口在查询与维护期间借用主 QuIVer 的导航结构，数据库读写锁保障借用期间的代际稳定。独立 `SsdQuiver::open` 加载制品中的自有路由。均值及行租约计入资源预算，关闭后仍被调用方持有的行继续持有中心和对应额度。原始向量事实由数据库存储模式管理，Ram 存储模式还需计入原始向量的常驻成本。

数据库通用入口为 `configure_quiver`、`search_quiver`、`quiver_stats`、`close_quiver`，既有 `*_quiver_int8` 入口继续调用同一实现。Python 与 Node 通过已有 JSON 请求接入，例如以下配置选择紧凑重排并用于普通 ANN 查询。

```json
{
  "operation": "configure",
  "options": {
    "encoding": "centered_int4",
    "query_precision": "fp32",
    "residency": "hot",
    "cache_bytes": 0,
    "use_for_ann": true
  }
}
```

`use_for_ann=true` 接入普通混合检索中的 QuIVer 召回、TQL QuIVer 向量来源与纯向量 TSNG 路径。带独立业务评分的 TSNG 与文档 ANN 路径继续遵循自身评分契约。TQL 的压缩重排分数标记为 Approximate。原始向量精确扫描通过现有精确入口选择。默认配置为 codec 1、INT8 查询、SSD 驻留、零 Ready 缓存以及 `use_for_ann=false`。

普通查询按实际使用的来源准备压缩索引。精确扫描、纯文本及图文档查询、仅输出计划的 `EXPLAIN` 直接使用各自的数据来源。向量写入后的压缩代际刷新由后续压缩 ANN 或显式维护操作触发。

> **精度提醒**　中心化 INT4 与中心化 INT8 的召回代价随数据分布和工作点变化。查询精度、编码与实际驻留方式均由统计返回，应用应在自身数据上选择配置。当前可用查询精度为 FP32、INT8 和 INT4。

> **配置提醒**　查询精度、驻留方式及 `use_for_ann` 属于运行时设置，重开后重新配置。普通压缩 ANN 使用默认 SM 余弦模式，显式校正及原生 L2 可配置 `use_for_ann=false`，继续通过原始向量评分入口执行。

Rust 底层模块 `triviumdb::index::quiver::cold` 提供通用 `ColdLayout`、`ColdRow`、`RerankQuery` 及历史 codec 1 的 `Int8Layout`、`Int8Row`、`Int8Query`，用于编码、读取缓冲校验和量化评分。维度覆盖 `1..=3072`，数据库原始输入支持 `f32/f16`，二进制整数指纹返回带 `dtype` 字段的 `TDB_INVALID_INPUT`。

### 编码与评分

codec 1 使用稳定归一化、group128 minmax 仿射 INT8 和重建范数校正。编码先以 f64 计算原向量范数，生成 f32 单位方向，再按实际分组的最小值和最大值生成 f32 `scale/bias`。整数编码采用 ties-even 舍入并限制在 `[-127,127]`。常量组使用 `scale=1、bias=常量、code=0`，全零行的范数校正为 1。极小组差值的 scale 至少为最小正 f32 次正规数。

每组最多 128 维，尾组只包含实际分量。例如 769 维使用六个完整组和一个单元素常量组。范数、组内求和、点积和偏置恢复均覆盖实际维度。

`Int8Query` 将查询按全局最大绝对值对称量化。标量评分按组执行 i32 点积，以 f64 恢复仿射项，并结合数据库重建范数和查询整数范数计算近似余弦，返回范围为 `[-1,1]`。任意一侧为零向量时返回 0。查询整数范数校正以及编码前的归一化属于该 codec 的明确语义，研究归档采用的原始幅值编码和评分过程有各自的实现身份。

### 布局与校验

所有多字节字段使用小端表示。40 字节布局头保存以下字段。

| 偏移 | 字段 | 字节数 |
| --- | --- | --- |
| 0 | Magic `QVCOLD01` | 8 |
| 8 | 布局版本 1 | 4 |
| 12 | codec 1 | 4 |
| 16 | 实际维度 | 4 |
| 20 | 完整组维度 128 | 4 |
| 24 | 行有效载荷字节数，含 CRC | 4 |
| 28 | 行步长字节数 | 4 |
| 32 | 保留字段，值为 0 | 4 |
| 36 | 前 36 字节的 CRC32 | 4 |

每行依次保存 `D` 字节整数编码、`ceil(D/128)` 组 f32 `scale/bias`、一个 f32 范数校正和四字节 CRC32，剩余行步长填零。CRC 覆盖编码和浮点参数，填充字节单独检查。有效载荷为 `D + 8 × ceil(D/128) + 8` 字节，768 维为 824 字节；选择 64 字节行对齐时步长为 832，选择 1024 字节行对齐时步长为 1024。

`Int8Layout::row` 在创建借用视图前验证长度、CRC、填充、整数编码、有限仿射参数及重算后的规范范数校正。损坏字节返回 `TDB_CORRUPTED_FILE`。布局参数和行区范围计算在分配及文件访问前验证维度、步长、编号和整数溢出。

行对齐由调用方选择，物理 I/O 对齐由后续读取器管理。布局头描述编码几何，容器目录负责绑定数据库、节点、slot、事实代际及区段位置。

### 调用与分层

编码器使用固定上限的单行工作区，输出缓冲由调用方提供。`Int8Row` 借用调用方字节，生命周期由行缓冲或后续缓存租约限定。查询编码工作区大小与数据库节点数量独立。

```rust
use triviumdb::index::quiver::cold::{Int8Layout, Int8Query};

fn main() -> triviumdb::Result<()> {
    let layout = Int8Layout::new(4, 64)?;
    let mut bytes = vec![0; layout.row_stride_bytes()];
    layout.encode(&[1f32, -1., 1., -1.], &mut bytes)?;
    let row = layout.row(&bytes)?;
    let query = Int8Query::new(&[1f32, -1., 1., -1.])?;
    assert_eq!(query.score(row)?, 1.);
    Ok(())
}
```

存储内核维护编码、索引代际、资源预算及读取生命周期，并提供流式编码、单文件发布、冷行缓存、驻留切换和动态缓存调容。SDK 适配模式配置、请求上下文及错误字段，数据库入口自动维护来源身份。

> **验收与使用提醒**　该 codec 的跨数据集召回、吞吐、安装峰值及平台 I/O 表现需要随集成单独测量。

---

## SSD 索引制品与 Reader

`triviumdb::index::quiver::ssd` 提供 `SsdQuiver`、`SsdOptions`、`SsdMetadata`、`SsdStats` 和 `SsdRow`。Rust 底层调用可从已有 QuIVer 图与稳定的 slot 向量来源流式生成单文件压缩索引，随后只读打开、按 NodeId 取得编码行，或执行显式压缩重排检索。

### 来源与单文件布局

底层 `publish_with_center` 接受可选冻结中心，用于宿主复现已有校准结果。传入 `None` 时使用当前发布代际的固定抽样规则，传入中心时核对维度、有限值与分量范围 `[-1,1]`，随后随制品持久化。

底层 `publish` 和 `open` 接收 32 字节来源身份，并检查其相等性。独立宿主可从包含数据库身份、dtype、维度和事实代际的不可变清单计算摘要，构建期间固定同一事实快照。数据库入口自动生成并校验来源身份。每次发布另生成 16 字节 UUID，`SsdMetadata` 返回来源、发布代际、维度、行数及区段大小。Reader 持有实际打开的文件句柄，后续同路径发布通过新的文件代际提供数据。

codec 1 制品采用 QVSSD v1，依次保存 160 字节目录头、既有 QUIV v2 热快照及冷记录区。中心化 codec 2／3 使用 QVSSD v2，在热快照与冷记录之间保存 `dimension × 4` 字节共享中心。头部偏移 108 保存中心 CRC32，偏移 152 保存中心字节数，40 字节行布局头中的编码身份分别为 1／2／3。头部 CRC 覆盖上述字段，中心在打开时验证，冷行在读取时验证。热快照包含默认 QDSM 路由状态，墓碑保存零冷行并跳过已复用 slot 的读取。构建与中心采样工作区均在分配前准入。

| 头部偏移 | 字段 | 字节数 |
| --- | --- | --- |
| 0 | Magic `QVSSD001` | 8 |
| 8 | 容器版本 1／2、头部长度 160 | 8 |
| 16 | 来源摘要 | 32 |
| 48 | 发布 UUID | 16 |
| 64 | 热区起点、热区长度、冷区起点、行数、记录步长 | 40 |
| 104 | 热区 CRC32、中心 CRC32（v1 为 0） | 8 |
| 112 | 40 字节压缩行布局头 | 40 |
| 152 | 中心字节数（v1 为 0）、头部 CRC32 | 8 |

每条冷记录由 32 字节身份前缀和完整压缩行组成。前缀依次保存 u64 NodeId、u64 slot、u64 内部行号、u32 墓碑标志及 u32 记录 CRC。记录 CRC 覆盖前 28 字节和完整压缩行，包含填充。读取时同时核对热区中的 NodeId／slot／墓碑、内部行号、记录 CRC 及编码行的完整校验。完整记录交换、跨节点或跨 slot 混入返回 `TDB_CORRUPTED_FILE`。

### 发布、读取与关闭

发布目标须为空路径或已有 QVSSD 普通文件，类型校验先于来源读取，并在原子替换前再次执行。发布采用同目录独占 UUID 临时文件、流式写入、文件同步、原子替换和父目录同步。构建或提交点前失败保留旧制品；重命名成功后目录同步失败返回 `publish/unknown`。并发发布采用最后完成原子替换的制品作为当前版本，各 Reader 固定自己的文件代际。宿主按预期来源打开，来源不匹配返回 `TDB_IMMUTABLE_ARTIFACT`。

打开阶段读取目录头、热区及共享中心，并在分配前核对来源、文件长度、区段几何及预算。冷区在行读取时校验。缓冲后端在 Windows 使用 `seek_read`、Unix 使用 `read_at`，并发请求各自携带行缓冲。Direct I/O 后端及其完成语义见下文。短读、I/O 错误和取消沿稳定错误码传播，精排错误结束本次请求。

`search` 复用默认 SM 导航、有效候选预算和 NodeId 同分规则，以所选编码和查询精度完成近似余弦重排，返回 `(NodeId, score)`。请求使用自有导航工作区，并在导航、冷读取前后及返回前检查请求控制。`close` 释放热图、文件和对应额度；已经取得的 `SsdRow` 继续持有其自有字节、中心及额度，最终释放时归还预算。

### 配额与使用入口

`SsdOptions.memory_limit_bytes` 对单次构建新增工作区或 Reader 的热区、在途查询及行租约总量进行准入，0 表示使用共享 Runtime 上限。每份额度同时登记到 Reader 局部账本和共享 `ResourceRuntime`，并发请求和调用方继续持有的行共同计量。`row_alignment` 在发布时决定编码行布局，Reader 采用制品中记录的布局。

加载采用两阶段准入，先预留热区读取缓冲，再根据 CRC 校验后的热区节点数和层数，预留解码数组、HashMap 及逐层 Vec 描述符。解码上界为 `3 × 热区字节数 + 行数 × (256 + 48 × 上层数) + 48 × 上层数 + 64 KiB`。完整校验后提取 SM 路由、底层邻接表和身份映射，释放 BQ 构图数据、上层邻接表及维护工作区，再按实际保留结构收缩额度。查询工作区按最坏全图候选队列准入，包含 `96 × 索引行数`、访问位图、单条冷记录和 64 KiB 固定工作区。冷读取另准入 4 KiB 批次目录及至多四条缺失记录的传输峰值。上述数字用于内核预算，进程 RSS 和 OS PageCache 单独测量。

```rust
use std::path::Path;
use triviumdb::index::quiver::{QuIVer, QuIVerConfig, QuIVerSearchConfig};
use triviumdb::index::quiver::ssd::{SsdOptions, SsdQuiver};
use triviumdb::{ResourceLimits, ResourceRuntime};

fn publish_and_query(path: &Path, source: [u8; 32]) -> triviumdb::Result<Vec<(u64, f32)>> {
    let vectors = [[1f32, 0.], [0., 1.]];
    let mut index = QuIVer::new(2, &QuIVerConfig::default());
    index.batch_build(&[1., 0., 0., 1.], &[1, 2], &[0, 1]);
    let runtime = ResourceRuntime::new(ResourceLimits::default())?;
    SsdQuiver::publish(&index, path, source, SsdOptions::default(), &runtime,
        |slot, output| { output.copy_from_slice(&vectors[slot]); Ok(()) })?;
    drop(index);
    let mut reader = SsdQuiver::open(path, source, SsdOptions::default(), runtime)?;
    let config = QuIVerSearchConfig { top_k: 2, ef_search: 2, rerank_limit: Some(2) };
    let hits = reader.search(&[1., 0.], &config)?;
    reader.close();
    Ok(hits)
}
```

### 数据库与 SDK 入口

数据库提供 `configure_quiver`、`search_quiver`、`quiver_stats`、`close_quiver` 和可写实例的 `collect_quiver_temporary`，旧 INT8 命名入口委托同一实现。Rust Reader 提供配置、查询、统计和关闭入口。Python 使用 `db.quiver_request(json)`，Node 使用 `db.quiverRequest(json)`，两端经过 `sdk::quiver` 调用同一 Core 契约。SDK 使用共享请求借用，状态维护和写权限由内核判定，冷查询期间可准入其他读取。

首次配置需要数据库包含浮点向量。可写实例按需构建图并 checkpoint，内核根据已确认的 dtype、事实文件与热图绑定元数据生成 SHA-256 来源身份，发布到 `<数据库路径>.qssd`。打开时在热区分配前同时核对来源身份、数据库热图长度及 CRC 绑定。数据库 Reader 以固定 64 KiB 工作区流式校验制品热区，并绑定主路由的运行期身份。后续向量写入与主路由重建由压缩查询、flush 或关闭 checkpoint 刷新派生绑定。只读实例配置已经发布且匹配当前事实的制品。读取、预算和来源失败沿结构化错误返回，调用方可在条件恢复后重试。

```python
import json

def request(command):
    return json.loads(db.quiver_request(json.dumps(command)))

request({"operation": "configure", "options": {
    "residency": "ssd", "cache_bytes": 64 * 1024 * 1024,
    "io_mode": "auto", "io_depth": 4
}})
hits = request({"operation": "search", "vector": query_vector,
                "top_k": 10, "ef_search": 128, "rerank_limit": 64})
request({"operation": "configure", "options": {
    "residency": "hot", "cache_bytes": 0, "io_mode": "auto", "io_depth": 4
}})
stats = request({"operation": "stats"})
request({"operation": "close"})
```

协议命令为 `configure`、`search`、`stats`、`close` 和 `collect_temporary`。搜索结果为 `{"hits":[{"id":"9007199254740993","score":1.0}]}`，ID 使用十进制字符串。`configure.options` 支持下面四项，省略时使用默认值，未知字段及非法枚举直接拒绝。

| 字段 | 默认值 | 契约 |
| --- | --- | --- |
| `residency` | `ssd` | `ssd` 按需读取，`hot` 预热全部所选编码记录 |
| `cache_bytes` | `0` | SSD Ready 行缓存的预算，含记录字节和每行 512 字节目录及租约额度 |
| `io_mode` | `buffered` | `buffered`、`auto`、`direct` |
| `io_depth` | `4` | `1..=32`，实际工作线程数受共享 Runtime 上限约束 |

### 缓存、驻留与 I/O 生命周期

数据库的内存估算与准入包含基础堆和 SSD Reader。构建及代际刷新同时覆盖旧 Reader 和候选状态，查询按当前基础堆占用更新 Reader 的可用额度。源代际变化采用全量冷记录重编码和发布。配置、预热和代际安装通过维护操作串行执行。

`routing_shared=true` 表示当前 Reader 使用数据库主路由，`reserved_hot_bytes` 统计 Reader 自有的固定账本开销。主路由及其 ID／slot 映射、墓碑和运行期身份在数据库基础堆中计量一次，均值、压缩记录、缓存与 I/O 工作区通过各自额度计量。独立 Reader 返回 `routing_shared=false`，其 `reserved_hot_bytes` 包含自有导航结构。主路由实例替换与向量代际分别检查，图重建完成后由下一次压缩查询确认新绑定。自有行租约只持有编码字节和中心，生命周期独立于主路由。

删除最后一个向量并完成 checkpoint 后，Reader 的 `closed` 统计变为 `true`，缓存、驻留行、I/O 后端和文件资源随之释放。运行时策略继续用于后续向量写入的自动刷新，空库 INT8 查询返回空集合。

缓存按行维护 LRU，命中返回共享行租约，同代际同一行的 Pending 请求合并读取。`cache_bytes=0` 保留在途合并能力。Reader 热区额度包含 4 KiB 固定目录，覆盖缓存账本、空树根及读取计数器。调小缓存时立即淘汰缓存引用，外部持有的行继续占用其真实额度。每行在分配前准入记录、目录和传输峰值，完成后收缩为持有额度。

最终重排按成功评分预算预取候选，过滤拒绝和墓碑在预取前排除。每个读取批次至多 64 行，批量 I/O 与评分 SIMD 分别派发。缓冲后端合并相邻记录，Direct I/O 后端同时合并相邻记录和重叠的实际对齐窗口，其余范围进入有界读取队列。批内请求先提交，再逐项等待实际完成并恢复输入顺序。每条记录独立验证身份、slot、墓碑和 CRC，查询持有本次预取行的租约。批次先完成自身持有的 Pending，再等待其他请求，预算拒绝、读取失败和取消均释放本批登记并唤醒等待者。

Hot 模式把所选编码的记录预热到内存，两种驻留策略采用相同评分。预热沿当前读取后端完成，成功后释放异步 I/O 后端、工作线程及其额度。`io_active` 表示当前异步后端是否已安装，Hot 状态为 `false`。切回 SSD 时重新准备请求的后端与额度，准备失败保留 Hot 状态及查询能力。切换成功后释放 Hot 持有，已取得的行租约按自身生命周期归还额度。转换与代际刷新需要旧状态和候选状态的峰值预算，资源不足时返回 `QueueFull`。

异步读取使用有界队列。Windows 后端通过 `ReOpenFile` 固定原文件代际，使用 `FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING`、独立事件及完成等待。Linux 后端使用同一已打开文件的 `/proc/self/fd` 路径、`O_DIRECT` 和内核 AIO，异常排空通过 `io_destroy` 等待全部未取消操作完成。线程、传输副本和对齐缓冲纳入 Runtime 准入。取消在实际 I/O 完成和缓冲归还后返回，关闭排空队列、join 工作线程，再释放文件与线程额度。[Windows 读取契约](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-readfile)、[Linux AIO 排空契约](https://man7.org/linux/man-pages/man2/io_destroy.2.html)。

Direct I/O 从固定代际的文件句柄查询对齐。Windows 使用文件存储扇区信息及文件内存对齐要求，Linux 使用 `statx(STATX_DIOALIGN)` 并结合文件系统块大小选择读取粒度。缺少查询能力时使用 64 KiB 保守对齐，再执行真实原生读取探测。当前后端支持最大 64 KiB 的请求和内存对齐，超出范围提供能力拒绝。跨窗口、文件末尾及大记录按逻辑长度返回完整内容。[Windows 存储信息](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_storage_info)、[Windows 对齐信息](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_alignment_info)、[Linux statx](https://man7.org/linux/man-pages/man2/statx.2.html)。

每个原生后端按实际请求大小分配并复用一个最多 64 KiB 的对齐读取窗口，后续请求完整落入有效内容时直接复制。窗口绑定该后端的文件句柄，重新读取前失效旧内容，短读和失败保留失效状态。更大的请求使用临时对齐缓冲，完成后归还。窗口及对齐余量计入每后端预留的 256 KiB I/O 工作区，该额度同时覆盖 128 KiB 线程栈。`cache_bytes` 控制 Ready 行缓存，设为零时仍可复用有界 I/O 窗口及合并 Pending。Hot 切换和关闭释放全部窗口。

I/O 后台线程为 CPU 维护保留一个共享线程额度。线程上限为 1 或共享 I/O 线程额度已用满时，请求线程执行原生提交和完成等待，图重建、刷新和关闭复用 CPU 工作线程。`auto` 对明确的平台／文件系统能力拒绝选择缓冲后端，`direct` 保留系统错误。`stats.io_mode` 返回实际后端，另提供缓存预算、缓存用量、命中、未命中、合并读取、冷区逻辑读取和总预留额度。

`cold_read_calls` 和 `cold_read_bytes` 统计已完成传输的逻辑记录请求及记录字节。`physical_read_calls` 统计冷区系统读取尝试，`physical_read_bytes` 统计系统实际返回的字节，包含对齐窗口和短读，排除打开热区及配置探测。`window_reuse_hits` 和 `window_reuse_bytes` 统计窗口命中的逻辑范围数及复制字节。缓冲后端可能由 OS PageCache 满足，设备侧流量通过系统或存储工具另行采集。Reader 重开后累计计数从零开始。

`io_alignment_bytes` 返回当前原生读取粒度，`io_memory_alignment_bytes` 返回缓冲对齐，`io_alignment_detected` 表示成功取得文件约束。缓冲模式或 Hot 状态的两项粒度为零。`io_window_bytes` 返回当前保留窗口的分配字节，数值受后端数乘以 64 KiB 约束，Hot 切换和关闭后归零。对齐填充及线程等资源通过总预留额度观察。

发布与回收使用 `<制品路径>.publish-lock` 管理租约。每个 UUID 暂存候选持有独立 `.tmp.lease`，活跃发布者继续工作。可写配置、发布和显式回收扫描同目标命名空间，仅删除已取得独占租约的遗留候选，普通完成和失败立即回收本次候选。进程强杀后的租约由 OS 释放，下次可写入口自动回收。只读入口只访问正式制品。

> **接入范围提醒**　`use_for_ann` 控制普通 ANN 入口的压缩重排选择。数据库入口共享主 QuIVer 路由，原始向量事实按存储模式保存。`.qssd` 为可重建派生制品，数据库快照保存已有事实文件组，恢复后由可写配置入口重建。只读配置要求已有匹配来源与编码的制品。独立低层宿主自行维护事实与制品的备份关系。CLI 和 Server 可通过 Core 适配此协议。

---

## 性能评估

召回率与吞吐取决于向量分布、图参数、搜索宽度和精排预算。应使用业务数据测量 Recall@K、QPS、P50/P95/P99 延迟以及构图、加载成本；对比时保持数据、度量、硬件与线程数一致。默认余弦查询与显式原生 L2 使用不同的评分语义，应分别生成精确 ground truth。

内存测量应区分索引热内存、原始向量和 Payload 冷层、OS PageCache 与临时工作区。基准入口及数据准备步骤见 [基准测试指南](../benches/README.md)。

---

## 检查与复现入口

源码入口：[QuIVer 构图与查询路由](../src/index/quiver.rs)、[SM 查询内核](../src/index/quiver/corrected_sm.rs)、[数据库生命周期](../src/database/mod.rs)。

```bash
cargo test --lib index::quiver::corrected_sm::production_tests
cargo test --lib index::quiver::corrected_sm::native_l2_tests
cargo test --lib index::quiver::tests::test_quiver_中等规模搜索 -- --exact
cargo test --lib index::quiver::connectivity_tests
RAYON_NUM_THREADS=1 cargo bench --features ablation --bench bench_quiver_connectivity
```

这些命令检查默认路由、预算、维度、增量/slot 复用、重载、原生 L2 和基础图查询。[连通性基准](../benches/bench_quiver_connectivity.rs)覆盖七组合成数据和四个查询宽度，运行时输出逐轮召回、耗时和热内存指标。可设置 `TRIVIUM_NO_AVX512=1` 验证禁用 AVX-512 的路径。

---

## 0.9.0-beta.3 检索与计量

BruteForce 使用分片有界 Top-K 堆，并按分数降序、外部 NodeId 升序合并。QuIVer 的 SM 与公开 BQ 检索入口统一采用外部 NodeId 处理同分；查询维度、有限值与冷向量视图按入口契约校验。写入准入直接读取增量维护的热内存计量，完整图统计用于诊断。详见[版本说明](changes/0.9.0-beta.3.md)。

---

## 索引事实绑定与 BQ 边界

`.quiver.meta` v2 使用 60 字节固定布局，记录文件长度、节点数、维度、QuIVer CRC、主文件与分离向量文件的 CRC，并校验元数据自身 CRC。主文件摘要覆盖节点身份和 slot 映射；加载后逐项核对活跃 ID 与冷向量 slot。相同文件长度下的节点替换、向量更新和旧索引混入均参与一致性检查。

v1 元数据通过既有 `MissingIndexPolicy` 处理：默认回退读取当前事实；严格模式返回 `ImmutableArtifactInvalid`。只读回退保留原 sidecar，可写实例可显式重建并随文件组发布更新后的索引与元数据。

Rust 底层 `Bq2Signature::distance` 及 `Bq2Store` 的 `distance`、`distance_to_sig`、`distance_to_sig_cheap`、`get_sig`、`from_raw` 返回 `Result`。调用方使用 `?` 处理索引、维度和平面几何错误。预取忽略无效行；便宜距离按有效维度屏蔽尾位。合法输入继续使用现有 SIMD/标量内核。
