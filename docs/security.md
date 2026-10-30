# TriviumDB 安全特性详解

> 本文说明当前内核的并发、数据完整性、格式校验、资源预算、ReadOnly/Immutable、输入验证与跨平台 I/O 实现。当前版本见[变更说明](changes/0.9.0.md)，后续更新见[未发布变更](changes/unreleased.md)。

---

## 目录

- [并发安全模型](#并发安全模型)
- [数据完整性保障](#数据完整性保障)
- [内存安全与 unsafe 边界](#内存安全与-unsafe-边界)
- [资源配额与输入验证](#资源配额与输入验证)
- [跨平台 I/O 加固](#跨平台-io-加固windows-兼容性)

---

## 并发安全模型

### 1. 进程级互斥锁（文件锁）

**实现位置**：[数据库打开与文件锁](../src/database/mod.rs)

```rust
let lock_file = std::fs::OpenOptions::new()
    .create(true).write(true)
    .open(&lock_path)?;
lock_file.try_lock_exclusive().map_err(|_| {
    TriviumError::DatabaseLocked(format!(
        "数据库锁冲突 (Database lock conflict): {path}"
    ))
})?;
```

数据库先将现有主文件或其父目录规范化为绝对路径，再从该规范身份派生 `<db_path>.lock`。Writer 通过 `try_lock_exclusive` 获取排他锁，ReadOnly 通过 `try_lock_shared` 获取共享锁，Immutable 验证代际后直接打开。文件锁由共享所有权句柄持有；显式关闭成功时释放，进行中的异步关闭继续持有它直到工作完成。

**保证**：
- 同一数据库文件**不可能**被两个进程同时写入
- 相对路径、绝对路径、`..` 和符号链接别名不能生成独立锁身份
- Unix 检测到主数据库硬链接时拒绝打开，避免同一 inode 搭配不同 sidecar/WAL/锁前缀
- 锁文件在进程异常退出后由 OS 自动释放（不同于 PID 文件，不会产生死锁残留）
- 在 Linux/macOS（`flock`）和 Windows（`LockFileEx`）上均有效

不得删除仍可能被进程持有的 `.lock` 文件。Unix advisory lock 绑定 inode，删除活跃锁文件后重建同名文件可能绕过原锁。

---

### 2. 线程级 `Arc<RwLock<MemTable>>` / `Arc<Mutex<Wal>>` + 中毒恢复

`MemTable` 使用 `RwLock`：普通无状态查询共享读锁，写入、flush 和 compaction 获取写锁；`Wal` 使用独立 `Mutex`。两类锁均通过恢复封装处理 poison，避免单线程 panic 直接拖垮进程。

完整查询 pipeline 持同一个 MemTable read guard，候选生成、Payload 过滤、QuIVer 精排和结果组装观察同一逻辑版本。开启 fatigue 的有状态查询额外经过 `stateful_search` 串行锁。Hook 在读锁作用域内同线程重入写 API 时返回结构化错误。

锁顺序不变量：

```text
有状态查询：stateful_search → MemTable read
持久化写入：MemTable write → WAL Mutex
QuIVer 构建：不得持有 MemTable guard 等待 singleflight Condvar
```

确定性交错测试验证了读读并发、读写隔离、QuIVer stale generation 拒绝、singleflight、fatigue 顺序和 compaction/WAL 边界。

确定性交错用例通过 `test-hooks` 启用，CI 和发布验收显式执行。TSan 检查 Linux 并发、实例生命周期与维护调度路径；Miri 执行裸指针和邻接表的内存安全子集。命令和执行范围见[测试实践](testing.md#threadsanitizer-与-miri)。

---

### 3. WAL 写入的锁隔离策略

**实现位置**：[直写入口](../src/database/mod.rs)与[事务提交](../src/database/transaction.rs)

写操作保持固定顺序 `MemTable write → WAL Mutex`。事务在持有 MemTable 写锁时完成 Dry-Run、容量预算和整批预留，随后获取 WAL 锁一次性追加事务，最后执行已验证的内存应用。Compaction 同样遵循该顺序，并在主数据保存成功后才清理 WAL。

**保证**：
- 所有多锁路径遵守同一方向，禁止反向获取；
- 容量、维度、ID、Payload 等错误发生在 WAL 之前；
- WAL 成功后内存应用只执行已经 Dry-Run 验证的操作。

数据库关闭使用 `Open → Closing → Closed` 状态机和活动操作 RAII 计数。进入 `Closing` 后新操作返回 `DatabaseClosed`。同步 `close()` 等待已有操作结束并完成持久化；失败时恢复 `Open` 并保留文件锁，成功时释放锁与配额。

受控关闭通过 `request_shutdown()` 登记排空与保存任务，`close_timeout()` 在期限内等待，`shutdown_status()` 提供阶段和结构化故障。等待超时后，任务继续持有数据、配额和文件锁。异步保存失败进入 `failed` 状态，调用方取得原始错误后可发起重试。详见[关闭协议](lifecycle-snapshots.md#关闭协议)。

---

### 4. 共享维护调度

**实现位置**：`storage/compaction.rs`

全进程共享维护计时线程，实际任务进入数据库所属的 `ResourceRuntime` 工作池。周期压实尝试获取 MemTable 写锁；锁忙或准入失败时延期。保存阶段持有写锁，成功后按固定顺序取得 WAL 锁并清理日志。已排空的关闭任务优先于周期压实。

维护根任务与前台根任务单独计数，工作区在保存前预留。每个维护登记记录成功、失败、延期次数以及最近原因。实例诊断通过尝试获取 MemTable 读锁采样内存，写锁占用时报告采样暂不可用。

---

## 数据完整性保障

### 文本 sidecar 的完整性与恢复边界

默认延期加载时保存原有 `.text` 内容；显式修改先验证文本长度、CRC 与完整快照格式。历史快照缺少元数据时，可通过完整解码验证恢复正文与关键词。延期加载和重建工作区同时执行实例预算与共享运行时准入检查。读取异常保留原文件，Rust、Python、Node 提供完整正文与关键词集合重建接口。

检查点将主数据、文本、属性和图索引纳入同一次文件组发布与恢复。文本登记通过检查点持久化；节点、向量、Payload 与业务边通过事务 WAL 确认。应用保存完整正文与别名来源，可在需要时调用 `rebuild_text_index` 重新生成文本索引。

### 5. WAL CRC32 逐条校验

**实现位置**：`storage/wal.rs`（写入），`storage/wal.rs`（读取）

每条 WAL 记录的磁盘格式为：

```
[magic: "TVWL"] [version: u16]
[len: u32 (4B)] [bincode 序列化数据: len bytes] [crc32: u32 (4B)]
```

当前 WAL 使用 `TVWL + v3` 头。ReadWrite 持有排他锁时，可完整校验 v2 的帧、CRC 和记录格式，经临时文件、fsync 与原子替换迁移到 v3；Link 记录补齐 metadata。当前版本的开库检查读取固定六字节头部，恢复按帧流式读取和回放。ReadOnly/Immutable 保持物理零写，旧格式迁移由 Writer 完成。无头、未知版本与非法迁移输入返回结构化错误，并保留原始 WAL。

写入时计算，读取（崩溃恢复）时验证：

```rust
// 写入
let checksum = crc32fast::hash(&data);
writer.write_all(&checksum.to_le_bytes())?;

// 恢复
let computed_crc = crc32fast::hash(&data);
if stored_crc != computed_crc {
    return Err(TriviumError::CorruptedFile(
        "WAL 完整帧 CRC 不匹配 (Complete WAL frame CRC mismatch)".into(),
    ));
}
```

完整帧的 CRC 或反序列化校验失败、完整长度字段超过单帧上限时，打开返回 `CorruptedFile`，原 WAL 字节保持完整。只有物理未写完的长度、数据或 CRC 尾字段，以及缺少提交封条的尾事务，按已完成提交边界恢复。故障日志可随备份保留供诊断；从经过验证的完整日志或快照恢复后，再验收已确认记录。

---

### 6. WAL 单条记录大小上界检查

WAL 回放在帧与 CRC 校验之后继续验证业务内容：Payload 与边元数据必须是可解码的 JSON，向量满足数据库维度和有限数值要求，插入 ID 位于 `1..u64::MAX`。这些检查先于检查点幂等跳过；语义非法记录返回 `CorruptedFile`。合法 `null` 按 JSON 值保存。回放或应用失败传播错误，原 WAL 保留供诊断与重试；未提交事务由封条过滤，不参与业务回放。

**实现位置**：`storage/wal.rs`

```rust
// 单条不超过 256MB
if len > 256 * 1024 * 1024 {
    break; // 损坏的 len 字段
}
```

防止损坏的 `len` 字段触发天量内存分配（OOM 或 DoS）。

---

### 7. WAL 事务原子性（TxBegin / TxCommit 封条）

**实现位置**：`storage/wal.rs`

崩溃恢复时对事务做两阶段过滤：只有见到匹配 `TxCommit` 的事务才会回放，否则整体丢弃：

| 状态 | 行为 |
|---|---|
| TxBegin + 匹配 TxCommit | 全量回放 |
| TxBegin，无 TxCommit（掉电） | **整体丢弃，并物理截断 WAL 尾部（Partial Truncation）** |
| 有当前版本头的独立操作 | 作为当前非事务 CRUD 记录直接回放，推进安全游标 |

**提交边界与物理截断**

恢复逐帧验证并复用字节缓冲，只在匹配事务封条后交付该批操作，临时事务集合随当前待提交批次增长。已完成的事务与独立操作推进 `safe_commit_offset`。读取和回放均成功后，WAL 截断到最后确认边界，再接受后续追加；I/O 或回放错误保留原 WAL 供处理后重试。

---

### 8. Mmap 三文件一致性标记（`.flush_ok` v3）

**实现位置**：`storage/file_format.rs`（写与读取）

Mmap 模式下，主数据、向量、generation 化 Payload 及索引文件均同步后，原子发布 `.flush_ok`。该标记校验 `.tdb`、`.vec` 和 `.pld.<generation>` 的代际与完整性。v3 标记共 53 字节：

```text
magic(4) + version(1) + generation(8)
+ tdb_size(8) + vec_size(8) + payload_size(8)
+ tdb_crc32(4) + vec_crc32(4) + payload_crc32(4) + marker_crc32(4)
```

- 三个文件 CRC 可发现等长位翻转和扇区撕裂；`marker_crc32` 保护提交标记本身。
- 只有 v3 marker 明确声明的 Payload generation 才允许加载；v1/v2 marker 旁的游离 `.pld` 不参与旧快照。
- v1（29 字节）与 v2（41 字节）保持只读兼容；ReadWrite 在下一次显式 flush 时升级为 v3，打开时不隐式迁移。
- generation 化 Payload 文件避免 Windows 上替换仍被 Reader mmap 的同名文件；marker 提交后旧 generation 才失去可见性，并在引用释放后 best-effort 回收。
- 发布前流式保存旧文件组，随后同步并发布 `.publish` 撤销日志。当前 `TPUB` v2 日志为 136 字节，记录主文件、向量、marker、QuIVer 及其元数据、文本及其元数据、属性和图索引的存在状态、长度、CRC、代际、存储模式和提交状态，并校验日志自身 CRC。读取同时支持历史 v1 日志。延期保存且正文保持原样时，可保留原文本文件并省去正文备份复制。
- Mmap 在全部索引同步后发布 marker；Rom 在文件组同步后原子更新日志提交状态，并记录新主文件的长度与 CRC。ReadWrite 打开时检查提交点：未提交则校验备份、恢复旧文件组并回放 WAL；已提交则验证新代并清理日志。恢复复制保留备份，全部恢复文件同步后才清理日志及备份，支持恢复途中再次中断。
- ReadOnly/Immutable 不执行恢复或清理；有待恢复日志/WAL 时返回 `RecoveryRequired`，保持字节级零写。无有效恢复基础的文件损坏返回 `CorruptedFile`，不构造缺向量、缺 Payload 或空库的伪恢复状态；已提交新代损坏不能回滚旧代。
- 旧代备份消耗纳入发布的文件组大小所对应的磁盘读写和临时空间，复制使用有界缓冲。映射有效时，同一句柄失败重试保留最初备份；向量映射在替换过程中失效时，实例进入待恢复状态，共享查询句柄统一拒绝后续操作。释放实例后由可写打开恢复文件组并回放 WAL。主文件使用 v10、marker 使用 v3；待恢复发布由支持对应日志版本的内核完成。
- 默认 `storage::publication_recovery` 用独立日志字节 oracle 验证恢复、损坏拒绝、提交点与只读零写；`test-hooks` 强杀矩阵要求完整恢复所有 `Full` 模式已同步事务，打开失败不能算通过。
- 唯一约束创建/删除持有写锁完成文件组检查点。发布失败隔离实例及共享查询句柄，保留原始错误；释放后可写重开按实际提交点恢复约束与 WAL。预检拒绝保持当前实例可用。
- `.pidx v8` 以 JSON 数组保存复合字段列表；历史复合 posting 通过事实扫描提供结果，唯一约束以主文件定义重建。`.gidx v3` 在采用前完成边记录、反向/标签目录和主文件事实的逐项验证；验证与 JSON 解码峰值参与加载预算准入，异常副本返回结构化损坏错误。

---

### 9. 文件魔数与最小尺寸校验

**实现位置**：`storage/file_format.rs`

载入先检查历史公共头的 58 字节、`TVDB` 魔数与支持的版本范围。v10 随后校验完整的 74 字节类型头：`VTYP`、dtype、元素宽度、reserved 和头部 CRC32。Rom 的向量块止于 `edge_offset`，Mmap 的 `.vec` 长度与 slot 数、维度、元素宽度精确对应。

ReadWrite/ReadOnly 类型不匹配返回 `DtypeMismatch`，并保留 `stored`、`requested` 字段。发布恢复先验证可信备份的类型，再复制正式文件；Readonly 路径保持原文件组。历史仅 WAL 或无类型空检查点附带非空 WAL 的数据，先用原版本恢复并完成检查点后升级。

---

### 10. 文件组原子发布

**实现位置**：[文件格式](../src/storage/file_format.rs)、[发布恢复](../src/storage/file_format/publication.rs)与[文件系统操作](../src/storage/fs.rs)。

发布顺序为临时文件写入、文件同步、原子替换、目录同步及最后的提交标记。发布日志记录文件组替换进度。重新打开时，恢复流程按提交点选择完整 generation；只读打开遇到待恢复发布时返回结构化错误。备份和复制使用[一致快照](lifecycle-snapshots.md#一致快照)接口捕获完整文件组。

---

### 11. WAL 三级落盘模式

**实现位置**：`storage/wal.rs`

```rust
pub enum SyncMode {
    Full,    // 每条后 fsync — 防 OS 崩溃，金融级
    Normal,  // 每条后 flush 到 OS 缓冲 — 防进程崩溃（默认）
    Off,     // 不主动 flush — 仅测试
}
```

---

### 12. WAL Drop 安全刷盘

**实现位置**：`storage/wal.rs`

`Database::drop` 时调用 `flush_writer()`，将 `BufWriter` 缓冲区主动刷入磁盘，防止正常退出时因 `Arc<Mutex<Wal>>` 析构链导致的静默数据丢失。

---

### 目录快照发布与恢复

独立目录快照在源库写锁下捕获已持久化代际，复制到同父目录的隐藏暂存目录，逐文件同步并校验 manifest，随后通过目录 rename 和父目录同步发布。恢复采用新的目标目录；目标已存在时返回专用错误。发布后父目录同步失败时保留完整目标，并返回发布状态不确定错误供调用方校验。文件集合、长度、CRC 和主文件头共同参与验证。协议见[一致快照](lifecycle-snapshots.md#一致快照)。

---

## 内存安全与 unsafe 边界

### 13. mmap 的 unsafe 安全契约

**实现位置**：`storage/vec_pool.rs:open()` 和 `flush_append()`

```rust
// SAFETY: MAP_PRIVATE (copy-on-write)
//   - VectorType 要求 T: Pod + Zeroable，字节对齐和全零初始化安全
//   - len 由 expected_count * dim * size_of::<T>() 精确计算，不超出文件大小
let mmap = unsafe {
    memmap2::MmapOptions::new().len(expected_size).map_copy(&file)?
};
```

`MAP_PRIVATE` 映射：写入只产生进程私有 COW 页，不影响底层文件，其他进程/映射不受影响。

---

### 14. mmap 字节到 `&[T]` 转换安全性

**实现位置**：`storage/vec_pool.rs:get()` 和 `rebuild_merged_cache()`

```rust
let ptr = bytes.as_ptr();
if (ptr as usize) % std::mem::align_of::<T>() == 0 {
    unsafe { std::slice::from_raw_parts(ptr as *const T, self.dim) }
} else {
    // 非对齐：bytemuck::pod_read_unaligned 安全回退
}
```

mmap 返回的地址始终页对齐（4096B）；`f32` 需 4B 对齐、`u64` 需 8B 对齐，均严格满足。代码中有运行时对齐检查和安全回退路径。

---

### 15. AVX2 SIMD 运行时 CPU 特性检测

**实现位置**：`vector.rs`

```rust
if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
    // SAFETY: 运行时已确认 CPU 支持
    return unsafe { cosine_similarity_avx2(a, b) };
}
cosine_similarity_scalar(a, b) // 安全标量回退
```

不支持 AVX2 的 CPU 自动回退到纯 Rust 路径，**不会执行非法指令**。

---

### 16. `bytemuck::Pod` 编译期内存安全约束

**实现位置**：`vector.rs`

```rust
pub trait VectorType: bytemuck::Zeroable + bytemuck::Pod + ...
```

`Pod`（Plain Old Data）是编译期保证：无指针、无引用、无 padding 歧义、全零合法。使得 `bytemuck::cast_slice` 在编译期被证明安全，完全不需要运行时 unsafe。

---

### 17. 数组边界守卫

**实现位置**：[分段向量池](../src/storage/vec_pool.rs)、[检索与精排](../src/database/pipeline.rs)。

向量池按 slot 和维度检查索引范围，并通过分段访问接口返回完整切片。mmap 基础层与增量层分别校验长度；精排通过同一访问接口读取向量。损坏的文件几何在打开阶段返回结构化错误。

---

## 资源配额与输入验证

### 18. 四维查询预算与原子预留

`Config.expected_nodes`、`Database::reserve_nodes()`、事务和绑定层 `batch_insert` 共用容量防御：

1. 使用 checked arithmetic 估算向量 delta、Payload/ID HashMap、slot 和 fast tag 的新增容量；
2. 与当前内核估算内存及 `memory_limit` 比较；
3. 超预算、溢出或 `try_reserve` 失败时返回结构化错误；
4. 只有预留和全部 Dry-Run 验证成功后才写 WAL。

逻辑原子性保证节点、ID、generation 和 WAL 在失败后保持原值。预留过程中已经取得的空 capacity 可能保留供后续操作使用。内核按实例内存估算和已接入工作区执行预算检查；进程容量评估另行记录 Python/NumPy、V8、其他线程、mmap 驻留与页缓存开销。

共享 `ResourceRuntime` 在数据库打开时登记声明配额，并在操作前预留前台或维护工作区。维护保护额度属于总预算。调整配额失败保留原值，成功关闭归还本实例登记。预算错误包含 `resource/requested/used/limit`，Python 异常与 Node 同步/异步错误提供对应字段。配置与统计见[共享资源运行时](resource-runtime.md)。

QuIVer 构建有独立峰值预算，流式准备 BQ2 与默认 SM 状态，避免 FP16 全库临时展开为 FP32。默认查询的紧凑状态与显式模式安装的临时峰值应分别核算，详见 [QuIVer 内存与重载说明](quiver.md#增量维护重载与内存)。纯 `flush()` 和 compaction 不会隐式触发 ANN 构建。

构图连通性保护的父节点数组、预分配遍历栈和访问位图在分配前纳入峰值预算；父节点数组也计入索引热内存。重载不补拓扑：旧图通过重建应用新规则，断连图按已有缺失索引策略处理。

查询统一受四类预算约束：QueryMemoryBudget（候选/union/精排向量/页读取字节）、TraversalBudget（visited/edges/frontier/depth）、PipelineBudget（NodeSet 节点与字节）和 QueryParallelismBudget（线程与并行阈值）。分配前检查、按阶段切片；Error 模式 fail-closed，只有显式 Partial 策略才允许带 truncated 标记返回。GraphFirst、路径、集合、图算法和聚合不得偷偷退化为无界全库执行。

---

### 19. TQL 中间结果与物化预算

**实现位置**：[TQL 执行器](../src/query/tql_executor.rs)、[物理管线](../src/query/pipeline.rs)与[返回处理](../src/query/tql_return.rs)。

候选阶段使用 NodeId、分数和按需保存的类型化绑定。WITH 在投影阶段捕获逐行值，EXPAND 保留源绑定；工作区准入同时计入绑定容器和动态内容。最终投影按表达式读取所需 Payload、向量或实体，行上限、Payload 读取和中间 NodeSet 分别按预算检查。

---

### 向量维度强校验

**实现位置**：[MemTable](../src/storage/memtable.rs)与[事务预检](../src/database/transaction.rs)

```rust
if vector.len() != self.dim {
    return Err(TriviumError::DimensionMismatch { expected: dim, got: vector.len() });
}
```

所有写入向量路径（`insert`、`insert_with_id`、`update_vector`、事务 Dry-Run）均强校验维度，返回类型化错误而非 panic。

---

### 20. 查询向量 NaN / Infinity 检测

**实现位置**：[查询向量校验](../src/database/pipeline.rs)、[搜索句柄](../src/database/mod.rs)

```rust
for item in qv {
    let f = item.to_f32();
    if f.is_nan() || f.is_infinite() {
        return Err(TriviumError::InvalidVector {
            reason: "查询向量含非有限值 (Query vector contains non-finite values)".into(),
        });
    }
}
```

NaN 进入余弦计算会传染整个结果。在检索管线 L0 层直接拦截，防止毒素向量污染后续所有计算。

---

### 21. 认知管线参数钳位（防数学奇点）

**实现位置**：[检索配置校验与规范化](../src/database/pipeline.rs)

```rust
config.top_k              = config.top_k.max(1);
config.fista_lambda       = config.fista_lambda.clamp(1e-5, 100.0);
config.teleport_alpha     = config.teleport_alpha.clamp(0.0, 1.0);
config.dpp_quality_weight = config.dpp_quality_weight.clamp(0.0, 10.0);
```

所有数学参数强制钳入合法范围：`fista_lambda` 过大全变 0，`teleport_alpha` 超出 `[0,1]` PPR 概率失去意义，`dpp_quality_weight` 过大导致 float 溢出。

---

### 22. 事务 Dry-Run 预检与极巨载荷拦截（零损伤回滚）

**实现位置**：[事务提交预检](../src/database/transaction.rs)与[直写 API](../src/database/mod.rs)

所有业务验证（节点是否存在、ID 是否冲突、维度是否匹配、是否包含 NaN/Inf）在**纯内存虚拟状态**上完成，不触碰 MemTable 或 WAL。

**防OOM极巨载荷拦截 (Payload Limiting)**：为防止恶意构造或意外产生的数以百兆计的超大 Payload 文本占用物理内存和撑爆日志，`Database::insert`、`update_payload` 和 `Transaction::commit` 会强制性对 JSON 载荷施加 **8MB 大小限制** (`MAX_PAYLOAD_SIZE`)。超越此数值的数据写入将被直接判定失败，保护操作系统的内存水位和文件视窗。

预检按缓冲操作顺序更新每个 ID 的存在状态，覆盖删除、重建、修改和再次删除。JSON 在 WAL 前检查可恢复性；预检失败时节点、索引、ID 分配状态和 WAL 保持原样。容量预留完成后先提交 WAL，再应用已校验的原始值并同步索引；WAL I/O 失败向调用方传播原始错误，该批内存操作尚未应用。

---

### 23. Tombstone 节点的防误更新

**实现位置**：`storage/memtable.rs:update_vector()`

```rust
// 必须检查 payload 存在性，而非 ids_to_indices
// delete() 移除 payload 但 ids_to_indices 中的槽位仍存在（指向已置零位置）
if !self.payloads.contains_key(&id) {
    return Err(TriviumError::NodeNotFound(id));
}
```

防止对已逻辑删除的节点进行向量更新。

---

### 24. QuIVer 索引与 FreeList 墓碑复用（零 Ghost Node）

**实现位置**：[节点删除](../src/database/mod.rs)、[MemTable](../src/storage/memtable.rs)、[QuIVer](../src/index/quiver.rs)

传统图索引（如 HNSW）在节点被逻辑删除后，会在索引图中留下幽灵引用，导致检索结果污染和性能退化。TriviumDB 通过 **FreeList 墓碑隐式复用机制** + **QuIVer 增量图维护** 彻底消除了该问题：

- **原位物理擦除**：无需进行极其耗时的全局紧凑，已删除的 `index` 将被推入快速复用队列。下一个插入的节点直接占据其所在的物理行。
- **并行特征网同步清零**：删除节点时，对应的 `fast_tags` 位特征槽立即置零，杜绝废弃指纹被误读；
- **QuIVer 增量同步**：QuIVer 图索引支持 Tombstone 软删除，删除比例达 25% 时自动触发重建，保证图结构质量。

**保证**：删除操作对检索质量零副作用，无空间碎片，无需用户手动触发重建，不仅杜绝了 Ghost Node 幽灵节点，更实现了无限频次改写下的 `O(1)` 平均生命周期开销！

---

### 25. 图谱扩散防爆炸截断

**实现位置**：`graph/traversal.rs`

```rust
// 能量阈值守护：得分 ≤ 0 的节点不再传播（防负反馈循环）
next_tier.retain(|_, energy| *energy > 0.0);

// 侧向抑制 Top-K（防稠密图 OOM）
if lateral_inhibition_threshold > 0 && next_tier.len() > lateral_inhibition_threshold {
    sorted_tier.truncate(lateral_inhibition_threshold);
}

if next_tier.is_empty() { break; } // 能量衰竭，提前终止
```

两道独立截断：能量守护防负反馈循环，侧向抑制 Top-K 防稠密图爆炸性展开导致 OOM。

---

## 跨平台 I/O 加固（Windows 兼容性）

### 26. mmap 释放先于 rename（P0 修复）

**实现位置**：`storage/vec_pool.rs:flush_rewrite()` 和 `flush_append()`

```rust
self.mmap = None;           // 先解除内核映射锁
robust_rename(&tmp, dst)?;  // 再执行原子替换
```

Windows 强制锁定语义：映射存活时 rename 目标文件必定 `ERROR_ACCESS_DENIED`。先 Drop mmap，COW 私有脏页安全丢弃（数据已写入 .tmp）。

---

### 27. 杀毒软件瞬态锁定重试（robust_rename）

**实现位置**：`storage/file_format.rs` 和 `storage/vec_pool.rs`

Windows 杀毒软件在文件关闭瞬间抢占扫描，通常几毫秒后自动释放。实现指数退避重试（1→2→4→…→50ms，最多 10 次）仅针对 `ERROR_ACCESS_DENIED(5)` 和 `ERROR_SHARING_VIOLATION(32)`，其他错误立即快速失败。非 Windows 平台编译为直接调用 `std::fs::rename`，零开销。

---

### 28. WAL 清空使用 truncate 而非 remove+create

**实现位置**：`storage/wal.rs`

`truncate(true)` 保留 inode，不触发杀软的"新文件扫描"；截断后立即写回固定 6 字节 WAL 版本头，避免 WAL clear 期间再次产生文件锁冲突。

---

### 29. OS 页面缓存的安全收回 `madvise`

**实现位置**：`storage/vec_pool.rs:advise_dontneed()`

```rust
// 通知 OS 立即回收刚刚写入磁盘的高维物理页，阻止污染 VFS 文件缓存
#[cfg(target_os = "linux")]
libc::madvise(ptr, len, libc::MADV_DONTNEED);
```
对于数十 GB 的向量基库，单纯依靠 OS 自我调节 LRU 会引发主机端周期性严重卡顿（Threshing）。引擎使用安全封装的非阻塞 FFI 建议系统，配合 Windows 的 `VirtualUnlock` 提供安全回收，以极低的成本维持了 60 帧 0 卡顿的主机交互体验。

---

## API 迁移与统一错误

历史静默入口已移除：

- `tql_mut()` 收到读查询返回 `ApiMigrationRequired`；
- `patch_payload()` 普通对象不再等价于 `$set`；
- Node 数字位置参数构造器返回 `TDB_API_MIGRATION_REQUIRED`；
- Rust `Database::open_with_sync()` 已删除；
- 无头 WAL 返回 `UnsupportedWalVersion`。

Python/Node 绑定应通过中央转换器保留核心错误类别；控制流依赖枚举或稳定 code，不匹配中英文错误字符串。QuIVer profiler/连通性信息统一走 `tracing`，不直接写 stderr。

## 生产路径零 panic 与故障注入验证

生产可达的 `panic!` / `unreachable!` 已全部消除：解析器枚举转换返回 parse error、执行器聚合分支返回 execution error、mmap 对齐在打开阶段返回结构化错误，内部不变量使用显式校验而非 `unwrap/expect`。静态门禁防止生产代码重新引入 `panic!/unreachable!`。

故障注入均使用小型 fixture 与隔离子进程，不会耗尽测试机的物理内存或磁盘：

- **发布阶段强杀矩阵**：启用 `test-hooks` 后，首次 flush、追加与 2048×65 更新/删除/compaction 在备份、向量、主文件、marker 和恢复途中由父进程强杀；必须重开并逐项验证全部已同步事务，再 flush/reopen 验证持久化，不能以拒绝打开通过验收。这是进程中断模拟，不等同于真实物理断电；
- **确定性 I/O failpoint**：在 Create/Write/Sync/Rename 等定点注入失败，验证临时文件清理与错误传播；
- **allocator failure**：仅在目标 failpoint 后拒绝分配，验证 `try_reserve` 路径的结构化错误与零部分提交；
- **格式规格测试**：独立于生产解析器的字段规格 + 结构化 mutation（字段边界/截断/位翻转/CRC 修复），并验证 ReadOnly 对任何损坏输入字节级零写。

## FFI Hook 插件安全

### 30. FfiHook 动态库加载的安全边界

**实现位置**：`hook.rs:FfiHook::load()`

`FfiHook` 允许在运行时加载 C/C++ 动态库（`.so` / `.dll` / `.dylib`）作为检索管线的自定义扩展。这是一个**有意设计的安全边界开放点**，需要用户明确理解其风险。

**威胁模型**：

| 风险 | 等级 | 说明 |
|------|------|------|
| 任意代码执行 | 🔴 高 | 动态库内的代码在进程内执行，拥有与宿主进程完全相同的权限 |
| 堆破坏 / 段错误 | 🔴 高 | C/C++ 插件的内存错误可导致宿主进程崩溃 |
| 数据窃取 | 🟡 中 | 插件可读取进程内存中的任何数据（向量、payload 等） |
| 死锁 | 🟡 中 | 插件在 Hook 回调中不当使用锁可能导致死锁 |

**缓解措施**：

1. **符号可选加载 + ABI 版本门禁**：`FfiHook` 使用 `libloading::Library::get()` 按名称查找符号。未找到的符号静默降级为 NoopHook，**不会因缺少符号而崩溃**。导出 `trivium_hook_invoke_v2` 六阶段入口的插件必须同时导出返回 ABI 版本的 `trivium_hook_abi_version`，与内核期望的 `FFI_HOOK_ABI_VERSION = 2` 不一致时明确拒绝加载，避免跨 ABI 误读内存。

2. **调用隔离**：所有 FFI 回调在 Rust 侧包装，返回值经过有效性检查后才被消费。C 侧返回的 `null` 指针会被安全处理。

3. **库生命周期**：`FfiHook` 持有 `libloading::Library` 的所有权，清除、替换 Hook 或成功关闭数据库后，在最后一个 Hook 所有者释放时卸载动态库；关闭失败保留原 Hook。已进入的查询持有 Hook，保证调用期间动态库有效。

**使用建议**：

```python
# ⚠️ 仅加载来源可信的动态库
db.load_ffi_hook("./libmy_verified_plugin.so")

# ✅ 不需要时及时清除
db.clear_hook()
```

> ⚠️ **安全警告**：`load_ffi_hook()` 加载的动态库将在进程内执行**任意原生代码**。请确保：
> 1. 动态库来自可信来源或经过安全审计
> 2. 在生产环境中不要加载用户提交的未经验证的动态库
> 3. 建议在沙箱/容器环境中隔离使用 FFI Hook

---

### 31. Hook 回调的线程安全约束

**实现位置**：`hook.rs:trait SearchHook: Send + Sync + Any`

`SearchHook` trait 要求实现 `Send + Sync`，这是编译器层面的强制约束。任何自定义 Hook 实现如果包含非线程安全的内部状态，编译器将直接拒绝编译。

**保证**：
- `NoopHook`：零状态，天然线程安全
- `CompositeHook`：通过 `Vec<Arc<dyn SearchHook>>` 持有子 Hook，Arc 自动保证线程安全
- `FfiHook`：`libloading::Library` 是 `Send + Sync` 的，函数指针无状态

> 💡 如果你的 Rust 自定义 Hook 需要内部可变状态，请使用 `Mutex<T>` 或 `RwLock<T>` 包装。

---

### 32. HookContext 的数据隔离

**实现位置**：`hook.rs:HookContext`

每次 `search_hybrid_with_context()` 调用创建一个独立的 `HookContext` 实例，不在多次查询之间共享。Hook 注入的 `custom_data` 和计时统计在查询结束后随 `HookContext` 一并返回给调用方，**不残留在引擎内部状态中**。

**保证**：
- 不同查询之间的 Hook 状态完全隔离
- Hook 无法通过 `HookContext` 修改引擎的持久化状态
- `abort` 标志仅影响当前查询的管线执行，不影响后续查询

---

## 附录：unsafe 使用汇总

| 位置 | unsafe 操作 | 安全契约 |
|---|---|---|
| `storage/vec_pool.rs` | mmap、裸切片与页面建议 | 文件/映射生命周期、长度与对齐、映射访问边界 |
| `storage/file_format.rs`、`graph_blocks.rs`、`payload_store.rs`、`index/property.rs` | 只读映射或字节转换 | 格式几何与完整性校验、对齐、引用寿命和发布顺序 |
| `index/bq.rs` | 标量裸指针、SIMD 与预取 | 有效行偏移、完整 chunk/尾块边界、指令集与分配长度 |
| `vector.rs`、`index/quiver/corrected_sm*` | SIMD 加载及计算 | 指令集分派、长度/尾部、对齐与浮点数值边界 |
| `index/quiver.rs` | UnsafeCell 邻接表、手工 Sync、其他并行构图代码 | 行锁与访问范围、别名约束、并行阶段结束后的冻结 |
| `hook.rs` | 外部函数调用与手工 Send/Sync | ABI、动态库与回调寿命、跨线程调用约束 |
| `bindings/python.rs`、`bindings/nodejs.rs` | 回调包装的手工 Send/Sync | 对象跨线程访问及 Python/Node 运行时约束 |
| `observability.rs` | Windows 外部系统调用 | API 参数、缓冲区布局与返回值检查 |

上表列出各模块的审核范围与安全契约。手工 `Send/Sync` 按内部访问、所有权和同步条件逐项审核。`unsafe_audit` 执行专项回归，ASan、TSan、Miri 按各自范围运行；验收记录关联具体提交、平台和实际执行路径。


