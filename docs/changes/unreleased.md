# 未发布变更

后续开发以 `0.9.1-beta.1` 为基线。本版完整变更见 [0.9.1-beta.1 中英文说明](0.9.1-beta.1.md)。

## 中文说明

### QuIVer 可选压缩重排

- 新增中心化 INT8、中心化 INT4 重排编码，查询表示可选择 FP32、INT8、INT4。精度优先和紧凑优先预设分别使用中心化 INT8／INT4 与 FP32 查询，均适用于 Hot 和 SSD 驻留。完整组采用 AVX2 派发，其他平台及尾组使用标量评分。
- 新编码按代际持久化共享均值、分组参数与完整重建范数。中心化制品采用 QVSSD v2，中心在打开时校验，行身份、slot、墓碑与 CRC 在读取时校验。codec 1 的 QVSSD v1 制品继续通过既有解码路径读取。
- 库编码、查询精度、驻留方式和字节缓存预算独立配置。Rust 提供通用 QuIVer 配置、查询、统计与关闭入口，Python／Node 复用现有 JSON 协议。`use_for_ann` 显式选择普通 ANN 路径的压缩重排，量化分数在 TQL 向量来源中标记为 Approximate。
- 最终候选预取与评分 SIMD 分别派发，单批至多 64 行。读取复用既有有界队列、窗口合并、取消排空及 Pending 生命周期。缓存保留压缩记录，查询与调用方持有的行租约继续计入资源预算。
- 数据库压缩 Reader 按代际借用主 QuIVer 的路由，打开时使用固定工作区流式校验绑定热区，减少路由副本及解码峰值内存。新增 `routing_shared` 统计，主路由及其 ID／slot 映射、墓碑和运行期身份在数据库基础堆中统一计量。图重建独立核对路由身份，关闭后的自有行租约继续持有其编码与中心。

> **使用提醒**　压缩重排适用于余弦查询。精度代价应按业务数据及召回工作点验证，原始向量事实按所选存储模式保存。只读配置需要已存在且来源、编码匹配的派生制品。接口说明与配置示例见 [QuIVer](../quiver.md)。

---

## English

Subsequent development builds on `0.9.1-beta.1`. Its complete changes are recorded in the [bilingual release notes](0.9.1-beta.1.md).

### Optional compressed QuIVer reranking

- Added centered INT8 and centered INT4 database encodings with FP32, INT8, or INT4 query representations. The accuracy and compact presets pair centered INT8 and centered INT4 with FP32 queries respectively. Both support Hot and SSD residency. Complete groups use AVX2 dispatch where available, with scalar scoring for other platforms and partial groups.
- Each centered artifact persists its shared mean, group parameters, and full reconstructed-vector norm correction. Centered artifacts use QVSSD v2. Opening validates the center, and row reads validate identity, slot, tombstones, and checksums. Existing codec 1 artifacts retain their QVSSD v1 decoding path.
- Database encoding, query precision, residency, and byte-based cache budgets are configured independently. Rust exposes general QuIVer configuration, search, statistics, and close methods. Python and Node share the existing JSON protocol. Explicit `use_for_ann` enables compressed reranking for ordinary ANN paths, and the TQL vector source labels quantized scores Approximate.
- Final-candidate prefetching and scoring SIMD dispatch operate independently, with up to 64 rows per read batch. Reads reuse bounded queues, window merging, cancellation draining, and Pending lifetimes. Caches retain compressed records, and query-held or caller-held row leases remain charged to resource budgets.
- Database compressed readers borrow the primary QuIVer routing generation under Core locks. Opening validates the bound hot section with a fixed streaming workspace, reducing routing copies and decoding peak memory. Added `routing_shared` statistics, with primary routing, ID/slot mappings, tombstones, and runtime identity charged once in the database base heap. Graph rebuilds independently validate routing identity, and owned row leases retain their encoded bytes and center after close.

> **Usage reminder**　Compressed reranking supports cosine queries. Evaluate its quality trade-off on application data and relevant recall operating points. Original vector facts follow the selected database storage mode. Read-only configuration requires an existing derived artifact with matching source and encoding. See [QuIVer](../quiver.md) for configuration and API details.
