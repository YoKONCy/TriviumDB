# 未发布变更

后续变更以 `0.9.0` 正式版为基线。已发布内容见[完整中英文变更说明](0.9.0.md)。

### 内核 SDK 分层、共享访问与请求拒绝

- 新增 `sdk` 模块与 `SharedDatabase`、`WeakDatabase`。多个调用者共用一个存储内核；事务可在锁外通过 `TxBuilder` 准备，SDK 准入成功后由内核原子提交。锁忙、重入或关闭状态拒绝时保留尚未提交的 builder。
- `DatabaseReader`、`DatabaseWriter` 和 `ConsoleHost` 的宿主逻辑归入 SDK；已有导入路径保持可用。`ConsoleHost::attach_shared` 通过弱引用接入共享实例。丢弃单个句柄只释放所有权，显式关闭作用于所有共享句柄；WAL、文件租约、恢复和 mmap 释放仍由存储内核负责。
- 新增 `TDB_BUSY`、`TDB_REENTRANT_ACCESS`、`TDB_HOST_LOCK_POISONED`，分别表达准入繁忙、重复进入同库和宿主异常展开后的锁中毒。检索 Hook 的重入检查按实例执行，同库受准入管理的读写和构建请求被拒绝，其他数据库保持各自准入。分组提交闭包内的关闭请求返回重入错误；Node 公共加载器在同步回调重入时于 N-API 前拒绝请求。
- `TriviumError::info()`、Python 和 Node 内核错误采用 `schema_version: 1`、`code`、`message`、`details` 基础字段。CLI JSON 模式将内核错误结构写入 stderr。nightly HTTP 使用 Core 错误码和完整详情，旧分类名放入 `legacyCode`；例如 `QUERY_PARSE_ERROR` 迁移为 `TDB_QUERY_PARSE`。Node/HTTP 错误详情中的大整数采用十进制字符串。
- Console 锁忙和 HTTP 并发额度耗尽由 `TDB_IO/WouldBlock` 改为 `TDB_BUSY`，`details.resource` 指定 `host_database`、`shared_database`、`console_request_id` 或 `console_in_flight`。Node 同步回调重入改为 `TDB_REENTRANT_ACCESS`。上层按结构化字段更新分支处理。
- Python 存根生成器补齐 CAS、原子批删、唯一索引和 n-gram 查询等已有方法的返回类型，以及子串和属性条件参数的类型映射；交付存根随当前 wheel 重新生成。
- Node 公共错误联合类型补齐编辑版本冲突、幂等键冲突和写入结果未知，调用方可按错误码取得对应详情类型。
- Python 数据库实例方法改用显式、可失败的 PyO3 借用：同库回调重入返回 `TDB_REENTRANT_ACCESS`，跨线程争用返回 `TDB_BUSY`。批量查询的工作线程保留回调所属实例身份，共享读借用继续允许并发读取。
- 修复 Python 事务提交在数据库借用前消费 builder、借用冲突触发 panic 的路径。提交先取得借用并完成内核准入，锁忙、重入、关闭或恢复保护拒绝保留操作，调用方可继续安排提交或回滚；已关闭实例拒绝创建事务对象。
- 独立 Server 开始关闭后，新读写请求统一返回 `TDB_DATABASE_CLOSED` 和空详情，HTTP 保持 503，旧分类通过 `legacyCode: "SERVICE_UNAVAILABLE"` 保留，`retryable` 为 `false`。

> **适用范围**：本轮保持磁盘格式与单写事务模型。绑定参数提取、其他包装对象的借用异常、完整字段定位和写入结果元数据继续按分阶段契约收敛。Python 事务对象的进展见下一节。宿主锁中毒需要结束相关句柄并检查恢复；关闭超时只结束等待。接入示例见[内核 SDK 文档](../kernel-sdk.md)。

### Prepared 参数诊断与公共错误目录

- Prepared 缺参、多参、命名 JSON 参数转换、向量槽位类型与 Rust 非有限参数错误增加 `stage=bind` 和 `target.parameter`。参数名按原始键返回，缺参与多参继续按名称稳定选择。
- 转换与绑定诊断保留原始错误码、详情和 cause。Rust 新增带参数身份的 `QueryParameter` 错误变体，可通过 `underlying()` 匹配原始变体；`ErrorTarget` 按查询位置与参数名提供不同表示。Python 保留原异常继承关系，Node、独立 Server、嵌入 HTTP 与 Console 数据传输保留相同诊断字段。
- Python 存根、Node 公共类型与类型消费同步扩展 `bind` 和参数定位。共享样例覆盖非法数组／对象、向量参数类型、排序后的缺参／多参及特殊参数名，并核对拒绝后正常查询、WAL 字节和重开完整事实。
- 新增公共错误目录，记录打开、共享访问、事务、Prepared 与关闭的校验层、字段和完成边界；分阶段列明 WAL、应用、分组同步及响应交付的事实来源。通用写入结果和其他 API 字段定位继续分批交付。

### 事务写入结果与失败请求回传

- 事务提交错误增加可选 `write`，包含 `phase`、`outcome` 和适用的 `transaction_consumed`。Core 区分准入、预检、WAL、应用与响应阶段，保留原错误码、详情和 cause。Rust 新增 `WriteFailure` 上下文变体，模式匹配通过 `underlying()` 取得原始类别。
- SDK 在消费事务前核对生命周期、写权限及 WAL 可写性。共享 Rust builder 与 Python 活动事务在准入拒绝后保留操作，Core 预检或后续失败按已消费状态返回。Node 的已知事务构造校验与回调重入提供相应状态，原参数数组由调用方持有。
- WAL 追加与应用失败返回 `unknown`。分组提交在组尾同步失败时返回 `group_sync/unknown`，Server 将结果送回本组已经执行的等待请求，保留独立校验或执行错误；分组准入拒绝明确送回待执行请求。
- Server 的队列满和执行前过期返回 `not_committed`，入队后等待响应超时或响应通道丢失返回 `unknown`。带写入上下文的 HTTP 错误使用 `retryable=false`，上层按阶段、消费状态和对账结果安排重试。
- Console 已提交事务重放时的响应预算失败保留 `details.committed=true`，提供 `response/committed`。原事务继续通过回执确认。Python 存根、Node 类型、CLI JSON、HTTP 与 Console 数据适配同步字段。受控故障覆盖 Rom／Mmap 的 WAL、应用及恢复，并验证 Server 分组同步与后台任务异常。

### 直接写入与 TQL 提交前边界

- 节点及边的直接 CRUD、Payload 补丁与 CAS 提供写入阶段与结果。准入和校验失败为 `not_committed`，WAL 或应用失败为 `unknown`，直接入口省略事务缓冲消费字段。原始错误码、详情、异常继承关系及单条 WAL 编码继续保留。
- 直接写入在 WAL 追加后的应用阶段使用统一恢复保护。失败后当前实例和已准入的共享查询拒绝继续访问内存事实，调用方释放资源并重新打开后核对日志恢复结果。原有索引同步方式继续使用，资源运行时保留带写入上下文错误的预算和 I/O 分类。
- TQL 写入在词法、解析、写计划和预算准入失败时返回 `validation/not_committed`，进入事务提交后保留内层阶段。CLI 的写语句预解析同步该状态，Python、Node、HTTP 与 Console 传递内核字段。

> **验收范围** 本批使用独立完整事实样例验证三种 dtype、Rom／Mmap、WAL 同步故障和应用故障，覆盖后续写入拒绝、文件组移动、重新打开及再次持久化。文本和索引维护、语言实参提取及外部传输的结果确认继续按入口收敛。`committed` 的耐久性依照同步模式，分组内结果依照组尾完成点。

### Python 事务构造器的请求隔离与回收

- Python 事务方法改用显式借用，方法内转换回调的同事务重入返回 `TDB_REENTRANT_ACCESS` 和 `operation: "python_transaction_callback"`，线程争用返回 `TDB_BUSY` 和 `resource: "python_transaction"`。拒绝保留缓冲操作，其他事务按自身准入继续工作。
- 事务结束状态由 builder 是否存在统一表达，移除 `TxBuilder missing` 的 panic 路径。新增 `TDB_TRANSACTION_FINISHED`，详情为空对象；已消费或回滚对象再次缓冲、提交、回滚或进入上下文返回此错误。Python 异常保留 `RuntimeError` 继承关系，Rust 错误目录、Node 类型及 HTTP 映射同步更新。
- 上下文提交复用相同准入检查，锁忙或数据库关闭时保留事务；宿主转换异常及上下文异常保留原异常。补齐 GC 的事务到数据库引用遍历，修复 Hook 捕获事务所形成引用环的资源滞留；仍被持有的事务继续持有所属数据库，显式关闭语义保持原样。
- 契约覆盖 f32／f16／u64 的真实转换回调、事件控制线程交错、提交／回滚／失败后的状态、活动事务持有、GC 后目录移动及重开完整内容。Python 方法签名和正常返回类型保持原样。

> **适用范围**：本批收敛 Python 事务对象的借用和结束状态。绑定框架自动参数提取、普通转换错误、其他包装对象与统一写入结果字段继续分批处理。

### TQL 结构化解析诊断

- 数据库查询、Prepared TQL 准备和写语句解析增加 `stage: "lex" | "parse"` 与可选 `target.query_span`，保留 `TDB_QUERY_PARSE`、`schema_version: 1` 和 `details.reason`。Rust 新增 `QuerySyntax` 变体及结构化解析入口；旧 `QueryParse(String)` 与独立解析入口保留，直接匹配数据库解析错误的 Rust 调用者需适配新变体。
- 修正词法失败和已消费错误 token 的定位。源码锚点采用原始 UTF-8 字节的零长度范围，行列从 1 开始；CRLF 合并换行，列按 Unicode 标量计数，Tab 计一列。缺少准确位置的语义检查和超长输入省略位置。
- Python、Node、CLI JSON、嵌入 Console HTTP 和独立 Server 保留同一结构。内置 Console 数据适配器在原生异常、JSON 错误信封与 fetch 响应中透传扩展字段。Python 对缺失扩展字段提供 `None`，Node／JSON 省略字段；HTTP 查询解析拒绝保持 400 和原兼容类别。共享样例覆盖 f32／f16／u64 绑定、Unicode 与换行、读取与写语句拒绝及关闭重开后的完整内容。

> **接入提醒**：本批覆盖解析阶段与已确认的诊断锚点。完整 AST 范围、Prepared 参数绑定位置、其他操作阶段和写入结果字段继续分批补齐，详见 [TQL 诊断](../tql-reference.md#结构化查询诊断)。

### 嵌入式 Console 编辑冲突与事务确认

- Console RPC v1 增加 `getNodes(withRevision:true)`、`guardedTransaction` 和 `transactionStatus`，Rust、Python、Node 共用现有 JSON 入口。快照版本与节点内容在同一读锁下取得，版本前置条件在 Core 事务写锁内、WAL 前校验，冲突拒绝整笔事务。
- 编辑版本由 Core 事实变化驱动，宿主直接 CRUD、CAS、TQL、普通事务、向量和边更新都会使旧版本失效，删除重建会产生新令牌。版本覆盖整个数据库实例，因此不同节点的修改也可能冲突。
- 受保护事务按幂等键、原版本和完整操作匹配实例内回执，重试命中回执后直接返回原结果。确认接口校验完整原事务。WAL 提交阶段消耗原版本；成功回执缺失时保留未知状态。
- 回执按先入先出保留至多 64 条、记录内容合计至多 8 MiB，回执队列、记录和响应缓冲均受内存准入约束；关闭释放相应资源。回执淘汰或重新打开后可能无法确认结果，旧令牌会被拒绝；幂等回执使用实例内存，磁盘格式保持原样。
- 嵌入 HTTP 适配器将版本冲突、幂等键错配和结果未知映射为 409，保留结构化 Core 错误。原有无条件写入口保持原语义；HTML 的嵌入模式通过内置客户端接入版本保护事务。接入和可执行请求示例见[编辑协议](../embedded-console.md#编辑快照冲突与原事务确认)。

### 嵌入式 WebUI 组件接入

- 新增 `console-client.js` 和 `console-embed.js`，通过同源 iframe 挂载现有 HTML，支持宿主 HTTP 或 IPC 桥接。页面查询、节点/边、索引、向量与图操作接到 Console RPC v1；独立 Server 保留原 HTTP 模式。
- 文档编辑、删除和导入使用审阅队列，捕获版本并冻结完整事务；同批次基线不一致时要求重新读取和审阅。提交结果不明时只查询原事务回执，继续保留完整请求与后续草稿。Console 路径以十进制字符串传递完整 u64 节点 ID。
- 宿主使用稳定 `storageKey` 隔离各数据库的草稿、历史和偏好。卸载在未处理草稿或队列存在时返回错误；显式退出可先导出原事务。客户端停止准入、发送取消与 detach，并等待已准入桥接调用退出，数据库持久化及文件锁由宿主管理。
- 资源包加入客户端、挂载入口及各资源 SHA-256。Rust Server crate 导出对应脚本，文档提供 HTTP/IPC 接入和卸载示例。

> **使用范围**：组件资源与宿主页面同源。Console 邻域沿出边扩展，路径入口提供一条最短路径，查询使用 JSON 响应。向量工作区支持 f32/f16，u64 向量通过 TQL 使用；GSAP、Sigma 等增强能力仍按需加载 CDN。关闭窗口时宿主负责补充卸载，回执继续遵守实例内保留范围。

验收围绕独立业务写入、并发编辑胜负、完整事务重开、缓存淘汰、WAL/应用故障和取消后真实提交展开，并覆盖三种 dtype 的 SDK 制品；不包含 UI/UX 或界面耗时阈值测试。实际平台和运行范围以本次验收记录为准。

### 生命周期、文件锁与关闭加固

- 普通数据库析构后，独立查询及构建句柄拒绝发起新工作；已经准入的任务与实际存储共同持有文件租约，直到相关句柄和任务释放。修复旧 mmap 或后台任务仍存活时，新 Writer 已能打开原路径的问题。显式关闭继续提供不依赖对象回收的资源释放完成点。
- Windows 与 Unix 一致拒绝硬链接主文件，避免同一文件通过不同路径使用不同 WAL、sidecar 和锁。拒绝路径不创建别名制品；不改变磁盘格式。
- 同步关闭因准入或持久化失败而恢复开放时，恢复原有周期压实登记和周期配置。异步关闭失败仍保持拒绝新操作，等待显式重试。
- 自定义 Rust Hook 在持久化完成后的析构 panic 转为结构化关闭错误；存储与文件锁仍被释放，状态为 `closed`，后续等待按已关闭状态处理。普通持久化错误仍保留资源和原始错误，供排障后重试。
- nightly Server 新增 `build_app_with_shutdown` 和可克隆的 `ServerShutdown`。`shutdown().await` 排空写队列、真实查询与预热任务，再等待 Core 关闭；等待者取消后实际工作继续运行，关闭失败可重试。独立 Server 的退出流程使用该完成点；旧 `build_app` 入口保持兼容。

非 UI 回归覆盖普通析构与存活任务、完整数据重开、硬链接拒绝、关闭失败后的维护恢复、Hook 析构异常，以及 Server 排空、取消和重试。跨平台执行范围以对应验收结果为准；不包含 UI/UX 或界面耗时阈值检查。

### nightly WebUI：节点文档工作区

以下为独立 Server HTTP v1 模式的文档工作区行为；嵌入模式的版本与回执流程见上方组件接入说明。

- 新增节点文档工作区：JSON 与映射的 Markdown/纯文本正文共享草稿，支持浏览器草稿恢复、预览、大纲、查找替换、行级差异及提交审阅。写入复用 `/v1/transactions`，没有新磁盘格式或编辑租约。
- 跨入口合并或替换暂存项后仍保留节点版本、幂等键和确认状态；重新获取版本时校验编辑基线，外部修改要求显式处理冲突。
- 提交批次冻结请求体、前置条件和操作快照。等待响应期间可以继续编辑草稿和暂存其他节点，但不能替换或撤回已发送批次；成功只确认实际发送的内容并移除对应队列项。
- 含文档操作的事务在网络失败、服务端错误或成功响应无法解析时保留未知状态，通过“确认原事务”以相同幂等键重试完整请求。确认覆盖混合事务的全部操作，按原事务身份处理导入结果。确认后的新草稿使用新批次与新键；未知事务重试发生冲突后，继续保留未知状态并核对实际数据。
- 修复 Ctrl+S 冒泡导致的重复暂存；异步读取版本期间，同一文档的暂存串行化。文档写入与其他入口一致拒绝超出 JavaScript 安全整数范围的节点 ID。

> **使用提醒**：此能力属于 nightly WebUI，数据库锁由宿主管理。草稿按 origin、页面路径与节点 ID 区分；同一地址切换数据库前，应导出并处理旧草稿。未知事务的原请求保存在当前页面会话，刷新或关闭后需核对完整事务的实际结果。服务端幂等回执保存在实例内存中，重启后需按业务身份确认数据。

验收采用代码审查、语法检查及 Server 非 UI 契约，不包含 UI/UX、样式、布局、动画或耗时阈值测试。平台执行结果以对应提交验收为准。

## Unreleased

Subsequent changes build on stable `0.9.0`. Published changes are recorded in the [complete Chinese and English release notes](0.9.0.md).

### Kernel SDK layering, shared access, and request rejection

- Added the `sdk` module with `SharedDatabase` and `WeakDatabase`. Callers share one storage kernel and prepare independent `TxBuilder` transactions outside host locks. The kernel performs atomic validation and commit after SDK admission. Admission failures caused by contention, reentry, or closing preserve the pending builder.
- The `DatabaseReader`, `DatabaseWriter`, and `ConsoleHost` host adapters now live in the SDK, with existing import paths retained. `ConsoleHost::attach_shared` uses weak ownership. Dropping one handle releases that handle; explicit shutdown affects all shared handles. The storage kernel continues to own WAL semantics, file leases, recovery, and mmap release.
- Added `TDB_BUSY`, `TDB_REENTRANT_ACCESS`, and `TDB_HOST_LOCK_POISONED` for admission contention, same-instance reentry, and poisoned host locks following unwinding. Search Hook guards identify the database instance: same-instance operations managed by admission are rejected, while other databases retain their own admission checks. Shutdown inside a group-commit callback returns a reentry error. The public Node loader rejects synchronous callback reentry before N-API.
- `TriviumError::info()` and Python/Node kernel errors expose `schema_version: 1`, `code`, `message`, and `details`. CLI JSON mode writes kernel error objects to stderr. Nightly HTTP responses preserve Core codes and details and expose former HTTP categories as `legacyCode`; for example, `QUERY_PARSE_ERROR` becomes `TDB_QUERY_PARSE`. Node/HTTP encode error integers outside the JavaScript safe range as decimal strings.
- Console lock contention and HTTP concurrency admission now return `TDB_BUSY` instead of `TDB_IO/WouldBlock`. `details.resource` identifies `host_database`, `shared_database`, `console_request_id`, or `console_in_flight`. Node synchronous callback reentry returns `TDB_REENTRANT_ACCESS`. Consumers should update their structured error handling accordingly.
- Completed Python stub-generator return mappings for existing CAS, atomic batch deletion, unique-index, and n-gram query APIs, together with substring and equality-filter argument types. The delivered stub is regenerated against the current wheel.
- Completed the public Node error union with edit revision conflicts, idempotency key conflicts, and unknown write outcomes, allowing callers to narrow detail types by error code.
- Python database instance methods now acquire PyO3 borrows explicitly and fallibly. Same-database callback reentry returns `TDB_REENTRANT_ACCESS`, and cross-thread borrow contention returns `TDB_BUSY`. Batch-query workers retain the callback's database identity; shared read borrows continue to allow concurrent reads.
- Fixed Python transaction commits consuming the builder before borrowing the database and panicking on borrow conflicts. Commit now acquires the borrow and passes kernel admission before consuming operations. Contention, reentry, closure, or recovery protection preserves pending operations for later submission or rollback. Closed instances reject creation of new transaction objects.
- Once standalone Server shutdown begins, new reads and writes return `TDB_DATABASE_CLOSED` with empty details. HTTP status remains 503, `legacyCode: "SERVICE_UNAVAILABLE"` preserves the former category, and `retryable` is `false`.

> **Scope**: Disk formats and the single-writer transaction model remain at their existing versions. Binding argument extraction, borrow exceptions on other wrapper objects, complete field locations, and write-outcome metadata remain phased follow-up work. The next section covers Python transaction objects. Poisoned host locks require releasing related handles and checking recovery. Shutdown timeout only ends the wait. See the [kernel SDK guide](../kernel-sdk.md) for usage.

### Prepared parameter diagnostics and public error catalog

- Missing, unexpected, named JSON conversion, vector-slot type, and Rust non-finite Prepared parameter errors now include `stage=bind` and `target.parameter`. Parameter names retain their literal mapping keys, while missing and unexpected names continue to be selected in stable name order.
- Conversion and binding diagnostics preserve the original code, details, and cause. Rust adds the `QueryParameter` context variant and `underlying()` for matching the original variant. `ErrorTarget` provides distinct query-location and parameter-name representations. Python preserves exception inheritance, and Node, standalone Server, embedded HTTP, and Console data transport preserve the same diagnostic fields.
- Python stubs, Node public declarations, and type-consumption checks include the binding stage and parameter targets. Shared cases cover invalid arrays and objects, vector parameter types, sorted missing and unexpected names, and literal special-character names, with valid queries after rejection, unchanged WAL bytes, and complete facts after reopening.
- Adds a public error catalog for opening, shared access, transactions, Prepared queries, and closing, recording validation layers, fields, and completion boundaries. It documents the facts available at WAL, application, grouped synchronization, and response-delivery stages. General write outcomes and other API field locations remain phased work.

### Transaction write outcomes and failure delivery

- Transaction commit errors add optional `write` metadata with `phase`, `outcome`, and applicable `transaction_consumed`. Core distinguishes admission, validation, WAL, application, and response stages while preserving the original code, details, and cause. Rust adds the `WriteFailure` context variant, with `underlying()` exposing the original category for pattern matching.
- SDK admission checks lifecycle, write permission, and WAL writability before consuming a transaction. Shared Rust builders and active Python transactions retain buffered operations after admission rejection. Core validation and later failures report consumption. Known Node transaction-construction checks and callback reentry expose the corresponding state, while callers retain their parameter arrays.
- WAL append and application failures report `unknown`. Grouped commits report `group_sync/unknown` when final synchronization fails. Server delivers this result to successfully executed requests awaiting confirmation and preserves independent validation or execution errors. Group admission failures explicitly reject pending requests.
- Server queue saturation and expiration before execution report `not_committed`. Response-wait timeouts and lost response channels after enqueueing report `unknown`. HTTP errors carrying write context set `retryable=false`, allowing callers to decide retries using the phase, consumption state, and reconciliation result.
- Response-budget failures when replaying an already committed Console transaction retain `details.committed=true` and add `response/committed`. The original transaction remains confirmable through its receipt. Python stubs, Node declarations, CLI JSON, HTTP, and Console data adapters carry the same fields. Controlled faults cover WAL, application, and recovery in Rom and Mmap modes, plus Server grouped synchronization and background-task failures.

### Direct writes and pre-commit TQL boundaries

- Direct node and edge CRUD, Payload patches, and CAS provide write phases and outcomes. Admission and validation failures report `not_committed`, while WAL or application failures report `unknown`. Direct entry points omit transaction-buffer consumption. Original error codes, details, exception inheritance, and single-entry WAL encoding remain in use.
- Direct writes share recovery protection during application after WAL append. Failure quarantines the current instance and already admitted shared queries. Callers release resources, reopen, and reconcile recovered facts. Existing index synchronization remains in use, and resource diagnostics retain budget and I/O classification through write context.
- TQL mutation lexing, parsing, planning, and budget-admission failures report `validation/not_committed`. Once transaction submission starts, its inner phase is preserved. CLI mutation pre-parsing provides the same outcome, and Python, Node, HTTP, and Console carry the Core fields.

> **Validation scope** Independent complete-fact fixtures cover three dtypes, Rom and Mmap, WAL synchronization faults, application faults, rejection of later writes, file-group moves, reopening, and another persistence cycle. Text and index maintenance, language argument extraction, and external transport confirmation remain phased work. Durability follows the synchronization mode, and grouped results depend on the final group completion point.

### Python transaction request isolation and collection

- Python transaction methods now acquire explicit fallible borrows. Reentry from an in-method conversion callback returns `TDB_REENTRANT_ACCESS` with `operation: "python_transaction_callback"`; cross-thread contention returns `TDB_BUSY` with `resource: "python_transaction"`. Rejection retains buffered operations, while other transactions follow their own admission state.
- Builder presence is the single source of transaction state, removing the `TxBuilder missing` panic paths. Adds `TDB_TRANSACTION_FINISHED` with empty details for buffering, committing, rolling back, or entering a context after consumption or rollback. Python exceptions retain their `RuntimeError` inheritance; the Rust catalog, Node declarations, and HTTP mappings are updated together.
- Context-manager commits use the same admission checks and retain the transaction when the database is busy or closed. Host conversion and context exceptions retain their original identity. GC now traverses the transaction's database reference, resolving resource retention in cycles formed by Hooks capturing transactions. A retained transaction continues to own its database; explicit close semantics remain unchanged.
- Contracts cover real conversion callbacks and event-controlled contention across f32/f16/u64, post-commit/rollback/failure states, retained ownership, directory moves after GC, and complete data after reopening. Python method signatures and normal return types remain unchanged.

> **Scope**: this batch covers Python transaction-object borrowing and finished state. Framework argument extraction, ordinary conversion errors, other wrapper objects, and general write-outcome fields remain phased work.

### Structured TQL parsing diagnostics

- Database queries, Prepared TQL preparation, and mutation parsing add `stage: "lex" | "parse"` and optional `target.query_span`, preserving `TDB_QUERY_PARSE`, `schema_version: 1`, and `details.reason`. Rust adds the `QuerySyntax` variant and structured parser entry points. The existing `QueryParse(String)` variant and standalone parser entry points remain available; Rust callers matching database parse errors must handle the new variant.
- Corrects locations for failed lexical units and consumed unexpected tokens. Anchors use zero-length ranges in the original UTF-8 byte offsets, with one-based lines and columns. CRLF counts as one newline, columns count Unicode scalar values, and a tab counts as one column. Semantic checks without an accurate location and oversized input omit the location.
- Python, Node, CLI JSON, embedded Console HTTP, and standalone Server preserve the same structure. The built-in Console data adapter forwards extension fields through native exceptions, JSON error envelopes, and fetch responses. Python uses `None` for absent extension fields; Node and JSON omit them. HTTP parsing rejection retains status 400 and the legacy category. Shared fixtures cover f32/f16/u64 bindings, Unicode and newlines, rejected reads and mutations, and complete data after close and reopen.

> **Integration note**: this batch covers parsing stages and confirmed diagnostic anchors. Full AST ranges, Prepared parameter binding locations, other operation stages, and write-outcome fields remain phased work. See [TQL diagnostics](../tql-reference.md#结构化查询诊断).

### Embedded Console edit conflicts and transaction confirmation

- Console RPC v1 adds `getNodes(withRevision:true)`, `guardedTransaction`, and `transactionStatus` through the existing JSON entry points shared by Rust, Python, and Node. Node data and its revision are captured under one read lock; Core validates the revision under the transaction write lock before WAL append, rejecting the entire transaction on conflict.
- Core fact changes advance the edit revision, including direct host CRUD, CAS, TQL, ordinary transactions, vectors, and edges. Deleting and recreating the same content produces a new token. Revisions cover the whole database instance, so changes to unrelated nodes can conflict.
- Guarded transactions match receipts by idempotency key, original revision, and complete operations, returning the original result when a retry matches a receipt. Status requests verify the complete original transaction. WAL submission consumes the original revision; a missing success receipt leaves the outcome unknown.
- Instance-local receipts retain at most 64 entries and 8 MiB of record contents, evicting the oldest first. Receipt queues, records, and acknowledgement buffers are admitted against memory budgets and released on close. Eviction or reopening can make confirmation unavailable; old tokens are rejected. Idempotency receipts use instance memory, and the disk format remains unchanged.
- The embedded HTTP adapter maps edit conflicts, key mismatches, and unknown outcomes to 409 while preserving structured Core errors. Existing unconditional writes retain their behavior. Embedded HTML uses its built-in client for guarded transactions. See the [editing protocol](../embedded-console.md#编辑快照冲突与原事务确认) for integration and request examples.

### Embedded WebUI component integration

- Adds `console-client.js` and `console-embed.js` to mount the existing HTML in a same-origin iframe through host HTTP or IPC bridges. Queries, nodes, edges, indexes, vector search, and graph operations use Console RPC v1. Standalone Server retains its HTTP mode.
- Document edits, deletion, and imports enter write review with captured revisions and frozen transaction identities. Mixed baselines require fresh reads and review. Unknown submissions only query the original receipt and retain the complete request plus later draft edits. Console node IDs use lossless decimal strings across the full u64 range.
- A stable host-provided `storageKey` isolates database drafts, history, and preferences. Disposal rejects unresolved drafts or queued work; hosts can export the original transaction before explicit exit. The client stops admission, requests cancellation and detach, and waits for admitted bridge calls. Hosts retain ownership of database persistence and file locks.
- Bundles include the client, mount entry, and SHA-256 hashes for each resource. The Rust Server crate exports both scripts; integration documentation includes HTTP/IPC mounting and disposal examples.

> **Scope**: component resources share the host page origin. Console neighborhoods follow outgoing edges, path requests return one shortest path, and queries return JSON. The vector workspace supports f32/f16; use TQL for u64 vectors. Enhancements including GSAP and Sigma still load from CDN on demand. Hosts complete session teardown when windows close; receipts retain their instance-local lifetime.

Validation focuses on independent host writes, concurrent editing outcomes, complete transaction recovery, receipt eviction, WAL/application failures, and real commits after cancellation, including SDK artifacts across three dtypes. UI/UX and interface latency thresholds are excluded; executed platforms and suites follow the validation record.

### Lifecycle, file leases, and shutdown hardening

- After the database owner is dropped, independent search and build handles reject new work. Admitted tasks retain the file lease together with their storage until the relevant handles and tasks are released. This prevents a replacement writer from opening the same path while old mappings or background tasks remain alive. Explicit close still provides a resource-release completion point independent of object collection.
- Windows now rejects hard-linked database files consistently with Unix, preventing one file from being accessed through different WAL, sidecar, and lock prefixes. Rejection creates no alias artifacts; the disk format is unchanged.
- When synchronous close rolls back to the open state after admission or persistence failure, its original periodic compaction registration and interval are restored. Failed asynchronous shutdown continues to reject new operations until explicitly retried.
- A panic from a custom Rust hook destructor after persistence is reported as a structured shutdown error. Storage and file leases are still released, the phase is `closed`, and later waits observe the closed state. Ordinary persistence failures continue to retain resources and the original error for recovery and retry.
- The nightly Server adds `build_app_with_shutdown` and a cloneable `ServerShutdown`. `shutdown().await` drains queued writes, actual queries, and warmup work before awaiting Core close. The underlying work continues after a waiter is cancelled; failed shutdown can be retried. The standalone server now awaits this completion point during exit. The original `build_app` remains compatible.

Non-UI regressions cover owner destruction with surviving tasks, complete data reopening, hard-link rejection, maintenance restoration after close failure, hook destructor panics, and server draining, cancellation, and retries. Platform coverage follows the reported validation results; UI/UX and interface latency thresholds are excluded.

### Nightly WebUI: node document workspace

The following describes the document workspace in standalone Server HTTP v1 mode. Embedded revision and receipt handling follows the component integration described above.

- Adds a node document workspace where JSON and mapped Markdown/plain-text views share drafts, with browser draft restoration, preview, outline, find/replace, line diffs, and write review. Writes reuse `/v1/transactions`, without a new disk format or edit lease.
- Merging or replacing staged work from another entry retains node versions, idempotency, and acknowledgement tracking. Refreshing a version also checks the editing baseline; external changes require explicit conflict handling.
- Submitted batches freeze their request body, preconditions, and operation snapshots. Users may continue editing drafts and staging other nodes while awaiting a response, but cannot replace or withdraw the submitted batch. Success acknowledges only transmitted content and removes only that batch's queue entries.
- Network failures, server errors, and unreadable success responses leave transactions containing document operations in an unknown state. “Confirm original transaction” retries the complete request with the original idempotency key. Confirmation checks the complete mixed transaction before acknowledging its operations. New drafts use a new batch and key after acknowledgement. A conflict while retrying an unknown transaction leaves its outcome unknown and requires reconciliation.
- Fixes duplicate staging from Ctrl+S event propagation and serializes staging within a document during asynchronous version reads. Document writes reject node IDs outside JavaScript's safe integer range, consistently with other write entries.

> **Usage note**: this is a nightly WebUI capability; the host manages database locks. Drafts are scoped by origin, page path, and node ID. Export and resolve old drafts before switching databases at the same address. The original request of an unknown transaction is held in the current page session; after refreshing or closing it, verify the entire transaction's actual outcome. Server idempotency receipts use instance memory; after restart, reconcile data by business identity.

Acceptance uses code review, syntax checks, and non-UI server contracts. UI/UX, styling, layout, animation, and elapsed-time threshold tests are excluded. Platform execution results are tied to the validated commit.
