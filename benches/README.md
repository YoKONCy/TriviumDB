# TriviumDB Benchmark 复现指南

本目录按用途组织 benchmark。所有 Rust benchmark 都在根 `Cargo.toml` 中显式注册；Cargo 自动发现已关闭。

## 运行原则

- 使用 `cargo bench --bench <名称>` 运行单个套件。
- 只比较同一提交、同一编译配置、同一数据规模和同一冷热缓存协议的结果。
- 延迟 Gate 使用 P50/P95/P99 或 Criterion 置信区间，不以单次耗时作为结论。
- 并行套件同时记录请求线程数、实际线程数和稳定结果哈希。
- JSON 报告统一写入 `target/bench-reports/`。
- 环境变量采用 `TDB_*` 前缀；每个自定义 runner 会把最终参数写入报告。

## A. 日常能力基准

| Cargo 名称 | 目的 | 默认负载 |
|---|---|---|
| `bench_queries` | TQL、属性索引、图读取和基础搜索微基准 | 文件内固定小型数据集 |
| `bench_indexes_and_leiden` | 复合索引、Roaring Bitmap、标准 Leiden | 100K 索引；10K/50K Leiden |
| `bench_index_graph_baseline` | 属性索引与图索引基线矩阵 | 可通过套件参数调整 |
| `bench_memory_pressure` | 内存压力与容量变化 | 文件内固定配置 |
| `bench_open` | 已持久化数据库的重开、首查与查询结果 | 128／384／768／1536维四组合成数据，各5000节点、三轮重开 |
| `bench_quiver_ssd` | 生产 INT8 路径的缓存、驻留及系统读取成本 | 128／129／768维三组合成数据，各4096节点、64查询、三轮 |
| `ci_report` | CI 可读的端到端能力报告 | CI 小规模配置 |

`bench_open` 通过真实 `Database` 创建带 QuIVer 的检查点，以严格索引策略逐轮只读打开。打开和首查分别计时，后续三个候选宽度记录召回、QPS、完整 ID／分数位模式及内存组成。精确预期由独立标量余弦计算得到。连续重开使用可能已预热的 OS 文件缓存，RSS 包含基准进程的数据集和参考结果。

```powershell
$env:TDB_OPEN_ROOT = 'target/bench-reports/open-databases'
$env:TDB_OPEN_NODES = '5000'
$env:TDB_OPEN_REPORT = 'open-before.json'
cargo bench --locked --profile release-contracts --bench bench_open
# 使用新内核对同一组基准文件执行显式检查点，再测其重开。
$env:TDB_OPEN_REWRITE = '1'
$env:TDB_OPEN_REPORT = 'open-after.json'
cargo bench --locked --profile release-contracts --bench bench_open
```

> **数据提醒**　`TDB_OPEN_REWRITE=1` 会打开并重写指定基准目录内的四个数据库检查点。为各批测量保留独立目录和报告，记录源码、工具链、线程数、文件缓存条件及索引版本。

## B. 三模管线与性能 Gate

| Cargo 名称 | 目的 | 主要参数/输出 |
|---|---|---|
| `bench_tsng_c0` | TSNG ground-truth 与成本模型基线 | `TDB_C0_*`；`tsng-c0.json` |
| `bench_tsng_c1` | matched-recall、多策略、Fixed/Selectivity/Density beam 三方 Gate | `TRIVIUM_TSNG_*`；`tsng-c1-matched-recall.json` |
| `bench_tsng_pipeline` | Q1–Q14 管线覆盖及 Q5 分层诊断 | `TDB_PIPELINE_NODES`、`TDB_PIPELINE_DIM` |
| `bench_pipeline_gate` | 四维度、四查询族最终 matched-result Gate | `TDB_PIPELINE_GATE_NODES`；`pipeline-gate.json` |

`bench_pipeline_gate` 对每个计划逐行验证预期的 `property_origin`，随后按原始顺序精确比较其余完整 `NodeRow` 字段。`property_filter_rank` 的全量评分加交集计划清除属性入口，直接属性召回加精排计划保留 `active=true`；其他三个查询族的两侧均按集合合并规则清除该入口。报告中的哈希用于结果标识，正确性由完整行比较验证。属性执行来源语义见 [TQL 参考](../docs/tql-reference.md)。

## C. 图与查询并行

| Cargo 名称 | 目的 | 默认矩阵 |
|---|---|---|
| `bench_query_parallel` | EXPAND、exact rerank、Degree、属性扫描查询内并行 | 200K × 768；1/2/4/8/16 线程 |
| `bench_graph_parallel` | BFS、PageRank、WCC、Betweenness、标签传播线程扩展 | 100K；1/2/4/8/16 线程 |
| `bench_deep_traversal` | 单源大前沿 BFS 与 TEPS | 1M × 出度 4/8/16 × 深度 3/5/8 |

这三个套件校验不同线程数的稳定结果哈希，哈希不一致按正确性失败处理。

## D. QuIVer 质量与规模

默认查询采用 BQ 构图、无训练 SM-L2 导航和原始向量余弦精排。基准测试应记录实际评分模式、搜索宽度和有效精排预算，并对余弦与原生 L2 分别生成 ground truth。

索引模式与生命周期见 [QuIVer 架构说明](../docs/quiver.md)。历史 runner 仍可能使用 `TRIVIUM_*` 参数，以对应入口实际实现为准。

| Cargo 名称 | 目的 |
|---|---|
| `bench_cohere1m` | Cohere 1M 外部数据集 |
| `bench_random1m` | 1M 随机向量规模测试 |
| `bench_random_sphere` | 单位球随机分布 |
| `bench_recall_at_k` | Recall@K |
| `bench_rbq2_precision` | RBQ2 精度 |
| `bench_sensitivity` | 参数敏感性 |
| `bench_quiver_ablation` | QuIVer 组件消融 |

> **数据准备**：运行外部数据集套件前，按源码顶部说明下载数据并配置输入路径，同时记录数据集版本和校验和。

## E. 研究与硬件消融

以下研究套件通过 `ablation` feature 显式启用：

```powershell
cargo bench --features ablation --bench bench_encoding_ablation
cargo bench --features ablation --bench bench_ssd_cold_hot
cargo bench --features ablation --bench bench_variance
```

| Cargo 名称 | 目的 |
|---|---|
| `bench_encoding_ablation` | 编码方案消融 |
| `bench_ssd_cold_hot` | SSD 冷热访问协议 |
| `bench_variance` | 多轮方差与稳定性 |

## 报告兼容性

自定义 runner 的报告应包含 `schema_version`。新增字段可保持当前版本；删除字段、改变单位或改变字段语义时必须提升版本。延迟统一使用毫秒，吞吐明确标注 QPS、TEPS 或 elements/s，字节量不得用节点数代替。

## 新增 benchmark 的要求

1. 在 `Cargo.toml` 对应分类中显式注册，禁止恢复 Cargo 自动发现。
2. 文件顶部说明被测能力、数据分布、正确性 oracle、计时边界和非目标。
3. 固定随机种子；若无随机数，也要说明确定性生成规则。
4. 将数据构建放在计时区间外，除非测试目标就是构建速度。
5. 多计划或多线程对照必须校验结果集合、顺序或稳定哈希。
6. 自定义报告通过 `benches/support/mod.rs` 写入统一目录。
7. 临时探索代码不得注册为公开 benchmark；结论稳定后再整理进入对应套件。

## 生产 QuIVer SSD 测量

`cargo bench --bench bench_quiver_ssd` 使用生产压缩重排及固定来源制品，比较缓冲零缓存、Direct I/O 零缓存、三档行缓存及全 Hot 驻留。适用平台为 Windows 和 Linux，严格 Direct I/O 失败时保留错误。数据采用固定种子 `20261113` 的均匀、聚簇及稀疏分布，默认图参数为 `m=16`、`ef_construction=96`、`alpha=1.2`，检索采用 `top_k=10` 和 `ef=rerank_limit=32／96`。

环境变量 `TDB_SSD_NODES`、`TDB_SSD_QUERIES` 和 `TDB_SSD_ROUNDS` 调整规模，`TDB_SSD_ROOT` 指定制品目录。默认目录为 `target/bench-reports/quiver-ssd/`，相同输入复用已存在的制品，构图参数变化时使用新的目录。缓存候选为完整行缓存估算的 `0、1/16、1/4、1`，并逐轮轮换配置顺序。每个配置重新打开 Reader，使用前八条查询预热，再记录完整测量窗口。

`TDB_SSD_ENCODING` 选择 `int8`、`centered_int8` 或 `centered_int4`，`TDB_SSD_QUERY_PRECISION` 选择 `fp32`、`int8` 或 `int4`。默认均为 `int8`。同一目录复用固定图，按编码分别保存派生制品，报告记录实际编码和查询精度。跨编码比较时使用相同输入、图和工作点，并分别报告召回代价。

报告位于 `target/bench-reports/quiver-ssd.json`，包含来源摘要、图制品摘要、实际参数、QPS、P50／P95／P99、逻辑与系统读取、Runtime 账本及进程内存。精确余弦真值由独立 f64 扫描产生，所有配置逐条核对完整 ID、分数和顺序。查询在稳定事实及编码下比较工程实现，计时用于研究报告。

> **测量提醒**　合成结果描述当前输入与平台。缓冲模式使用当时的 OS PageCache；运行器保持系统缓存状态。进程内存包含数据集、真值及此前阶段的分配，Runtime 账本独立记录每个 Reader 的工作区峰值。真实大规模数据、容器密度和设备流量按对应负载另行测量。
