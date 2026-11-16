# 未发布变更

> 后续开发以 `0.9.1` 为基线，已交付能力见 [0.9.1 中英文说明](0.9.1.md)。

---

## 中文说明

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
