# TriviumDB 索引与三模管线工程实施方案

> 版本：**v2 重整版** · 2026-08-31
> 本文档由旧版《index-and-tsng-engineering-plan.md》（2567 行）重整而来。旧文档保留为历史存档，不再维护。
>
> 重整原则：
> 1. **主线优先**。全文以「三模算子管线」为主线组织，属性/图索引地基与工业执行层作为主线的支撑章节。
> 2. **已落地优先于计划**。所有已完成阶段按「实现记录 + 验收数据」书写；未启动任务统一收敛到末尾执行清单。
> 3. **否证内容一笔带过**。C1 单队列导航实验已被 Matched-Recall Gate 否证，只保留结论存档与转向记录，不保留过程数据。
> 4. **数据可溯源**。所有性能数字均标注 benchmark 入口与报告文件。
> 5. **论文贡献边界**。QuIVer 已作为独立工作发表于 VLDB，是 TriviumDB 既有的高性能向量检索底座与产品卖点；本文档涉及的新论文不得再次将 QuIVer、BQ2 或其纯向量性能表述为新增贡献或单独投稿方向。
>
> 配套文档：《TriviumDB 演进路线图》（产品定位、文献竞争、学术发表路线）。

### 1.1.1 阅读路径

| 读者 | 建议路径 |
|---|---|
| 新 contributor | §一 → §二 → §三 → `benches/README.md` → §十一 |
| 索引/存储方向 | §一 → §三 → §四 → §五 → 附录 A.1/A.2 |
| 查询/优化器方向 | §一 → §三 → §七 → §八 → §十三 |
| 论文实验复现 | 附录 A → §十 → `benches/README.md` → §十二（验证口径） |
| 发布/CI 维护 | §十一 → §十二 → §十四 → §十五 |

### 1.1.2 与旧文档的关系

| 旧文档章节 | 新文档位置 | 处理 |
|---|---|---|
| §1–4 工程原则/基线 | §一、§二 | 合并重排 |
| §5–8 A1/A2/B1/B2 | §四、§五 | 补字节布局与算法细节 |
| §9–10 C0/C1 | §6.5 | **压缩为结论存档**（已否证） |
| §11–12 C2/C3 | §九 | **压缩为判据清单** |
| §13 A3/A4、§14 跨语言 | §4.3/4.4 | 展开为完成态 |
| §15 C1.5.1 | §六 | 数据保留，流程叙述精简 |
| §16 C1.6/C1.7 | §七、§八 | 主线章节，最详细 |
| §17–23 错误/测试/CI/Gate/DoD | §3.5、§十一–十五 | 保留并补充 |
| §21.x 实装结果（12 节） | 分散至各能力章节 + 附录 A | 去重合并 |

---

## 目录

```text
一、总纲
    1.1 主线声明 / 1.2 工程原则 / 1.3 工业线硬约束 / 1.4 高维优先与维度全兼容
二、产品能力现状
    2.1 索引与能力全景 / 2.2 持久化格式版本 / 2.3 代码模块地图 / 2.4 基线快照
三、公共接口契约
    3.1 属性索引门面 / 3.2 NodeSet / 3.3 PipelineOperator / 3.4 预算体系 / 3.5 错误模型
四、轨道 A：持久化属性索引
    4.1 A1 Hash / 4.2 A2 Ordered+ART / 4.3 A3 复合 / 4.4 A4 Bitmap / 4.5 .pidx 格式
五、轨道 B：图查询优化
    5.1 B1 Planner / 5.2 B2 预算与双向 BFS / 5.3 B3 直方图 / 5.4 .gidx 格式
六、工业查询执行层（C1.5/C1.5.1）
    6.1 六条 AccessPath / 6.2 字节预算 / 6.3 BQ 预筛与缓存 / 6.4 高维验收 / 6.5 已否证实验存档
七、三模管线（C1.6，主线）
    7.1 第一性原理 / 7.2 TQL WITH / 7.3 图算法子集化 / 7.4 标准 Leiden
    7.5 Cascades / 7.6 Q1–Q14 / 7.7 Q5 深挖 / 7.8 最终 Gate
八、查询内并行（C1.7）
    8.1 预算与确定性 / 8.2 第一批 / 8.3 第二批 / 8.4 大前沿 BFS / 8.5 确定性方法学
九、条件性后续任务（C2/C3/C4）
十、Benchmark 体系
十一、测试矩阵
十二、CI 分层与发布门禁
十三、Stop/Go Gate 总表
十四、版本里程碑与 DoD
十五、剩余执行清单
附录 A 性能数据总表 / 附录 B 测试文件清单 / 附录 C 术语表 / 附录 D 跨语言 API 面
```

---

## 一、总纲

### 1.1 主线声明

TriviumDB 的工程主线是：

> **基于原生统一索引和 SoA slot 寻址，在单一执行引擎内把向量检索/精排、属性索引、图遍历与图算法组合为有界、可优化、零拷贝候选传递的多阶段查询管线。**

四个支撑层：

| 支撑层 | 内容 | 状态 |
|---|---|---|
| 持久化元数据索引（轨道 A） | Hash / Ordered ART / Composite ART / Roaring Bitmap | ✅ 全部完成 |
| 图查询优化（轨道 B） | Planner v1 / TraversalBudget + 双向 BFS / 度数直方图 | ✅ 全部完成 |
| 工业查询执行层（C1.5/C1.5.1） | 六条 AccessPath、字节优先预算、internal-slot 缓存、高维 Gate | ✅ 全部完成 |
| 三模管线（C1.6/C1.7） | NodeSet 算子代数、TQL WITH 链、图算法子集化、Cascades Planner、查询内并行 | ✅ 全部完成 |

条件性后续任务（C2 导航期剪枝、C3 多信号构图、C4 自适应权重）不在主线内，判据见 §九。

### 1.2 工程原则

**1.2.1 不破坏已有正确性**

- 每个阶段必须携带 old-vs-new differential 测试，旧语义在新实现下逐字段一致。
- 纯向量（QuIVer）、纯属性（FIND）、单 MATCH 路径的结果与访问序列在任何优化后不得改变。
- 持久化格式只向前兼容：新版本必须能读旧 sidecar，旧版本读新 sidecar 时明确报版本错误而不是静默误读。

**1.2.2 不一次重写整个内核**

- 每个阶段是独立可合并、可回滚的增量；不允许「一套新执行器长期旁路旧 TQL」。
- 旧单入口 AST 通过 lowering 变成单阶段 pipeline，语义保留，执行器只有一套。

**1.2.3 索引只保存 NodeId/slot，不复制主数据**

- 所有属性索引 posting 只存 NodeId（内存态）或 internal slot（磁盘态），向量与 payload 永远只有一份。
- 这是内存有界的前提，也是 slot 复用安全设计的来源。

**1.2.4 先建立基准，再优化**

- 任何优化必须先在相同固定 Seed、相同规模、相同冷热协议下建立基线，优化后重跑对照。
- 延迟结论使用 P50/P95/P99；不以单次耗时作为结论。
- 优化必须证明结果不变（逐字段或稳定哈希），再证明变快。

**1.2.5 确定性是产品属性，不是测试便利**

- 相同输入、相同配置、相同线程数下输出逐位一致；浮点排序固定以 NodeId 打破平局。
- 并行路径的归并顺序固定（按 source、按 slot 升序），不依赖线程调度。
- 这条原则贯穿 A/B 轨道、C1.6 管线与 C1.7 并行。

### 1.3 工业线硬约束：内存有界、磁盘优先、SSD 低磨损

内存占用与 SSD 写放大和查询延迟、正确性同等级的 Gate，不允许作为后续优化补丁处理。

核心原则：

1. **冷数据尽量走硬盘**：原始向量、Payload、持久化 postings、ART/Bitmap 冷块优先保持在只读 sidecar/mmap 中，查询时按页读取；不得默认反序列化成第二份全量 Rust 堆对象。
2. **热数据只保留导航必要集合**：QuIVer BQ2、Vamana 邻接、Fast Tags、顶层索引目录、块级 offset/CRC、少量统计和活跃查询 scratch 常驻内存。
3. **只读 mmap 不等于 SSD 磨损**：page fault 和文件读取只产生读 I/O；磨损风险来自 WAL、checkpoint、sidecar 全量重写、临时 spill、压缩和 pagefile 写入。
4. **不以写放大换内存**：禁止把 CandidateSet 高频 spill 到临时文件作为正常执行路径；禁止每次 CRUD 重写整个 `.pidx`；禁止后台周期性无条件重建索引。
5. **不可变代 + 原子发布**：磁盘索引采用 append/build-new-generation → fsync → 原子切换 current → lease 安全回收旧代；更新先进内存 delta/WAL，达到大小或显式 flush 条件后批量合并。
6. **避免双份 PageCache**：mmap 数据不再复制到长期 Vec/HashMap；顺序/随机访问使用 `advise_sequential/random`，阶段结束 `advise_dontneed` 释放冷页。
7. **预算先于执行**：Planner 必须同时估算候选数、临时内存、精排向量读取量和预计磁盘页数；超预算时选择更节省内存的 AccessPath，不能先分配再 OOM。
8. **索引按需创建**：同一字段已有 Ordered Index 时默认不再建 Hash Index；低基数字段优先 Bitmap；高基数一次性字段不建驻留索引。

冷热布局约定：

| 数据 | 默认位置 | 写入策略 | 说明 |
|---|---|---|---|
| BQ2 + Vamana 热导航 | 内存/可 mmap 热页 | 构建时批量写 | 纯向量性能底线 |
| 原始向量 | mmap 冷文件 | append + generation flush | 只对 rerank 候选读取 |
| Payload | mmap 主文件 | WAL + 批量 flush | 避免全库 JSON 反序列化 |
| Hash/ART postings | mmap 不可变块 + 小型内存 delta | 批量 merge | 不加载全量 `Vec<NodeId>` |
| ART 上层目录/前缀 | 内存 | generation 发布 | 定位磁盘叶块 |
| Roaring Bitmap | mmap serialized containers | 批量重写受影响容器 | 低基数集合运算 |
| Graph adjacency | mmap 块 + 热 hub cache | append/批量 compact | 避免三份完整边对象常驻 |
| CandidateSet | 查询内存 | 有界、消费式 union/intersection | 默认不落临时盘 |
| 统计/直方图 | 内存 | flush 时更新 | 小而热 |

SSD 磨损 Gate：

- benchmark 报告必须记录 logical bytes written、physical/estimated sidecar bytes written、WAL bytes、checkpoint bytes、temporary spill bytes 和 write amplification。
- 普通只读查询的数据库写入必须为 **0 bytes**。
- 单条 CRUD 只允许 WAL + 小型 delta，不允许 O(index_size) sidecar 重写。
- 自动 flush/merge 必须有最小 delta 大小和冷却间隔；数据库空闲本身不能触发周期写入。
- ReadOnly/Immutable 必须做到字节级零写入。
- 禁止依赖 pagefile 作为索引内存管理策略；逼近预算前卸载冷索引页或拒绝高风险计划。

### 1.4 高维优先与维度全兼容

**目标场景**：384 维以上文本 embedding（768–3072 覆盖绝大多数生产负载）。QuIVer 本身就是放弃低维通用性换取高维极端性能的设计。因此：

- 所有 Gate 判据以高维为默认场景，低维只作兼容性验证。
- 只在 64 维小规模数据上得出的结论，不足以否决或确立任何算法路线。

**维度全兼容三原则**：

1. **字节是唯一的一等预算单位**。候选阈值必须由 `bytes / (dim × size_of::<T>())` 推导，不得硬编码固定计数。
2. **不得存在维度分档的 if-else 策略选择**。384 维 SIMD 专用 kernel 属于同语义性能特化，允许；改变查询策略、候选规模或 Recall 行为的维度分支，禁止。
3. **维度参数化测试是硬性要求**。每个新增内存/候选/页读取约束必须在 384/768/1536/3072 验证行为一致且不超预算。

计数阈值在维度间的真实字节差异（历史缺陷的反面教材）：

| 维度 | 100K 精排向量 | 1000 直接候选 |
|---:|---:|---:|
| 384 | 153 MB | 1.5 MB |
| 768 | 307 MB | 3.0 MB |
| 1536 | 614 MB | 6.0 MB |
| 3072 | **1.23 GB** | 12 MB |

同一组计数在 3072 维静默使用 1.23 GB 查询临时内存、在 384 维过度保守。该缺陷已由 C1.5.1 Step 0 修复（§6.2）。

---

## 二、产品能力现状

### 2.1 索引与能力全景

| 类型 | 结构 | 持久化 | 复杂度 | 状态 |
|---|---|---|---|---|
| QuIVer 向量 ANN | BQ2 签名 + Vamana 图 | sidecar | O(log N) 近似 | ✅ |
| Payload 属性（Hash） | HashMap 等值倒排 | `.pidx` v1–v4 | O(1) 等值 | ✅ A1 |
| Payload 属性（Ordered） | ART 有序倒排 | `.pidx` v2+ | O(key_len + K) 范围 | ✅ A2 |
| Payload 属性（Composite） | 复合键 ART | `.pidx` v4 | O(prefix + K) | ✅ A3 |
| Payload 属性（Bitmap） | RoaringTreemap posting | `.pidx` v4 | O(containers) | ✅ A4 |
| BM25 文本 | 2-Gram 倒排 | sidecar | O(terms × docs) | ✅ |
| AC 关键词 | Aho-Corasick 自动机 | sidecar | O(text_len) | ✅ |
| 出边邻接表 | HashMap\<NodeId, Vec\<Edge\>\> | 主文件 | O(度数) | ✅ |
| 反向入边 | HashMap\<NodeId, Vec\<NodeId\>\> | 主文件 + `.gidx` v2 目录 | O(度数) | ✅ |
| 边标签倒排 | HashMap\<String, Vec\<(src, dst)\>\> | 主文件 + `.gidx` v2 目录 | O(匹配边数) | ✅ |
| NodeId ↔ slot | 双向映射 | 主文件 | O(1) | ✅ |
| Fast Tags | 64-bit 行级签名 | 主文件 | O(1) 预过滤 | ✅ |
| Planner + AccessPath | 选择性统计 + 自动路由 | — | — | ✅ B1 |
| TraversalBudget + 双向 BFS | 预算 + 双端搜索 | — | O(√V) 最短路径 | ✅ B2 |
| GraphStats + 度数直方图 | generation-aware 缓存 | — | O(1) 热读 | ✅ B2/B3 |
| 工业混合搜索 AccessPath | 六路径自动路由 | — | — | ✅ C1.5 |
| NodeSet 管线 + Cascades | 算子代数 + 跨阶段优化 | — | — | ✅ C1.6 |
| 查询内并行 | Rayon + 确定性归并 | — | — | ✅ C1.7 |

### 2.2 持久化格式版本总表

| 格式 | 版本 | 引入内容 | 兼容策略 |
|---|---|---|---|
| `.pidx` | v1 | Hash posting 基础布局 | 当前可读 |
| `.pidx` | v2 | 每索引 kind 标记（Hash/Ordered） | 当前可读 |
| `.pidx` | v3 | posting block 长度 + 逐块 CRC + RO mmap | 当前可读 |
| `.pidx` | v4 | Composite（kind 2）与 Bitmap（kind 3） | 当前写入版本 |
| `.gidx` | v1 | 源节点分块、逐块 CRC、惰性出边解码 | 当前可读 |
| `.gidx` | v2 | 反向源目录 + 标签热目录独立持久化 | 当前写入版本 |

所有格式共性：magic + version 头、整文件 CRC、逐块 CRC（v3+/v2+）、checked offset/count 解析、逐字节截断 fail-closed、`.tmp` 写入 → `sync_all` → 原子替换。

### 2.3 代码模块地图

```text
src/
├── index/
│   ├── property.rs      # PropertyIndexRegistry：Hash/Ordered/Composite/Bitmap 统一门面
│   ├── art.rs           # Safe Rust ART（Node4/16/48/256 + 路径压缩 + prefix scan）
│   └── quiver.rs        # QuIVer：BQ2 + Vamana + NavigationScorer
├── storage/
│   ├── memtable.rs      # SoA 布局、索引生命周期、GraphStats 缓存
│   └── graph_blocks.rs  # .gidx v2 mmap 边块 + 目录
├── query/
│   ├── planner.rs       # NodeAccessPlan / AccessPath 选择
│   ├── cascades.rs      # Memo 优化器、直方图 fanout
│   ├── pipeline.rs      # NodeSet / PipelineOperator / 算子实现
│   ├── tql_executor.rs  # TQL lowering 与执行
│   └── parallel.rs      # QueryParallelismBudget + 复用线程池
├── graph/
│   ├── traversal.rs     # BFS/双向 BFS/SA-PPR（确定性累加）
│   ├── reachability.rs  # 预算化可达性 + 并行 bitmap BFS
│   ├── subset.rs        # 子集化 PageRank/WCC/degree/betweenness/LP
│   └── leiden.rs        # 标准多层 Leiden
└── tsng.rs              # 工业混合搜索：六条 AccessPath + 预算 + 观测
```

模块职责与依赖方向：

| 模块 | 职责 | 依赖 | 禁止 |
|---|---|---|---|
| `index/property.rs` | 四类属性索引注册、CRUD 同步、持久化 | `index/art`、`storage` | 不读向量、不知道查询 |
| `index/art.rs` | 纯键值有序结构 | 无（叶子模块） | 不含业务语义 |
| `index/quiver.rs` | 向量 ANN、BQ2、Vamana、NavigationScorer | `storage`（slot） | 不读 payload 业务字段 |
| `storage/memtable.rs` | SoA 数据、索引生命周期、GraphStats | `index`、`storage/graph_blocks` | 不含查询计划逻辑 |
| `storage/graph_blocks.rs` | `.gidx` mmap 解析与目录 | 无 | 不做图算法 |
| `query/planner.rs` | 单阶段 AccessPath 选择 | `storage` | 不执行算子 |
| `query/cascades.rs` | 跨阶段 Memo 优化 | `query/pipeline`、`storage` | 不改变语义只重排 |
| `query/pipeline.rs` | NodeSet 与全部算子实现 | `graph`、`index`、`storage` | 不解析 TQL 文本 |
| `query/tql_executor.rs` | 文本 → AST → 管线 lowering | `query/*` | 不含优化决策 |
| `graph/*` | 图算法基元（可独立调用） | `storage` | 不依赖查询层 |
| `tsng.rs` | 混合搜索六路径 + 预算观测 | 全部上述 | 不参与建图 |

依赖方向自上而下单向；`graph` 与 `index` 互不依赖，均只面向 `storage`。这保证图算法基元可被管线复用，也可被旧 API 独立调用。

### 2.4 基线快照（v0.8.2 → A1）

统一进程级 runner `bench_index_graph_baseline`。环境：Windows x86_64、release + LTO、Seed `0x5452495649554DDB`、100K 节点、平均出度 4、50 次查询 5 次预热、数据库约 71.1 MB。

轨道 A 属性查询 100K P50：

| 场景 | 全扫描 | 内存 Hash Index | 加速 |
|---|---:|---:|---:|
| 等值 20% + LIMIT 100 | 23.308 ms | 5.747 ms | 4.1× |
| 等值 1% + LIMIT 100 | 26.278 ms | 0.211 ms | 124.5× |
| 等值 0.01% | 22.729 ms | 0.0124 ms | 1833× |
| AND 条件 + LIMIT 100 | 26.608 ms | 0.209 ms | 127.4× |
| 四字段索引构建 | — | 167.678 ms | — |

基线阶段发现的两个关键缺陷（均已修复）：

1. `FIND` 执行路径直接扫描 `all_node_ids()` 不调用属性索引；正式属性基线改用零边 `MATCH` 走 `find_tql_candidates()`。
2. 索引命中但值不存在时返回 `None` 被解释为「无法使用索引」退化全扫描；A1 后负命中 P50 从 22.772 ms 降至 0.0017 ms（约 13,395×）。

A1 实装后同口径对比：各场景变化均在 ±6% 内（微秒噪声级），负命中修复为唯一大项，图查询热路径无污染。

轨道 B 基线（100K P50/P95/P99）：

| 场景 | P50 | P95 | P99 | 平均返回量 |
|---|---:|---:|---:|---:|
| 原生 BFS 1 跳 | 0.0019 ms | 0.0056 ms | 0.0063 ms | 23.8 |
| 原生 BFS 2 跳 | 0.0148 ms | 0.0240 ms | 0.0304 ms | 152.1 |
| 原生 BFS 5 跳 | 0.8273 ms | 2.9067 ms | 3.2147 ms | 7080.1 |
| 原生 BFS 10 跳 | 44.166 ms | 46.278 ms | 46.951 ms | 99997.6 |
| 标签过滤 BFS 5 跳 | 0.0029 ms | 0.0032 ms | 0.0034 ms | 20.0 |
| TQL 主键起点 1 跳 | 0.0489 ms | 0.0789 ms | 0.0802 ms | 23.8 |
| TQL 属性索引起点 1 跳 | 0.0885 ms | 0.0993 ms | 0.1310 ms | 100.0 |
| TQL 主键 2 跳 + LIMIT 100 | 0.2352 ms | 0.2517 ms | 0.2706 ms | 100.0 |

结论：局部邻域微秒级，10 跳遍历几乎全图 P50 跃升至 44 ms——为 B2 预算、双向 BFS 与起点重排提供了明确靶点。

报告：`target/bench-reports/index-graph-baseline-100k.json`、`index-graph-track-a-100k.json`。

---

## 三、公共接口契约

### 3.1 属性索引统一门面

```rust
pub struct PropertyKey(Vec<u8>);          // 类型稳定编码，标量 only

pub struct PropertyIndexRegistry {
    indexes: HashMap<String, HashPropertyIndex>,           // 等值
    ordered_indexes: HashMap<String, OrderedPropertyIndex>, // 范围/排序
    composite_indexes: HashMap<CompositeDefinition, CompositePropertyIndex>,
    bitmap_indexes: HashMap<String, BitmapPropertyIndex>,  // 低基数
    mapped: Option<Arc<MappedPostingStore>>,               // mmap posting 视图
}
```

统一生命周期入口（MemTable 层）：

- `insert/remove` 在 CRUD 时同步维护四类索引 posting。
- 空 posting 删除 key；tombstone 槽位复用不产生幽灵命中。
- 批量注册（`register_hash/ordered/composite/bitmap`）用于建索引时全量回填。
- `drop_*` 系列安全下线并释放内存计量。

### 3.2 NodeSet：算子间唯一交换格式

```rust
pub struct NodeSet {
    pub ids: CandidateSet,
    pub columns: ScoreColumns,
    pub provenance: Provenance,
}

pub enum CandidateSet {
    SortedIds(Vec<NodeId>),
    Slots(RoaringBitmap),                          // 绑定 slot generation
    LazyIntersection(Box<CandidateSet>, Box<CandidateSet>),
    LazyUnion(Box<CandidateSet>, Box<CandidateSet>),
}
```

契约：

- NodeId/slot 有序去重；slot bitmap 绑定 generation，防 ABA/幽灵命中。
- `ScoreColumns` 是命名列集合，可同时携带 similarity、PageRank、centrality、depth、path strength、community；禁止覆盖匿名 score。
- `Provenance` 记录来源算子、近似/精确状态、向量模型/查询标识、generation；近似值不得冒充精确值。
- union/intersection 优先惰性或消费式执行，默认禁止全量中间物化与临时 spill。
- 每列与 NodeId 行对齐；删除、过滤、去重后同步压缩。

`ScoreColumns` 携带的命名分数种类：

| 列名 | 产生算子 | 语义 |
|---|---|---|
| `similarity` | SEARCH / RANK / exact rerank | 查询向量与节点原始向量的精确余弦相似度 |
| `graph_score` | EXPAND / SA-PPR | 图信号分（可达性/扩散），绑定 depth |
| `pagerank` | PageRankOperator | 诱导子图 PageRank 值 |
| `centrality` | DegreeCentralityOperator / BetweennessOperator | 归一化中心性 |
| `depth` | EXPAND | 最短跳数 |
| `path_strength` | ALL_PATHS | 路径边权聚合 |
| `community_id` | LabelPropagationOperator / LeidenOperator | 稳定社区编号 |

`Provenance` 逐行记录的最小字段：

```rust
pub struct Provenance {
    pub source_ids: Vec<NodeId>,        // 该行的产生来源（EXPAND 源）
    pub stage: u32,                     // 产生阶段序号
    pub score_kind: ScoreKind,          // Exact / Approximate / DepthBounded
    pub generation: u64,                // MemTable generation，防陈旧 slot
}
```

行对齐规则：过滤保留行时同步压缩所有列；去重合并行时分数按算子声明的聚合策略（min depth、sum score、first source）确定，不允许静默丢弃。

### 3.3 PipelineOperator 与上下文

```rust
pub trait PipelineOperator {
    fn apply(&self, input: NodeSet, ctx: &mut QueryContext) -> Result<NodeSet>;
    fn estimate(&self, input: &SetStats, ctx: &PlanContext) -> OperatorCost;
}
```

`QueryContext` 包含：当前 generation 只读视图、统一 `QueryBudget`（字节 + TraversalBudget + 最大阶段数/迭代数）、命名向量查询、逐阶段 metrics 与 Error/Partial 状态。

`OperatorCost` 输出：estimated rows、temp bytes、vector/payload/graph page reads、CPU cost、selectivity、是否需要物化。

新增图算法只需实现此 trait 即自动进入组合框架，不修改查询层。

### 3.4 三套预算的统一语义

| 预算 | 结构 | 管辖 |
|---|---|---|
| 查询内存 | `QueryMemoryBudget`（字节优先） | 候选 ID 字节、union 字节、精排向量字节、预计页读取 |
| 遍历 | `TraversalBudget` | max_visited_nodes / max_examined_edges / max_frontier_size / max_depth，配 `BudgetExhaustionPolicy::{Error, Partial}` |
| 并行 | `QueryParallelismBudget` | max_threads（0=自动，硬上限 64）/ min_parallel_rows（低于阈值保持串行） |

原则：预算在分配前检查；任一 worker 超限后整算子 fail-closed，不返回竞态 partial；上游不得吃光下游预算，Planner 按阶段切片。

### 3.5 错误模型

```rust
PropertyIndexCorrupted { path, reason }
PropertyIndexUnsupportedVersion { found, current }
PropertyIndexNotFound { name }
PropertyIndexAlreadyExists { name }
UnsupportedIndexValue { field, kind }
TraversalBudgetExceeded { visited, edges, depth }
TsngUnsupportedQuery { reason }
TsngInvalidWeights { reason }
TsngInvalidAnchor { reason }
TsngPathBudgetExceeded { paths, limit }
QueryMemoryBudgetExceeded { requested_bytes, limit_bytes }
```

要求：Python/Node 稳定映射；错误文本中英双语；不以字符串匹配驱动控制流。

---

## 四、轨道 A：持久化属性索引

### 4.1 A1 持久化 Hash Index ✅

数据结构：`HashMap<PropertyKey, Vec<NodeId>>`，键为类型稳定编码（数字/字符串/布尔标量；数组/对象不进入索引）。

键编码规则（类型稳定，跨重启不变）：

| JSON 类型 | 编码 | 可索引 |
|---|---|---|
| 正/负整数 | 类型标记 + 长度 + 大端补码 | ✅ |
| 无符号大整数 | 同上，统一补码空间 | ✅ |
| 浮点 | total-order 编码（符号位翻转保证全序） | ✅ |
| 字符串 | 类型标记 + 长度 + UTF-8 字节 | ✅ |
| 布尔 | 类型标记 | ✅ |
| null / 数组 / 对象 | — | ❌ 安全回退扫描 |

设计意图：`-0.0`、`0`、`0.0` 等跨类型相等值共享同一键，避免同值不同表示分裂 posting；这也让 A2 的数字范围顺序天然成立。

MemTable 侧迁移：索引注册表从散落的 per-field HashMap 收敛为 `PropertyIndexRegistry`，统一注册、CRUD 同步（insert/update/delete 走同一入口）、批量回填（建索引时全量扫描 payload 一次性注册）与 `drop` 下线。

`.pidx` sidecar 要点：

- 独立文件，固定 magic + version + 主文件大小/CRC + 节点数 + 文件 CRC。
- `.tmp` 写入 → `sync_all` → 原子替换 → 父目录同步。
- ReadWrite/ReadOnly/Immutable 三态加载；Immutable manifest 纳入 `.pidx`。
- 旧库缺 sidecar 兼容为无索引打开；损坏时 Fallback 无索引打开，Error 模式明确拒绝。

验收（100K/50 次/固定 Seed）：

- 负命中从全扫描 22.772 ms 降至 0.0017 ms（约 13,395×）。
- 稳定类型键与 Registry 未造成查询/构建退化（±6% 内）。
- `list_indexes()` 按字段名稳定排序。

测试覆盖：稳定键类型隔离、浮点零、重启恢复、CRUD、槽位复用、旧库缺 sidecar、CRC/截断损坏、逐字节截断、ReadOnly/Immutable 零副作用、2K 节点多选择性 differential。

### 4.2 A2 Ordered Index（BTreeMap → ART）✅

两阶段策略：先用 `BTreeMap<PropertyKey, SortedIds>` 冻结语义与接口，再在相同接口下替换为自研 ART，differential 保证等价。

A2a（BTreeMap 语义版）要点：

- 数字统一转可比较 f64 total-order 键；正负整数、无符号、浮点共享数值顺序；字符串 UTF-8 字节字典序。
- Null/Bool/数组/对象不进入 Ordered Index，无法索引谓词安全回退全扫描。
- `$gt/$gte/$lt/$lte` 数字与字符串范围候选；`ORDER BY` 同字段升降序迭代；安全 FIND 场景 `LIMIT + OFFSET` 下推到迭代器。
- BTreeMap 双端迭代，不提前物化整个键范围。

A2b/A2b.1（真实 ART）要点：

- Safe Rust，路径压缩 + Node4/16/48/256 + occupancy bitmap；Node48 用 256-byte index + 48 slots；Node256 直接 child slots。
- 100K 随机操作 + 1000 边界与 BTreeMap differential 通过；`ArtMap::get()` 不可变查找与 `prefix_values()` 前缀扫描为 A3 服务。
- `.pidx` v2 每索引记录 kind；加载器兼容 v1。

ART 节点布局设计（自研可控演进的落点）：

| 节点类型 | 容量 | 布局 | 升级阈值 |
|---|---:|---|---|
| Node4 | 4 | 排序 key 数组 + 4 child 指针，线性扫描 | 满 4 → Node16 |
| Node16 | 16 | 排序 key 数组（SIMD 友好连续字节）+ 16 指针 | 满 16 → Node48 |
| Node48 | 48 | 256-byte 直接索引表（value=slot 或 0xFF 空）+ 48 slots | 满 48 → Node256 |
| Node256 | 256 | 256 个直接 child 槽位，无索引表 | 不再升级 |

路径压缩：内部节点保存压缩前缀字节，跳过单链节点；长前缀字符串 workload（UUID、URL）因此显著省内存。occupancy bitmap 用于 Node48/256 的快速空槽定位与计数。

不采用 B+Tree 的原因：页式 B+Tree 需要页管理、分裂合并与页缓存，与 mmap sidecar 的不可变块 + 内存 delta 模型不匹配；ART 天然支持前缀扫描（A3 复合索引的左前缀依赖此能力），且键长可变无需定长页。

100K 性能（A2a 语义，50 次 5 预热）：

| 场景 | 全扫描 P50 | A2a P50 | 加速 |
|---|---:|---:|---:|
| 范围约 1% | 27.781 ms | 1.192 ms | 23.3× |
| 高命中范围 + LIMIT 100 | 0.299 ms | 0.0849 ms | 3.5× |
| 全范围 ORDER BY DESC LIMIT 100 | 25.925 ms | 0.0903 ms | 287× |
| 构建 100K 单字段 | — | 78.150 ms | — |

ART 与 BTreeMap 对比（100K）：

| 场景 | BTreeMap | ART |
|---|---:|---:|
| 构建 | 78.2 ms | 108.8 ms |
| 1% 范围 | 1.19 ms | 1.64 ms |
| LIMIT 100 | 0.085 ms | 0.093 ms |
| ORDER BY DESC LIMIT | 0.090 ms | 0.095 ms |

ART 当前未超过 BTreeMap，但提供自研可控演进基础（arena 分配、批量自底向上构建、长前缀字符串 workload 是后续发挥点）。ORDER BY/LIMIT 是最大收益：从全量输入排序降为从树尾直接读 100 个候选。

报告：`index-graph-track-a2-10k.json`、`index-graph-track-a2-100k.json`。

### 4.3 A3 复合索引 ✅

复合键编码（类型稳定，长度前缀 + 字节）：

```rust
fn append_composite_part(bytes: &mut Vec<u8>, value: &Value) -> Option<()> {
    let key = PropertyKey::from_json(value)?;
    bytes.extend_from_slice(&(key.0.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&key.0);
    Some(())
}
```

能力：

- `create_composite_index(&["tenant", "kind", "state"])`，至少两字段。
- 最长左前缀匹配：给定前缀等值可复用索引扫描。
- 完整等值：所有复合字段提供等值时 exact lookup（ART `get()`）。
- 最后一列范围：前缀确定后对末列做范围扫描 + LIMIT。
- CRUD/删除/slot 复用/重启/统计/内存计量全生命周期接通。
- Planner 在没有单字段索引时也能独立提取全部 Eq 并发现复合 AccessPath。

Planner 选择序（等值字段可用时）：Composite exact → Bitmap intersection → Hash/Ordered posting 交集 → 全扫描。

验收（`bench_indexes_and_leiden`，100K，Criterion 10 samples）：

| Gate | 实测 |
|---|---:|
| Composite exact | **1.823 µs** |

### 4.4 A4 Roaring Bitmap 低基数索引 ✅

结构：`HashMap<PropertyKey, roaring::RoaringTreemap>`，posting 为压缩位图，适合 type/status/category 等低基数字段。

能力：

- `create_bitmap_index(field)`；posting 用 `insert/remove` 增删，空 posting 删 key。
- 多条件集合代数：可完全索引化的 Eq/In/And/Or/Ne/Nin 直接位图运算产出计划；无法完整索引化时安全回退扫描。
- `.pidx` v4 将 Bitmap 转稳定 NodeId 列表落盘，重启恢复为 RoaringTreemap。
- 更新/删除无幽灵命中；ReadOnly/Immutable 零副作用。

验收（`bench_indexes_and_leiden`，100K）：

| Gate | 实测 |
|---|---:|
| Bitmap OR（两值并集） | **1.156 ms** |

### 4.5 `.pidx` 格式规范汇总

v4 当前布局（概念）：

```text
magic "TPIX" | version u16 = 4
主文件 size + CRC | node_count | 文件 CRC
for each index:
    kind u8        # 0=Hash 1=Ordered 2=Composite 3=Bitmap
    field names    # Composite 为 \0 分隔多字段
    entries        # Hash/Ordered/Composite 为 ART 可恢复键集 + posting
                   # Bitmap 为稳定 NodeId 列表
    per-entry: posting block len + CRC   # v3 起
```

加载器行为：v1/v2/v3 兼容读取；未知 version 明确报错；逐块 CRC 失败 fail-closed；checked 计数上界防伪造超大分配。

专项验收 9 项：v4 往返、v1–v3 兼容、CRC 损坏、逐字节截断、Composite/Bitmap CRUD 与 slot 复用、重启恢复、ReadOnly 零写、内存计量、list/drop API。

写入与原子发布流程：

```text
save:
  1. 序列化全部注册索引到 .pidx.tmp（逐块长度+CRC 边写边算）
  2. fsync(.pidx.tmp)
  3. 原子 rename(.pidx.tmp → .pidx)
  4. fsync(父目录)                      # 保证 rename 持久
load:
  - header 校验 → 逐块 CRC → 结构化重建 Registry
  - 任一步失败：ReadOnly → 明确报错；ReadWrite → Fallback 无索引打开 + 警告
RW 增量：
  - 不重写整文件；写操作进内存 delta，flush/close 时一次合并
```

版本兼容测试矩阵（`tests/pidx_persistence.rs`）：

| 用例 | 期望 |
|---|---|
| v1 文件 → 当前 | Hash 索引恢复，Ordered/Composite/Bitmap 为空 |
| v2 文件 → 当前 | Hash + Ordered 恢复 |
| v3 文件 → 当前 | 全恢复 + mmap posting 可用 |
| v4 文件 → 当前 | 全能力恢复 |
| v5（伪造）→ 当前 | `PropertyIndexUnsupportedVersion` 拒绝 |
| 任一版本逐字节截断 | fail-closed，不 panic、不部分加载 |

---

## 五、轨道 B：图查询优化

### 5.1 B1 Planner v1 ✅

`src/query/planner.rs` 统一定义 AccessPath：

```rust
pub enum AccessPath {
    PrimaryKey { id: NodeId },
    PropertyIndex { field },
    OrderedPropertyIndex { field, descending },
    CompositePropertyIndex { fields },
    BitmapPropertyIndex { fields },
    PropertyIndexIntersection { fields },
    EdgeLabelIndex { labels },
    FullNodeScan,
}
```

实现范围：

- FIND 与 MATCH 共享访问路径规划。
- AND 等值按候选数量、字段名字典序确定性选择；有序双指针求交。
- 单链、固定一跳、非 OPTIONAL MATCH 在末端存在过滤约束时比较正反方向估算行数，安全反转节点/边顺序与方向。
- 可变长路径与 OPTIONAL MATCH 禁止反转；OPTIONAL 无起点过滤时禁止标签下推，保持左外连接每左节点至少一行。
- 先估算，只有最终选中 `FullNodeScan` 才物化全节点候选，避免双向估算复制 100K NodeId。
- `PropertyIndexStats`：field、entry_count、distinct_count、null_count。
- EXPLAIN 输出结构化 access_path、estimated_rows、reversed、索引统计；EXPLAIN ANALYZE 补 actual_rows、elapsed_ms。

MATCH 起点反转的安全门禁：

| 模式 | 反转 | 标签下推 | 理由 |
|---|---|---|---|
| 固定一跳单链、末端有过滤 | ✅ 比较正反估算 | ✅ | 结果集等价，仅遍历方向变 |
| 可变长路径 `*1..3` | ❌ | — | 反向遍历的最短深度语义不等价 |
| OPTIONAL MATCH | ❌ | 无起点过滤时 ❌ | 左外连接语义要求每个左节点至少一行 |
| 多目标/复杂形状 | ❌ | — | 反转需要形状级等价证明，暂不冒险 |
| 估算并列（两侧候选相同） | 取原始方向 | — | 确定性 tie-break |

AND 等值交集算法：

```text
按 estimated_rows 升序取 posting（并列按字段名字典序）
result := posting(最小者)
for 其余 posting（已排序）:
    result := 有序双指针求交(result, posting)
    if result 为空: 提前终止
```

posting 恒为有序 NodeId 序列（Registry 维护时排序），交集因此 O(n₁+n₂) 且结果天然有序，无需再排序。

性能（固定 Seed）：

| 场景 | 规模 | 旧路径 P50 | B1 P50 | 结果 |
|---|---:|---:|---:|---|
| 高选择性终点、Planner 反转 + 属性索引 | 10K | 14.324 ms（全扫描） | 0.0163 ms | 约 879× |
| 同类未索引终点路径 | 100K | 超预算失败 | 0.120 ms | 从失败变为可完成 |
| 主键起点一跳 | 100K | 0.0499 ms | 0.0738 ms | +0.024 ms 规划成本 |
| 属性起点一跳 | 100K | 0.0870 ms | 0.1354 ms | +0.048 ms |
| 主键起点两跳 | 100K | 0.2302 ms | 0.3344 ms | +0.104 ms |
| EXPLAIN ANALYZE 选择性终点 | 100K | — | 0.6816 ms | 含执行与 JSON 计划 |

收益集中在「过滤条件位于路径末端」的查询；普通已优化 MATCH 增加几十微秒固定规划成本（后续可用 AST/计划缓存消除，当前不引入全局缓存复杂度）。

报告：`index-graph-track-b1-10k.json`、`index-graph-track-b1-100k.json`。

### 5.2 B2 TraversalBudget 与双向 BFS ✅

- 四维 `TraversalBudget`：visited nodes / examined edges / frontier size / depth；`BudgetExhaustionPolicy::{Error, Partial}`。
- 确定性双向 BFS：`ShortestPathOutput` + parent map 路径重建；800 组随机图最短路径 differential 通过。
- `GraphStats` generation-aware 缓存，热读取 < 1 μs。

`TraversalBudget` 语义表：

| 维度 | 检查时机 | Error 策略 | Partial 策略 |
|---|---|---|---|
| max_visited_nodes | 每次节点入队前 | 返回结构化错误 | 返回已得结果 + truncated 标记 |
| max_examined_edges | 每次邻接扫描累计后 | 同上 | 同上 |
| max_frontier_size | 每层开始前 | 同上 | 同上 |
| max_depth | 层推进前 | 同上 | 同上 |

双向 BFS 要点：

- 交替扩展较小一端 frontier，保证 O(√V) 量级在均匀图上成立。
- 相遇判定：新扩展节点出现在对侧 visited 中即记录候选路径，继续至当前层结束取最短，保证确定性。
- parent map 双侧独立记录，相遇后从相遇点向两端回溯拼接路径。
- 预算对两侧统一核算，不允许一侧耗尽后另一侧继续。

`GraphStats` 结构：

```rust
pub struct GraphStats {
    pub node_count: usize,
    pub edge_count: usize,
    pub avg_out_degree: f64,
    pub out_degree_histogram: Vec<(u32, u64)>,   // (度数上界桶, 计数)，末桶为 usize::MAX
    pub label_distribution: BTreeMap<String, u64>,
}
```

缓存身份为 MemTable generation：任何写操作递增 generation 并失效缓存；只读查询间缓存稳定复用。

性能：100K 双向 BFS P50 0.308 ms（单向 16.1 ms，约 52.3×）。

### 5.3 B3 度数直方图 Planner ✅

无标签 EXPAND 的 fanout 估算使用度数直方图 P90 与二阶矩 size-biased fanout，替代平均度：

- 幂律图中平均度被大量低度节点拉低，hub 节点真实代价被严重低估。
- size-biased 期望 `E[D²]/E[D]` 正确反映「到达一个节点后下一步的期望邻居数」。
- 最后一桶为 `usize::MAX` 上界桶时不当作实际度数。
- source-specific 实际度数可得时覆盖全局估算。

专项 power-law 测试证明：同平均度附近的 hub 图得到更保守的基数估算，Planner 据此选择更优计划（Cascades 专项 8 项通过）。

### 5.4 `.gidx` 格式规范（v2）

```text
magic "TGIX" | version u16 = 2 | header
edge blocks:      # v1 起
    per source: block len + CRC + 紧凑边编码
incoming 目录:    # v2 起
    per target: source_count + 源 ID 列表（带长度 + CRC）
label 目录:       # v2 起
    per label: pair_count + (source, target) 列表（带长度 + CRC）
```

要点：

- v1 打开时解码边块重建目录（兼容路径）；v2 打开跳过边块解码，直接 mmap 读目录——冷启动不再全量解码。
- directory block 独立长度与 CRC；checked offset/count；拒绝重复目录、尾部垃圾、伪造超大计数。
- 惰性出边解码（`OnceLock`）；ReadOnly/Immutable 只读 mmap 零写。

专项验收 4 项：v1/v2 兼容、逐字节截断、定点损坏、ReadOnly/Immutable 零写。

安全解析规则（fail-closed 清单）：

| 异常输入 | 行为 |
|---|---|
| magic/version 不符 | 明确错误拒绝 |
| 任一块 CRC 不匹配 | 定位到块类型（edge/incoming/label）后拒绝 |
| 逐字节截断（任意偏移） | 拒绝，不 panic、不部分加载 |
| 重复 incoming 目录条目（同 target） | 拒绝 |
| 尾部多余字节 | 拒绝 |
| 伪造超大 count（如 u64::MAX） | checked 上界（≤ edge_count / node_count）拦截，不做大分配 |
| offset + len 溢出或越界 | checked 算术拒绝 |

冷启动收益来源：v1 需解码全部边块才能构建 incoming/label 目录；v2 目录独立落盘后打开只读 header + 目录块，出边块保持惰性（首次访问某源时才解码并缓存于 `OnceLock`）。

---

## 六、工业查询执行层（C1.5 / C1.5.1）

### 6.1 六条 AccessPath 与自动路由

```rust
pub enum IndustrialAccessPath {
    PropertyFirst,        // 小型完整属性 posting 直接精排
    PropertyFilteredAnn,  // 中等选择性：过滤感知 ANN + 选择性自适应 beam
    GraphFirst,
    PropertyVectorUnion,  // 属性候选 ∪ ANN 候选 → 联合精排
    GraphVectorUnion,     // 精确图候选 ∪ ANN 候选 → 联合精排
    AnnPostFilter,        // 默认 ANN + 后置检查
    ExactFallback,        // 安全回退
}
```

不变量：

- 元数据不参与、不主导 Vamana 建图、BQ 签名或 RobustPrune；纯向量路径零退化。
- 属性 AND posting 有序求交；统一精确重排（重算原向量 similarity + 完整 Filter + 精确图信号 + NodeId tie-break）。
- 候选/临时内存/预计页读取预算；默认零 spill，超预算分配前拒绝。
- 内存压力不触发自动全量 flush；insert/update payload/vector/link 在 WAL 前执行增长预算检查。
- WAL 写入采用实例生命周期累计计数；只读查询 WAL/sidecar/checkpoint/spill 增量均为 0。

路径选择逻辑（由选择性与预算共同决定）：

| 条件 | 选择 | 理由 |
|---|---|---|
| 属性 posting 完整且 ≤ direct_candidate_limit | PropertyFirst | 直接精排比 ANN 导航便宜 |
| 属性中等选择性、posting > direct 限制 | PropertyFilteredAnn | 过滤感知 beam，选择性自适应扩 ef |
| 精确图候选集小且可达 | GraphVectorUnion | 图候选直接进精排池（C1 实验唯一正收益方向） |
| 属性候选大、BQ 预筛可用 | PropertyVectorUnion | BQ2 粗排 16× 压缩后精排 |
| 无属性/图约束 | AnnPostFilter | 纯 ANN + 后置检查（默认） |
| 无 QuIVer 或配置禁用 ANN | ExactFallback | 暴力精确（小数据安全兜底） |

选择依据全部记录在 `TsngSearchMetrics`（navigation scores、property/graph checks、rerank candidates、candidate peak、临时字节、三类页读取估算、BQ 各阶段耗时、slot cache 命中），SEARCH EXPLAIN 直接输出 `industrial_access_path` 与全套指标。

### 6.2 字节优先预算（C1.5.1 Step 0，缺陷修复）

```rust
pub struct QueryMemoryBudget {
    pub max_candidate_id_bytes: usize,      // 候选 ID 字节上限
    pub max_union_bytes: usize,             // union 字节上限
    pub max_rerank_vector_bytes: usize,     // 精排向量字节上限（一等约束）
    pub max_estimated_page_reads: usize,    // 预计页读取上限
}

pub fn max_rerank_vectors<T: VectorType>(self, dim: usize) -> usize {
    self.max_rerank_vector_bytes / dim.saturating_mul(size_of::<T>()).max(1)
}
```

用户配置「这个查询能用多少内存」，任意维度（含 400、900）自动获得一致字节行为。

验收：384/400/768/900/1536/3072 推导一致性、单调性、超预算零写入测试通过；无维度分档分支；64 维既有结论不退化。

### 6.3 BQ 预筛与 internal-slot 缓存（C1.5.1 Step 2）

BQ 预筛基线（高维合理对照）：

```text
属性候选全集 → BQ2 压缩码粗排（有界 max-heap）→ top-N → f32 精排 → top-K
```

- 768 维 BQ2 约 192 bytes/向量，比 f32 小 16×。
- 有界 max-heap 使临时内存从 O(posting) 降为 O(candidate_pool)。

算法细节：

```text
for slot in posting_slots:                 # internal slot 序（确定性）
    d := bq2_distance(query_sig, signature(slot))   # POPCNT，无向量读取
    if heap.len() < pool or d < heap.max():
        heap.push((d, slot)); 若超 pool 弹出最差
# heap 中的 slot 才读 f32 向量做精确余弦，进入统一精确重排
```

观测分解（进入 `TsngSearchMetrics`）：`bq_posting_lookup_ns`、`bq_node_mapping_ns`、`bq_heap_scan_ns`、`bq_output_sort_ns`、`bq_slot_cache_hits/misses`、`bq_mapped_candidates`。

internal-slot posting 缓存：

- 缓存身份 = 过滤表达式稳定身份 + 独立 `property_generation`。
- 重复查询不再逐 NodeId HashMap 映射，直接产出 internal slot 序列。
- 总预算 64 MiB，按 posting 实际字节计费，近似 LRU；oversized posting 不缓存。
- 不落盘、不 spill，不参与 BQ 签名与建图。

缓存失效矩阵（全部由测试覆盖）：

| 写操作 | 失效范围 |
|---|---|
| insert 节点 | 若命中任一已缓存过滤表达式则该表达式失效 |
| soft-delete / 恢复 | 同上 + tombstone 槽位不可达 |
| payload 更新 | 字段相关的表达式失效 |
| 向量更新 | 不失效（缓存只管 slot 集合，不管向量内容） |
| 索引创建/删除/恢复 | 该字段全部缓存失效 |
| `property_generation` 递增 | 全部缓存逻辑失效（批量写后统一递增） |

### 6.4 高维验收数据（C1.5.1 最终）

200K × 10% 选择性 × ef=1024：

| 维度 | P95 | Recall/NDCG |
|---:|---:|---|
| 384 | 2.646 ms | 1.0 / 1.0 |
| 768 | 3.072 ms | 1.0 / 1.0 |
| 1536 | 3.113 ms | 1.0 / 1.0 |
| 3072 | 4.185 ms | 1.0 / 1.0 |

200K/768 选择性两端：

| 选择性 | posting | P95 | Recall/NDCG |
|---:|---:|---:|---|
| 1% | 1,984 | 1.331 ms | 1.0 / 1.0 |
| 50% | 101,504 | 6.046 ms | 0.95 / 0.9723 |

1M/768/10%（100,320 posting）internal-slot 加速：

| ef | 有界 heap | internal-slot | 加速 |
|---:|---:|---:|---:|
| 1024 | 30.498 ms | 4.364 ms | 6.99× |
| 256 | 29.178 ms | 3.140 ms | 9.29× |

其余要点：

- 属性 epoch 命中后，200K 四维度 posting lookup 约 0.0005–0.0008 ms；剩余成本是逐 slot 的 BQ2+heap 扫描。
- 所有最终报告查询写入与 spill 均为 0。
- 准确性边界：未穷举四维度 × 五选择性 20 点；已完成四维度 10% 主点、768 维 1%/50% 两端、1M/768/10% 代表点（4-query）。不宣称未达标场景的 matched-recall 延迟结论。

报告：`tsng-c1-5-1-200k-{384,768,1536,3072}-slot-final.json`、`tsng-c1-5-1-200k-768-selectivity-{1,50}-slot-final.json`、`tsng-c1-5-1-1m-768-slot-cache.json`。

低维历史验收（64 维，作为兼容性记录）：

- 100K：vector+property / vector+graph / three-signal matched-recall P95 提升 1.60× / 12.25× / 1.57×，精排候选减少 96.88% / 89.32% / 91.78%。
- 1M：mapped 265.6 MB；vector+property 候选 -93.75%（P95 14.99 ms，访问量 Gate）；vector+graph P95 1.78 ms（5.26×）；three-signal 候选 -61.52%（P95 6.08 ms）。
- 64 维每向量仅 256 bytes，100K 候选精排只有 25.6 MB，远低于 page cache 悬崖；这些数字不外推到高维。

### 6.5 早期单队列导航实验结论存档（一笔带过）

C1 阶段曾假设「三信号线性加权单队列 scorer 能加速混合查询」。Matched-Recall Gate 下的最终结论：

| 策略 | 向量+属性 | 向量+图 | 三信号 |
|---|---|---|---|
| 线性加权单队列 | ❌ | ❌ | ❌ |
| Bounded Bonus 限幅 | ❌ 最优 cap=0% | ❌ | ❌ 最优 cap=0% |
| 双队列（无图 seeds） | ❌ | — | — |
| 双队列 + 图 seed 通道 | ❌ | ✅ 3.93× | ✅ 2.29× |
| 图候选直接并集 | — | ✅ ~9.7× | ✅ ~7.4× |

三条定论：

1. 单队列 scorer 的元数据 bonus 会把 beam 拉向「属性正确但向量簇错误」的区域；属性信号在导航层至今无价值。
2. 图场景收益来自**图候选直接进入候选集**，而非沿 Vamana 扩展 seeds。
3. 正确工程方向是 Planner 的 graph-first / candidate-union AccessPath——即 C1.5 已落地的路径。

该结论直接催生了 C1.5 的 Planner 化路线；双队列与图 seed 通道保留为默认关闭的实验 API。C0 的 exact ground truth、Matched-Recall Gate 框架与固定数据集继续作为评测基座复用。

---

## 七、三模管线（C1.6，主线）

### 7.1 第一性原理与架构

使用者会把向量、属性、原生图按任意业务顺序串联：

```text
前序结果集 → 向量检索/重排 → 图遍历/图算法 → 属性过滤 → 集合运算 → 再次重排/聚合
```

按 4 类入口 × 9 类图算子 × 3 类过滤 × 可选重排，两到四段就有数百种合法组合。不能靠枚举 API 覆盖，必须建立可组合的算子代数。

旧架构的根本缺陷：`QueryEntry` 四选一（MATCH/OPTIONAL/FIND/SEARCH），一轮 `WHERE → RANK → RETURN`；`TqlExpr` 只有 Property 与 Literal，相似度不是一等表达式；上一阶段结果不能成为下一阶段输入。

架构级优势（跨模组合代价对比）：

| 系统 | 跨模组合代价 |
|---|---|
| Neo4j + 外部向量库 | 跨系统传 ID、网络往返与序列化 |
| Qdrant / Milvus | 无原生业务图，图算法外置 |
| Kùzu / NaviX | 图与向量同库，但查询路径仍以固定策略为主 |
| **TDB** | **SoA 同进程、同 mmap、NodeId↔slot 直接寻址，算子间零拷贝传候选集** |

管线执行数据流（以 Q6 为例）：

```text
TQL 文本
  │ tql_executor: lexer/parser → TqlPipeline AST（lowering 旧语法）
  ▼
逻辑计划:  Source(Search) → Expand → GraphAlgorithm(PageRank, INDUCED)
            → Filter(pagerank > 0.05) → Rank(top-10) → Return
  │ cascades: Memo 探索 + 安全规则 + estimate 成本 → 确定性选择
  ▼
物理计划:  SearchOp → ExpandExactRerank(融合) → PageRankOp
            → FilterOp(下推的属性索引谓词可再提前) → RankOp
  │ 每阶段 apply(NodeSet) → NodeSet；预算切片逐段扣减
  ▼
NodeSet 流:
  [seed: 200 ids + similarity 列]
  → [ego: ~3000 ids + similarity + depth + provenance]
  → [+ pagerank 列]
  → [~500 ids]
  → [10 ids + 精确 similarity]
  ▼
TqlValue 行（Node/Float/...）→ Rust / Python / Node
```

每一层的可替换性：AST 稳定后，优化器规则、物理算子、并行实现可独立演进；differential 测试锚定在「逻辑计划语义」上，不锚定具体物理选择。

### 7.2 TQL `WITH` 链与一等分数表达式

```text
SEARCH VECTOR [...] TOP 100 AS seed
WITH seed
EXPAND FROM seed VIA :cites *1..3 AS related
WITH related
WHERE similarity(related) > 0.5
   OR related MATCHES {status: "active"}
RETURN related, similarity(related) AS sim
ORDER BY sim DESC
LIMIT 10
```

AST 演进：

```rust
pub struct TqlPipeline {
    pub stages: Vec<PipelineStage>,
    pub returns: ReturnClause,
}

pub enum PipelineStage {
    Source(SourceStage),
    Filter(Predicate),
    Expand(ExpandStage),
    Rank(RankStage),
    GraphAlgorithm(GraphAlgorithmStage),   // pagerank/wcc/degree/betweenness/label_propagation/leiden/sa_ppr
    SetOperation(SetOperationStage),
    Aggregate(AggregateStage),
    Iterate(IterateStage),
}
```

一等分数表达式（可出现在 WHERE/RETURN/ORDER BY/聚合/OR/AND/NOT）：

```text
Similarity(var)
GraphScore(var, column)
PathStrength(var)
Depth(var)
StageColumn(var, column)
```

语义硬规则：

- `similarity(var)` 未引用已有精确列时，由 Planner 插入 exact similarity 算子；不得静默复用近似 BQ 分数。
- EXPAND 后自动插入 exact rerank；不把源节点分数或 approximate 分数当作扩展节点精确相似度。
- 列不存在、generation 不一致、近似值用于精确谓词时返回结构化错误。
- 同一管线多个命名 query vector 必须显式引用。

标量经 `TqlValue::{Node, Int, Float, String, Bool, Null}` 独立返回；Python/Node 绑定直接输出顶层标量列，不写入节点 payload。

作用域与遮蔽规则：

| 规则 | 行为 |
|---|---|
| alias 重复声明 | 解析期拒绝（结构化错误） |
| 引用未声明 alias | 解析期拒绝 |
| 同名 alias 再声明 | 允许遮蔽（`WITH x ... WITH x`），旧集合在遮蔽后不可达 |
| 跨阶段引用 | 只允许引用当前可见 alias |
| 未使用变量 | 允许（不报错，便于链式探索） |
| `similarity(var)` 引用非向量列 | 执行期结构化错误 |
| 近似列用于精确谓词 | 执行期结构化错误，不静默降级 |

错误恢复原则：解析错误定位到 stage 与 token，不尝试部分执行；执行错误携带 stage 序号与算子名，Error/Partial 状态在 `QueryContext` 逐阶段记录。

旧 TQL 兼容：单入口查询（FIND/MATCH/SEARCH 无 WITH）通过 lowering 变成单阶段 pipeline；语义与访问序列经 differential 验证不变；不保留两套执行器。

### 7.3 图算法子集化与语义审计

边界语义三态（必须显式，否则结果不可复现）：

| 模式 | 语义 |
|---|---|
| `INDUCED` | 仅输入 NodeSet 诱导子图（内部边），输出输入节点 |
| `EXPAND k` | 先按方向/标签有限扩展，在扩展诱导子图运行并返回扩展集合 |
| `BOUNDARY k` | 有限边界作上下文，只返回原输入节点 |

核心算法语义细节：

**PageRank（子集版）**

```text
输入：诱导子图 G'=(V', E')
初始化：PR(v) = 1/|V'|
每轮：PR'(v) = (1-d)/|V'| + d × Σ_{u→v} PR(u)/outdeg(u)
      悬挂节点（outdeg=0）的质量均分给全部 V'
收敛：L1 残差 < tol 或达 max_iterations
tie-break：迭代顺序按 NodeId 升序，结果排序 (PR DESC, id ASC)
```

**Brandes Betweenness（子集版）**

```text
全源：对 V' 每个源做 Brandes 依赖累加 → Exact
采样：固定取按 NodeId 排序的前 k 个源 → DeterministicApproximate（sample_size 记入 ScoreKind）
无权有向；平行边去重；sample_size=0 拒绝
```

**标准 Leiden（多层）**

```text
repeat:
  local moving: 按稳定 NodeId 顺序，逐节点移入使 ΔQ 最大的邻接社区（含 resolution）
  refinement: 对每个社区做内部连通性切分，保证子社区内部连通
  aggregation: 社区聚合为超节点，边权求和
until 一轮内无节点移动
展开：逐层把社区编号映射回原节点；编号按社区最小 NodeId 稳定
```

**SA-PPR（DepthBounded）**

```text
有限深度业务扩散：score(anchor)=1，沿边按衰减因子传播至 max_hops
显式标记 DepthBounded（不是收敛型 PPR）
累加容器为 BTreeMap<NodeId, f32>：按 NodeId 有序累加，保证逐位确定
（该规则源自 differential 发现的 1 ULP 漂移修复）
```

语义审计结论（2026-08-30/31）：

| 能力 | 实际语义 | 质量标记 |
|---|---|---|
| PageRank | 有向、非加权、悬挂质量均分、L1 收敛 | Exact；输出 iterations/residual/converged |
| WCC | 诱导子图无向投影弱连通分量 | Exact；确定性 component 编号 |
| degree centrality | 有向入+出度，按理论最大 `2(N-1)` 归一化 | Exact |
| betweenness | 无权有向 Brandes | 全源 Exact；固定前 k 源 DeterministicApproximate |
| Label Propagation | 同步双缓冲轮次 | Approximate + community 列 |
| Leiden（标准） | 加权无向 modularity local moving + refinement + aggregation + 多层展开 | Exact（相对其质量函数）；稳定 NodeId 编号 |
| shortest/reachability | 预算化 BFS/双向 BFS | Exact |
| ALL_PATHS | 目标集合、深度、路径数、聚合、标签序列、禁经节点 | Exact，带显式路径数上界 |
| SA-PPR | 有限深度业务扩散（DepthBounded），非收敛型 PPR | 标记 DepthBounded |

统一保障：稳定 NodeId tie-break、标签过滤、`max_examined_edges` fail-closed、NodeSet 字节/节点预算；禁止先算全图再过滤伪装子集语义。

### 7.4 标准 Leiden

替换历史兼容入口中的标签传播近似，实现完整标准流程：

1. **local moving**：加权无向 modularity（含 resolution），确定性节点顺序（稳定 NodeId）。
2. **refinement**：社区内部连通性保证，不产出断开社区。
3. **aggregation**：社区聚合为超节点，边权合并。
4. **多层展开**：迭代至无改进，逐层回映射到原节点。

结果按最小 NodeId 稳定编号。TQL 算子 `LEIDEN input AS output`（induced-subgraph 语义），写入 `community_id` 列，受 examined-edge 预算 fail-closed 约束。

验收（`bench_indexes_and_leiden`）：

| Gate | 规模 | 实测 |
|---|---:|---:|
| Standard Leiden weighted multilevel | 10K | **9.687 ms** |
| Standard Leiden weighted multilevel | 50K | **76.110 ms** |

专项：Leiden 核心 6 项（modularity 单调不减、refinement 连通性、多层展开、确定性编号、预算拒绝、与参考实现对照）。

与历史标签传播的兼容策略：

| 入口 | 语义 | 说明 |
|---|---|---|
| `run_leiden`（Rust/Python/Node） | **标准多层 Leiden** | 历史函数名保留，语义已升级为标准实现 |
| `deterministic_label_propagation` | 确定性同步标签传播 | 管线中的 `LABEL_PROPAGATION` 算子，明示为近似 |
| TQL `LEIDEN input AS output` | 标准多层 Leiden（INDUCED） | 写入 `community_id` 列 |

决策依据：历史 `run_leiden` 实为加权标签传播，不含 modularity/refinement/aggregation；曾规定「TQL 不得以 Leiden 名称承诺标签传播」。标准实现落地后，函数名与语义统一为真 Leiden，标签传播以独立名称并存——不再有任何入口以 Leiden 之名输出非 Leiden 结果。

确定性来源：local moving 按 NodeId 升序遍历；ΔQ 并列时取社区编号最小者；多层展开后社区编号按「社区最小成员 NodeId」稳定；无任何随机数或 HashMap 迭代序依赖。

### 7.5 Cascades 跨阶段 Planner

第一版能力：

- Memo / Group / GroupExpression 结构；逻辑/物理算子候选。
- 确定性物理候选选择（相同输入必选相同计划）。
- GraphStats（含度数直方图）+ PropertyIndexStats 基数估算。
- 查询内存预算按阶段切片；搜索空间上限。
- 按需 exact rerank：只在分数被精确谓词引用时插入。
- 规则安全记录：保守拒绝会改变路径、候选集合或 Recall 语义的 EXPAND/RANK/Filter 换序。

Memo 结构示意：

```text
Group（等价表达式集合）
 ├─ GroupExpression: 逻辑算子 + 子 Group 引用
 │    ├─ 物理候选 A: ExpandExactRerank { … }
 │    └─ 物理候选 B: Expand → Filter → Rank { … }
 └─ …

探索流程：
  1. 逻辑 AST → 初始 Group 树
  2. 应用安全变换规则（下推/融合/合并）生成等价 GroupExpression
  3. 对每个 Group 的物理候选调用 estimate() 得到 OperatorCost
  4. 按（cost, 规则序号, 候选序号）确定性选择——相同输入必选相同计划
  5. 搜索空间上限（GroupExpression 数/估算调用数）防组合爆炸
```

成本模型输入：

| 输入 | 来源 |
|---|---|
| 基数/选择性 | PropertyIndexStats（entry/distinct/null） |
| 图 fanout | GraphStats 直方图 P90 + size-biased 二阶矩（§5.3） |
| 页读取 | 候选数 × 每行字节 / 页大小，分 vector/payload/graph 三类 |
| 临时字节 | NodeSet 估算行数 × 行宽（含列） |
| 精排成本 | 候选数 × dim × size_of::<T>() |

优化规则集：

1. 属性索引谓词下推，不跨语义敏感的聚合/路径算子。
2. 比较 `RANK → EXPAND` 与 `EXPAND → exact similarity filter` 的选择性、页读取与 Recall 风险。
3. 合并相邻集合运算，避免中间 NodeSet 物化。
4. 相邻 `EXPAND → RANK top-k` 安全融合为 `ExpandExactRerank`；exact top-k 用 `select_nth_unstable_by` + top-k 排序替代全量排序。
5. 全局预算按阶段预留，下游 exact rerank 与 RETURN 物化保底。
6. 不重排有副作用或 Partial 语义敏感的阶段。

EXPLAIN 每阶段输出：operator、input/output rows、temp bytes、vector/payload/graph page reads、boundary mode、selected AccessPath、budget slice、approximate/exact、materialized。

### 7.6 Q1–Q14 查询族 corpus

```text
Q1  SEARCH → EXPAND
Q2  SEARCH → EXPAND → similarity > θ
Q3  SEARCH → EXPAND → RANK
Q4  图 seeds → RANK → 属性过滤
Q5  FIND → EXPAND → RANK
Q6  SEARCH → EXPAND → PageRank → 头部过滤 → RANK
Q7  SEARCH → Leiden → 每簇 centrality top-1 → RANK
Q8  多锚点 SEARCH → 各自 EXPAND → INTERSECT → RANK
Q9  SEARCH → 路径约束（边权/标签/禁经节点）→ RANK
Q10 SEARCH → EXPAND → 属性/相似度 OR → PageRank → LIMIT
Q11 SEARCH → WCC → 最大连通分量 → RANK
Q12 SEARCH → SA-PPR → 属性索引过滤 → 聚合
Q13 FIND → Leiden → 簇质心检索 → EXPAND
Q14 迭代 SEARCH → EXPAND → similarity > θ → 新种子，直到收敛
```

代表性 TQL 语句对照：

```text
# Q2 相似度阈值过滤
SEARCH VECTOR [0.1, ...] TOP 100 AS seed
WITH seed
EXPAND seed [:cites*1..3] AS related
WITH related
WHERE similarity(related) > 0.5
RETURN related, similarity(related) AS sim
ORDER BY sim DESC LIMIT 10

# Q6 PageRank 头部过滤后再精排
SEARCH VECTOR [...] TOP 200 AS seed
WITH seed
EXPAND seed [:knows*1..2] AS ego
WITH ego
PAGERANK ego INDUCED AS pr
WITH pr
WHERE pagerank(pr) > 0.05
RANK pr TOP 10

# Q7 Leiden 每簇代表
SEARCH VECTOR [...] TOP 500 AS seed
WITH seed
LEIDEN seed AS clustered
WITH clustered
GROUP BY community_id(clustered)
  -> centrality top-1 -> RANK TOP 10

# Q11 最大连通分量
SEARCH VECTOR [...] TOP 1000 AS seed
WITH seed
WCC seed INDUCED AS comp
WITH comp
FILTER component_size(comp) = max
RANK comp TOP 10

# Q14 有界迭代
SEARCH VECTOR [...] TOP 50 AS seed
WITH seed
ITERATE:
  EXPAND $seed [:cites*1..2] AS nxt
  WITH nxt WHERE similarity(nxt) > 0.6
  $seed = nxt
UNTIL no_new_nodes OR rounds = 5
RANK $seed TOP 10
```

corpus 数据协议：向量、payload、next/related 边均由 NodeId 确定性生成（无随机数）；高维矩阵 384/768/1536/3072；先 200K，代表族再跑 1M。

differential 体系：

- Q1–Q14 均有独立逐阶段 reference（直接调用向量评分、属性读取、reachability/pathfinding、`graph::subset` 基元，不经 Cascades）。
- 每族执行「直接流水计划」与「显式阶段物化计划」两个真实物理计划，逐字段一致并同时对照 reference。
- 该体系曾发现并修复 SA-PPR HashMap 非确定顺序累加导致的 1 ULP 漂移；改为 NodeId 有序 BTreeMap 累加后逐位确定。

### 7.7 Q5 深挖与 Amdahl 分析（工程方法论存档）

Q5（FIND → EXPAND → RANK）优化的分层诊断方法值得保留为方法论：

第一轮（compact BFS，无路径物化）：200K/768 从完整路径参考 124.343 ms 降至 75.691 ms（1.64×），两者均返回 119,984 行。

分层实测定位剩余瓶颈：

| 层 | 内容 | 耗时 | 说明 |
|---|---|---:|---|
| L1-detailed | 只做 `traverse_detailed` | 127.821 ms | 旧实现成本几乎全在此 |
| L1-compact | 只做 `traverse_compact` | 39.318 ms | 遍历层本身已 3.25× |
| L2 | `Expand::apply`（BTreeMap 去重 + 119,984 次 NodeRow 构造 + provenance push/sort/dedup） | 84.149 ms | 净增约 44.8 ms |
| L3a | 一次 `NodeSet` 深拷贝 | 11.218 ms | 每次跨阶段传递都要付 |
| L3b | `normalize()` | 1.061 ms | 可忽略 |

结论：端到端 1.64× 是 Amdahl 定律的直接结果——行物化层约 45 ms 占优化后总延迟 60%。据此实施第二轮：

- `Expand` 改为连续 `ExpandHit` 追加、一次排序线性分组，避免逐命中维护含大 NodeRow 的树/哈希节点。
- 相邻 `EXPAND → RANK top-k` 融合为 `ExpandExactRerank`。
- exact top-k 从全量排序改为 `select_nth_unstable_by` + top-k 排序。

最终独占复测：200K/768 Q5 = 55.371 ms（参考 121.924 ms，**2.20×**，仍返回 119,984 行）；1M/768 Q5 = 367.222 ms（599,984 行），通过 < 500 ms Gate。

### 7.8 C1.6 最终 Gate（✅ 2026-08-31）

四维度 × 四类真实三阶段等价查询共 16 点全部通过：

- 所有基线/优化计划输出逐字段及哈希一致。
- 候选减少 **80.0%–98.7%**，P95 加速 **6.38×–179.84×**。
- 满足「P95 ≥1.5× 或候选/读取减少 ≥50%」，四维度结论一致。

报告：`bench_pipeline_gate` → `target/bench-reports/pipeline-gate.json`；Q1–Q14 corpus 明细 `tsng-c1-6-pipeline-*.json`。

最终验证：完整 `cargo test`（311 单元 + 全部集成 + 文档测试）、`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings` 通过。

---

## 八、查询内并行（C1.7）

### 8.1 并行预算与确定性策略

`QueryParallelismBudget { max_threads, min_parallel_rows }`：`max_threads=0` 自动取可用核心数，硬上限 64；低于 `min_parallel_rows` 的小工作集保持串行，禁止为微查询支付 Rayon 调度成本。

确定性策略（每个算法单独设计，不依赖 CAS 竞态）：

| 算法 | 确定性手段 |
|---|---|
| 多源 EXPAND | worker 本地紧凑命中 + 预算计数；主线程按 source 顺序合并，统一核算全局预算 |
| Exact Rerank | 行并行评分 + 固定 `(score DESC, id ASC)` top-k |
| Degree | dense slot + `AtomicUsize(Relaxed)` 整数累加（顺序无关） |
| PageRank | 每个目标按固定 source 顺序求和，避免浮点 reduce 漂移 |
| WCC | 只向较小 dense slot 合并的 atomic union-find，root 与调度无关 |
| Betweenness | 按 source 独立计算后按 source 顺序归并 |
| Label Propagation | 同步双缓冲轮次，串并行语义完全一致 |
| 单源 BFS | internal-slot dense bitmap 下标；worker 本地位图，层末固定 chunk 顺序 OR，与 visited 差后按 slot 升序提交 |

安全 Gate：1/2/4/8/16 线程输出逐字段一致；并行路径不绕过任何预算；线程硬上限 64；ReadOnly/Immutable 并行 TQL 后文件长度/mtime/CRC/WAL/lock 不变；纯 QuIVer ANN 单查询 beam search 保持串行低延迟（依靠查询间并发）。

单源并行 BFS 核心伪代码（确定性归并的关键设计）：

```text
输入：source, max_depth, budget, threads
visited := {source}; frontier := {source}
for depth in 1..=max_depth:
    chunks := frontier 按固定大小切分（顺序固定）
    par_for chunk in chunks:            # Rayon worker
        local := 空位图（按 internal slot 下标）
        for v in chunk:
            for (u, labels) in out_edges(v):
                budget.edges -= 1（线程本地计数，层末统一核算）
                if u not in visited_all && label 匹配:
                    local.set(slot(u)); local_depth[slot(u)] = depth
    # —— 层末确定性归并（主线程）——
    for chunk in chunks (固定顺序):
        candidates := candidates OR local(chunk)
    next := candidates AND NOT visited       # 位图差
    visited := visited OR next
    frontier := next 按 slot 升序物化        # 输出顺序与线程调度无关
```

关键点：

- worker 之间零共享写：每个 worker 只写线程本地位图，无 CAS 竞态。
- 预算不在线程间抢夺：edge 计数线程本地累计，层末统一核算；超限整算子 fail-closed，不产生竞态相关 partial。
- 归并顺序固定：chunk 顺序、slot 升序，双保险保证任何线程数下输出位相同。

### 8.2 第一批验收（低风险算子，200K/768，16 核）

| 算子 | 1 线程 | 4 线程 | 8 线程 | 最佳 | 最佳加速 |
|---|---:|---:|---:|---:|---:|
| 多源 EXPAND（20K 源/160K 结果） | 84.717 ms | 36.297 ms | 36.090 ms | 38.643 ms @8T | **2.35×** |
| Exact Rerank（200K×768→Top100） | 51.858 ms | 26.010 ms | 26.041 ms | 25.079 ms | **2.07×** |
| Degree（200K 节点/800K 边） | 208.636 ms | 58.229 ms | 49.244 ms | 46.921 ms | **4.45×** |
| 无索引属性扫描（200K→20K） | 21.945 ms | 6.256 ms | 6.066 ms | 5.758 ms | **3.81×** |

四类算子均通过 4 线程 ≥1.5× Gate；多源 EXPAND 8 线程后受 per-source 小 BFS 与最终排序限制。报告：`query-parallel-200000-768.json`。

### 8.3 第二批验收（图算法，100K/约 800K 边，16 核）

| 算法 | 1 线程 | 4 线程 | 16 线程 | 最佳加速 |
|---|---:|---:|---:|---:|
| 单源 BFS（本组仅约 3 ms） | 3.170 ms | 2.574 ms | 2.822 ms | 1.31× |
| PageRank | 123.353 ms | 51.971 ms | 44.845 ms | **2.75×** |
| WCC（atomic union-find） | 334.850 ms | 21.740 ms | 13.256 ms | **25.26×** |
| Betweenness sample64 | 593.456 ms | 158.009 ms | 74.842 ms | **7.93×** |
| Label Propagation | 1830.953 ms | 362.178 ms | 237.905 ms | **7.70×** |

单源 BFS 在该图上仅 3 ms，调度与层 barrier 主导——属于阈值应保持串行的微查询，不是大前沿吞吐结论；大前沿性能以 1M/高出度 TEPS 判定（§8.4）。报告：`graph-parallel.json`。

### 8.4 单源大前沿 BFS 最终验收（1M 矩阵）

`traverse_compact_parallel` 使用 MemTable 稳定 internal slot 作 dense bitmap 下标；查询持有 `&MemTable`，借用规则保证遍历期间不能删除/插入/复用槽位。

自动选择：位图路径内部 frontier < 1024 串行扫描；Pipeline 单源 EXPAND 以 GraphStats 平均出度 × max depth 估算可达规模，低于 100K 保持原串行 compact BFS。

1M 节点 × 出度 4/8/16 × 深度 3/5/8 × 1/2/4/8/16 线程全矩阵。代表点：

| 出度/深度 | 边数 | 1 线程 | 8 线程 | 16 线程 | 8 线程加速 | 最佳 TEPS |
|---|---:|---:|---:|---:|---:|---:|
| 8 / 8 | 3,397,208 | 573.736 ms | 219.373 ms | 212.189 ms | **2.62×** | 16.01 M |
| 16 / 8 | 15,999,792 | 2257.134 ms | 546.119 ms | 456.431 ms | **4.13×** | **35.05 M** |

8 线程 ≥2× Gate 在两个真正大前沿点通过。出度 16/深度 5 仅 1.39×，说明约 1M 边仍受位图归并与层 barrier 限制；产品自动阈值继续避免中小查询误判为必须并行。报告：`deep-traversal-1000000.json`。

### 8.5 确定性验证方法学（C1.7 全套件通用）

三层验证，缺一不可：

**层 1：逐字段 differential**

```text
for threads in [1, 2, 4, 8, 16]:
    result[threads] := 执行（同一输入、同一预算）
assert 所有 result[threads] 与 result[1] 逐字段相等
```

覆盖字段：节点 ID 序列、全部命名分数列（含浮点位模式）、depth、provenance source_ids、metrics 计数。

**层 2：稳定哈希**

对结果序列计算 FNV-1a 风格混合哈希（NodeId + 分数位模式），写入 benchmark 报告；跨线程档哈希不一致直接判定测试失败。这层在 benchmark 中持续生效，捕获 differential 测试未覆盖的输入。

**层 3：重复执行**

同一配置连续执行 N 次（默认 20），全部逐位一致；排除「并行偶然正确」（如 union-find 合并方向偶然稳定）。

已知非确定源与消除手段：

| 非确定源 | 消除手段 |
|---|---|
| 浮点加法顺序（reduce） | BTreeMap / 固定 source 顺序求和 |
| 并集/归并顺序 | 固定 chunk 顺序 + slot 升序 |
| union-find 合并方向 | 只向较小 dense slot 合并 |
| 原子计数读序 | 整数累加（顺序无关），不用浮点原子 |
| HashMap 迭代序 | 结果集合一律排序后再物化 |
| 线程调度 | 全部输出经层末主线程确定性归并 |

---

## 九、条件性后续任务（不在主线）

### 9.1 C2 导航期 I/O 剪枝（属性侧工程，条件性）

定位：**完全不改动 Vamana 图结构与建边**的前提下，把结果准入信息提前到导航阶段，剪掉 beam 扩展与向量读取。

与 Filtered-DiskANN 的本质分歧（不可混淆）：

| | Filtered-DiskANN | TDB 的 C2 |
|---|---|---|
| 图结构 | 标签参与 RobustPrune 选边 | 完全不变 |
| 纯向量性能 | 可能受影响 | 零退化 |
| 过滤时机 | 建图 + 搜索 | 仅搜索 |
| 与底线 | 冲突 | 符合 |

现有基础：`NavigationScorer` 的 `accept_result` 已能在精排阶段拒绝节点并跳过 `get_vec_f32`；缺口是 beam 仍会扩展不合格节点（5% 选择性下席位可能 95% 浪费）。

三个前提（全部通过才启动）：

1. **高维页读取悬崖存在**：存在选择性区间使 `estimated_vector_page_reads` 超过可用 page cache 50% 或 major fault 显著上升。物理依据：768 维 100K 候选 = 307 MB，3072 维 = 1.23 GB。
2. **导航期剪枝优于强基线**：相同 Recall@10 下 P95 优于「BQ 预筛 + 精排」（已在 C1.5.1 实装）和 `PropertyFilteredAnn` 至少 1.5×，且精排向量页读取降 ≥70%。赢不过 BQ 预筛则 C2 无必要。
3. **HashSet 信号减少无效 I/O**：相同 ef 下向量页读取降 ≥50%，Recall 下降 ≤0.02。

Step A 实验设计（一次性、低成本、结论确定）：

```text
三方对照（同一数据、同一 Recall 目标、同一 ef 扫描）：
  A. BQ 预筛 + 精排（现有 C1.5.1 路径，强基线）
  B. 导航期 HashSet 剪枝：沿 Vamana 走，图与导航评分完全不变，
     不合格节点不进 beam、不参与 BQ 距离计算
  C. PropertyFilteredAnn（现有自适应 beam + post-accept）

矩阵：384/768/1536/3072 × 选择性 1%/5%/10%/20%/50%，先 200K
记录：P50/P95/P99、Recall@10、导航评分数、精排向量页读取、major_faults
判定：B 在相同 Recall 下 P95 优于 A 和 C ≥1.5× 且页读取降 ≥70% → 通过
     否则 → 保留 C1.5 路径，记录负面结果，C2 关闭
```

附带验证（前提 1 的自检）：四个维度的候选数不同，但页读取悬崖应出现在相近**字节**位置；不对齐说明存在未建模的维度相关成本（SIMD 尾块、BQ 码长非线性），需先补成本模型。

已知代价：不改图就无法保证极低选择性下同标签节点连通性，beam 可能走不到目标区域；只能靠选择性 sweep 标定有效区间下界，区间外回退 C1.5 路径。

属性侧已降级为工程能力（非创新点），即使三前提通过也排在论文实验之后。

### 9.2 C3 构图时多信号邻居选择（冻结）

前置 C2 通过 Gate。约束：绝不修改原始 Vamana；必须独立 overlay 邻接；纯向量只走原始图；overlay 随 payload/graph 增量维护。

冻结理由：C2 未验证即叠加属纯投机；约束把收益空间压得很小（Filtered-DiskANN 的全部收益来源恰是标签介入建边，被底线禁止）；C1 已否证属性信号在导航层的价值，C3 是把同一信号搬到构图期再试，先验概率低。

### 9.3 C4 自适应权重（待定）

取决于是否出现需要动态调权的真实场景。当前无排期。

若未来重启，前置条件：存在至少一类查询族，其最优 `TsngWeights` 与默认值差异显著且可由运行时统计推导；否则手调/默认权重已足够，自动化只增加复杂度。

---

## 十、Benchmark 体系

### 10.1 组织原则

- 按能力而非开发阶段组织；`Cargo.toml` 显式注册全部 bench target，`autobenches = false`。
- 数据构建在计时区间外（除非测构建本身）；固定随机种子或确定性生成规则。
- 多计划/多线程对照必须校验结果集合、顺序或稳定哈希；哈希不一致属正确性失败。
- JSON 报告统一 `target/bench-reports/`，含 `schema_version`；延迟毫秒、吞吐标注 QPS/TEPS。
- 共享基础设施：`benches/support/mod.rs`（环境变量解析、稳定哈希、离散百分位、报告落盘）。

环境变量接口（`TDB_*` 前缀，最终参数写回报告保证可审计）：

| 变量 | 默认 | 用途 |
|---|---:|---|
| `TDB_PIPELINE_GATE_NODES` | 20,000 | `bench_pipeline_gate` 每维度节点数 |
| `TDB_PIPELINE_NODES` | 200,000 | `bench_tsng_pipeline` 节点数 |
| `TDB_PIPELINE_DIM` | 768 | 管线 corpus 维度 |
| `TDB_PARALLEL_NODES` / `TDB_PARALLEL_DIM` | 200,000 / 768 | 查询并行套件规模 |
| `TDB_GRAPH_PARALLEL_NODES` | 100,000 | 图算法并行套件规模 |
| `TDB_TRAVERSAL_NODES` | 1,000,000 | 深遍历 TEPS 套件规模 |
| `TDB_C0_*` | 套件内定义 | ground truth 参数 |
| `TDB_C1_*` | 套件内定义 | 工业搜索 Gate 参数 |

结果一致性哈希：`support::mix_hash`（FNV-1a 风格）对输出 NodeId 序列与分数位模式计算；同参数不同线程/计划必须得到相同哈希，否则测试失败。这不是密码学摘要，只用于发现输出漂移。

百分位定义：`percentile_sorted` 为离散百分位（已排序样本取 `ceil(n×p/100)-1` 位置），不插值——避免产生从未观测到的延迟值。

### 10.2 能力套件清单

| 类别 | 套件 | 用途 |
|---|---|---|
| 日常能力 | `bench_queries` | TQL/索引/图读取/搜索微基准 |
| 日常能力 | `bench_indexes_and_leiden` | A3/A4/标准 Leiden（composite 1.823 µs；bitmap OR 1.156 ms；Leiden 10K 9.687 ms / 50K 76.110 ms） |
| 日常能力 | `bench_index_graph_baseline` | 属性/图基线矩阵 |
| 日常能力 | `bench_memory_pressure` / `ci_report` | 内存压力 / CI 端到端报告 |
| 管线 Gate | `bench_tsng_c0` / `bench_tsng_c1` | ground truth 与工业搜索 Gate |
| 管线 Gate | `bench_tsng_pipeline` | Q1–Q14 覆盖 + Q5 分层诊断 |
| 管线 Gate | `bench_pipeline_gate` | 四维度四族最终 Gate → `pipeline-gate.json` |
| 并行 | `bench_query_parallel` / `bench_graph_parallel` / `bench_deep_traversal` | 线程扩展 + TEPS，全部带跨线程哈希校验 |
| QuIVer | `bench_cohere1m` / `bench_random1m` / `bench_recall_at_k` / `bench_rbq2_precision` / `bench_sensitivity` / `bench_quiver_ablation` / `bench_random_sphere` | 规模、质量与参数研究 |
| 消融 | `bench_encoding_ablation` / `bench_ssd_cold_hot` / `bench_variance`（`--features ablation`） | 研究与硬件 |

复现细节见 `benches/README.md`。

### 10.3 报告文件索引

`target/bench-reports/` 下的主要报告与产出入口对照（不提交仓库，随版本标签归档）：

| 报告文件 | 产出入口 | 内容 |
|---|---|---|
| `index-graph-baseline-100k.json` | `bench_index_graph_baseline` | v0.8.2 属性/图基线 |
| `index-graph-track-a-100k.json` | 同上（A1 工作树） | A1 前后对比 |
| `index-graph-track-a2-*.json` | 同上 | A2a Ordered |
| `index-graph-track-b1-*.json` | 同上 | B1 Planner |
| `quiver-cohere1m-c0-baseline.json` | `bench_cohere1m` | QuIVer 纯向量基线 |
| `quiver-cohere1m-c1a.json` | 同上 | NavigationScorer 等价性 |
| `tsng-c1-industrial-mmap-100k-v2.json` | `bench_tsng_c1` | C1.5 100K Gate |
| `tsng-c1-industrial-mmap-1m.json` | 同上 | C1.5 1M Gate |
| `tsng-c1-5-1-200k-*-slot-final.json` | 同上 | C1.5.1 四维度主点 |
| `tsng-c1-5-1-200k-768-selectivity-*-slot-final.json` | 同上 | 768 维选择性两端 |
| `tsng-c1-5-1-1m-768-slot-cache.json` | 同上 | 1M internal-slot |
| `tsng-c1-6-pipeline-*.json` | `bench_tsng_pipeline` | Q1–Q14 corpus + Q5 分层 |
| `pipeline-gate.json` | `bench_pipeline_gate` | C1.6 最终 16 点 Gate |
| `query-parallel-200000-768.json` | `bench_query_parallel` | C1.7 第一批 |
| `graph-parallel.json` | `bench_graph_parallel` | C1.7 第二批 |
| `deep-traversal-1000000.json` | `bench_deep_traversal` | 大前沿 BFS TEPS |

历史 C1 实验报告（`tsng-c1-10k/100k*`、`tsng-c1-matched-recall-*`、`tsng-c1-bounded-bonus-*`、`tsng-c1-dual-graph-*`）属于已否证路线的存档数据，仅用于回溯当时判定，不再纳入常规对比。

---

## 十一、测试矩阵

### 11.1 正确性分层

| 层 | 必测 |
|---|---|
| Property Index | 全扫描 differential、CRUD、事务、slot 复用 |
| Planner | old-vs-new executor、计划稳定性、LIMIT/ORDER 安全 |
| Graph | 单/双向 BFS、环、自环、高出度、预算 |
| 工业混合搜索 | 自动 AccessPath、字节预算、零 spill、只读零写 |
| 维度全兼容 | 384/768/1536/3072 字节预算一致、非常规维度单调 |
| 三模管线 | WITH 链作用域、NodeSet 契约、分数 provenance、Q1–Q14 双计划 differential、图算法子集语义、hub/迭代有界 |
| 并行 | 1/2/4/8/16 线程逐字段一致、预算 fail-closed、阈值下零退化 |

管线必测边界场景清单（`tests/pipeline_*` 全覆盖）：

| 边界 | 场景 |
|---|---|
| 空集 | 空输入进任何算子；空 filter 结果；空图子图 |
| 单节点 | 自环、无邻居、单节点社区 |
| 重复 ID | 输入含重复；去重后列对齐 |
| slot 复用 | 删除后新建复用槽位，posting/位图无幽灵命中 |
| 陈旧 generation | bitmap 视图跨写操作（测试专用，生产由借用阻止） |
| 多 query vector | 同管线两个命名向量显式引用 |
| 列名冲突 | 两算子写同名列（结构化错误或显式覆盖规则） |
| 自环/环/重边 | EXPAND 深度语义、PageRank 悬挂、betweenness 平行边 |
| hub | 出度 ≥ 10⁴ 的锚点预算与估算 |
| 最大边界 | max_depth、max_paths、ITERATE 轮数上界触发 |
| ReadOnly/Immutable | 全管线查询后文件字节零变化 |
| tombstone | 已删节点出现在任何候选源 |

### 11.2 持久化与故障

- 逐字节截断（每个格式全覆盖）。
- bit flip、sector tear。
- tmp 文件残留、rename 前/后断电。
- manifest 与 sidecar generation 不一致。
- node_count、dtype、dim 不一致。
- 跨 v0.8.1 → 新版本迁移。
- 独立块 CRC 损坏（`.pidx` posting block、`.gidx` edge/directory block）。

### 11.3 访问模式与跨平台

- ReadWrite：允许构建和修复。
- ReadOnly：内存 fallback，文件字节不变。
- Immutable：严格 manifest，零副作用。
- GenerationStore：`.pidx`/`.gidx` 纳入 current publish/reclaim。
- 平台：Linux x86_64、Windows x86_64、macOS x86_64/ARM64、Linux ARM64 QEMU；ASan、Coverage、短 fuzz。

### 11.4 Fuzz 目标

```text
fuzz_property_index_parse
fuzz_art_operations
fuzz_tql_planner
fuzz_tsng_query
fuzz_tsng_sidecar_parse
fuzz_graph_block_parse
fuzz_tsng_graph_signal
```

---

## 十二、CI 分层与发布门禁

### 每次提交（阻断）

```text
cargo fmt --all -- --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test                       # 311 unit + 全部集成 + doctest
属性索引 differential（小规模）
planner differential（小规模）
工业搜索 deterministic + exact filter
Python/Node 编译及类型声明同步
```

### 每次提交（非阻断观测）

```text
ci_report（CI 小规模端到端能力报告）
benchmark 快速冒烟（固定 seed、200K 以下）
```

### 夜间

```text
bench_pipeline_gate            # 四维度 16 点 Gate
bench_indexes_and_leiden       # A3/A4/Leiden
bench_query_parallel / bench_graph_parallel / bench_deep_traversal
只读零写全量断言（ReadOnly + Immutable corpus）
fuzz 短运行（各 target 60s）
```

### 手动发布门禁

```text
完整 cargo test + clippy -D warnings
跨语言绑定冒烟（Python/Node 全 API 面）
报告归档 target/bench-reports/ → 版本标签
版本号一致性（Cargo.toml / pyproject / package.json）
CHANGELOG 与文档同步检查
```

### 历史验证口径（供复现参考）

| 项 | 值 |
|---|---|
| 机器 | Windows x86_64，16 逻辑核 |
| 编译 | release + LTO，`codegen-units=1`；QuIVer 基线另加 `-C target-cpu=native` |
| 固定 Seed | `0x5452495649554DDB`（基线套件）；各专项套件在文件头声明 |
| 重复 | 50 次查询 5 预热（基线）；7 次取离散 P95（Gate）；Criterion 10 samples（能力套件） |
| 报告目录 | `target/bench-reports/`（不提交仓库） |

---

## 十三、Stop/Go Gate 总表（历史判定汇总）

| Gate | 判据 | 结果 |
|---|---|---|
| Gate A1 | 等值 < 0.1 ms、重启恢复、零副作用 | ✅ |
| Gate B1 | 末端过滤查询数量级提升、普通路径微秒级开销 | ✅ |
| Gate A2 | 范围/ORDER BY/LIMIT 正确且加速、ART differential | ✅ |
| Gate B2 | 双向 BFS 正确（800 组随机图）、预算生效 | ✅ |
| Gate Step 0 | 四维度字节推导一致、无维度分档 | ✅ |
| Gate C1.5.1 | 高维代表矩阵 Memory/SSD/访问量 | ✅ |
| Gate C1.6 | Q1–Q14 differential + 16 点性能 Gate | ✅ |
| Gate C1.7 | 线程确定性 + 4T≥1.5×/8T≥2×（大工作集） | ✅ |
| Gate A3/A4/B3/Leiden | 专项测试 + `bench_indexes_and_leiden` | ✅ |
| Gate C1（单队列导航） | Matched-Recall ≥1.5× 或导航减 50% | ❌ 已否证，转向 Planner 并集 |
| Gate C2 | 三前提（见 §9.1） | ⬜ 未启动 |
| Gate C3 | C2 通过 + 纯向量零退化 | ⬜ 冻结 |

Gate 判定的共同结构（新 Gate 模板）：

```text
1. 前置条件显式列出（数据规模/维度/线程/缓存状态）。
2. 通过判据量化（倍率、百分比、逐字段一致），不留主观空间。
3. 失败处理预先声明（回退路径/关闭特性/记录负面结果），不临时决定。
4. 数据集与判据一经运行即冻结，作为后续改动的拒绝回归基线。
5. 结果落盘 target/bench-reports/，报告名与 benchmark 入口一一对应。
```

历史上两类失败的处理范例：

- **C1 失败（❌）**：三信号 scorer 未过 Gate → 不删代码、不改数据，保留为默认关闭实验 API；结论写入文档；工程方向转向 Planner 候选并集（C1.5），最终被证明正确（vector+graph P95 最高 12.25×）。
- **低维误判风险（⚠️→✅）**：64 维 1M 验收中 vector+property Industrial P95 慢 18%，曾险些被解读为路径无效；识别出「64 维无 page cache 悬崖」的物理原因后，改为高维重跑（C1.5.1），四维度全部通过。教训固化为 §1.4 的「低维结论不外推」原则。

---

## 十四、版本里程碑与 DoD

| 版本 | 核心交付 | 状态 |
|---|---|---|
| v0.8.x | A1 + B1 + A2 + B2 + C0 + C1 实验 | ✅ |
| v0.9.0 | C1.5 查询执行层 + 磁盘块 + EXPLAIN（64 维 Gate） | ✅ |
| v0.9.x | C1.5.1：字节预算 + BQ 有界预筛 + internal-slot + 高维 Gate | ✅ |
| v0.10.0 | C1.6.A–C：管线骨架、WITH、一等分数、最小算子 | ✅ |
| v0.11.0 | C1.6.D–G：图算法子集化、Cascades、SA-PPR/迭代 | ✅ |
| v0.11.x | C1.7 查询内并行（两批 + 大前沿 BFS） | ✅ |
| v0.12.0 | A3/A4/B3、`.gidx` v2、标准 Leiden 收尾 | ✅ |
| v0.13.0 | 论文实验补强：外部数据集 + 竞品对照（见路线图） | ⬜ |
| v1.0.0 | 全面 Benchmark + 三模管线论文提交 | ⬜ |

各版本 DoD（Definition of Done）通用条款：

1. 功能 + 测试 + benchmark + Gate 四件套齐备。
2. `cargo test` / `fmt --check` / `clippy -D warnings` 全绿。
3. ReadOnly/Immutable 零写断言覆盖新查询路径。
4. 确定性：同输入同配置重复执行逐位一致（并行路径含线程矩阵）。
5. 预算 fail-closed：所有新算子接入统一预算并测试超限行为。
6. 跨语言绑定与类型声明同步。
7. 文档（本方案 + 路线图 + `docs/`）同步更新。
8. 负面结果如实记录，不宣称未验证场景。

v1.0.0 额外条款：

- 至少一个外部真实数据集（非合成）进入 nightly。
- 至少三个竞品对照基线（filtered HNSW / 多信号单图 / 执行层协调风格）。
- 论文实验复现包：脚本 + 数据准备说明 + 报告模板。
- 全部 benchmark 报告随版本标签归档。

---

## 十五、剩余执行清单

按优先级排序（详细论证见路线图）：

| # | 任务 | 验收标准 |
|---|---|---|
| 1 | 论文实验补强：外部数据集接入 | ≥1 个外部真实数据集进入 nightly；数据准备脚本 + 校验和 |
| 2 | 论文实验补强：竞品对照基线 | ≥3 条基线（filtered HNSW / 多信号单图 / 执行层协调风格）可复现 |
| 3 | Scalability 与敏感性 | 10K→10M 曲线 + top-K/跳数/选择性敏感性报告 |
| 4 | TSNG 命名与叙事清理 | 代码/文档/论文对 TSNG 的角色表述一致（执行策略层，非索引） |
| 5 | delta merge 产品化 | 最小阈值 + 冷却窗口 + 空闲不写测试 |
| 6 | OS 物理写计数 | Linux 侧真实计数；Windows 明确 `unsupported` |
| 7 | AST/计划缓存 | 普通 MATCH 规划成本降至微秒以下；缓存失效正确性测试 |
| 8 | 20 点矩阵穷举 | 四维度 × 五选择性补齐（当前代表矩阵已满足地基判定） |
| 9 | 条件性 C2 Step A | 三方对照曲线产出；无论输赢形成确定结论 |

详细展开：

1. **论文实验补强**
   - 接入外部数据集：SIFT1M+合成图、ogbn-papers100M 子集、Wikidata 子集、Amazon Product Graph。
   - 建立竞品对照基线：NaviX/Qdrant 风格 filtered HNSW、Allan-Poe 风格多信号单图、Compass 风格执行层协调、graph-first、brute-force。
   - Scalability 10K → 10M 与参数敏感性（top-K、跳数、选择性）。
2. **Benchmark 数据集外部化**
   - 外部数据集套件不静默下载；准备输入路径、版本与校验和说明。
3. **TSNG 命名与叙事清理**
   - 代码与论文中明确 TSNG 为「工业混合搜索执行策略层」而非索引结构；`src/tsng.rs` 与 `bench_tsng_*` 命名按论文需要评估是否重命名。
4. **工程遗留（低优先）**
   - 自动 delta merge 最小阈值与冷却窗口产品化。
   - OS 级真实物理写计数；Windows 不支持指标显式 `unsupported`。
   - AST/计划缓存消除 Planner 几十微秒固定成本。
   - 四维度 × 五选择性 20 点穷举（当前代表矩阵已满足地基判定）。
5. **条件性 C2 实验（Step A only）**
   - 导航期 HashSet I/O 剪枝 + 三方曲线对照；无论输赢产出确定结论，再决定是否正式设计。

### 铁律

- 每个功能必须有测试兜底、benchmark 实测、明确 Gate、ReadOnly/Immutable 零写、确定性、预算 fail-closed、文档同步，缺一不合并。
- 不为通过 Gate 修改数据分布；数据集与 Gate 固定后作为拒绝回归基线。
- 诚实报告负面结果；不宣称未验证场景的结论。

---

## 附录 A：全量性能数据总表

同机口径分组；跨表不可直接比较（数据规模与协议不同）。

### A.1 属性索引（100K，固定 Seed，P50）

| 能力 | 场景 | 全扫描 | 索引 | 加速 |
|---|---|---:|---:|---:|
| A1 Hash | 等值 0.01% | 22.729 ms | 0.0124 ms | 1833× |
| A1 Hash | 等值 1% + LIMIT 100 | 26.278 ms | 0.211 ms | 124.5× |
| A1 Hash | AND + LIMIT 100 | 26.608 ms | 0.209 ms | 127.4× |
| A1 Hash | 负命中 | 22.772 ms | 0.0017 ms | 13,395× |
| A2 Ordered | 范围 1% | 27.781 ms | 1.192 ms | 23.3× |
| A2 Ordered | ORDER BY DESC LIMIT 100 | 25.925 ms | 0.0903 ms | 287× |
| A3 Composite | 双等值 exact（100K） | — | **1.823 µs** | — |
| A4 Bitmap | 双值 OR（100K） | — | **1.156 ms** | — |

构建成本：Hash 四字段 163 ms；Ordered 单字段 78 ms（BTreeMap）/ 108.8 ms（ART）。

### A.2 图查询（100K，P50）

| 能力 | 场景 | 对照 | 结果 | 加速 |
|---|---|---:|---:|---:|
| B1 Planner | 末端过滤 10K | 14.324 ms | 0.0163 ms | 879× |
| B1 Planner | 末端过滤 100K | 预算失败 | 0.120 ms | 失败→完成 |
| B2 双向 BFS | 随机两点 | 16.1 ms（单向） | 0.308 ms | 52.3× |
| C1.7 BFS | 1M 出度16/深8 | 2257.134 ms（1T） | 546.119 ms（8T） | 4.13×，35.05 M TEPS |

### A.3 工业混合搜索（C1.5.1 高维）

| 规模 | 维度 | 选择性 | P95 | Recall/NDCG |
|---|---:|---:|---:|---|
| 200K | 384 | 10% | 2.646 ms | 1.0 / 1.0 |
| 200K | 768 | 10% | 3.072 ms | 1.0 / 1.0 |
| 200K | 1536 | 10% | 3.113 ms | 1.0 / 1.0 |
| 200K | 3072 | 10% | 4.185 ms | 1.0 / 1.0 |
| 200K | 768 | 1% | 1.331 ms | 1.0 / 1.0 |
| 200K | 768 | 50% | 6.046 ms | 0.95 / 0.9723 |
| 1M | 768 | 10%（ef=1024） | 30.498 → 4.364 ms | 6.99×（internal-slot） |
| 1M | 768 | 10%（ef=256） | 29.178 → 3.140 ms | 9.29× |

### A.4 三模管线（C1.6 最终 Gate，16 点）

| 指标 | 范围 |
|---|---|
| matched-result | 全部 16 点逐字段一致 |
| 候选减少 | 80.0% – 98.7% |
| P95 加速 | 6.38× – 179.84× |
| Q5 深挖（200K/768） | 121.924 → 55.371 ms（2.20×） |
| Q5 深挖（1M/768） | 367.222 ms（< 500 ms Gate） |

### A.5 查询内并行（C1.7，16 核）

| 算子/算法 | 规模 | 1 线程 | 最佳 | 加速 |
|---|---|---:|---:|---:|
| 多源 EXPAND | 200K/768，20K 源 | 84.717 ms | 36.090 ms @8T | 2.35× |
| Exact Rerank | 200K×768→Top100 | 51.858 ms | 25.079 ms | 2.07× |
| Degree | 200K/800K 边 | 208.636 ms | 46.921 ms | 4.45× |
| 无索引属性扫描 | 200K→20K | 21.945 ms | 5.758 ms | 3.81× |
| PageRank | 100K/800K 边 | 123.353 ms | 44.845 ms | 2.75× |
| WCC | 同上 | 334.850 ms | 13.256 ms | 25.26× |
| Betweenness sample64 | 5K 子图/64 源 | 593.456 ms | 74.842 ms | 7.93× |
| Label Propagation | 100K | 1830.953 ms | 237.905 ms | 7.70× |
| 单源 BFS（大前沿） | 1M 出度8/深8 | 573.736 ms | 219.373 ms @8T | 2.62× |
| 单源 BFS（大前沿） | 1M 出度16/深8 | 2257.134 ms | 546.119 ms @8T | 4.13× |

全部线程档（1/2/4/8/16）结果哈希一致。

### A.6 标准 Leiden

| 规模 | P50 |
|---:|---:|
| 10K | 9.687 ms |
| 50K | 76.110 ms |

### A.7 QuIVer 纯向量基线（Cohere1M-768，C0/C1a）

| efSearch | Recall@10 | QPS | 平均延迟 |
|---:|---:|---:|---:|
| 64 | 94.71% | 37,457 | 0.0267 ms |
| 128 | 97.52% | 22,574 | 0.0443 ms |
| 256 | 98.87% | 10,105 | 0.0990 ms |
| 512 | 99.52% | 4,795 | 0.2085 ms |
| 1024 | 99.78% | 3,201 | 0.3124 ms |

构建 63.125 s（15,842 vectors/s）；Exact Flat 单线程 2.084 QPS / 多线程 17.332 QPS。C1a scorer 重构后 Recall 波动 ±0.01pp（并发构图噪声），访问序列 differential 证明算法等价。

---

## 附录 B：测试文件清单（按能力）

| 文件 | 覆盖 |
|---|---|
| `tests/property_index*.rs` | A1/A2/A3/A4 CRUD、槽位复用、重启、differential |
| `tests/pidx_persistence.rs`（专项 9 项） | `.pidx` v1–v4 兼容、CRC、逐字节截断、RO/Immutable 零写 |
| `tests/gidx_persistence.rs`（专项 4 项） | `.gidx` v1/v2 兼容、目录损坏、截断、零写 |
| `tests/tql_planner.rs` | B1/B3 AccessPath 选择、AND 交集、tie-break、反转门禁、EXPLAIN、索引/扫描 differential |
| `tests/tql_pipeline_parser.rs`（12 项） | WITH 链、作用域遮蔽、表达式、错误恢复 |
| `tests/pipeline_operator_contract.rs` | NodeSet 列对齐、provenance、预算契约 |
| `tests/pipeline_differential.rs` | Q1–Q14 双计划 + reference differential |
| `tests/pipeline_graph_algorithms.rs`（10 项） | 子集图算法 reference、边界三态、预算拒绝 |
| `tests/pipeline_budget.rs` | hub、稠密图、迭代、Partial 语义 |
| `tests/query_parallel*.rs` | 线程矩阵 differential、预算 fail-closed、阈值 |
| `tests/query_memory_budget_dimensions.rs` | 四维度字节推导、非常规维度单调 |
| `tests/leiden*.rs`（核心 6 项） | modularity 单调、refinement 连通、多层展开、确定性 |
| `tests/tsng_c0.rs` / `tests/tsng_c1.rs` | ground truth、AccessPath、确定性、回退 |
| `tests/graph_algorithms.rs`（37 项） | 全图算法回归 |

---

## 附录 C：术语表

| 术语 | 定义 |
|---|---|
| internal slot | MemTable 内部稳定密集下标；删除后可复用，所有持久化 posting 与并行位图以其为基准 |
| generation | MemTable 写单调递增计数；索引缓存、GraphStats、slot bitmap 的失效身份 |
| posting | 属性值 → NodeId/slot 列表的倒排项 |
| NodeSet | 管线算子间唯一交换格式（候选集 + 命名分数列 + 溯源） |
| induced subgraph | 仅由输入节点集合及其内部边构成的子图 |
| matched-recall | 双方在同一 Recall 水平下比较延迟的评价协议 |
| TEPS | Traversed Edges Per Second，图遍历吞吐标准指标 |
| fail-closed | 预算或约束超限时返回结构化错误/截断标记，不静默产出部分或超限结果 |
| BQ2 | 2-bit 量化签名，QuIVer 导航期廉价向量距离来源 |
| delta merge | 内存增量达到阈值后与 mmap 不可变块批量合并的写策略 |

---

## 附录 D：跨语言 API 面

三层 API 保持签名同步；新能力先落 Rust，再绑定 Python/Node，类型声明（`.pyi` / `.d.ts`）同 PR 更新。

### D.1 索引与查询（Rust → Python/Node 对照）

| Rust | Python | Node |
|---|---|---|
| `create_index(field)` | `create_index(field)` | `createIndex(field)` |
| `create_ordered_index(field)` | `create_ordered_index(field)` | `createOrderedIndex(field)` |
| `create_composite_index(fields)` | `create_composite_index(fields)` | `createCompositeIndex(fields)` |
| `create_bitmap_index(field)` | `create_bitmap_index(field)` | `createBitmapIndex(field)` |
| `drop_index / drop_ordered_index` | 同名 snake_case | camelCase |
| `list_indexes()` | `list_indexes()` | `listIndexes()` |
| `tql(query)` | `tql(query)` | `tql(query)` |
| `tql_values(query)` | `tql()` 顶层标量自动展开 | 同左 |

### D.2 图算法入口

| Rust | Python | Node |
|---|---|---|
| `run_pagerank / run_wcc / run_degree_centrality` | 同名 | camelCase |
| `run_betweenness`（含 sample_size） | 同名 | camelCase |
| `run_leiden`（标准多层，兼容历史名） | 同名 | camelCase |
| `deterministic_label_propagation` | 同名 | camelCase |
| `shortest_path / all_paths / k_hop` | 同名 | camelCase |

管线算子（PAGERANK/WCC/LEIDEN/SA_PPR/ITERATE/ALL_PATHS）通过 TQL 字符串统一暴露，不重复建三语言 API。

### D.3 混合搜索

| Rust | Python | Node |
|---|---|---|
| `search_tsng(query, config)` | `search_tsng(...)` | `searchTsng(...)` |
| `search_tsng_industrial(...)` | 同名 | camelCase |
| `tsng_ground_truth(query)` | 同名 | camelCase |

配置对象（`TsngSearchConfig`、`IndustrialSearchConfig`、`QueryMemoryBudget`）字段与 Rust 一一对应；字节字段（`max_rerank_vector_bytes` 等）以字节为单位，绑定层不做计数换算。

### D.4 打开模式与零写保证

| 模式 | Rust | 绑定行为 |
|---|---|---|
| ReadWrite | `Database::open(path)` | 可写；写操作走 WAL + delta |
| ReadOnly | `Database::open_read_only(path)` | 拒绝任何写 API（结构化错误） |
| Immutable | `Database::open_immutable(path)` | 严格 manifest；不创建 WAL/lock |

ReadOnly/Immutable 下全部查询（含管线、图算法、混合搜索、并行路径）保证文件字节零变化；该保证由测试断言（长度 + mtime + CRC + 无新文件）。
