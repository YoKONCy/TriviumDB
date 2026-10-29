# TriviumDB API 完整参考

> **版本**: `0.9.0-beta.4` 公共 API；见[变更说明](changes/0.9.0-beta.4.md)。
> **语言**: Rust 核心 + Python 绑定 (PyO3) + Node.js 绑定 (napi-rs)  
> **许可**: Apache-2.0

共享资源运行时、实例诊断、受控关闭和目录快照从 `0.9.0-beta.1` 起提供。配置与示例见[资源运行时](resource-runtime.md)和[关闭、快照与恢复](lifecycle-snapshots.md)。

---

## 目录

- [数据库生命周期](#数据库生命周期)
- [节点 CRUD](#节点-crud)
- [图谱操作](#图谱操作)
- [搜索与召回](#搜索与召回)
- [Hook 扩展系统](#hook-扩展系统)
- [元数据过滤](#元数据过滤)
- [TQL、Prepared 与一等值](#tqlprepared-与一等值)
- [持久化索引管理与观测](#持久化索引管理与观测)
- [存储格式观测与迁移错误](#存储格式观测与迁移错误)
- [持久化与压缩](#持久化与压缩)
- [多实例资源运行时与诊断](#多实例资源运行时与诊断)
- [内存管理](#内存管理)
- [工具方法](#工具方法)
- [维度迁移](#维度迁移)
- [事务支持](#事务支持)
- [Pythonic 魔术方法](#pythonic-魔术方法)
- [数据类型说明](#数据类型说明)

---

## 数据库生命周期

实例通过同步关闭或受控关闭完成持久化与资源释放。独立目录快照支持捕获、校验和恢复已打开数据库的状态。

### Python

```python
import triviumdb

# 基础打开方式（默认 f32 向量、1536 维、normal 同步模式）
db = triviumdb.TriviumDB("my_data.tdb", dim=1536)

# 完整参数
db = triviumdb.TriviumDB(
    path="my_data.tdb",    # 文件路径（不存在则新建）
    dim=1536,              # 向量维度（一旦创建不可更改）
    dtype="f32",             # 向量类型："f32" | "f16" | "u64"
    sync_mode="normal",      # WAL 同步模式："full" | "normal" | "off"
    load_text_index=False,    # 打开时是否加载持久化全文索引
    auto_build_quiver=True,   # 是否允许查询自动构建 QuIVer；flush 不会触发 ANN 构建
    expected_nodes=3_600_000, # 预计总节点数，用于本次打开的容量预留
    memory_limit_mb=28_672,   # 内核内存预算，0 表示不限制
    runtime=None,            # 使用进程共享运行时，可传入自建 ResourceRuntime
)

# 推荐：使用上下文管理器（退出时自动 flush 落盘）
with triviumdb.TriviumDB("my_data.tdb", dim=1536) as db:
    # ... 所有操作 ...
    pass  # 退出时自动调用 db.flush()
```

**参数说明：**

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `path` | `str` | *必填* | `.tdb` 文件路径，不存在时自动创建 |
| `dim` | `int` | `1536` | 向量维度，必须与后续插入的向量长度一致；强烈建议不超过 3072 |
| `dtype` | `str` | `"f32"` | 向量存储精度：`f32`（标准）、`f16`（省内存）、`u64`（SimHash） |
| `sync_mode` | `str` | `"normal"` | WAL 写入安全级别，详见[持久化与压缩](#持久化与压缩) |
| `load_text_index` | `bool` | `False` | 是否在打开时加载持久化全文索引 |
| `auto_build_quiver` | `bool` | `True` | 是否允许查询准备阶段自动构建 QuIVer；纯 `flush()` 不构建 ANN |
| `expected_nodes` | `int \| None` | `None` | 预计总节点数，用于本次打开时预留核心容器容量 |
| `memory_limit_mb` | `int` | `0` | TriviumDB 内核内存预算（MiB）；0 表示不限制 |
| `payload_cache_mb` | `int` | `64` | Payload 解析缓存总预算（MiB）；0 表示禁用，淘汰不影响结果 |
| `payload_cache_entry_mb` | `int` | `8` | 单条解析后 Payload 可进入缓存的最大估算大小（MiB）；超大条目不驻留 |
| `access_mode` | `str` | `"read_write"` | `read_write` 使用排他锁；`read_only` 使用共享锁；`immutable` 验证 manifest 后无锁打开 |
| `runtime` | `ResourceRuntime \| None` | `None` | 所属共享资源运行时；默认使用进程共享运行时 |

`search_with_context` / `searchWithContext` 返回的上下文除阶段耗时和候选数外，还包含 `observations`：`estimated_heap_bytes`、`mmap_vector_bytes`、`node_count`、查询路由与 QuIVer `ef_search`；Linux 额外提供进程 RSS、累计主缺页和次缺页。缺页计数是进程累计值，调用方应计算相邻采样差值。

> ⚠️ **强烈建议 `dim <= 3072`。** 数据库存储和精确 BruteForce 检索支持 3073–65536 维，但 QuIVer 仅支持 1–3072 维。高于 3072 维时，自动 QuIVer 构建会被安全跳过，搜索回退到 BruteForce；显式构建 QuIVer 会返回错误，不会 panic。若数据规模可能达到当前维度的动态 QuIVer 阈值并依赖 ANN 性能，请在建库前选择不超过 3072 维的 embedding 模型或先做降维。

### 只读访问模式

只读模式面向已完成 `flush()` 的不可变数据库代际。多个进程可以同时以共享锁打开同一路径，但同路径 Reader 与 Writer 互斥。

TriviumDB 采用单写多读模型。调用方通过单一写队列、`Lock` / `Mutex` 或 actor 串行化同一实例的写操作，ReadOnly/Immutable Reader 承担并发读取。

```python
db = TriviumDB("generation-42/data.tdb", dim=768, access_mode="read_only")
```

```typescript
const db = new TriviumDB("generation-42/data.tdb", {
  dim: 768,
  accessMode: "readOnly",
})
```

Rust 可使用 `Database::<f32>::open_read_only(path, dim)`，或在 `Config` 中设置 `AccessMode::ReadOnly`。

只读句柄具有以下安全语义：

- 不创建数据库、WAL、sidecar 或一致性标记；
- 不截断、回放或清理 WAL；检测到待恢复 WAL 时返回 `RecoveryRequired`；
- 不恢复或清理 `.publish` 撤销日志；未提交的发布须先由 ReadWrite 恢复，返回 `RecoveryRequired`；
- 不删除损坏或错配的 TextIndex/QuIVer sidecar；
- `.tdb/.vec/.pld.<generation>/.flush_ok` 代际不完整时拒绝打开，不执行 metadata-only 降级；
- 禁止 CRUD、事务提交、TQL mutation、索引修改、QuIVer 构建、flush 和 compact；
- `close()` 和 Drop 不执行持久化；
- QuIVer 不可用时保持内存只读并走安全检索路径，不写回磁盘。

发布只读代际前必须由 Writer 完成 `flush()` 或 `close()`，并保留相邻 `.lock` 文件供 Reader 获取共享锁。不要在仍有进程持锁时删除 `.lock` 文件。

### 不可变 Generation

Writer 可在完成持久化后原子生成 generation manifest：

```rust
db.publish_generation_manifest("generation-42")?;
```

Python 使用 `db.publish_generation_manifest("generation-42")`，Node.js 使用 `db.publishGenerationManifest("generation-42")`。该操作会先执行安全 `flush()`，随后为当前 `.tdb`、`.vec`、`.pld.<generation>`、`.flush_ok` 及已存在的 TextIndex/QuIVer sidecar 记录文件大小和 CRC32，并最后原子发布 `<path>.manifest.json`。

发布完成后可在不需要 `.lock` 和 `.wal` 的部署目录中使用：

```rust
let db = Database::<f32>::open_immutable(path, 768)?;
```

```python
db = TriviumDB(path, dim=768, access_mode="immutable")
```

```typescript
const db = new TriviumDB(path, { dim: 768, accessMode: "immutable" })
```

Immutable 会在加载前验证 manifest 版本、完成状态、dtype、维度、文件存在性、大小和 CRC32，不创建锁文件或 WAL，也不修复任何文件。句柄存活期间禁止原地修改、覆盖或删除 generation 中的文件；正确发布方式是构建新目录或新路径，然后由应用原子切换 `current` 指针。

`Config.missing_index_policy` 控制 Reader 遇到 QuIVer/TextIndex 缺失、错配或损坏时的行为：

| 策略 | 行为 |
|---|---|
| `Fallback` | 默认；忽略不可用 sidecar，QuIVer 回退安全检索路径 |
| `BuildInMemory` | 允许当前进程在内存中惰性构建 QuIVer，但绝不写回 sidecar |
| `Error` | 严格模式；立即返回 `ImmutableArtifactInvalid`，不修改任何文件 |

Python 使用 `missing_index_policy="fallback|build_in_memory|error"`；Node.js 使用 `missingIndexPolicy: "fallback" | "buildInMemory" | "error"`。ReadOnly/Immutable Reader 禁止启用 `enable_refractory_fatigue`，避免不同 Worker 因进程本地疲劳状态产生不一致结果。

### Rust 类型化门面

Rust 应用可以使用类型化门面，让只读代码在编译期无法调用写 API：

```rust
use triviumdb::{DatabaseReader, DatabaseWriter};

let mut writer = DatabaseWriter::<f32>::open("data.tdb", 768)?;
writer.insert(&vector, payload)?;
writer.publish_generation_manifest("generation-42")?;

let reader = DatabaseReader::<f32>::open_read_only("data.tdb", 768)?;
let hits = reader.search(&query, 10, 0, 0.0)?;

let immutable = DatabaseReader::<f32>::open_immutable("data.tdb", 768)?;
let hits = immutable.search_exact(&query, 10)?;
```

`DatabaseReader<T>` 只暴露查询、节点读取、图遍历、TQL 只读查询、统计和关闭能力，不实现 `Deref<Target = Database<T>>`，因此无法绕过门面访问 `insert`、事务、索引修改、flush 或 compact。`DatabaseWriter<T>` 保留完整 `Database<T>` 能力，并通过 `Deref/DerefMut` 保持使用体验。

原有 `Database<T>` API 继续保留，用于动态语言绑定和需要运行时切换访问模式的兼容场景。

### Generation Store

服务端或多 Worker 应使用 `GenerationStore` 管理不可变代际，不要原地覆盖 Reader 正在 mmap 的文件：

```rust
use triviumdb::{DatabaseWriter, GenerationStore};

let store = GenerationStore::new("./generations");
let path = store.prepare_generation("generation-42", "data.tdb")?;

let mut writer = DatabaseWriter::<f32>::open(path.to_str().unwrap(), 768)?;
writer.insert(&vector, payload)?;
writer.publish_generation_manifest("generation-42")?;
drop(writer);

store.publish_current("generation-42", "data.tdb")?;

let reader = store.open_current::<f32>(768)?;
assert_eq!(reader.generation_id(), "generation-42");
```

目录结构：

```text
generations/
├── current.json
├── current.json
├── generation-41/
│   └── data.tdb...
└── generation-42/
    └── data.tdb...
```

`publish_current()` 使用临时文件、文件 `fsync`、原子 rename 和父目录同步，并在切换前完整验证 manifest、文件 checksum、generation ID 与实际 node count。`open_current()` 在解析 current 与获取 generation 共享租约期间持有管理共享锁，避免与切代或回收竞态。`GenerationReader` 在整个生命周期持有外部 runtime 共享租约；`reclaim_generation()` 只有取得排他租约后才删除旧代际，并且在 current 损坏时 fail closed。

租约不会写入不可变 generation。默认 runtime 目录位于系统临时目录的 `triviumdb-runtime/store-<hash>`，也可以使用 `GenerationStore::with_runtime_dir(root, runtime_dir)` 指定可写位置。generation root 因此可以保持只读。

构造时固定相对根目录和运行目录的绝对位置；默认锁空间在根目录存在后按规范路径解析，根目录创建前后的 Store 共享租约。跨进程参与者使用一致的规范根路径与运行目录配置；跨容器将运行目录设为共享可写挂载，并保持根目录挂载路径一致。Reader 存活期间保持根目录位置稳定。升级时先结束旧版本 Reader 进程，再恢复旧代回收。

generation ID 和数据库文件名必须是单一安全路径组件，`..`、绝对路径和路径分隔符都会被拒绝。旧 Reader 在切代后继续读取原 generation，新 Reader 读取新的 current。调用方应在 Writer 关闭并移除非制品 `.wal/.lock` 后再分发 generation。

CRC32 用于检测随机损坏，不提供对抗性防篡改或来源认证。不可信分发应由外层对 manifest 使用 SHA-256/BLAKE3 并进行数字签名。

### Rust

```rust
use triviumdb::Database;
use triviumdb::database::{Config, StorageMode};
use triviumdb::storage::wal::SyncMode;

// 基础打开（默认 Mmap 模式 + Normal 同步）
let mut db = Database::<f32>::open("my_data.tdb", 1536)?;

// 指定同步与存储模式统一使用 Config；open_with_sync 已移除
// 高级配置——同时指定存储模式和同步模式
let mut db = Database::<f32>::open_with_config("my_data.tdb", Config {
    dim: 1536,
    storage_mode: StorageMode::Rom,  // Rom：单文件便携 | Mmap：分离零拷贝（默认）
    sync_mode: SyncMode::Normal,
    expected_nodes: Some(3_600_000),
    memory_limit: 28 * 1024 * 1024 * 1024,
    ..Default::default()
})?;

// 运行时切换同步模式
db.set_sync_mode(SyncMode::Off)?;
```

`close()` 先进入 `Closing`，拒绝新操作并等待已有操作结束，完成必要检查点及最终 WAL 同步，再释放 WAL、mmap、Payload、数据库持有的 Hook 与文件锁并进入 `Closed`。关闭成功后，即使包装对象仍被持有，数据库自身的文件资源也已释放；没有其他占用时可以立即移动、删除文件组或重新打开。持久化失败会返回原始错误，恢复为 `Open` 并保留内存和文件锁供重试。

数据访问统一执行生命周期检查：Rust 的 `get`、`get_payload`、`get_edge`、`get_edges`、`get_incoming_edges`、`get_all_ids`、`all_node_ids`、`node_count`、`contains`、`neighbors`、`neighbors_with_labels`、`shortest_path`、`graph_stats`、`validate_graph`、`list_indexes`、`list_unique_indexes`、`index_info` 与 `clear_search_state` 返回 `Result`；`set_hook` 也返回 `Result<()>`。调用方使用 `?` 或显式处理错误。Python/Node 的打开状态返回类型保持一致，关闭后抛错。状态、资源统计和配置诊断继续可读。

`flush()` 通过 `.publish` 撤销日志管理主数据、向量、QuIVer、文本、属性与图索引文件组。Mmap 在各文件同步后提交 marker，Rom 在同步后提交日志状态；ReadWrite `open()` 先完成中断恢复，再流式回放 WAL。备份使用有界缓冲，额外磁盘空间与纳入发布的旧文件组大小对应。主文件使用 v10、marker 使用 v3、发布日志使用 v2，并支持读取历史 v1 日志。协议见[恢复说明](security.md#8-mmap-三文件一致性标记flush_ok-v3)。

WAL 的完整帧发生 CRC、解码或超长字段损坏时，`open()` 返回 `CorruptedFile` 并保留完整日志；物理未写完的尾帧继续按提交边界恢复。QuIVer 元数据 v2 将索引绑定到主文件、向量内容及活跃 ID/slot；失配按 `missing_index_policy` 回退或报错。底层 BQ Rust 距离、签名读取和原始存储解码接口返回 `Result`，详见 [QuIVer 边界](quiver.md#索引事实绑定与-bq-边界)。

**泛型类型参数 `T`：**

| 类型 | 说明 | 适用场景 |
|------|------|----------|
| `f32` | 32 位浮点 | 标准 embedding（OpenAI、BGE 等） |
| `half::f16` | 16 位半精度浮点 | 大规模数据集省内存 |
| `u64` | 64 位无符号整数 | SimHash / 二值化向量 |

`f32/f16` 使用余弦相似度；普通有限幅值保留 SIMD 路径，中间范数或分母出现上溢、下溢时使用 `f64` 回退计算，零向量得分为 `0`。`u64` 使用精确汉明匹配位数，单分量得分范围为 `0..64`，保持完整位模式；普通、高级、批量和 TQL 基础检索共享该度量。QuIVer 自动构建适用于浮点向量，显式为 `u64` 构建返回 `TDB_INVALID_INPUT`。

`u64` 的 Hook 可观察浮点查询视图并调整配置；可观察的查询向量改写返回 `TDB_INVALID_INPUT`，基础检索继续使用原始整数切片。高级便利管线的浮点 FISTA/DPP 配置面向浮点向量。

普通、精确、GraphFirst 与物理检索算子在候选生成前校验查询维度及有限值；NaN、正负 Infinity 返回 `TDB_INVALID_VECTOR`，空库和空 anchor 集合遵守相同规则。候选池扩展采用饱和算术，超大 Top-K 按实际候选返回结果。

---

### 关闭请求与限时等待

| Rust | Python | Node.js | 行为 |
| --- | --- | --- | --- |
| `request_shutdown()` | `request_shutdown()` | `requestShutdown()` | 停止新操作，登记排空与持久化任务，返回关闭状态 |
| `shutdown_status()` | `shutdown_status()` | `shutdownStatus()` | 返回阶段、活动操作数、最近故障与关闭维护状态 |
| `close_timeout(Duration)` | `close_timeout(timeout_ms)` | `closeTimeout(timeoutMs)` | 完成返回 `true`，等待超时返回 `false`，关闭失败返回错误 |
| `close()` | `close()` | `close()` | 同步等待关闭完成 |

`ShutdownStatus.phase` 为 `open`、`draining`、`flushing`、`failed` 或 `closed`。`last_failure` 提供故障分类和预算详情，`maintenance` 提供当前关闭登记的执行或延期信息。等待超时后，关闭任务继续持有所需资源并推进完成。

```python
db.request_shutdown()
if not db.close_timeout(5000):
    print("关闭进度：", db.shutdown_status())
```

Python 关闭等待释放 GIL。Node `closeTimeout()` 同步等待；需要继续处理事件循环时，可结合 `requestShutdown()`、`closeTimeout(0)` 和宿主定时器轮询。阶段转换、失败重试与关闭完成语义见[关闭协议](lifecycle-snapshots.md#关闭协议)。

### 一致快照与恢复

| Rust | Python | Node.js | 返回值 |
| --- | --- | --- | --- |
| `db.create_snapshot(destination, generation_id)` | 同名方法 | `db.createSnapshot(destination, generationId)` | generation manifest |
| `storage::snapshot::verify_snapshot(directory)` | `triviumdb.verify_snapshot(directory, runtime=None)` | `verifySnapshot(directory, runtime?)` | 验证后的 manifest |
| `storage::snapshot::restore_snapshot(source, destination)` | `triviumdb.restore_snapshot(source, destination, runtime=None)` | `restoreSnapshot(source, destination, runtime?)` | 恢复后的 manifest |

Rust 独立校验与恢复提供接收 `&ResourceRuntime` 的 `_with_runtime` 版本。创建快照使用可写实例；目标采用已存在的父目录和全新的子目录。恢复目录中的数据库文件固定为 `database.tdb`，宿主按 manifest 中的维度和原数据库 dtype 显式打开。

manifest 包含 generation ID、dtype、维度、节点数、制品长度和 CRC。已有目标、发布后目录同步失败及校验失败的错误映射见[一致快照](lifecycle-snapshots.md#一致快照)。

---

## 节点 CRUD

### insert — 插入节点

向数据库写入一个新节点，同时携带向量和 JSON 元数据。返回自动分配的 `u64` 节点 ID。

**Python：**
```python
node_id = db.insert(
    vector=[0.12, -0.45, 0.78, ...],       # 向量（长度必须等于 dim）
    payload={"text": "小明喜欢吃苹果", "ts": 1711440000}  # 任意 JSON
)
```

**Rust：**
```rust
let id = db.insert(&[0.12, -0.45, 0.78], json!({"text": "Hello"}))?;
```

### insert_with_id — 带自定义 ID 插入

适用于从外部系统导入数据时，保持原始 ID 不变。如果 ID 已存在会返回错误。

**Python：**
```python
db.insert_with_id(id=42, vector=[0.1, 0.2, 0.3, ...], payload={"source": "external"})
```

**Rust：**
```rust
db.insert_with_id(42, &[0.1, 0.2, 0.3], json!({"source": "external"}))?;
```

### reserve_nodes — 主动预留增量容量

`expected_nodes` 表示目标总节点数；`reserve_nodes(additional)` 表示从当前容量再增加可插入空间。两者只覆盖向量增量层、Payload/ID 映射、槽位与快速标签，不会提前构建 BQ、QuIVer、文本索引或图边。

```python
db.reserve_nodes(200_000)
```

```rust
db.reserve_nodes(200_000)?;
```

预留受 `memory_limit` 约束。预算不足、整数溢出或 allocator 拒绝时会在写 WAL 前失败；节点、ID、generation 和 WAL 不变。已成功取得但尚未使用的空 capacity 允许保留。

### Node.js 配置对象

Node.js 构造器只接受配置对象。旧数字位置参数已移除，并返回 `TDB_API_MIGRATION_REQUIRED`：

```ts
const db = new TriviumDB('my_data.tdb', {
  dim: 1024,
  dtype: 'f16',
  storageMode: 'mmap',
  expectedNodes: 3_600_000,
  memoryLimitMb: 28 * 1024,
  payloadCacheMb: 64,
  payloadCacheEntryMb: 8,
  autoBuildQuiver: false,
})
db.reserveNodes(200_000)
```

所有 Node 数量参数必须是 JavaScript 安全整数，负数、小数、NaN、Infinity 和超过 `Number.MAX_SAFE_INTEGER` 的值都会拒绝。

### batch_insert — 批量插入

一次性插入多个节点，返回所有新 ID 的列表。Python/Node 绑定会先转换并验证整个批次，再为整批预留核心容器，最后通过单个事务写入；任一向量、ID、容量预算或 WAL 步骤失败都不会产生半批数据。

**Python：**
```python
ids = db.batch_insert(
    vectors=[[0.1, 0.2, ...], [0.3, 0.4, ...]],
    payloads=[{"name": "A"}, {"name": "B"}]
)
```

### batch_insert_with_ids — 带自定义 ID 批量插入

**Python：**
```python
db.batch_insert_with_ids(
    ids=[100, 101],
    vectors=[[0.1, 0.2, ...], [0.3, 0.4, ...]],
    payloads=[{"name": "A"}, {"name": "B"}]
)
```

### get — 获取单个节点

按 ID 获取节点的完整视图，包含向量、元数据和边的数量。不存在时返回 `None`。

**Python：**
```python
node = db.get(42)
if node:
    print(node.id)         # 42
    print(node.vector)     # [0.1, 0.2, ...]
    print(node.payload)    # {"name": "Alice", ...}
    print(node.num_edges)  # 3
```

**Rust：**
```rust
if let Some(view) = db.get(42)? {
    println!("ID={}, edges={}", view.id, view.edges.len());
    println!("payload={:?}", view.payload);
}
```

### update_payload — 更新元数据

整体替换节点的 JSON 元数据（向量和图谱关系不受影响）。

**Python：**
```python
db.update_payload(id=42, payload={"text": "更新后的文本", "version": 2})
```

### update_vector — 更新向量

就地替换节点的向量（维度必须一致，元数据和图谱关系不受影响）。

**Python：**
```python
db.update_vector(vector=[0.5, 0.6, 0.7, ...], id=42)
```

### delete — 删除节点

**三层原子联删**：同时清除该节点的向量、元数据以及所有关联的图谱边（包括其他节点指向它的入边）。

**Python：**
```python
db.delete(42)
```

**Rust：**
```rust
db.delete(42)?;
```

> ⚠️ 删除操作不可逆。删除后，该节点的向量区间被逻辑置零，待 Compaction 时物理回收。

### get_payload — 轻量级获取元数据

只获取节点的 JSON Payload，不含向量，比 `get()` 更轻量。

**Python：**
```python
payload = db.get_payload(42)
if payload:
    print(payload["name"])  # "Alice"
```

**Node.js：**
```js
const payload = db.getPayload(42)
if (payload) console.log(payload.name)
```

### get_edges — 获取出边列表

获取节点的所有出向边（不含向量和 Payload）。

**Python：**
```python
edges = db.get_edges(42)
for e in edges:
    print(f"{e.target_id} ({e.label}, w={e.weight})")
```

**Node.js：**
```js
const edges = db.getEdges(42)
edges.forEach(e => console.log(`${e.targetId} (${e.label})`))
```

### contains — 节点存在检查

**Python：**
```python
if db.contains(42):     # 或用 42 in db
    print("节点存在")
```

**Node.js：**
```js
if (db.contains(42)) console.log('节点存在')
```

---

## 图谱操作

### link — 建立有向边

在两个节点之间建立一条有向带权边。边以 `(src, dst, label)` 三元组唯一；重复调用会更新 weight，不会增加重复边。两个端点必须已存在，weight 必须是有限 `f32`。标签长度最多为 65,535 个 UTF-8 字节，标签、Metadata、图索引容量与暂存峰值在写 WAL 前执行校验和预算准入。

**Python：**
```python
db.link(src=1, dst=2, label="knows", weight=0.95)
```

**Rust：**
```rust
db.link(1, 2, "knows", 0.95)?;
```

**参数说明：**

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `src` | `u64` | *必填* | 源节点 ID |
| `dst` | `u64` | *必填* | 目标节点 ID |
| `label` | `str` | `"related"` | 边的类型标签（自定义字符串） |
| `weight` | `f32` | `1.0` | 边的权重（支持负值，可用于表达抑制关系） |

> 💡 边是**有向**的。如需双向关系，需调用两次 `link()`：`link(A, B)` + `link(B, A)`。

### 精确边读取与 Upsert

边的唯一键是 `(src, dst, label)`。`get_edge/getEdge` 精确读取单边；`upsert_edge/upsertEdge` 创建或覆盖 `weight` 与任意 JSON `metadata`；`update_edge/updateEdge` 只更新已存在边的指定字段。边元数据会贯穿 WAL、v7 `.tdb` 文件、迁移和各语言绑定。

```rust
let metadata = serde_json::json!({"confidence": 0.98});
db.upsert_edge(1, 2, "knows", 0.95, metadata)?;
let edge = db.get_edge(1, 2, "knows");
db.update_edge(1, 2, "knows", Some(0.9), None)?;
```

```python
db.upsert_edge(1, 2, "knows", 0.95, {"confidence": 0.98})
edge = db.get_edge(1, 2, "knows")
db.update_edge(1, 2, "knows", weight=0.9)
```

```ts
db.upsertEdge(1, 2, 'knows', 0.95, { confidence: 0.98 })
const edge = db.getEdge(1, 2, 'knows')
db.updateEdge(1, 2, 'knows', 0.9)
```

### unlink — 断开边

移除从 `src` 到 `dst` 的**所有**边（无论 label 是什么）。

**Python：**
```python
db.unlink(src=1, dst=2)
db.unlink(src=1, dst=2, label="knows")  # 仅删除指定标签
```

**Rust：**
```rust
db.unlink(1, 2)?;
db.unlink_label(1, 2, "knows")?;
```

### neighbors — N 跳邻居

从指定节点出发，沿有向边进行广度优先遍历（BFS），返回 N 跳以内所有可达节点的 ID。

**Python：**
```python
neighbor_ids = db.neighbors(id=1, depth=2)  # 2 跳以内的所有邻居
knows_only = db.neighbors(id=1, depth=2, labels=["knows"])
```

**Rust：**
```rust
let ids = db.neighbors(1, 2);
let knows = db.neighbors_with_labels(1, 2, Some(&["knows".to_string()]));
```

返回顺序固定为 BFS 最短距离升序、同距离 NodeId 升序。`labels=None` 表示遍历全部边，空列表表示禁止扩散。

### get_incoming_edges — 获取完整入边

返回 `source_id`、`target_id`、`label` 和 `weight`，可按单个标签过滤。

```python
incoming = db.get_incoming_edges(id=2, label="knows")
```

```rust
let incoming = db.get_incoming_edges(2, Some("knows"));
```

### reachable — 确定性结构可达性

`reachable` 执行纯结构 BFS，不计算相关性 score。默认沿出边遍历，也可选择入边或双向遍历；每个目标只返回一条确定性的最短路径，结果包含完整 NodeId 路径和逐跳 label。

**Python：**
```python
paths = db.reachable(
    id=1,
    min_depth=1,
    max_depth=3,
    labels=["knows", "works_with"],
    direction="both",
    max_visited_nodes=10_000,
)
for item in paths:
    print(item.target_id, item.depth, item.path)
    print([(step.from_id, step.to_id, step.label) for step in item.steps])
```

**Rust：**
```rust
use triviumdb::graph::reachability::{ReachabilityConfig, ReachabilityDirection};

let paths = db.reachable(1, &ReachabilityConfig {
    min_depth: 1,
    max_depth: 3,
    labels: Some(vec!["knows".into(), "works_with".into()]),
    direction: ReachabilityDirection::Both,
    max_visited_nodes: 10_000,
})?;
```

**Node.js / TypeScript：**
```ts
const paths = db.reachable(1, {
  minDepth: 1,
  maxDepth: 3,
  labels: ['knows', 'works_with'],
  direction: 'both',
  maxVisitedNodes: 10_000,
})
```

`labels=None` 或省略表示全部 label，空列表表示禁止遍历。`min_depth=0` 可让源节点作为深度 0 结果返回。三个预算分别限制访问节点、结果和扫描边；详细接口在超限时保留已完成结果并设置 `truncated=true`。

- Rust：`reachable_detailed()`、`query_subgraph()`
- Python：`reachable_detailed()`、`query_subgraph()`
- Node.js：`reachableDetailed()`、`querySubgraph()`

详细可达性同时返回 `visited_nodes/visitedNodes`、`traversed_edges/traversedEdges` 与 `truncated`。逐跳结果包含遍历方向的 `from/to`、原始边权重和元数据。子图按 NodeId 和原始 `(source, target, label)` 确定性排序，并保持入边的原始方向。

### 图统计、校验与修复

`graph_stats/graphStats` 返回节点数、边数、孤立节点数和标签数。`validate_graph/validateGraph` 检查悬空边、重复三元组以及入边、入度、标签派生索引。Writer 可调用 `repair_graph_indexes/repairGraphIndexes` 清理无效边、从权威出边表重建派生索引并立即持久化。

### 自定义 ID Upsert 与 Node.js 原子事务

`upsert_with_id/upsertWithId` 在 ID 不存在时插入，存在时原子覆盖向量和 Payload。Node.js 的 `commitTransaction(operations)` 可在同一个 WAL-first 事务中混合 `insert`、`insertWithId`、`delete`、`updatePayload`、`updateVector`、`link/upsertEdge`、`unlink` 和 `unlinkLabel`；任何预检失败都不会应用部分操作。

---

## 搜索与召回

### search_graph_first — 图候选集内精确向量排名

GraphFirst 先由业务图查询产生 anchor ID 集合，再只在该集合内进行精确向量评分。输入 anchor 会按 NodeId 去重；超过 `max_anchor_nodes` 时直接报错，不会退化为全库搜索。

anchor 容器与有界 Top-K 堆在分配前申请共享工作区，最终命中集合确定后再解析和复制 Payload。精确检索采用相同的最终物化步骤；工作区余额不足返回结构化资源预算错误，失败和完成均归还预留。

```python
hits = db.search_graph_first(
    query_vector=[0.1, 0.2, 0.3],
    anchor_ids=[12, 18, 25],
    top_k=5,
    max_anchor_nodes=100_000,
)
```

```rust
let hits = db.search_graph_first(
    &[0.1, 0.2, 0.3],
    &[12, 18, 25],
    5,
    100_000,
)?;
```

```ts
const hits = db.searchGraphFirst(
  [0.1, 0.2, 0.3],
  [12, 18, 25],
  5,
  100_000,
)
```

该 API 保证 anchor 集合内 Top-K 完备，但不会使用 QuIVer ANN。查询维度不匹配、`top_k=0`、预算为 0 或去重后的 anchor 数量超预算时返回错误；不存在的 anchor ID 会被忽略。

### search_hybrid — 双路混合认知检索 (强推)

TriviumDB 核心杀手锏：引入稀疏文本表示（BM25/AC自动机）与稠密向量（Dense Vector）构成双路融合召回锚定，再在第二阶段进行图谱激活扩散。这极大弥补了纯向量检索容易导致的专有名词幻觉（Hallucination）。

固定混合检索的稀疏分数为 AC 关键词命中次数与 BM25 分数之和，再与向量排名进行 RRF 融合，`text_boost` 控制稀疏排名的融合权重。需要独立配置关键词权重时，可使用 TQL 的 `TEXT HYBRID ... AC_WEIGHT ...`；`TEXT AC` 与 `TEXT BM25` 分别提供独立文本召回。

**Python：**
```python
results = db.search_hybrid(
    query_vector=[0.10, -0.48, 0.80, ...], 
    query_text="Rust 内存安全",
    top_k=5,
    expand_depth=2,
    min_score=0.1,
    hybrid_alpha=0.7  # 0.7 偏向量，0.3 偏精确文本
)
for hit in results:
    print(f"[{hit.id}] score={hit.score:.3f} | {hit.payload}")
```

### search — 纯向量图扩散检索 (基础)

TriviumDB 的基础检索能力（退化态）：**先用核心稠密向量相似度找到锚点，再沿图谱关系向外扩散**。

**Python：**
```python
results = db.search(
    query_vector=[0.10, -0.48, 0.80, ...],  # 查询向量
    top_k=5,            # 向量阶段返回的锚点数量
    expand_depth=2,     # 图谱扩散跳数（0 = 纯向量检索）
    min_score=0.5       # 最低相似度阈值
)
for hit in results:
    print(f"[{hit.id}] score={hit.score:.3f} | {hit.payload}")
```

**Rust：**
```rust
let results = db.search(&[0.10, -0.48, 0.80], 5, 2, 0.5)?;
for hit in &results {
    println!("[{}] score={:.3} {:?}", hit.id, hit.score, hit.payload);
}
```

**参数说明：**

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `query_vector` | `list[float]` | *必填* | 查询向量 |
| `top_k` | `int` | `5` | 向量阶段返回的最相似节点数 |
| `expand_depth` | `int` | `0` | 图谱扩散深度。设为 0 则退化为纯向量检索 |
| `min_score` | `float` | `0.5` | 余弦相似度下限，低于此值的结果被过滤 |

**返回值 `SearchHit`：**

| 属性 | 类型 | 说明 |
|------|------|------|
| `id` | `u64` | 命中节点的 ID |
| `score` | `f32` | 相似度得分（余弦相似度或扩散热度） |
| `payload` | `dict` | 节点的 JSON 元数据 |

### search_batch — 批量并行 ANN 检索

同一个数据库实例一次接收多条向量查询，由 Rust 共享线程池并行执行。所有查询在执行前完成整批校验；任一向量维度错误或包含 NaN/Infinity 时整批失败，不返回部分结果。外层结果顺序严格对应输入顺序。

**Python：**
```python
results = db.search_batch(
    query_vectors=[[0.10, -0.48, 0.80], [0.72, 0.11, -0.35]],
    top_k=10,
    recall_k=0,
    rerank_k=0,
    expand_depth=0,
    min_score=0.0,
    parallelism=0,
)
```

Python 在整个 Rust 查询阶段释放 GIL，避免逐条 FFI 调用造成的吞吐损失。

**Node.js：**
```javascript
const results = await db.searchBatch(queryVectors, 10, 0, 0.0)
```

Node.js 接口返回 Promise，ANN 工作在后台线程执行，不阻塞事件循环。参数依次为 `queryVectors`、`topK`、`parallelism`、`minScore`。

**Rust：**
```rust
let results = db.search_batch(
    &queries,
    &SearchConfig {
        top_k: 10,
        expand_depth: 0,
        min_score: 0.0,
        ..Default::default()
    },
    &BatchSearchConfig { parallelism: 0 },
)?;
```

| 约束 | 语义 |
|------|------|
| `parallelism = 0` | 自动选择并发度 |
| `parallelism = 1..64` | 指定批内最大并发度 |
| `parallelism > 64` | 明确拒绝，防止过度并行 |
| 空查询批次 | 返回空列表 |
| `top_k = 0` | 明确拒绝 |
| fatigue | 明确拒绝，批量 API 仅支持无状态查询 |
| 生命周期 | close 阻止新批次，并等待已进入批次完成 |

**检索流程：**
```
查询向量 ──→ [向量索引层] ──→ Top-K 锚点
                                  │
                                  ▼
              [图谱扩散层] ──→ N 跳邻居（Spreading Activation）
                                  │
                                  ▼
                           最终排序结果
```

---

### search_advanced — 认知管线检索

内置认知管线的全功能便利入口。通过 `SearchConfig` 参数化控制 AC+BM25 文本召回、FISTA 残差寻隐、SA-PPR 有限深度扩散、DPP 多样性采样与疲劳不应期等高级特性。各组件独立启停，执行顺序采用官方固定模板。需要自由编排无状态组件时，使用 TQL 的 `TEXT`、`RESIDUAL`、`SA_PPR_CONFIG`、`DIVERSIFY` 和 `TOPICS`。两条入口共享底层算法；SA-PPR 采用带个性化重启的有限深度 Spreading Activation。

**Python：**
```python
results = db.search_advanced(
    query_vector=[0.10, -0.48, 0.80, ...],
    top_k=10,
    recall_k=200,                   # 初始稠密/稀疏召回池
    rerank_k=50,                    # SA-PPR/FISTA/DPP 前候选池
    expand_depth=2,
    min_score=0.1,
    teleport_alpha=0.15,            # SA-PPR 个性化重启比例
    enable_advanced_pipeline=True, # 总开关
    enable_sparse_residual=True,   # FISTA 影子查询
    fista_lambda=0.1,
    fista_threshold=0.3,
    enable_dpp=True,               # DPP 多样性采样
    dpp_quality_weight=1.0,
    expand_labels=["knows", "related"], # None=全部，[]=禁止图扩散
    max_edges_per_node=20,         # 每节点按绝对权重仅保留最强 20 条边
    min_edge_weight=0.3,           # 过滤绝对权重低于 0.3 的弱边
    edge_direction="both",         # out / in / both
)
for hit in results:
    print(f"[{hit.id}] score={hit.score:.3f} | {hit.payload}")
```

图扩散先按方向、标签和 `abs(weight)` 阈值过滤，再按 `abs(weight)` 降序、目标 ID 升序、标签字典序稳定排序并应用 `max_edges_per_node`。传播预算只在最终保留的边之间重新归一化。`inhibition` 和负权边同样按绝对值参与阈值与强边选择，但继续传播负能量。

**Node.js：**
```javascript
const results = db.searchAdvanced(queryVector, {
    topK: 10,
    expandDepth: 2,
    teleportAlpha: 0.15,
    enableAdvancedPipeline: true,
    enableSparseResidual: true,
    enableDpp: true,
    expandLabels: ['knows', 'related'],
});
```

**Rust：**
```rust
use triviumdb::database::SearchConfig;

let config = SearchConfig {
    top_k: 10,
    expand_depth: 2,
    min_score: 0.1,
    teleport_alpha: 0.15,
    enable_advanced_pipeline: true,
    enable_sparse_residual: true,
    fista_lambda: 0.1,
    fista_threshold: 0.3,
    enable_dpp: true,
    dpp_quality_weight: 1.0,
};
let results = db.search_advanced(&query_vec, &config)?;
```

**SearchConfig 参数说明：**

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `top_k` | `usize` | `5` | 最终返回的结果数量 |
| `recall_k` | `usize` | `0`（自动） | 初始召回池；自动值为 `max(top_k × 8, 64)` |
| `rerank_k` | `usize` | `0`（自动） | 高级处理候选池；自动值为 `max(top_k × 4, 32)`，且不超过 `recall_k` |
| `expand_depth` | `usize` | `2` | SA-PPR 有限深度扩散跳数 |
| `min_score` | `f32` | `0.1` | 余弦相似度下限 |
| `teleport_alpha` | `f32` | `0.0` | PPR 回跳概率 (0.0~1.0)，越高越抑制深层扩散 |
| `enable_advanced_pipeline` | `bool` | `false` | 认知管线总开关，关闭时退化为普通检索 |
| `enable_sparse_residual` | `bool` | `false` | 启用 FISTA 残差寻隐 + 影子查询 |
| `fista_lambda` | `f32` | `0.1` | FISTA L1 正则化系数 |
| `fista_threshold` | `f32` | `0.3` | 残差范数超过此值时触发影子查询 |
| `enable_dpp` | `bool` | `false` | 启用 DPP 多样性采样 |
| `dpp_quality_weight` | `f32` | `1.0` | DPP 质量权重幂次 |
| `enable_text_hybrid_search`| `bool`| `false`| 是否开启 BM25/AC 双路混合搜索 |
| `text_boost` | `f32` | `1.5` | 加权 RRF 中的稀疏排名权重；不再与余弦分数直接相加 |
| `hybrid_alpha` | `f32` | `0.7` | 混合检索中向量权重 (0~1)，(1-alpha) 为稀疏文本权重 |
| `custom_query_text` | `str`| `None` | (可选) 手动传入用于文本匹配的原始文本 |
| `force_brute_force` | `bool` | `false`| 强制使用暴力搜索，禁用 QuIVer 图索引（用于基准测试和需要精确结果的场景） |

> `search_advanced()` 提供“组件可开关、顺序固定”的便利模板：文本/向量召回 → FISTA 残差 → SA-PPR → Payload 过滤 → DPP。TQL 提供“组件与顺序均可编排”的无状态管线，并增加 NMF 主题分配；Refractory Fatigue 和 Hook 仍保留在高级 API，因为它们具有状态或回调语义。

> 所有参数均执行安全钳位：`teleport_alpha` 被约束在 [0, 1]，`fista_lambda` 在 [1e-5, 100]，`dpp_quality_weight` 在 [0, 10]。

> 💡 当 `enable_advanced_pipeline = false` 时，`search_advanced` 的行为与 `search` 完全一致。

---

## Hook 扩展系统

TriviumDB v0.6.0 新增的检索管线 Hook 系统，允许开发者在 6 个关键阶段注入自定义逻辑，高度自定义检索管线。

### 管线 Hook 点整体架构

```text
  查询输入
      │
  🔌 #1 on_pre_search        — 查询预处理（改写向量 / 修改配置 / 提前终止）
      │
  🔌 #2 on_custom_recall     — 自定义召回（可替代内置召回）
      │
  ┌── 内置召回管线 ──┐
  │  L1 文本稀疏召回  │
  │  L2 向量稠密召回  │
  │  L3 布隆预过滤    │
  └──────────────────┘
      │
  🔌 #3 on_post_recall       — 召回后处理（业务过滤 / 分数调权）
      │
  🔌 #4 on_pre_graph_expand  — 图扩散前拦截
      │
  ┌── 图谱扩散 ──────┐
  │  L6 SA-PPR 扩散   │
  │  L7 不应期/抑制    │
  └──────────────────┘
      │
  🔌 #5 on_rerank            — 自定义重排序
      │
  🔌 #6 on_post_search       — 最终后处理
      │
  返回结果
```

### load_ffi_hook — 加载 C/C++ 动态库插件

加载一个导出了 C ABI 符号的动态库（`.so` / `.dll` / `.dylib`）作为检索管线 Hook。动态库中的所有符号均为可选，未找到的符号将自动被无操作替代。

**Python：**
```python
db.load_ffi_hook("./libmy_plugin.so")
results = db.search(query_vec)  # 自动经过 C++ Hook
```

**Node.js：**
```javascript
db.loadFfiHook('./libmy_plugin.so')
const results = db.search(queryVec)  // 自动经过 C++ Hook
```

**Rust：**
```rust
use triviumdb::hook::FfiHook;

let ffi_hook = FfiHook::load("./libmy_plugin.so")?;
db.set_hook(ffi_hook)?;
```

### clear_hook — 清除已注册 Hook

清除当前的 Hook，恢复为默认的空实现 `NoopHook`。

**Python：**
```python
db.clear_hook()
```

**Node.js：**
```javascript
db.clearHook()
```

**Rust：**
```rust
db.clear_hook();
```

### search_with_context — 带管线上下文的检索

与 `search` 相同的检索能力，但额外返回 `HookContext` 对象，包含管线各阶段的计时统计和 Hook 注入的自定义数据。

**Python：**
```python
hits, ctx = db.search_with_context(
    query_vector=[0.10, -0.48, 0.80, ...],
    top_k=10,
    expand_depth=2,
    min_score=0.1,
)

print(ctx.timings)
# {'hook_pre_search': 0.012, 'hook_custom_recall': 0.001, 'graph_expand': 2.34, ...}

print(ctx.custom_data)   # Hook 注入的自定义数据
print(ctx.aborted)       # 管线是否被 Hook 提前终止
```

**Node.js：**
```javascript
const { hits, context } = db.searchWithContext(queryVec, {
    topK: 10,
    expandDepth: 2,
    minScore: 0.1,
})

console.log(context.timings)     // { hook_pre_search: 0.012, graph_expand: 2.34, ... }
console.log(context.customData)  // Hook 注入的自定义数据
console.log(context.aborted)     // 管线是否被提前终止
```

**Rust：**
```rust
use triviumdb::database::SearchConfig;

let config = SearchConfig {
    top_k: 10,
    expand_depth: 2,
    ..Default::default()
};
let (results, ctx) = db.search_hybrid_with_context(None, Some(&query_vec), &config)?;

for (stage, dur) in &ctx.stage_timings {
    println!("{}: {:.2}ms", stage, dur.as_secs_f64() * 1000.0);
}
```

### Rust 原生 Hook Trait

在 Rust 中，开发者可以直接实现 `SearchHook` trait 来创建自定义 Hook：

```rust
use triviumdb::hook::{SearchHook, HookContext};
use triviumdb::database::SearchConfig;
use triviumdb::node::SearchHit;

struct MyHook;

impl SearchHook for MyHook {
    fn on_pre_search(
        &self,
        query_vector: &mut Vec<f32>,
        config: &mut SearchConfig,
        ctx: &mut HookContext,
    ) {
        // 修改查询向量、调整配置等
        ctx.custom_data = serde_json::json!({"user_id": "u_12345"});
    }

    fn on_rerank(
        &self,
        results: &mut Vec<SearchHit>,
        _ctx: &mut HookContext,
    ) -> Option<Vec<SearchHit>> {
        // 自定义重排序逻辑
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        None // 返回 None 表示使用原地修改，返回 Some 替换结果
    }
}

// 注册 Hook
db.set_hook(MyHook)?;
```

默认 `NoopHook` 的回调为空实现。查询通过共享 Hook 槽取得本次执行的所有权，调用方可按需注册和替换扩展。

> ⚠️ **FFI 插件安全提示**：`FfiHook` 加载的动态库将在进程内执行任意代码，请确保动态库来源可信。

---

## 元数据过滤

### filter_where — 高级条件过滤

使用类 MongoDB 语法对所有节点的 Payload 进行条件过滤。返回匹配的 `NodeView` 列表。

**Python：**
```python
# 单条件
adults = db.filter_where({"age": {"$gt": 18}})

# 多条件组合
results = db.filter_where({
    "$and": [
        {"age": {"$lt": 30}},
        {"role": {"$in": ["admin", "mod"]}}
    ]
})

# OR 组合
results = db.filter_where({
    "$or": [
        {"age": {"$lt": 18}},
        {"role": "admin"}
    ]
})
```

**支持的操作符：**

| 操作符 | 含义 | 值类型 | 示例 |
|--------|------|--------|------|
| `$eq` | 等于 | 任意 | `{"name": {"$eq": "Alice"}}` 或直接 `{"name": "Alice"}` |
| `$ne` | 不等于 | 任意 | `{"status": {"$ne": "deleted"}}` |
| `$gt` | 大于 | 数字或字符串 | `{"age": {"$gt": 18}}` |
| `$gte` | 大于等于 | 数字或字符串 | `{"name": {"$gte": "Bob"}}` |
| `$lt` | 小于 | 数字或字符串 | `{"age": {"$lt": 30}}` |
| `$lte` | 小于等于 | 数字或字符串 | `{"name": {"$lte": "David"}}` |
| `$before` | 早于 | RFC3339 字符串 | `{"createdAt": {"$before": "2026-08-22T00:00:00Z"}}` |
| `$beforeEq` | 早于或等于 | RFC3339 字符串 | `{"createdAt": {"$beforeEq": "2026-08-22T00:00:00Z"}}` |
| `$after` | 晚于 | RFC3339 字符串 | `{"createdAt": {"$after": "2026-08-21T00:00:00Z"}}` |
| `$afterEq` | 晚于或等于 | RFC3339 字符串 | `{"createdAt": {"$afterEq": "2026-08-21T00:00:00Z"}}` |
| `$in` | 包含于列表 | 数组 | `{"role": {"$in": ["admin", "mod"]}}` |
| `$nin` | 不在列表中 | 数组 | `{"status": {"$nin": ["banned", "deleted"]}}` |
| `$startsWith` | 前缀匹配 | 字符串 | `{"folder": {"$startsWith": "/地理"}}` |
| `$contains` | 包含子串 | 字符串 | `{"tag": {"$contains": "重要"}}` |
| `$exists` | 字段是否存在 | 布尔 | `{"email": {"$exists": true}}` |
| `$size` | 数组长度 | 正整数 | `{"tags": {"$size": 3}}` |
| `$all` | 数组包含所有 | 数组 | `{"tags": {"$all": ["A", "B"]}}` |
| `$type` | 字段类型 | 字符串 | `{"age": {"$type": "number"}}` |
| `$and` | 逻辑与 | 条件数组 | `{"$and": [{...}, {...}]}` |
| `$or` | 逻辑或 | 条件数组 | `{"$or": [{...}, {...}]}` |

数字只与数字比较，字符串只与字符串按原始 Unicode 字典序比较，类型不同视为不匹配。RFC3339 操作符会解析时区并按绝对时间比较；非法查询操作数直接报错，节点中的脏时间字段视为不匹配。范围查询当前不会使用属性 Hash 索引加速。

**字符串匹配示例（v0.7.2 新增）：**

```python
# 前缀匹配：匹配 /地理 及其所有子路径
results = db.filter_where({"folder": {"$startsWith": "/地理"}})

# 多前缀 OR 组合：匹配多个路径前缀
results = db.filter_where({
    "$or": [
        {"folder": {"$startsWith": "/地理"}},
        {"folder": {"$startsWith": "/天文"}}
    ]
})

# 子串包含
results = db.filter_where({"description": {"$contains": "关键词"}})

# search() 中使用 payload_filter 前缀过滤
results = db.search(
    query_vector=[0.1, ...],
    payload_filter={"folder": {"$startsWith": "/地理"}}
)
```

**Rust：**
```rust
use triviumdb::filter::Filter;

let filter = Filter::And(vec![
    Filter::Gt("age".into(), 18.0),
    Filter::In("role".into(), vec![json!("admin"), json!("mod")]),
]);
let results = db.filter_where(&filter);
```

---

## TQL、Prepared 与一等值

TQL 公共 API 提供 **自由 DIY 混合查询能力**。调用者可以用一条查询按需组合向量源、属性索引、图扩展、图算法、路径、集合代数、迭代、聚合与重排；Prepared TQL 支持同一管线重复绑定业务参数。

### tql / tql_values

支持 MATCH、OPTIONAL MATCH、FIND、SEARCH、WITH 管线、聚合、图算法、路径与集合代数。Rust `tql()` 返回同时承载节点、边、标量、Object 和 Path/List 的一等值；`tql_values()` 为兼容别名，`tql_nodes()` 提供节点绑定容器。Python/Node 的 `tql` 同步承载一等值。

普通查询、WITH 与单跳具名边共享分组、COUNT(expr)、DISTINCT、别名排序与分页。返回列独立保存；省略 AS 时使用表达式名，例如 `Count(*)`。WITH 使用实例行数、内存工作区与取消配置。完整语义见 [TQL 返回处理](tql-reference.md#return--order-by--limit--offset)。

WITH 按行保存求值结果与实体身份，后续扩展、过滤和返回使用保存的值。同一阶段的投影同时赋值，完整检查表达式作用域；节点与对象返回别名支持字段排序。Rust `tql_nodes()` 同样保留被返回的源节点别名。物理管线的 `NodeRow.bindings` 承载共享的 `BindingValue` 快照，投影内存纳入查询与共享资源预算。

MATCH 内联 `id` 和节点的 `.id` 属性使用完整无符号节点 ID；`FIND` 与 `MATCHES` 文档条件读取 Payload 字段。单跳边变量在 WHERE、RETURN、COALESCE、聚合和空值判断中保持边绑定；OPTIONAL MATCH 为符合起点条件且无合格边的记录补齐 Null 列。

MATCH 按物理边保留结果行重数：不同标签的平行边分别计行，双向单跳中的同一自环计一次；属性索引与反向计划保持这一语义。路径采用显式栈执行，有边模式的节点、邻边扫描、路径展开和回溯共享 100,000 步预算，每步检查取消与期限；路径栈和 visited 集合的峰值参与工作区准入。

**Python：**
```python
# 图遍历
rows = db.tql('MATCH (a)-[:knows]->(b) WHERE b.age > 18 RETURN b')
for row in rows:
    node = row.row["b"]    # {"id": ..., "payload": {...}, "num_edges": ...}
    print(node["payload"])

# 文档过滤
rows = db.tql('FIND {type: "event", heat: {$gte: 0.7}} RETURN *')

# 带内联属性 + WHERE
rows = db.tql('MATCH (a {id: 1})-[]->(b) WHERE b.score >= 0.8 RETURN a, b')
```

**Node.js：**
```js
const rows = db.tql('MATCH (a)-[:knows]->(b) WHERE b.age > 18 RETURN b')
rows.forEach(row => console.log(row.b.payload))
```

**Rust：**
```rust
let rows = db.tql("MATCH (a)-[:knows]->(b) WHERE b.age > 20 RETURN b")?;
for row in &rows {
    if let Some(node) = row.get("b") {
        if let Some(node) = node.as_node() {
            println!("节点 (Node) {}: {:?}", node.id, node.payload);
        }
    }
}
```

### prepare_tql / execute_prepared_tql

```python
prepared = db.prepare_tql(
    "FIND {kind: \"note\"} RETURN $bonus + 1 AS score"
)
rows = db.execute_prepared_tql(prepared, {"bonus": 4})
```

```ts
const prepared = db.prepareTql('FIND {kind: "note"} RETURN $bonus + 1 AS score')
const rows = db.executePreparedTql(prepared, { bonus: 4 })
```

Prepared 参数只接受 null/bool/string/number；缺参、额外参数、数组/对象和非有限数值明确失败。

SEARCH 向量的字面量与数值参数按数据库 dtype 转换，u64 分量保持整数位模式。RANK 和 RESIDUAL 的向量字面量采用相同转换。Rust 的 `QueryEntry::Search.vector`、`RankClause.vector` 和 `ResidualStage.vector` 使用 `Vec<serde_json::Number>`；`pipeline::lower_search_entry()` 返回 `Result`，调用方传播数值转换错误。

### 一等值映射

| TQL 值 | Python | Node |
|---|---|---|
| Node | dict | object，NodeId 为字符串 |
| Edge | `{type, source, target, label, weight, metadata}` | `{type, source, target, label, weight, metadata}`，NodeId 为字符串 |
| Int/UInt | 精确 int | 安全整数为 number；超范围整数为十进制字符串 |
| Float/String/Bool | 原生标量 | 原生标量 |
| Path | `list[int]` | `string[]`，避免 u64 精度损失 |
| List | list | array |
| Object | dict，嵌套整数保持精度 | object，嵌套超安全整数转十进制字符串 |
| Null | None | null |

Rust 使用 `TqlValue::Object(serde_json::Map<String, serde_json::Value>)` 承载对象。数组和对象作为完整 JSON 值参与空值判断、COALESCE、去重与分组；节点和边按实体身份参与表达式求值。

### tql_mut — 执行 TQL 写操作

支持 CREATE / SET / DELETE / DETACH DELETE 语法，返回受影响行数和新创建的节点 ID。

多字段 SET 按节点累计，重复字段采用最后一次赋值。MATCH … CREATE 每条匹配建立独立的新节点作用域，零匹配返回零次创建。SET VECTOR 按目标 dtype 检查，u64 整数字面量覆盖完整范围；任一目标失败时整个 mutation 保持原子性。

DETACH DELETE 按去重后的节点集合生成统一事务删除，由内核清理入边、出边、平行边与自环。相连节点和同一节点的重复匹配共享同一个 WAL 提交边界，集合外的节点与其剩余关系保持完整。

**Python：**
```python
# 创建节点
result = db.tql_mut('CREATE (a {name: "Alice", age: 30})')
print(result["affected"])      # 1
print(result["created_ids"])   # [1]

# 更新属性
db.tql_mut('MATCH (a {name: "Alice"}) SET a.age == 31')

# 删除节点
db.tql_mut('MATCH (a {name: "Alice"}) DELETE a')

# 删除节点及其所有关联边
db.tql_mut('MATCH (a {type: "temp"}) DETACH DELETE a')
```

**Node.js：**
```js
const result = db.tqlMut('CREATE (a {name: "Alice", age: 30})')
console.log(result.affected)     // 1
console.log(result.createdIds)   // [1]
```

**返回值：**

| 字段 | 类型 | 说明 |
|------|------|------|
| `affected` | `int` | 受影响的节点数 |
| `created_ids` / `createdIds` | `list[int]` / `number[]` | CREATE 新建的节点 ID 列表 |

**语法规范：**

```
Query      := MATCH Pattern (WHERE Condition)? RETURN ReturnList
            | MATCH Pattern (WHERE Condition)? (SET SetExpr | DELETE Ident | DETACH DELETE Ident)
            | CREATE NodePat
            | FIND JsonFilter RETURN ReturnList
Pattern    := NodePat (EdgePat NodePat)*
NodePat    := '(' Ident? ('{' PropList '}')? ')'
EdgePat    := '-[' (':' Ident)? ']->' 
Condition  := CompareExpr ((AND | OR) CompareExpr)*
ReturnList := Ident (',' Ident)* | '*'
```

> 💡 当起始节点已知 ID 时，强烈建议将 `id` 写入节点属性过滤器。主键 `id` 走 **O(1) 哈希短路扫描**，而 `type` 等非主键字段会触发 O(N) 全表扫描（除非已建立属性索引）。

> MATCH 使用显式有向模式；管线 EXPAND 和 Reachability 另行支持 OUTGOING/INCOMING/BOTH。

---

## 持久化索引管理与观测

四类属性索引均持久化到 `.pidx` 并由 Planner 透明选择：

| 类型 | Python | Node | Rust |
|---|---|---|---|
| Hash | `create_index` | `createIndex` | `create_index` |
| Ordered ART | `create_ordered_index` | `createOrderedIndex` | `create_ordered_index` |
| Composite ART | `create_composite_index` | `createCompositeIndex` | `create_composite_index` |
| Roaring Bitmap | `create_bitmap_index` | `createBitmapIndex` | `create_bitmap_index` |

删除接口使用对应 `drop_*` 名称。`index_info/indexInfo` 返回 kind、完整 fields、entry/distinct/null 计数并稳定排序。

Ordered 索引提供完整排序候选时要求字段覆盖全部活动节点、类型统一为数字或字符串，并满足数值精度边界；其他情况按 Payload 排序。复合索引的前缀与范围访问检查后续字段的覆盖情况，缺失字段或容器值使访问路径回退到完整候选扫描。完整标量等值条件继续使用复合索引并执行事实复核。复合唯一约束使用保持 JSON 标量类型和值的完整键，精确区分相邻大整数；打开数据库时从 Payload 事实恢复约束映射。

Ordered 范围候选按边界类型访问 ART 子树，再执行 LIMIT/OFFSET；数字与字符串混合存储时保持范围查询结果完整。复合唯一约束先验证事务最终键集合，提交阶段统一移除旧所有者、安装最终所有者，支持同一节点多次改键、删除重建与多节点交换。

Hash、Ordered、Bitmap、Composite、N-gram 及唯一索引创建共享预算准入。构建先检查冷记录解析与 posting/键构建峰值，在隔离候选中验证并建立索引，成功后安装到实例。预算不足、非法字段值或唯一键重复时保留已有索引、约束、Payload 缓存和 WAL。重复创建已有同类定义保持幂等。

### create_index — 创建 Hash 属性索引

对指定的 JSON Payload 字段建立 O(1) 倒排索引。创建时自动回填全表现有数据，后续 insert / update_payload / delete 自动维护索引一致性。

**Python：**
```python
db.create_index("name")    # 之后 tql('FIND {name: "Alice"} RETURN *') 使用 O(1) 索引
db.create_index("type")
```

**Node.js：**
```js
db.createIndex('name')
db.createIndex('type')
```

**Rust：**
```rust
db.create_index("name");
```

### drop_index — 删除 Hash 属性索引

删除指定字段的 Hash 索引及其唯一约束定义。查询按剩余可用索引或 O(N) 扫描执行；删除唯一索引后，字段允许写入重复值，关闭和重开保持此定义。

**Python：**
```python
db.drop_index("name")
```

**Node.js：**
```js
db.dropIndex('name')
```


## 存储格式观测与迁移错误

### 三端结构化错误

内核的 `TriviumError::code()` 和 `details()` 提供稳定错误码与类别所需字段。Python 的内核异常提供 `code: str`、`details: dict[str, Any]`；Node 的同步异常与异步 Promise 拒绝提供同名属性。消息用于诊断与展示，程序通过错误码和字段处理重试、输入修正与迁移。

| 情况 | `code` | `details` 字段 |
| --- | --- | --- |
| 向量维度 | `TDB_DIMENSION_MISMATCH` | `expected`、`got` |
| 节点不存在/重复 | `TDB_NODE_NOT_FOUND` / `TDB_NODE_ALREADY_EXISTS` | `id` |
| 唯一约束 | `TDB_UNIQUE_CONSTRAINT` | `fields`、`existing_id`、`conflicting_id` |
| Prepared 参数 | `TDB_MISSING_QUERY_PARAMETER` / `TDB_UNEXPECTED_QUERY_PARAMETER` | `parameter` |
| 只读操作 | `TDB_READ_ONLY` | `operation` |
| 查询行预算 | `TDB_QUERY_BUDGET` | `budget` |
| Payload 查询预算 | `TDB_PAYLOAD_BUDGET_EXCEEDED` | `dimension`、`used`、`budget` |
| 实例容量准入 | `TDB_CAPACITY_RESERVATION_REJECTED` | `requested_nodes`、`estimated_bytes`、`current_bytes`、`memory_limit` |
| 共享资源准入 | `QueueFull` | `resource`、`requested`、`used`、`limit` |
| 文件系统 | `TDB_IO` | `io_kind`、`os_code`；系统码可为 `null` |
| 数据库格式版本 | `TDB_UNSUPPORTED_DATABASE_VERSION` | `found`、`minimum_supported`、`current` |
| API 迁移 | `TDB_API_MIGRATION_REQUIRED` | `removed_api`、`replacement` |

完整类别与字段由 [Rust 错误模型](../src/error.rs)及 [Node 类型声明](../triviumdb.d.ts)列出。Python 的 `TriviumDBError` 继承 `RuntimeError`；已有只读、恢复、代际和资源异常继承它。`InvalidInputError`、`CapacityError`、`SnapshotDestinationError` 分别继承 `ValueError`、`MemoryError`、`FileExistsError`，并提供同样的错误码与详情。SDK 参数提取阶段沿用各语言的参数类型异常。

Node 错误详情中的安全整数使用 Number，超出 `Number.MAX_SAFE_INTEGER` 的整数使用十进制字符串；Python 与 Rust 保留原始整数精度。已有资源错误的 `resource/requested/used/limit` 直接属性继续提供，字段内容与 `details` 相同。

```python
try:
    db.execute_prepared_tql(prepared, {})
except triviumdb.TriviumDBError as error:
    if error.code == "TDB_MISSING_QUERY_PARAMETER":
        required_parameter = error.details["parameter"]
```

`storage_info/storageInfo` 是只读诊断入口，返回产品版本、`.tdb` 当前/最低版本、WAL/Property/Graph/QuIVer/Text/Manifest 格式、dim、node count、访问模式、估算内存和 sidecar 存在状态。它不会创建、修复或重写文件。

```python
info = db.storage_info()
print(info["database_format_current"])  # 10
print(info["property_index_format"])    # 8
print(info["sidecars"])
```

`tql_mut` 读查询、patch 普通对象简写、Node 数字构造参数分别返回迁移错误；Rust 同步配置通过 `Config.sync_mode` 提供。ReadWrite 打开支持完整校验并原子迁移 v2 WAL，当前 v3 WAL 按帧流式恢复。ReadOnly/Immutable 保持物理零写，旧格式升级由 Writer 完成。无头、未知版本及非法迁移输入返回结构化错误。

## 持久化与压缩

### flush — 手动落盘

将当前内存中的全部数据写入 `.tdb` 文件。安全写入流程：先写临时文件 → fsync → 原子 rename → 清除 WAL。

**Python：**
```python
db.flush()
```

### WAL 同步模式

通过构造函数参数或运行时方法切换 WAL（Write-Ahead Log）的同步策略：

| 模式 | 安全性 | 性能 | 说明 |
|------|--------|------|------|
| `"full"` | ★★★ | 最慢 | 每条写入后 fsync，断电零丢失 |
| `"normal"` | ★★☆ | 均衡 | flush 到 OS 缓冲区，操作系统崩溃可能丢少量数据（**默认**） |
| `"off"` | ★☆☆ | 最快 | 不主动 flush，仅适合测试/批量导入 |

**运行时切换：**
```python
db.set_sync_mode("full")   # 切到最安全模式
db.set_sync_mode("off")    # 批量导入时临时提速
```

### enable_auto_compaction — 后台自动压缩

向进程共享计时调度器登记周期压实任务，实际工作进入所属 `ResourceRuntime` 的计算池。任务取得维护名额与 MemTable 写锁后保存数据，成功后清理 WAL；资源不足或写锁繁忙时延期。`instance_stats().compaction` 报告执行次数、延期原因和最近故障。

**Python：**
```python
db.enable_auto_compaction(interval_secs=30)  # 间隔必须大于 0
# db.enable_auto_compaction(interval_secs=0)  # ValueError：拒绝忙循环
db.disable_auto_compaction()                 # 撤销周期登记并等待已派发任务完成
```

**Rust：**
```rust
db.enable_auto_compaction(Duration::from_secs(30))?;
db.disable_auto_compaction();
```

### compact — 手动强制压实 (Manual Compaction)

主动触发一次全量数据重写与压实。**此调用会阻塞当前线程**，直到所有的内存数据被安全落盘，并彻底截断清理旧的 WAL 文件。
为了极致的崩溃安全性，执行压实时会短暂阻塞前台读写。强烈建议在关闭了自动压缩后，于业务低峰期（如凌晨调度）执行此方法。

**Python：**
```python
db.compact()
```

**Rust：**
```rust
db.compact()?;
```


---

## 多实例资源运行时与诊断

多个数据库通过 `open_with_runtime`（Rust）或构造参数 `runtime`（Python/Node）共享一个运行时。

| Rust `ResourceLimits` | Python `ResourceRuntime` 参数 | Node `ResourceRuntime` 选项 | 含义 |
| --- | --- | --- | --- |
| `memory_limit` | `memory_limit_mb` | `memoryLimitMb` | 声明配额与已接入工作区的总预算；0 表示不限 |
| `maintenance_memory_limit` | `maintenance_memory_limit_mb` | `maintenanceMemoryLimitMb` | 总预算中保护的维护额度；有限总额下 0 自动取八分之一，至少 1 字节 |
| `max_threads` | `max_threads` | `maxThreads` | 工作线程总上限，最大 64；0 按可用并行度选择 |
| `max_active_tasks` | `max_active_tasks` | `maxActiveTasks` | 前台根任务名额；0 表示不限 |

Rust 内存参数单位为字节，Python/Node 参数单位为 MiB。有共享内存总额时，每个数据库声明非零实例配额。维护根任务按 `max_threads` 单独准入。默认数据库打开入口使用进程共享运行时，其数据库配额与任务名额不限，缓存计算池的工作线程总数最多为 64。

| Rust / Python | Node.js | 返回内容 |
| --- | --- | --- |
| `runtime.stats()` | `runtime.stats()` | 共享配额、工作区、线程、任务、维护计数与可用预算余额 |
| `db.resource_stats()` | `db.resourceStats()` | 所属运行时的共享统计 |
| `db.instance_stats()` | `db.instanceStats()` | 本实例路径、配置配额、内存采样、关闭状态及周期压实状态 |

`InstanceStats.memory` 包含 `reserved_database_bytes`、`estimated_memory_bytes`、`node_count`。MemTable 写锁繁忙时返回 `None` / `null`，表示本次采样暂不可用。共享余额由 `available_foreground_bytes`、`available_maintenance_bytes`、`available_task_slots` 提供，未设置对应上限时为 `None` / `null`。

运行时资源拒绝在 Rust 中返回 `ResourceBudgetExceeded { resource, requested, used, limit }`；Python 抛出 `ResourceBudgetError`，Node 错误码为 `QueueFull`，两端均携带上述四个字段。Node `searchBatch()` 的 Promise 拒绝采用相同字段。

```python
from triviumdb import ResourceBudgetError

try:
    db.set_memory_limit(80)
except ResourceBudgetError as error:
    print("预算拒绝：", error.resource, error.requested, error.used, error.limit)
print(db.instance_stats())
```

配置示例、完整统计口径与维护诊断见[多实例共享资源运行时](resource-runtime.md)。

---

## 内存管理

### set_memory_limit — 内存预算控制

设置本实例内存预算。写入增量、容量预留和已接入的大工作区在执行前检查预算。共享运行时先核对配额调整，成功后同步更新实例配置，失败时保留原配额。Rust `set_memory_limit(bytes)` 返回 `Result<()>`。

**Python：**
```python
db.set_memory_limit(mb=256)  # 设置为 256 MiB
db.set_memory_limit(mb=0)    # 共享运行时内存总额不限时，可取消实例预算
```

### estimated_memory — 查询当前内核内存估算

**Python：**
```python
usage_bytes = db.estimated_memory()
print(f"当前内存占用: {usage_bytes / 1024 / 1024:.1f} MB")
```

---

## 文本索引与稀疏检索

> 以下正文替换、删除清理与延期加载行为从 0.8.11 起提供。

### index_text — 建立全文稀疏索引
对指定节点的长文本内容提取 BM25 特征，用于后续的混合检索召回。需在节点 insert 后调用；节点不存在时返回错误。

重复调用会替换该节点的旧正文与词频，空字符串会清除其 BM25 内容；显式注册的关键词保持不变。多字段或多段正文应拼接后一次登记。修改 Payload 不会自动改写单独登记的正文。中文使用字符二元分词，两字名字可参与检索。

**Python：**
```python
db.index_text(id=42, text="Rust 在嵌入式领域取得突破")
```

### index_keyword — 建立精确关键词索引
建立基于 AC 自动机 (Aho-Corasick) 的精确词汇匹配索引，极速锁定特征锚点。

同一关键词可以关联多个节点。同节点的重复注册按小写规范化后去重，不重复加分；空关键词或不存在的节点会返回错误。删除节点时同步清理其正文、关键词和统计，包括事务删除及 WAL 删除回放。

**Python：**
```python
db.index_keyword(id=42, keyword="Rust")
```

### build_text_index — 预热文本索引
可在批量登记后调用，以提前编译 AC 自动机。正文统计随写入更新，关键词变更后也可由首次查询按需编译，无需每次修改都显式构建。构建失败会向 Rust、Python 和 Node.js 调用方返回错误；此方法不会扫描 Payload 自动生成全文索引。

**Python：**
```python
db.build_text_index()
```

`load_text_index=True` 会在打开时加载持久化的全文索引。默认 `False` 省略加载，后续纯向量写入、`flush()` 和 `close()` 保留已有文本文件；显式文本登记或预热会先加载原索引并合并修改。延期加载与清理受内核内存预算约束，超限会先返回错误。

文本登记通过 `flush()` 或可写句柄的 `close()` 持久化，尚不属于 `Transaction` 的 WAL 操作。应用需保留权威正文与别名来源，以便重新登记。

历史 `.text`（TIDX v2）快照缺少 `.text.meta` 时，完整格式校验通过后可恢复正文与关键词，并在保存时补齐长度和 CRC。已有元数据中的文本长度和 CRC 用于校验内容；主文件长度随存储布局和节点变化更新。加载异常时保留原文本文件，错误消息区分长度、CRC、元数据格式与快照格式。

### rebuild_text_index — 按完整集合重建文本索引

Rust 使用 `rebuild_text_index(&[TextIndexDocument])`，Python 使用 `rebuild_text_index(documents)`，Node.js 使用 `rebuildTextIndex(documents)`。每条记录包含 `id`、`text` 与可选的 `keywords`。

```python
db.rebuild_text_index([
    {"id": 42, "text": "完整正文", "keywords": ["实体别名", "产品名称"]},
    {"id": 43, "text": "另一篇正文", "keywords": []},
])
db.flush()
```

提供的集合构成重建后的完整文本索引，空集合清空全部正文与关键词。内核验证节点、重复记录、关键词和工作区预算，构建成功后整体替换内存索引，随后通过 `flush()` 或可写关闭持久化。宿主可用保存的权威正文和别名恢复文本检索。

检查点将主数据、文本及其元数据、属性索引和图索引纳入发布恢复组。可写打开先恢复中断的文件组，再回放 WAL。Mmap 在索引同步完成后提交 `.flush_ok`；Rom 在完整文件组同步后提交发布日志。

---

## 工具方法

### all_node_ids — 获取全部节点 ID

返回当前数据库中所有活跃节点的 ID 列表（顺序不定）。可用于遍历全库或批量操作。

**Python：**
```python
ids = db.all_node_ids()          # 返回 list[int]
print(f"共 {len(ids)} 个节点")
```

**Rust：**
```rust
let ids = db.all_node_ids()?;    // Vec<NodeId>
```

### QuIVer 自动索引说明

QuIVer 是自研 ANN 图索引，常规查询自动选择与维护索引。从 `0.8.9` 起默认采用 BQ 构图、无训练 SM-L2 导航和原始向量余弦精排，无需调用 `enable_quiver_sm_l2()`：

| 条件 | 检索引擎 | 召回行为 |
|------|----------|----------|
| 低于当前维度的动态阈值 或 QuIVer 未就绪 | **BruteForce** | 100% 精确召回，零误差 |
| 达到动态阈值 + 索引就绪 | **QuIVer (BQ 构图 + SM-L2 导航)** | 原始向量余弦精排；Recall 取决于数据、搜索宽度与预算 |

底层未显式指定 `rerank_limit` 时默认精排预算为 64，实际不超过有效候选宽度，并至少覆盖 `top_k`；上层显式预算继续生效。过滤拒绝和墓碑不占成功精排名额。SM-L2 是导航评分名称，普通查询返回值仍是余弦分数；原生 L2 返回负平方距离，需要 Rust 显式安装，不能混用两类分数。

完整模式、生命周期与度量限制见 [QuIVer 架构说明](quiver.md)。

自动阈值按 `ceil(666,667 / dim)` 计算并限制在 209–834 节点；384/768 维为 834，1024 维为 652，1536 维为 435，3072 维为 218。

QuIVer 支持增量 Insert/Delete/Update，并保护入口导航树，入口升层时连接旧树。旧索引通过重建应用 `0.8.10` 的签名多样性和连通性规则；查询召回率由数据、搜索参数与精排预算共同决定。索引以 `.tdb.quiver` 持久化，并通过 `.tdb.quiver.meta` 的主数据代际、文件尺寸、节点数、维度与 CRC 校验后 mmap 加载。有效图按保存的邻接表加载，默认 SM 状态从原始向量池流式恢复，启动时读取相应冷向量；断连图按缺失索引策略重建或返回错误。显式评分模式由宿主重新配置。TextIndex 对应 `.tdb.text` 和 `.tdb.text.meta`。

批量导入可在构造时设置 `auto_build_quiver=False`，或运行时调用 `set_auto_build_quiver(False)`，避免导入中途因查询触发索引构建；导入完成后重新开启并执行一次查询，或显式调用 Rust `build_quiver_index()`。`flush()` 只负责持久化，不会隐式构建 QuIVer。无状态评测或独立查询之间可调用 `clear_search_state()` 清空疲劳状态。

> 💡 如果你的业务对 100% 召回率有强需求（如金融/医疗），可以通过 `force_brute_force: true` 强制使用 BruteForce。

---

## 维度迁移

> 文件格式兼容边界：当前 `.tdb` 为 v10，可读取 v5–v10；可写句柄在后续持久化时按当前格式发布。Mmap v9 的 raw Payload 位于 generation 绑定的 `.pld`，详见[持久化布局](features.md)。ReadOnly/Immutable 不升级、不写制品。早于 v5 的主文件必须通过旧内核导出迁移；未来版本明确拒绝。sidecar 独立版本化：`.pidx v8`、`.gidx v3`、`.text v2`、`.quiver v1`。

复合索引字段列表按 JSON 数组保存，支持 NUL 和 Unicode 字段名；字段名总长度上限为 1 MiB，字段数量上限为 65,535。旧复合 posting 通过事实扫描保持结果，唯一约束从主文件恢复；普通复合索引可按原字段列表重新创建。唯一约束创建或删除在发布失败后返回原始错误，并将实例标为待恢复，后续读写返回 `TDB_WAL_CLOSED`。释放实例后可写重开，由发布提交点决定恢复旧定义或采用新定义。

Mmap 只读打开在采用 `.gidx` 前校验所有边块、派生目录与主文件事实；语义或结构失配返回 `TDB_CORRUPTED_FILE`，保留原文件组。可写打开从主文件读取业务边，通过 `repair_graph_indexes` 显式发布重建副本。

当需要更换 Embedding 模型（维度发生变化）时，使用 `migrate` 将旧库的结构迁移到新维度。

### migrate — 迁移到新维度

将当前数据库的所有节点 Payload、图谱边复制到一个全新的数据库文件中，向量以零向量占位（因为维度变了，旧向量无法直接复用）。

目标主文件或同名前缀的数据制品已经存在时，迁移在写入前返回 `TDB_IO`，`details.io_kind` 为 `AlreadyExists`。迁移在目标同盘的独占暂存目录完成构建与同步，持有目标写锁发布文件组并交接给返回实例；发布使用原子硬链接创建目录项，要求文件系统支持同盘硬链接。提交前失败只回收本次创建的文件，目标可重试；提交后错误保留完整新库，可按新维度重新打开检查。空闲 `.lock` 文件用于目标协调，支持失败后重试。

**参数：**

| 参数 | 类型 | 说明 |
|------|------|------|
| `new_path` | `str` | 新数据库文件路径 |
| `new_dim` | `int` | 新的向量维度 |

**返回值：** 所有已迁移节点的 ID 列表（`list[int]`）

**Python：**
```python
# 第一步：迁移结构（保留 payload + 边，向量置零）
with triviumdb.TriviumDB("old.tdb", dim=768) as old_db:
    node_ids = old_db.migrate("new.tdb", new_dim=1536)

# 第二步：打开新库，用新模型逐节点更新向量
with triviumdb.TriviumDB("new.tdb", dim=1536) as new_db:
    for nid in node_ids:
        payload = new_db.get(nid).payload
        new_vec = new_model.encode(payload["text"]).tolist()
        new_db.update_vector(new_vec, nid)
```

**Rust：**
```rust
// 迁移结构
let (mut new_db, node_ids) = old_db.migrate_to("new.tdb", 1536)?;

// 更新向量
for &nid in &node_ids {
    let new_vec = new_model.encode(&payload_map[&nid]);
    new_db.update_vector(nid, &new_vec)?;
}
new_db.flush()?;
```

> ⚠️ 迁移不修改原数据库，原库仍可正常使用。新库创建完毕后，需要手动更新所有向量后才能进行有效的向量检索。

> 💡 如果希望同时切换 dtype（例如从 f32 换 f16），需在创建新库时指定 `dtype` 参数：`TriviumDB("new.tdb", dim=1536, dtype="f16")`。

## 事务支持

TriviumDB 提供轻量级事务，采用**验证前置（Dry-Run）架构**：操作先缓冲在内存中，`commit()` 按操作顺序检查节点存在状态、维度、ID 冲突、唯一约束和 JSON 可恢复性，完成容量预留后提交 WAL，再按同一顺序应用数据。

**特性：**
- 预检失败保持原节点、索引、ID 分配状态与 WAL；WAL 写入失败时内存数据保持原样，并向调用方返回原始错误
- 同一事务内，新插入或删除后重建的节点可立即参与 Payload 更新、向量更新、连边和再次删除；当前仍存在的 ID 再次插入时返回 `NodeAlreadyExists`
- 返回的 ID 按插入操作排序。`delete(10) → insert_with_id(10) → delete(10) → insert_with_id(10)` 成功时返回 `[10, 10]`，最终节点内容来自最后一次插入
- `rollback()`（或直接 `drop` 事务对象）将丢弃所有缓冲操作

```rust
let mut tx = db.begin_tx();
tx.insert(&vec1, json!({"type": "event"}));
tx.insert_with_id(9999, &vec2, json!({"type": "person"}));
tx.link(1, 9999, "attended", 1.0);

// 原子提交 → 两阶段: 干跑验证 → 物理写入
let ids = tx.commit()?;

// 或显式回滚（丢弃所有操作）
// tx.rollback();
```

> Python 通过 `db.transaction()` 暴露同一套缓冲提交语义；Node.js 当前未公开事务构建器。

---

## Pythonic 魔术方法

| 语法 | 等价调用 | 说明 |
|------|----------|------|
| `len(db)` | `db.node_count()` | 当前活跃节点数 |
| `42 in db` | `db.contains(42)` | 节点是否存在 |
| `print(db)` | `db.__repr__()` | 输出如 `TriviumDB(dtype=f32, nodes=100, dim=1536)` |
| `with db:` | `__enter__` / `__exit__` | 退出时自动 `flush()` |

---

## 数据类型说明

### NodeView

节点的完整视图，通过 `get()` 或 `filter_where()` 返回。

| 属性 (Python) | 属性 (Rust) | 类型 | 说明 |
|---------------|-------------|------|------|
| `id` | `id` | `u64` | 全局唯一节点 ID |
| `vector` | `vector` | `list[float]` / `Vec<T>` | 节点的特征向量 |
| `payload` | `payload` | `dict` / `serde_json::Value` | JSON 元数据 |
| `edges` | `edges` | `list[Edge]` / `Vec<Edge>` | 详细出边列表（包含 target_id, label, weight） |
| `num_edges` | `edges.len()` | `int` / `usize` | 快速获取出边数量 |

### SearchHit

向量检索命中结果，通过 `search()` 返回。

| 属性 | 类型 | 说明 |
|------|------|------|
| `id` | `u64` | 命中节点 ID |
| `score` | `f32` | 相似度得分 |
| `payload` | `dict` | 节点元数据 |

### QueryRow

Cypher 查询结果行，通过 `query()` 返回。

| 属性 | 类型 | 说明 |
|------|------|------|
| `row` | `dict[str, dict]` | 变量名 → 节点摘要字典 |

### Edge (Rust)

图谱边的内部结构。

| 字段 | 类型 | 说明 |
|------|------|------|
| `target_id` | `NodeId (u64)` | 目标节点 ID |
| `label` | `String` | 关系类型标签 |
| `weight` | `f32` | 权重（支持负值） |

### HookContext

Hook 管线执行上下文，通过 `search_with_context()` 返回。

| 属性 (Python) | 属性 (Node.js) | 属性 (Rust) | 类型 | 说明 |
|---------------|----------------|-------------|------|------|
| `timings` | `timings` | `stage_timings` | `dict` / `Object` / `Vec<(String, Duration)>` | 各管线阶段的耗时（Python/JS 单位毫秒） |
| `custom_data` | `customData` | `custom_data` | `dict` / `Object` / `serde_json::Value` | Hook 注入的自定义数据 |
| `aborted` | `aborted` | `abort` | `bool` | 管线是否被 Hook 提前终止 |


## 写入与数值边界

Payload 与边元数据采用 JSON 解码一致的容器深度边界，最多 127 层。直接写入、事务、Upsert、条件更新及 `$set` 补丁在追加 WAL 前校验最终值；单个 Payload 的编码大小上限为 8 MiB。`get()`、`get_payload()` 的解码异常返回结构化损坏错误。Payload 更新、补丁和事务的预算准入计入编码缓冲、原始字节与解析值的增量暂存，失败时保留节点、索引与 WAL。

`$inc` 对整数执行精确的有符号/无符号加法，结果覆盖 `i64::MIN..=u64::MAX`；负结果使用 `i64`，非负整数结果使用 `u64`。缺失字段从零开始递增。小数参与运算时检查整数到浮点的精确转换、有限结果与有效增量。溢出、类型错误及精度损失返回参数错误，整份补丁保持原子性。

Python 的直接写入、批量与事务按 dtype 提取向量，`u64` 分量保留 `0..=2**64-1` 的整数精度。Payload 支持 `None`、布尔、`i64/u64` 整数、有限浮点数、字符串、列表、元组和字符串键字典；元组保存为 JSON 数组。超范围整数、非有限数、非字符串键、循环引用和其他对象在写入前返回异常。读取 JSON 的无符号整数时返回精确 Python `int`。

Rust 和 Python 的节点 ID 写入范围为 `1..=u64::MAX-1`；`0` 与 `u64::MAX` 为保留边界，自动分配耗尽时在写 WAL 前返回参数错误。JSON Payload 的整数保留完整 `i64/u64` 范围。

Node 的数值节点 ID 输入与输出使用 `0..=Number.MAX_SAFE_INTEGER` 内的整数；插入 ID 遵守内核的非零约束。搜索、批量搜索、节点、图遍历与索引查询遇到超范围结果时，整个调用返回 `TDB_INVALID_INPUT`。TQL 查询使用十进制字符串返回完整 `u64` ID。自动插入、批量、事务与 TQL CREATE 在写 WAL 前检查待分配 ID；迁移在创建目标文件前检查返回 ID。

Node 数量参数在转换为整数前检查原始 Number 的有限性、整数性与安全范围。`topK` 与维度要求正整数；其他数量参数保留各自的零值语义。`u64` 向量分量要求有限、非负、整数且小于 `2**64`，直接操作、批量、事务与各检索入口采用一致校验；JavaScript `number` 按 binary64 表示分量。

WAL 追加、flush 或同步失败时返回原始错误并隔离该 writer。后续写入与检查点返回 `WalClosed`；调用方释放数据库句柄并重新打开后继续使用。恢复保留此前已确认数据，失败操作的结果按实际持久化的完整日志确定。业务方可使用稳定 ID 核对该操作后再重试。

事务 WAL 完整提交后，若内存应用阶段发生错误或异常展开，实例及其共享搜索句柄进入恢复等待状态，数据操作返回 `WalClosed`。释放全部相关句柄并重新打开，恢复端应用完整提交事务。事务编号使用进程内单调计数，达到编号上限时返回参数错误。


## 持久化类型、解析与打开准入

- 文件头保存 dtype、元素宽度与 CRC32。ReadWrite/ReadOnly 使用其他 dtype 打开返回 `TDB_DTYPE_MISMATCH`，详情包含 `stored` 和 `requested`，取值为 `f32`、`f16`、`u64`。Rom 与 Mmap 共用此契约。
- v5–v9 非空检查点通过完整向量块几何验证类型；无类型基础检查点的历史 WAL，以及空的无类型检查点附带非空 WAL，使用原版本、原 dtype 恢复并完成检查点或导出后再升级。v10 检查点需要支持 v10 的内核；回退使用升级前备份。
- 发生向量文件替换或重映射失败时，原调用保留 I/O 错误。映射已经失效的实例及其共享句柄随后返回 `TDB_WAL_CLOSED`；释放这些实例后，重新以可写模式打开，恢复发布基础并回放 WAL。
- TQL 字符串入口完整消费输入，语句尾部非法内容在生成写操作前返回 `TDB_QUERY_PARSE`。输入上限为 4 MiB、262,144 个 token、128 层括号嵌套和 128 个表达式运算符；已有递归预算共同约束表达式与谓词。向量及 JSON 字面量中的负号按字面量处理。
- `memory_limit` 覆盖打开时的节点与图目录、Rom 向量堆副本、Payload 解码、派生索引恢复和 WAL 暂存。各阶段在分配前进行保守峰值准入，打开完成前核对总估算占用；预算不足返回 `TDB_CAPACITY_RESERVATION_REJECTED`，本次共享资源预留释放。原始 mmap 向量和 Payload 映射按文件映射管理，堆目录及解码工作区参与准入。
- 打开阶段暂缓填充 Payload 解析缓存；恢复成功后，缓存上限取配置值与剩余实例堆预算中的较小值。索引反序列化的保守峰值可高于稳态估算；可提高打开预算或延期加载文本索引。WAL 截断、历史 WAL 迁移和空检查点升级在容量准入完成后执行。
