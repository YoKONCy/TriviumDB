# 未发布变更

> 后续开发以 `0.9.1` 为基线，已交付能力见 [0.9.1 中英文说明](0.9.1.md)。

---

## 中文说明

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

- 检查点在文件组发布前同步 WAL v7 的唯一封条，TMET v2 将封条、主文件长度及 CRC、文本长度及 CRC 绑定并校验。恢复按封条区分检查点覆盖的旧删除与后续真实删除，覆盖节点 ID 复用、立即／延期加载、Rom／Mmap 和 Off 同步模式。历史 TMET v1 保留读取能力；旧文本缺少封条且恢复涉及已登记节点的删除及 ID 复用时，返回 `TDB_RECOVERY_REQUIRED`，定位字段为 `text_index.recovery_boundary`，保留原始文件供处理。
- WAL 回放实际插入新向量后使已有 QuIVer 路由失效，后续普通搜索按配置重建或精确回退，恢复后的当前实例可检索全部已确认节点。
- 已有文本检查点在无待恢复日志时，由 Writer 打开流程补齐封条及 TMET v2，覆盖元数据发布后、日志清理前失败的重试。旧 WAL 升级按帧与事务暂存执行内存准入，长日志中反复更新少量节点时按实际恢复峰值处理。
- 自定义召回、召回后、图扩散前和 DPP 最终回调结束时立即检查 Hook 错误，统一返回 `TDB_HOOK_EXECUTION`。空召回和普通返回沿相同错误传播契约执行。
- `$set`、`$inc`、`$unset` 的操作数在应用整份补丁前验证为 JSON 对象。非法操作数返回 `TDB_INVALID_INPUT` 及 `patch.<操作符>` 字段定位，Payload 与 WAL 保持提交前内容。

> **升级与恢复提醒**　升级前保留完整文件组备份。当前开发内核写入 WAL v7 与 TMET v2，历史 WAL v2–v6 由 Writer 按对应布局校验并升级。旧版本环境从升级前备份恢复。遗留恢复歧义通过完整备份恢复数据库，或由宿主保留的事实创建新库并登记正文与关键词。已被旧版本清除的文本按备份或来源重建。

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

- Checkpoints synchronize a unique WAL v7 seal before publishing their file group. TMET v2 binds and checks the seal, main-file size and CRC, and text-file size and CRC. Recovery distinguishes checkpoint-covered deletions from subsequent deletions, covering node ID reuse, eager/deferred loading, Rom/Mmap, and Off synchronization. TMET v1 remains readable. When legacy text has no seal and recovery deletes and reuses an indexed node ID, opening returns `TDB_RECOVERY_REQUIRED` with target `text_index.recovery_boundary` and preserves the original files for recovery.
- Actual vector insertion during WAL replay invalidates existing QuIVer routing. Subsequent ordinary searches rebuild or use exact fallback according to configuration, making all acknowledged nodes searchable in the recovered instance.
- Writers add a seal and TMET v2 to an existing text checkpoint when opening with no pending recovery log, including retries after metadata publication succeeds before WAL cleanup fails. Legacy WAL upgrades admit memory for individual frames and pending transactions, accounting for the actual recovery peak when a long log repeatedly updates a small set of nodes.
- Custom recall, post-recall, pre-graph, and final DPP callbacks immediately propagate Hook errors as `TDB_HOOK_EXECUTION`. Empty recall and ordinary returns follow the same error contract.
- `$set`, `$inc`, and `$unset` operands are validated as JSON objects before applying a patch. Invalid operands return `TDB_INVALID_INPUT` with a `patch.<operator>` target, retaining the pre-commit Payload and WAL contents.

> **Upgrade and recovery note**　Retain a complete file-group backup before upgrading. Development builds write WAL v7 and TMET v2. Writers validate and upgrade historical WAL v2–v6 using their corresponding layouts. Restore the pre-upgrade backup when returning to an older engine. For ambiguous legacy recovery, restore a full database backup or create a new database from the host's retained facts and register its text and keywords. Rebuild text already removed by an older engine from a backup or retained source.

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
