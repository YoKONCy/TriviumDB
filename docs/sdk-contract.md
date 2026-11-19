# SDK 正式接口清单

> 本文登记应用侧 SDK 的支持范围与兼容边界，作为 `1.0.0-beta.1` SDK 冻结的审查基线。当前交付版本为 `0.9.2`，完整内容见[版本说明](changes/0.9.2.md)。

---

## 目录

- [范围与签名来源](#范围与签名来源)
- [Rust 正式接口](#rust-正式接口)
- [Python 与 Node 正式接口](#python-与-node-正式接口)
- [查询与请求协议](#查询与请求协议)
- [兼容承诺与验收](#兼容承诺与验收)
- [内核扩展与发布通道](#内核扩展与发布通道)

---

## 范围与签名来源

正式接口覆盖下表登记的类型、公开方法、配置字段、返回值与错误字段。每个范围均链接其完整声明；重载、默认值、可空值、同步与异步返回类型以对应声明及公共契约为准。新增方法与字段在实现、声明、文档和实际交付物中同步审查。

| 语言或协议 | 完整声明来源 | 应用导入入口 |
| --- | --- | --- |
| Rust | [`lib.rs`](../src/lib.rs)、[`sdk`](../src/sdk/mod.rs) 及下节登记的应用类型 | `triviumdb`、`triviumdb::sdk` |
| Python | [`__init__.pyi`](../python/triviumdb/__init__.pyi)，wheel 内附带 `py.typed` | `import triviumdb` |
| Node | [`triviumdb.d.ts`](../triviumdb.d.ts)，由 `package.json.types` 指向 | `require('triviumdb')` 或包名导入 |
| TQL | [语言参考](tql-reference.md) 与 Prepared 参数类型 | 数据库查询方法 |
| JSON 请求 | [文档](document-ingestion.md)、[命名图](named-graphs.md)、[QuIVer](quiver.md)、[嵌入控制台数据协议](embedded-console.md) | SDK 请求方法 |
| 错误 | [`ErrorInfo`](../src/error.rs) 与 [错误契约](error-contracts.md) | Rust 错误对象、Python 异常、Node Error |

> **使用提醒**：直接从源码内部路径导入的对象，需要按下文的应用范围或内核扩展范围核对支持状态。以 `_` 开头的绑定桥接与构建生成的 `index.d.ts` 供内部装配使用。

---

## Rust 正式接口

| 接口族 | 正式应用类型与入口 | 支持内容 |
| --- | --- | --- |
| 数据库与配置 | 根导出 `Database<T>`；`database::{Config, AccessMode, StorageMode, MissingIndexPolicy, SearchConfig, BatchSearchConfig, RowOverflowPolicy}` | 打开、类型与维度、读写权限、查询和资源配置；`Database` 上公开的应用方法 |
| 共享所有权 | `SharedDatabase<T>`、`WeakDatabase<T>`、`sdk::RequestContext` | 共享实例、请求借用、弱引用、取消与截止时间 |
| 类型化借用 | `DatabaseReader`、`DatabaseWriter`，以及 `sdk::facade` 与 `database::facade` 的兼容导出 | 读写作用域与各自公开方法；事务状态由内核维护 |
| 节点与默认图 | `NodeId`、`NodeView`、`SearchHit`、`GroupedSearchResult`、`Edge`；`node::Node`、`EdgeDirection` | 可选向量节点、Payload、默认图边及元数据、批量 CRUD |
| 向量与过滤 | `VectorType` 的 `f32`、`f16`、`u64` 实现，`Filter`，数据库检索方法 | dtype、整数精度、向量校验、过滤、同分排序、精确与 ANN 路由 |
| 事务 | `database::{Transaction, TxBuilder, TxOp}` 与数据库事务方法 | 准备、一次提交、失败完成点、唯一约束、原子批量与条件写入 |
| 文档 | `document` 中用于数据库方法的请求、结果、范围、引用、预算类型及 `document::parsing` 的解析输入输出；`sdk::documents::request` | 文档解析、发布、分批上传、回映射、分组检索与修订条件 |
| 命名图 | `named_graph` 中用于数据库方法的身份、写入、作用域、边、预算和结果类型；`sdk::named_graphs::request` | 成员共享、图内独立边、修订与遍历 |
| 文本与属性索引 | 数据库登记、重建、创建、删除、查找与统计方法；`index::text::TextIndexDocument`，这些方法签名引用的 `index::property` 配置和统计类型 | 登记正文与关键词、普通及唯一索引定义、查询结果 |
| TQL | 数据库 `tql`、`tql_values`、`tql_nodes`、`query`、写查询与 Prepared 方法；其签名引用的 `TqlValue`、结果、`QueryControl`、Prepared 与参数类型 | 查询值、绑定、定位、计划解释、执行与取消 |
| 图计算 | 数据库图算法方法及其配置、结果类型；根导出的 Reachability/Subgraph 类型、`GraphStats`、`GraphIntegrityReport`、`GraphRepairReport` | 遍历预算、路径、修复与图算法结果 |
| 压缩运行时 | 数据库 QuIVer 配置、切换、关闭和统计方法；`sdk::quiver::{request, measure_io, QuiverIoStats}` 及这些方法引用的配置和结果类型 | 热内存/SSD、缓存字节预算、请求级 I/O、中心化 INT8/INT4、FP32/INT8/INT4 查询 |
| Hook | `SearchHook`、`HookContext`、`CompositeHook`、`NoopHook`、`FfiHook` 与已声明 FFI ABI | 同步回调、错误传播、重入拒绝、动态库所有权 |
| 资源与维护 | `ResourceRuntime`、`ResourceLimits`、`ResourceStats`、`ResourceFailure`、`ResourceBudgetDetails`；实例统计、关闭和维护状态类型 | 实例/宿主预算、后台工作、排空、关闭、内存与维护观测 |
| 快照与代际 | 数据库快照方法；`storage::snapshot` 的 `verify_snapshot`、`restore_snapshot` 及其 runtime 变体和返回类型；根导出的 `GenerationStore`、`CurrentGeneration`、`GenerationReader` | 完整文件组、目标排他发布、Immutable 代际与读取租约 |
| 嵌入宿主 | `ConsoleHost`、`ConsoleSession`、`ConsoleOptions`、`ConsoleFeature` 与数据库数据请求方法 | 会话权限、请求版本、条件编辑、幂等回执与传输完成点 |
| 错误 | `Result`、`TriviumError`、`ErrorInfo`，公开错误定位与写入完成点类型 | 稳定错误码、必要字段、原始原因与结果判定 |

上述类型在应用签名中引用的 DTO、枚举、参数及返回字段随调用一起进入支持范围。被标记 `doc(hidden)`、仅 `test-hooks` 可用的方法和直接访问存储容器的实现接口归入内核工具范围。Rust 类型参数约束和所有权规则随对应方法一起维护。

`SearchConfig::default().min_score` 为 `0.0`，`with_exact(bool)` 设置既有 `force_brute_force` 字段。Python 的关键字参数 `exact` 与 Node 的可选 `exact` 参数／配置字段复用相同向量召回路径。精确召回读取数据库存储的向量，继续执行调用配置中的过滤、排序、Top-K 和后续处理。高级入口的兼容开关与 `exact` 任一为真时启用精确召回，完整签名和示例见 [检索 API](api-reference.md#search--纯向量图扩散检索-基础)。

---

## Python 与 Node 正式接口

Python 正式对象包括 `TriviumDB`、`Transaction`、`PreparedTql`、`RequestContext`、`ResourceRuntime`、`ConsoleSession`，以及声明中的结果对象 `SearchHit`、`GroupedSearchResult`、`Edge`、`IncomingEdge`、`NodeView`、`ReachabilityStep`、`ReachabilityResult`、`QueryRow`、`HookContext`。模块函数为 `init_logger`、`verify_snapshot`、`restore_snapshot`。完整异常族以存根中公开异常声明为准，调用方通过 `code` 和结构化字段区分错误。

Node 正式运行对象和函数以 `triviumdb.d.ts` 的 `export class`、`export function` 为全集；声明中的全部 `export interface`、`export type` 构成应用参数、结果和错误字段契约。`TriviumDB` 的同步方法及返回 Promise 的异步方法保持各自完成点；`null`、`undefined`、缺省参数和空集合按声明与共享契约处理。

两端完整方法覆盖：构造与生命周期、节点读写、批量与事务、边读写、检索、遍历及算法、TQL/Prepared、文本及属性索引、文档/命名图/压缩运行时请求、Hook、快照、资源维护、统计和嵌入数据会话。声明列出的重载和上下文入口都在支持范围内。

| 边界 | 契约 |
| --- | --- |
| ID 与整数指纹 | Rust/Python 保持 `u64` 范围；Node 按各入口声明使用安全整数、BigInt 或十进制字符串；拒绝会丢精度的隐式转换 |
| `f16` | 转换后的值继续执行有限性与向量契约校验 |
| Hook | 同步回调；空替换值沿声明语义保留结果；非法成员与 Promise 返回明确报错 |
| 请求上下文 | 取消在准入、执行与完成点按内核规则传播；资源借用结束后归还 |
| 异常与 Error | `schema_version`、`code`、`message`、`details` 以及适用的 `target`、`stage`、`write` 保持结构化语义 |

---

## 查询与请求协议

TQL 的文档来源、命名图作用域、文本/向量/图算子共享 Planner 和执行器。Prepared 参数和 AST 范围按[定位契约](error-contracts.md)返回。应用依赖语义、结果、预算和来源定位；成本估计数值及内部物理算子拆分按实现与数据调整。

文档、命名图与 QuIVer JSON 请求的操作名、字段和响应由 `sdk` 适配层维护。Console 数据协议的版本、字符串 ID、权限、条件写入与回执沿现有协议维护；HTML 样式和界面结构沿前端发布流程维护。

[Pelorus](pelorus.md) 提供独立的双曲索引配置、驻留切换、单次和批量检索。Rust 暴露类型化入口，Python 与 Node 共用 JSON 协议。TQL 保留命名图作用域及同度量重排。

文档快照及片段标签统一使用结构 2。普通文档与解析文档共享这一输出结构，已有持久化结构 1 沿外部入口升级，正文、范围、节点身份、文档修订与回执保持迁移契约。

普通打开只接受[当前格式家族](storage-format.md)。历史身份返回 `TDB_MIGRATION_REQUIRED`，`details` 包含 `component`、`found`、`current`、`migration_entrypoint`。应用统一通过 [`scripts/migrate.py`](migration.md)显式发起迁移；当前格式的 WAL 恢复和文件组中断恢复继续由内核执行。

---

## 兼容承诺与验收

SDK 冻结覆盖调用签名、公开导入、参数默认值、数据类型、结果结构、错误类别和字段，以及稳定排序、预算、事务原子性、取消、只读物理零写、关闭与资源释放。内部改动由 SDK 适配，新增功能遵守原调用默认行为。

验收使用独立需求预期：Rust 公共契约、安装后的 Python wheel 与存根消费、安装后的 npm tarball 与 TypeScript 消费、TQL 执行及错误定位、快照与迁移往返。方法名称清单用于接口审查，实际制品与内容断言用于行为验收。验证入口见[测试实践](testing.md)。

> **发布提醒**：本文登记冻结范围。完成跨平台 CI、实际制品验收并发布对应 beta 后，才能以该发布版本作为冻结交付基线。

---

## 内核扩展与发布通道

`storage` 的文件布局、MemTable、WAL、内部加载器，`index` 的具体算法与缓存实现，`query` 的 AST/Planner/执行器内部结构，`database::inspection` 离线核验接口及绑定内部模块属于内核扩展范围。它们在 SDK 冻结阶段继续演进；上表登记的应用参数和结果类型按 SDK 契约维护。

内部模块、函数和变量按职责命名，Hook 阶段派发使用 `invoke_stage`，当前格式准入直接比较唯一的格式身份。磁盘头部及组件保留各自身份字段，用于来源识别、完整性校验和外部迁移定位；已公开的插件符号按 FFI 契约维护。

`1.0.0` 再登记并冻结 SDK 与存储内核之间的正式接口、所有权及持久化语义。私有实现、文件组织和缓存策略持续允许重构。C、Go、Wasm 按各自后续交付登记接口；Server HTTP 沿 nightly 通道验收。详细分层见[内核 SDK 与存储内核](kernel-sdk.md)。
