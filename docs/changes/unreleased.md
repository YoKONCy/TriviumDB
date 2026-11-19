# 未发布变更

**中文** | [**English**](#english)

---

## 中文说明

- QuIVer SSD 直读支持同代际共享对齐页和跨查询同页合并。精排可在等待缺失记录时先评分已验证的命中行。Linux 新增显式 `io_uring` 模式，使用固定文件和对齐缓冲批量读取。配置、线程与内存要求见 [QuIVer I/O 说明](../quiver.md#缓存驻留与-io-生命周期)。
- Windows QuIVer SSD 直读接入 IOCP，单调度线程批量提交并逐项回收完成。IOCP 和 `io_uring` 按设备对齐和读取跨度分配固定缓冲槽位。运行状态通过 `io_fixed_buffer_bytes` 单独报告固定池占用。
- QuIVer 压缩精排支持请求级 I/O 计量。逻辑冷读、系统读取和行／页复用按请求归属。Rust 通过 `measure_io` 读取计量，Python 和 Node 的压缩检索可用 `profile_io` 返回本次计数。

已发布内容见 [0.9.2 双语发布说明](0.9.2.md)。

---

## English

- QuIVer SSD direct reads share aligned pages within a file generation and coalesce pending reads across queries. Reranking can score validated cache hits while other records are in flight. Linux adds an explicit `io_uring` mode for batched reads with a registered file and aligned buffers. See the [QuIVer I/O configuration](../quiver.md#缓存驻留与-io-生命周期) for resource and platform requirements.
- Windows QuIVer SSD direct reads use IOCP to submit batches and handle individual completions on one dispatcher thread. IOCP and `io_uring` size fixed buffer slots according to device alignment and read span. Runtime statistics expose the fixed buffer payload size in `io_fixed_buffer_bytes`.
- QuIVer compressed reranking can measure logical cold reads, system reads, and row and page reuse per request. Rust exposes the counters through `measure_io`. Python and Node callers can set `profile_io` on compressed searches to retrieve the counters with their results.

Published changes are recorded in the [0.9.2 bilingual release notes](0.9.2.md).
