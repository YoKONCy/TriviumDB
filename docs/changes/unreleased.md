# 未发布变更

后续变更以 `0.9.0` 正式版为基线。已发布内容见[完整中英文变更说明](0.9.0.md)。

### nightly WebUI：节点文档工作区

- 新增节点文档工作区：JSON 与映射的 Markdown/纯文本正文共享草稿，支持浏览器草稿恢复、预览、大纲、查找替换、行级差异及提交审阅。写入复用 `/v1/transactions`，没有新磁盘格式或编辑租约。
- 跨入口合并或替换暂存项后仍保留节点版本、幂等键和确认状态；重新获取版本时校验编辑基线，外部修改要求显式处理冲突。
- 提交批次冻结请求体、前置条件和操作快照。等待响应期间可以继续编辑草稿和暂存其他节点，但不能替换或撤回已发送批次；成功只确认实际发送的内容并移除对应队列项。
- 含文档操作的事务在网络失败、服务端错误或成功响应无法解析时保留未知状态，通过“确认原事务”以相同幂等键重试完整请求。不能仅因某个节点内容相等就确认混合事务，从而避免重复导入。确认后的新草稿使用新批次与新键；未知事务重试遇到冲突时继续保留未知状态，不推断原事务未提交。
- 修复 Ctrl+S 冒泡导致的重复暂存；异步读取版本期间，同一文档的暂存串行化。文档写入与其他入口一致拒绝超出 JavaScript 安全整数范围的节点 ID。

使用限制：此能力属于 nightly WebUI，数据库锁由宿主管理。草稿按 origin、页面路径与节点 ID 区分，无法可靠识别同一地址切换的不同数据库；切换前应导出并处理旧草稿。未知事务的原请求保留在当前页面会话，刷新或关闭后需核对完整事务的实际结果，不能以恢复的正文草稿作为提交证明；服务端幂等缓存也不是跨重启永久日志。

验收采用代码审查、语法检查及 Server 非 UI 契约，不包含 UI/UX、样式、布局、动画或耗时阈值测试。平台执行结果以对应提交验收为准。

## Unreleased

Subsequent changes build on stable `0.9.0`. Published changes are recorded in the [complete Chinese and English release notes](0.9.0.md).

### Nightly WebUI: node document workspace

- Adds a node document workspace where JSON and mapped Markdown/plain-text views share drafts, with browser draft restoration, preview, outline, find/replace, line diffs, and write review. Writes reuse `/v1/transactions`, without a new disk format or edit lease.
- Merging or replacing staged work from another entry retains node versions, idempotency, and acknowledgement tracking. Refreshing a version also checks the editing baseline; external changes require explicit conflict handling.
- Submitted batches freeze their request body, preconditions, and operation snapshots. Users may continue editing drafts and staging other nodes while awaiting a response, but cannot replace or withdraw the submitted batch. Success acknowledges only transmitted content and removes only that batch's queue entries.
- Network failures, server errors, and unreadable success responses leave transactions containing document operations in an unknown state. “Confirm original transaction” retries the complete request with the original idempotency key. Matching one node's content cannot confirm a mixed transaction or cause imports to be repeated. New drafts use a new batch and key after acknowledgement. A conflict while retrying an unknown transaction keeps its outcome unknown rather than implying it never committed.
- Fixes duplicate staging from Ctrl+S event propagation and serializes staging within a document during asynchronous version reads. Document writes reject node IDs outside JavaScript's safe integer range, consistently with other write entries.

Limitations: this is a nightly WebUI capability; the host manages database locks. Drafts are scoped by origin, page path, and node ID and cannot reliably distinguish different databases served at the same address. Export and resolve old drafts before switching databases there. An unknown transaction's original request exists in the current page session; after refreshing or closing it, verify the entire transaction's actual outcome. A restored body draft is not proof of commit, and the server idempotency cache is not a permanent log across restarts.

Acceptance uses code review, syntax checks, and non-UI server contracts. UI/UX, styling, layout, animation, and elapsed-time threshold tests are excluded. Platform execution results are tied to the validated commit.
