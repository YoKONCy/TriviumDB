# TriviumDB 5:1 测试体系建设计划

> 状态：实施中（阶段 A、阶段 B 首批、格式变异和严格统计已落地）  
> 当前严格基线：业务有效行 33,759，测试有效行 27,974，比率 0.829:1；距离 5:1 尚缺 140,821 行有效测试资产。  
> 目标：在不堆砌重复用例、不制造危险资源压力的前提下，将可维护测试资产建设到业务源码约 500%，并显著提升故障、状态与组合空间覆盖。  
> 范围：测试基础设施、reference model、差分测试、状态机、故障注入、格式变异、跨语言契约、mutation testing。  
> 非目标：不以空行、复制用例、生成后的临时代码或大型二进制语料灌充行数；不要求穷举完整笛卡尔积；不修改 README 和正式文档。

## 一、目标与度量

### 1.1 规模目标

以排除注释、空行、生成文件后的业务源码 SLOC 为基准：

| 测试资产 | 目标规模 | 主要内容 |
|---|---:|---|
| 确定性语义与公共 API | 1.0S | Rust 单元/集成、Python、Node、FFI 契约 |
| TQL/Planner/Pipeline 差分 | 1.0S | AST generator、reference evaluator、物理计划差分 |
| 持久化与故障矩阵 | 1.0S | allocator、I/O、短写、sync、rename、power-loss |
| 格式损坏与回归语料 | 0.8S | `.tdb/.vec/.wal/sidecar/manifest` 结构化变异 |
| Model-based 状态机 | 0.6S | CRUD、事务、索引、图、flush、compact、reopen |
| Fuzz、metamorphic 与历史回归 | 0.4S | corpus、缩减输入、不变量和关系测试 |
| 跨配置执行矩阵 | 0.2S | dtype、存储/访问模式、线程数、feature、平台 |

目标总量约为 5.0S。手写 harness 与断言优先控制在 2S～3S，其余由声明式 case、可审查生成规则和最小化 corpus 构成。

### 1.2 质量门禁

代码量只作为建设规模指标，不能代替以下门禁：

1. 核心存储、WAL、事务和格式加载达到高 branch coverage。
2. 关键条件表达式逐步达到 MC/DC 或等价 boolean-vector 覆盖。
3. 关键模块 mutation score 达标，存活 mutant 必须分类处理。
4. 所有持久化发布阶段均有真实子进程 kill 验证。
5. 所有显式 `try_reserve` 和可控 I/O 路径均可定点失败。
6. 所有索引具备“索引结果 vs 全扫描 reference”差分测试。
7. 所有优化计划具备“优化前 vs 优化后”语义差分。
8. 所有并行路径具备“单线程 vs 多线程”确定性差分。
9. Rust/Python/Node/FFI 公共能力由同一契约描述驱动。
10. 每个已修复缺陷永久进入最小化回归 corpus。

## 二、总体架构

测试体系分为四层，避免一套 harness 的共同错误掩盖实现缺陷：

### L1：快速确定性回归

- 现有 Rust unit/integration tests。
- Python、Node 公共 API 测试。
- FFI 动态库 fixture。
- 每次普通 CI 执行，要求稳定、低资源、无随机失败。

### L2：独立 reference 与差分

- 朴素内存 reference database。
- TQL reference evaluator。
- 图算法、集合代数、过滤、排序和聚合 reference。
- 索引/优化/并行实现不得作为自己的 oracle。

### L3：状态、格式和故障生成

- Model-based 操作序列生成器。
- 格式感知 mutation engine。
- allocation/I/O/power-loss failpoint enumerator。
- 配置组合采用 pairwise、风险分层和固定种子采样。

### L4：持续探索

- Parser、TQL、数据库文件和 WAL fuzz。
- Differential fuzz 与 metamorphic fuzz。
- corpus 自动缩减；只有稳定、最小且有独特行为的输入进入仓库。

## 三、实施阶段

### P0：测试资产计量与清单

1. 增加可复现的 SLOC 统计规则，分别统计：
   - 业务 Rust；
   - 各语言 binding；
   - 手写测试与 harness；
   - 声明式 case；
   - corpus；
   - 自动生成且不计入目标的文件。
2. 建立“功能 × 模式 × 故障 × 语言端”覆盖清单。
3. 为现有测试标注类别，识别重复测试、无断言测试和名称与机制不符的测试。
4. 不把 benchmark、TEMP 文档、构建产物、日志和 vendor 代码计入测试资产。

**验收：** 任意开发机可得到一致的统计口径；500% 指标不能通过复制文件或生成临时代码提高。

### P0：Reference Model 与状态机

建立独立且刻意简单的 reference model：

- `BTreeMap<NodeId, ReferenceNode>` 保存节点；
- 显式边集合保存图关系；
- 线性扫描向量搜索和 payload filter；
- 朴素集合代数、BFS、最短路径、排序与聚合；
- 禁止复用 TriviumDB 索引、Planner、Pipeline 和持久化实现。

生成带固定 seed 的操作序列：

```text
insert / insert_with_id / update / delete
link / unlink
begin / commit / abort
create_index / drop_index
flush / compact / close / reopen
search / filter / path / aggregate / TQL
```

每一步对比：

- 节点、payload、边和 node_count；
- 查询结果和稳定排序；
- 索引结果与 reference 全扫描；
- flush/compact/reopen 前后状态；
- 失败操作零部分提交；
- 相同 seed 可精确重放和缩减。

**安全约束：** 默认序列、节点和 payload 均设硬上限；长序列在独立慢速任务执行，不申请危险内存。

### P0：TQL 与执行计划差分

1. 建立类型正确的 TQL AST generator，而非随机拼接字符串。
2. AST 可打印为 TQL，再走 Lexer → Parser → Logical Plan → Cascades → Pipeline。
3. 对比以下执行路径：
   - reference evaluator；
   - 优化关闭与开启；
   - Prepared 与非 Prepared；
   - 单线程与多线程；
   - 索引计划与全扫描；
   - 融合算子与拆分算子；
   - ReadWrite、ReadOnly、Immutable；
   - flush/compact/reopen 前后。
4. 覆盖 Filter、集合运算、Path、聚合、NULL/缺失字段、Limit/Offset、图方向和预算边界。
5. 失败 case 输出 seed、AST、TQL、计划和最小数据库状态，支持自动缩减。

### P1：Metamorphic 测试

实现不依赖完整 oracle 的关系断言：

```text
A UNION A = A
A INTERSECT A = A
A EXCEPT A = empty
Filter(true, A) = A
Prepared(Q) = Direct(Q)
Index(Q) = FullScan(Q)
Serial(Q) = Parallel(Q)
BeforeFlush(Q) = AfterFlush(Q)
BeforeCompact(Q) = AfterCompact(Q)
ReadWrite(Q) = ReadOnly(Q) = Immutable(Q)
```

向量与图关系：

- cosine 查询向量乘正标量后排序不变；
- 添加不可达节点不改变既有路径；
- 添加必被过滤节点不改变过滤结果；
- NodeId 双射重映射后图算法结果同构；
- top_k 增长满足合法前缀/集合关系，近似召回场景按其明确契约验收。

### P1：确定性 allocator 与 I/O 故障注入

为可失败操作建立编号化 failpoint：

```text
allocation #N
open/read/write #N
short-write #N
flush/sync_all #N
rename #N
metadata #N
```

采用两阶段执行：

1. 无故障运行并记录目标操作经过的 failpoint 数量。
2. 在专用子进程中逐点失败并重开验证。

每个失败点必须验证：

- 返回稳定结构化错误或进程按预期终止；
- 无 panic、死锁和无限等待；
- 无半 WAL、半事务、半 MemTable；
- 磁盘状态只能是旧完整状态或新完整状态；
- ReadOnly/Immutable 前后文件逐字节一致；
- 失败后可重开，或明确 fail-closed；
- allocator 测试只拒绝目标分配，严禁耗尽物理内存。

### P1：真实 Power-loss 发布矩阵

将发布流程暴露为仅测试 feature 可用的阶段：

```text
AfterWalFrameWrite / BeforeWalSync / AfterWalSync
AfterVecTmpSync / BeforeVecRename / AfterVecRename
AfterTdbTmpSync / BeforeTdbRename / AfterTdbRename
AfterMarkerTmpSync / BeforeMarkerRename / AfterMarkerRename
BeforeWalClear / AfterWalClear
```

父进程等待子进程报告到达阶段后真实 kill；fixture 保持小规模。

组合维度：

- 空库/非空库；
- 首次 flush/后续 generation；
- insert/update/delete/transaction；
- Mmap/Rom；
- 有旧 snapshot/无旧 snapshot；
- SyncMode；
- WAL 完整帧/尾部撕裂帧。

**验收：** 只能恢复旧完整 generation 或新完整 generation，不允许跨文件混代、幽灵节点、半条边和静默损坏。

### P1：格式感知 Mutation Engine

统一描述格式变异：

```text
TruncateAt(offset)
FlipBit(offset, bit)
OverwriteU16/U32/U64(field, value)
SwapBlocks(a, b)
DuplicateBlock(block)
RepairCrc(scope)
CrossGeneration(files...)
```

覆盖：

- `.tdb`；
- `.vec`；
- `.flush_ok`；
- WAL；
- QuIVer、属性索引、文本索引、图块 sidecar；
- generation manifest 和 CURRENT。

重点组合：

- offset/count/length 的 0、1、边界、最大值和溢出；
- 逐字节截断与等长 bit flip；
- CRC 错误；
- 修复 CRC 后仍然语义非法；
- 文件尺寸相同但 generation 混代；
- block 重叠、乱序、重复和引用越界。

所有 loader 必须在限定时间和内存内返回，不得 panic；ReadOnly/Immutable 必须零写。

### P1：跨语言声明式契约

定义一份语言无关 contract case，驱动 Rust、Python、Node 和 FFI：

```json
{
  "name": "hook_error_propagation",
  "setup": [],
  "operation": {},
  "expected": {
    "error_code": "TDB_HOOK_EXECUTION",
    "message_contains": "hook-failure"
  }
}
```

首批契约：

- CRUD、事务和 NodeId；
- SearchConfig 与 payload filter；
- Prepared TQL、Path/List、聚合；
- 四类属性索引；
- Hook 6/6、abort、非法 hit、异常传播；
- ReadOnly/Immutable 错误；
- FFI ABI mismatch 和容量越界；
- 历史 API 的明确迁移错误。

生成器只生成薄适配层，断言语义来自同一 contract；不得提交大规模重复生成代码。

### P2：Fuzz 与 Corpus 生命周期

1. Parser fuzz：任意字节和结构化 TQL。
2. Database fuzz：格式感知 `.tdb/.vec/sidecar` 联合变异。
3. WAL fuzz：帧、CRC、事务边界和尾部垃圾。
4. Stateful API fuzz：有限操作序列。
5. Differential fuzz：reference 与真实执行结果比较。
6. 每个失败输入自动缩减；记录固定 seed 和触发性质。
7. corpus culling：只保留带来新行为、分支或历史缺陷的最小输入。
8. 日常 CI 运行固定 corpus；持续 fuzz 在独立任务运行，不阻塞普通开发流程。

### P2：Mutation Testing

按风险分批实施，优先模块：

1. `.flush_ok`、WAL、manifest 和 sidecar 校验；
2. ReadOnly/Immutable 写保护；
3. 事务原子性与预算检查；
4. TQL Parser/Executor；
5. Filter、集合、路径、排序和聚合；
6. Hook 错误传播和 FFI 边界。

重点 mutant：

- 反转比较；
- 删除错误返回；
- 删除 CRC、长度、版本或预算检查；
- 删除 sync/rename 步骤；
- 修改边界 `+1/-1`；
- 升降序互换；
- ReadOnly 检查恒真/恒假；
- 忽略 Hook 错误。

存活 mutant 必须分类为测试缺口、等价 mutant 或不可达防御代码，不允许只追求百分比。

## 四、配置组合策略

禁止无边界地跑完整笛卡尔积，采用三层矩阵：

### 快速矩阵

- 默认 dtype 和存储模式；
- 单线程；
- 固定小数据；
- 所有 PR 执行。

### 风险矩阵

- dtype × Mmap/Rom × ReadWrite/ReadOnly/Immutable；
- SyncMode × WAL/flush/power-loss；
- index × filter × reopen；
- thread count × parallel operator；
- 使用 pairwise 加高风险三元组合。

### 扩展矩阵

- 多编译器、平台、feature 和 sanitizer；
- 长状态序列与持续 fuzz；
- 定期或发布前执行，不纳入普通 `cargo test`。

所有随机任务必须固定并输出 seed；超时、节点数、边数、payload、文件尺寸和分配量均设硬上限。

## 五、目录建议

在真正实施时优先复用现有 `tests/` 和 `fuzz/`，避免无必要文件膨胀。建议按职责组织：

```text
tests/
  model/          reference model 与状态机
  differential/   TQL、Planner、Pipeline、索引差分
  fault/          allocator、I/O、power-loss harness
  format/         格式描述与 mutation engine
  contracts/      语言无关公共 API case
  corpus/         最小化固定回归输入
fuzz/
  fuzz_targets/   持续探索入口
```

是否拆目录以实施时的 Rust integration test 编译开销和现有布局为准，不为规划机械创建空目录。

## 六、实施顺序与真实进展

```text
1. 统一统计口径与覆盖清单                         [已完成]
2. Reference Model 最小核心                      [已完成]
3. CRUD/事务/重开状态机                          [已完成首版]
4. 索引 vs 全扫描差分                            [已完成首版]
5. TQL AST generator 与 reference evaluator      [已完成 FIND/MATCH 首版]
6. 单线程/并行、优化前/后差分                    [待扩展]
7. allocator/I/O failpoint harness               [已有安全基础，待逐点枚举]
8. power-loss 发布阶段矩阵                       [已有真实 kill，待发布阶段全覆盖]
9. 格式感知 mutation engine                      [已完成 .tdb/.vec/.flush_ok 首版]
10. Rust/Python/Node/FFI 声明式契约              [已有多端测试和 Rust 权威矩阵，待统一 case schema]
11. fuzz corpus 生命周期和自动缩减                [待实施]
12. mutation testing 与存活 mutant 清零计划       [待实施]
```

每一步先提供最小端到端闭环，再扩充 case；禁止先生成大量无法稳定运行或没有 oracle 的测试。

## 七、从 0.829:1 到 5:1 的资产预算

当前严格统计由 `tests/tools/test_asset_ratio.py` 给出：

```text
业务有效行：33,759
测试有效行：27,974
当前比率：0.829:1
目标测试资产：168,795
剩余缺口：140,821
```

剩余资产按验证职责分配，避免全部堆到容易编写的固定用例：

| 测试资产 | 新增有效行目标 | 主要交付物 |
|---|---:|---|
| Reference Model 与状态机 | 20,000 | 完整操作模型、前置条件、失败原子性、replay、shrinker |
| TQL 独立解释器与差分 | 28,000 | 完整值系统、表达式、关系运算、图算子、计划差分 |
| 存储格式规格模型 | 20,000 | 各格式字段描述、语义校验、RepairCrc、跨代组合 |
| 确定性故障注入矩阵 | 20,000 | allocation/I/O/short-write/sync/rename/kill 逐点失败 |
| 跨端公共契约 | 14,000 | 语言无关 case schema，驱动 Rust/Python/Node/FFI/CLI |
| Metamorphic 与属性测试 | 12,000 | 查询等价、图同构、持久化不变量、索引透明性 |
| 并发与确定性交错 | 10,000 | 锁、发布、缓存、并行 Pipeline、跨线程结果确定性 |
| 历史回归与最小 corpus | 10,000 | 每个真实缺陷的最小可重放 fixture |
| Mutation 补洞测试 | 6,821 | 针对存活 mutant 的独立断言与边界 case |
| **合计** | **140,821** | 达到严格 5:1 |

这些数字是资产建设预算，不允许通过模板展开、复制用例或生成产物直接填充。

## 八、持续建设闭环

```text
实现独立验证基础设施
→ 固定 seed 探索状态与配置空间
→ 发现差异、panic、静默截断或部分提交
→ 自动/人工缩减到最小复现
→ 明确被验证的需求、不变量或 mutant
→ 沉淀为永久 regression/corpus
→ 扩展生成规则和故障点
→ 重新统计有效资产与质量指标
```

每个新增测试资产至少关联以下一项，否则不计入严格 5:1：

- 明确需求或公共契约；
- 独立 reference 差分；
- 新状态转换或配置交互；
- 新持久化/分配/I/O 故障点；
- 新格式字段和非法编码；
- 存活 mutant；
- 已确认历史缺陷。

## 九、逐级门禁

不立即启用 `--target 5 --enforce`，先采用单调增长门禁：

```text
0.829 → 1.0 → 1.5 → 2.0 → 3.0 → 4.0 → 5.0
```

每一级同时要求：

- 测试资产比率不得随普通功能提交下降；
- 新公共 API 必须有多端契约；
- 新格式字段必须有格式 mutation；
- 新持久化阶段必须有 power-loss/fault case；
- 新优化必须有 reference differential；
- 修复缺陷必须有最小 regression；
- 关键模块 mutation score 不下降。

## 十、完成定义

达到“测试体系 500%”必须同时满足：

- 测试资产按统一 SLOC 口径达到业务源码 5:1；
- 不依赖复制用例或提交生成产物达标；
- 核心模块 branch/mutation 指标达到预设门槛；
- 持久化和故障点清单无未覆盖高风险项；
- 所有 differential 和 metamorphic 性质稳定通过；
- Rust/Python/Node/FFI 公共契约一致；
- ReadOnly/Immutable 在全部失败场景零写；
- 普通测试不会耗尽物理内存、磁盘或启动无上限进程；
- 每个失败都能输出 seed/phase/case，并可在单机精确重放；
- 快速测试、风险矩阵和扩展矩阵职责分离。

## 十一、禁止事项

- 为代码量指标复制测试或展开海量模板。
- 通过真实耗尽内存、磁盘、句柄或线程制造故障。
- 在主测试进程执行预期 abort、非法指令或不可恢复 allocator OOM。
- 用优化器、索引或生产 Pipeline 自己作为 reference oracle。
- 将随机失败且不能输出 seed 的测试纳入 CI。
- 将 fuzz 运行次数、日志行数或构建产物计为测试源码。
- 在没有最小复现和明确性质时无限积累 corpus。
