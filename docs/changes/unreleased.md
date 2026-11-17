# 未发布变更

> 后续开发以 `0.9.1` 为基线，已交付能力见 [0.9.1 中英文说明](0.9.1.md)。

---

## 中文说明

### SDK 接口清单、外部迁移与单格式准入

- 普通打开先取得已有协调租约，再检查 WAL 和索引组件的可变文件头。关闭或检查点清理 WAL 时，竞争的读写打开返回 `TDB_DATABASE_LOCKED`。缺少协调文件的历史来源先完成格式拒绝，保持源文件零写；首次预检期间出现新 Writer 时重新探测租约。请求计量使用标准库 `try_update` 名称，保留原子饱和计数。历史发布样本按原始字节检出，在 Windows 保持来源摘要。
- [SDK 正式接口清单](../sdk-contract.md)登记 Rust 应用入口与内核扩展边界、Python/Node 声明来源、TQL 与 JSON 协议、错误字段和生命周期。`1.0.0-beta.1` 目标为 SDK 冻结，`1.0.0` 再冻结正式内核契约；当前版本元数据保持 `0.9.1`。
- 生产内部函数与当前格式用例按职责命名，Hook 阶段派发统一为 `invoke_stage`。主文件和 CLI 直接比较唯一的当前格式身份，存储观测的最低格式字段取同一身份。磁盘格式字段与已公开的 FFI 符号沿各自持久化及插件契约维护。
- 普通打开仅处理当前主格式 15、WAL 7、统一索引容器及当前组件布局。在创建协调文件前和获得租约后检查格式，历史身份返回 `TDB_MIGRATION_REQUIRED` 及 `component`、`found`、`current`、`migration_entrypoint`。当前格式的 WAL 和文件组中断恢复继续由内核维护。
- `scripts/migrate.py` 调用独立 workspace 的原生工具。来源恢复后端拥有独立源码，通过逻辑事实协议调用当前内核写出目标，生产模块装配维护当前格式。支持主格式 5–15、三种 dtype、Rom/Mmap，在副本恢复并比较节点、向量位模式、Payload、边元数据、登记文本、索引定义、文档、命名图及回执，核验后发布新目录。旧普通复合索引字段无法无歧义恢复时明确拒绝。
- 普通文档、解析文档及片段标签统一使用结构 2，目标主格式标记为 15。外部迁移统一旧结构 1 并保留文档身份、范围、修订、业务字段、唯一约束和持久回执。来源格式 5–9 按完整向量几何识别类型，旧空检查点要求 `--source-dtype f32/f16/u64`，缺参返回 `TDB_MIGRATION_DTYPE_REQUIRED`。
- 工具身份更新为 `tdb-migrate-2`，编排协议保持 1。来源与目标分别使用独立暂存子目录，当前数据库读取要求完整 `.idx`。当前独立 SSD 制品及属性 sidecar 通过各自显式入口读写，检查点暂存组件由内核发布流程管理。
- 检查模式依赖既有来源锁并保持来源零写。执行模式可创建协调锁，保留来源数据。工具实施源/目标租约、摘要核对、资源准入、目标归属及原子目录提交；发布确认标记在复核后移除，进程中断可用原命令重试，发布后错误提供提交结果。
- QVSSD 全部 codec 统一使用包络 2。迁移目标保留完整向量并清除旧压缩缓存，宿主重新配置编码与缓存预算。来源快照 manifest 保留在来源；目标按普通数据库交付，Immutable 分发使用当前 SDK 重新发布快照。
- 独立样本由不可变 `v0.9.0` 源码生成，覆盖三种 dtype × Rom/Mmap × 检查点/WAL；`v0.9.1` 原始源码样本补充解析文档、来源映射、命名图与持久回执。验收核对完整内容、再次重开、未提交事务与截断尾帧、源文件保持、拒绝、预算、租约和发布前后进程退出重试；跨平台命令加入 CI。
- 格式 11/WAL 4 与格式 12/WAL 5 的既有原生 wheel 样本接入外部迁移验收，逐文件核对来源摘要。三种 dtype 分别迁入 Rom/Mmap，核对默认图及命名图、修订、旧回执状态与原命令幂等重试，后续写入验证自动 ID 和唯一约束。脚本入口覆盖两种来源的检查、发布及同源重试。

```bash
cargo build --locked --manifest-path crates/triviumdb-migrate/Cargo.toml --target-dir target/migration
python scripts/migrate.py --source /data/old.tdb --destination /data/current --check
python scripts/migrate.py --source /data/old.tdb --destination /data/current --execute
```

> **迁移提醒**　停止来源持有者并保留完整备份。目标是新目录，数据库为其下 `database.tdb`。支持范围、资源选项和确认重试见[迁移指南](../migration.md)。平台验收以对应提交的 CI 为准。

### 正式命名与研究路径隔离

- QuIVer 正式批量构图入口统一为 `batch_build`，采用此前数据库默认使用的并行构图、延迟反向剪枝和连通性保护。旧串行、追加式及 checked 对照构图从生产库移出；逐节点更新继续使用现有增量入口。`TRIVIUM_EAGER_PRUNE` 和旧构图选择开关随研究对照退出。
- SM 查询模块采用 `index::quiver::sm`，快照类型为 `SmIndex`。`search`、`search_traced` 和 `search_with_vectors` 自动使用生产派发，调用参数移除 `SmSearchMode`；独立标量参考仅参与内部正确性测试。研究图导入／导出和串行研究构图归入冻结快照。
- 压缩冷热运行时采用 `index::quiver::runtime::{QuiverRuntime, QuiverRuntimeOptions, QuiverArtifactOptions, QuiverRuntimeStats}`，分别替代原 `ssd` 模块的 `SsdQuiver`、`SsdRuntimeOptions`、`SsdOptions` 和 `SsdStats`。数据库及类型化 Reader 使用 `configure_quiver`、`search_quiver`、`quiver_stats`、`close_quiver`；原 INT8 别名移除。中心化 INT4／INT8、查询精度、缓存额度及冷热切换沿既有配置执行。
- TSNG 研究模块、crate 根导出、Database／Reader 对照入口及专用双队列导航退出正式 SDK；配套研究用例和消融基准随源码快照保留。正式查询继续由 TQL／Planner 和已有检索入口执行，文档 ANN 使用的自定义评分接口继续维护。EXPLAIN 的 `industrial_access_path` 字段改为 `vector_access_path`，取值与规划行为保持一致，原结构化 `access_path` 独立保留。Cargo 的 `ablation` 特性移除，研究目录排除出发布包，已有 ICDE 归档保持原始内容。
- 保留部署诊断覆盖 `TRIVIUM_NO_AVX512`、`TRIVIUM_DISABLE_384_KERNEL` 和构图计量 `TRIVIUM_BUILD_PROFILE`。硬件自动派发、标量回退、合法维度处理与 `test-hooks` 故障验证继续维护。

> **调用提醒**　这是 SDK 冻结前的接口整理，Rust 调用方按上述名称更新导入和调用参数，各语言的 EXPLAIN 消费方改读 `vector_access_path`。Python／Node 的公共方法、压缩运行时 JSON 字段和类型声明沿用原契约。命名整理保持默认算法参数，格式准入与外部迁移见上节。

### 统一索引容器与验收范围收敛

- 主文件格式 15 保存持久化数据库身份与每次发布的检查点标识，`.idx` 容器以主文件长度、CRC 和两种身份绑定文件组，拒绝另一数据库或同库旧检查点的容器。QuIVer、登记正文与关键词、属性索引定义、图目录和可选压缩重排组件集中保存，正常提交清理暂存组件。完整布局见[文件组说明](../storage-format.md)。
- 容器使用固定目录、组件 CRC 与 64 KiB 对齐范围。属性及图索引直接映射组件，SSD 读取包含组件基址；已持有的 Reader 沿旧文件句柄完成读取。发布日志、快照及 CLI 导出路径保护共享制品目录，快照同时包含已发布的 SSD 组件。
- 容器缺失、身份错配和非法目录返回结构化错误。登记文本和属性索引定义损坏时保留源件并拒绝不完整加载，显式文本重建接受宿主提供的完整内容。重开处理已提交的待清理日志时，先验证完整容器和各组件；损坏时保留恢复日志、备份及暂存组件。
- 删除正常内核自动升级旧磁盘格式的测试，保留历史 fixture 和当前格式的数据往返、损坏拒绝、事务恢复、只读零写及资源释放契约。Python／Node 文本用例改用当前格式核对重建与重开。测试清理范围补齐统一容器，故障矩阵增加容器写入、同步及替换失败。

> **使用提醒**　新格式由当前开发版写入，旧版内核无法打开。容器发布会重写完整容器，需为暂存组件、候选容器和旧代备份预留空间；完整数据备份使用快照 API。

### CLI JSONL 数据正确性

- 导出按节点 ID 升序读取原始 dtype。u64 向量使用 JSON 无符号整数保留完整位模式，f32／f16 保留对应数值；每条边同时输出 `target`、`label`、`weight` 和 `metadata`。
- 导出在数据库打开前检查主文件和关联制品名称族，覆盖 `.text`／`.text.meta`、Payload 代际、发布日志及 QuIVer SSD 制品，并通过实际文件身份检查路径别名。结果先写暂存文件并同步，再原子替换独立的导出目标。
- JSONL 解析时按目标 dtype 的原生表示缓冲向量，完整文件的全部节点和边通过同一笔内核事务提交。维度、重复 ID、无效分量和缺失边端点等预检错误使整批请求失败，命令返回非零退出码，已存在记录与 WAL 保留原内容。
- 事务通过插入序号解析自动分配的真实 ID，支持混合显式／自动编号、保留删除后的 ID 水位，以及指向本批后续节点的边。Rust `upsert_edge_from_insert()` 与 Rust／Python／Node 共用的 Console `linkFromInsert` 请求复用此能力；持久化继续使用现有 Insert／Link WAL 记录。
- u64 JSONL 导入要求范围内的无符号整数字面量，拒绝浮点及指数表示，避免 JSON 浮点解析造成整数位模式舍入。边 `metadata` 缺省为 `null`，已有无 metadata 的 JSONL 记录按该默认值导入。

> **数据交换提醒**　旧版经 f32 舍入的 u64 JSONL 文件需要从原始数据库或完整备份重新导出。完整数据库备份使用快照接口，保留文档正文、命名图、文本登记与索引定义。

### 边版本失效与认知算子数值边界

- Server 删除节点和批量删除时，收集实际入边、出边及全部标签，成功提交后更新关联边和端点版本。不带标签的 `unlink` 按实际存储边更新版本，覆盖 Server 打开前已持久化的边。携带旧边 ETag 的事务返回 `409 TDB_HTTP_PRECONDITION_FAILED`，保留 `legacyCode=WRITE_CONFLICT`。失败事务保留提交前版本，无关边沿用原 ETag。
- DPP 在对数域统一缩放质量核与正则项，采用 `f64` Cholesky 工作区和宽类型范数计算。高质量分数、大有限向量与 `QUALITY_WEIGHT 10` 保持确定性选择，每个候选最多选择一次。高级检索与 TQL 共用实现，工作区按实际元素宽度准入。
- NMF 的初始化、矩阵乘积、乘法更新和残差计算使用 `f64`，返回前检查分解因子的有限性。`[1e30, 0]` 等有限大向量可生成有限主题权重，矩阵预算计入输入、输出及各中间矩阵。
- TQL 管线的 Rust `ScoreValue.value` 调整为 `f64`，图算法结果沿投影、排序、聚合及 HTTP 保留该精度。权重 `1e-40` 对应的约 `1e40` 调和中心性保持有限值。Python／Node 继续使用其浮点数类型。SA-PPR 的种子适配核对有限 `f32` 范围，越界返回结构化查询错误。
- 编辑恢复用例更新为当前 WAL v7 头断言，继续验证封条完整性、未提交事务隔离和已确认回执恢复。

> **Rust 调用提醒**　直接构造 `ScoreValue` 的调用方将 `f32` 分数显式转换为 `f64`。向量存储 dtype 与 `SearchHit.score` 沿各自公共契约使用。

### Payload 预过滤与 Hook 返回契约

- 行级 64 位预过滤签名与查询掩码共享 JSON 的 `Eq/Hash` 规则，按顶层字段及完整值编码，覆盖 null、空及嵌套数组／对象、正负零、字面点号字段与空字段名。完整 Payload 谓词继续核对候选，写入、更新和重开使用一致的等值语义。
- Hook 改写向量在转换为目标 dtype 后再次执行 Core 维度与有限性校验。`[70000, 0]` 转为 f16 后溢出时返回 `TDB_INVALID_VECTOR`，后续合法查询正常执行。
- Node 重排及后置命中回调返回 `null` 时保留当前命中，自定义召回返回 `null` 时执行内置召回。空数组显式替换为空结果，与 Core／Python 的可选替换语义一致。
- Node Hook 返回 Promise、非法向量表示或错误命中成员时返回 `TDB_HOOK_EXECUTION`。命中列表按 ID 与有限 f32 分数整批校验后替换，正常的 Promise 拒绝通知由适配器接收。公共声明补充返回语义，类型消费覆盖合法 nullable 返回及异步返回拒绝。

### 文本恢复、向量索引与检索回调

- 检查点在文件组发布前同步 WAL v7 的唯一封条，TMET v2 将封条、主文件长度及 CRC、文本长度及 CRC 绑定并校验。恢复按封条区分检查点覆盖的旧删除与后续真实删除，覆盖节点 ID 复用、立即／延期加载、Rom／Mmap 和 Off 同步模式。恢复边界无法确认时返回 `TDB_RECOVERY_REQUIRED`，定位字段为 `text_index.recovery_boundary`，保留原始文件供处理。历史文本格式由外部迁移工具读取和转换。
- WAL 回放实际插入新向量后使已有 QuIVer 路由失效，后续普通搜索按配置重建或精确回退，恢复后的当前实例可检索全部已确认节点。
- 当前文本检查点在文件组发布中写出完整元数据及恢复封条，覆盖元数据发布后、日志清理前失败的重试。历史 WAL 在外部工具的副本中按帧与事务暂存执行内存准入，恢复完成后交付逻辑事实，由当前内核生成目标文件组。
- 自定义召回、召回后、图扩散前和 DPP 最终回调结束时立即检查 Hook 错误，统一返回 `TDB_HOOK_EXECUTION`。空召回和普通返回沿相同错误传播契约执行。
- `$set`、`$inc`、`$unset` 的操作数在应用整份补丁前验证为 JSON 对象。非法操作数返回 `TDB_INVALID_INPUT` 及 `patch.<操作符>` 字段定位，Payload 与 WAL 保持提交前内容。

> **升级与恢复提醒**　升级前保留完整文件组备份。当前开发内核使用主格式 15、WAL v7 与 TMET v2，历史数据库通过 `scripts/migrate.py` 显式迁入新目录。旧版本环境从原来源或升级前备份打开。遗留恢复歧义通过完整备份恢复数据库，或由宿主保留的事实创建新库并登记正文与关键词。已被旧版本清除的文本按备份或来源重建。

### 查询计量、路径深度与文档解析

- Payload 查询额度和 PROFILE 使用每次管线执行独立的账本。属性并行扫描及图工作线程继承所属查询的账本，实例累计统计继续由存储层维护。复用管线上下文时重新开始本次计量、阶段计数和 PROFILE。
- 全路径枚举分别处理查询 `DEPTH` 与执行深度预算。预算阻止继续扩展时，Core 按 Error 策略返回 `TDB_TRAVERSAL_BUDGET_EXCEEDED`，按 Partial 策略返回 `truncated=true`。TQL 管线使用完整结果的 Error 策略，查询自身的深度及路径数量限制按各自语义执行。前沿容量在构造邻接工作区前准入。
- Markdown 中的 HTML 按实际事件内容解析，正文逐段回映射至原文。引用符号和列表缩进保留其 Markdown 结构作用，HTML 实体转换保留对应来源范围。
- Markdown 内嵌 `script`、`style`、`template` 按完整元素范围过滤，覆盖跨事件正文、模板嵌套及 raw-text 词法状态。行内代码、代码围栏和转义文本保留字面内容。
- 路径内平均边权和多路径均值采用 `f64` 累加，再生成 `f32` 分数。两个 `f32::MAX` 边权的平均值返回 `f32::MAX`。TQL 路径聚合分数越出有限 `f32` 范围时返回结构化查询错误。
- TQL `AVG` 使用补偿求和，在中间和溢出时按最大绝对值缩放，覆盖普通整数、同号大数、正负抵消、零值和极小有限数。两个 `1e308` 的平均值返回 `1e308`。

> **解析更新提醒**　已入库文档通过宿主的重新解析与发布流程应用本轮正文及来源映射修正。

### 文档恢复、确定性排序与图遍历边界

- 修复普通节点删除后复用相同 ID 发布分批文档，在检查点已发布、WAL 清理失败时，旧 Delete 重放清除新正文归档的问题。恢复按检查点水位和上传修订协调删除级联，保留新代正文、结构清单、片段与上传状态。检查点后的有效删除继续回收对应归档和候选。
- 修复 `RETURN _.nested AS picked ORDER BY picked.rank LIMIT 1` 误用事实层 `rank` 索引的问题。排序别名按投影值求值，依赖投影的排序读取完整候选后分页。
- 显式排序的同分行按节点绑定、命名图边身份、管线绑定与路径身份确定顺序，聚合行使用稳定分组键。范围索引及排序方向保持相同的同分分页规则。
- SA-PPR 管线在种子、扫描边、前沿与深度扩展前执行遍历预算，工作区按实际增长准入，并在图步骤响应取消。预算耗尽返回 `TDB_TRAVERSAL_BUDGET_EXCEEDED`。管线沿用完整结果契约，统一采用 Error 耗尽策略。评分阶段保留紧凑节点和分数，最终投影按需读取 Payload。
- 扩散的种子质量与绝对边权归一化采用 `f64` 累加。两个权重均为 `f32::MAX` 的目标，在深度 1、重启比例 0、单位种子分数时分别获得 `0.5`。
- 双向最短路径在初始化两端和增长前沿前检查预算，Partial 输出的计数保持在额度内。普通最短路径先核对端点，缺失节点的同起终点查询返回空路径结果。
- 回归验收覆盖 Rom／Mmap、受控检查点 I/O 故障、再次持久化与重复重开、索引前后分页、各维预算及取消后的重试，并由 Rust／Python／Node 共享查询样例核对完整内容。

> **恢复提醒**　已被旧版本清除并再次持久化的正文归档，需要使用保留的快照恢复，或根据宿主的原始文档重新发布。

### Node 包的命名导出与 Prepared 类型

- 修复 CommonJS 加载器的动态导出使 ESM 命名导入在加载时抛出 `SyntaxError` 的问题。显式提供数据库、Prepared、运行时、会话、请求上下文及快照函数的公共导出。
- 原生 Prepared 类统一为 `PreparedTql`，与 TypeScript 声明一致。JavaScript 的 `JsPreparedTql` 作为同一构造器的兼容名称，Rust 保留原有 `JsPreparedTql` 导出路径，返回句柄可通过两种 JavaScript 名称执行 `instanceof`。
- 实际 npm tarball 消费验收增加 `.cts` 与 `.mts`，按 NodeNext 编译后分别执行 CommonJS／ESM，核对公开值身份、Prepared 参数及完整查询结果。

---

## English

### SDK inventory, external migration and current-format admission

- Ordinary opens acquire an existing coordination lease before checking mutable WAL and index-component headers. Competing reader or writer opens return `TDB_DATABASE_LOCKED` while shutdown or checkpoint cleanup truncates the WAL. Historical sources without a coordination file are rejected before creating one, preserving source bytes; admission probes the lease again if a Writer appears during the first check. Request accounting uses the standard library's `try_update` name with the same atomic saturating semantics. Historical release fixtures retain their original checkout bytes and provenance digests on Windows.
- The [SDK inventory](../sdk-contract.md) records supported Rust application interfaces and kernel boundaries, Python/Node declarations, TQL and JSON protocols, errors and lifecycle semantics. `1.0.0-beta.1` targets the SDK freeze; `1.0.0` adds the supported kernel contract freeze. Package metadata remains `0.9.1`.
- Production internals and current-format tests use responsibility-based names, with Hook stage dispatch named `invoke_stage`. The main-file reader and CLI compare a single current-format identity directly, and storage diagnostics report that same identity as the minimum format. Disk-format fields and published FFI symbols follow their persistence and plugin contracts.
- Ordinary opens accept only main format 15, WAL 7, the unified index container and current component layouts. Admission runs before coordination files are created and again under the lease. Historical identities return `TDB_MIGRATION_REQUIRED` with `component`, `found`, `current` and `migration_entrypoint`. Current-format WAL and interrupted-publication recovery remain kernel responsibilities.
- `scripts/migrate.py` invokes a native tool in a separate workspace. Its source recovery backend owns independent source code and transfers logical facts to the current kernel, which writes the destination. Production module assembly maintains the current format. The tool accepts main formats 5–15, all three dtypes and Rom/Mmap. Complete node, vector-bit, Payload, edge-metadata, text, index-definition, document, named-graph and receipt facts are compared before a new directory is published. Ambiguous historical ordinary composite-index field lists are rejected.
- Plain documents, parsed documents and fragment tags share document schema 2 in destination main format 15. External migration upgrades schema 1 while retaining document identity, ranges, revisions, business fields, unique constraints and persistent receipts. Formats 5–9 identify dtype through exact vector geometry. Legacy empty checkpoints require `--source-dtype f32/f16/u64`, with `TDB_MIGRATION_DTYPE_REQUIRED` for missing confirmation.
- Tool identity advances to `tdb-migrate-2` with orchestration protocol 1. Source and destination use separate staging subdirectories. Database reads require a complete `.idx` container. Current standalone SSD artifacts and property sidecars retain explicit read/write entry points, and kernel publication manages current staging components.
- Check mode requires an existing source lock and preserves source files. Execute mode may create coordination locks while preserving source data. The tool enforces source/destination leases, digest validation, resource admission, ownership and atomic directory publication. A confirmation marker remains until verification finishes; the same command retries after interruption, and post-publication errors report the commit outcome.
- Every QVSSD codec uses envelope 2. Migration preserves complete vectors and removes old compressed caches; hosts configure encoding and cache budgets again. Source snapshot manifests remain at the source. Destinations are ordinary databases; use the current SDK to publish a new snapshot for Immutable distribution.
- Independent fixtures generated from immutable `v0.9.0` source cover three dtypes, Rom/Mmap and checkpoint/confirmed-WAL states; fixtures from original `v0.9.1` source add parsed documents, source mappings, named graphs and persistent receipts. Acceptance verifies full contents, reopening, uncommitted transactions and torn WAL tails, source preservation, rejection, budgets, leases and retry after process exit before and after publication. Cross-platform commands are added to CI.
- Existing fixtures from historical native wheels now verify external migration for format 11/WAL 4 and format 12/WAL 5, with per-file provenance digest checks. All three dtypes migrate into Rom and Mmap targets, preserving default and named graphs, revisions, historical receipt status and idempotent retry of the original command. Subsequent writes verify automatic IDs and unique constraints. The script entry point covers check, publication and same-source retry for both formats.

```bash
cargo build --locked --manifest-path crates/triviumdb-migrate/Cargo.toml --target-dir target/migration
python scripts/migrate.py --source /data/old.tdb --destination /data/current --check
python scripts/migrate.py --source /data/old.tdb --destination /data/current --execute
```

> **Migration note**　Stop source holders and keep a complete backup. The destination is a new directory containing `database.tdb`. See the [migration guide](../migration.md) for supported sources, resource options and confirmation retries. Platform acceptance depends on CI for the corresponding commit.

### Production naming and research isolation

- `QuIVer::batch_build` is the production batch constructor, using the parallel construction, deferred reverse pruning and connectivity protection previously selected by the database. Historical serial, append and checked comparison builders move out of the production library; individual updates retain the existing incremental API. The research-only `TRIVIUM_EAGER_PRUNE` and historical builder-selection switches retire with those comparisons.
- SM queries live in `index::quiver::sm`, with snapshots represented by `SmIndex`. `search`, `search_traced` and `search_with_vectors` select the production dispatch automatically and no longer take `SmSearchMode`. The independent scalar reference is available only to internal correctness tests. Research graph import/export and the serial research builder are preserved in frozen snapshots.
- Compressed hot/SSD execution uses `index::quiver::runtime::{QuiverRuntime, QuiverRuntimeOptions, QuiverArtifactOptions, QuiverRuntimeStats}`, replacing `SsdQuiver`, `SsdRuntimeOptions`, `SsdOptions` and `SsdStats` from the former `ssd` module. Database and typed Reader calls use `configure_quiver`, `search_quiver`, `quiver_stats` and `close_quiver`; the INT8 aliases are removed. Centered INT4/INT8, query precision, cache budgets and residency switching retain their configuration semantics.
- The TSNG research module, root exports, Database/Reader comparison APIs and dedicated dual-queue navigation leave the production SDK. Related research cases and ablation benchmarks are retained with the source snapshot. TQL/Planner and the existing search APIs remain the application query paths, and document ANN retains its custom scoring interface. EXPLAIN renames `industrial_access_path` to `vector_access_path`, retaining its values and planning behavior alongside the existing structured `access_path`. Cargo's `ablation` feature is removed, research directories are excluded from release packages, and the existing ICDE archive retains its original content.
- Deployment diagnostics retain `TRIVIUM_NO_AVX512`, `TRIVIUM_DISABLE_384_KERNEL` and construction profiling through `TRIVIUM_BUILD_PROFILE`. Hardware dispatch, scalar fallback, supported dimensions and `test-hooks` fault verification remain maintained.

> **Caller guidance**　These API changes precede the SDK freeze. Update Rust imports and call arguments using the mapping above, and read `vector_access_path` in EXPLAIN consumers across languages. Python/Node public methods, compressed-runtime JSON fields and declarations retain their contracts. Naming changes retain default algorithm parameters; current-format admission and migration are described above.

### Unified index container and acceptance scope

- Main-file format 15 stores a persistent database identity and a fresh checkpoint identifier for each publication. The `.idx` container binds to the main file through its length, CRC and both identities, rejecting containers from another database or an older checkpoint of the same database. QuIVer, registered text and keywords, property index definitions, graph directories and optional compressed reranking artifacts share this container; successful checkpoint cleanup removes staging components. See the [file-group specification](../storage-format.md).
- The container uses a fixed directory, component CRCs and 64 KiB aligned ranges. Property and graph indexes map their component ranges directly; SSD reads include the component base offset. Existing readers finish through retained file handles. Publication recovery, snapshots and CLI export protection share the artifact catalog. Snapshots include published SSD components.
- Missing containers, identity mismatches and invalid directories produce structured errors. Corrupted registered text or property index definitions remain intact on disk and cause incomplete loading to be rejected. Explicit text rebuilding accepts complete replacement content from the host. Reopening with a committed journal awaiting cleanup validates the complete container and its components first; corruption preserves recovery journals, backups, and staging components.
- Tests requiring automatic historical disk-format upgrades in the normal engine are removed. Historical fixtures remain available; current-format round trips, corruption rejection, transaction recovery, physical read-only behavior and resource release remain in scope. Python and Node text contracts now exercise current-format rebuilding and reopening. Test cleanup covers the container, and fault matrices include container write, sync and replacement failures.

> **Usage note**　The development engine writes the new format, which older engines cannot open. Publication rewrites the complete container and requires space for staging components, the candidate container and previous-generation backups. Use snapshot APIs for complete database backups.

Subsequent development builds on `0.9.1`. See the [bilingual release notes](0.9.1.md) for delivered capabilities.

### CLI JSONL data correctness

- Exports read original vector dtypes in ascending node-ID order. u64 components retain their complete bit patterns as unsigned JSON integers, and f32/f16 retain their corresponding numeric values. Each edge includes `target`, `label`, `weight`, and `metadata`.
- Export checks the main database and associated artifact name families before opening the database, covering `.text`/`.text.meta`, Payload generations, publication journals, and QuIVer SSD artifacts. File identity checks detect path aliases. Output is staged and synchronized before atomically replacing the independent export destination.
- Parsing buffers vectors in the target dtype's native representation. All nodes and edges in the complete JSONL file commit through one Core transaction. Validation failures involving dimensions, duplicate IDs, invalid components, or missing edge endpoints reject the whole request with a nonzero exit status, preserving existing records and WAL contents.
- Transactions resolve automatically assigned source IDs by insert ordinal, supporting mixed explicit/automatic IDs, ID high-water marks retained after deletion, and edges targeting later records in the same batch. Rust `upsert_edge_from_insert()` and the Console `linkFromInsert` request shared by Rust/Python/Node use this capability. Persistence continues to use existing Insert/Link WAL records.
- u64 JSONL imports require unsigned integer literals within range and reject floating-point and exponent notation to avoid rounding integer bit patterns during JSON parsing. Missing edge `metadata` defaults to `null`, preserving support for existing JSONL records that omit it.

> **Data exchange note**　Regenerate u64 JSONL files rounded through f32 by an older exporter from the original database or a complete backup. Full database backups use snapshots to retain document bodies, named graphs, registered text, and index definitions.

### Edge version invalidation and cognitive numeric boundaries

- Server node deletion and batch deletion collect actual incoming and outgoing edges across all labels, updating edge and endpoint versions after successful commit. Unlabeled `unlink` updates versions from stored edges, including edges persisted before Server startup. Transactions carrying stale edge ETags return `409 TDB_HTTP_PRECONDITION_FAILED` with `legacyCode=WRITE_CONFLICT`. Failed transactions retain their previous versions, and unrelated edges retain their ETags.
- DPP jointly rescales its quality kernel and regularization in log space, with an `f64` Cholesky workspace and wider norm calculation. High quality scores, large finite vectors, and `QUALITY_WEIGHT 10` retain deterministic selection with each candidate selected at most once. Advanced search and TQL share the implementation, and workspace admission accounts for element widths.
- NMF initialization, matrix products, multiplicative updates, and residual calculations use `f64`, with factor finiteness checked before returning query results. Large finite vectors such as `[1e30, 0]` produce finite topic weights. Matrix budgets include input, output, and intermediate matrices.
- The Rust TQL pipeline changes `ScoreValue.value` to `f64`, retaining graph scores through projection, sorting, aggregation, and HTTP. Harmonic centrality near `1e40` from an edge weight of `1e-40` stays finite. Python and Node continue using their floating-point types. SA-PPR checks its seed adapter's finite `f32` range and returns a structured query error on overflow.
- Editing recovery tests assert the current WAL v7 header and continue verifying complete seals, uncommitted transaction isolation, and acknowledged receipt recovery.

> **Rust caller note**　Callers constructing `ScoreValue` directly should explicitly convert `f32` scores to `f64`. Vector storage dtypes and `SearchHit.score` follow their respective public contracts.

### Payload prefiltering and Hook return contracts

- Row-level 64-bit prefilter signatures and query masks share JSON's `Eq/Hash` rules for top-level fields and complete values. Coverage includes null, empty and nested arrays/objects, signed zero, literal dotted fields, and empty field names. Full Payload predicates verify candidates, with consistent equality across writes, updates, and reopening.
- Hook-modified vectors undergo Core dimension and finiteness validation after conversion to the target dtype. Converting `[70000, 0]` to f16 returns `TDB_INVALID_VECTOR` on overflow, and subsequent valid queries continue normally.
- Node reranking and post-recall hit callbacks preserve current hits when returning `null`. Custom recall returning `null` uses built-in recall. An empty array explicitly replaces hits with an empty result, matching Core/Python optional replacement semantics.
- Node Hook Promises, invalid vector representations, and malformed hit members return `TDB_HOOK_EXECUTION`. Complete hit lists are validated for IDs and finite f32 scores before replacement, and the adapter receives ordinary Promise rejection notifications. Public declarations clarify return semantics, with type-consumption coverage for nullable returns and asynchronous-return rejection.

### Text recovery, vector indexes, and search callbacks

- Checkpoints synchronize a unique WAL v7 seal before publishing their file group. TMET v2 binds and checks the seal, main-file size and CRC, and text-file size and CRC. Recovery distinguishes checkpoint-covered deletions from subsequent deletions, covering node ID reuse, eager/deferred loading, Rom/Mmap, and Off synchronization. Unverifiable recovery boundaries return `TDB_RECOVERY_REQUIRED` with target `text_index.recovery_boundary` and preserve the original files for recovery. Historical text formats are read and converted by the external migration tool.
- Actual vector insertion during WAL replay invalidates existing QuIVer routing. Subsequent ordinary searches rebuild or use exact fallback according to configuration, making all acknowledged nodes searchable in the recovered instance.
- Current text checkpoints write complete metadata and recovery seals as part of file-group publication, including retries after metadata publication succeeds before WAL cleanup fails. The external tool admits memory for historical WAL frames and pending transactions in its source copy. Recovered logical facts are then passed to the current kernel to create the destination file group.
- Custom recall, post-recall, pre-graph, and final DPP callbacks immediately propagate Hook errors as `TDB_HOOK_EXECUTION`. Empty recall and ordinary returns follow the same error contract.
- `$set`, `$inc`, and `$unset` operands are validated as JSON objects before applying a patch. Invalid operands return `TDB_INVALID_INPUT` with a `patch.<operator>` target, retaining the pre-commit Payload and WAL contents.

> **Upgrade and recovery note**　Retain a complete file-group backup before upgrading. Development builds use main format 15, WAL v7 and TMET v2. Explicitly migrate historical databases into a new directory with `scripts/migrate.py`. Older engines can open the retained source or pre-upgrade backup. For ambiguous legacy recovery, restore a full database backup or create a new database from the host's retained facts and register its text and keywords. Rebuild text already removed by an older engine from a backup or retained source.

### Query accounting, path depth, and document parsing

- Payload query budgets and PROFILE use a separate ledger for each pipeline execution. Parallel property scans and graph workers inherit the owning query's ledger, while the storage layer maintains instance-wide totals. Reusing a pipeline context starts fresh accounting, stage counts, and PROFILE data.
- All-path enumeration handles query `DEPTH` and the execution depth budget independently. When the budget prevents further expansion, Core returns `TDB_TRAVERSAL_BUDGET_EXCEEDED` under Error policy or `truncated=true` under Partial policy. TQL pipelines use the complete-result Error policy, and query depth and path-count limits retain their specified semantics. Frontier capacity is admitted before constructing adjacency workspace.
- Markdown HTML is parsed from actual event contents, with text mapped back to individual source segments. Quote markers and list indentation retain their Markdown structural role, and HTML entity decoding preserves its source ranges.
- Inline Markdown `script`, `style`, and `template` elements are filtered over their full element ranges, covering text across events, nested templates, and raw-text tokenizer state. Inline code, fenced code, and escaped text retain their literal contents.
- Per-path average weights and averages across paths accumulate in `f64` before producing `f32` scores. Two `f32::MAX` edge weights average to `f32::MAX`. TQL path aggregate scores outside the finite `f32` range return structured query errors.
- TQL `AVG` uses compensated summation and scales by the maximum absolute value when intermediate accumulation overflows, covering ordinary integers, large same-sign inputs, signed cancellation, zeros, and tiny finite values. Two `1e308` values average to `1e308`.

> **Parsing update note**　Apply the text and source-mapping corrections to stored documents through the host's reparse-and-publish workflow.

### Document recovery, deterministic sorting, and graph traversal boundaries

- Fixed stale Delete replay removing the new body archive after a node ID was reused for a staged document and checkpoint publication succeeded before WAL cleanup failed. Recovery coordinates deletion cascades using the checkpoint watermark and upload revision, retaining the new body, manifest, representations, and upload state. Effective post-checkpoint deletions continue to reclaim associated archives and candidates.
- Fixed `RETURN _.nested AS picked ORDER BY picked.rank LIMIT 1` incorrectly selecting the fact-level `rank` index. Sort aliases resolve against projected values, and projection-dependent ordering collects its full candidate input before pagination.
- Explicit ordering resolves ties through node bindings, named-graph edge identity, pipeline bindings, and path identity. Aggregate rows use stable grouping keys. Range indexes and sort direction preserve the same tie-breaking pagination rules.
- SA-PPR pipelines enforce traversal budgets before admitting seeds, examining edges, growing frontiers, and expanding depth. Workspace admission follows actual growth, and graph steps honor cancellation. Exhaustion returns `TDB_TRAVERSAL_BUDGET_EXCEEDED`. Pipelines retain their complete-result contract and consistently use the Error exhaustion policy. Scoring retains compact node IDs and scores, with Payload reads deferred to projections that need them.
- Diffusion accumulates seed mass and absolute edge-weight normalization in `f64`. With a unit seed score, depth 1, restart ratio 0, and two target edges weighted `f32::MAX`, each target receives `0.5`.
- Bidirectional shortest paths check budgets before admitting both endpoints and growing frontiers. Partial-result counters stay within their limits. Ordinary shortest paths validate endpoint existence before handling equal endpoints, returning no path for a missing node.
- Regression coverage includes Rom/Mmap, controlled checkpoint I/O failures, repeated reopening and persistence, pagination across index changes, individual budget dimensions, and retries after cancellation. Rust/Python/Node share complete expected query results.

> **Recovery note**　Body archives already removed and persisted by an older version require restoration from a retained snapshot or republication from the host's original documents.

### Node named exports and Prepared types

- Fixed ESM named imports throwing `SyntaxError` when loading the dynamic CommonJS entry point. The loader explicitly exposes public database, Prepared, runtime, session, request-context, and snapshot-function exports.
- The native Prepared class uses `PreparedTql`, matching the TypeScript declaration. JavaScript retains `JsPreparedTql` as an alias of the same constructor, and Rust retains its existing `JsPreparedTql` export path. Returned handles support `instanceof` with either JavaScript name.
- Installed npm tarball validation includes `.cts` and `.mts` consumers compiled with NodeNext and executed as CommonJS and ESM. Checks cover public value identity, Prepared parameters, and complete query results.
