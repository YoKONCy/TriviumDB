# TriviumDB 安全特性详解

> 基于 v0.8.4 当前源码的完整文档，覆盖并发安全、数据完整性、格式版本门禁、预算、ReadOnly/Immutable 零写、输入验证与跨平台 I/O 加固。

---

## 目录

- [并发安全模型](#并发安全模型)
- [数据完整性保障](#数据完整性保障)
- [内存安全与 unsafe 边界](#内存安全与-unsafe-边界)
- [输入验证与参数防御](#输入验证与参数防御)
- [跨平台 IO 加固](#跨平台-io-加固)

---

## 并发安全模型

### 1. 进程级互斥锁（文件锁）

**实现位置**：`database.rs:169-181`

```rust
let lock_file = std::fs::OpenOptions::new()
    .create(true).write(true)
    .open(&lock_path)?;
lock_file.try_lock_exclusive().map_err(|_| {
    TriviumError::Generic(format!(
        "Database '{}' is already opened by another process. ...", path, lock_path
    ))
})?;
```

数据库先将现有主文件或其父目录规范化为绝对路径，再从该规范身份派生 `<db_path>.lock`。Writer 通过 `try_lock_exclusive` 获取排他锁，ReadOnly 通过 `try_lock_shared` 获取共享锁，Immutable 不创建锁文件。锁由 `_lock_file` 持有，`close()` 成功或对象 drop 后释放。

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

完整查询 pipeline 持同一个 MemTable read guard，候选生成、Payload 过滤、QuIVer 精排和结果组装观察同一逻辑版本。开启 fatigue 的有状态查询额外经过 `stateful_search` 串行锁。Hook 在读锁作用域内禁止同线程重入写 API，直接返回错误而不是自锁。

锁顺序不变量：

```text
有状态查询：stateful_search → MemTable read
持久化写入：MemTable write → WAL Mutex
QuIVer 构建：不得持有 MemTable guard 等待 singleflight Condvar
```

确定性交错测试验证了读读并发、读写隔离、QuIVer stale generation 拒绝、singleflight、fatigue 顺序和 compaction/WAL 边界。

---

### 3. WAL 写入的锁隔离策略

**实现位置**：`database.rs:279-293`（以 `insert` 为例）

写操作保持固定顺序 `MemTable write → WAL Mutex`。事务在持有 MemTable 写锁时完成 Dry-Run、容量预算和整批预留，随后获取 WAL 锁一次性追加事务，最后执行已验证的内存应用。Compaction 同样遵循该顺序，并在主数据保存成功后才清理 WAL。

**保证**：
- 所有多锁路径遵守同一方向，禁止反向获取；
- 容量、维度、ID、Payload 等错误发生在 WAL 之前；
- WAL 成功后内存应用只执行已经 Dry-Run 验证的操作。

数据库关闭使用 `Open → Closing → Closed` 状态机和活动操作 RAII 计数。进入 `Closing` 后新操作立即收到 `DatabaseClosed`，close 无超时等待已进入操作结束；最终 flush 失败时恢复 `Open` 并保留文件锁，成功时才释放锁。绑定层不再暴露绕过 flush 的 `release_lock`。

---

### 4. Compaction 线程的并发安全

**实现位置**：`storage/compaction.rs`

后台 Compaction 线程通过 `Arc::clone` 共享 `RwLock<MemTable>` 和 `Mutex<Wal>`。准备和保存阶段获取 MemTable 写锁，因此不会与写入交错；保存成功后在同一锁顺序下清理 WAL。该设计优先保证崩溃一致性，compaction 期间前台读可能等待。

---

## 数据完整性保障

### 5. WAL CRC32 逐条校验

**实现位置**：`storage/wal.rs:113-115`（写入），`wal.rs:232-244`（读取）

每条 WAL 记录的磁盘格式为：

```
[magic: "TVWL"] [version: u16]
[len: u32 (4B)] [bincode 序列化数据: len bytes] [crc32: u32 (4B)]
```

WAL 必须包含 `TVWL + v3` 头。历史无头 WAL 不再自动猜测解析，返回 `UnsupportedWalVersion { found: 0 }`；带记录的旧版本 WAL 也必须使用对应旧内核恢复并 flush。只有恰好 6 字节、零记录的旧版本 WAL 可由持有排他锁的 ReadWrite 打开流程通过临时文件、fsync 与原子替换升级版本头；ReadOnly/Immutable 不执行该升级并保持字节级零写。未知未来版本同样在任何记录回放前拒绝。

写入时计算，读取（崩溃恢复）时验证：

```rust
// 写入
let checksum = crc32fast::hash(&data);
writer.write_all(&checksum.to_le_bytes())?;

// 恢复
let computed_crc = crc32fast::hash(&data);
if stored_crc != computed_crc {
    tracing::error!("WAL CRC mismatch ... Stopping recovery.");
    break; // 停止回放，丢弃后续数据
}
```

**保证**：磁盘坏块、OS 写半条记录均可被检测。CRC 不匹配时**停止回放**而非跳过，防止损坏数据静默渗入。

---

### 6. WAL 单条记录大小上界检查

**实现位置**：`storage/wal.rs:214-217`

```rust
// 单条不超过 256MB
if len > 256 * 1024 * 1024 {
    break; // 损坏的 len 字段
}
```

防止损坏的 `len` 字段触发天量内存分配（OOM 或 DoS）。

---

### 7. WAL 事务原子性（TxBegin / TxCommit 封条）

**实现位置**：`storage/wal.rs:260-297`

崩溃恢复时对事务做两阶段过滤：只有见到匹配 `TxCommit` 的事务才会回放，否则整体丢弃：

| 状态 | 行为 |
|---|---|
| TxBegin + 匹配 TxCommit | 全量回放 |
| TxBegin，无 TxCommit（掉电） | **整体丢弃，并物理截断 WAL 尾部（Partial Truncation）** |
| 有当前版本头的独立操作 | 作为当前非事务 CRUD 记录直接回放，推进安全游标 |

**极限防御：LSN (Log Sequence Number) 与物理截断**
如果在回放时发现未闭合的事务（通常因为机器暴力断电），系统不仅在内存中丢弃它们，还会通过计算**最后一个完美闭环事务的精确物理字节偏移量 (safe_commit_offset)**，在重播前直接调用内置 `set_len()`。这彻底切断了具有传染性的“幽灵尾部”，防止系统下次启动接收正常追加后，由于 `in_tx=true` 的状态污染，将新的健康数据错吞进旧的失效事务里。

**保证**：要么全部回放，要么全部丢弃；不出现"插入了 5 条、应该 10 条"的部分状态。并且绝对防止失效事务封条污染后续追加数据。

---

### 8. Mmap 双文件一致性标记（`.flush_ok` v2）

**实现位置**：`storage/file_format.rs`（写与读取）

Mmap 模式下，`.tdb` 和 `.vec` 均写入成功后，才原子写 `.flush_ok` 标记。v2 标记共 41 字节：

```text
magic(4) + version(1) + generation(8)
+ tdb_size(8) + vec_size(8)
+ tdb_crc32(4) + vec_crc32(4) + marker_crc32(4)
```

- `tdb_crc32` / `vec_crc32` 是 `.tdb` 与 `.vec` 的**整文件 CRC32**，可发现等长位翻转、扇区撕裂等大小校验抓不到的损坏；空 `.vec`（纯 Rom/无向量库）使用明确的空文件 CRC。
- `marker_crc32` 保护标记自身，防止 marker 字节被部分破坏后"恰好看起来合法"。
- v1 标记（29 字节，仅 generation + size）保持有界兼容读取；ReadWrite 在下一次显式 flush 时自然升级为 v2，打开时不做隐式写入。
- 校验失败时**fail-closed**：ReadOnly/Immutable 字节级零写并拒绝打开；ReadWrite 不再降级为"忽略 `.vec` 的零向量骨架恢复"——那会在 WAL 无法完整重建基础向量时产生"节点存在、向量全零"的伪恢复状态。现在统一拒绝打开，由用户从备份或上一代际恢复。
- 发布路径（`.vec` 落盘 → `.tdb` 落盘 → marker 原子替换）由真实子进程强杀测试逐阶段验证：任何阶段断电后，重开只允许"旧完整代际"或"新完整代际"。

---

### 9. 文件魔数与最小尺寸校验

**实现位置**：`storage/file_format.rs:257-267`

```rust
const MAGIC: &[u8; 4] = b"TVDB";

if mmap.len() < HEADER_SIZE as usize {
    return Err(TriviumError::Generic("File too small for header".into()));
}
if &bytes[0..4] != MAGIC {
    return Err(TriviumError::Generic(
        format!("Invalid file magic: expected TVDB, got {:?}", &bytes[0..4])
    ));
}
```

防止加载非 TriviumDB 文件或截断损坏文件，同时避免后续偏移量计算出现越界读取。

---

### 10. 原子写入协议：write-tmp → fsync → rename

**实现位置**：`storage/file_format.rs:142-240`，`storage/vec_pool.rs:flush_rewrite()`

所有持久化路径均遵循：

```
① 写 .tmp 临时文件 → ② sync_all() → ③ robust_rename(tmp → 正式文件)
```

任何步骤崩溃，旧文件完好。`.tmp` 在下次启动时可安全忽略。

---

### 11. WAL 三级落盘模式

**实现位置**：`storage/wal.rs:48-68`

```rust
pub enum SyncMode {
    Full,    // 每条后 fsync — 防 OS 崩溃，金融级
    Normal,  // 每条后 flush 到 OS 缓冲 — 防进程崩溃（默认）
    Off,     // 不主动 flush — 仅测试
}
```

---

### 12. WAL Drop 安全刷盘

**实现位置**：`storage/wal.rs:336-341`

`Database::drop` 时调用 `flush_writer()`，将 `BufWriter` 缓冲区主动刷入磁盘，防止正常退出时因 `Arc<Mutex<Wal>>` 析构链导致的静默数据丢失。

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

**实现位置**：`vector.rs:130-141`

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

**实现位置**：`vector.rs:13-14`

```rust
pub trait VectorType: bytemuck::Zeroable + bytemuck::Pod + ...
```

`Pod`（Plain Old Data）是编译期保证：无指针、无引用、无 padding 歧义、全零合法。使得 `bytemuck::cast_slice` 在编译期被证明安全，完全不需要运行时 unsafe。

---

### 17. 数组边界守卫

**实现位置**：`database.rs`（QuIVer 精排分支）

```rust
if offset + dim <= vectors.len() {
    let score = T::similarity(query_vector, &vectors[offset..offset + dim]);
}
```

访问 `flat_vectors` 时，始终通过 `offset + dim <= len` 守卫，防止 `mmap_count` 与实际数组大小不一致时的越界访问。

---

## 资源配额与恶意负载防御 (Anti-DoS & OOM)

### 18. 四维查询预算与原子预留

`Config.expected_nodes`、`Database::reserve_nodes()`、事务和绑定层 `batch_insert` 共用容量防御：

1. 使用 checked arithmetic 估算向量 delta、Payload/ID HashMap、slot 和 fast tag 的新增容量；
2. 与当前内核估算内存及 `memory_limit` 比较；
3. 超预算、溢出或 `try_reserve` 失败时返回结构化错误；
4. 只有预留和全部 Dry-Run 验证成功后才写 WAL。

逻辑原子性保证节点、ID、generation 和 WAL 在失败后不变。Rust 容器在后续容器失败前已经成功取得的空 capacity 可能保留，但不形成可见数据。内存预算不包含 Python/NumPy、V8、其他线程和 OS Page Cache，因此不是进程 RSS 的硬上限。

QuIVer 构建有独立峰值预算，并采用流式 BQ2 编码，避免 FP16 全库临时展开为 FP32。纯 `flush()` 和 compaction 不会隐式触发 ANN 构建。

查询统一受四类预算约束：QueryMemoryBudget（候选/union/精排向量/页读取字节）、TraversalBudget（visited/edges/frontier/depth）、PipelineBudget（NodeSet 节点与字节）和 QueryParallelismBudget（线程与并行阈值）。分配前检查、按阶段切片；Error 模式 fail-closed，只有显式 Partial 策略才允许带 truncated 标记返回。GraphFirst、路径、集合、图算法和聚合不得偷偷退化为无界全库执行。

---

### 19. Cypher 引擎的 Lazy Evaluation 防 OOM 内存大爆炸

**实现位置**：`query/executor.rs:eval_expr_by_id()` 和路径扩展逻辑

```rust
// 🚀 OOM 防御：我们只在中间计算层存储 NodeId，绝对不克隆包含 Vector 和 Payload 的巨型 Node 结构！
let mut bindings_set: Vec<HashMap<String, u64>> = Vec::new();
```

由于图查询匹配（如 `(a)-[]->(b)-[]->(c)`）会产生爆炸性的笛卡尔积中间结果，若在每一步扩展中传递深拷贝的实体节点数据（包含高维向量、全量 JSON Payload），1 万条路径即会瞬间击穿几个 GB 的内存。
TriviumDB 执行器在整条中间路径遍历中**仅保留轻量级 `u64` IDs**，所有的 `WHERE` 财产过滤条件使用即时查询计算，仅仅在最后的 `RETURN` 吐出最终小切片结果时，才去执行昂贵的 `build_node` 数据装填，物理上隔离了内存爆炸。

---

### 向量维度强校验

**实现位置**：`storage/memtable.rs`，`database.rs:Transaction::commit()`

```rust
if vector.len() != self.dim {
    return Err(TriviumError::DimensionMismatch { expected: dim, got: vector.len() });
}
```

所有写入向量路径（`insert`、`insert_with_id`、`update_vector`、事务 Dry-Run）均强校验维度，返回类型化错误而非 panic。

---

### 20. 查询向量 NaN / Infinity 检测

**实现位置**：`database.rs:462-475`

```rust
for item in qv {
    let f = item.to_f32();
    if f.is_nan() || f.is_infinite() {
        return Err(TriviumError::Generic("Query vector contains NaN or Infinity".into()));
    }
}
```

NaN 进入余弦计算会传染整个结果。在检索管线 L0 层直接拦截，防止毒素向量污染后续所有计算。

---

### 21. 认知管线参数钳位（防数学奇点）

**实现位置**：`database.rs:478-485`

```rust
safe_cfg.top_k              = safe_cfg.top_k.max(1);
safe_cfg.fista_lambda       = safe_cfg.fista_lambda.clamp(1e-5, 100.0);
safe_cfg.teleport_alpha     = safe_cfg.teleport_alpha.clamp(0.0, 1.0);
safe_cfg.dpp_quality_weight = safe_cfg.dpp_quality_weight.clamp(0.0, 10.0);
```

所有数学参数强制钳入合法范围：`fista_lambda` 过大全变 0，`teleport_alpha` 超出 `[0,1]` PPR 概率失去意义，`dpp_quality_weight` 过大导致 float 溢出。

---

### 22. 事务 Dry-Run 预检与极巨载荷拦截（零损伤回滚）

**实现位置**：`database.rs:Transaction::commit()` 第一阶段及其他直写 API

所有业务验证（节点是否存在、ID 是否冲突、维度是否匹配、是否包含 NaN/Inf）在**纯内存虚拟状态**上完成，不触碰 MemTable 或 WAL。

**防OOM极巨载荷拦截 (Payload Limiting)**：为防止恶意构造或意外产生的数以百兆计的超大 Payload 文本占用物理内存和撑爆日志，`Database::insert`、`update_payload` 和 `Transaction::commit` 会强制性对 JSON 载荷施加 **8MB 大小限制** (`MAX_PAYLOAD_SIZE`)。超越此数值的数据写入将被直接判定失败，保护操作系统的内存水位和文件视窗。

任何验证失败直接返回 `Err`，MemTable 和 WAL 零损伤。只有 Dry-Run 全部通过，才进入不可失败的 WAL 写入 → MemTable 应用路径。

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

**实现位置**：`database.rs:delete()`，`storage/memtable.rs`，`index/quiver.rs`

传统图索引（如 HNSW）在节点被逻辑删除后，会在索引图中留下幽灵引用，导致检索结果污染和性能退化。TriviumDB 通过 **FreeList 墓碑隐式复用机制** + **QuIVer 增量图维护** 彻底消除了该问题：

- **原位物理擦除**：无需进行极其耗时的全局紧凑，已删除的 `index` 将被推入快速复用队列。下一个插入的节点直接占据其所在的物理行。
- **并行特征网同步清零**：删除节点时，对应的 `fast_tags` 位特征槽立即置零，杜绝废弃指纹被误读；
- **QuIVer 增量同步**：QuIVer 图索引支持 Tombstone 软删除，删除比例达 25% 时自动触发重建，保证图结构质量。

**保证**：删除操作对检索质量零副作用，无空间碎片，无需用户手动触发重建，不仅杜绝了 Ghost Node 幽灵节点，更实现了无限频次改写下的 `O(1)` 平均生命周期开销！

---

### 25. 图谱扩散防爆炸截断

**实现位置**：`graph/traversal.rs:69-83`

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

**实现位置**：`storage/wal.rs:307-330`

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

- **真实断电矩阵**：子进程在 `.vec` 落盘 / `.tdb` 落盘 / marker 替换前后到达指定发布阶段后由父进程强杀，重开验证只存在旧完整代或新完整代，不允许跨文件混代或半事务；
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

3. **库生命周期**：`FfiHook` 持有 `libloading::Library` 的所有权，`clear_hook()` 或 `Database::drop()` 时自动卸载动态库。不会出现悬垂函数指针。

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

**实现位置**：`hook.rs:trait SearchHook: Send + Sync`

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
| `vec_pool.rs:open()` | `MmapOptions::map_copy()` | T: Pod+Zeroable；MAP_PRIVATE；len ≤ 文件实际大小 |
| `vec_pool.rs:flush_append()` | `MmapOptions::map_copy()` | 同上；重映射前旧 mmap 已释放 |
| `vec_pool.rs:get()` | `slice::from_raw_parts()` | 运行时对齐检查；index < mmap_count 守卫 |
| `vec_pool.rs:rebuild_merged_cache()` | `slice::from_raw_parts()` | 同上 |
| `file_format.rs:load()` | `Mmap::map()` | 仅读；mmap Drop 前不删文件 |
| `file_format.rs:load_bq()` | `ptr::copy_nonoverlapping()` | bytemuck Pod 对齐；dst Vec 已预分配足够容量；src 长度精确边界 |
| `vector.rs:cosine_similarity_avx2()` | AVX2 SIMD 指令 | 运行时 `is_x86_feature_detected!` 检测通过才调用 |
| `index/bq.rs:popcount_distance()` | CPU 原生 `popcnt` 指令 | 运行时自动检测 CPU 支持；纯数学运算，无内存安全风险 |

所有 `unsafe` 块均附有明确的 `// SAFETY:` 注释。整个代码库没有 `unsafe impl Send/Sync`——`Send + Sync` 由 `Arc<Mutex<T>>` 自动推导，类型系统级别安全。


