# 未发布变更

> 后续开发以 `0.9.2-beta.1` 为基线，已交付内容见 [0.9.2-beta.1 中英文说明](0.9.2-beta.1.md)。

---

## 中文说明

- 代际回收按实际目录保护当前代和 Reader 租约，拒绝带路径分隔符的名称。WAL 在首帧写入前校验全部记录的编码体长度，写入与回放共用 256 MiB 上限。
- SDK 维度迁移保留单字段和组合唯一约束。快照校验与恢复核对主文件类型、元素宽度及向量块长度，发现清单类型错配时拒绝发布目标目录。
- Node 保留 Payload 安全整数的整数编码，CAS 对完整 JSON 结构中的数字按精确数值比较。Python 和 Node 返回存储边权的精确扩展值。TQL 浮点 `SUM` 在中间溢出后重算有限总和。

---

## English

Subsequent development builds on `0.9.2-beta.1`. Delivered changes are recorded in the [bilingual release notes](0.9.2-beta.1.md).

- Generation reclamation protects the current directory and active reader leases by resolved directory identity. Names containing path separators are rejected. WAL writes check every encoded record before the first frame and share the 256 MiB limit with replay.
- SDK dimension migration preserves single-field and composite unique constraints. Snapshot verification and restore check the main file's dtype, element width, and vector extent. A manifest dtype mismatch prevents the restored directory from being published.
- Node encodes safe integral Payload numbers as integers. Payload CAS compares numbers exactly throughout the JSON structure. Python and Node widen stored edge weights without rounding. TQL floating-point `SUM` recomputes after intermediate overflow to preserve finite totals.
