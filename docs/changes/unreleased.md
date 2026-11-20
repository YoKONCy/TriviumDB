# 未发布变更

**中文** | [**English**](#english)

---

## 中文说明

- QuIVer SSD 直读支持同代际共享对齐页和跨查询同页合并。精排可在等待缺失记录时先评分已验证的命中行。Linux 新增显式 `io_uring` 模式，使用固定文件和对齐缓冲批量读取。配置、线程与内存要求见 [QuIVer I/O 说明](../quiver.md#缓存驻留与-io-生命周期)。
- Windows QuIVer SSD 直读接入 IOCP，单调度线程批量提交并逐项回收完成。IOCP 和 `io_uring` 按设备对齐和读取跨度分配固定缓冲槽位。运行状态通过 `io_fixed_buffer_bytes` 单独报告固定池占用。
- QuIVer 压缩精排支持请求级 I/O 计量。逻辑冷读、系统读取和行／页复用按请求归属。Rust 通过 `measure_io` 读取计量，Python 和 Node 的压缩检索可用 `profile_io` 返回本次计数。
- 旧 Linux 内核的 XFS 可通过 `XFS_IOC_DIOINFO` 取得直接读取约束。QuIVer 使用经原生读取验证的文件系统粒度，减少 64 KiB 保守窗口带来的读取放大。
- 新增可显式选择的 [Pelorus 双曲索引](../pelorus.md)。固定曲率配置、构图和几何感知导航与 Poincaré 精确扫描共用原始向量。支持文档、命名图、Hot／Mmap／SSD 和统一索引容器。普通查询默认选择 QuIVer。
- Pelorus 构建成功时曲率同步持久化，事实指纹包含曲率。Server 根据实际发布结果更新 ETag。Pelorus 批次校验空批次选项，按累计响应 Payload 计费，文档阈值按公开分数的精度比较。Node.js 的 `pelorusRequest()` 缺参返回结构化诊断。
- 快照拒绝占用源库制品路径，校验主库与索引容器的绑定，并在恢复发布前检查取消。旧代回收校验当前库与目标代际。CLI 在仅有 WAL 时报告缺少基础检查点。
- 检查点的内部编码使用维护配额。查询在读取和解析 Payload 前检查预算。简单路径保持节点唯一，有界路径仅在确认遗漏或耗尽遍历预算时标记截断。

- Pelorus 懒重建在安装派生索引前复核配置和向量代际。SDK 按实际 JSON 编码字节数准入响应，文档分数超出 `f32` 有限范围时返回查询错误。
- 快照、维度迁移和 CLI 导出共用源库制品保护，识别 Windows Unicode 大小写别名。校验快照及发布迁移时响应请求取消。`prepare_generation()` 拒绝已存在的代际和保留的 `current.json` 名称。
- 扫描在创建超预算的结果行前停止。双向最短路的部分模式保留已经找到的路径。CLI 修复检查校验完整文件头，底层恢复插入拒绝重复 ID，自动分配从有效 ID 开始。

已发布内容见 [0.9.2 双语发布说明](0.9.2.md)。

---

## English

- QuIVer SSD direct reads share aligned pages within a file generation and coalesce pending reads across queries. Reranking can score validated cache hits while other records are in flight. Linux adds an explicit `io_uring` mode for batched reads with a registered file and aligned buffers. See the [QuIVer I/O configuration](../quiver.md#缓存驻留与-io-生命周期) for resource and platform requirements.
- Windows QuIVer SSD direct reads use IOCP to submit batches and handle individual completions on one dispatcher thread. IOCP and `io_uring` size fixed buffer slots according to device alignment and read span. Runtime statistics expose the fixed buffer payload size in `io_fixed_buffer_bytes`.
- QuIVer compressed reranking can measure logical cold reads, system reads, and row and page reuse per request. Rust exposes the counters through `measure_io`. Python and Node callers can set `profile_io` on compressed searches to retrieve the counters with their results.
- On Linux kernels without `STATX_DIOALIGN`, QuIVer can obtain XFS direct-read alignment with `XFS_IOC_DIOINFO`. A native read validates the selected geometry, reducing read amplification from the conservative 64 KiB window.
- Added an opt-in [Pelorus hyperbolic index](../pelorus.md). Fixed-curvature graph construction, geometry-aware navigation, and exact Poincaré scans use the same stored vector facts. Pelorus supports document and named-graph scopes, Hot/Mmap/SSD residency, and the unified index container. Ordinary searches default to QuIVer.
- Successful Pelorus builds persist the curvature, which is included in the durable fact fingerprint. The Server updates its ETag according to the publication outcome. Pelorus validates options even for empty batches, budgets the cumulative response payload, and compares document thresholds at the precision of the returned score. Node.js provides a structured diagnostic for missing arguments to `pelorusRequest()`.
- Snapshots protect the source artifact paths, validate that the index container belongs to the database, and check cancellation before publishing a restore. Generation reclamation validates both the active database and the target generation. The CLI reports a missing base checkpoint when only a WAL is present.
- Checkpoint encoding uses the maintenance quota. Queries check their budgets before reading or parsing payloads. Simple paths contain unique nodes. Bounded path queries report truncation upon finding an omitted path or exhausting the traversal budget.

- Pelorus lazy rebuilds recheck the configuration and vector generation before installing derived indexes. The SDK budgets actual JSON response bytes. Document queries report a query error for scores outside the finite `f32` range.
- Snapshots, dimension migration, and CLI exports share source artifact protection, including Windows Unicode case aliases. Snapshot verification and migration publication honor request cancellation. `prepare_generation()` rejects existing generations and the reserved `current.json` name.
- Scans stop before allocating a result row beyond the budget. Partial bidirectional shortest-path queries retain paths already found. The CLI repair check validates the full header. Low-level recovery inserts reject duplicate IDs and automatic allocation starts at a valid ID.

Published changes are recorded in the [0.9.2 bilingual release notes](0.9.2.md).
