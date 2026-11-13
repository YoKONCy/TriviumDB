# 内核 SDK 与存储内核分层

> 本文定义嵌入式调用的职责、共享所有权、请求失败与关闭边界，适用于以 `0.9.0` 为基线的当前开发版本。实现入口见 [`src/sdk/`](../src/sdk/mod.rs)，变更记录见[未发布变更](changes/unreleased.md)。

---

## 目录

- [职责与依赖](#职责与依赖)
- [当前依赖记录](#当前依赖记录)
- [共享访问与事务提交](#共享访问与事务提交)
- [统一请求上下文](#统一请求上下文)
- [类型化绑定与包装对象](#类型化绑定与包装对象)
- [所有权与关闭](#所有权与关闭)
- [请求失败与错误字段](#请求失败与错误字段)
- [现有入口与后续收敛](#现有入口与后续收敛)

---

## 职责与依赖

QuIVer 冷向量编码位于 Core 的 `index::quiver::cold`，维护版本化行布局、浮点量化、完整字节校验及标量评分。`index::quiver::ssd` 维护 SM 导航、INT8 行租约、LRU/Pending 缓存及驻留转换，`storage::quiver_cold` 与 `storage::quiver_io` 管理流式发布、来源绑定、代际句柄、异步 Direct I/O、排空和暂存回收。`database::quiver_ssd` 计算数据库来源身份，并在 checkpoint 后刷新启用的派生代际。SDK 的 `quiver` 模块转换 JSON 与字符串 ID，Python／Node 复用请求借用和结构化错误。数据库关闭复用现有操作排空和存储释放路径，Reader 缓冲及 I/O 线程分别在内核与共享 Runtime 中准入。当前接口及使用条件见 [QuIVer SSD Reader](quiver.md#ssd-索引制品与-reader)。

命名图的身份、成员、独立边、修订比较、节点删除级联和作用域由Core及存储层维护。`sdk::named_graphs`负责共享JSON协议，Python／Node沿现有请求借用和错误适配接入。Reader暴露图读取、列举、显式联合及遍历，Writer调用Core事务入口。持久事实位于主文件版本化图块和WAL，详细接口见[命名图](named-graphs.md)。

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

公开调用形状由语言适配层检查，`sdk::errors` 统一构造字段、`details.argument_kind` 及已知的写入前结果。Python 的方法与模块函数复用原生签名，构造器由 SDK 签名元数据驱动实参绑定及 __signature__；Node 加载器登记必需位置参数，继续复用已有回调重入准入。有效调用进入原生绑定后沿用存储内核语义。完整规则见[公开调用形状](error-contracts.md#公开调用形状)。

构造参数的已覆盖整数、字符串、枚举和 MiB 换算复用语言适配的字段诊断，Node 同时覆盖数据库布尔配置。维度与资源配置的语义约束由内核判定，绑定映射公开字段和单位。打开失败后的任务、配额及文件资源按内核所有权释放。Python 三个构造器复用公开签名完成实参绑定，Node 数据库构造转发保留原生对象和子类身份，具体见[构造参数契约](error-contracts.md#构造参数值)。

Node 外层配置在已知属性读取前检查对象形状，拒绝数组、函数和原始值。Python 布尔提取复用既有 PyO3 语义，两端 runtime 适配复用各自原生句柄校验，并同时服务数据库与快照模块函数。已知拒绝提供字段，宿主 getter／Proxy／转换异常保留原对象；恢复模块的输入拒绝按执行前事实提供写入结果。

十五个普通 CRUD 的语言值与 JSON 转换在 SDK／绑定层完成，成员路径由 sdk::errors 的 JSON Pointer 编码辅助构造。已知转换拒绝按执行前事实提供 validation/not_committed；事务校验、WAL、应用、恢复保护与文件资源释放继续由存储内核实现。Python 的整数与浮点协议在请求借用内执行，Node 的属性读取复用公开加载器的重入保护。

两类批量插入复用内核事务执行原子提交，临时 builder 由绑定持有。SDK 从公开批量错误中省略 transaction_consumed，保留内核的原始类别、阶段及提交结果。显式事务接口继续提供其消费状态。

检索、图与文本入口复用带读／写上下文的标量、序列和 JSON 转换。SDK 换算混合权重并拒绝非有限输入及溢出，绑定保留原宿主异常、Python 请求借用与 Node Promise 完成方式。检索配置、扩散偏置及查找范围由存储内核验证并提供字段，空批次复用内核配置校验。文本重建在候选提交前完成记录映射与成员校验，后续预算、索引安装及持久化沿内核维护入口执行。

`DatabaseReader`、`DatabaseWriter` 的实现位于 `sdk::facade`；`ConsoleHost` 位于 `sdk::console`。已有根路径与 `database::facade`、`database::console::ConsoleHost` 导入继续可用。Console 命令执行与条件事务校验继续通过内核现有入口完成。

---

## 当前依赖记录

| 调用方 | 当前依赖 | 职责与保留条件 |
| --- | --- | --- |
| sdk::shared | Core Database、Config、TxBuilder、关闭状态及 Core RequestContext | 持有 Arc／Weak、借用、重入与缓冲消费边界，提交和关闭继续调用 Core |
| sdk::console | Core ConsoleSession、借用型请求准备、QueryControl 和 Database 执行入口 | 弱引用接入宿主、取得请求借用、传递身份及控制，命令、权限、编辑版本和回执由 Core 决定 |
| sdk::facade | Core Database 的方法及查询／图／存储结果类型 | Reader 提供受限能力，Writer 复用完整写入口，GraphStats、GenerationManifest 属于返回数据类型 |
| sdk::errors | Core 错误及写入结果类型、serde_json | 编码语言调用形状、成员路径与 JSON 安全整数，保留 Core cause 和提交事实 |
| Python／Node 原生绑定及公共加载器 | SDK 请求借用、错误辅助、Core Database／查询／Hook／快照入口 | 管理语言对象、转换、回调、线程及异常，快照模块函数仍直接复用 storage::snapshot 的公共操作 |
| CLI | Core 及现有 dtype 擦除门面 | 解析命令、文件输入输出及诊断，存储和 TQL 语义由 Core 提供 |
| Server 引擎及嵌入 HTTP | Core Database、ConsoleHost、QueryControl 与结构化错误 | 引擎维护自己的有界队列、组尾同步和实际任务排空，HTTP 负责外层身份及协议编码 |
| database::facade、database::console::ConsoleHost | SDK 对应类型的兼容重导出 | 现有公共导入路径继续解析到同一实现 |
| database、query、storage、index、graph、resources、request 的执行逻辑 | Core 模块之间的类型和方法 | 锁顺序、查询控制、WAL、恢复、持久回执、任务与 mmap 释放由 Core 管理 |

当前 Core 执行模块指向 SDK 的引用为上表两处兼容重导出。查询控制定义于 Core，SDK 再导出 RequestContext。语言绑定和 Server 的直接 Core 调用按表中职责保留，后续分层调整以实际纵向路径推进，相关返回类型和兼容导出同步维护。

精确扫描在进入 Rayon 池前捕获控制器，图算法在实际工作线程安装组合上下文，内部并行闭包共享控制信号。取消经可失败执行路径返回，工作区和任务额度按 Core 所有权归还。SDK 继续管理请求借用与语言对象生命周期。

---

## 共享访问与事务提交

文档来源的公共类型位于 `document`，Core 的文档读取、修订检查、结构边及原子更新位于 `database::documents`。`sdk::documents` 适配跨语言 JSON、字符串 ID 和响应预算。Python／Node 的文档请求调用同一适配器，Rust Reader 直接委托 Core 读取方法。字段、所有权和当前接入范围见[文档来源与结构](documents.md)。

解析与来源映射位于 `document::parsing` 及文档公共类型，`database::document_ingestion` 管理候选接入和最终发布。`storage::document_publications` 维护版本化批次事实，复用 Core 的事务、WAL、预算及文件组恢复。SDK 转换命令和语言表示，事实校验、摘要、CAS、幂等序号及回收由 Core 执行。Reader 暴露解析、状态与列表，Writer 调用相同内核写入口。PDF／Office／OCR 由宿主生成统一解析结果。协议见[文档解析与分批发布](document-ingestion.md)。

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

## 统一请求上下文

`sdk::RequestContext` 由 Core 请求控制层定义，保存关联 ID、共享 QueryControl 与写入意图。`request_id()` 返回原 ID，`control()` 供实际查询和事务传播取消及期限。克隆共享同一个取消信号，准入结果对应本次 SDK 调用。事务原子性通过 TxBuilder 和 Core 提交入口提供。

`SharedDatabase::try_read_with_context` 和 `try_write_with_context` 在借用前及取得实例后检查上下文，继续使用立即拒绝的共享锁和重入规则。业务闭包返回的内核错误保留原始阶段与结果。`commit_tx_with_context` 在上下文和内核准入后消费 builder，SDK 准入拒绝保留缓冲并返回 `transaction_consumed=false`，进入 Core 后按实际消费状态返回。事务预检完成、WAL 尚未追加时再次观察取消，拒绝提供 `validation/not_committed`。

```rust
use triviumdb::sdk::RequestContext;
use triviumdb::query::tql_executor::QueryControl;
use triviumdb::database::TxBuilder;

let context = RequestContext::new("edit-42", QueryControl::default(), true)?;
let mut transaction = TxBuilder::new();
transaction.insert(&[1.0_f32, 0.0], serde_json::json!({"text":"正文"}));
let created_ids = shared.commit_tx_with_context(&context, &mut transaction)?;
```

Console 在 Core 解析请求信封和命令种类，派生同一上下文。SDK 借用前保留借用型原始命令，业务参数在实例准入后物化。会话在业务解码前登记活动请求，取消、期限和排空使用同一身份与控制器。Python、Node 与 HTTP 的现有 Console 协议复用该路径。实际写入的取消传播到 WAL 前检查点，WAL 开始后的事务按原子协议完成。

持久回执由存储内核写入同一 WAL 事务并随检查点发布，SDK 传递回执及错误字段。跨重开确认、容量与文件版本见[嵌入式 Console](embedded-console.md#回执与结果确认)。

---

## 类型化绑定与包装对象

Rust 的 `RequestContext::run` 建立当前线程的同步调用作用域。`DatabaseReader::with_request_context` 和 `DatabaseWriter::with_request_context` 向闭包提供原门面，方法调用继续经过 Core 的准入、权限与提交检查。SDK 借用 Guard 在创建线程退出，请求控制句柄可克隆传递到工作线程。

```rust
let context = triviumdb::sdk::RequestContext::with_timeout(
    "document-edit", std::time::Duration::from_secs(30), true,
)?;
writer.with_request_context(&context, |writer| {
    writer.update_payload(7, serde_json::json!({"body":"正文"}))
})?;
```

Python 和 Node 提供 `RequestContext(request_id, timeout_ms=0)`／`new RequestContext(requestId, timeoutMs=0)`。ID 使用 1–128 个 UTF-8 字节，期限从构造时开始计算，0 由宿主通过 `cancel()` 管理。`check()` 读取该对象自身的控制器状态，取消状态持续有效。两端用户作用域最多嵌套 32 层，数据库调用继承组合后的外层取消及期限约束。

```python
from triviumdb import RequestContext

context = RequestContext("document-edit", timeout_ms=30_000)
with context:
    rows = db.tql("MATCH (n) RETURN n LIMIT 10")
    transaction.commit()
```

```javascript
const { RequestContext } = require('triviumdb');
const context = new RequestContext('document-query', 30_000);
const hits = await context.run(async () => {
  await prepareInput();
  return db.searchBatch(vectors);
});
```

Python 使用 ContextVar，Node 使用 AsyncLocalStorage 保存任务局部状态。实际原生调用期间进入 Core 作用域，异常退出时归还。批量检索显式携带同一控制器到原生工作线程，整个批次按最终结果交付。事务提交在借用与准入成功后消费缓冲，准备期间取消保留已知阶段及消费状态。响应返回后沿用已经确定的内核结果。

关闭、关闭状态、事务回滚及已登记的资源诊断使用各自的借用和生命周期检查，可在取消后清理资源。Prepared 元数据、独立节点／边结果和运行时诊断按各对象所有权访问。请求上下文保存身份与控制状态，文件租约、后台任务和映射继续由内核持有。

一个作用域可包含多个独立调用。原子提交通过 TxBuilder、Python Transaction 或 Node commitTransaction 完成，版本保护及持久确认通过编辑协议提供。控制器在 Core 准入、TQL 执行检查点、检索 Hook 边界、批量工作线程及已接入的 WAL 前检查点观察取消。文件组发布按自身的原子流程完成。

> **宿主提醒**　Rust `run` 用于同步调用，Node JavaScript Hook 在注册线程同步执行。宿主自行创建线程、进程或外部任务时，显式传递控制句柄，或通过 IPC 传递请求身份及取消消息。各宿主负责外部任务的调度与完成确认。

---

## 所有权与关闭

- 一个底层实例持有文件租约、WAL、MemTable 和生命周期状态。重复打开同一路径时，文件锁冲突返回 `TDB_DATABASE_LOCKED`，原实例继续服务。
- 释放单个 `SharedDatabase` 克隆只释放该句柄的所有权。`downgrade` 返回 `WeakDatabase`，供 Hook 与组件避免所有权环；`upgrade` 在底层实例仍被持有时取得共享句柄。
- `ConsoleHost::attach_shared` 使用弱句柄接入同一个 SDK 实例。已有 `attach(&Arc<Mutex<Database<T>>>, options)` 继续支持外部宿主锁。
- `ConsoleSession::detach()` 关闭会话准入并发送取消信号。`is_drained()` 在所有 Core 请求实际退出后返回 `true`，绑定与 HTTP 复用该判定。传输任务和响应消费分别由其宿主确认完成。
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

TQL 解析错误保留 `TDB_QUERY_PARSE` 与 `details.reason`，增加 `stage: "lex" | "parse"`。可定位错误通过 `target.query_span` 提供原始 UTF-8 半开字节范围和一基 Unicode 标量行列。缺少准确位置时省略 `target`。Prepared 缺参、多参、命名 JSON 参数转换及向量槽位类型错误提供 `stage: "bind"` 和 `target.parameter`，数据库准备的已引用参数同时提供首个真实引用范围。Core 保存源码并附加位置，SDK 与绑定复用转换，传输层编码既有事实。原错误类别、详情、cause、异常继承与写入结果沿原链保留。JSON／Node 省略未提供的扩展字段，Python 内核异常将其设为 `None`。详见[公共错误契约](error-contracts.md)。

AST 构造来源、符号检查、算子装配、写计划与执行来源由 `src/query/tql_source.rs` 和查询内核维护，诊断补齐 `plan` 与 `execute`。配对解析结果提供只读 AST 与来源视图，Prepared 保留该来源。请求上下文独立于公开预算配置与取消句柄，内核生成的子查询沿原入口追溯。算子、预算、投影和事务在实际检查点附加来源，融合路径保留参与节点的包围范围。SDK 与绑定负责传递阶段和目标字段，Node 字段命名适配同时保留查询范围。

nightly Server 的内核错误使用同一 `code` 和 `details`，保留 RFC Problem Details 的 `detail`，并增加同义 `message` 和 `schema_version`。原 HTTP 分类名通过 `legacyCode` 提供迁移信息，例如 `QUERY_PARSE_ERROR` 对应 `TDB_QUERY_PARSE`。Server 自身的请求超时、传输和队列错误沿用当前 HTTP 契约。

独立 Server 开始关闭后，新读写请求返回 HTTP 503、`TDB_DATABASE_CLOSED`、空 `details`、`legacyCode: "SERVICE_UNAVAILABLE"` 和 `retryable: false`。执行线程故障继续使用服务不可用类别，由对应故障状态决定恢复方式。

> **故障提醒**：普通争用和输入拒绝只结束当前请求。持久化不确定、损坏、宿主异常展开等情况按实际一致性状态处理。错误码本身不足以判断任意事务是否提交；编辑确认继续使用既有回执协议。

---

## 现有入口与后续收敛

Rust 共享宿主优先使用 `SharedDatabase`。直接持有 `Database` 的调用者继续负责外层借用与调度。Python 和 Node 保留当前对象及事务构造接口；Python 数据库方法采用显式借用准入，Node 公共加载器在同步回调重入时先返回结构化拒绝。Server 继续使用既有有界 Writer 队列和分组合批。

Python 的容量、额度、压实间隔和自动构建开关在取得请求借用后提取参数，Prepared 的 `Mapping` 也在请求范围内收集。自定义转换回调再次访问同一数据库时按重入契约拒绝，原请求继续执行或传播宿主原异常。Node 管理标量按原始类型校验，保留公开声明中的数值与布尔类型。

同步模式和 generation 标识的字符串提取复用同一管理边界。Node Prepared 在方法内校验原始容器与标量，属性访问处的宿主异常直接传播，已知表示转换失败附加参数名。公共加载器持续持有同步请求身份，转换回调结束后归还；后续有效请求按正常准入继续执行。

已覆盖的 Python 检索整数、TQL 字符串和 Filter Mapping 在请求借用后转换。通用参数适配按读取或写入上下文附加字段和可选写入结果，宿主协议异常沿原路径传播。Node 检索整数与查询字符串复用显式类型校验，实际异步批量结果在任务完成后交付。Filter 语义由 Core 的 `Filter::from_json` 统一判定。

精确、图优先、上下文、高级及混合检索的已覆盖整数参数复用上述边界。Python 锚点序列及元素协议也在读请求范围内执行，回调中的同库写请求由 SDK 拒绝。Node 配置 getter 的原异常继续直接传播，数量字段在语言表示转换后进入既有安全整数检查。向量及序列成员的路径诊断按公共契约维护。

邻居、可达性、详细可达性和子图入口补齐 ID、深度／预算及方向诊断。Python 标签序列也在读请求借用内转换，长度及取值回调共享请求身份。十五个直接 CRUD 入口的 SDK 准入拒绝提供 `admission/not_committed`，已进入内核后的完成点继续由内核报告。Node 公共加载器在原生借用之前提供同一拒绝结果。

节点、Payload、存在性、出边、入边和精确边读取也在适配层保留字段诊断。Python 的整数协议在读请求借用内执行，受控跨线程读取可共享只读借用，竞争写入返回 Busy；同一调用链重入按 SDK 身份拒绝。返回的节点、向量、Payload 和边继续使用独立结果所有权，关闭完成后仍可读取已返回内容。

Core 查询控制分别提供主动取消和期限到期类别，SDK 与传输直接保留规范码。Console 已解析写命令的会话、能力及权限拒绝提供 `admission/not_committed`，事务／导入在 WAL 前检查点拒绝时提供 `validation/not_committed`。准备过程中的工作区随请求退出归还。

Core 会话通过 `is_drained()` 提供 detach 后的执行完成确认。Server 在取得实际写锁后逐条核对接收端与期限，Console 数据适配器在桥接分发后的写入响应丢失时提供 `response/unknown`。客户端卸载等待自身队列、detach 回调和已经发出的取消调用，原事务确认继续使用完整请求。SDK 与 Console 复用 Core 请求上下文，有界回执支持恢复及跨重开确认。各层完成点见[公共错误契约](error-contracts.md#请求与传输完成点)。

类型化绑定通过上述作用域接入 Core，Rust Reader／Writer 和动态语言事务沿原所有权执行。自定义桥接传递不可变请求元数据，并可提供显式 drain 完成确认，详见[桥接协议](embedded-console.md#宿主自定义桥接)。

请求体解析前的外层身份、完整重试目录和当前字段清单已登记于公共契约。宿主专用调度通过显式上下文传递及 drain 接入。

当前管理入口、事务缓冲、关闭与目标发布的语言转换使用同一字段规则。精确扫描、图算法、遍历和结构分析在可失败路径观察取消。数据库与事务借用、TQL 来源、WAL、发布和恢复继续沿表中职责执行，整批交付按最终源码及平台补齐验收记录。

后续按真实文档编辑负载评估冲突检测和写锁范围，相关模型变更沿长期规划推进。

文档 `search`、`context` 和 `clear_index` 复用共享 JSON 适配。SDK 解码和编码请求，Core 在同一事实读视图中核对修订、构建派生语料、分组评分及展开证据。可重建文档缓存由内核维护并计入索引内存估算，文档写入沿既有事务使缓存失效。Rust Reader 委托相同读取实现，Python／Node 保留各自请求借用和错误映射。字段与默认值见[文档检索契约](documents.md#分组文本检索)。

分析器、原文字节映射、位置posting、短语与有序邻近条件均由 Core 维护。SDK 传递 `match_mode`、`max_gap`、`include_matches` 和分析器身份，三端使用相同候选过滤、最短窗口及预算。精确命中的修订引用直接衔接原文解析和上下文入口。

字段语料统计与权重、文档／片段元数据作用域、成员倒排交并及完整事实谓词复核由 Core 执行。SDK 将 `fields`、`document_filter`、`block_filter` 与 `members` 转换到同一类型化请求，沿用内核的参数定位、预算、取消及修订错误。字段语料和成员倒排复用可重建文档缓存，事实仍由现有节点、Payload 和 WAL 管理。

文档 `search_vector` 与 `search_hybrid` 由 Core 在同一事实读视图中执行原生向量评分、QuIVer 候选扩展和既有 RRF 融合。Core 负责统一候选约束、组覆盖目标、近似范围、停止原因和工作预算。SDK 校验 JSON 数值表示，保留 f16 有限范围与完整 u64 位模式，传递原始请求及结构化结果。Python／Node 继续复用现有字符串请求与借用完成点。QuIVer 的查询控制检查点在请求侧保留取消或额度失败，内核返回完整错误并归还工作区。
