# 公共错误与请求完成契约

> 本文记录以 `0.9.0` 为基线的当前开发版本，覆盖打开、共享访问、事务、Prepared TQL 与关闭路径。错误定义见 `src/error.rs`，SDK 职责见[内核 SDK 与存储内核](kernel-sdk.md)。

---

## 目录

- [公共表示](#公共表示)
- [写入结果字段](#写入结果字段)
- [操作与输入字段](#操作与输入字段)
- [提交阶段与事实来源](#提交阶段与事实来源)
- [语言及传输边界](#语言及传输边界)
- [后续收敛](#后续收敛)

---

## 公共表示

| 字段 | 当前契约 |
| --- | --- |
| `schema_version` | 当前为 `1` |
| `code` | Core 稳定类别，具体详情与对应语言声明配套使用 |
| `message` | 面向调用方的诊断说明，项目上下文使用中英文双语 |
| `details` | 错误类别对应的结构化事实，空详情使用空对象 |
| `stage` | 已覆盖的 TQL 阶段为 `lex`、`parse`、`bind` |
| `target.query_span` | 原始查询的 UTF-8 字节锚点与一基 Unicode 标量行列 |
| `target.parameter` | Prepared 参数的原始名称，与传入映射中的键一致 |
| `write` | 已覆盖写入口的失败阶段、结果及操作缓冲消费状态 |

`target` 根据错误来源使用查询位置或参数名一种表示。JSON／Node 省略缺失的扩展字段，Python 使用 `None`。参数名中的点、斜杠、方括号及 Unicode 字符按原始键传递。缺参和多参的选择按参数名排序，缺参检查先执行。

参数转换与绑定定位保留原始 `code`、`details` 和错误链。Rust 可通过 `TriviumError::underlying()` 取得原始错误变体，Python 按原始变体保留异常继承关系。Prepared 参数 JSON 容器值沿用 `TDB_INVALID_INPUT`，向量槽位中的非数值沿用 `TDB_QUERY_EXECUTION`，两者增加 `stage=bind` 与参数定位。

Rust 与 Python 保留错误整数的完整精度。Node 与 HTTP 将超出 JavaScript 安全整数域的详情编码为十进制字符串。全部 Core 错误码与详情类型见 npm 公共声明 `ErrorDetailsByCode`，真实行为由共享契约与各语言实际制品验证。

> **处理提醒**：根据 `code`、`details` 与已提供的扩展字段处理错误。第三方与 OS 的原始 cause 用于诊断。

---

## 写入结果字段

已覆盖的写入口错误提供可选 `write` 对象。Core 在真实执行阶段提供事实，SDK 补充进入 Core 前的拒绝状态。外层准入保留内层已经提供的写入上下文。Rust 使用 `WriteFailure` 承载上下文，通过 `underlying()` 匹配原错误；规范错误码、详情、message 与 cause 继续传递。

| 字段 | 契约 |
| --- | --- |
| `phase` | `admission`、`validation`、`wal`、`apply`、`group_sync`、`response` |
| `outcome` | `not_committed` 表示本请求尚未提交事实。`unknown` 表示需要恢复或对账。`committed` 表示全部事实已完成应用，耐久性依照同步模式与组尾完成点 |
| `transaction_consumed` | 适用时提供布尔值，表示本次请求是否已消费待提交操作缓冲。直接 CRUD、聚合组结果及回执响应省略此字段 |

Rust `Database::commit_tx` 和 `Transaction::commit` 按值取得操作，进入提交入口后失败时 `transaction_consumed=true`。`SharedDatabase::commit_tx(&mut builder)` 与 Python 活动事务在 SDK 准入拒绝时保留操作，返回 `false`；Core 预检拒绝与后续失败返回 `true`。已结束 Python 对象再提交时，当前请求在准入阶段拒绝，消费字段为 `false`。

Node `commitTransaction` 的 Core 拒绝保留相同字段，绑定内已知的构造校验与同步重入拒绝提供相应阶段和 `transaction_consumed=false`。Node 参数数组由调用方继续持有，消费字段描述绑定生成的内核操作缓冲。参数自动提取和宿主自行抛出的异常继续使用各自语言契约。

分组提交在组尾完成同步。组尾失败返回 `group_sync/unknown`，消费字段省略。Server 将该错误逐一送回本组已经成功执行的等待请求，独立操作已返回的校验或执行错误保持原字段。分组准入失败将 `admission/not_committed` 送回尚未执行的请求。

Server 入队前的关闭、过期和队列满，以及 Worker 明确拒绝的待执行请求，提供 `admission/not_committed`。入队后等待响应超时或响应通道丢失，提供 `response/unknown`。Server 省略这些边界的消费字段，具体事务的执行结果由有效回执或事实对账确认。

Console 已提交事务的重放响应若超过预算，保留 `details.committed=true`，增加 `response/committed`。调用方可继续用原幂等键及原事务查询回执。有效回执受实例、容量和淘汰规则约束。

> **重试提醒** 先读取 `outcome`，再结合 `transaction_consumed`、错误类别与原事务回执决定重试。`not_committed` 的输入错误需要修正输入，`unknown` 需要对账或恢复，`committed` 需要处理响应或读取结果。分组内的结果同时依照组尾完成点。HTTP 携带 `write` 时使用 `retryable=false`，由调用方明确安排重新准入和重放。

---

## 操作与输入字段

| 操作与字段 | 校验或准入位置 | 当前错误与完成边界 |
| --- | --- | --- |
| 打开数据库的 `path` | Core 文件身份、文件租约与文件组打开 | `TDB_DATABASE_LOCKED`、`TDB_IO`；失败打开清理本次取得的资源 |
| 打开的 `dim`、`dtype`、访问模式与配置 | 语言提取、配置构造、Core 打开与文件身份校验 | `TDB_INVALID_INPUT`、`TDB_DIMENSION_MISMATCH`、`TDB_DTYPE_MISMATCH`、格式与恢复类别；语言提取仍按各端现有行为 |
| 共享句柄的读、写、关闭请求 | SDK 请求身份及 try-lock | `TDB_BUSY` 的 `resource`、`TDB_REENTRANT_ACCESS` 的 `operation`、`TDB_HOST_LOCK_POISONED` 的 `resource` |
| Python 数据库及事务对象的方法入口 | 显式借用与请求身份 | 同线程回调重入返回重入类别，跨线程借用冲突返回忙碌类别；原持有者继续执行 |
| 事务缓冲的 ID、向量、Payload、边及权重 | 语言转换、事务构造、Core 预检 | 维度、有限值、保留 ID、节点引用、唯一性、大小、容量与权限分别验证；Core 事务按完整操作集预检 |
| 已结束的 Python 事务对象 | builder 状态 | 缓冲、提交、回滚及进入上下文返回 `TDB_TRANSACTION_FINISHED`；`pending_count()` 为 0 |
| 事务提交及上下文退出 | SDK 准入、Core WAL 与原子应用 | SDK 准入拒绝保留 builder，进入 Core 后消费事务；宿主异常按上下文契约传播 |
| 节点及边的直接 CRUD、Payload 补丁与 CAS | Core 准入、校验、单条 WAL 与应用 | 准入或校验失败为 `not_committed`，WAL 或应用失败为 `unknown`；直接入口省略 `transaction_consumed` |
| TQL 写语句构造及计划准入 | Core 解析、写计划、操作预算与事务提交 | 事务构造前失败为 `validation/not_committed`；进入事务后保留内层提交阶段及消费状态 |
| 条件编辑的版本与幂等键 | Core 编辑状态与事务预检 | `TDB_WRITE_CONFLICT`、`TDB_IDEMPOTENCY_CONFLICT`、`TDB_WRITE_OUTCOME_UNKNOWN`；版本作用域为数据库实例，回执保存在实例内 |
| TQL 的 `query` | Core 词法与解析入口 | `TDB_QUERY_PARSE`，`stage` 为 `lex` 或 `parse`，可靠位置使用 `target.query_span` |
| Prepared 的参数名与必需集合 | Core 参数绑定 | `TDB_MISSING_QUERY_PARAMETER`、`TDB_UNEXPECTED_QUERY_PARAMETER`，`details.parameter` 与 `target.parameter` 保持一致 |
| Prepared 的参数值 | 共用命名 JSON 转换与 Core 绑定 | 容器、有限值及向量槽位类型按原始错误类别返回，已知参数名写入 `target.parameter` |
| 关闭、关闭超时与状态读取 | SDK、Core 生命周期和任务排空 | `TDB_DATABASE_CLOSED`、`TDB_SHUTDOWN_FAILED`、WAL 与 I/O 类别；`close_timeout` 返回 `false` 时关闭任务继续执行 |

共享契约入口包括 `tests/contracts/error_cases.json`、`query_diagnostics.json`、`prepared_diagnostics.json` 和 `console_editing_cases.json`。这些用例同时核对对应字段、拒绝后的数据库事实及重开内容。

---

## 提交阶段与事实来源

下表记录现有实现能够判定的事实及事务错误中的相应字段。

| 阶段 | 现有实现提供的事实 | 调用方处理依据 |
| --- | --- | --- |
| SDK 借用与数据库准入 | 锁忙、重入、关闭或恢复保护在取走 builder 前拒绝 | `admission/not_committed`，待提交操作保留 |
| 参数转换与缓冲 | 完整转换后才追加本次操作 | 转换异常保留此前缓冲操作，调用方可修正当前输入 |
| Core 事务预检与预留 | 权限、引用、维度、唯一性、预算及 WAL 条目构造在追加前完成 | 权限与生命周期准入为 `admission`，其余预检为 `validation`；结果为 `not_committed`，Core 已消费操作 |
| WAL 追加前检查 | writer 可用性、事务 ID 和编辑回执准备先于追加 | 需要依据具体失败位置判定本次写入是否开始 |
| WAL 追加、flush 与 sync | I/O 失败可能留下部分帧或完整事务帧；writer 进入保护状态 | 单次事务追加失败为 `wal/unknown`，组尾同步失败为 `group_sync/unknown` |
| MemTable 应用 | WAL 已完成本次追加；应用失败由保护状态隔离部分内存事实 | `apply/unknown`，重新打开后核对完整事务与恢复结果 |
| 普通提交成功 | 按配置的同步模式完成该次事务 | 持久化保证依照 `SyncMode`；分组提交以组尾同步完成点确认 |
| 响应交付 | 内核已知的响应编码或预算失败保留写入事实，宿主传输可能丢失响应 | 已知全部应用完成时提供 `response/committed`；其他传输错误依照原事务回执或事实对账 |

`SyncMode::Normal`、`Full` 和 `Off` 的耐久性分别遵守其同步契约。分组内的单次事务成功、组尾同步和传输响应具有各自完成点。实例内编辑回执的容量、淘汰与重开行为见[嵌入式控制台](embedded-console.md)。

直接节点和边写入保留单条 WAL 编码及原有索引同步方式。WAL 追加后的应用失败复用事务恢复保护，隔离当前实例及已准入的共享查询；释放句柄并重新打开后核对恢复事实。资源诊断通过原始错误类别提取预算和 I/O 信息。

> **覆盖范围** 本批字段覆盖 Core 事务、直接节点及边 CRUD、Payload 补丁与 CAS、TQL 写计划及预算准入、共享 SDK、Python 事务、Node 事务的已知绑定校验、分组提交、Server 队列与响应等待，以及已知 Console 写响应。文本和索引维护入口、语言实参提取、HTTP／IPC 外部传输、宿主取消与结果确认继续按入口收敛。具体 I/O 错误的写入结果来自阶段上下文。

---

## 语言及传输边界

| 入口 | 已覆盖的结构 | 继续收敛的边界 |
| --- | --- | --- |
| Rust Core／SDK | `TriviumError::info()`、共享准入、TQL 与命名参数诊断、已覆盖写入口的结果 | 通用请求上下文、其余维护入口的写入结果与其他输入字段定位 |
| Python | Core 属性、数据库与事务借用、命名 JSON 参数与绑定错误 | PyO3 自动实参提取、普通宿主类型转换及其他包装对象 |
| Node | Core 属性、无损整数编码、同步回调重入、命名参数诊断、自动 ID 预检查的准入拒绝 | N-API 提取、参数映射外层类型、非有限 JSON 值及其他适配错误 |
| CLI | JSON 模式的 Core 错误、TQL 解析阶段与位置、写语句预解析及计划拒绝的结果 | 命令行解析、配置和输入文件等其他错误来源 |
| 独立 Server | Core 类别、`legacyCode`、Prepared 参数诊断、已覆盖的队列和响应等待结果 | 外部传输取消、缓存句柄及其余 `retryable` 的分阶段语义 |
| 嵌入 Console／HTTP／IPC | Core 请求协议及错误信封、TQL 和参数诊断 | 宿主传输自身产生的错误分类与写入响应完成点 |

HTTP 状态与旧分类遵守各适配器当前协议，规范 `code`、`details`、`stage` 和 `target` 从 Core 传递。Server 的 `legacyCode` 用于旧调用方迁移。

---

## 后续收敛

- TODO: 将其他公共 API 的输入字段逐项加入目录，记录校验层、参数位置、原始 cause、兼容决定和实际制品证据。
- TODO: 将写入阶段与结果扩展到文本和索引维护入口，继续统一语言实参提取、宿主取消、外部传输及跨重开的结果确认。
- TODO: 扩展完整 AST、参数引用的源码范围、规划与执行诊断，并维护普通 API 的字段路径表示。
- TODO: 逐项收敛 `QueueFull`、Server 旧类别与 `retryable`，明确重新准入和重新执行写入的适用条件。
