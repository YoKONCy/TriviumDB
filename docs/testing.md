# TriviumDB 测试实践

> 本文说明 TriviumDB 的测试组织、契约设计、故障注入与验收证据。以 `0.9.0` 为基线，版本记录见[变更说明](changes/0.9.0.md)，后续加固见[未发布变更](changes/unreleased.md)。

---

## 目录

- [近期契约验收](#近期契约验收)
- [测试体系概览](#测试体系概览)
- [用例设计与验收证据](#用例设计与验收证据)
- [快速运行](#快速运行)
- [关闭与资源所有权回归](#关闭与资源所有权回归)
- [自动阈值与进程退出验收](#自动阈值与进程退出验收)
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
- [附录：依赖工具版本](#附录依赖工具版本)
- [向量存储开销回归](#向量存储开销回归)
- [文本索引生命周期](#文本索引生命周期)
- [写入边界与索引事实一致性](#写入边界与索引事实一致性)
- [类型、解析与加载边界回归](#类型解析与加载边界回归)
- [SDK 请求上下文与持久回执](#sdk-请求上下文与持久回执)

---

## 近期契约验收

`cargo test --features test-hooks --lib index::quiver::ssd -- --test-threads=1` 验证 SSD 单文件制品。独立 QVSSD v1／QUIV v2／QDSM v1 样例核对完整元数据、节点和分数，覆盖逐字节损坏、所有截断、重算 CRC 后的恶意字段、完整冷行交换、墓碑 slot 复用、只读字节、并发位置读取、旧 Reader 跨代读取、局部及共享配额、持有行后关闭和 Windows 文件释放。受控 Hook 覆盖读取前后取消、在途额度、发布前取消、写入／同步／重命名失败及目录同步后的未知完成点。提交点强杀父用例显式运行隔离子进程，核对旧制品或完整新制品的查询内容，并再次发布和重新打开；子进程入口按专用环境运行。

`cargo test --features test-hooks --test core quiver_ssd -- --test-threads=1` 验证数据库自动来源绑定、写入后的刷新、关闭 checkpoint、只读物理零写和 SDK 结构化拒绝。SSD lib 套件另验证缓存淘汰与继续持有行、零缓存在途合并、Hot／SSD 往返、预热预算失败重试、真实严格 Direct I/O 尾记录与 Windows 文件释放。严格 Direct I/O 用例要求本次运行环境提供对应后端，能力缺失按失败报告。Python／Node 公共 API 和实际 npm 制品使用 `tests/contracts/quiver_ssd_cases.json` 的独立方向样例核对完整 ID、分数、缓存与重开结果。

`cargo test --lib index::quiver::cold` 检查 SSD 编码基底。独立小端字节与 CRC 样例核对完整头、编码行和重建内容，已知重建向量的余弦定义独立验证整数评分。覆盖动态维度、尾组偏置、查询 ties-even、f32／f16、零及极端有限幅值、参数拒绝时的输出缓冲、编号溢出、逐字节损坏、截断和重算 CRC 后的非法字段。

`tests/contracts/named_graph_cases.json`维护命名图的共享成员与跨图同键独立边样例。Core及实际Python／Node制品核对完整成员和边、显式联合、批量编辑、修订冲突及跨重开数据。内核验收覆盖混合节点／图事务、全库删除级联、受控取消、WAL应用故障、检查点发布后日志重叠恢复、只读文件组字节及Windows文件释放。历史v11检查点和v4日志具有三种dtype独立fixture，格式用例验证截断、逐字节损坏与恶意计数。具体执行结果以本次平台记录为准。

`tests/contracts/document_cases.json` 提供含 CRLF、Unicode、结构目录及超过 JavaScript 安全整数范围 ID 的独立文档样例。Core 核对三种 dtype、Rom／Mmap、完整正文／范围／向量／边、修订保护、只读零写、取消、预算、节点争用、增量更新后的严格索引重开和 WAL 同步失败恢复。Python／Node 公共 API 套件复用该样例，类型消费验证字符串请求与返回值。验收入口和数据契约见[文档来源与结构](documents.md)。

`tests/contracts/document_retrieval_cases.json` 提供多来源、重复高分片段、空表示和不同清单／字节顺序的独立检索样例。Core 按 Okapi 定义独立代入小语料验证分数、完整分组、稳定排序、过滤后的全局统计、冷热缓存预算、更新失效、修订保护、混合粒度邻居和纯目录祖先。恢复用例注入真实 WAL 同步故障，再核对重开和再次持久化后的检索正文及证据修订。两端实际绑定复用独立样例核对 JSON 协议、缓存开关、上下文和跨重开结果。

缓存清理的 `test-hooks` 用例在清理与账本重置之间暂停真实请求，检查互斥锁仍被持有，并核对完成后的零额度、重新检索的完整内容及缓存重新登记。大量纯目录块的用例核对输出计量覆盖 JSON 数组分隔符。

`tests/contracts/document_positions_cases.json` 独立指定短语、累计邻近距离、重复词、标点、跨行／块边界、大小写宽度变化和中文重叠词项的完整原文证据。Core 覆盖三种 dtype 与 Rom／Mmap，两端实际绑定和 npm 安装制品复用同一组样例。小语料差分用例穷举递增下标组合，独立核对每个起点的最短窗口及词序，来源分组用例验证位置条件先于截断。

位置预算用例覆盖缓存命中／重建、后继表分配前的重复词工作量准入及工作区归还。`test-hooks` 屏障在真实位置执行中触发取消，WAL 同步故障用例核对旧视图、完整新修订的重开恢复与再次持久化。批量行列定位使用独立 CRLF、Unicode 和 Tab 边界样例。

`tests/contracts/document_fields_cases.json` 独立指定字段语料数量、平均长度、BM25 加权分数、来源及组内顺序。根／片段过滤覆盖任意／全部成员、重复值、空查询、缺失／Null／标量、整数表示、来源交集及分组前筛选。`cargo test --features test-hooks --test core document_fields -- --test-threads=1` 验证三种 dtype、Rom／Mmap、Reader、缓存升级与冷热预算、只读文件组零写、关闭后持有证据和 Windows 文件移动。受控成员 posting 屏障验证取消与重试，WAL 同步故障核对旧视图、重开后的完整新事实及再次持久化。Python wheel、Node 原生模块及实际安装的 npm 包复用相同样例。

`tests/contracts/document_vector_cases.json` 独立指定向量及混合结果的来源、命中顺序、RRF 分数和两路排名，复用文档字段样例中的真实来源范围。`cargo test --features test-hooks --test core document_vectors -- --test-threads=1` 覆盖三种 dtype、Rom／Mmap、Reader、完整整数指纹、空正文、缓存开关、过滤与位置约束、只读零写及文件移动。集中于单一来源的高分片段验证 ANN 按组扩展、候选宽度上限与硬资源预算，f32／f16 同时执行二维和 384 维路径。受控向量评分与 ANN 导航 Hook 验证执行中取消和资源归还，WAL 同步故障覆盖旧向量视图、完整新修订恢复、索引重建及再次持久化。实际 Python wheel、Node 模块和 npm 安装包复用精确及混合共享样例，额外核对完整 u64 JSON 分量。

默认 SM 路由快照由独立 QDSM 样例及 QUIV v1 图布局验证，覆盖位平面、幅值、极端精算比例、墓碑、截断、CRC 和重算 CRC 后的非法字段。`tests/storage/quiver_open.rs` 通过真实数据库核对 f32／f16、Rom／Mmap、完整节点／边／分数、只读零写、配额归还、文件组移动及 WAL 更新后的再次持久化。校验读取记录用于确认单次打开对主文件与向量分别执行一次完整摘要，随后重开仍识别同长度内容损坏。数据库重开性能通过 `bench_open` 单独报告。

`tests/contracts/index_maintenance_cases.json` 定义维护准入、字段拒绝及文本查询的独立预期。Rust 覆盖 28 个入口，Python／Node 覆盖已开放的 23 个入口；三种 dtype 核对只读、关闭、预算拒绝、完整事实及后续持久化。Rust 与 Node 使用 Rom／Mmap，Python 使用当前构造接口的默认 Mmap。故障套件验证 WAL 关闭后的全部维护拒绝、唯一约束发布前后恢复、刷盘与压实的 `publish` 结果，并通过文件组移动及再次重开验证资源和数据。共享配额用例核对文本工作区拒绝、归还、释放其他实例后的真实重试及完整内容。

Python／Node 的维护用例补充管理整数与布尔提取、原始类型拒绝、默认参数、实际转换回调中的重入和失败后继续调用。Python 保留宿主协议异常对象与 cause，Node 核对错误类型拒绝过程中强制转换回调次数为零。Prepared 用例验证 Python 的 `MappingProxyType`／`UserDict`、映射回调、容器及具名值诊断，核对 WAL、完整重开内容和类型消费。

管理字符串用例验证同步模式和 generation 标识的类型拒绝、Python 字符串子类与编码异常、manifest 的回调／关闭／只读拒绝。Node Prepared 用例核对原始 NaN、无穷、undefined、BigInt、Symbol、函数及复合参数的具名诊断，保留 NUL／Unicode 键名、有效标量转换和既有属性枚举。循环复合值在标量边界拒绝，getter 和 Proxy 原异常按身份及 cause 验证，随后继续查询并核对 WAL 与重开数据。

`tests/python/query_arguments.py` 和 `tests/node/query_arguments.js` 验证检索整数、TQL 文本及 Filter 映射。整数协议回调和 Mapping 回调使用真实数据库借用，拒绝同库写入后继续验证外层完整结果。浮点相同向量以余弦 1 为预期，两段相同 u64 以 128 个匹配位为预期，普通检索用零深度隔离图扩散。用例核对原宿主异常、默认参数、过滤后的完整命中与排除集合、WAL、有效写入及重开内容。npm 制品入口显式等待异步查询契约完成，任务失败和缺少完成回执均导致验收失败。

同一模块扩展到精确、图优先、上下文、高级和混合检索。Python 验证锚点序列的实际取值与元素整数回调，Node 验证配置 getter 的重入及原异常，分别核对配置字段和方法实参的 null／省略边界。混合检索按其入口契约核对完整 ID／Payload 和有限分数，精确及隔离图扩散的其他入口核对独立分数。关闭后保留旧绑定对象，移动独立数据库目录，再重开核对真实更新后的向量、Payload 和边。

`tests/contracts/graph_arguments.json` 提供三节点链的独立事实、完整路径及出向／双向扫描量，Python／Node 的 `graph_arguments` 模块通过既有公共接口和实际 npm 包入口执行。用例覆盖四个图读取入口的字段、默认值、真实整数／标签序列／配置 getter 回调、宿主原异常和十五个 CRUD 入口的 SDK 拒绝结果。双向展开会在中间节点额外扫描回源边。拒绝后核对 WAL、路径、逐跳属性和子图，再实际更新向量、关闭并移动 Windows 文件组，重开核对全部记录。随后执行条件 Payload 更新与原子批量删除，再次重开核对持久化内容。Node 类型消费同时验证这两个已有运行接口的公开声明。

`read_arguments` 用例复用三节点事实并通过 `tests/contracts/read_arguments.json` 增加空标签和 Unicode／NUL 标签，验证六类基础读取的完整节点／向量／Payload／边、入边顺序、不存在节点和语言整数表示边界。Python 的真实整数回调必须执行，同线程读取／写入重入明确拒绝；Event 控制的跨线程读取继续返回完整内容，写争用返回 Busy。Node 核对原始类型拒绝过程中数值／字符串强制转换次数为零，并通过真实参数 getter 验证全部读取入口的重入拒绝。保留已返回的结果与旧数据库对象后关闭并移动目录，再核对旧结果快照和两次重开的完整持久化事实。

`tests/contracts/call_arguments.json` 独立列出基础读取、图、检索、TQL、CRUD、快照和关闭的缺参字段。Python／Node 的 `call_arguments` 模块通过公共契约及实际安装包执行，核对调用形状、错误继承、字段和执行前写入结果。Python 补充未知关键字、Unicode／NUL／孤立 surrogate 键、重复赋值、位置专用参数、额外位置、原宿主异常及事务拒绝后真实提交。Node 验证缺参与显式 undefined 的区别、额外位置参数和回调重入优先级。错误调用的值转换回调次数为零，WAL 保持原字节；随后提交独立事务、持有旧结果关闭、移动 Windows 文件组并两次重开核对完整记录。存根生成和 npm 类型正反消费覆盖公开表示。

`tests/contracts/constructor_arguments.json` 登记构造器的数量、字符串和运行时字段及独立保存记录。Python／Node 的 constructor_arguments 模块验证内置类型、范围、MiB 换算、枚举、会话配置、原宿主异常及 Node getter。共享运行时逐次核对失败后的活动任务、工作区、附加数据库和登记配额，再验证重复 writer 的持有者、Windows 文件组移动、身份不匹配后的重试、完整节点与两次持久化重开。Python 覆盖三种 dtype 的现有 Mmap 构造入口，Node 覆盖三种 dtype 的 Rom／Mmap。Rust boundary 用例以 usize::MAX 验证维度拒绝的诊断算术及同路径重试。

同一构造模块扩展 Python 内置布尔及 runtime 类型拒绝、Node 外层配置容器与旧数字迁移详情。正向覆盖 null 默认配置的完整 1536 维记录、无原型配置、继承 getter 与未知属性，反向验证数组在属性读取前拒绝。runtime 用例逐一经过数据库、快照校验与恢复入口，核对原生类型、无宿主属性转换、错误句柄、getter／Proxy 原异常及恢复的执行前结果。持有 writer 时拒绝无效构造并比较 WAL，随后实际创建／校验／恢复快照并重开核对完整节点及配额。类型正反消费同步可空默认形式。

构造模块同时覆盖 Python 三个构造器的缺参、重复赋值、未知原始关键字和额外位置，以及错误形状下的零次协议调用。Node 验证 path 缺参、严格 Unicode 解码和原生构造转发中的子类、prototype、constructor 与 instanceof，实际写入完整节点验证转发后的实例。

构造签名的独立预期同时约束参数顺序和默认值。Python 3.9 与当前解释器使用同一份实际 wheel 检查构造和普通 CRUD，核对原生类的公开签名及数值协议兼容。SDK 单元用例验证批量错误省略内部消费字段后仍保留 validation、wal、apply、publish 阶段、原提交结果和错误类别。

`tests/contracts/crud_arguments.json` 给出十五个普通写入口的字段矩阵及独立完整前后事实。两端 crud_arguments 模块逐字段验证类型、表示范围、批量和 JSON 成员路径、转义键、原宿主异常、同库回调拒绝及 WAL。Python 补充长度提示失败、巨大提示下的小序列与回调修改批量输入；Node 补充 getter、Proxy 和非法 Unicode。随后通过全部十五个入口执行有效写入，持有旧结果及错误关闭数据库，移动 Windows 文件组，两次重开核对全部 ID、向量、Payload 和边，并再次持久化。Python 使用当前 Mmap 入口，Node 覆盖 Rom／Mmap，均覆盖 f32／f16／u64。实际 npm tarball 与安装 wheel 使用相同契约模块。

`tests/contracts/retrieval_fields.json` 登记检索浮点／布尔字段、完整查询事实和文本重建记录拒绝路径。两端 retrieval_fields 模块覆盖八类检索及六类查找／文本入口，图模块补充四类遍历的标签成员、Node 配置容器、原始 getter 和 Python 长度提示策略。检索验证完整 ID、分数和 Payload、u64 批量的 Promise 拒绝位置、原宿主异常与 WAL，文本维护验证失败保留完整正文／关键词查询和节点，再通过 Mapping 或原始对象重建、持有结果关闭、移动文件组并两次重开核对完整事实。线程争用通过 Event 控制，原持有者继续执行。Python 使用三种 dtype 的 Mmap，Node 使用三种 dtype 的 Rom／Mmap，文本重开显式加载现有文本索引。

Core 的 retrieval_validation 补充配置非有限值在普通和空批次中的准确字段、原错误及拒绝后的完整恢复事实。SDK 单元用例以独立权重样例验证混合换算，并拒绝非有限输入及溢出。实际 wheel、npm tarball、存根和类型消费使用同一公共入口，覆盖 Python 3.9 的适配兼容。

`tests/core/query_cancellation.rs` 通过已取消、已过期及两种条件同时成立的控制器验证读取、Prepared 和 Console 分发，覆盖三种 dtype 的 Rom／Mmap、同请求 ID 重用、完整事实和资源账本。`tests/concurrency/console_cancellation.rs` 在真实 WAL 前屏障取消四类事务／导入，核对未提交阶段、WAL 字节、回执工作区、完整记录和后续有效写入，并在持有旧宿主时移动文件组及重开。HTTP 用不完成的请求体触发真实请求期限，再次取得唯一许可并恢复有效读取。该类用例验证核心期限语义，断言以返回类别、真实完成及资源状态为依据。

管理契约补充普通字段定位、同步模式和单位转换拒绝、SDK 重入、额度失败回退、只读额度调整与文件组字节比较。既有构建句柄的预算变化用例执行真实拒绝、释放和重试；图修复故障沿用完整事实 fixture，覆盖发布前后失败及二次持久化。错误包装用例核对字段／Prepared 上下文透传既有 WAL 结果。

请求完成点验收使用真实 Console 请求和受控 Hook 验证 detach 后仍有活动执行时的 `is_drained=false`，实际退出后才返回 true。Rust、Python 和 Node 复用 Core 状态，嵌入 HTTP 核对独立状态入口及 `no-store`。Server 在队列筛选后注入接收端退出和期限到期，确认实际写锁内逐条拒绝，并验证同批有效写入及两次重开完整事实。排队取消用真实队列接收和关闭排空确认完成。

Console 数据传输用例在真实事务提交后注入连接失败、损坏 JSON、错误响应身份和无效回执，核对 `response/unknown`、完整原事务确认和数据库事实。外部 fetch 边界验证原异常 cause、HTTP 状态、预分发取消与活动 AbortSignal。取消和卸载用受控 Promise 验证去重及排空，断言以明确的进入、释放与完成通知为依据。

后台构建使用通道和发布 Hook 分别控制执行前与发布前的 WAL 失效，核对拒绝字段、工作区归还和重开后的全部记录。周期压实通过真实登记派发和完成通知验证 I/O 故障。快照／迁移沿用现有故障套件，补齐独立目标结果、完整内容及再次持久化，迁移配额在发布屏障上验证。

当前未发布加固增加 `tests/contracts/error_cases.json`：Rust、已构建 Python wheel 与 Node 原生模块通过真实 API 核对错误码及完整字段，覆盖 Prepared 参数、维度、节点、唯一约束、只读、行预算、迁移、关闭、锁、I/O 与格式版本。Rust 描述符及 Python 映射测试使用独立的全部错误变体样例；Node 另验大整数详情与 Promise 拒绝。语言参数提取错误按各语言类型契约验证。

`tests/contracts/prepared_diagnostics.json` 为 Prepared 缺参、多参、JSON 容器值和向量参数类型提供独立诊断预期。Core、三种 dtype 的 Python／Node 实际包、独立 Server、嵌入 HTTP 与 Console 数据传输核对原始类别、`stage=bind`、参数原名和首个真实引用范围，覆盖注释、Unicode／CRLF／Tab、重复引用、文档过滤操作符同名，以及 WITH、WHERE、ORDER BY、聚合和嵌套表达式。适用入口核对拒绝后的正常查询、WAL 字节与重开完整事实。Rust 另覆盖 NaN 与正负无穷参数、独立标量转换及手工 AST 的已知信息、原查询释放与 Prepared 克隆继续持有时的关闭和文件移动。Python 核对原异常继承关系。

`tests/contracts/query_execution_diagnostics.json` 固定规划与执行错误的原始范围、阶段、类别、HTTP 状态及写入结果。Core、三种 dtype 的实际 wheel／npm 包、Console 与两类 HTTP 核对同一结构，验证拒绝后的正常绑定、WAL 字节和重开完整节点。`tests/query/source_locations.rs` 逐层核对表达式、谓词、聚合、分页、全部 PipelineStage 及写入构造的范围，验证合成来源、取消、原始 AST、空匹配集的写目标绑定，以及融合与拆分路径各自的来源范围。`DisableFusion` Hook 执行原始逻辑阶段，拆分 RANK 使用真实算子完成验证；差分套件继续核对完整结果等价性。

`tests/contracts/write_outcomes.json` 指定写入阶段、结果和事务消费状态。Core 与语言实际制品核对准入保留、预检消费、只读拒绝、响应失败后的回执及重开事实；Rom／Mmap 故障用例覆盖 WAL、应用和恢复。Server 的 `test-hooks` 套件使用真实分组同步故障与受控 blocking 任务异常，验证请求逐一收到结果、独立校验错误保留、任务排空和重开完整内容。队列满与执行前过期用例核对 `not_committed` 及 HTTP 重试字段，CLI JSON 与 Console 数据适配验证实际错误出口。

`tests/contracts/direct_write_cases.json` 独立列出十类直接写入的完整前后事实。`fault_io::direct_write_outcomes` 在 f32／f16／u64、Rom／Mmap 下分别注入 WAL 同步故障和应用故障，核对写入状态、后续拒绝、WAL 字节、文件组移动、重开向量／Payload／边及再次持久化。已准入共享句柄的恢复隔离在内核单元用例验证；TQL 构造前拒绝通过各语言实际包、CLI 和 HTTP 核对。

嵌入式 Console 使用 `tests/contracts/console_cases.json` 的独立样例验证三端请求、完整记录及重开持久化。`cargo test --features test-hooks --test core console_host -- --test-threads=1` 覆盖真实子进程文件锁竞争、持有旧会话时释放锁、同路径新实例隔离、Hook 屏障下取消/关闭、只读文件组完整字节及关闭失败重试；标为 ignored 的探针由父用例以必需参数实际执行。Server 的 `console_embedding` 验证真实请求内容、客户端退出后的许可归还与宿主锁；Node 已安装 tarball 的消费测试同时验证 N-API 前重入拒绝。`python -m unittest discover -s tests/tools -p test_console_bundle.py` 验证实际 ZIP 的完整 HTML、协议、文档、许可证及可复现内容。

全部错误变体的 Python 映射测试由 Windows Python Binding 任务执行 `cargo test --lib --features python 核心错误完整映射到_python异常`，链接 Windows Python 运行时；跨平台 wheel 继续通过真实公开 API 用例验证交付行为。

`cargo test --features test-hooks --test concurrency opening -- --test-threads=1` 用真实文件移动和路径屏障覆盖打开预检与发布交错：持有租约时准确返回锁冲突且数据制品不变；发布完成后持锁重读完整已提交事实；持续缺失时保留 I/O 错误并归还探测租约；Unix 验证规范路径取得后的调用路径身份，以及符号链接不能误用别名协调文件。Node 工作线程退出回归继续严格要求锁冲突，并验证重开的完整节点内容；意外错误输出原始消息及结构化详情。

发布来源校验执行 `python -m unittest discover -s tests/tools -p test_release_source.py`，CI 编译检查任务显式运行。用例创建真实 Git 历史，网络边界使用独立 PR/CI 响应；验证普通祖先提交、错误仓库或分支、未合并 PR、错误 SHA、缺失/失败/取消 CI 的拒绝，以及 master 前进后的历史 Release 补发。两条 CD 直接运行同一个校验脚本。

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

---

## 用例设计与验收证据

资源账本断言使用显式创建的 `ResourceRuntime`，以用例为单位持有和释放。需要验证多实例共享额度时，在同一用例内显式复用该 Runtime，并在任务排空和对象释放后核对账本。并行用例各自的工作区、任务和配额具有独立身份。

`cargo test --test core tql_semantics` 验证普通 MATCH、FIND、WITH 与具名边的完整返回内容、分组、聚合、去重、别名排序和分页。`tests/contracts/tql_return_cases.json` 由 Rust、Python、Node 共同消费；Node 同时通过 `npm run test:package` 在独立安装的 tarball 上执行。DML 用例验证多字段合并、每行 CREATE 作用域、完整 u64 位模式、浮点转换边界、拒绝后的 WAL 内容及关闭重开。纯向量管线在分页后核对具体节点与 Payload 读取量。

`cargo test --test core tql_boundaries` 验证内联 ID、结构 ID 与 Payload 索引域、OPTIONAL 起点过滤、具名边谓词、实体空值、JSON 容器、重复变量、默认列名、Prepared 非法槽位与精确向量输入。固定数值样例覆盖整数/浮点等价表示、2^53 相邻整数、有符号边界及 u64 上限，并在 Hash、Ordered、Bitmap、Composite 配置中核对具体结果。`tests/contracts/tql_boundary_cases.json` 为三端提供固定输入和完整预期；Node 与 Python 的公共 API 入口还验证拒绝后的 WAL 保持、完整大 ID 更新、容器输出及安装制品上的 u64 SEARCH/RANK/Prepared 行为。

`cargo test --features test-hooks --test core tql_aliases -- --test-threads=1` 验证投影快照、多源汇合、连续扩展、同时赋值、标量/节点重命名、递归作用域、实体与对象别名排序，以及串行、并行和融合精排。`tests/contracts/tql_alias_cases.json` 在 Rust Rom/Mmap 与两端实际 SDK 中核对固定完整结果或明确的排序 ID。工作区测试分别检查实例上限与共享配额拒绝，并核对拒绝后配额归还和后续查询。

检索边界使用预先给定的 NodeId 次序验证槽位复用、分段扫描、并行度与 Top-K 截断；数值排序覆盖 `i64/u64` 端点、2^53 相邻整数、正负小数及索引重载。内存计量测试独立遍历实际容器，核对增量账本在替换、重复登记、删除、编译和重载后的值。事务应用故障测试在 WAL 提交后注入失败，要求当前句柄与已准入的共享搜索拒绝部分状态，并在重开后核对完整事务和再次持久化结果。

定向入口：`cargo test --test core search_boundaries`、`cargo test --lib --features test-hooks guard_tests`、`cargo test --test fault_io --features test-hooks wal_failure_state -- --test-threads=1`。QuIVer 调整的质量与耗时对照同时记录数据分布、种子、维度、候选宽度、精排预算、平台和构建参数。

预期结果依据公开契约、独立样例或独立参考模型给出。多物理计划对比验证执行策略的一致性，独立 oracle 验证查询语义；文件组前后快照验证零写与内容保持。每份测试报告注明提交、平台、特性、命令、实际断言及尚待执行的路径。

关键断言直接核对完整结果。节点结果校验 ID、Payload 和对应关系，持久化恢复要求已确认写入的具体记录存在且内容相同。Normal 模式的进程崩溃用例要求检查点和后续成功 WAL 写入全部恢复，逐条检查向量与完整 Payload。字段缺失、采样失败、子进程启动失败和制品缺失均使相关验收失败。测试边界数字说明其含义，mock 用于隔离外部服务或进程边界。

`pipeline_differential` 为十四类管线分别建立用例：可达集合由样例边关系计算，精排使用解析余弦，PageRank 解线性方程，标签传播和 SA-PPR 使用独立矩阵，路径用例核对枚举数量、强度和具体路径。完整 ID 集合与顺序先于每行分数检查，两个物理计划分别接受同一独立预期校验。

Node 类型验收有两个入口：`npm run test:types` 严格检查公共声明与消费样例；`npm run test:package` 将实际 npm tarball 安装到独立项目，编译正向调用与错误输入样例，再加载原生模块检查事务、Prepared TQL、向量分数、关键词关联和重新打开。制品验收同时注入一个未定义的公共类型，要求编译失败。普通 CI 使用制品入口，发布质量门禁先检查类型，npm 发布前再验收装配完成的制品。

定时浸泡使用一百条存活记录的有界负载，要求每次查询成功并返回存活节点的完整 Payload。五分钟用例配置 8 MiB 内核预算及其中的 256 KiB Payload 解析缓存，在维护完成点核对内存估算、缓存占用及工作区／任务归还，关闭后检查数据库配额与附着计数归零。最终逐项验证节点、向量、Payload 与边，再完成可写重开、再次持久化及只读重开；工作负载异常退出时先释放对象并清理文件，再传播原失败。文件用例执行五百轮维护并统计完整文件组，上限为基线的五倍。

RSS 记录初始、峰值和最终字节，作为独立的操作系统资源观测。Windows 采样进程读取 WorkingSet64，Linux 读取 VmRSS，其他平台使用 ps；观测必须有效。资源调查结合工作集、映射、分配器保留、缓存及内核对象存量解释增长。`unsafe_audit` 的向量文件用例先关闭数据库，再截断或删除向量文件，并保留提交标记，验证重新打开返回结构化损坏错误。

Windows 优化故障契约使用 `release-contracts` profile，继承 release 的优化设置并显式关闭 LTO。CI 在该配置下实际执行 allocator、定点 I/O 和发布强杀套件。复现入口如下。

```bash
cargo test --locked --profile release-contracts --features test-hooks --test fault_allocator --test fault_io --test fault_power -- --test-threads=1
```

定位 release 编译器崩溃时，对同一源码、工具链和锁文件分别记录原始 `--release` 与 `--profile release-contracts` 的完整编译及运行结果。Rust 工具链升级另建对照，保留每项成功或失败的实际范围。

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

嵌入式编辑协议的定向入口为 `cargo test --features test-hooks --test core console_editing -- --test-threads=1`。`console_editing_cases.json` 为 Rust、已安装 Python wheel 和 Node 原生/安装包消费测试提供独立内容预期；验证宿主写入后的旧快照拒绝、整笔事务的完整结果、原请求重试、错误键确认及重开边界。Core 额外验证业务写入口矩阵、并发编辑、无事实变化事务的版本消耗、回执淘汰、WAL/应用故障和提交开始后卸载会话。Server 的 `console_embedding` 验证相同语义及 HTTP 错误映射。所有检查均为非 UI 数据和生命周期契约。

以下检查随普通 Rust 测试以及三平台原生绑定 CI 执行：

```bash
cargo test --lib database::lifecycle::tests
cargo test --test core lifecycle_resources
cargo test --test contracts 关闭
cargo test --test fault_lock 硬链接 -- --test-threads=1
cargo test --test concurrency resources -- --test-threads=1
cargo test --features test-hooks --test fault_io lifecycle_resources -- --test-threads=1
python tests/python/test_resource_lifecycle.py
npm run test:resource-lifecycle
```

`tests/core/lifecycle_resources.rs` 覆盖 f32/f16/u64、Mmap/Rom、同步与异步关闭，保留关闭对象并重新打开、移动目录；Windows 使用独占文件打开验证数据库句柄已释放。另有 manifest 内容保持、索引变更持久化、关闭后读取拒绝及分组提交 panic 后强制进程退出的恢复检查。故障注入验证最终 WAL 同步失败保留数据、锁、Hook 与原始错误，并可重试。

普通析构回归保留独立查询句柄、构建句柄或已准备任务，验证新操作被拒绝、旧任务执行或取消前文件锁持续有效、最后一个句柄释放后映射解除，并逐条核对重开的向量和 Payload。Hook 析构异常分别覆盖同步和异步关闭的结构化错误、终态及同路径新实例隔离。预算与最终 WAL 同步失败用例验证同步关闭恢复周期维护，异步失败仍停止服务。Unix/Windows 硬链接用例核对拒绝打开时完整文件组零变更，以及原 Writer 确认写入的重开结果。

Server 关闭回归使用通道控制真实读取和关闭交错，覆盖写队列排空、取消请求后 blocking 任务继续持有租约、取消关闭等待者、打断合批等待、持久化失败重试，以及保留 Router 时文件资源释放。独立非 UI 入口见 [Server 验收](server.md#按需开发自测)。

Python 测试覆盖 `with` 的正常和异常退出、强引用、Hook 循环 GC 与动态库卸载。Node 通过 `--expose-gc` 验证清除、替换、关闭、对象回收和循环引用的 Hook 释放，并覆盖注册失败保留旧 Hook、冻结包装对象、`dispose`、`Symbol.dispose`、Worker 退出与 FFI 动态库释放。FFI 测试在临时目录使用 `rustc` 构建最小插件，绑定 CI 与 npm 发布校验均准备 Rust 工具链。Node 还以原生回调屏障固定占用任务名额，验证数据访问返回完整的 `QueueFull` 预算字段，且被拒绝的 Hook 注册保留原扩展。

Linux TSan 继续执行生命周期单元测试；Windows 本地结果对应原生文件句柄和语言 GC 验收，跨平台结果由 CI 提供。

---

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

---

## 测试分层架构

`tests/` 按领域 harness 组织：每个顶层目录是一个独立 integration target，通过各自 `main.rs` 汇聚同领域的测试文件。

默认 `cargo test` 包含 `storage::publication_recovery`：使用独立构造的 `TPUB` v1 日志验证旧事实 + WAL 完整恢复、Rom/Mmap 旧基础、二次持久化、损坏日志/备份拒绝、提交后不回滚和只读字节级零写。开启 `test-hooks` 后，发布强杀另覆盖 3 种写入场景 × 6 个中断阶段；其中更新/删除/compaction 使用 2048 节点、65 维，验证全部已同步数据与更新边。恢复成功要求重新打开并核对完整记录。该规模覆盖多个 mmap 页。

> **验收范围**：超内存工作集、物理断电和 PageCache 压力需要分别设计并记录专项验收。

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

> **故障注入提醒**：`fault_*` 系列使用小型 fixture，在隔离子进程中执行强杀、受控分配失败或硬件异常。部分异常日志来自预期退出的子进程，验收结果由父用例对退出状态和恢复数据的断言确定。

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
| `WAL`       | `unit/wal.rs`       | v4 显式头、v2 显式迁移、v3 兼容读取、无头拒绝、未来版本门禁、CRC、SyncMode、崩溃恢复 |
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
| `unsafe_audit.rs` | 安全边界回归 | 覆盖向量数值/尾部边界、VecPool 文件变更和畸形文件加载；其他 unsafe 路径需单独审核 |
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
| 3   | Filter 匹配结果         | 500        | `from_json` 成功解析的 Filter 对任意 payload 调用 `matches` 正常返回布尔结果 |
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

---

## ThreadSanitizer 与 Miri

`ci.yml` 为普通 PR/push 配置独立的 TSan 和 Miri 任务，与多平台测试、ASan 和代码审核共同构成验收流程。各项执行结果记录在对应提交的 CI 中。

| 检查 | 当前执行范围 | 边界 |
| --- | --- | --- |
| 默认测试 | 默认 feature 下的库与集成测试 | `test-hooks` 套件需显式启用 |
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

> **提醒**：属性测试的价值在于发现人类测试者难以想到的边界组合。例如：空向量的 self-similarity、全零向量的余弦计算、超长 insert/delete 交替序列后的状态一致性。

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

> **提醒**：变异测试非常耗时（每个变异体需要独立编译整个项目）。建议仅对高风险、高覆盖率的模块按需运行，不适合放入 CI 常规流程。

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

### 覆盖率报告

覆盖率报告记录提交、平台、启用特性、执行命令及总量和模块级指标。正式发布使用对应提交的 `cargo llvm-cov` 输出和 CI artifact。

> **结果解读**：行覆盖率记录执行到的代码行。分支行为、断言有效性和边界条件还需结合属性测试、变异测试及代码审查评估。

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

常规 CI 与嵌入式发布验收覆盖 Core、Rust/Python/Node SDK 和 CLI。Server 与 WebUI 采用 nightly 开发流程，Server 非 UI 契约独立执行。禁止新增或执行 UI/UX、样式、布局、视觉基线、动画及耗时阈值测试，不以冒烟、行为、诊断或手动入口绕过禁令，也不将其作为本地、CI、合并或发布门禁，见 [WebUI 验收边界](webui-testing.md)。

### 1. 核心 CI 管线 (`ci.yml`)
面向 master/dev 的 Push 或 PR 会运行多平台 Rust 测试、CLI 命令及数据契约、Python/Node 生命周期、短时 fuzz、ASan、TSan、Miri、覆盖率、类型存根和 Clippy 等门禁。另有内存压力观测，以及手动 mutation/benchmark Job。平台矩阵和 Job 数会随工具链演进，具体以 workflow 为准。

- **Compile Check**：默认、Python、Node 和 aarch64 编译检查
- **Rust Test**：Linux/Windows/macOS x86_64、macOS ARM64，并在 Ubuntu 24.04 Arm 原生 Runner 上补充 Linux ARM64
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

> **提醒**：变异测试不建议放入 CI 常规流程（单次运行可能超过 30 分钟）。CI 中已配置为手动触发（`workflow_dispatch`），作为版本发布前的质量门禁。

---

## 附录：依赖工具版本

| 工具             | 用途       | 安装方式                                |
| ---------------- | ---------- | --------------------------------------- |
| `proptest 1.11`  | 属性测试   | `Cargo.toml` dev-dependencies（已内置） |
| `cargo-llvm-cov` | 覆盖率度量 | `cargo install cargo-llvm-cov`          |
| `cargo-mutants`  | 变异测试   | `cargo install cargo-mutants`           |

---

## 向量存储开销回归

`tests/storage/vector_memory.rs` 验证旧 BQ 默认零常驻、显式视图按容量计量和写入失效、两种持久化模式的合法空 BQ 块、mmap 单层零拷贝、混合层 COW/墓碑复用及过滤查询与 Rom 对照。`brute_force` 的独立串行 reference 覆盖 FP32/FP16/u64、尾维、同分、过滤、空段与混合分段；`test-hooks` 保留显式合并视图发布前的独占交错验证。历史 v5 fixture 使用独立的 32/48-chunk 位布局。

```bash
cargo test --test storage vector_memory
cargo test --lib 分段精确扫描
cargo test --features test-hooks --test concurrency 显式连续视图交错
```

---

## 文本索引生命周期

`tests/storage/text_index_lifecycle.rs` 覆盖 Rom/Mmap 未加载保存、正文替换、关键词幂等、节点删除和 ID 复用、WAL 回放、预算拒绝、写入失败重试、只读零写和旧 v2 快照兼容。BM25 分数使用独立词袋 reference 校验；短中文检索同时验证 BM25 与 `CONTAINS` 扫描回退。Rust、Python、Node 的公共 API 测试复用 `tests/contracts/public_cases.json` 中的文本生命周期用例，并检查关闭后构建错误的透传。

```bash
cargo test --test storage text_index_lifecycle -- --test-threads=1
cargo test --test contracts 共享json契约由rust适配器完整执行
```

---

## 写入边界与索引事实一致性

- `tests/core/write_boundaries.rs` 覆盖三种 dtype、Rom/Mmap、解析缓存开启/关闭的 JSON 深度边界，验证当前读取、WAL 独立回放及关闭重开；更新入口同时验证预算拒绝后的原值、向量和 WAL。`$inc` 通过固定独立数值样例检查精确整数、跨符号边界、小数、溢出和整份补丁原子性。
- `tests/fault_io/wal_failure_state.rs` 在 `test-hooks` 下分别注入 WAL flush/fsync 故障，子进程直接退出，再验证此前确认记录、隔离后的所有拒绝结果及恢复后的再次持久化。未确认操作允许按已持久化日志恢复；已确认数据要求完整核对。
- `tests/storage/sidecar_facts.rs` 验证同长度 Payload 更新与边内容更新后混入旧 sidecar 的处理，检查严格拒绝、属性索引扫描回退、历史索引重建、平行边入度、分数顺序与只读零写。
- `tests/node/write_boundaries.js` 与 `tests/python/write_boundaries.py` 由各端公共 API 入口执行，覆盖直接/批量/事务、查询、完整整数边界、合法容器往返，以及无效输入拒绝后的内容和 WAL。

- `tests/core/index_graph_boundaries.rs` 验证唯一约束删除、部分覆盖索引的完整结果、复合唯一大整数、UTF-8 标签边界、图堆准入、零入度清理、三种 dtype 的 ID 分配耗尽、WAL 回放和只读零写。内核单元测试独立核对图容器容量、字符串分配和复合唯一映射账本。
- `tests/fixtures/numeric_ids/wal_v3.bin` 提供固定完整 u64 ID 跨端样本；Rust 核对完整记录、向量与边，Python 安装制品核对精确整数，Node 安装制品核对字符串 ID 与数字输出拒绝。`tests/node/numeric_boundaries.js` 覆盖数量转换、同步/异步检索、遍历、索引查询及自动分配的 WAL 原子拒绝；`tests/python/index_graph_boundaries.py` 覆盖绑定的索引与图持久化行为。两者均接入现有公共 API CI 入口。

- `tests/core/query_numeric_indexes.rs` 使用独立固定结果验证临时唯一键冲突、原键恢复、删除与 WAL 回放，混合类型范围的 LIMIT/OFFSET，完整 u64 投影/参数/比较/聚合，以及正规数、次正规数和正负零。索引 DDL 覆盖七类入口的预算拒绝、定义保留、配额调整重试与只读重开；唯一构建失败继续保留已有非唯一索引。
- `tests/node/query_numeric_indexes.js` 与 `tests/python/query_numeric_indexes.py` 在实际绑定中执行最终唯一键、数值精度与构建预算断言；Node 用例同时接入独立安装的 npm 制品验收。差分参考模型使用自身的有界整数运算，完整 u64 域由独立常量样例验收。

---

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
- WAL 独立样本搭配同类型基础检查点，历史 fixture 保留原始字节。格式规格覆盖 v10 引入的类型头及 v11 的有界回执尾块，WAL v4 增加同事务回执；历史格式兼容与当前格式校验分别执行。

---

## SDK 请求上下文与持久回执

- `cargo test --features test-hooks --test core shared_sdk -- --test-threads=1` 验证同一请求控制器贯通 SDK 准入和内核 WAL 前检查。受控 Hook 在事务预检完成后取消，核对固定错误码、写入阶段、builder 消费状态、WAL 字节、空事实及后续有效提交与完整重开。准入拒绝单独验证 builder 保留和回调执行次数。
- `tests/core/console_editing.rs` 使用真实完整 WAL 与缺少提交封条的 WAL 两组恢复样本，分别核对完整节点、Payload、边、创建 ID、原事务版本及回执。已封条事务支持幂等重放且 WAL 字节保持，未封条事务与回执共同不可见，随后检查持久化及只读再次打开。容量用例填满 64 条后重开追加无事实变化事务，核对首条淘汰、正常关闭及 WAL 恢复后的关闭保存新回执，随后只读重开核对全部事实。既有用例覆盖 Rom／Mmap、WAL／应用故障与资源释放。
- `tests/storage/receipt_history.rs` 读取 `tests/fixtures/v0900_format10/` 的独立历史制品样本。三种 dtype 分别覆盖 v10 检查点、v3 WAL 和物理截断尾帧，核对只读零写或恢复拒绝、全部事实、格式升级及再次只读打开。样本来源、生产制品身份与内容预期由 `provenance.json` 记录。
- `storage::edit_receipts` 单元用例使用独立编码样例验证完整 u64、每个截断前缀、过量 ID、回执数量及尾块长度／CRC。Python、Node 和实际安装 npm 包的 Console 公共契约核对跨重开确认及完整原事务重放。

### 类型化作用域与宿主桥接

- `cargo test --features test-hooks --test core request_contexts -- --test-threads=1` 覆盖 Reader／Writer 作用域、外层取消与期限、64 帧内部边界、异常展开后的归还，以及直接写入和普通共享事务取得 WAL 锁后的取消。Hook 断言事务消费状态、自动 ID 保留、WAL 字节和完整重开事实。
- Python／Node 的 `request_contexts` 复用独立字段与内容样例，执行转换期间取消、事务准入、嵌套控制、32 层边界和异步任务隔离。Python 在第二次检索的原生工作线程中取消，Node 分别通过同步 Hook 与参数 getter 验证 Core 检查点及异步任务控制，整批返回按最终结果判断。后续有效批量查询核对全部分数与 Payload，再持有上下文关闭、移动 Windows 文件组并只读重开。
- 自定义桥接验证冻结身份和写入类别、真实原生 RequestContext／Console 调用、受控 Promise 的 drain 等待、负面及非法确认、失败后重试，以及 null 拒绝的原始 cause 和未知写入结果。同步依据来自实际 Hook、事件和 Promise 完成。
- Python 存根由安装制品生成，Node 类型消费核对同步与 Promise 的精确返回值及非法调用。实际 wheel 与 npm tarball 分别消费新增作用域和清理契约，范围见本批验收记录。

### 工程收口的定向验证

QuIVer SSD 使用 `cargo test --features test-hooks --lib index::quiver::ssd -- --test-threads=1` 和 `cargo test --features test-hooks --test core quiver_ssd -- --test-threads=1` 验证单文件、来源、预算、缓存、驻留、批内合并、重复候选、取消排空、故障重试及文件释放。`cargo test --features test-hooks --lib storage::quiver_io -- --test-threads=1` 通过独立字节样例核对实际原生对齐、跨界和尾部读取、窗口复用、短读失效重试及路径替换后的文件代际。SSD 运行时另覆盖零行缓存下的有界窗口、请求线程／工作线程后端、Hot 释放及外部行租约。原生 Direct I/O 用例在 Windows／Linux 执行，缓冲路径参与跨平台验证。CI 平台矩阵执行这些入口，TSan 另执行运行时交错子集。云端结论对应实际提交和运行状态。

生产 SSD 测量入口为 `cargo bench --bench bench_quiver_ssd`，参数和报告范围见[基准说明](../benches/README.md#生产-quiver-ssd-测量)。测量记录独立于正确性验收。

文档接入的 `tests/core/document_ingestion.rs` 与共享 `document_ingestion_cases.json` 核对五种内置格式、UTF-8 原文证据、外部页码／区域、分批重试、根修订竞争、大于单 Payload 上限的正文、只读零写、检查点／WAL 重叠、同步未知结果和 Windows 文件组释放。`test-hooks` 启用真实 WAL 故障路径。独立历史 v12／v5 制品保留命名图日志，分别验证 f32／f16／u64 的恢复、升级及再次持久化。Python wheel 与 npm tarball 通过公开文档请求消费相同解析及上传契约。

- request_contexts 的受控 Hook 位于精确评分分片和图计算迭代。用例分别选择串行／并行策略，在到达计算检查点后取消，核对固定错误、工作区和任务归还、WAL 原字节、稳定 Top-K 及完整只读重开内容。
- retrieval_fields 复用共享入口表验证属性索引、复合字段、Leiden、快照、迁移、关闭超时和会话参数。Python 事务用例核对成员定位、原宿主异常及已有缓冲，Node 事务用例核对 operations 路径和未消费状态。
- Console 嵌入 HTTP 验证请求体前身份、Unicode 编码、头与正文冲突、正确读回及完整重开。独立 Server 验证路径、方法、JSON、请求体预算、legacyCode 与请求头身份，CLI 验证 JSON 参数和输入文件错误。
- 上述定向命令用于检查本批回归。整批交付验收另外执行适用的完整套件、安装制品、优化构建及平台矩阵，证据对应最终源码与提交。
