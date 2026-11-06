# 内核 SDK 与存储内核分层

> 本文定义嵌入式调用的职责、共享所有权、请求失败与关闭边界，适用于以 `0.9.0` 为基线的当前开发版本。实现入口见 [`src/sdk/`](../src/sdk/mod.rs)，变更记录见[未发布变更](changes/unreleased.md)。

---

## 目录

- [职责与依赖](#职责与依赖)
- [共享访问与事务提交](#共享访问与事务提交)
- [所有权与关闭](#所有权与关闭)
- [请求失败与错误字段](#请求失败与错误字段)
- [现有入口与后续收敛](#现有入口与后续收敛)

---

## 职责与依赖

| 层 | 职责 | 主要实现 |
| --- | --- | --- |
| 宿主与传输适配 | 认证、HTTP/IPC、请求队列、取消传递、应用调度 | Server、CLI、宿主程序 |
| 内核 SDK | 类型化入口、共享句柄、按请求借用、重入拒绝、语言与错误字段适配 | `src/sdk/`、`src/bindings/`、公共加载器 |
| 存储内核 | 事务验证与原子提交、查询执行、WAL、索引、文件租约、恢复、后台任务排空与 mmap 释放 | `src/database/`、`src/query/`、`src/storage/`、索引与资源模块 |

SDK 调用存储内核。存储内核保有数据一致性与生命周期的最终判定权；SDK 的请求准入检查之后，具体操作仍执行内核检查。共享错误类型位于 `src/error.rs`，JSON/JavaScript 整数编码位于 SDK 层。

写入阶段与结果由存储内核记录。直接 CRUD 和事务共享 WAL 后应用失败的恢复保护，TQL 构造前的解析、计划与预算拒绝由内核写入口标记。SDK 与传输层保留这些字段，资源运行时通过原始错误类别提取故障详情。

文件组发布提供 `publish` 阶段和提交标记完成状态，Core 公共入口将其附加到统一错误中。文本和派生索引的内存安装、唯一约束的立即发布、刷盘及压实各自遵守实际完成点。

独立快照、恢复、manifest 和迁移的最外层结果对应本次目标发布，内部源检查点与暂存库继续保留原始错误链。后台构建在发布前重新核对 WAL 和恢复保护。周期维护的规范错误字段通过 `ResourceFailure` 传递，SDK 复用这些内核判定。

普通 API 字段通过 `target.field` 定位，绑定按公开参数命名映射，原始错误类别及既有完成点由内核保留。图修复、容量预留和同步配置在修改前核对 WAL；资源额度调整按自身生命周期与预算处理。既有 QuIVer 构建句柄在后续捕获任务时读取当前内核额度。

`DatabaseReader`、`DatabaseWriter` 的实现位于 `sdk::facade`；`ConsoleHost` 位于 `sdk::console`。已有根路径与 `database::facade`、`database::console::ConsoleHost` 导入继续可用。Console 命令执行与条件事务校验继续通过内核现有入口完成。

---

## 共享访问与事务提交

`SharedDatabase<T>` 接收已有 `Database<T>`，也提供 `open` 和 `open_with_config`。克隆句柄共享一个内核实例。`try_read` 提供共享借用，`try_write` 提供独占借用；争用直接返回 `TDB_BUSY`。同线程再次进入同一共享实例返回 `TDB_REENTRANT_ACCESS`。

```rust
use triviumdb::{SharedDatabase, database::TxBuilder};
use serde_json::json;

fn example() -> triviumdb::Result<()> {
    let database = SharedDatabase::<f32>::open("memory.tdb", 2)?;
    let editor = database.clone();

    let mut transaction = TxBuilder::new();
    transaction.insert_with_id(1, &[1.0, 0.0], json!({"text":"正文"}));
    editor.commit_tx(&mut transaction)?;

    let node = database.try_read(|core| core.get(1))?;
    drop(editor);
    database.close()?;
    Ok(())
}
```

多个编辑者可各自持有 `TxBuilder`，在宿主锁外完成正文处理、向量计算和事务准备。`commit_tx(&mut builder)` 在准入成功后取走操作列表，事务验证与提交由存储内核执行。锁忙、重入、已关闭或恢复保护导致的 SDK 准入拒绝保留 builder；进入内核提交后的成功或错误均已消费本次 builder。

`try_read` 表达 Rust 共享借用。数据库的实际写权限由 `AccessMode` 和具体 API 决定；需要只读能力边界时使用 `DatabaseReader` 或 ReadOnly/Immutable 配置。

> **调度提醒**：闭包内只保留当前同步数据库工作。网络请求、向量计算、等待 IPC 和其他应用任务放在闭包外。宿主收到 `TDB_BUSY` 后按自己的有界队列或退避策略重新准入；SDK 本身采用立即返回的准入方式。

---

## 所有权与关闭

- 一个底层实例持有文件租约、WAL、MemTable 和生命周期状态。重复打开同一路径时，文件锁冲突返回 `TDB_DATABASE_LOCKED`，原实例继续服务。
- 释放单个 `SharedDatabase` 克隆只释放该句柄的所有权。`downgrade` 返回 `WeakDatabase`，供 Hook 与组件避免所有权环；`upgrade` 在底层实例仍被持有时取得共享句柄。
- `ConsoleHost::attach_shared` 使用弱句柄接入同一个 SDK 实例。已有 `attach(&Arc<Mutex<Database<T>>>, options)` 继续支持外部宿主锁。
- `close`、`request_shutdown` 和 `close_timeout` 作用于底层实例及所有共享句柄。SDK 锁忙时关闭请求尚未进入内核；准入成功后按内核关闭协议执行。
- `close_timeout` 返回 `false` 表示等待结束。关闭任务继续持有资源与租约；通过 `shutdown_status` 观察阶段，按结果继续等待或重试。
- 显式关闭完成后，可继续持有 SDK、会话和 Router 对象；存储内核释放相应映射与文件资源。最后一个强句柄的普通析构沿用内核析构规则。

> **持久化提醒**：使用显式关闭完成点确认排空和资源释放。关闭失败的处理按[生命周期与快照](lifecycle-snapshots.md)执行；已确认写入、未知写入结果与关闭等待超时分别处理。

---

## 请求失败与错误字段

`TriviumError::info()` 提供 `schema_version: 1`、`code`、`message`、`details`。Python 内核异常和 Node 原生错误提供对应属性；CLI 的 JSON 模式将内核错误写入 stderr，退出码为 1。Rust/Python 保留整数精度，Node 与 HTTP 将超出 JavaScript 安全整数域的错误字段编码为十进制字符串。

| 错误码 | 详情 | 处理范围与建议 |
| --- | --- | --- |
| `TDB_DATABASE_LOCKED` | `reason` | 本次打开被拒绝；复用现有实例或等待文件租约释放 |
| `TDB_BUSY` | `resource` | 本次请求未进入对应资源；持有者释放后重新准入 |
| `TDB_REENTRANT_ACCESS` | `operation` | 当前调用链重复进入实例；调整回调或提交顺序 |
| `TDB_HOST_LOCK_POISONED` | `resource` | 宿主持锁闭包发生异常展开；结束相关句柄并检查重新打开的恢复结果 |
| `TDB_DATABASE_CLOSED` | 空对象 | 实例正在关闭或已经关闭；通过生命周期入口处理 |
| `TDB_TRANSACTION_FINISHED` | 空对象 | 事务构造器已消费或回滚；创建新的事务对象 |
| `TDB_WAL_CLOSED` | 空对象 | WAL 或恢复保护拒绝继续操作；检查实例与持久化状态 |

内核 Hook 的返回错误按实例识别。检索 Hook 内再次调用同库的受准入管理操作返回 `TDB_REENTRANT_ACCESS`，其他数据库按自己的资源和生命周期检查执行。分组提交闭包内关闭本库也返回该错误，原同步组保持有效。

Python 数据库实例方法在取得 PyO3 借用前登记请求身份。同一调用链及批量查询工作线程上的 Hook 访问同库时，返回 `TDB_REENTRANT_ACCESS`，详情为 `operation: "python_callback"`。其他线程的借用争用返回 `TDB_BUSY`，详情为 `resource: "python_database"`；共享读借用允许其他线程继续读取。上述错误均保留四个基础字段。

Python `Transaction.commit()` 先取得数据库借用并完成内核准入，再消费 builder。锁忙、重入、实例关闭或恢复保护拒绝时，`pending_count()` 保留原操作数；可按错误类别安排后续提交或显式 `rollback()`。已经关闭的实例拒绝创建新的事务对象。进入内核提交后的成功或失败沿用事务消费规则。

事务错误的可选 `write` 字段提供 `phase`、`outcome` 与适用的 `transaction_consumed`。SDK 准入拒绝返回 `admission/not_committed/false`，Core 预检拒绝返回 `validation/not_committed/true`，WAL 或应用失败返回对应阶段与 `unknown/true`。Rust 的上下文错误通过 `underlying()` 保留原始变体。分组同步及响应阶段的定义与重试条件见[公共错误契约](error-contracts.md#写入结果字段)。

Python `Transaction` 的缓冲、提交、回滚、状态读取和上下文管理入口也采用显式借用。方法内的向量转换回调再次访问同一事务时返回 `TDB_REENTRANT_ACCESS`，详情为 `operation: "python_transaction_callback"`；跨线程借用争用返回 `TDB_BUSY`，详情为 `resource: "python_transaction"`。事务准备独立持有自己的操作集，其他事务和数据库按各自准入状态继续服务。

事务成功提交、内核提交失败或回滚后进入结束状态，再次缓冲、提交、回滚或进入上下文返回 `TDB_TRANSACTION_FINISHED`。`pending_count()` 在结束后返回 0；已结束事务的上下文退出保留原异常语义。方法内的宿主转换异常继续作为原异常传播，已缓冲操作保留。事务向 Python GC 暴露所属数据库引用，使 `Database → Hook → Transaction → Database` 环可被回收；调用方仍持有事务时，数据库所有权继续有效。

TQL 解析错误保留 `TDB_QUERY_PARSE` 与 `details.reason`，增加 `stage: "lex" | "parse"`。可定位错误通过 `target.query_span` 提供原始 UTF-8 字节锚点和一基 Unicode 标量行列；缺少准确位置时省略 `target`。Prepared 缺参、多参、命名 JSON 参数转换及向量槽位类型错误提供 `stage: "bind"` 和 `target.parameter`，保留原错误类别、详情和异常继承关系。JSON／Node 省略未提供的扩展字段，Python 内核异常将其设为 `None`。各端直接传递内核提供的位置，详见[公共错误契约](error-contracts.md)。

nightly Server 的内核错误使用同一 `code` 和 `details`，保留 RFC Problem Details 的 `detail`，并增加同义 `message` 和 `schema_version`。原 HTTP 分类名通过 `legacyCode` 提供迁移信息，例如 `QUERY_PARSE_ERROR` 对应 `TDB_QUERY_PARSE`。Server 自身的请求超时、传输和队列错误沿用当前 HTTP 契约。

独立 Server 开始关闭后，新读写请求返回 HTTP 503、`TDB_DATABASE_CLOSED`、空 `details`、`legacyCode: "SERVICE_UNAVAILABLE"` 和 `retryable: false`。执行线程故障继续使用服务不可用类别，由对应故障状态决定恢复方式。

> **故障提醒**：普通争用和输入拒绝只结束当前请求。持久化不确定、损坏、宿主异常展开等情况按实际一致性状态处理。错误码本身不足以判断任意事务是否提交；编辑确认继续使用既有回执协议。

---

## 现有入口与后续收敛

Rust 共享宿主优先使用 `SharedDatabase`。直接持有 `Database` 的调用者继续负责外层借用与调度。Python 和 Node 保留当前对象及事务构造接口；Python 数据库方法采用显式借用准入，Node 公共加载器在同步回调重入时先返回结构化拒绝。Server 继续使用既有有界 Writer 队列和分组合批。

Python 的容量、额度、压实间隔和自动构建开关在取得请求借用后提取参数，Prepared 的 `Mapping` 也在请求范围内收集。自定义转换回调再次访问同一数据库时按重入契约拒绝，原请求继续执行或传播宿主原异常。Node 管理标量按原始类型校验，保留公开声明中的数值与布尔类型。

同步模式和 generation 标识的字符串提取复用同一管理边界。Node Prepared 在方法内校验原始容器与标量，属性访问处的宿主异常直接传播，已知表示转换失败附加参数名。公共加载器持续持有同步请求身份，转换回调结束后归还；后续有效请求按正常准入继续执行。

已覆盖的 Python 检索整数、TQL 字符串和 Filter Mapping 在请求借用后转换。通用参数适配按读取或写入上下文附加字段和可选写入结果，宿主协议异常沿原路径传播。Node 检索整数与查询字符串复用显式类型校验，实际异步批量结果在任务完成后交付。Filter 语义由 Core 的 `Filter::from_json` 统一判定。

精确、图优先、上下文、高级及混合检索的已覆盖整数参数复用上述边界。Python 锚点序列及元素协议也在读请求范围内执行，回调中的同库写请求由 SDK 拒绝。Node 配置 getter 的原异常继续直接传播，数量字段在语言表示转换后进入既有安全整数检查。向量及序列成员的统一路径诊断按公共契约继续推进。

Core 查询控制分别提供主动取消和期限到期类别，SDK 与传输直接保留规范码。Console 分发前及事务／导入的 WAL 前检查点按已知执行阶段报告未提交结果，准备过程中的工作区随请求退出归还。等待结束后的后续提交状态继续依据 Core 结果及回执判断；完整宿主取消原因与通用请求上下文按公共契约继续收敛。

TODO：逐项统一绑定参数提取、其他包装对象的借用错误、完整 AST 与 Prepared 参数引用的源码范围、其余管理入口、外部传输和宿主取消等阶段。已交付数据库与 Python 事务的借用和结束类别、TQL／Prepared 诊断、事务与直接写入、文本及属性索引维护、自动构建与运行期模式、快照及迁移目标、刷盘及压实、SDK 事务消费状态和 Server 队列／响应等待的结果字段。函数实参在绑定框架中的自动提取及普通类型转换错误继续按公共契约收敛。

TODO：根据真实文档编辑负载评估更细粒度的冲突检测和写锁范围，继续复用内核事务与文件组原子发布协议。
