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
| `ci_report` | CI 可读的端到端能力报告 | CI 小规模配置 |

## B. 三模管线与性能 Gate

| Cargo 名称 | 目的 | 主要参数/输出 |
|---|---|---|
| `bench_tsng_c0` | TSNG ground-truth 与成本模型基线 | `TDB_C0_*`；`tsng-c0.json` |
| `bench_tsng_c1` | matched-recall、多策略、Fixed/Selectivity/Density beam 三方 Gate | `TRIVIUM_TSNG_*`；`tsng-c1-matched-recall.json` |
| `bench_tsng_pipeline` | Q1–Q14 管线覆盖及 Q5 分层诊断 | `TDB_PIPELINE_NODES`、`TDB_PIPELINE_DIM` |
| `bench_pipeline_gate` | 四维度、四查询族最终 matched-result Gate | `TDB_PIPELINE_GATE_NODES`；`pipeline-gate.json` |

## C. 图与查询并行

| Cargo 名称 | 目的 | 默认矩阵 |
|---|---|---|
| `bench_query_parallel` | EXPAND、exact rerank、Degree、属性扫描查询内并行 | 200K × 768；1/2/4/8/16 线程 |
| `bench_graph_parallel` | BFS、PageRank、WCC、Betweenness、标签传播线程扩展 | 100K；1/2/4/8/16 线程 |
| `bench_deep_traversal` | 单源大前沿 BFS 与 TEPS | 1M × 出度 4/8/16 × 深度 3/5/8 |

这三个套件都校验不同线程数的稳定结果哈希。哈希不一致属于正确性失败，而不是性能噪声。

## D. QuIVer 质量与规模

| Cargo 名称 | 目的 |
|---|---|
| `bench_cohere1m` | Cohere 1M 外部数据集 |
| `bench_random1m` | 1M 随机向量规模测试 |
| `bench_random_sphere` | 单位球随机分布 |
| `bench_recall_at_k` | Recall@K |
| `bench_rbq2_precision` | RBQ2 精度 |
| `bench_sensitivity` | 参数敏感性 |
| `bench_quiver_ablation` | QuIVer 组件消融 |

外部数据集套件不会静默下载数据。运行前应按源码顶部说明准备输入路径，并记录数据集版本和校验和。

## E. 研究与硬件消融

以下套件不是日常回归基准，必须显式启用 `ablation` feature：

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
