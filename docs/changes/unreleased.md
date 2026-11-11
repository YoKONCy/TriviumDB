# 未发布变更

后续变更以 `0.9.0` 正式版为基线。已发布内容见[完整中英文变更说明](0.9.0.md)。

### 管线基准来源与浸泡资源验收

- 修复 `bench_pipeline_gate` 在属性过滤前后精排对照中，因两种算子序列的 `property_origin` 差异而停止的问题。基准逐项验证各计划的属性入口，再按原始顺序精确比较其余完整行字段；回归覆盖完整解析分数、来源、成员及可选零分差异。公开文档补齐该字段的物理计划语义。
- 五分钟有界浸泡采用明确的内核与 Payload 缓存配额，验证维护后的资源归还、完整记录和两次持久化后的重开事实。RSS 保留初始、峰值与最终观测，用于分析工作集和内存组成；失败退出先释放对象并清理样本。
- 新增继承 release 优化设置并关闭 LTO 的 `release-contracts` profile，提供 Windows 优化故障契约入口。CI 在该配置下实际执行 allocator、定点 I/O 和发布阶段强杀套件；原始 release 与工具链诊断按各自配置记录。

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

> **适用范围**　共享访问采用单写事务模型，格式变化见统一请求上下文与跨重开确认一节。其他包装对象及其余公共入口按分阶段契约收敛。宿主锁中毒需要结束相关句柄并检查恢复，关闭超时结束当前等待。接入示例见[内核 SDK 文档](../kernel-sdk.md)。

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

> **验收范围** 本批使用独立完整事实样例验证三种 dtype、Rom／Mmap、WAL 同步故障和应用故障，覆盖后续写入拒绝、文件组移动、重新打开及再次持久化。语言实参提取及外部传输的结果确认继续按入口收敛。`committed` 的耐久性依照同步模式，分组内结果依照组尾完成点。

### 文本与索引维护完成点

- 文本登记、关键词、预热、完整重建、属性索引创建／删除、Rust 显式 QuIVer 构建、刷盘和压实补齐写入结果。维护入口统一核对权限、生命周期与 WAL 可写性，关闭的 writer 在改变文本或索引定义前拒绝请求。
- 新增 `phase=publish`，由存储内核按文件组提交点记录结果。提交标记完成前的发布失败为 `unknown`，提交后的 WAL 清理失败为 `committed`。唯一约束候选预检失败保留原定义，发布失败隔离实例，重新打开后按提交点恢复约束及完整事实。
- 正文与关键词更新在分词、转换和索引增长前核对实例预算并取得共享工作区。预算拒绝保留原文本和 WAL，返回时归还临时预留。创建唯一索引使用已有的有序活跃 ID 目录，重复值错误中的已有节点与冲突节点身份保持确定。
- Node 类型增加 `publish`，共享样例同步 Rust、实际 Python wheel、Node 原生模块及 npm tarball。验证覆盖完整节点、向量、Payload、边、索引定义、文本查询、只读文件组、失败后重开和再次持久化。

> **入口范围** Rust 与 Node 覆盖 Rom／Mmap，Python 当前构造接口使用默认 Mmap，三端均覆盖 f32／f16／u64。显式 QuIVer 构建、已捕获任务与运行期模式为现有 Rust 入口。

### 后台维护与独立目标发布

- QuIVer 独立任务及显式构建在发布候选前重新检查 WAL 和恢复保护，防止构建期间 writer 失效后仍安装候选。任务执行或丢弃后归还工作区与准入资源，已准入任务继续遵循原有文件租约生命周期。
- 运行期 QuIVer 模式、自动构建开关及周期压实配置补齐写入错误字段和 WAL 准入。新压实登记成功后替换旧登记，配置失败保留原登记。后台 `last_failure` 增加规范 `code`、`details` 与可选 `write`，周期压实按真实发布阶段提供结果。
- 快照、恢复、manifest 及维度迁移的最外层 `write` 描述目标发布。源检查点或暂存库失败在目标发布前报告 `validation/not_committed`，原错误链继续保留。快照重命名后的目录同步失败为 `publish/unknown`。
- 迁移在公开目标前取得最终实例配额，防止提交与返回实例之间额度被其他实例占用。提交前回滚清理完成报告 `publish/not_committed`，标记重命名后的同步失败为 `publish/unknown`，耐久提交后的清理失败为 `publish/committed`。`generation_id` 在快照及 manifest 的源检查点前校验。
- 验收复用维护契约与既有故障套件，覆盖受控构建交错、真实后台压实、目标配额竞争、完整恢复事实和再次持久化。类型声明与 Python／Node 实际制品同步检查。

### 管理入口与普通字段诊断

- 错误定位增加 `target.field`，为已覆盖的普通 API 输入保留字段名、原始类别、详情和 cause。字段与 Prepared 包装中的既有写入结果继续透传，外层准入保留实际执行点的事实。
- 同步模式、容量预留和图索引修复在修改前核对 WAL；内存额度调整补齐拒绝阶段与字段定位，失败保留原配置和共享登记。Rust 使用 `bytes`，Python／Node 的 MiB 接口使用 `mb`；ReadOnly／Immutable 实例可调整自身运行时额度。
- 图修复在内存应用后发生检查点错误时隔离实例，并按实际 `apply`／`publish` 阶段报告结果，释放相关句柄后通过重新打开恢复。既有 QuIVer 构建句柄在后续捕获任务时读取当前额度，修复运行期修改预算后仍采用旧额度的问题。
- Python／Node 的运行期同步模式字符串错误和显式管理参数转换提供规范字段与 `validation/not_committed`。Python 的 MiB 换算溢出保留 `OverflowError` 继承。SDK 在同步模式、额度、容量及图修复回调重入时提供 `admission/not_committed`。
- 共享维护矩阵扩展到 Rust 28 个入口及 Python／Node 23 个入口，补齐真实回调、预算拒绝回退、只读额度与文件组、图修复故障、重开内容和类型消费。语言框架其他自动提取、完整 AST 定位及外层传输继续分批收敛。

### 管理实参提取与 Prepared 映射

- Python 的容量预留、MiB 额度、自动压实间隔和自动构建开关在取得数据库请求借用后转换。已覆盖内置类型错误保留 `TypeError`／`OverflowError` 继承，并增加 `TDB_INVALID_INPUT`、`target.field` 和 `validation/not_committed`。自定义整数协议异常保留原对象、cause 与已有诊断。
- Python Prepared 实现与声明统一接受字符串键 `Mapping`，包括 `MappingProxyType` 和 `UserDict`。容器及键拒绝使用 `target.field=parameters`，已知参数值转换失败使用 `stage=bind` 和原参数名。映射协议中的宿主异常沿原路径传播，转换回调的同库重入按公共准入契约拒绝。
- Node 管理标量按原始数值或布尔类型校验，已覆盖类型拒绝提供规范字段与写入阶段。自动构建及自动压实回调重入提供 `admission/not_committed`。已转换的 Prepared 非对象容器使用 `target.field=parameters`。公开类型、Python 默认参数和 Node 可选间隔的默认行为保持原契约。
- 实际绑定用例覆盖三种 dtype、宿主转换异常与 cause、回调必达、拒绝后的 WAL 和完整数据、后续有效请求及再次持久化。其余入口的语言框架实参提取、JSON 转换与完整 AST 定位继续分批收敛。

### Prepared 原始标量与管理字符串

- Node Prepared 在方法内读取原始参数，为 NaN、无穷、undefined、BigInt、Symbol、函数及复合参数值提供 `TDB_INVALID_INPUT`、`stage=bind` 和原始 `target.parameter`。外层错误容器提供 `target.field=parameters`，键名完整保留 NUL 和 Unicode。
- 数组和对象参数在标量边界拒绝，循环复合值按结构化错误返回。属性枚举与 getter 抛出的宿主异常保留原对象、cause 和已有错误字段。有效标量复用原数值转换，属性枚举保持既有规则。
- Python／Node 的同步模式和 generation 标识补齐字符串类型拒绝。Python 在请求借用后提取字符串，已覆盖错误保留原类型异常及规范字段；Node 严格检查原始字符串类型。manifest 回调重入提供 `admission/not_committed`，发布结果沿用 Core 完成点。
- 实际制品覆盖三种 dtype、有效标量边界、getter／Proxy 异常、回调拒绝、错误后有效查询、WAL 与重开完整内容。其余 N-API／PyO3 实参、其他 JSON 入口、AST 定位和外部传输继续分批收敛。

### 查询实参与 Filter 映射

- Python 普通及分组检索的整数数量／展开字段、批量检索的数量／并行度字段在请求借用后提取。已覆盖内置类型错误保留 `TypeError`／`OverflowError` 及字段定位，宿主整数协议异常保留原对象与 cause。Node 对应入口补齐原始数值类型及安全整数拒绝，保留现有默认值与可选 null 行为。
- 三个 TQL 查询入口补齐字符串类型拒绝的 `target.field=query`。拒绝发生在解析之前，阶段省略；读取错误省略 `write`，写查询类型拒绝提供 `validation/not_committed`，SDK 重入提供 `admission/not_committed`。Python 编码异常保留原类型。
- Python 普通、分组、上下文、高级及混合检索实现声明中的外层 `Mapping` Filter 契约，支持 `MappingProxyType`／`UserDict`。转换在请求借用内执行，宿主映射异常原样传播。Python 的已覆盖 Filter 表示错误及两端 Core 语义拒绝提供规范类别和对应字段，继续复用 Core 过滤规则。
- 实际绑定用例核对三种 dtype 的拒绝、真实转换回调、有效默认调用、原宿主异常、完整命中与排除集合、WAL、后续有效写入及重开内容。npm 实际安装包显式等待异步批量查询验收完成，类型消费保持公开参数边界。

### 查询取消原因与期限传输

- Core 的主动取消继续返回 `TDB_QUERY_CANCELLED`，期限到达增加 `QueryDeadlineExceeded` 及 `TDB_QUERY_DEADLINE_EXCEEDED`，详情为空对象。同一协作检查点同时观察到取消与期限到达时，期限优先；克隆共享取消信号，外部期限与 Console 会话期限取较早者。
- 独立 Server 的期限拒绝统一使用规范期限码和 `legacyCode=REQUEST_TIMEOUT`，保持 HTTP 504 并纳入超时指标。已知写入结果继续附带原阶段与 `retryable=false`。嵌入 Console 的请求体读取期限为 HTTP 408，执行期限为 HTTP 504，主动取消仍为 HTTP 408。此前将执行期限归为取消的调用方需处理新增规范码，Rust 穷尽匹配需增加新变体分支。
- Console 在分发前确认终止的写请求提供 `admission/not_committed`。事务、版本保护事务、节点导入及边导入在 WAL 前准备完成处终止时提供 `validation/not_committed`，准备的回执与响应工作区随请求退出归还。进入 WAL 后沿用原子提交完成点。
- 用例以已过期控制器、真实提交前屏障和不完成的外部请求体验证原因、同一请求 ID 重用、WAL 与完整事实、回执工作区、实际异步退出及文件释放；覆盖三种 dtype、Rom／Mmap、HTTP 许可归还和后续有效写入。Python 错误映射、实际绑定及 npm 类型目录同步新增类别。

### 扩展检索参数与锚点序列

- 精确、图优先、上下文、高级及混合检索的已覆盖整数参数补齐类型拒绝与字段定位。Python 在请求借用后提取整数并保留原类型及宿主协议异常，Node 继续使用安全整数和既有正数规则。公开默认值与类型消费同步验证。
- Python 图优先检索的锚点序列移入请求范围，序列取值及元素整数回调中的同库写入返回重入错误。混合检索文本提取补齐 `query_text`／`queryText` 字段，读取错误省略写入结果。
- Node 上下文与高级配置的已覆盖数量字段提供规范类型诊断。配置字段以省略或 `undefined` 使用默认值，`null` 按既有类型边界拒绝。可选方法实参继续支持 `null` 默认值，配置 getter 的原异常和 cause 直接传播。
- 三种 dtype 的实际绑定用例验证字段拒绝、整数／序列／配置回调、外层完整结果、原宿主异常、有效默认调用及后续写入。关闭后继续持有旧对象并移动 Windows 文件组，重开核对更新向量、Payload 和边。向量／嵌套成员定位与框架序列长度提示策略继续按独立契约收敛。

### 图读取参数与直接 CRUD 准入结果

- 邻居、可达性、详细可达性及子图读取补齐 ID、深度／预算和方向的字段诊断。Python 在请求借用内转换参数及标签序列，保留原类型异常、方向枚举的 ValueError 和宿主协议异常。Node 对已覆盖字段执行原始类型校验，并保留配置 getter 的原异常及既有 null 默认规则。
- 十五个直接 CRUD 入口在 SDK 层拒绝重入或借用争用时提供 admission/not_committed，消费字段省略。覆盖节点及边写入、两类批量插入、原子批量删除、Payload 补丁／条件替换及向量更新。进入内核后的事务、WAL 与发布结果继续由内核确定。
- Node 公共声明补齐已有运行接口 deleteManyAtomic 和 compareAndSetPayloadField，并加入精确返回类型及错误输入消费样例。
- 独立三节点链样例覆盖三种 dtype、完整路径、逐跳属性、出向／双向扫描量、默认值、回调拒绝及 WAL 完整性。关闭后持有旧对象并移动 Windows 文件组，重开核对全部事实，再执行条件更新和原子删除并再次重开。嵌套成员、框架长度提示异常策略和其他入口继续按各自契约收敛。

### 基础读取参数与结果所有权

- 节点、Payload、存在性、出边、入边和精确边读取补齐 ID／标签提取诊断，字段使用 id、src、dst 和 label。Python 在读请求借用内执行整数协议，内置错误保留异常继承，自定义整数协议原异常直接传播。Node 执行原始类型与安全整数检查。
- Python 存在性指南采用实际公开入口 id in db。入边读取的 None／null 与省略标签表示全部标签，空标签按其内容精确匹配；精确边读取要求标签实参。Unicode／NUL 标签完整保留。
- 真实制品验证三种 dtype、完整节点和边、不存在节点、语言整数范围、回调重入、Python 跨线程共享读取及写争用。关闭时继续持有节点、向量、Payload、边和旧数据库对象，文件组移动后旧结果保持独立快照，更新后的事实在两次重开中完整保留。

### 公开调用形状与框架诊断

- Python 包按原生公开签名预检实例方法、选定魔术方法和模块函数的实参结构。缺参、未知关键字、重复赋值、位置专用参数和位置数量错误保留 TypeError 继承，提供 TDB_INVALID_INPUT、target.field 及 details.argument_kind。多项问题按固定结构顺序定位，Python 目标字段保留原始 Unicode／NUL／孤立 surrogate 关键字。
- Node 公共加载器在 N-API 转换前定位已登记的必需位置缺参，显式 undefined 继续执行既有类型校验，额外位置参数和配置对象沿用各入口规则。回调重入继续优先拒绝。
- SDK 统一构造已覆盖写调用的 validation/not_committed 结果，Python 事务和 Node commitTransaction 提供 transaction_consumed=false。Python 有效调用继续转交原生绑定，宿主异常对象、cause、类身份、签名与默认值保留。
- 三种 dtype 的实际制品用例核对诊断、转换回调零次、完整事实及 WAL，随后真实提交、持有旧结果关闭并移动 Windows 文件组，两次重开验证持久化。构造期诊断、错误 receiver 和 callable 入口之前的解释器展开错误继续单独收敛。

### 构造参数值与失败资源归还

- 数据库和资源运行时构造的已覆盖整数、字符串、枚举及 MiB 换算提供 TDB_INVALID_INPUT 和 target.field，Node 同时覆盖数据库布尔配置。Python 保留内置类型异常继承、自定义整数协议的原异常与 cause，Node 保留配置 getter 的原异常。会话配置的类型与 JSON 拒绝定位 options_json／optionsJson。
- 修复极端维度拒绝诊断中的整数乘法溢出。零维度和超过 65536 的维度由内核直接拒绝并定位 dim，运行时线程上限与维护额度冲突同样提供字段。打开失败后已登记任务与配额按所有权归还。
- 独立记录验证三种 dtype、重复 writer、打开失败后的配额归还、持有旧结果及错误对象时的 Windows 文件移动、身份不匹配后的同路径重试和两次持久化重开。Python 使用当前 Mmap 构造接口，Node 覆盖 Rom／Mmap。Python 公共存根将缓存 MiB 参数明确为 int，正反类型消费同步。
- 本节记录构造参数值；构造调用形状及普通 CRUD 的后续收敛见下文。

### 配置容器与 runtime 句柄诊断

- Node 数据库和资源运行时的 options 在属性读取前校验为非数组对象。资源运行时先前将原始值或数组按默认配置接受的路径现返回 TDB_INVALID_INPUT，字段为 options；省略、undefined 和 null 保留默认配置。具名属性按既有原型链读取，未知属性沿各入口规则处理。
- Node 旧数字位置数据库构造补齐 TDB_API_MIGRATION_REQUIRED、迁移详情和 options 字段。npm 声明补齐配置及快照模块 runtime 的既有 null 默认形式，数据库配置中的 runtime 成员继续要求有效句柄。
- Python 数据库的内置布尔提取和原生 runtime 类型拒绝补齐字段。两端 runtime 校验同时服务数据库、快照校验和恢复模块；已知恢复句柄拒绝提供 validation/not_committed。Node getter／Proxy 及 Python 宿主转换异常保留原对象、cause 和已有属性。
- 实际制品验证非法配置、原异常、完整默认维度记录、无原型配置、继承 getter、未知属性及持有 writer 时的 WAL。随后实际创建、校验和恢复快照，重开核对完整节点与配额，再执行关闭、文件移动和再次持久化。

### 构造调用形状与普通 CRUD 转换

- Python 的 TriviumDB、ResourceRuntime 和 ConsoleSession 在原生构造入口按 SDK 公开签名元数据绑定实参，补齐缺参、未知关键字、重复赋值和额外位置的 TypeError、TDB_INVALID_INPUT、target.field 与 details.argument_kind。相同元数据提供 __signature__，兼容 Python 3.9，生成存根同步准确类型。批量插入的 vectors／payloads 外层声明更正为 list，与既有原生表示一致；ID 集合与单条向量继续采用 Sequence。形状拒绝发生在值协议、文件创建和资源登记前，原生类身份、签名及默认值保留。
- Node 数据库构造在缺少 path 时提供规范缺参，构造转发保留 prototype、constructor、instanceof、子类与 newTarget。显式 undefined 路径沿类型拒绝处理，额外位置参数遵循原有 JavaScript 调用规则。
- 十五个普通 CRUD 的已知语言类型、表示范围和 JSON 转换拒绝补齐 validation/not_committed，省略事务消费字段。批量插入的内部临时事务消费字段由 SDK 隐去，保留内核阶段、结果与 cause；Node 自动 ID 的安全整数拒绝补齐提交前结果。顶层 target.field 保留参数名，批量和 JSON 成员使用 RFC 6901 JSON Pointer，保留数组下标、转义键、Unicode 与 NUL。无法表示为 JSON 字符串的对象键定位所属容器。
- Python 的 ID、向量分量、边权和 JSON 转换在请求借用内完成，普通整数／浮点协议和原有 Sequence 输入继续接受。长度提示异常沿既有规则忽略，缓冲按实际元素增长。批量转换固定外层下标范围，回调缩短列表造成成员缺失时定位成员并拒绝整批，追加成员留待后续调用处理。宿主整数、浮点、迭代、getitem、getter 与 Proxy 的原异常身份、cause 和已有属性保留，同库回调重入按请求准入拒绝。
- Node 构造字符串、普通 CRUD 字符串和 JSON 字符串严格解码 UTF-16，孤立 surrogate 按输入错误拒绝，避免替换字符改变路径、标签或 JSON 键。有效 Unicode 和 NUL 完整保留。可选边参数沿各语言原有默认规则，Node upsertEdge 的 weight 继续必填。移除 Payload 补丁的已废弃简写示例。
- 新增独立 CRUD 字段矩阵及完整前后事实。实际绑定契约覆盖三种 dtype、Python 当前 Mmap 和 Node Rom／Mmap，逐字段验证失败后的内容与 WAL，再通过全部十五个入口写入、持有旧对象关闭、移动 Windows 文件组及两次持久化重开。解释器在 callable 之前的展开错误、错误 receiver 和 JavaScript 非构造调用遵循宿主异常契约。其余检索、索引、AST 与包装对象继续按各自入口推进。

### 检索、图读取与文本入口字段

- 八类检索补齐浮点、布尔、向量、锚点、标签和 JSON 成员的转换诊断。顶层字段沿公开参数名提供，嵌套字段使用 JSON Pointer，读取错误省略 write。Python 转换在请求借用内执行，Node 保留 getter 原异常、数组读取和既有同步／Promise 完成方式，u64 批量范围错误包含行列位置。
- 检索配置的有限性、扩散偏置维度与最小边权约束由内核提供准确字段，空批次在返回空结果前验证同一配置。混合权重由共享 SDK 换算，hybrid_alpha／hybridAlpha 的 NaN、Infinity 及换算溢出按输入错误拒绝，修复 max 运算掩盖非法输入的路径。Rust 通过 underlying() 取得带字段错误的原类别。
- 四类图读取补齐 Node 标签容器和成员、配置外层及方向的诊断。Python 图标签与检索序列共用既有长度提示容错规则，缓冲按实际元素增长。Node 高级／上下文检索和可达性配置按非数组对象校验，省略／null 使用默认配置，可选位置参数的 null 与配置成员规则分别保留，npm 声明同步有效默认形式。
- 属性／子串查找补齐对象、JSON、数量、字段和内容诊断，1—1000000 数量范围、空条件及空子串输入由内核判定。正文、关键词、构建和完整文本重建入口补齐已知参数转换与 SDK 拒绝的写入结果，失败保留既有文本和索引事实。
- 完整重建记录使用 id、text 和可选 keywords。成员缺失、类型、编码与未知字段提供记录路径，Node 先前忽略的未知重建字段按闭合记录契约拒绝。本批 Node 文本、标签、字段和记录字符串严格解码 UTF-16，孤立 surrogate 按输入错误拒绝，Unicode 与 NUL 完整传递。Python 按声明接受 Mapping，先固定本批记录对象，再物化和转换字段，回调修改外层列表时继续处理本批已捕获记录。宿主映射、数值协议和 getter 的异常保留原对象、cause 与已有属性。
- 独立字段矩阵覆盖三种 dtype，Python 当前 Mmap 与 Node Rom／Mmap。用例验证完整分数／Payload、字段路径、原异常、线程借用争用、拒绝后的 WAL、完整正文／关键词与图事实，关闭后移动文件组并两次重开。公共签名、默认值、存根和实际包的类型正反消费同步维护。

### Python 事务构造器的请求隔离与回收

- Python 事务方法改用显式借用，方法内转换回调的同事务重入返回 `TDB_REENTRANT_ACCESS` 和 `operation: "python_transaction_callback"`，线程争用返回 `TDB_BUSY` 和 `resource: "python_transaction"`。拒绝保留缓冲操作，其他事务按自身准入继续工作。
- 事务结束状态由 builder 是否存在统一表达，移除 `TxBuilder missing` 的 panic 路径。新增 `TDB_TRANSACTION_FINISHED`，详情为空对象；已消费或回滚对象再次缓冲、提交、回滚或进入上下文返回此错误。Python 异常保留 `RuntimeError` 继承关系，Rust 错误目录、Node 类型及 HTTP 映射同步更新。
- 上下文提交复用相同准入检查，锁忙或数据库关闭时保留事务；宿主转换异常及上下文异常保留原异常。补齐 GC 的事务到数据库引用遍历，修复 Hook 捕获事务所形成引用环的资源滞留；仍被持有的事务继续持有所属数据库，显式关闭语义保持原样。
- 契约覆盖 f32／f16／u64 的真实转换回调、事件控制线程交错、提交／回滚／失败后的状态、活动事务持有、GC 后目录移动及重开完整内容。Python 方法签名和正常返回类型保持原样。

> **适用范围**：本批收敛 Python 事务对象的借用和结束状态。绑定框架自动参数提取、普通转换错误、其他包装对象与统一写入结果字段继续分批处理。

### TQL 结构化解析诊断

- 数据库查询、Prepared TQL 准备和写语句解析增加 `stage: "lex" | "parse"` 与可选 `target.query_span`，保留 `TDB_QUERY_PARSE`、`schema_version: 1` 和 `details.reason`。Rust 新增 `QuerySyntax` 变体及结构化解析入口；旧 `QueryParse(String)` 与独立解析入口保留，直接匹配数据库解析错误的 Rust 调用者需适配新变体。
- 修正词法失败和已消费错误 token 的定位。可靠位置采用原始 UTF-8 半开字节范围，终点来自实际扫描，空白与注释独立于相邻 token。输入结束及 token 预算边界使用零长度锚点。行列从 1 开始，CRLF 合并换行，列按 Unicode 标量计数，Tab 计一列。缺少准确位置的语义检查和超长输入省略位置。
- 数据库准备的 Prepared 参数错误同时携带参数名与首个真实引用范围，包含 `$` 前缀。源码由 Core 保存，缺参、已引用参数的 JSON 转换和向量槽位绑定共享定位，字符串、注释与文档过滤操作符按解析语义区分。额外参数及独立标量转换、手工 AST 准备使用已知参数名。源码元数据由 Prepared 克隆共享，Rust `QueryParameterSource` 包装保留原始类别、详情、cause 和已知写入结果，原始变体通过 `underlying()` 匹配。
- Python、Node、CLI JSON、嵌入 Console HTTP 和独立 Server 保留同一结构。内置 Console 数据适配器在原生异常、JSON 错误信封与 fetch 响应中透传扩展字段。Python 对缺失扩展字段提供 `None`，Node／JSON 省略字段；HTTP 查询解析拒绝保持 400 和原兼容类别。共享样例覆盖 f32／f16／u64 绑定、Unicode 与换行、读取与写语句拒绝及关闭重开后的完整内容。

- AST 构造来源树覆盖查询入口、模式、过滤、全部 PipelineStage、表达式与谓词、返回／聚合／排序／分页，以及写入源、动作、节点／边、赋值、向量和 JSON。Rust 新增配对的 `LocatedTql` 解析入口、只读 AST／来源视图和 `PreparedTql::from_located`，手工 AST 执行按持有的信息提供阶段与定位。
- 诊断增加 `plan` 和 `execute`。符号校验沿 AST 路径定位具体引用或冲突返回项，算子装配、计算、预算、投影与事务在实际检查点记录来源。融合算子提供参与逻辑节点的最小包围范围，内核生成的子查询沿原入口追溯。`QuerySource` 与字段／参数包装保留原始类别、cause、详情及写入结果，普通字段或参数可与查询范围同时出现。
- 空匹配集同样校验 SET、SET VECTOR、DELETE 的节点变量，GraphFirst RANK 校验绑定及节点类型。基础 SEARCH 将取消控制传入真实管线。修正 `DisableFusion` 测试 Hook 沿用融合消除标记、可能跳过 RANK 的问题，拆分路径执行原始逻辑阶段。共享样例验证三种 dtype、实际语言制品、两类 HTTP、原始 AST、取消、WAL 与重开事实。

> **接入提醒**　Rust 错误变体匹配使用 `underlying()`，公共诊断通过 `info()` 读取。查询阶段与写入结果分别判断，详见 [TQL 诊断](../tql-reference.md#结构化查询诊断)。

### 工程收口的参数、取消与传输契约

- 补齐属性索引、复合字段和 Leiden 参数的字段诊断，快照、迁移、关闭超时、FFI 路径、Console 字符串及取消 ID 使用相同的表示校验。Python 转换在请求借用内执行，Node 复用同步重入准入，原宿主异常继续传播。
- Python 事务缓冲的标量、向量和 JSON 成员拒绝附加字段及 validation/not_committed、transaction_consumed=false，此前操作保留，转换中取消在追加缓冲前拒绝。Node commitTransaction 增加 operations 成员路径，显式错误类型的 label／weight 按字段拒绝，省略字段保留原默认值。ConsoleSession 与 PreparedTql 句柄的已知拒绝附加对应字段，Node 公共加载器核对句柄来源，重入错误的写入字段复用 SDK 操作目录。
- 精确扫描在 Rayon 分片中观察取消，图算法、遍历及结构分析增加节点、边和迭代检查点。工作线程继承组合控制，受控拒绝归还工作区。现有低层同步辅助函数的返回类型保留，数据库和 TQL 使用可失败路径，WAL 之后的原子提交沿原协议完成。
- 嵌入 HTTP 增加可选 X-TDB-Request-ID，采用 ASCII JSON 字符串编码并在正文读取前保存身份，执行前核对正文 requestId。数据适配器自动发送身份头，支持 Unicode，早期拒绝附加原请求身份。
- Server 的队列满改用 TDB_BUSY 和 write_queue 资源，HTTP、缓存和服务类别采用规范码，legacyCode 保留旧类别。路径、方法、请求体及适配异常使用结构化响应，外层请求身份覆盖提取拒绝。带 write 的错误继续提供 retryable=false，调用方依照阶段、结果及消费状态处理。
- CLI 显式 JSON 格式覆盖命令解析、非 Core 命令错误、I/O 及配置警告。入口字段、重试决策、兼容处理和 SDK／Core 当前依赖同步记入公共文档。
- 清除遗留的 TUI／REPL 交互、着色、布局、显示截断及 WebUI 页面用例。CLI 继续验证命令与 JSON／CSV 数据契约，Server HTTP 套件直接执行数据、事务及传输验证。

### 类型化请求作用域与宿主桥接

- Python 和 Node 新增公开 `RequestContext`，分别通过 `with context` 和 `context.run` 将请求身份、取消及期限传递到普通类型化调用。ID 使用 1–128 个 UTF-8 字节，毫秒期限从构造时开始计算，0 由宿主取消管理。嵌套作用域保留外层控制，两端最多嵌套 32 层。
- Rust 提供同步 `RequestContext::run` 和 Reader／Writer 的 `with_request_context`。SDK 借用与 Core 作用域在创建线程退出，控制句柄可克隆传递。Python ContextVar 和 Node AsyncLocalStorage 隔离任务，原生批量查询显式将控制器传递到工作线程。
- 类型化事务在 SDK 借用及准入成功后消费缓冲，普通共享事务继承 WAL 前取消检查。直接 CRUD 和事务取得实际 WAL 锁后再次检查控制，覆盖等待锁及回执编码期间的取消。Console 执行安装组合后的 Core 作用域，TQL 与检索沿已接入的执行检查点检查。关闭、回滚及已登记的资源诊断使用自身的生命周期与借用规则，调用结果沿实际内核完成点返回。
- Console 自定义桥接接收冻结的请求元数据，创建时捕获宿主回调，并提供可等待的 drain 完成确认。false 返回 `TDB_BUSY/console_bridge_drain`，其他无效值返回 `TDB_CONSOLE_TRANSPORT`。卸载失败后维持停止准入并可重试清理。宿主原始值拒绝保留 cause，已分发写入保留未知响应结果。
- 共享独立样例覆盖三种 dtype、转换回调取消、事务缓冲、异步任务隔离、第二个查询工作线程的取消、嵌套边界、宿主原异常及完整重开事实。Python 存根、Node 声明、实际安装包及非 UI 桥接契约同步验收。

> **调度提醒**　控制器在实际检查点观察取消。宿主自行建立的线程、进程与外部任务显式传递身份及控制事件，并通过对应完成点确认排空和资源释放。

### 统一请求上下文与跨重开确认

- Core 定义 `RequestContext`，统一保存请求 ID、取消／期限控制和写入类别，通过 `sdk` 导出。Rust 共享句柄新增 `try_read_with_context`、`try_write_with_context` 和 `commit_tx_with_context`。准入拒绝保留事务 builder，消费后的预检取消明确返回 `validation/not_committed` 和 `transaction_consumed=true`，实际提交结果由存储内核提供。
- Console 先解析借用的协议信封和命令类别，取得宿主借用及内核准入后再解析完整命令。会话登记、取消、SDK 准入和事务 WAL 前检查共享同一控制器。Rust、Python、Node 及嵌入 HTTP 通过原有 Console JSON 入口复用该路径。
- 版本保护事务的完整身份、创建 ID 和提交版本与事实写入同一个 WAL 事务，完整提交封条生效后一起恢复，随后随检查点原子发布。保留的回执支持关闭、恢复及重开后的 `transactionStatus` 和原事务幂等重放。每个数据库最多保留 64 条、合计 8 MiB 回执，按先入先出淘汰。缺失或淘汰的事务返回 `unknown`，应用按持久化业务身份核对完整事实。
- 主文件格式升级至 v11，在唯一约束块后增加带长度和 CRC32 的回执块，WAL 升级至 v4。当前内核读取 v5–v11 主文件，可写打开按原有路径升级历史 v2 WAL。v3 WAL 先流式验证，再原子转换完整事务前缀并丢弃物理截断尾帧。空的合法 v3 WAL 支持只读打开，v2 WAL 继续由可写恢复处理。
- 新增受控提交前取消、事务缓冲消费、同封条恢复、未封条事务不可见及旧制品来源样本。三种 dtype 的独立 v10／v3 样本验证只读零写、升级、完整节点／向量／Payload／边和再次重开；样本来源与哈希保存在测试 fixture 的 provenance 中。

> **持久化提醒**　回执的耐久性遵循 SyncMode。重新打开会创建新的编辑版本身份，保留的原回执确认历史提交，后续编辑使用当前快照。切换到 v11／v4 前保留完整文件组备份，旧内核环境通过升级前备份恢复。

### 请求执行与传输完成点

- ConsoleSession 新增 Rust／Python `is_drained()` 和 Node `isDrained()`，在 detach 已关闭准入且全部 Core 请求退出时返回 true。嵌入 HTTP 新增 `GET /v1/console/status`，返回相同的 drained 状态。Core 已解析写命令的会话、能力与权限拒绝补齐 admission/not_committed，保留原错误类别和详情。
- 修复 Server 写请求在出队筛选后等待写锁或同批前序操作期间发生取消、到期，仍被执行的问题。取得实际写锁后逐条核对响应接收端和期限，跳过已经退出的请求，到期请求返回 admission/not_committed，同批有效操作继续提交。
- Console 数据适配器将连接、解码、响应身份与回执结构失败按 TDB_CONSOLE_TRANSPORT 返回，保留原异常 cause。已分发写请求与原事务确认失败提供 response/unknown，本地排队取消提供 TDB_QUERY_CANCELLED、AbortError 和 admission/not_committed。HTTP 错误保留实际状态、内核诊断及既有写入结果，活动请求的 AbortSignal 传入 fetch。
- 请求身份与命令内容使用排队前的序列化快照。取消调用按活动请求去重，dispose 等待 detach 回调、请求队列和已发出的取消调用。Core 会话排空、传输任务结束和数据库关闭按各自完成点确认。受控验收覆盖真实写入后的响应损坏、完整原事务确认、WAL／重开事实、锁内迟到拒绝，以及各语言实际制品和类型消费。

### 嵌入式 Console 编辑冲突与事务确认

- Console RPC v1 增加 `getNodes(withRevision:true)`、`guardedTransaction` 和 `transactionStatus`，Rust、Python、Node 共用现有 JSON 入口。快照版本与节点内容在同一读锁下取得，版本前置条件在 Core 事务写锁内、WAL 前校验，冲突拒绝整笔事务。
- 编辑版本由 Core 事实变化驱动，宿主直接 CRUD、CAS、TQL、普通事务、向量和边更新都会使旧版本失效，删除重建会产生新令牌。版本覆盖整个数据库实例，因此不同节点的修改也可能冲突。
- 受保护事务按幂等键、原版本和完整操作匹配有界持久回执，重试命中回执后直接返回原结果。确认接口校验完整原事务。WAL 提交阶段消耗原版本；成功回执缺失时保留未知状态。
- 回执按先入先出保留至多 64 条、记录内容合计至多 8 MiB，回执队列、记录和响应缓冲均受内存准入约束，关闭释放相应内存。保留回执通过 WAL 与检查点持久化，重开后仍可确认原事务，缺失或淘汰后需要按应用身份对账。
- 嵌入 HTTP 适配器将版本冲突、幂等键错配和结果未知映射为 409，保留结构化 Core 错误。原有无条件写入口保持原语义；HTML 的嵌入模式通过内置客户端接入版本保护事务。接入和可执行请求示例见[编辑协议](../embedded-console.md#编辑快照冲突与原事务确认)。

### 嵌入式 WebUI 组件接入

- 新增 `console-client.js` 和 `console-embed.js`，通过同源 iframe 挂载现有 HTML，支持宿主 HTTP 或 IPC 桥接。页面查询、节点/边、索引、向量与图操作接到 Console RPC v1；独立 Server 保留原 HTTP 模式。
- 文档编辑、删除和导入使用审阅队列，捕获版本并冻结完整事务；同批次基线不一致时要求重新读取和审阅。提交结果不明时只查询原事务回执，继续保留完整请求与后续草稿。Console 路径以十进制字符串传递完整 u64 节点 ID。
- 宿主使用稳定 `storageKey` 隔离各数据库的草稿、历史和偏好。卸载在未处理草稿或队列存在时返回错误；显式退出可先导出原事务。客户端停止准入、发送取消与 detach，并等待已准入桥接调用退出，数据库持久化及文件锁由宿主管理。
- 资源包加入客户端、挂载入口及各资源 SHA-256。Rust Server crate 导出对应脚本，文档提供 HTTP/IPC 接入和卸载示例。
- 资源打包命令使用 UTF-8 标准输出，支持 Windows Runner 的重定向日志和中文输出路径，完成提示按中英文双语输出。

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

### Pipeline benchmark provenance and soak resource validation

- Fixed `bench_pipeline_gate` stopping on the `property_origin` difference between scoring before and after property filtering. The benchmark validates each plan's expected property entry, then compares every remaining row field in its original order. Regressions cover complete analytical scores, provenance, membership, and an absent score versus an explicit zero score. Public documentation defines the field's physical-plan semantics.
- The five-minute bounded soak uses explicit kernel and Payload-cache budgets, verifies resource return after maintenance, checks complete records, and reopens after two persistence cycles. Initial, peak, and final RSS observations support working-set and memory-composition analysis. Failure paths release objects and clean up fixtures before propagating the failure.
- Added a `release-contracts` profile that inherits release optimization settings and disables LTO for Windows optimized fault contracts. CI executes allocator, targeted I/O, and publication-kill suites with this profile. Original release and toolchain diagnostics retain their respective configurations and results.

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

> **Scope**　Shared access uses the single-writer transaction model. Format changes appear in the unified request contexts and cross-reopen confirmation section. Other wrappers and remaining public entries follow the phased contract. Poisoned host locks require releasing related handles and checking recovery. Shutdown timeout ends the current wait. See the [kernel SDK guide](../kernel-sdk.md) for usage.

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

> **Validation scope** Independent complete-fact fixtures cover three dtypes, Rom and Mmap, WAL synchronization faults, application faults, rejection of later writes, file-group moves, reopening, and another persistence cycle. Language argument extraction and external transport confirmation remain phased work. Durability follows the synchronization mode, and grouped results depend on the final group completion point.

### Text and index maintenance completion points

- Text registration, keywords, warmup, full rebuilding, property-index creation and removal, explicit Rust QuIVer construction, flush, and compaction provide write outcomes. Maintenance admission checks permissions, lifecycle, and WAL writability before changing text or index definitions.
- Adds `phase=publish`, recorded by the storage kernel at the file-group publication boundary. Publication failures before the commit marker completes report `unknown`, while later WAL cleanup failures report `committed`. Unique-constraint candidate rejection preserves the previous definitions. Publication failure quarantines the instance, and reopening restores constraints and complete facts according to the actual commit point.
- Text and keyword updates check instance budgets and reserve shared workspace before tokenization, conversion, and index growth. Budget rejection preserves previous text and WAL contents, and temporary reservations return on completion. Unique-index construction uses the existing ordered active-ID directory to produce deterministic existing and conflicting node identities.
- Node declarations add `publish`. Shared cases cover Rust, installed Python wheels, Node native modules, and installed npm tarballs, checking complete nodes, vectors, Payloads, edges, index definitions, text queries, read-only file groups, reopening, and another persistence cycle.

> **Entry-point scope** Rust and Node cover Rom and Mmap. The current Python constructor uses default Mmap. All three cover f32, f16, and u64. Explicit QuIVer construction, captured tasks, and runtime modes are existing Rust entry points.

### Background maintenance and independent destination publication

- Captured QuIVer tasks and explicit builds recheck WAL writability and recovery protection before installing a candidate, rejecting publication when the writer fails during construction. Execution or cancellation returns workspace and admission resources. Admitted tasks retain their existing file-lease lifecycle.
- Runtime QuIVer modes, automatic-build toggles, and periodic-compaction configuration provide write metadata and WAL admission. A replacement compaction registration is installed after creation succeeds, preserving the previous registration on configuration failure. Background `last_failure` adds canonical `code`, `details`, and optional `write`, with compaction reporting its actual publication progress.
- The outermost `write` on snapshot creation, restoration, manifest publication, and dimension migration describes the destination. Source-checkpoint or staging failures before destination publication report `validation/not_committed` and retain the original error chain. Snapshot directory synchronization failure after rename reports `publish/unknown`.
- Migration reserves the final database quota before exposing the destination, preventing another instance from taking that capacity between commit and return. Completed pre-commit rollback reports `publish/not_committed`, synchronization failure after marker rename reports `publish/unknown`, and cleanup failure after durable commit reports `publish/committed`. Snapshot and manifest operations validate `generation_id` before checkpointing the source.
- Acceptance extends existing maintenance contracts and fault suites with controlled build interleavings, real background compaction, destination-quota contention, complete recovered facts, and another persistence cycle. Public declarations and actual Python and Node artifacts are checked together.

### Management entry points and ordinary field diagnostics

- Adds `target.field` for covered ordinary API inputs while retaining the original category, details, and cause. Existing write outcomes survive field and Prepared-parameter wrappers, preserving facts supplied by the actual execution point when outer admission adds context.
- Synchronization configuration, capacity reservation, and graph-index repair check WAL writability before changes. Memory-quota rejection reports its phase and field while preserving the prior configuration and shared registration. Rust uses `bytes`; Python and Node use `mb` for their MiB interfaces. ReadOnly and Immutable instances can adjust their runtime quotas.
- Checkpoint failure after in-memory graph repair quarantines the instance and reports its actual `apply` or `publish` phase. Recovery proceeds by releasing relevant handles and reopening. Existing QuIVer build handles read the current quota when capturing later tasks, fixing stale limits after runtime quota changes.
- Runtime synchronization-mode string errors and covered explicit management conversions in Python and Node provide canonical fields and `validation/not_committed`. Python retains `OverflowError` inheritance for MiB conversion overflow. SDK callback reentry for synchronization, quota, capacity, and graph repair provides `admission/not_committed`.
- Shared maintenance contracts cover 28 Rust entry points and 23 exposed Python and Node entry points, adding real callbacks, quota rollback, read-only quotas and file groups, graph-repair faults, reopened contents, and type consumption. Other automatic language extraction, complete AST locations, and outer transport remain phased work.

### Management argument extraction and Prepared mappings

- Python capacity reservation, MiB quotas, automatic compaction intervals, and automatic-build switches extract arguments after acquiring the database request borrow. Covered built-in type failures retain `TypeError` or `OverflowError` inheritance and add `TDB_INVALID_INPUT`, `target.field`, and `validation/not_committed`. Custom integer-protocol exceptions retain their original identity, cause, and existing diagnostics.
- Python Prepared execution now implements its declared string-keyed `Mapping` contract, including `MappingProxyType` and `UserDict`. Container and key rejection uses `target.field=parameters`. Conversion failures for named values use `stage=bind` and the original parameter name. Mapping-protocol host exceptions propagate unchanged, and same-database conversion callback reentry follows the public admission contract.
- Node management scalars validate their original numeric or boolean types. Covered type rejection includes canonical fields and write phases. Automatic-build and compaction callback reentry provides `admission/not_committed`. Converted non-object Prepared containers use `target.field=parameters`. Public types, Python defaults, and the default behavior of Node's optional interval retain their established contracts.
- Actual binding cases cover three dtypes, host conversion exceptions and causes, mandatory callbacks, WAL and complete data after rejection, subsequent valid requests, and another persistence cycle. Framework extraction and JSON conversion in other entry points, together with complete AST locations, remain phased work.

### Raw Prepared scalars and management strings

- Node Prepared reads original arguments inside the method, providing `TDB_INVALID_INPUT`, `stage=bind`, and the literal `target.parameter` for NaN, infinities, undefined, BigInt, Symbol, functions, and compound parameter values. Invalid outer containers provide `target.field=parameters`. Parameter keys retain NUL and Unicode characters.
- Arrays and objects are rejected at the scalar boundary, including cyclic compound values. Host exceptions from property enumeration and getters retain their original identity, cause, and existing error fields. Valid scalars reuse the existing numeric conversion, and property enumeration retains its established rules.
- Python and Node add structured string-type rejection for synchronization modes and generation identifiers. Python extracts strings after acquiring the request borrow, retaining original exception types and canonical fields for covered failures. Node strictly checks original string types. Manifest callback reentry provides `admission/not_committed`, while publication outcomes continue to follow Core completion points.
- Actual artifacts cover three dtypes, valid scalar boundaries, getter and Proxy exceptions, callback rejection, valid queries after errors, WAL contents, and complete reopened data. Other N-API and PyO3 arguments, other JSON entry points, AST locations, and external transport remain phased work.

### Query arguments and Filter mappings

- Python integer count and expansion fields in ordinary and grouped search, together with count and parallelism fields in batch search, are extracted after the request borrow is acquired. Covered built-in failures retain `TypeError` or `OverflowError` and field locations. Host integer-protocol exceptions retain their original identity and cause. Corresponding Node entry points add original numeric-type and safe-integer rejection while preserving established defaults and optional null behavior.
- The three TQL query entry points add `target.field=query` for string-type rejection. These failures precede parsing and omit the stage. Read failures omit `write`, mutation-query type failures provide `validation/not_committed`, and SDK reentry provides `admission/not_committed`. Python encoding failures retain their original exception type.
- Python ordinary, grouped, contextual, advanced, and hybrid search now implement their declared outer `Mapping` Filter contract, including `MappingProxyType` and `UserDict`. Conversion runs within the request borrow, and host mapping exceptions propagate unchanged. Covered Python Filter representation failures and Core semantic rejection in both languages provide canonical categories and the corresponding field while reusing Core filtering rules.
- Actual binding cases verify rejection across three dtypes, real conversion callbacks, valid default calls, original host exceptions, complete matching and excluded result sets, WAL contents, subsequent valid writes, and complete reopened data. Actual npm installation explicitly awaits asynchronous batch-query acceptance, and type consumers retain the public argument boundaries.

### Query cancellation causes and deadline transport

- Core retains `TDB_QUERY_CANCELLED` for explicit cancellation and adds `QueryDeadlineExceeded` with `TDB_QUERY_DEADLINE_EXCEEDED` for expired deadlines, using empty details. When both conditions are observed at one cooperative checkpoint, the deadline takes precedence. Clones share the cancellation signal, and external and Console-session deadlines use the earlier bound.
- Standalone Server deadline rejection uses the canonical deadline code and `legacyCode=REQUEST_TIMEOUT`, retaining HTTP 504 and timeout metrics. Known write outcomes retain their original phase and `retryable=false`. Embedded Console request-body deadlines use HTTP 408, execution deadlines use HTTP 504, and explicit cancellation retains HTTP 408. Callers that previously handled execution deadlines as cancellation must handle the new code, and exhaustive Rust matches must add the new variant.
- Console write requests terminated before dispatch provide `admission/not_committed`. Transactions, guarded transactions, node imports, and edge imports terminated after preparation but before WAL submission provide `validation/not_committed`. Prepared receipt and response workspaces are returned when the request exits. Operations that enter WAL retain their atomic commit completion contract.
- Cases use already-expired controls, real pre-commit barriers, and incomplete external request bodies to verify causes, reuse of request IDs, WAL and complete facts, receipt workspaces, actual asynchronous exit, and file release. Coverage includes three dtypes, Rom and Mmap, HTTP permit return, and subsequent valid writes. Python error mapping, actual bindings, and the npm type catalog include the new category.

### Extended retrieval arguments and anchor sequences

- Covered integer arguments in exact, graph-first, contextual, advanced, and hybrid retrieval add structured type rejection and field locations. Python extracts integers after acquiring the request borrow and retains original type and host-protocol exceptions. Node reuses safe-integer validation and established positivity rules. Public defaults and type consumption are verified together.
- Python graph-first retrieval extracts anchor sequences inside the request scope. Same-database writes from sequence access and element integer callbacks return reentry errors. Hybrid text extraction adds `query_text` or `queryText` locations, and read failures omit write outcomes.
- Covered count fields in Node contextual and advanced configuration provide canonical type diagnostics. Omitted or undefined fields use defaults, while null retains the established rejection boundary. Optional method arguments continue to support null defaults. Original configuration-getter exceptions and causes propagate directly.
- Actual binding cases across three dtypes verify field rejection, integer, sequence and configuration callbacks, complete outer results, original host exceptions, valid defaults, and subsequent writes. Tests retain old objects after close, move the Windows file group, and reopen to verify updated vectors, payloads, and edges. Vector and nested-member locations and framework sequence length-hint policies remain separate contract work.

### Graph arguments and direct CRUD admission outcomes

- Neighbor, reachability, detailed reachability, and subgraph reads add field diagnostics for IDs, depths, budgets, and directions. Python converts parameters and label sequences inside the request borrow while retaining original type exceptions, ValueError for direction enumeration rejection, and host-protocol exceptions. Node checks raw types for the covered fields and retains original configuration-getter exceptions and established null default rules.
- Fifteen direct CRUD entries provide admission/not_committed when the SDK rejects reentry or borrow contention, with the consumption field omitted. Coverage includes node and edge writes, both batch insertion forms, atomic batch deletion, payload patches and conditional replacement, and vector updates. Transaction, WAL, and publication outcomes after core entry remain determined by the core.
- Public Node declarations include the existing deleteManyAtomic and compareAndSetPayloadField runtime methods, with precise return-type and invalid-input consumption cases.
- An independent three-node chain covers three dtypes, complete paths, per-hop attributes, outgoing and bidirectional scan counts, defaults, callback rejection, and unchanged WAL contents. Tests retain old objects after close, move the Windows file group, reopen to verify all facts, then perform conditional updates and atomic deletion and reopen again. Nested members, framework length-hint exception policies, and other entries continue under their respective contracts.

### Basic read arguments and result ownership

- Node, payload, existence, outgoing-edge, incoming-edge, and exact-edge reads add ID and label extraction diagnostics through id, src, dst, and label fields. Python executes integer protocols inside the read-request borrow, retains built-in exception inheritance, and propagates original custom integer-protocol exceptions. Node checks raw types and safe integers.
- The Python existence guide uses the public id in db entry. None, null, and omitted labels select all incoming labels, while an empty label is matched by its contents. Exact-edge reads require the label argument. Unicode and NUL labels remain intact.
- Actual artifacts cover three dtypes, complete nodes and edges, absent nodes, language integer ranges, callback reentry, and Python cross-thread shared reads with competing writes. Tests retain nodes, vectors, payloads, edges, and old database objects during close. After moving the file group, old results remain independent snapshots and updated facts survive two reopenings.

### Public call shapes and framework diagnostics

- The Python package validates argument structure against native public signatures for instance methods, selected special methods, and module functions. Missing arguments, unknown keywords, duplicate values, positional-only arguments, and excessive positional counts retain TypeError inheritance and expose TDB_INVALID_INPUT, target.field, and details.argument_kind. Calls with multiple issues use a fixed structural ordering. Python target fields preserve original Unicode, NUL, and isolated-surrogate keyword names.
- The Node public loader identifies registered missing required positions before N-API conversion. Explicit undefined continues through existing type checks, while extra positional arguments and configuration objects follow their entry-specific rules. Callback reentry retains rejection priority.
- The SDK constructs validation/not_committed results for covered write calls, with transaction_consumed=false for Python transactions and Node commitTransaction. Valid Python calls continue into the native binding, preserving original host exception objects, causes, class identity, signatures, and defaults.
- Actual artifact tests across three dtypes check diagnostics, zero conversion callbacks, complete facts, and unchanged WAL bytes, followed by a real commit, close while retaining old results, Windows file-group movement, and two reopenings. Constructor diagnostics, invalid receivers, and interpreter expansion errors before callable entry remain separate follow-up boundaries.

### Constructor values and resource return after failure

- Covered integer, string, enum, and MiB conversions in database and resource-runtime constructors expose TDB_INVALID_INPUT and target.field. Node also covers database boolean configuration. Python retains built-in exception inheritance and original custom integer-protocol exceptions and causes. Node retains original configuration-getter exceptions. Session configuration type and JSON rejection identify options_json or optionsJson.
- Fixed integer multiplication overflow while formatting an extreme-dimension rejection. The kernel rejects zero dimensions and dimensions above 65536 with the dim field, and identifies runtime worker-limit and maintenance-budget conflicts. Registered tasks and quotas return through ownership after open failures.
- Independent records cover three dtypes, duplicate writers, quota return, Windows file movement while retaining old results and error objects, same-path retry after stored-identity mismatch, and two persistence reopenings. Python uses its current Mmap constructor API, while Node covers Rom and Mmap. Python public stubs now specify int for cache MiB arguments, with positive and negative type consumption.
- This section records constructor values. Constructor call shapes and ordinary CRUD convergence are described below.

### Configuration containers and runtime-handle diagnostics

- Node database and resource-runtime options are checked as non-array objects before reading properties. Resource-runtime calls that previously treated primitive values or arrays as default configuration now return TDB_INVALID_INPUT with the options field. Omitted, undefined, and null options retain default configuration. Named properties follow the existing prototype-chain lookup, and unknown properties follow their entry-specific rules.
- Legacy numeric-position database construction now provides TDB_API_MIGRATION_REQUIRED, migration details, and the options field. npm declarations add the existing null default forms for configuration and snapshot-module runtime arguments. The runtime member inside database options continues to require a valid handle.
- Python database built-in boolean extraction and native runtime type rejection add field diagnostics. Runtime validation serves database construction, snapshot verification, and restoration in both bindings. Known restoration-handle rejection provides validation/not_committed. Node getter and Proxy exceptions and Python host conversion exceptions preserve their original objects, causes, and existing attributes.
- Actual artifacts cover invalid configuration, original exceptions, complete default-dimension records, prototype-free configuration, inherited getters, unknown properties, and WAL integrity while retaining a writer. Tests then create, verify, and restore real snapshots, reopen to check complete nodes and quota state, and perform close, file movement, and subsequent persistence.

### Constructor call shapes and ordinary CRUD conversion

- Python TriviumDB, ResourceRuntime, and ConsoleSession bind arguments against SDK public-signature metadata inside the native constructor entry. Missing arguments, unknown keywords, duplicate values, and excess positional arguments provide TypeError, TDB_INVALID_INPUT, target.field, and details.argument_kind. The same metadata provides __signature__ for Python 3.9 compatibility, with precise generated stub types. Batch-insert vectors/payloads outer declarations are corrected to list to match their existing native representation. ID collections and individual vectors retain Sequence. Shape rejection precedes value protocols, file creation, and resource registration. Native class identity, signatures, and defaults are preserved.
- Node database construction supplies the canonical missing-path diagnostic. Constructor forwarding preserves prototype, constructor, instanceof, subclasses, and newTarget. An explicit undefined path follows type rejection, and extra positional arguments follow the existing JavaScript call rules.
- Known language-type, representation-range, and JSON conversion failures in fifteen ordinary CRUD entry points provide validation/not_committed and omit transaction consumption. The SDK removes internal temporary-transaction consumption from batch-insert errors while preserving kernel phases, outcomes, and causes. Node automatic-ID safe-integer rejection adds its pre-commit outcome. Top-level target.field retains the parameter name. Batch and JSON members use RFC 6901 JSON Pointer with array indexes, escaped keys, Unicode, and NUL preserved. Object keys that cannot be represented as JSON strings identify their containing object.
- Python converts IDs, vector components, edge weights, and JSON while holding the request borrow. Existing integer, floating-point, and Sequence protocols remain accepted. Length-hint exceptions retain their existing tolerance, and buffers grow from actual elements. Batch conversion fixes its outer index range. If a callback shortens a list and removes a required member, the entire batch is rejected with that member's location. Appended members are left for subsequent calls. Original integer, float, iterator, getitem, getter, and Proxy exceptions retain identity, causes, and existing attributes. Same-database callback reentry follows request admission.
- Node constructor strings, ordinary CRUD strings, and JSON strings use strict UTF-16 decoding. Isolated surrogates are rejected as input errors, preventing replacement characters from changing paths, labels, or JSON keys. Valid Unicode and NUL are preserved. Optional edge arguments retain each language's default rules, and Node upsertEdge still requires weight. Removes obsolete Payload patch shorthand examples.
- An independent CRUD field matrix defines complete before-and-after facts. Actual binding contracts cover all three dtypes, Python's current Mmap entry, and Node Rom/Mmap. They verify content and WAL after field rejection, then use all fifteen write entry points, retain old objects across close, move Windows file groups, and reopen and persist twice. Interpreter unpacking before callable entry, invalid receivers, and JavaScript non-constructor calls follow host exception contracts. Retrieval, index, AST, and other wrapper work continues in their respective entry groups.

### Retrieval, graph-read, and text-entry fields

- Eight retrieval entry points add conversion diagnostics for floats, booleans, vectors, anchors, labels, and JSON members. Top-level fields use public parameter names, nested fields use JSON Pointer, and read errors omit write. Python conversion runs within the request borrow. Node preserves original getter exceptions, array reads, and existing synchronous/Promise completion, with row and component locations for batch u64 range failures.
- The kernel supplies precise fields for finite search configuration, diffusion-bias dimensions, and minimum-edge-weight constraints. Empty batches validate the same configuration before returning an empty result. Shared SDK conversion rejects NaN, Infinity, and arithmetic overflow in hybrid_alpha/hybridAlpha, fixing paths where max masked invalid input. Rust callers use underlying() to inspect original categories beneath field context.
- Four graph-read entry points add Node label-container, member, configuration-container, and direction diagnostics. Python graph labels and retrieval sequences retain existing length-hint tolerance, with buffers growing from actual elements. Node advanced/context search and reachability configuration require non-array objects. Omitted/null configuration retains defaults, positional null defaults and configuration-member rules remain distinct, and npm declarations reflect valid default forms.
- Indexed equality and substring lookup add object, JSON, limit, field, and text diagnostics. The kernel validates the 1–1000000 limit, empty equality conditions, and empty substring inputs. Text, keyword, build, and complete-rebuild entry points add write outcomes for known conversion and SDK-admission failures. Failed requests preserve existing text and index facts.
- Complete rebuild records contain id, text, and optional keywords. Missing members, types, encoding failures, and unknown fields provide record locations. Node now rejects previously ignored unknown rebuild fields under the closed-record contract. Text, label, field, and record strings in these Node entry points use strict UTF-16 decoding, rejecting isolated surrogates while preserving valid Unicode and NUL. Python accepts the declared Mapping input, captures this batch's record objects before materializing and converting fields, and continues with captured records when a callback modifies the outer list. Host mapping, numeric-protocol, and getter exceptions retain identity, causes, and existing attributes.
- An independent field matrix covers all three dtypes, Python's current Mmap entry, and Node Rom/Mmap. Cases verify complete scores/Payloads, member locations, original exceptions, thread-borrow contention, WAL after rejection, complete text/keyword and graph facts, file movement after close, and two reopenings. Public signatures, defaults, generated stubs, and positive/negative type consumption against actual artifacts are maintained together.

### Python transaction request isolation and collection

- Python transaction methods now acquire explicit fallible borrows. Reentry from an in-method conversion callback returns `TDB_REENTRANT_ACCESS` with `operation: "python_transaction_callback"`; cross-thread contention returns `TDB_BUSY` with `resource: "python_transaction"`. Rejection retains buffered operations, while other transactions follow their own admission state.
- Builder presence is the single source of transaction state, removing the `TxBuilder missing` panic paths. Adds `TDB_TRANSACTION_FINISHED` with empty details for buffering, committing, rolling back, or entering a context after consumption or rollback. Python exceptions retain their `RuntimeError` inheritance; the Rust catalog, Node declarations, and HTTP mappings are updated together.
- Context-manager commits use the same admission checks and retain the transaction when the database is busy or closed. Host conversion and context exceptions retain their original identity. GC now traverses the transaction's database reference, resolving resource retention in cycles formed by Hooks capturing transactions. A retained transaction continues to own its database; explicit close semantics remain unchanged.
- Contracts cover real conversion callbacks and event-controlled contention across f32/f16/u64, post-commit/rollback/failure states, retained ownership, directory moves after GC, and complete data after reopening. Python method signatures and normal return types remain unchanged.

> **Scope**: this batch covers Python transaction-object borrowing and finished state. Framework argument extraction, ordinary conversion errors, other wrapper objects, and general write-outcome fields remain phased work.

### Structured TQL parsing diagnostics

- Database queries, Prepared TQL preparation, and mutation parsing add `stage: "lex" | "parse"` and optional `target.query_span`, preserving `TDB_QUERY_PARSE`, `schema_version: 1`, and `details.reason`. Rust adds the `QuerySyntax` variant and structured parser entry points. The existing `QueryParse(String)` variant and standalone parser entry points remain available; Rust callers matching database parse errors must handle the new variant.
- Corrects locations for failed lexical units and consumed unexpected tokens. Confirmed positions use half-open UTF-8 byte ranges whose ends come from actual scanning. Whitespace and comments are separate from adjacent tokens. End-of-input and token-budget boundaries use zero-length anchors. Lines and columns are one-based, CRLF counts as one newline, columns count Unicode scalar values, and a tab counts as one column. Semantic checks without an accurate location and oversized input omit the location.
- Errors for database-prepared parameters carry both the parameter name and its first actual reference range, including the `$` prefix. Core retains the source and supplies locations for missing parameters, JSON conversion of referenced parameters, and vector-slot binding. Parsing distinguishes references from strings, comments, and document-filter operators. Extra parameters, standalone scalar conversion, and preparation from manually constructed ASTs retain their known parameter names. Prepared clones share source metadata. The Rust `QueryParameterSource` wrapper preserves the original category, details, cause, and known write outcomes, with `underlying()` exposing the original variant for matching.
- Python, Node, CLI JSON, embedded Console HTTP, and standalone Server preserve the same structure. The built-in Console data adapter forwards extension fields through native exceptions, JSON error envelopes, and fetch responses. Python uses `None` for absent extension fields; Node and JSON omit them. HTTP parsing rejection retains status 400 and the legacy category. Shared fixtures cover f32/f16/u64 bindings, Unicode and newlines, rejected reads and mutations, and complete data after close and reopen.

- AST construction origins cover entries, patterns, filters, every PipelineStage, expressions and predicates, return/aggregate/order/pagination nodes, and mutation sources, actions, nodes/edges, assignments, vectors, and JSON. Rust adds paired `LocatedTql` parser entry points, read-only AST/source views, and `PreparedTql::from_located`. Execution from manually constructed ASTs reports stages and locations according to its available provenance.
- Diagnostics add `plan` and `execute`. Symbol checks follow AST paths to the exact reference or conflicting return item. Operator construction, computation, budgets, projection, and transactions attach origins at their actual checkpoints. Fused operators use the smallest enclosing range of participating logical nodes, and generated subqueries retain their original entry origin. `QuerySource` and field/parameter wrappers preserve the underlying category, cause, details, and write outcomes. Field or parameter names can accompany a query range.
- SET, SET VECTOR, and DELETE validate node-variable bindings for empty match sets, and GraphFirst RANK validates bindings and node type. Basic SEARCH forwards cancellation control to its real pipeline. Fixes the `DisableFusion` test hook retaining fused elision markers and potentially skipping RANK. The split path executes the original logical stages. Shared cases verify three dtypes, real language artifacts, both HTTP forms, raw ASTs, cancellation, WAL contents, and complete data after reopening.

> **Integration note**　Rust variant matching uses `underlying()`, and `info()` exposes the public diagnostic. Query stages and write outcomes are evaluated separately. See [TQL diagnostics](../tql-reference.md#结构化查询诊断).

### Closing parameter, cancellation, and transport contracts

- Adds field diagnostics to property-index, composite-field, and Leiden arguments. Snapshot, migration, close-timeout, FFI-path, Console-string, and cancellation-ID inputs use the same representation checks. Python converts within request borrowing, Node reuses synchronous reentry admission, and original host exceptions propagate.
- Python transaction-buffer conversion failures include member paths and validation/not_committed with transaction_consumed=false, preserving earlier operations. Cancellation during conversion rejects the operation before appending to the buffer. Node commitTransaction reports operations member paths and rejects explicitly mistyped label/weight fields while retaining defaults for omitted fields. Known ConsoleSession and PreparedTql handle rejections identify the corresponding field. The public Node loader validates handle provenance and obtains reentry write metadata from the SDK operation catalog.
- Exact scans observe cancellation inside Rayon shards. Graph algorithms, traversal, and structural analysis check at node, edge, and iteration boundaries. Worker threads inherit combined controls and release workspaces on rejection. Existing low-level synchronous helper return types remain available, while Database and TQL use fallible execution. Transactions follow their atomic protocol after entering WAL.
- Embedded HTTP accepts X-TDB-Request-ID as an ASCII JSON string, preserving identity before reading the body and verifying the body's requestId before execution. The data adapter sends the header automatically, supports Unicode, and receives the original identity on early rejection.
- Server queue saturation uses TDB_BUSY with resource write_queue. HTTP, cache, and service failures use canonical codes with legacyCode for previous categories. Path, method, body, and adapter failures return structured envelopes with outer request identity. Errors carrying write retain retryable=false, leaving decisions to the caller's phase, outcome, and consumption handling.
- Explicit CLI JSON output covers argument parsing, non-Core command errors, I/O, and configuration warnings. Public documentation records entry fields, retry decisions, compatibility, and current SDK/Core dependencies.
- Removes legacy TUI/REPL interaction, highlighting, layout, display-truncation, and WebUI page cases. CLI acceptance covers commands and JSON/CSV data contracts, while the Server HTTP suite directly verifies data, transactions, and transport.

### Typed request scopes and host bridges

- Python and Node add public `RequestContext` classes. `with context` and `context.run` propagate request identity, cancellation, and deadlines into ordinary typed calls. IDs use 1–128 UTF-8 bytes. Millisecond deadlines start at construction, and zero delegates cancellation to the host. Nested scopes retain outer controls, with at most 32 user scopes in each binding.
- Rust adds synchronous `RequestContext::run` and Reader/Writer `with_request_context` methods. SDK borrow guards and Core scopes exit on their creating thread, while control handles can be cloned for workers. Python ContextVar and Node AsyncLocalStorage isolate tasks, and native batch queries explicitly carry their controller into worker threads.
- Typed transactions consume buffers after SDK borrowing and admission succeed. Ordinary shared transactions inherit the pre-WAL cancellation check. Direct CRUD and transactions recheck control after acquiring the actual WAL lock, covering cancellation during lock waiting and receipt encoding. Console execution installs the combined Core scope. TQL and retrieval check their integrated execution checkpoints. Close, rollback, and registered resource diagnostics use their own lifecycle and borrowing rules. Results follow actual kernel completion points.
- Custom Console bridges receive frozen request metadata and capture host callbacks at creation. Optional drain callbacks provide awaitable host completion. A false acknowledgement returns `TDB_BUSY` with resource `console_bridge_drain`, and other invalid acknowledgements return `TDB_CONSOLE_TRANSPORT`. Failed disposal keeps admission closed and permits cleanup retries. Primitive host rejections retain their cause, and dispatched writes retain unknown response outcomes.
- Independent shared cases cover three dtypes, cancellation from conversion callbacks, transaction buffers, asynchronous task isolation, cancellation in the second query's worker, nesting boundaries, original host exceptions, and complete reopened facts. Python stubs, Node declarations, installed packages, and non-UI bridge contracts are validated together.

> **Scheduling reminder**　Controllers observe cancellation at actual checkpoints. Host-created threads, processes, and external tasks explicitly carry identities and control events and confirm drain and resource release at their respective completion points.

### Unified request contexts and confirmation across reopening

- Core defines `RequestContext` with a request ID, shared cancellation/deadline control, and write classification, exported through `sdk`. Shared Rust handles add `try_read_with_context`, `try_write_with_context`, and `commit_tx_with_context`. Admission rejection preserves the transaction builder. Cancellation during validation after consumption returns `validation/not_committed` with `transaction_consumed=true`. The storage kernel supplies the actual commit outcome.
- Console first parses a borrowed protocol envelope and command classification, then decodes the complete command after acquiring the host borrow and kernel admission. Session registration, cancellation, SDK admission, and the final pre-WAL check share one controller. Existing Console JSON entry points in Rust, Python, Node, and embedded HTTP use this path.
- Guarded transactions store their complete identity, created IDs, and commit revision in the same WAL transaction as their facts. Recovery exposes both after a complete commit seal, and checkpoints publish them atomically. Retained receipts support `transactionStatus` and idempotent replay after close, recovery, and reopening. Each database retains at most 64 receipts totaling 8 MiB, with FIFO eviction. Missing or evicted receipts return `unknown`, requiring reconciliation against complete facts using application-persisted identities.
- The main-file format advances to v11, adding a length- and CRC32-protected receipt block after unique constraints, and WAL advances to v4. The current kernel reads main-file versions v5–v11. Writable opening retains the existing v2 WAL upgrade path. It validates v3 WAL as a stream, atomically converts the complete transaction prefix, and discards physically truncated tail frames. Valid empty v3 WAL supports read-only opening. v2 WAL continues through writable recovery.
- Controlled regression cases cover pre-WAL cancellation, transaction-buffer consumption, recovery under one commit seal, and invisible unsealed transactions. Independent v10/v3 fixtures produced by the previous artifact verify three dtypes, byte-preserving read-only access, upgrades, complete nodes/vectors/Payloads/edges, and subsequent reopening. Fixture provenance records the producer and hashes.

> **Persistence reminder**　Receipt durability follows SyncMode. Reopening creates a fresh edit-revision identity. Retained receipts confirm historical commits, and subsequent edits use the current snapshot. Preserve a complete file-group backup before switching to v11/v4. Earlier kernels use the pre-upgrade backup for recovery.

### Request execution and transport completion

- ConsoleSession adds `is_drained()` in Rust/Python and `isDrained()` in Node. It returns true after detach closes admission and every Core request exits. Embedded HTTP adds `GET /v1/console/status` with the same drained state. Session, capability, and permission rejection for parsed write commands now carry admission/not_committed while preserving the original category and details.
- Fixes Server write requests being executed after cancellation or expiry while waiting for the actual write lock or earlier operations in the same batch. Each operation rechecks its response receiver and deadline under the actual database write lock. Requests whose waiters exited are skipped, expired requests return admission/not_committed, and valid operations in the batch continue to commit.
- The Console data adapter reports connection, decoding, response identity, and receipt-shape failures as TDB_CONSOLE_TRANSPORT and preserves the original cause. Dispatched writes and failed original-transaction confirmation carry response/unknown. Local queued cancellation carries TDB_QUERY_CANCELLED, AbortError, and admission/not_committed. HTTP failures retain their actual status, kernel diagnostics, and existing write outcomes. Active request AbortSignals are forwarded to fetch.
- Request identity and command contents use the serialized snapshot taken before queueing. Cancellation calls are deduplicated per active request. Dispose waits for its detach callback, request queue, and issued cancellation calls. Core session drain, transport-task termination, and database close have separate completion points. Controlled validation covers damaged responses after real writes, complete original-transaction confirmation, WAL and reopened facts, late rejection under the write lock, and actual language artifacts and type consumption.

### Embedded Console edit conflicts and transaction confirmation

- Console RPC v1 adds `getNodes(withRevision:true)`, `guardedTransaction`, and `transactionStatus` through the existing JSON entry points shared by Rust, Python, and Node. Node data and its revision are captured under one read lock; Core validates the revision under the transaction write lock before WAL append, rejecting the entire transaction on conflict.
- Core fact changes advance the edit revision, including direct host CRUD, CAS, TQL, ordinary transactions, vectors, and edges. Deleting and recreating the same content produces a new token. Revisions cover the whole database instance, so changes to unrelated nodes can conflict.
- Guarded transactions match receipts by idempotency key, original revision, and complete operations, returning the original result when a retry matches a receipt. Status requests verify the complete original transaction. WAL submission consumes the original revision; a missing success receipt leaves the outcome unknown.
- Bounded receipts retain at most 64 entries and 8 MiB of record contents, evicting the oldest first. Receipt queues, records, and acknowledgement buffers are admitted against memory budgets, with memory released on close. Retained receipts persist through WAL and checkpoints and confirm original transactions after reopening. Missing or evicted receipts require reconciliation using application identities.
- The embedded HTTP adapter maps edit conflicts, key mismatches, and unknown outcomes to 409 while preserving structured Core errors. Existing unconditional writes retain their behavior. Embedded HTML uses its built-in client for guarded transactions. See the [editing protocol](../embedded-console.md#编辑快照冲突与原事务确认) for integration and request examples.

### Embedded WebUI component integration

- Adds `console-client.js` and `console-embed.js` to mount the existing HTML in a same-origin iframe through host HTTP or IPC bridges. Queries, nodes, edges, indexes, vector search, and graph operations use Console RPC v1. Standalone Server retains its HTTP mode.
- Document edits, deletion, and imports enter write review with captured revisions and frozen transaction identities. Mixed baselines require fresh reads and review. Unknown submissions only query the original receipt and retain the complete request plus later draft edits. Console node IDs use lossless decimal strings across the full u64 range.
- A stable host-provided `storageKey` isolates database drafts, history, and preferences. Disposal rejects unresolved drafts or queued work; hosts can export the original transaction before explicit exit. The client stops admission, requests cancellation and detach, and waits for admitted bridge calls. Hosts retain ownership of database persistence and file locks.
- Bundles include the client, mount entry, and SHA-256 hashes for each resource. The Rust Server crate exports both scripts; integration documentation includes HTTP/IPC mounting and disposal examples.
- The packaging command uses UTF-8 standard output for redirected Windows Runner logs and output paths containing Chinese characters. Its completion message includes both Chinese and English.

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
