# 未发布变更

> 后续开发以 `0.9.2-beta.1` 为基线，已交付内容见 [0.9.2-beta.1 中英文说明](0.9.2-beta.1.md)。

---

## 中文说明

- 代际回收按实际目录保护当前代和 Reader 租约，拒绝带路径分隔符的名称。WAL 在首帧写入前校验全部记录的编码体长度，写入与回放共用 256 MiB 上限。
- SDK 维度迁移保留单字段和组合唯一约束。快照校验与恢复核对主文件类型、元素宽度及向量块长度，发现清单类型错配时拒绝发布目标目录。
- Node 保留 Payload 安全整数的整数编码，CAS 对完整 JSON 结构中的数字按精确数值比较。Python 和 Node 返回存储边权的精确扩展值。
- 有序索引键失去精度时，TQL 先按原始值排序再分页。全路径乘积保留可表示的最终值，SCC 投影保留无符号分量标识。
- 维度迁移保留登记正文的词频与文档长度、关键词和普通属性索引。可达性在标签过滤和去重前扣除扫描预算。Python 的 `index_info()` 声明与运行时的索引描述列表一致。
- 命名图的简写向量扩展遵守所选图，模式的结构 ID 条件独立于 Payload 索引。带标签的零跳匹配保留自身关系，读查询和写语句使用同一规划规则。索引候选在精确向量 Top-K 前按 JSON 值及类型复核。
- `FIND` 的 `_` 固定引用节点。浮点求和及均值保留混合输入抵消后的有效尾项。空 `COLLECT` 返回列表，`DISTINCT COUNT(*)` 返回解析错误。
- Yen 路径行保留起点绑定。全路径查询去除重复目的地，禁经集合覆盖起终点，标签匹配完整序列，跨路径强度聚合保留可表示的结果。
- Payload 补丁在写入前拒绝未知操作符。字段过滤拒绝空运算符对象，时间比较覆盖宽范围 RFC3339 日期。维度迁移保留节点 ID 的分配高水位。
- Node Payload 使用自有可枚举字符串键，支持写回读出的 `bigint`。CLI 按投影返回真实列和值，CSV 保留完整单元格，JSON 写入确认包含影响行数及新建 ID。

---

## English

Subsequent development builds on `0.9.2-beta.1`. Delivered changes are recorded in the [bilingual release notes](0.9.2-beta.1.md).

- Generation reclamation protects the current directory and active reader leases by resolved directory identity. Names containing path separators are rejected. WAL writes check every encoded record before the first frame and share the 256 MiB limit with replay.
- SDK dimension migration preserves single-field and composite unique constraints. Snapshot verification and restore check the main file's dtype, element width, and vector extent. A manifest dtype mismatch prevents the restored directory from being published.
- Node encodes safe integral Payload numbers as integers. Payload CAS compares numbers exactly throughout the JSON structure. Python and Node widen stored edge weights without rounding.
- When ordered index keys lose precision, TQL sorts by the original values before pagination. All-paths products preserve representable final values, and SCC projection preserves unsigned component IDs.
- Dimension migration preserves registered text term frequencies and lengths, keywords, and ordinary property indexes. Reachability charges edge scans before filtering and deduplication. The Python `index_info()` stub describes the list of index records returned at runtime.
- Named-graph shorthand vector expansion uses the selected graph. Structural node ID predicates remain independent of Payload indexes. Labeled zero-hop patterns retain self-matches in reads and mutations. Indexed candidates are checked against exact JSON values and types before vector top-K selection.
- The implicit `_` variable in `FIND` refers to a node. Floating-point sums and averages preserve representable residuals after cancellation across mixed inputs. Empty `COLLECT` returns a list, and `DISTINCT COUNT(*)` is rejected during parsing.
- Yen path rows retain the bindings of the path origin. All-paths queries deduplicate destinations, apply the exclusion set to both endpoints, match the full label sequence, and preserve representable aggregates across paths.
- Payload patches reject unknown operators before writing. Field filters reject empty operator objects, and time filters compare dates across the wider RFC3339 range. Dimension migration preserves the node ID allocation high-water mark.
- Node Payloads use own enumerable string keys and accept `bigint` values read back from the database. The CLI emits projected columns and values, preserves complete CSV cells, and returns affected rows and created IDs in JSON mutation acknowledgments.
