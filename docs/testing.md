# TriviumDB 测试实践

> 从单元测试到属性测试、从覆盖率度量到变异测试：TriviumDB 的多层防御测试体系详解。
>
> 本文说明 `0.9.0-beta.4` 的测试与验收方法，见[变更说明](changes/0.9.0-beta.4.md)。

当前未发布加固增加 `tests/contracts/error_cases.json`：Rust、已构建 Python wheel 与 Node 原生模块通过真实 API 核对错误码及完整字段，覆盖 Prepared 参数、维度、节点、唯一约束、只读、行预算、迁移、关闭、锁、I/O 与格式版本。Rust 描述符及 Python 映射测试使用独立的全部错误变体样例；Node 另验大整数详情与 Promise 拒绝。语言参数提取错误按各语言类型契约验证。

嵌入式 Console 使用 `tests/contracts/console_cases.json` 的独立样例验证三端请求、完整记录及重开持久化。`cargo test --features test-hooks --test core console_host -- --test-threads=1` 覆盖真实子进程文件锁竞争、持有旧会话时释放锁、同路径新实例隔离、Hook 屏障下取消/关闭、只读文件组完整字节及关闭失败重试；标为 ignored 的探针由父用例以必需参数实际执行。Server 的 `console_embedding` 验证真实请求内容、客户端退出后的许可归还与宿主锁；Node 已安装 tarball 的消费测试同时验证 N-API 前重入拒绝。`python -m unittest discover -s tests/tools -p test_console_bundle.py` 验证实际 ZIP 的完整 HTML、协议、文档、许可证及可复现内容。

全部错误变体的 Python 映射测试由 Windows Python Binding 任务执行 `cargo test --lib --features python 核心错误完整映射到_python异常`，链接 Windows Python 运行时；跨平台 wheel 继续通过真实公开 API 用例验证交付行为。

`cargo test --features test-hooks --test concurrency opening -- --test-threads=1` 用真实文件移动和路径屏障覆盖打开预检与发布交错：持有租约时准确返回锁冲突且数据制品不变；发布完成后持锁重读完整已提交事实；持续缺失时保留 I/O 错误并归还探测租约；Unix 验证规范路径取得后的调用路径身份，以及符号链接不能误用别名协调文件。Node 工作线程退出回归继续严格要求锁冲突，并验证重开的完整节点内容；意外错误输出原始消息及结构化详情。

发布来源校验执行 `python -m unittest discover -s tests/tools -p test_release_source.py`，CI 编译检查任务显式运行。用例创建真实 Git 历史，网络边界使用独立 PR/CI 响应；验证普通祖先提交、错误仓库或分支、未合并 PR、错误 SHA、缺失/失败/取消 CI 的拒绝，以及 master 前进后的历史 Release 补发。两条 CD 直接运行同一个校验脚本。

---

## 目录

- [测试体系概览](#测试体系概览)
- [用例设计与验收证据](#用例设计与验收证据)
- [快速运行](#快速运行)
- [测试分层架构](#测试分层架构)
- [单元测试](#单元测试)
- [集成测试](#集成测试)
- [属性测试 (Property-Based Testing)](#属性测试-property-based-testing)
- [模糊测试 (Fuzz Testing)](#模糊测试-fuzz-testing)
- [AddressSanitizer 内存安全检测](#addresssanitizer-内存安全检测)
- [ThreadSanitizer 与 Miri](#threadsanitizer-与-miri)
- [变异测试 (Mutation Testing)](#变异测试-mutation-testing)
- [覆盖率度量](#覆盖率度量)
- [编写新测试的指南](#编写新测试的指南)
- [CI/CD 管线](#cicd-管线)

---

## 测试体系概览

文本生命周期回归覆盖 PyPI 0.8.10 历史文件组升级、元数据缺失与失配、正文和关键词重建、预算与只读权限。`fault_io` 验证文本及派生索引发布失败后的重开与重试，`fault_power` 在 Mmap、Rom 文件组提交点前后强杀真实子进程，并精确检查正文、关键词和 WAL 节点。

持有关系回归覆盖数据库对象、Reader、事务、查询结果、Prepared TQL 和未完成的查询 Promise。测试在持续持有这些对象时验证文件组移动、删除、同路径替换、新实例锁与配额隔离，以及多个读者的独立关闭。Windows 额外执行文件独占打开检查，Linux 检查 `/proc/self/maps` 中的数据库文件映射。连续故障矩阵覆盖三种向量类型、Rom/Mmap 转换、ID 复用、文本清空和重建、恢复途中再次失败，以及同步与异步关闭交替重试。关键发布阶段的测试屏障支持按数据库规范路径选择目标，可独立控制共享运行时内的多个实例。

TriviumDB 采用 **五层防御式测试体系**，从函数级正确性到系统级崩溃恢复逐层保障：

| 层级 | 类型 | 验证对象与通过条件 |
| ---- | ---- | ------------------ |
| L1 | 单元测试 | 明确输入对应的数值、返回值和结构化错误 |
| L2 | 集成测试 | 跨模块查询内容、持久化事实、崩溃恢复与资源所有权 |
| L3 | 属性测试 | 随机输入下的数学契约、事务原子性和状态不变量 |
| L4 | 模糊测试 | 解析器和磁盘输入在给定时间与语料内的边界行为 |
| L5 | 变异测试 | 指定生产代码变异能否被现有断言检出 |

## 用例设计与验收证据

`cargo test --test core tql_semantics` 验证普通 MATCH、FIND、WITH 与具名边的完整返回内容、分组、聚合、去重、别名排序和分页。`tests/contracts/tql_return_cases.json` 由 Rust、Python、Node 共同消费；Node 同时通过 `npm run test:package` 在独立安装的 tarball 上执行。DML 用例验证多字段合并、每行 CREATE 作用域、完整 u64 位模式、浮点转换边界、拒绝后的 WAL 内容及关闭重开。纯向量管线在分页后核对具体节点与 Payload 读取量。

`cargo test --test core tql_boundaries` 验证内联 ID、结构 ID 与 Payload 索引域、OPTIONAL 起点过滤、具名边谓词、实体空值、JSON 容器、重复变量、默认列名、Prepared 非法槽位与精确向量输入。固定数值样例覆盖整数/浮点等价表示、2^53 相邻整数、有符号边界及 u64 上限，并在 Hash、Ordered、Bitmap、Composite 配置中核对具体结果。`tests/contracts/tql_boundary_cases.json` 为三端提供固定输入和完整预期；Node 与 Python 的公共 API 入口还验证拒绝后的 WAL 保持、完整大 ID 更新、容器输出及安装制品上的 u64 SEARCH/RANK/Prepared 行为。

`cargo test --features test-hooks --test core tql_aliases -- --test-threads=1` 验证投影快照、多源汇合、连续扩展、同时赋值、标量/节点重命名、递归作用域、实体与对象别名排序，以及串行、并行和融合精排。`tests/contracts/tql_alias_cases.json` 在 Rust Rom/Mmap 与两端实际 SDK 中核对固定完整结果或明确的排序 ID。工作区测试分别检查实例上限与共享配额拒绝，并核对拒绝后配额归还和后续查询。

检索边界使用预先给定的 NodeId 次序验证槽位复用、分段扫描、并行度与 Top-K 截断；数值排序覆盖 `i64/u64` 端点、2^53 相邻整数、正负小数及索引重载。内存计量测试独立遍历实际容器，核对增量账本在替换、重复登记、删除、编译和重载后的值。事务应用故障测试在 WAL 提交后注入失败，要求当前句柄与已准入的共享搜索拒绝部分状态，并在重开后核对完整事务和再次持久化结果。

定向入口：`cargo test --test core search_boundaries`、`cargo test --lib --features test-hooks guard_tests`、`cargo test --test fault_io --features test-hooks wal_failure_state -- --test-threads=1`。QuIVer 调整的质量与耗时对照同时记录数据分布、种子、维度、候选宽度、精排预算、平台和构建参数。

预期结果依据公开契约、独立样例或独立参考模型给出。多物理计划对比验证执行策略的一致性，独立 oracle 验证查询语义；文件组前后快照验证零写与内容保持。每份测试报告注明提交、平台、特性、命令、实际断言及尚待执行的路径。

关键断言直接核对完整结果。节点结果校验 ID、Payload 和对应关系，持久化恢复要求已确认写入的具体记录存在且内容相同。Normal 模式的进程崩溃用例要求检查点和后续成功 WAL 写入全部恢复，逐条检查向量与完整 Payload。字段缺失、采样失败、子进程启动失败和制品缺失均使相关验收失败。测试边界数字说明其含义，mock 用于隔离外部服务或进程边界。

`pipeline_differential` 为十四类管线分别建立用例：可达集合由样例边关系计算，精排使用解析余弦，PageRank 解线性方程，标签传播和 SA-PPR 使用独立矩阵，路径用例核对枚举数量、强度和具体路径。完整 ID 集合与顺序先于每行分数检查，两个物理计划分别接受同一独立预期校验。

Node 类型验收有两个入口：`npm run test:types` 严格检查公共声明与消费样例；`npm run test:package` 将实际 npm tarball 安装到独立项目，编译正向调用与错误输入样例，再加载原生模块检查事务、Prepared TQL、向量分数、关键词关联和重新打开。制品验收同时注入一个未定义的公共类型，要求编译失败。普通 CI 使用制品入口，发布质量门禁先检查类型，npm 发布前再验收装配完成的制品。

定时浸泡使用一百条存活记录的有界负载，要求每次查询成功并返回存活节点的完整 Payload。RSS 用例预热后连续运行五分钟，最终 RSS 上限为基线的三倍；文件用例执行五百轮维护并统计完整文件组，上限为基线的五倍。RSS 取自独立操作系统观测：Windows 采样进程读取 WorkingSet64，Linux 读取 VmRSS，其他平台使用 ps；观测必须有效。`unsafe_audit` 的向量文件用例先关闭数据库，再截断或删除向量文件，并保留提交标记，验证重新打开返回结构化损坏错误。

---

## 快速运行

实例资源诊断还覆盖写锁占用时返回未知内存、共享余额与独立配额、维护延期/失败/恢复，以及 Python 异常和 Node 同步/异步错误上的预算字段。维护状态的定向检查为 `cargo test --lib storage::compaction::tests`；该测试也接入 Linux TSan。Rust、Python、Node 继续共用 `resource_cases.json`，绑定测试还验证预算释放后的查询重试。

生命周期和快照的定向检查：`cargo test --lib database::lifecycle::tests`、`cargo test --test contracts 快照恢复`、`cargo test --test storage snapshot`、`cargo test --test fault_io --features test-hooks snapshot_shutdown -- --test-threads=1`、`cargo test --test fault_power --features test-hooks snapshot -- --test-threads=1`。共享 `lifecycle_cases.json` 覆盖三端 f32/f16/u64；故障用例验证目录发布前失败、发布后同步不确定、真实强杀、超时锁保留及关闭失败重试。TSan 工作流也包含生命周期单元测试；本地 Windows 不执行 Linux TSan/Miri。

共享资源层的定向检查：`cargo test --lib resources::tests`、`cargo test --test concurrency resources`、`cargo test --test contracts 共享资源`。这组测试默认启用，覆盖配额回收、扩容回滚、写入前拒绝、回调线程、单线程维护以及实际工作线程峰值。`tests/contracts/resource_cases.json` 由 Rust、Python、Node 公共 API 测试共同执行，覆盖 f32/f16/u64。详见[资源运行时的配置与统计](resource-runtime.md)。

```bash
# 运行全部测试（单元 + 集成 + 属性）
cargo test

# 仅运行单元测试
cargo test --test unit

# 仅运行属性测试
cargo test --test core proptest          # tests/core/proptest_core.rs
cargo test --test query proptest         # tests/query/proptest_query.rs

# 运行特定领域的集成测试
cargo test --test storage ordered        # storage 领域中的 ordered 用例
cargo test --test pipeline cascades      # Cascades 优化器用例

# 运行确定性交错、物理策略差分、定点 I/O 故障和真实发布阶段强杀测试
cargo test --features test-hooks --test concurrency --test differential --test fault_io --test fault_power -- --test-threads=1

# 本地快捷入口，包含以上四个套件与全部 query 测试
cargo test-critical

# 运行差分测试与磁盘格式规格测试
cargo test --test differential --features test-hooks
cargo test --test format_spec -- --test-threads=1

# 独立管线预期、长时浸泡与 npm 消费制品
cargo test --test pipeline pipeline_differential
cargo test --test long_running soak -- --ignored --test-threads=1
npm run test:types
npm run test:package

# 运行真实发布阶段断电矩阵（子进程强杀）
cargo test --test fault_power --features test-hooks

# 静态验收可使用 cargo check/clippy --all-targets。
# 不要把 cargo test --all-targets 当作普通全量测试命令：它会执行 benchmark，
# 其中包括依赖外部百万向量数据集的 bench_cohere1m。

# 运行覆盖率报告（需要安装 cargo-llvm-cov）
cargo llvm-cov --summary-only
cargo llvm-cov --html --open          # 生成 HTML 可视化报告

# 运行变异测试（需要安装 cargo-mutants，耗时较长）
cargo mutants --file src/filter.rs --timeout 60
```

---

## 关闭与资源所有权回归

以下检查随普通 Rust 测试以及三平台原生绑定 CI 执行：

```bash
cargo test --lib database::lifecycle::tests
cargo test --test core lifecycle_resources
cargo test --test contracts 关闭
cargo test --features test-hooks --test fault_io lifecycle_resources -- --test-threads=1
python tests/python/test_resource_lifecycle.py
npm run test:resource-lifecycle
```

`tests/core/lifecycle_resources.rs` 覆盖 f32/f16/u64、Mmap/Rom、同步与异步关闭，保留关闭对象并重新打开、移动目录；Windows 使用独占文件打开验证数据库句柄已释放。另有 manifest 内容保持、索引变更持久化、关闭后读取拒绝及分组提交 panic 后强制进程退出的恢复检查。故障注入验证最终 WAL 同步失败保留数据、锁、Hook 与原始错误，并可重试。

Python 测试覆盖 `with` 的正常和异常退出、强引用、Hook 循环 GC 与动态库卸载。Node 通过 `--expose-gc` 验证清除、替换、关闭、对象回收和循环引用的 Hook 释放，并覆盖注册失败保留旧 Hook、冻结包装对象、`dispose`、`Symbol.dispose`、Worker 退出与 FFI 动态库释放。FFI 测试在临时目录使用 `rustc` 构建最小插件，绑定 CI 与 npm 发布校验均准备 Rust 工具链。Node 还以原生回调屏障固定占用任务名额，验证数据访问返回完整的 `QueueFull` 预算字段，且被拒绝的 Hook 注册保留原扩展。

Linux TSan 继续执行生命周期单元测试；Windows 本地结果对应原生文件句柄和语言 GC 验收，跨平台结果由 CI 提供。

## 自动阈值与进程退出验收

自动阈值用例检查全部 `1..=3072` 维度相对原阈值按十二分之一向上取整，并覆盖单调性、上下界、禁用配置与连续视图入口。`core/quiver_threshold.rs` 在阈值前后实际执行查询，通过 `vector_route_quiver` 观测验证路由；矩阵覆盖三种 dtype、Mmap/Rom、最高合法维度和持久化重载。

```bash
cargo test --test unit quiver自动
cargo test --test core quiver_threshold
npm run test:process-lifecycle
```

进程生命周期套件使用独立 Node 原生子进程，包含 30 万节点、32 维的 WAL 原会话、跳过析构后的恢复、检查点重开，以及 4000 节点、2048 维的 QuIVer 刷盘与重开内存采样。恢复后验证全部节点的 Payload 和确定性校验和，并抽查原始向量。WAL 回放前后检查逻辑堆结构一致，映射重开保持紧凑结构；RSS 记录用于比较启动路径和页驻留。

三平台均执行 30 万节点的关闭排空：原生回调屏障保持在途查询，周期维护因读锁延期，宿主发起关闭后观察 `draining`，再释放查询。Linux 额外由父进程向独立宿主发送真实 SIGTERM，覆盖待检查点 WAL 与在途查询加周期维护两种情形。退出前检查关闭状态、配额与任务清零；父进程验证自然退出、全部数据恢复及文件组可移动。

子进程有界等待用于捕获卡死。采样包含 RSS、峰值 RSS、内核堆估算、关闭阶段、活跃操作与维护状态；Linux 另记录 `VmHWM`、`RssAnon`、`RssFile`、`RssShmem`，区分匿名堆与文件驻留页。QuIVer 在冷映射建立后采样，再验证重开前后查询结果及分数一致。CI 上传 `process-lifecycle-<runner>` artifact，其中的 `process-lifecycle.json` 保存每个子进程的结果与诊断。共享运行时可继续保留其线程池，实例关闭后归还数据库配额和任务登记。

`storage::wal::streaming_tests` 以解码对象存活计数验证独立操作逐条释放、事务只保留当前批次，并以预先指定的提交边界校验每个字节截断、CRC 错误、非法枚举、嵌套及不匹配封条。真实设备读取错误与回放回调错误必须传播。向量池回归验证加载结束后的干净映射替换、COW 修改及增量保留；Windows 同时检查加载用文件句柄释放。内存采样用于观测，CI 继续使用结构和数据不变量验收。

## 测试分层架构

`tests/` 按领域 harness 组织：每个顶层目录是一个独立 integration target，通过各自 `main.rs` 汇聚同领域的测试文件。

默认 `cargo test` 包含 `storage::publication_recovery`：使用独立构造的 `TPUB` v1 日志验证旧事实 + WAL 完整恢复、Rom/Mmap 旧基础、二次持久化、损坏日志/备份拒绝、提交后不回滚和只读字节级零写。开启 `test-hooks` 后，发布强杀另覆盖 3 种写入场景 × 6 个中断阶段；其中更新/删除/compaction 使用 2048 节点、65 维，验证全部已同步数据与更新边。打开失败不能计为恢复通过。该规模跨越多个 mmap 页，但不代表超内存、真实断电或 PageCache 压力验收。

```
tests/
├── unit/                        # L1: 单元测试（集中管理）
│   ├── main.rs                  #   统一入口
│   ├── memtable.rs              #   MemTable CRUD、图关系、属性索引
│   ├── database.rs              #   Database 公开 API + 事务测试
│   ├── filter.rs                #   Filter 数字/字符串/RFC3339 + from_json 解析
│   ├── vector.rs                #   VectorType + SIMD 标量回退
│   ├── wal.rs                   #   WAL 序列化/反序列化/恢复
│   ├── traversal.rs             #   图谱扩散 (PPR, 抑制, 疲劳)
│   ├── tql_ast.rs               #   TQL AST 数据结构
│   ├── cognitive.rs             #   认知管线 (FISTA, DPP)
│   ├── core.rs                  #   核心工具函数
│   └── index.rs                 #   QuIVer/BruteForce 索引
│
├── core/                        # L2: 内核集成（CRUD/事务/搜索/dtype/公共 API 对齐）
├── query/                       # L2: TQL 全链路（parser/executor/DML/索引/planner）
├── pipeline/                    # L2: NodeSet 管线、Cascades 优化器与 TSNG
├── graph/                       # L2: 图算法、路径与反向边一致性
├── storage/                     # L2: 恢复/压缩/格式迁移/四类属性索引持久化
├── model/                       # L2: 独立 Reference Model 状态机（随机 CRUD/事务/索引/flush/reopen）
├── differential/                # L3: 差分测试（Direct=Prepared、索引=扫描、Mmap=Rom、串行=并行、fusion=split）
├── contracts/                   # L3: 三语言公共 API 对齐与 FFI Hook ABI v2 契约
├── concurrency/                 # L3: 确定性交错与并发不变量
├── hardening/                   # L3: 安全防御
├── long_running/                # L3: soak / 压力
├── python/  node/               # L3: 绑定生命周期与公共 API 冒烟
│
├── format_spec/                 # L4: 磁盘格式规格测试（独立字段规格 + 结构化 mutation）
│   ├── spec.rs                  #   .tdb/.flush_ok/WAL/.pld/.pidx/.gidx 字段规格与不重叠校验
│   ├── mutation.rs              #   字段边界/截断/位翻转/追加/CRC 修复变异
│   ├── snapshot_spec.rs         #   .flush_ok v1/v2/v3、.tdb header、.vec/.pld 边界矩阵
│   ├── wal_spec.rs              #   WAL header/帧/事务边界结构化损坏矩阵
│   ├── sidecar_spec.rs          #   .pidx/.gidx header、CRC 修复后语义校验
│   ├── cross_generation.rs      #   跨文件混代组合拒绝 + ReadOnly 零写 oracle
│   └── replay.rs                #   失败回放元数据与 mutation shrinker
│
├── fault_power/                 # L4: 真实发布阶段断电（子进程强杀，父进程验收）
├── fault_io/                    # L4: 确定性 I/O failpoint、扇区撕裂、WAL 断写
├── fault_crash/                 # L4: 真实崩溃（内联汇编触发 CPU 异常的隔离子进程）
├── fault_allocator/             # L4: 目标 failpoint 后拒绝分配的失败分配器
├── fault_lock/                  # L4: 文件锁竞争
├── fault_hardware/              # L4: EMI 位翻转、硬件入侵检测、unsafe 审计
└── generation/                  # L4: 代际发布与跨进程租约
```

> ⚠️ `fault_*` 系列会启动真实子进程并施加故障（强杀、分配失败、硬件异常），日志中的 `TRAE Sandbox Error` 等输出来自预期的被杀子进程；主测试进程不会自我崩溃，fixture 均为小型数据，不会耗尽真实机器的内存或磁盘。

### 设计原则

- **单元测试集中管理**：所有函数级测试统一在 `tests/unit/` 目录，通过 `main.rs` 统一入口，便于维护和批量运行
- **集成测试独立文件**：每个集成测试场景一个文件，职责单一，失败时可快速定位
- **内联测试保留最小化**：仅 `src/hook.rs` 等少量模块保留 `#[cfg(test)]` 内联测试，用于测试 `pub(crate)` 内部逻辑

---

## 单元测试

单元测试是整个体系的基石，覆盖所有公开 API 的正常路径和错误路径。

### 覆盖范围

| 模块        | 测试文件            | 覆盖要点                                                       |
| ----------- | ------------------- | -------------------------------------------------------------- |
| `MemTable`  | `unit/memtable.rs`  | CRUD、边三元组 upsert、标签 unlink、派生索引与 in_degree 不变量 |
| `Database`  | `unit/database.rs`  | open/close、CRUD、search、TQL、事务（全 7 种 TxOp）、Hook 管理 |
| `Filter`    | `unit/filter.rs`    | 数字/字符串范围、RFC3339 时区、错误路径、bloom mask             |
| `Vector`    | `unit/vector.rs`    | 余弦相似度、SIMD 尾部处理、标量回退、多 dtype                  |
| `WAL`       | `unit/wal.rs`       | v3 显式头、无头拒绝、未来版本门禁、CRC、SyncMode、崩溃恢复       |
| `.tdb` 格式 | `storage/format_migration.rs` | v5 32/48-chunk 自动迁移、v6 自描述布局、未来/过旧版本门禁与损坏拒绝 |
| 图扩散参数 | `unit/traversal.rs` | 出/入/双向、强边上限、绝对权重阈值、稳定排序、自环去重与组合过滤 |
| Python 类型 | `python/typing_smoke.py` | Wheel 内 `py.typed/.pyi`、mypy strict 与 pyright 双门禁 |
| `Traversal` | `unit/traversal.rs` | PPR 扩散、Reachability 方向/最短路径/环/预算、GraphFirst 参数门禁 |
| `TQL AST`   | `unit/tql_ast.rs`   | 语法树节点构造、枚举完整性                                     |
| `Cognitive` | `unit/cognitive.rs` | FISTA 稀疏残差、DPP 多样性采样                                 |
| `Index`     | `unit/index.rs`     | QuIVer/BQ 二值化、BruteForce 精确搜索；默认 SM-L2 专项入口见 [架构说明](quiver.md#检查与复现入口) |

TQL 集成测试还覆盖 WITH 作用域、Cascades、聚合/空输入、算术/COALESCE/Null、Prepared 严格绑定、Path/Shortest、UNION/INTERSECT/EXCEPT、图算法、预算、确定性和 EXPLAIN。`core/public_api_alignment.rs`、`python/public_api.py`、`node/public_api.js` 与 `contracts/` 验证三语言能力对齐和历史入口拒绝。数据库生命周期测试验证 `reachable()` 与 `search_graph_first()` 在 close 后与其他可失败 API 一样返回 `DatabaseClosed`。

### 事务测试示例

事务是 TriviumDB 最关键的正确性保证之一。`unit/database.rs` 中覆盖了事务的全部操作类型和失败场景：

```rust
#[test]
fn tx_insert_和_commit() {
    let mut db = open_db("tx_insert");
    let mut tx = db.begin_tx();
    tx.insert(&[1.0, 0.0, 0.0], json!({"name": "Alice"}));
    tx.insert(&[0.0, 1.0, 0.0], json!({"name": "Bob"}));
    assert_eq!(tx.pending_count(), 2);
    let ids = tx.commit().unwrap();
    assert_eq!(ids.len(), 2);
    assert_eq!(db.node_count().unwrap(), 2);
}

#[test]
fn tx_insert_NaN向量报错() {
    let mut db = open_db("tx_nan");
    let mut tx = db.begin_tx();
    tx.insert(&[f32::NAN, 0.0, 0.0], json!({}));
    assert!(tx.commit().is_err());
    assert_eq!(db.node_count().unwrap(), 0, "失败的事务不应改变状态");
}

#[test]
fn tx_insert_后_在同一事务link() {
    let mut db = open_db("tx_insert_link");
    let mut tx = db.begin_tx();
    tx.insert_with_id(10, &[1.0, 0.0, 0.0], json!({}));
    tx.insert_with_id(20, &[0.0, 1.0, 0.0], json!({}));
    tx.link(10, 20, "related", 0.5);   // 顺序状态投影已记录两个节点存在
    let ids = tx.commit().unwrap();
    assert_eq!(ids.len(), 2);
}
```

### 事务顺序状态与 WAL 语义

`core/transaction_state.rs` 使用独立的单节点状态模型，穷举插入、删除、更新 Payload、更新向量的一至四步组合，分别从存在与不存在的初态执行。矩阵包含 f32/f16/u64 和 Rom/Mmap；成功时验证完整向量、Payload、返回 ID 与未触及节点，失败时验证原内容和 WAL 字节。额外检查自动 ID 分配、图边、自环、文本删除、唯一索引与持久化重开。

`tests/contracts/transaction_cases.json` 为三端提供独立固定预期，覆盖删除重建后的重复插入、更新、反复重建、最终删除和自动 ID 混用。Rust、Python、Node 各自执行当前会话、复制已确认 WAL 后的独立恢复与检查点重开，核对全部节点 ID、向量及 Payload。Rust 和 Node 执行 Rom/Mmap 矩阵；Python 使用其当前构造 API 的默认 Mmap 模式。

`storage/wal_semantics.rs` 使用正常 WAL API 生成 CRC 合法帧，分别构造非法 JSON、错误维度、非有限数值、保留 ID 和 ID 推进溢出。既有检查点与 WAL-only、独立记录与已提交事务均要求结构化拒绝；重复打开失败后比较完整原始文件组。合法 `null`、有效幂等重放、最大合法 ID 和未提交非法记录另有正向样例。

```bash
cargo test --test core transaction_state -- --test-threads=1
cargo test --test storage wal_semantics -- --test-threads=1
npm run test:public-api
python tests/python/public_api.py
```

### Filter 错误路径覆盖

`from_json` 的每个操作符都有对应的类型不匹配错误测试，验证畸形输入返回明确错误：

```rust
#[test]
fn from_json_gt_非法类型报错() {
    let r = Filter::from_json(&json!({"age": {"$gt": true}}));
    assert!(r.is_err());
}

#[test]
fn from_json_嵌套and_or() {
    let f = Filter::from_json(&json!({
        "$and": [
            {"$or": [{"x": 1}, {"x": 2}]},
            {"y": {"$gt": 0}}
        ]
    })).unwrap();
    assert!(f.matches(&json!({"x": 1, "y": 5})));
    assert!(!f.matches(&json!({"x": 3, "y": 5})));
}
```

---

## 集成测试

集成测试验证多个模块协同工作的正确性，特别是涉及 IO、并发和崩溃恢复的场景。

### 关键集成测试矩阵

| 测试文件          | 验证目标   | 核心场景                                 |
| ----------------- | ---------- | ---------------------------------------- |
| `fault_crash/`    | 硬件崩溃   | 使用真实内联汇编触发 CPU 异常（`ud2`、`div/0`、`int3`、内存违例），验证操作系统杀进程后的 WAL 恢复能力 |
| `fault_hardware/` | 硬件侵入   | 使用真实内联汇编（`bts` 翻转 bit、`movnti` 绕过缓存）直接篡改 mmap 页，验证容错能力；EMI 脉冲突发多比特错误 |
| `fault_io/wal_midwrite.rs` | 断写安全   | WAL 写入中途中断、逐字节截断、CRC 校验拦截损坏记录 |
| `fault_power/`    | 进程强杀   | 首次 flush、追加、2048×65 更新/删除/compaction；备份发布后、`.vec`/`.tdb` 落盘后、marker 前后与恢复途中强杀，必须完整恢复 `Full` 模式已同步事务 |
| `fault_io/deterministic_failpoint.rs` | I/O 故障 | Create/Write/Sync/Rename 定点确定性失败，验证临时文件清理与错误传播 |
| `format_spec/`    | 格式规格   | 独立字段规格 + 字段边界/截断/位翻转/追加/CRC 修复 mutation，验证损坏输入 fail-closed 与 ReadOnly 零写 |
| `fault_allocator/`| OOM 拦截   | 目标 failpoint 后拒绝分配，验证 `try_reserve` 结构化错误与零部分提交（不耗尽真实内存） |
| `unsafe_audit.rs` | 安全边界回归 | 覆盖向量数值/尾部边界、VecPool 文件变更和畸形文件加载；不代表全部 unsafe 均已审计或证明安全 |
| `core/transaction.rs`  | 事务原子性 | 多操作原子提交、NaN/维度拦截、跨事务依赖 |
| `concurrency/`    | 线程安全   | 多线程并发读写、无数据竞争、确定性交错、generation/singleflight/fatigue、容量预算与 WAL 原子性 |
| `hardening/security.rs`     | 安全防御   | NaN 注入、超大 payload(10MB)、幽灵事务截断 |
| `long_running/stress.rs`       | 压力极限   | 高频写入、自环重边图谱震荡、空库极端操作 |
| `query/tql_executor.rs` | TQL 全链路 | MATCH/FIND/SEARCH 三种入口的完整执行     |
| `query/tql_dml.rs`      | TQL 写操作 | CREATE/SET/DELETE/DETACH DELETE；读查询误用拒绝 |
| `query/tql_pipeline_parser.rs` | 三模管线 | 聚合、Prepared、路径、集合、作用域、旧语法回归 |
| `differential/`   | 差分 | 独立 reference、多物理计划一致性、Prepared/Direct、索引/扫描、Mmap/Rom、串行/并行、fusion/split |
| `core/public_api_alignment.rs` | 公共 API | 四类索引、StorageInfo、Prepared、一等值、除旧 |

### WAL 断写安全测试示例

```rust
// 模拟 WAL 写入中途中断（只写了一半数据）
// 验证重启后 CRC 校验能检测到损坏记录并在损坏边界停止回放
#[test]
fn wal_半写条目被安全跳过() {
    // 1. 写入正常数据
    // 2. 人为截断 WAL 文件模拟断电
    // 3. 重新打开数据库
    // 4. 验证：正常数据完好，损坏条目被跳过，无 panic
}
```

---

## 属性测试 (Property-Based Testing)

属性测试使用 [proptest](https://docs.rs/proptest) 随机生成大量输入，验证系统的 **数学不变量** 和 **安全性契约**。

### 不变量清单

TriviumDB 定义了以下 6 类核心不变量，由 `tests/core/proptest_core.rs` 持续验证：

| #   | 不变量                            | 随机用例数 | 描述                                                                   |
| --- | --------------------------------- | ---------- | ---------------------------------------------------------------------- |
| 1   | MemTable CRUD node_count 一致性   | 200        | 任意 insert/delete 序列后，`node_count` == 实际存活节点数              |
| 2   | MemTable insert/get/delete 可见性 | ~200       | 插入后 `contains` 为 true，删除后为 false                              |
| 3   | Filter matches 绝不 panic         | 500        | `from_json` 成功解析的 Filter 对任意 payload 调用 `matches` 绝不 panic |
| 4   | 余弦相似度 self-similarity = 1.0  | 500        | 非零向量与自身的相似度恒为 1.0                                         |
| 5   | 余弦相似度对称性                  | 500        | `similarity(a, b) == similarity(b, a)`                                 |
| 6   | 余弦相似度绝对范围                | 500        | 结果恒在 `[-1.0, 1.0]` 范围内                                          |
| 7   | Transaction 原子性                | 50         | commit 成功则全部可见，失败则数据库状态完全不变                        |
| 8   | link/unlink in_degree 一致性      | 100        | `link` 后 in_degree +1，`unlink` 后 -1                                 |
| 9   | WAL 序列化往返                    | 100        | `append` → `read_entries` 数据完全一致                                 |

---

## 模糊测试 (Fuzz Testing)

TriviumDB 包含持续运行的 [LibFuzzer](https://llvm.org/docs/LibFuzzer.html) 模糊测试目标，覆盖了所有安全敏感的外部输入解析器。

### Fuzz 目标

1. **`fuzz_wal_parse`**: 验证 WAL 解析器在面对任意截断、错乱的二进制流时，能否通过 CRC32 拦截而不发生 OOM 或 panic。
2. **`fuzz_query_parse`**: 验证 TQL 解析器（词法+递归下降）在面对畸形语法时，能否优雅报错而不产生栈溢出。
3. **`fuzz_filter_parse`**: 验证 Filter 解析器处理病态嵌套 JSON 时不恐慌。

### 运行方式

```bash
cargo install cargo-fuzz

# 运行 WAL 解析器模糊测试
cargo fuzz run fuzz_wal_parse
```

> CI/CD 中配置了两套 Fuzz 策略：
> - **短时（每次 PR）**: 运行 30s × 3 目标，快速拦截低级错误。
> - **长时（每周日）**: 运行 600s × 3 目标，发现深层隐患并持久化语料库。

---

## AddressSanitizer 内存安全检测

CI 使用 nightly Rust 的 AddressSanitizer (ASan) 检查测试执行路径中的内存访问，覆盖范围由平台、CPU 指令集、feature 和测试输入共同决定。数据竞争由独立的 TSan 任务检查。

运行本地 ASan 测试（需要 Linux/macOS nightly 环境）：

```bash
RUSTFLAGS="-Zsanitizer=address" cargo +nightly test --lib --tests --target x86_64-unknown-linux-gnu -Zbuild-std -- --test-threads=1
```

ASan 能有效检测：堆栈溢出 (Buffer Overflow)、内存泄漏 (Memory Leak)、使用后释放 (Use After Free)。

## ThreadSanitizer 与 Miri

`ci.yml` 为普通 PR/push 配置独立的 TSan 和 Miri 任务，与多平台测试、ASan 和代码审核共同构成验收流程。各项执行结果记录在对应提交的 CI 中。

| 检查 | 当前执行范围 | 边界 |
|---|---|---|
| 默认测试 | 默认 feature 下的库与集成测试 | 不会隐式启用 `test-hooks` |
| 确定性交错与故障注入 | 多平台显式启用 `test-hooks`，运行 concurrency、differential、fault_io、fault_power；规划器回归另行执行 | 验证预设线程交错、定点故障和强杀进程中断场景 |
| TSan | Linux x86_64 下启用 `test-hooks` 的 concurrency，以及 `memory_safety_`、`database::lifecycle::tests`、`storage::compaction::tests` 库测试子集 | 数据竞争检测；标准库一起插桩，未覆盖其他平台、FFI 或全部图构建路径 |
| Miri | `memory_safety_` 库测试子集，随机种子 `0..4` | BQ 精确分配长度、尾部掩码、扩容后行偏移，以及并发邻接表锁保护、容量边界和冻结；不执行 mmap、FFI 或完整 SIMD 路径 |

TSan 本地命令（Linux x86_64，nightly 与 rust-src）：

```bash
RUSTFLAGS="-Zsanitizer=thread" TSAN_OPTIONS="halt_on_error=1" cargo +nightly test --locked -Zbuild-std --target x86_64-unknown-linux-gnu --features test-hooks --test concurrency -- --test-threads=1
RUSTFLAGS="-Zsanitizer=thread" TSAN_OPTIONS="halt_on_error=1" cargo +nightly test --locked -Zbuild-std --target x86_64-unknown-linux-gnu --lib memory_safety_ -- --test-threads=1
RUSTFLAGS="-Zsanitizer=thread" TSAN_OPTIONS="halt_on_error=1" cargo +nightly test --locked -Zbuild-std --target x86_64-unknown-linux-gnu --features test-hooks --lib database::lifecycle::tests -- --test-threads=1
RUSTFLAGS="-Zsanitizer=thread" TSAN_OPTIONS="halt_on_error=1" cargo +nightly test --locked -Zbuild-std --target x86_64-unknown-linux-gnu --lib storage::compaction::tests -- --test-threads=1
```

Miri 本地命令：

```bash
rustup component add --toolchain nightly miri rust-src
MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-many-seeds=0..4" cargo +nightly miri test --locked --lib memory_safety_ -- --test-threads=1
```

Windows 主机可加 `--target x86_64-unknown-linux-gnu` 解释执行 Linux 目标的 Miri 用例；Windows 特有的文件映射和锁语义由 Windows 原生测试验证。Miri 配置保留默认的别名、数据竞争与泄漏检查，并使用多个随机种子。

unsafe 审核按文件、安全契约和实际执行路径记录。`bq.rs`、`vector.rs` 和 SM 相关模块包含标量裸指针或 SIMD 路径；`quiver.rs` 包含并发邻接表及其他构图路径；存储与属性索引包含 mmap；`hook.rs` 和 Python/Node 绑定包含外部调用或手工 Send/Sync。扩展测试时逐项核对长度、对齐、生命周期及并发条件，并记录对应输入和平台。[Rust Sanitizer 说明](https://doc.rust-lang.org/unstable-book/compiler-flags/sanitizer.html)、[Miri 说明](https://github.com/rust-lang/miri)列出了工具的适用范围。

### 运行属性测试

```bash
# 运行全部属性测试（约 2450 个随机用例，~10 秒）
cargo test --test proptest_core --test proptest_query

# proptest 支持 shrinking：失败时自动找到最小可复现输入
# 失败用例会保存在 proptest-regressions/ 目录，后续自动回归
```

> 💡 属性测试的价值在于发现人类测试者难以想到的边界组合。例如：空向量的 self-similarity、全零向量的余弦计算、超长 insert/delete 交替序列后的状态一致性。

---

## 变异测试 (Mutation Testing)

变异测试通过 [cargo-mutants](https://github.com/sourcefrog/cargo-mutants) 对源码进行微小修改（如 `>` 改为 `>=`、删除一行代码），验证现有测试是否能检测到这些"人工 bug"。

### 安装

```bash
cargo install cargo-mutants
```

### 使用

```bash
# 对单个文件运行（推荐，因为全项目变异非常耗时）
cargo mutants --file src/filter.rs --timeout 60
cargo mutants --file src/vector.rs --timeout 60

# 查看存活的变异体（= 测试未覆盖的逻辑）
cargo mutants --file src/filter.rs --timeout 60 2>&1 | grep "MISSED"

# 列出所有可能的变异位点（不实际运行）
cargo mutants --list --file src/filter.rs
```

### 解读结果

| 结果          | 含义                          | 行动             |
| ------------- | ----------------------------- | ---------------- |
| `killed`      | ✅ 测试成功杀死了这个变异体   | 无需行动         |
| `survived`    | ⚠️ 测试未能检测到这个代码变更 | 需要补充测试     |
| `timeout`     | 变异导致死循环/性能退化       | 通常算作"killed" |
| `build error` | 变异导致编译失败              | 无需行动         |

> ⚠️ 变异测试非常耗时（每个变异体需要独立编译整个项目）。建议仅对高风险、高覆盖率的模块按需运行，不适合放入 CI 常规流程。

---

## 覆盖率度量

### 工具安装

```bash
# 安装 cargo-llvm-cov（基于 LLVM 的精确覆盖率工具）
cargo install cargo-llvm-cov
```

### 使用方式

```bash
# 终端摘要（按文件统计行覆盖/函数覆盖/分支覆盖）
cargo llvm-cov --summary-only

# 生成 HTML 可视化报告（推荐）
cargo llvm-cov --html --open

# 导出 JSON 格式（供 CI 解析）
cargo llvm-cov --json --output-path coverage.json
```

### 当前覆盖率记录

以下数字是历史实测快照，不代表本次超海量更新后的当前覆盖率；正式发布应以对应提交的 `cargo llvm-cov` 输出和 CI artifact 为准。

| 指标             | 历史快照 |
| ---------------- | ------ |
| **总行覆盖率**   | 93.29% |
| **总函数覆盖率** | 91.68% |
| **总分支覆盖率** | 90.52% |

#### 关键模块覆盖率

| 模块              | 行覆盖 | 函数覆盖 | 说明                                           |
| ----------------- | ------ | -------- | ---------------------------------------------- |
| `filter.rs`       | 99.48% | 100%     | 核心过滤逻辑，全路径覆盖                       |
| `transaction.rs`  | 84.82% | 92.86%   | 事务原子性，全 7 种 TxOp 覆盖                  |
| `hook.rs`         | 77.65% | —        | CompositeHook + FfiHook 逻辑                   |
| `database/mod.rs` | 71.20% | 86.54%   | Database 公开 API                              |
| `vector.rs`       | ~73%   | —        | 含 ARM NEON 等平台特定代码（x86 上物理不可达） |
| `compaction.rs`   | ~0%    | —        | 后台 IO 线程，需专用集成测试                   |

> 💡 覆盖率 ≠ 质量。100% 行覆盖率不代表所有逻辑分支都被验证。属性测试和变异测试是覆盖率的重要补充。

---

## 编写新测试的指南

### 原则

1. **公开 API → `tests/unit/`**：所有 `pub fn` 的测试写在 `tests/unit/` 对应模块中
2. **内部逻辑 → `#[cfg(test)]` 内联**：仅 `pub(crate)` 的辅助函数使用内联测试
3. **跨模块协作 → `tests/` 独立文件**：涉及 IO、WAL、多模块交互的场景使用集成测试
4. **随机输入 → `proptest`**：数学契约和不变量使用属性测试

### 添加新单元测试

1. 在 `tests/unit/` 中找到对应模块的文件（如 `filter.rs`）
2. 添加 `#[test]` 函数
3. 命名规范：`fn 被测方法_场景描述()`，允许使用中文描述

```rust
// tests/unit/filter.rs
#[test]
fn filter_eq_精确匹配() {
    let f = Filter::eq("role", json!("admin"));
    assert!(f.matches(&json!({"role": "admin"})));
    assert!(!f.matches(&json!({"role": "user"})));
}
```

### 添加新模块的单元测试

1. 创建 `tests/unit/新模块.rs`
2. 在 `tests/unit/main.rs` 中注册：`mod 新模块;`
3. 编写测试

```rust
// tests/unit/main.rs
mod memtable;
mod database;
mod filter;
mod vector;
mod wal;
mod traversal;
mod tql_ast;
mod core;
mod cognitive;
mod index;
mod 新模块;     // ← 添加这一行
```

### 添加新属性测试

在 `tests/core/proptest_core.rs` 中追加新的 `proptest!` 块：

```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn 新不变量_描述(input in 生成策略()) {
        // 执行操作
        // prop_assert!(不变量条件);
    }
}
```

---

## CI/CD 管线

TriviumDB 通过多条独立 GitHub Actions 工作流执行测试、故障注入、基准观测和发布。

常规 CI 与嵌入式发布验收覆盖 Core、Rust/Python/Node SDK 和 CLI。Server 与 WebUI 采用 nightly 开发流程，服务端自测独立执行，浏览器与截图诊断由开发者按需手动运行，见 [nightly WebUI 开发诊断](webui-testing.md)。

### 1. 核心 CI 管线 (`ci.yml`)
面向 master/dev 的 Push 或 PR 会运行多平台 Rust 测试、CLI/TUI、Python/Node 生命周期、短时 fuzz、ASan、TSan、Miri、覆盖率、类型存根和 Clippy 等门禁；另有内存压力观测，以及手动 mutation/benchmark Job。平台矩阵和 Job 数会随工具链演进，具体以 workflow 为准。

- **Compile Check**：默认、Python、Node 和 aarch64 编译检查
- **Rust Test**：Linux/Windows/macOS x86_64、macOS ARM64，并以 QEMU 补充 Linux ARM64
- **Test Hooks**：多平台补跑确定性交错、物理策略差分、定点 I/O 故障和发布阶段强杀；发布质量门禁同样执行这些套件
- **Binding Lifecycle**：Python wheel + mypy/pyright；Node 原生绑定、生命周期测试与 npm tarball 的 TypeScript 消费运行验收
- **Short Fuzz / ASan / Coverage**：解析器、WAL、QuIVer fuzz，unsafe 路径检测与行覆盖率 80% 门禁。Unix 硬件崩溃样本的子进程按用例将所注入的 SEGV/BUS 或 FPE 信号交给操作系统直接处理，保留 ASan 内存插桩和其余选项；父测试进程沿用完整 ASan 配置，并验证异常终止与恢复内容
- **TSan / Miri**：独立的数据竞争与内存安全子集检查；覆盖率任务同时启用 `test-hooks`
- **Python Stubs / Lint**：存根生成漂移、stubtest、类型断言和 `clippy -D warnings`
- **Manual Mutation / Benchmark**：benchmark 先编译全部目标，再运行无外部数据依赖的稳定套件并上传 `target/criterion/` 与 `target/bench-reports/`

独立的 `benchmark-reports.yml` 还会手动或定时运行 `ci_report`、查询 Criterion 与索引/图基线，均只生成 artifact，不作为合并门禁。

### 2. 持续模糊测试 (`fuzz.yml`)
由于深度的 Fuzz 测试非常耗时，配置为每周日自动运行的长时任务：
- 每个目标运行 600s，发现深层隐患
- 崩溃用例（Crash Corpus）自动打包为 Artifact 供本地复现

### 3. 发布管线 (`release.yml`)
打上 `vX.Y.Z` tag 后自动触发：
- **版本预检**：自动检测 PyPI / NPM 库是否已存在该版本
- **多语言绑定构建**：
  - Python (PyO3 + Maturin)：编译 Linux/macOS/Win 跨平台 Wheels
  - Node.js (napi-rs)：编译跨平台原生模块
- **自动发布**：分别上传至 PyPI 和 NPM 注册中心

### 覆盖率门禁阈值

| 指标       | CI 门禁阈值 | 当前值 |
| ---------- | ----------- | ------ |
| 行覆盖率   | 80%         | 93.29%（历史快照） |
| 函数覆盖率 | —           | 91.68%（历史快照） |

> ⚠️ 变异测试不建议放入 CI 常规流程（单次运行可能超过 30 分钟）。CI 中已配置为手动触发（`workflow_dispatch`），作为版本发布前的质量门禁。

---

## 附录：依赖工具版本

| 工具             | 用途       | 安装方式                                |
| ---------------- | ---------- | --------------------------------------- |
| `proptest 1.11`  | 属性测试   | `Cargo.toml` dev-dependencies（已内置） |
| `cargo-llvm-cov` | 覆盖率度量 | `cargo install cargo-llvm-cov`          |
| `cargo-mutants`  | 变异测试   | `cargo install cargo-mutants`           |

## 向量存储开销回归

`tests/storage/vector_memory.rs` 验证旧 BQ 默认零常驻、显式视图按容量计量和写入失效、两种持久化模式的合法空 BQ 块、mmap 单层零拷贝、混合层 COW/墓碑复用及过滤查询与 Rom 对照。`brute_force` 的独立串行 reference 覆盖 FP32/FP16/u64、尾维、同分、过滤、空段与混合分段；`test-hooks` 保留显式合并视图发布前的独占交错验证。历史 v5 fixture 使用独立的 32/48-chunk 位布局。

```bash
cargo test --test storage vector_memory
cargo test --lib 分段精确扫描
cargo test --features test-hooks --test concurrency 显式连续视图交错
```

## 文本索引生命周期

`tests/storage/text_index_lifecycle.rs` 覆盖 Rom/Mmap 未加载保存、正文替换、关键词幂等、节点删除和 ID 复用、WAL 回放、预算拒绝、写入失败重试、只读零写和旧 v2 快照兼容。BM25 分数使用独立词袋 reference 校验；短中文检索同时验证 BM25 与 `CONTAINS` 扫描回退。Rust、Python、Node 的公共 API 测试复用 `tests/contracts/public_cases.json` 中的文本生命周期用例，并检查关闭后构建错误的透传。

```bash
cargo test --test storage text_index_lifecycle -- --test-threads=1
cargo test --test contracts 共享json契约由rust适配器完整执行
```


## 写入边界与索引事实一致性

- `tests/core/write_boundaries.rs` 覆盖三种 dtype、Rom/Mmap、解析缓存开启/关闭的 JSON 深度边界，验证当前读取、WAL 独立回放及关闭重开；更新入口同时验证预算拒绝后的原值、向量和 WAL。`$inc` 通过固定独立数值样例检查精确整数、跨符号边界、小数、溢出和整份补丁原子性。
- `tests/fault_io/wal_failure_state.rs` 在 `test-hooks` 下分别注入 WAL flush/fsync 故障，子进程直接退出，再验证此前确认记录、隔离后的所有拒绝结果及恢复后的再次持久化。未确认操作允许按已持久化日志恢复；已确认数据要求完整核对。
- `tests/storage/sidecar_facts.rs` 验证同长度 Payload 更新与边内容更新后混入旧 sidecar 的处理，检查严格拒绝、属性索引扫描回退、历史索引重建、平行边入度、分数顺序与只读零写。
- `tests/node/write_boundaries.js` 与 `tests/python/write_boundaries.py` 由各端公共 API 入口执行，覆盖直接/批量/事务、查询、完整整数边界、合法容器往返，以及无效输入拒绝后的内容和 WAL。


- `tests/core/index_graph_boundaries.rs` 验证唯一约束删除、部分覆盖索引的完整结果、复合唯一大整数、UTF-8 标签边界、图堆准入、零入度清理、三种 dtype 的 ID 分配耗尽、WAL 回放和只读零写。内核单元测试独立核对图容器容量、字符串分配和复合唯一映射账本。
- `tests/fixtures/numeric_ids/wal_v3.bin` 提供固定完整 u64 ID 跨端样本；Rust 核对完整记录、向量与边，Python 安装制品核对精确整数，Node 安装制品核对字符串 ID 与数字输出拒绝。`tests/node/numeric_boundaries.js` 覆盖数量转换、同步/异步检索、遍历、索引查询及自动分配的 WAL 原子拒绝；`tests/python/index_graph_boundaries.py` 覆盖绑定的索引与图持久化行为。两者均接入现有公共 API CI 入口。

- `tests/core/query_numeric_indexes.rs` 使用独立固定结果验证临时唯一键冲突、原键恢复、删除与 WAL 回放，混合类型范围的 LIMIT/OFFSET，完整 u64 投影/参数/比较/聚合，以及正规数、次正规数和正负零。索引 DDL 覆盖七类入口的预算拒绝、定义保留、配额调整重试与只读重开；唯一构建失败继续保留已有非唯一索引。
- `tests/node/query_numeric_indexes.js` 与 `tests/python/query_numeric_indexes.py` 在实际绑定中执行最终唯一键、数值精度与构建预算断言；Node 用例同时接入独立安装的 npm 制品验收。差分参考模型使用自身的有界整数运算，完整 u64 域由独立常量样例验收。


## 类型、解析与加载边界回归

- `tests/storage/wal_integrity.rs` 覆盖首、中、尾完整帧的 CRC 和合法 CRC 下的解码错误、非法长度、反复打开后原日志保持，以及物理尾帧的每个截断位置。恢复原始测试副本后核对三条已确认记录；原始日志保留和恢复成功分别断言。
- `fault_io::migration_publication` 在真实发布屏障下直接竞争目标锁文件，独立证明排他租约被持有；各平台均核对锁竞争错误。随后验证碰撞文件保留、提交前回收及提交后重开内容。
- `tests/storage/quiver_facts.rs` 在 Rom/Mmap 中验证有效索引、相同长度的节点替换与向量更新、旧 sidecar 混入、历史 v1 元数据、严格错误与只读回退。`memory_safety_` 子集包含 BQ 空存储、越界、巨大维度、畸形几何、最后有效行和尾位断言，随现有 Miri job 执行。
- `tests/query_memory` 使用带 epoch 的独立分配器测量查询线程的堆请求峰值。20 节点、400 条边和 512 KiB 查询上限覆盖普通、具名边、OPTIONAL 和 SET 的复制前拒绝；解除限额后核对完整计数、写入和重开内容。共享 SDK 契约另验证三种 dtype 下读写 TQL 的结构化预算错误及资源归还。

- `tests/generation/lease_identity.rs` 验证根目录创建前后重新构造默认 Store、规范路径、点路径、Windows 大小写别名和显式运行目录的租约一致性；实际子进程使用相对根目录构造 Store 后切换工作目录，持有旧 Reader 时回收必须失败，进程退出后再验证回收成功。
- `tests/core/system_boundaries.rs` 验证共享前台余额为零、预算释放后重试、Top-K 前的 Payload 零读取，以及大库中小范围查询的增量准入。Rust 范围 Filter 通过独立真值表对照 JSON、Prepared AST、Ordered 索引和 Rom/Mmap 重开，覆盖 2^53 相邻整数、i64/u64 端点、小数、Infinity 与 NaN。有限 f64 阈值按其实际表示值比较，完整整数阈值使用 `Filter::Range`。
- `graph::reachability::admission_tests` 覆盖详细、紧凑、并行和子图执行的工作区拒绝、Metadata 复制中途失败与账本归还；`resources::tests::工作区扩缩容拒绝与释放准确维护两类账本` 验证前台与维护凭证扩缩容。
- 三端复用 `tests/contracts/system_boundaries.json` 的相连集合删除与预算用例。Python 安装 wheel、Node 本地原生模块及独立安装的 npm 制品均执行三种 dtype 的拒绝、重试、反向边/平行边/自环删除和只读重开断言。`tests/fault_io/detach_batch.rs` 在 WAL flush、fsync 和事务应用处注入故障，以独立完整状态模型验证原子恢复与再次打开。

- `tests/query/match_boundaries.rs` 使用独立物理边样例验证索引前后、反向、具名边、多段平行边与双向自环的行重数；覆盖零跳、环与绑定恢复。父用例在独立进程的 2 MiB 栈中执行 12,000 跳路径、程序化深模式及 55 节点指数路径的预算/期限回归，并要求完整回执。ARM64/QEMU 环境中的小栈解析与路径子进程显式携带仿真器和目标动态库目录，执行相同断言及完成回执。`test-hooks` 用屏障确认展开已经开始后，再取消或等待真实期限。Linux CI 同时运行 release 小栈解析与图遍历测试。
- `tests/core/retrieval_validation.rs` 验证空库、持有库和重开后的 f32/f16 非有限输入、空 anchor、直接物理算子，以及 `usize::MAX` 附近的 Top-K/召回池在精确和索引路径的完整内容。Rust、Node 与 Python 共享 `graph_query_boundaries.json` 的计数需求；两端实际产物同时验证结构化非法向量错误。

- `tests/core/retrieval_migration.rs` 以固定整数和几何样例验证各检索入口的完整 `u64` 位模式、汉明分数、Hook 改写拒绝、极端有限浮点自相似度、重开、已有迁移目标字节保持和完整新库迁移。内核向量用例覆盖标量、当前 CPU 支持的 SIMD、384 维缓存范数、极端尺度与零向量，预期来自独立几何关系。
- `database::migration::tests` 用真实文件系统路径作为独立判据，核对主文件、多段与 Unicode 侧文件的大小写别名在目标租约及暂存目录创建前被拒绝，空闲锁别名可复用，区分大小写的目录允许独立目标。Linux 额外检查非 UTF-8 侧文件（macOS Runner 在创建该夹具时即返回 `EILSEQ`），Unix 检查悬空主文件与协调锁链接。`tests/core/retrieval_migration.rs` 对 Rom/Mmap 的已有多代数据库验证迁移拒绝后完整文件组不变、节点/向量/Payload/边可重复打开与持久化，并验证空闲锁别名下的新库完整迁移。
- `tests/fault_io/migration_publication.rs` 验证暂存构建、首文件发布后、marker 提交前与提交后故障，核对原库、目标完整内容及重试；确定性交错验证目标锁和碰撞文件保留。Node/Python 通过共享 `retrieval_boundaries.json` 在安装制品上核对整数检索、有限浮点分数和迁移拒绝的结构化错误。
- `cargo test --features test-hooks --test fault_power migration_publication -- --test-threads=1` 在首文件发布后、marker 提交前和提交后使用屏障确认并强杀子进程，不依赖析构清理。Rom/Mmap 源库均逐字节保留；提交前目标回滚，提交后核对完整节点、零向量、Payload 和边，再次持久化并重开。Unix 额外核对主文件只有一个链接；只读成功与拒绝路径均验证文件组零写。该用例模拟进程终止，不代替真实断电测试。
- `python -m unittest discover -s tests/tools -p 'test_release_*.py'` 验证真实 Git 发布来源与发布摘要命令入口；摘要用例覆盖上传成功、失败、取消、上游阻断、既有版本跳过、门禁失败、两端不同结果及状态不一致，不能以 `should_publish` 代替实际作业结果。

- `tests/fault_io/schema_publication.rs` 在发布前备份、属性/图副本写入和提交后 WAL 清理处注入故障，覆盖 Rom/Mmap、单字段/复合唯一约束及创建/删除。断言原始错误、持有句柄时读写隔离、已确认数据保留，以及按提交点重开后的约束、重复值判定和再次持久化。
- `tests/storage/index_schema_boundaries.rs` 使用独立字段/记录样例验证 NUL、转义、Unicode 与同分隔符不同边界定义的往返、旧 v7 复合副本回退和只读零写；CRC 一致的畸形图样例覆盖 JSON、Metadata、有限边权变更、端点、目录、数量、UTF-8 和保留位，ReadOnly/Immutable 在采用前拒绝，文件组保持原样。正向样例核对平行标签、自环、大 ID 和完整容器 Metadata。两端 `kernel_boundaries` 使用共享字段契约在实际 SDK 产物中验收；Node 同时覆盖嵌套 NUL、`__proto__` 自有属性、CRUD/事务/图/TQL 输出和非法 JSON 拒绝后的 WAL 保持。

- `cargo test --test storage kernel_boundaries -- --test-threads=1`：三种 dtype 的 Rom/Mmap 交叉打开、拒绝时文件组字节保持、带类型空检查点与未检查点 WAL、实例/共享预算准入及预算不足后的恢复重试。Windows 用真实文件共享标志阻止 `.vec` 替换，核对当前句柄隔离及重新打开后的向量和精确结果。
- `cargo test --release --test query parser_limits -- --test-threads=1`：独立进程在 2 MiB 栈中解析连续 NOT、长加法/乘法、逻辑链与函数嵌套，要求正常返回拒绝。普通测试覆盖合法边界、最大维度负向量、完整输入消费，以及非法写尾部下的事实和 WAL 保持。
- Linux CI 增加 release 小栈解析回归；现有跨平台测试继续执行 Windows 文件占用回归。Python/Node 公共 API 使用 `tests/contracts/kernel_boundaries.json`，基于实际原生模块核对错误码、完整详情、原值与重开事实。
- WAL 独立样本搭配同类型基础检查点，历史 fixture 保留原始字节。格式规格覆盖 v10 新增文件头字段；v5–v9 兼容验证与 v10 类型校验分别执行。
