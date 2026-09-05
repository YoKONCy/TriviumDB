# TriviumDB Payload 冷热分离一期工程计划

> 状态：待实施。
> 工程定位：独立一期工程，不并入当前图算法、认知算子或 Server 专项。
> 核心目标：在尽量不影响 QuIVer/BQ/精排等向量检索热路径的前提下，消除“全部原始 Payload JSON 必须复制到进程堆”的内存基线，并为解析后 JSON 建立有界生命周期。
> 产品边界：Embedded Core 仍是正式产品主体；Server 复用 Core 能力，不维护另一套 Payload 存储实现。
> 非目标：一期不引入 Payload 压缩、不实现通用列存、不把 TDB 改造成关系型数据库、不增加性能 benchmark CI Gate。

## 一、背景与当前基线

当前 `MemTable` 使用：

```text
HashMap<NodeId, PayloadEntry>

PayloadEntry:
  raw: Box<[u8]>
  parsed: OnceLock<serde_json::Value>
```

数据库从 `.tdb` 打开时会：

1. 顺序扫描全部 Payload 记录；
2. 校验每条 JSON；
3. 将每条原始 JSON 复制到独立堆分配；
4. 首次访问时惰性解析成 `serde_json::Value`；
5. 解析结果在数据库关闭前不淘汰。

该设计具备点查简单、热访问延迟低、无主动磁盘回读等优点，但存在两个明确的容量边界：

```text
冷 Payload 最低成本 = 全部 raw JSON + HashMap/Box/allocator 开销
热 Payload 追加成本 = parsed JSON DOM，且当前不可淘汰
```

当 Payload 总量较大或访问面逐渐扩大时，进程堆会同时保留 raw JSON 和 parsed DOM。向量基础层虽然可以 mmap，但 Payload 仍决定较高的常驻堆内存下限。

## 二、一期目标

一期必须完成以下闭环：

1. 引入统一 `PayloadStore`，隔离查询层与具体存储介质；
2. 将已发布 generation 的 raw Payload 放入只读 mmap 冷基础层；
3. 新写入、更新和删除继续进入内存 delta；
4. 引入有硬字节上限的 parsed Payload cache；
5. 查询采用 late materialization，ANN 内循环不得访问 Payload；
6. Property Index 命中时不得为了过滤而反序列化完整 Payload；
7. 无索引 Payload 扫描受到读取字节、解析字节和工作量预算约束；
8. ReadOnly/Immutable 保持严格零文件写入；
9. `.tdb/.vec/.pld` 作为同一 generation 原子发布；
10. 新版本安全读取旧格式，迁移失败保持原文件不变；
11. Rust/Python/Node 的公共语义保持一致；
12. 所有测试使用小 fixture 和定点故障，不耗尽真实物理内存或磁盘。

## 三、一期非目标

一期明确不做：

- Payload 大块压缩或字典压缩；
- JSON 字段列式拆分；
- 自动把任意 Payload 字段提升为列；
- 远程对象存储；
- 网络透明冷数据拉取；
- 跨数据库共享 Payload cache；
- 无上限后台预取；
- 依赖 Tokio 的 Core 实现；
- 修改 QuIVer 图结构或 ANN 算法；
- 为 benchmark 增加阻塞普通开发的 CI Gate；
- 通过兼容 shim 长期保留两套查询实现。

二期候选包括块压缩、顺序预取、批量 hydrate、TinyLFU admission 和更细粒度列式字段块，但均不进入一期完成定义。

## 四、核心架构

一期目标结构：

```text
┌──────────────────────────────────────────────────────┐
│ ParsedPayloadCache                                   │
│ NodeId → Arc<Value>，有界、可淘汰、仅匿名内存         │
├──────────────────────────────────────────────────────┤
│ PayloadDelta                                         │
│ NodeId → raw bytes / tombstone，新写与更新，常驻堆    │
├──────────────────────────────────────────────────────┤
│ MappedPayloadBase                                    │
│ 紧凑目录 + mmap raw JSON，已发布 generation，只读     │
└──────────────────────────────────────────────────────┘
```

读取优先级固定为：

```text
tombstone
→ delta raw
→ parsed cache
→ mmap base raw slice
→ 按需解析
→ 预算允许时加入 parsed cache
```

Flush/Compact：

```text
旧 mmap base
+ 当前 delta
+ tombstones
→ 生成新的 .pld.tmp
→ 完整校验
→ sync_all
→ 与 .tdb/.vec 同 generation 发布
→ 切换新 mmap base
→ 清空已发布 delta
→ 延迟清理旧 generation
```

任何失败只能留下：

```text
旧完整 generation
或
新完整 generation
```

不得出现 `.tdb/.vec/.pld` 混代。

## 五、PayloadStore 抽象

建议新增内部抽象，不直接暴露为公共 API：

```rust
pub(crate) trait PayloadStore {
    fn contains(&self, id: NodeId) -> bool;
    fn raw(&self, id: NodeId) -> Result<Option<PayloadBytes>>;
    fn get(&self, id: NodeId) -> Result<Option<Arc<serde_json::Value>>>;
    fn visit<R>(
        &self,
        id: NodeId,
        visitor: impl FnOnce(&serde_json::Value) -> R,
    ) -> Result<Option<R>>;
    fn iter_ids(&self) -> impl Iterator<Item = NodeId>;
    fn memory_stats(&self) -> PayloadMemoryStats;
}
```

实际实现可拆为：

```text
PayloadStore
├─ MappedPayloadBase
├─ PayloadDelta
└─ ParsedPayloadCache
```

一期不要求 trait object。若泛型或具体结构更简单，应优先使用具体 `PayloadStore`，避免热路径虚调用和对象安全复杂度。

### 5.1 PayloadBytes 生命周期

冷层 raw 数据来自 mmap，不能返回脱离映射生命周期的裸引用。候选设计：

```rust
pub(crate) enum PayloadBytes<'a> {
    Mapped(&'a [u8]),
    Delta(&'a [u8]),
}
```

或者由闭包访问：

```rust
fn with_raw<R>(
    &self,
    id: NodeId,
    visitor: impl FnOnce(&[u8]) -> R,
) -> Result<Option<R>>;
```

一期优先采用闭包访问，减少 mmap slice 被跨层长期持有的风险。

### 5.2 Parsed 值访问

当前 `get_payload()` 返回 `Option<&Value>`，与可淘汰 cache 不兼容。内部接口应逐步迁移为：

```rust
Result<Option<Arc<Value>>>
```

以及不需要所有权时的：

```rust
with_payload(id, |payload| ...)
```

原则：

- 查询内部优先闭包访问；
- 需要跨阶段持有时使用 `Arc<Value>`；
- 公共 API 继续返回当前约定的拥有型结果；
- 不允许通过泄漏引用或全局 pin 规避生命周期问题。

## 六、`.pld` 一期磁盘格式

一期推荐新增独立 `.pld` sidecar，而不是继续把 mmap 生命周期绑在可替换的 `.tdb` 上。

推荐逻辑布局：

```text
Header
Directory
Raw Payload Data
Footer / checksum
```

### 6.1 Header

建议字段：

```text
magic:          [u8; 4] = "TPLD"
version:        u16
flags:          u16
generation:     u64
node_count:     u64
directory_off:  u64
data_off:       u64
file_len:       u64
header_crc32:   u32
```

所有偏移和长度使用 checked arithmetic；读取时先验证 header，再进行任何按声明大小分配。

### 6.2 Directory

一期采用固定宽度条目：

```text
node_id: u64
offset:  u64
length:  u32
flags:   u32
crc32:   u32
reserved:u32
```

约 32 字节/节点；100 万节点目录约 30.5 MiB。后续可通过 slot 顺序、稠密 ID 或分块索引压缩，但一期优先保证简单、确定、可审计。

目录约束：

- NodeId 严格升序且唯一；
- `offset + length` 不得溢出；
- 每条记录必须落在 data 区；
- 数据区间不得重叠；
- tombstone 不写入已发布 base；
- 单条长度不得超过 Payload 公共上限；
- 目录尺寸必须与 `node_count` 精确一致。

### 6.3 Raw Data

一期直接保存紧凑 JSON UTF-8 字节：

```text
[json bytes][json bytes][json bytes]...
```

不压缩、不对每条记录做额外对齐。点查通过目录定位，首次访问时校验条目 CRC 并解析 JSON。

是否在打开阶段校验全部 JSON：

- 默认安全模式：打开时验证目录、全文件 checksum；JSON 语义在首次访问时验证；
- 严格校验模式：可显式要求打开时扫描所有 JSON；
- 一期不得因为惰性语义校验而允许损坏数据静默返回 `null`。

解析错误必须返回结构化 `CorruptedFile`，不能用 `unwrap_or(Value::Null)` 掩盖。

### 6.4 完整性

至少包含：

- Header CRC；
- 每条 Payload CRC，或固定大小数据块 CRC；
- 整文件 checksum；
- generation；
- 文件长度。

一期若实现成本需要取舍，优先：

```text
目录严格校验
+ 每条 Payload CRC
+ 整文件 CRC
```

所有 checksum 必须在写入完成后计算，并在 rename 前验证输出长度。

## 七、兼容与迁移

### 7.1 旧数据库读取

新版本必须支持当前“Payload 内嵌 `.tdb`”格式。

ReadWrite 首次打开旧格式时：

```text
读取旧 .tdb Payload
→ 构建 .pld.tmp
→ 校验
→ sync
→ 发布新 generation/marker
→ 切换 PayloadStore
```

若迁移失败：

- 原 `.tdb/.vec/.wal/.flush_ok` 不变；
- 不留下可被误识别为已发布的 `.pld`；
- 临时文件尽力清理；
- 下次可安全重试。

### 7.2 ReadOnly/Immutable

ReadOnly/Immutable 打开旧格式时严禁自动迁移。

一期必须在以下方案中明确选择一个：

1. **兼容堆后端**：只读旧格式时使用现有 PayloadEntry 加载，保持可读但内存收益暂不可用；
2. **显式迁移要求**：返回结构化 `MigrationRequired`，要求先以 ReadWrite 完成迁移。

推荐首期选择兼容堆后端：

- 不破坏旧数据库的只读可用性；
- 保持零写；
- 用户完成一次 ReadWrite flush/compact 后即可获得 mmap 冷层。

不得在 ReadOnly/Immutable 中创建 `.pld`、临时文件或 marker。

### 7.3 旧版本读取新格式

旧版本无法理解新的 generation，必须明确版本不支持或完整性不匹配，不能忽略 `.pld` 后只读取 `.tdb/.vec`。

## 八、ParsedPayloadCache

### 8.1 硬预算

配置建议：

```text
payload_cache_max_bytes
payload_cache_shards
payload_cache_max_entry_bytes
```

默认值应按嵌入式定位保守设置，不根据物理内存自动无限扩张。

规则：

- cache 当前估算字节不得超过硬上限；
- 单条 parsed 值超过 `max_entry_bytes` 时可解析并返回，但不缓存；
- cache 插入前进行 checked estimate；
- 淘汰不能阻塞 ANN 热路径；
- cache 关闭时仍可正确读取；
- 缓存属于性能层，不得影响查询结果。

### 8.2 淘汰策略

一期推荐分片 CLOCK 或 SLRU：

- 固定 shard 数；
- 每 shard 独立锁和字节预算；
- NodeId 决定 shard；
- 无后台线程；
- 操作可在固定访问序列下复现；
- 淘汰顺序可由测试观测。

一期不实现 TinyLFU，以免同时引入频率 sketch、admission 和衰减逻辑。

### 8.3 Pin 与并发

读取返回 `Arc<Value>`，被查询持有的条目可以从 cache 目录淘汰，但底层对象在最后一个 Arc 释放前继续存在。

需要单独统计：

```text
cache_accounted_bytes
pinned_evicted_bytes
```

硬预算只能严格约束 cache 自身持有的对象。请求长期持有的 Arc 必须由查询结果和请求内存预算约束，不能宣称 cache 上限等于进程绝对上限。

## 九、PayloadDelta

建议结构：

```rust
struct PayloadDeltaEntry {
    raw: Box<[u8]>,
    parsed: OnceLock<Arc<Value>>,
    generation: u64,
}

HashMap<NodeId, PayloadDeltaEntry>
RoaringTreemap tombstones
```

写入流程：

```text
校验大小和 JSON
→ 预估并保留 delta 内存
→ WAL
→ 更新 delta/tombstone
→ 失效相同 NodeId 的 parsed cache
→ 更新属性索引和统计
```

事务要求：

- 事务 validation 阶段预估全部新增 delta 字节；
- 分配或预算失败时零部分提交；
- Commit 后所有读取立即看到新 Payload；
- Rollback 后 cache/base/delta 语义不变；
- CAS、Unique Constraint 和 Atomic Delete Set 继续使用统一可见视图。

## 十、Late Materialization 与查询路径

### 10.1 纯向量检索不变量

下列路径在最终结果 hydrate 之前不得访问 PayloadStore：

```text
BQ 签名初筛
QuIVer 图导航
向量候选收集
Exact rerank
Top-K 截断
```

正确顺序：

```text
ANN/BQ/Exact
→ Top-K NodeId + score
→ RETURN/公共 API 需要时 hydrate Payload
```

必须增加测试 hook 或 profile 计数，证明纯向量候选阶段：

```text
payload_lookup_count = 0
payload_parsed_bytes = 0
```

若公共 API 最终返回完整节点，可在 Top-K 后只 hydrate 最终 K 条。

### 10.2 属性索引过滤

有索引字段：

```text
Property Index
→ NodeId 候选集合
→ 与向量候选组合
→ 最终必要时 hydrate
```

索引结果验证不得逐行解析完整 Payload，除非公共契约明确要求 residual predicate。

### 10.3 未索引扫描

未索引 Payload 条件不可避免需要访问 raw JSON。执行器必须：

- 按目录顺序扫描，避免 HashMap 随机页访问；
- 尽量使用 raw JSON visitor/局部解析；
- 只在表达式需要完整对象时构建完整 DOM；
- 受工作量和字节预算约束；
- Planner 将其标记为 ColdPayloadScan；
- `EXPLAIN ANALYZE` 暴露 cold reads、parsed bytes 和 cache hit/miss。

### 10.4 图算法与认知算子

只消费 NodeId、向量、边和命名指标的算法不得 hydrate Payload。只有以下情况可访问：

- Payload predicate；
- Payload 字段排序/聚合；
- RETURN 节点完整值；
- 算法明确声明使用某个 Payload 字段。

## 十一、预算与配置

建议新增配置：

```rust
pub struct PayloadStorageConfig {
    pub mode: PayloadStorageMode,
    pub parsed_cache_max_bytes: usize,
    pub parsed_cache_max_entry_bytes: usize,
    pub cache_shards: usize,
}

pub enum PayloadStorageMode {
    Auto,
    Heap,
    Mapped,
}
```

一期可先只公开 `Auto` 和内存上限；`Heap/Mapped` 主要供测试和回退验证，不一定立即成为稳定公共 API。

查询预算建议新增：

```text
max_payload_lookups
max_payload_raw_bytes
max_payload_parsed_bytes
max_payload_cache_insert_bytes
max_payload_scan_rows
```

超限行为：

- 返回明确结构化错误；
- 不静默截断；
- 不自动扩大预算；
- 不因缓存命中与否改变结果；
- Prepared 与 Direct 行为一致。

## 十二、内存统计

现有 `estimated_memory()` 应保留，但拆出可观测字段：

```text
payload_directory_bytes
payload_delta_raw_bytes
payload_delta_parsed_bytes
payload_cache_bytes
payload_cache_entries
payload_cache_hits
payload_cache_misses
payload_cache_evictions
payload_pinned_evicted_bytes
payload_mmap_file_bytes
payload_mmap_resident_estimate（若平台可靠，否则不提供）
payload_cold_reads
payload_raw_bytes_read
payload_bytes_parsed
```

不得把 mmap 文件总尺寸伪装成进程堆占用，也不得把 cache 预算宣传为进程 RSS 上限。

Rust/Python/Node 至少保持现有汇总统计一致；细分统计可通过稳定 stats 结构逐步公开。

## 十三、Cascades 与执行计划

新增或细化物理成本项：

```text
PayloadDirectoryLookup
HotPayloadLookup
ColdPayloadLookup
SequentialPayloadScan
PayloadParse
PayloadHydrate
```

成本模型至少考虑：

- 候选节点数；
- 是否有 Property Index；
- 预计 cache hit ratio；
- 平均 raw Payload 大小；
- 预计解析字节；
- 顺序扫描或随机点查；
-最终返回行数。

规则：

1. 向量检索优先在 hydrate 前完成 Top-K；
2. 有索引过滤优先索引集合；
3. 大量随机冷 Payload lookup 的成本高于顺序扫描；
4. 计划选择不得依赖不可复现的瞬时 cache 状态；
5. 同一统计快照下计划必须确定；
6. `EXPLAIN` 展示估算，不触发 Payload 读取；
7. `EXPLAIN ANALYZE` 展示实际 cold read/cache 指标。

## 十四、并发与锁

Core 继续保持单写多读。

锁顺序必须固定，建议：

```text
Database/MemTable 写锁
→ PayloadDelta
→ Property Index
→ Parsed cache shard
```

读路径不得持有全局 MemTable 写锁进行 JSON 解析。

cache 要求：

- shard 间无嵌套锁；
- cache miss 并发解析同一 NodeId 可容许短暂重复计算，或使用单航班机制；
- 一期优先避免复杂 singleflight，除非重复解析在测试中证明严重；
- cache 淘汰不得调用数据库写路径；
- flush 切换 base 时，旧 mmap 必须由 Arc 生命周期安全延迟释放。

Server 请求取消只影响当前 hydrate/scan，不得破坏 cache 或 delta。

## 十五、文件发布与 Windows 约束

Windows 不能可靠替换仍被映射的同名文件。因此一期不得依赖：

```text
保持旧 .pld mmap 打开
同时原地覆盖/替换同名 .pld
```

推荐 generation 文件名：

```text
<db>.pld.<generation>
```

由 marker/manifest 指向当前 generation。发布顺序：

```text
写 .pld.<new>.tmp
→ sync
→ rename 为 .pld.<new>
→ 发布 .tdb/.vec 新 generation
→ 最后原子发布 marker/manifest
→ 新读者打开新 .pld
→ 旧 mmap Arc 释放后再清理旧文件
```

清理失败不影响正确性，只产生可识别的 orphan generation；下次 ReadWrite 打开可在锁保护下安全回收。ReadOnly/Immutable 不执行 orphan 清理。

## 十六、实施阶段

### P0：语义冻结与观测

任务：

1. 给现有 Payload 访问点分类；
2. 统计点查、过滤、排序、聚合、返回 hydrate、索引验证；
3. 增加仅测试/profile 使用的 Payload access counters；
4. 建立当前 Heap 后端 reference；
5. 固化 Rust/Python/Node 公共结果与错误契约；
6. 建立纯向量路径 `payload lookup = 0` 测试。

验收：

- 所有生产访问点有明确类别；
- 当前行为由测试冻结；
- 不改变磁盘格式和公共 API。

### P1：PayloadStore + HeapBackend

任务：

1. 引入统一 PayloadStore；
2. 将现有 `HashMap<NodeId, PayloadEntry>` 收敛为 HeapBackend；
3. 将查询、事务、属性索引、文本入口迁移到统一访问 API；
4. 移除生产路径对 `&Value` 永久生命周期的依赖；
5. 维持结果逐字段一致。

验收：

- 默认仍使用 HeapBackend；
- 所有现有测试通过；
- old-vs-new differential 逐字段一致；
- 纯向量性能路径没有新增 PayloadStore 调用。

### P2：MappedPayloadBase + 旧格式兼容

任务：

1. 实现 `.pld` parser/writer；
2. 实现紧凑目录；
3. 实现 mmap raw lookup；
4. 实现旧内嵌格式兼容加载；
5. ReadWrite 支持安全迁移；
6. ReadOnly/Immutable 旧格式走零写兼容后端；
7. generation 和 checksum 联动。

验收：

- 大型冷 Payload 不再全量复制进堆；
- `.pld` 任意截断、bit flip、offset 溢出都 fail-closed；
- 旧格式数据可读且迁移不丢字段；
- 迁移失败原文件状态不变。

### P3：PayloadDelta + 原子发布

任务：

1. 新写、更新、删除进入 delta/tombstone；
2. WAL recovery 写入 delta；
3. Flush/Compact 合并 base 和 delta；
4. 实现 generation 文件发布与旧 mmap 生命周期；
5. 更新 Unique/CAS/Atomic Delete/索引维护；
6. 增加完整 power-loss 与 I/O failpoint。

验收：

- Commit 后立即可见；
- Rollback 零部分提交；
- 重开后结果一致；
- 只能恢复旧完整代或新完整代；
- Windows 下不依赖替换已映射文件。

### P4：有界 Parsed Cache

任务：

1. 实现分片 CLOCK/SLRU；
2. 字节预算和超大条目 bypass；
3. cache hit/miss/eviction/pinned 统计；
4. 查询取消与 cache 安全；
5. 多线程读一致性测试；
6. Heap/Mapped 后端差分。

验收：

- cache 关闭、极小、正常和零容量时结果一致；
- cache 自持有字节不超过预算；
- 淘汰后可从 mmap 正确重新解析；
- ReadOnly/Immutable 文件逐字节零写；
- 无死锁、panic 和永久 pin 泄漏。

### P5：Late Materialization 与 Planner

任务：

1. 审计 ANN/BQ/QuIVer/Exact rerank；
2. Top-K 前禁止 Payload hydrate；
3. Property Index filter 避免完整 JSON 读取；
4. 无索引扫描顺序化并预算化；
5. Cascades 加入冷热 Payload 成本；
6. EXPLAIN/ANALYZE 增加观测。

验收：

- 纯向量候选阶段 Payload lookup 为 0；
- 返回 K 行最多 hydrate 必要结果行；
- 有索引等值过滤不做全量 Payload parse；
- 未索引扫描超预算明确拒绝；
- Heap/Mapped 结果完全一致。

## 十七、测试计划

### 17.1 独立差分

同一声明式数据库状态同时驱动：

```text
HeapBackend
MappedBase + empty delta
MappedBase + delta
ReadOnly mapped
Immutable mapped
flush/reopen mapped
```

逐字段比较：

- get_payload；
- FIND/WHERE；
- ORDER BY；
- 聚合；
- Prepared/Direct；
- TQL 多阶段 WITH；
- Rust/Python/Node 返回值；
- 错误码。

### 17.2 状态机

操作序列：

```text
insert / insert_with_id
update_payload / delete
begin / commit / rollback
CAS / unique constraint
create/drop property index
flush / compact / reopen
switch access mode
cache resize / cache clear
```

每一步比较独立 reference 和各后端。

### 17.3 格式变异

覆盖 `.pld`：

```text
逐字节截断
magic/version/generation 错误
node_count 溢出
offset/length 溢出
目录乱序/重复 NodeId
数据区重叠
条目 CRC 错误
整文件 checksum 错误
修复 CRC 后语义非法 JSON
跨 generation 混代
orphan generation
```

所有 loader 必须在有限内存和时间内返回，不得按文件声明值直接危险分配。

### 17.4 I/O 与 Power-loss

发布阶段：

```text
AfterPldCreate
AfterPldDirectoryWrite
AfterPldDataWrite
BeforePldSync
AfterPldSync
BeforePldRename
AfterPldRename
BeforeGenerationPublish
AfterGenerationPublish
BeforeOldGenerationCleanup
```

父进程使用小 fixture 定点终止子进程；不得通过填满磁盘模拟故障。

### 17.5 Cache 属性测试

- `cache=0` 与正常 cache 结果一致；
- 重复读取增加 hit，不改变结果；
- 淘汰后重读一致；
- 超大条目 bypass；
- 更新后旧 cache 不可见；
- 删除后 cache 不得复活节点；
- shard 数变化不改变结果；
- 1/2/4/8 线程结果一致；
- 固定访问序列可复现淘汰统计。

### 17.6 向量检索保护

功能测试必须验证：

```text
纯 ANN：候选阶段零 Payload lookup
ANN + 返回节点：只在最终结果 hydrate
ANN + 有索引过滤：不扫描全 Payload
ANN + post-filter：读取量受候选和预算约束
```

性能 benchmark 仅用于专项对比和发布前人工验收，不加入当前普通 CI Gate。

### 17.7 资源安全

- 使用小 mmap 文件验证页面行为；
- allocator failure 使用定点拒绝；
- I/O failure 使用 failpoint；
- 不分配接近物理内存上限的数据；
- 不创建无上限文件；
- 所有随机测试固定并输出 seed；
- cache 压力使用极小预算和少量对象模拟。

## 十八、性能与资源验收口径

一期不承诺所有 Payload 查询无性能损失。验收按路径分类：

### A. 纯向量检索

目标：

- QuIVer/BQ/Exact 候选算法不变；
- Payload lookup counter 在 hydrate 前为 0；
- 无额外全局锁进入 ANN 内循环；
- 结果、排序和确定性不变。

专项 benchmark 观察：

- p50/p95/p99；
- throughput；
- allocations/op；
- payload lookups/query。

不设置普通 CI 性能 Gate，但任何稳定的大回归都必须调查后才能合入。

### B. 索引过滤 + 向量

目标：

- 索引候选阶段不解析完整 Payload；
- hydrate 数量与最终候选规模相关，而非数据库总节点数；
- 计划不因 cache 瞬时状态随机变化。

### C. 冷 Payload 点查

允许首次访问增加 page fault 和 JSON parse，但要求：

- 后续 cache 命中明显降低解析次数；
- 有明确 cache bypass 和预算行为；
- 错误不被转成 Null。

### D. 未索引全扫描

允许比全堆模型慢，但要求：

- 顺序访问；
- 有界工作量；
- 可由索引建议识别；
- 不污染向量热路径；
- 超预算 fail-closed。

### E. 内存收益

对固定 fixture 比较：

```text
HeapBackend estimated heap
MappedBackend directory + delta + cache heap
```

至少证明：

- 冷 raw Payload 不再按总字节复制到堆；
- parsed cache 可由固定预算限制；
- mmap 文件大小与 heap 统计分离；
- 访问全部 Payload 后 cache 仍保持有界。

不使用 RSS 单一指标作为正确性断言；RSS 仅作为专项观测。

## 十九、错误模型

建议新增或复用稳定错误：

```text
TDB_UNSUPPORTED_PAYLOAD_VERSION
TDB_PAYLOAD_CORRUPTED
TDB_PAYLOAD_GENERATION_MISMATCH
TDB_PAYLOAD_BUDGET_EXCEEDED
TDB_PAYLOAD_MIGRATION_REQUIRED
TDB_PAYLOAD_MIGRATION_FAILED
```

要求：

- 中英双语上下文遵循项目现有错误规范；
- 不泄漏完整 Payload 内容；
- 不在日志输出 raw JSON；
- 错误包含文件角色、generation、NodeId（若安全）和失败阶段；
- Python/Node/Server 映射保持一致；
- corruption、budget、migration、I/O 不得混成同一错误。

## 二十、日志与可观测性

允许记录：

- Payload 后端类型；
- `.pld` generation 和文件大小；
- 目录条目数；
- cache 预算和 shard 数；
- cache 命中率；
- cold read/parse 字节；
- migration 阶段；
- orphan 清理数量。

禁止记录：

- 完整 Payload；
- raw JSON；
- Prepared 参数；
- 敏感字段值；
- 外部存储凭据。

## 二十一、风险与应对

### 风险 1：冷过滤延迟回归

应对：索引优先、顺序扫描、预算、Planner 成本和明确 profile。

### 风险 2：Arc pin 让实际内存超过 cache 上限

应对：统计 pinned bytes，并由查询结果预算限制长期持有对象。

### 风险 3：Windows mmap 阻止替换

应对：generation 文件名和延迟清理，不原地覆盖映射文件。

### 风险 4：`.tdb/.vec/.pld` 混代

应对：统一 generation、marker/manifest、CRC 和原子发布顺序。

### 风险 5：查询层大规模 API 改造

应对：先引入 HeapBackend 保持语义，再切 MappedBackend；禁止一次性重写全部查询。

### 风险 6：属性索引与 Payload 可见视图不一致

应对：base/delta/tombstone 与索引更新纳入同一事务发布边界，增加状态机差分。

### 风险 7：缓存改变确定性

应对：cache 只能改变性能，任何命中/淘汰组合都必须返回相同值、排序和错误。

### 风险 8：格式兼容长期拖累

应对：旧格式兼容集中在 loader/migrator，不让生产查询长期维护两套分支。

## 二十二、Stop/Go Gate

### Gate A：PayloadStore 抽象

Go：

- HeapBackend 行为逐字段一致；
- ANN 候选阶段零 Payload 访问；
- 无公共 API 破坏。

Stop：

- 为适配 cache 泄漏引用；
- 查询大量复制 Value；
- 引入全局热路径锁。

### Gate B：Mapped Base

Go：

- 冷 raw Payload 堆占用显著下降；
- 格式 mutation 全部 fail-closed；
- ReadOnly/Immutable 零写。

Stop：

- 必须原地替换已映射文件；
- offset/length 无法在分配前验证；
- 损坏 JSON 被静默转 Null。

### Gate C：Delta 与发布

Go：

- power-loss 只恢复旧代或新代；
- WAL/事务/索引一致；
- 失败可重试。

Stop：

- 混代可见；
- Rollback 后残留 cache/delta；
- 需要危险资源故障测试才能验证。

### Gate D：Parsed Cache

Go：

- 自持有字节严格有界；
- 淘汰不改语义；
- 并行读无死锁。

Stop：

- cache 上限被单条合法 Payload 绕过；
- cache 锁进入 ANN 内循环；
- 更新后可读取旧值。

### Gate E：Late Materialization

Go：

- 纯向量检索路径结构不变；
- 有索引过滤不扫描全 Payload；
- hydrate 数量可观测且有界。

Stop：

- ANN 候选阶段开始加载 Payload；
- Planner 因瞬时 cache 状态产生不确定计划；
- 默认查询出现稳定的大幅回归且无法解释。

## 二十三、一期完成定义

只有同时满足以下条件，才能将一期标记完成：

- `PayloadStore` 成为唯一内部 Payload 访问入口；
- 已发布 raw Payload 使用 mmap base，不再全量复制到堆；
- 写入和更新使用内存 delta；
- parsed cache 有硬字节上限和超大条目 bypass；
- Heap/Mapped/Delta 各模式结果差分一致；
- QuIVer/BQ/Exact 候选阶段不访问 Payload；
- Property Index 路径避免无必要完整解析；
- 未索引扫描有读取与解析预算；
- `.pld` 纳入 generation、CRC 和原子发布；
- 旧格式可安全读取和迁移；
- ReadOnly/Immutable 在全部成功和失败场景零写；
- WAL recovery、事务、CAS、Unique、删除和索引一致；
- `.pld` mutation、I/O failpoint、power-loss 测试完整；
- Rust/Python/Node 公共契约一致；
- 生产可达路径无新增显式 panic；
- Clippy 和全量测试通过；
- 不增加普通 CI 性能 benchmark Gate；
- 专项性能报告确认纯向量路径无结构性回归；
- 测试不会耗尽真实物理内存、磁盘、线程或句柄。

## 二十四、推荐执行顺序

```text
1. P0 访问点清单与 Payload counters
2. PayloadStore + HeapBackend
3. 内部 &Value API 迁移为闭包/Arc
4. HeapBackend old-vs-new differential
5. `.pld` 格式与 parser/writer
6. MappedPayloadBase
7. 旧格式兼容和 ReadWrite migrator
8. ReadOnly/Immutable 零写兼容
9. PayloadDelta + tombstones
10. WAL/事务/索引统一可见视图
11. generation 原子发布与 Windows 生命周期
12. Parsed cache 与硬预算
13. Late materialization 全路径审计
14. Cascades 成本和 EXPLAIN/ANALYZE
15. 格式 mutation、failpoint、power-loss、状态机
16. Rust/Python/Node 契约
17. 专项性能与内存验收
18. 全量 Clippy/test 收口
```

每一步必须先完成最小端到端闭环和独立差分，再进入下一步；不得先大规模重写格式、查询和缓存后统一调试。
