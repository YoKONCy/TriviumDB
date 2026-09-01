# TriviumDB API 完整参考

> **版本**: v0.8.4
> **语言**: Rust 核心 + Python 绑定 (PyO3) + Node.js 绑定 (napi-rs)  
> **许可**: Apache-2.0

---

## 目录

- [数据库生命周期](#数据库生命周期)
- [节点 CRUD](#节点-crud)
- [图谱操作](#图谱操作)
- [向量检索](#向量检索)
- [Hook 扩展系统](#hook-扩展系统)
- [元数据过滤](#元数据过滤)
- [TQL、Prepared 与一等值](#tql-prepared-与一等值)
- [持久化索引管理与观测](#持久化索引管理与观测)
- [存储格式观测与迁移错误](#存储格式观测与迁移错误)
- [持久化与压缩](#持久化与压缩)
- [内存管理](#内存管理)
- [工具方法](#工具方法)
- [维度迁移](#维度迁移)
- [事务支持](#事务支持-rust-only)
- [Pythonic 魔术方法](#pythonic-魔术方法)
- [数据类型说明](#数据类型说明)

---

## 数据库生命周期

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
    expected_nodes=3_600_000, # 预计总节点数，仅本次进程预留，不是硬上限
    memory_limit_mb=28_672,   # 内核内存预算，0 表示不限制
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
| `expected_nodes` | `int \| None` | `None` | 预计总节点数；仅预留核心容器、不持久化、不是硬上限 |
| `memory_limit_mb` | `int` | `0` | TriviumDB 内核内存预算（MiB）；0 表示不限制 |
| `access_mode` | `str` | `"read_write"` | `read_write` 使用排他锁；`read_only` 使用共享锁；`immutable` 验证 manifest 后无锁打开 |

`search_with_context` / `searchWithContext` 返回的上下文除阶段耗时和候选数外，还包含 `observations`：`estimated_heap_bytes`、`mmap_vector_bytes`、`node_count`、查询路由与 QuIVer `ef_search`；Linux 额外提供进程 RSS、累计主缺页和次缺页。缺页计数是进程累计值，调用方应计算相邻采样差值。

> ⚠️ **强烈建议 `dim <= 3072`。** 数据库存储和精确 BruteForce 检索支持 3073–65536 维，但 QuIVer 仅支持 1–3072 维。高于 3072 维时，自动 QuIVer 构建会被安全跳过，搜索回退到 BruteForce；显式构建 QuIVer 会返回错误，不会 panic。若数据规模可能达到 1 万节点以上并依赖 ANN 性能，请在建库前选择不超过 3072 维的 embedding 模型或先做降维。

### 只读访问模式

只读模式面向已完成 `flush()` 的不可变数据库代际。多个进程可以同时以共享锁打开同一路径，但同路径 Reader 与 Writer 互斥。

TriviumDB 采用单写多读模型。同一语言绑定实例可被多个线程调用且不会再抛出底层借用冲突，但写 API 仍应由调用方通过单一写队列、`Lock` / `Mutex` 或 actor 串行化；这不是并行多 Writer 提交能力。高并发读取应使用 ReadOnly/Immutable Reader。

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
- 不删除损坏或错配的 TextIndex/QuIVer sidecar；
- `.tdb/.vec/.flush_ok` 代际不完整时拒绝打开，不执行 metadata-only 降级；
- 禁止 CRUD、事务提交、TQL mutation、索引修改、QuIVer 构建、flush 和 compact；
- `close()` 和 Drop 不执行持久化；
- QuIVer 不可用时保持内存只读并走安全检索路径，不写回磁盘。

发布只读代际前必须由 Writer 完成 `flush()` 或 `close()`，并保留相邻 `.lock` 文件供 Reader 获取共享锁。不要在仍有进程持锁时删除 `.lock` 文件。

### 不可变 Generation

Writer 可在完成持久化后原子生成 generation manifest：

```rust
db.publish_generation_manifest("generation-42")?;
```

Python 使用 `db.publish_generation_manifest("generation-42")`，Node.js 使用 `db.publishGenerationManifest("generation-42")`。该操作会先执行安全 `flush()`，随后为当前 `.tdb`、`.vec`、`.flush_ok` 及已存在的 TextIndex/QuIVer sidecar 记录文件大小和 CRC32，并最后原子发布 `<path>.manifest.json`。

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
db.set_sync_mode(SyncMode::Off);
```

`close()` 会先进入 `Closing`，拒绝新操作并等待已经进入的操作结束，再执行最终 flush。只有 flush 成功后才释放文件锁并进入 `Closed`；flush 失败会恢复为 `Open` 且继续持有文件锁。关闭后的旧对象不能再次用于查询、写入或 flush。

**泛型类型参数 `T`：**

| 类型 | 说明 | 适用场景 |
|------|------|----------|
| `f32` | 32 位浮点 | 标准 embedding（OpenAI、BGE 等） |
| `half::f16` | 16 位半精度浮点 | 大规模数据集省内存 |
| `u64` | 64 位无符号整数 | SimHash / 二值化向量 |

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
if let Some(view) = db.get(42) {
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

在两个节点之间建立一条有向带权边。边以 `(src, dst, label)` 三元组唯一；重复调用会更新 weight，不会增加重复边。两个端点必须已存在，weight 必须是有限 `f32`，否则在写 WAL 前返回错误。

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

内置认知管线的全功能入口。通过 `SearchConfig` 参数化控制 FISTA 残差寻隐、SA-PPR 有限深度扩散、DPP 多样性采样等高级特性。SA-PPR 是带个性化重启的有限深度 Spreading Activation，不是迭代至收敛的标准 PageRank。

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

> 💡 所有参数均内置安全钳位：`teleport_alpha` 被约束在 [0, 1]，`fista_lambda` 在 [1e-5, 100]，`dpp_quality_weight` 在 [0, 10]。传入越界值不会崩溃，而是被静默钳平。

> 💡 当 `enable_advanced_pipeline = false` 时，`search_advanced` 的行为与 `search` 完全一致。

---

## 🔌 Hook 扩展系统

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
db.set_hook(ffi_hook);
```

### clear_hook — 清除已注册 Hook

清除当前的 Hook，恢复为默认的零开销 `NoopHook`。

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
db.set_hook(MyHook);
```

> 💡 **零开销设计**：未注册 Hook 时，默认的 `NoopHook` 的所有方法均为空实现，编译器会将它们完全内联消除，对无 Hook 的普通检索完全零开销。

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

TQL 公共 API 暴露的是一套 **自由 DIY 混合查询能力**，而不是单一的混合搜索函数。调用者可以用一条查询按需组合向量源、属性索引、图扩展、图算法、路径、集合代数、迭代、聚合与重排；Prepared TQL 则让同一管线安全地重复绑定业务参数。

### tql / tql_values

支持 MATCH、OPTIONAL MATCH、FIND、SEARCH、WITH 管线、聚合、图算法、路径与集合代数。Rust `tql()` 只适合纯节点绑定；标量、聚合、Path/List 必须使用 `tql_values()`。Python/Node 的 `tql` 已自动承载一等值。

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
        println!("{}: {:?}", node.id, node.payload);
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

### 一等值映射

| TQL 值 | Python | Node |
|---|---|---|
| Node | dict | object，NodeId 为字符串 |
| Int/Float/String/Bool | 原生标量 | 原生标量 |
| Path | `list[int]` | `string[]`，避免 u64 精度损失 |
| List | list | array |
| Null | None | null |

### tql_mut — 执行 TQL 写操作

支持 CREATE / SET / DELETE / DETACH DELETE 语法，返回受影响行数和新创建的节点 ID。

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

删除指定字段的索引。查询仍然可用，只是退化为 O(N) 全表扫描。

**Python：**
```python
db.drop_index("name")
```

**Node.js：**
```js
db.dropIndex('name')
```


## 存储格式观测与迁移错误

`storage_info/storageInfo` 是只读诊断入口，返回产品版本、`.tdb` 当前/最低版本、WAL/Property/Graph/QuIVer/Text/Manifest 格式、dim、node count、访问模式、估算内存和 sidecar 存在状态。它不会创建、修复或重写文件。

```python
info = db.storage_info()
print(info["database_format_current"])  # 7
print(info["property_index_format"])    # 4
print(info["sidecars"])
```

历史 API 不再静默翻译：`tql_mut` 读查询、patch 普通对象简写、Node 数字构造参数分别返回迁移错误；Rust `open_with_sync` 已删除。历史无头 WAL 返回 `UnsupportedWalVersion`；带记录的旧版本 WAL 需用旧内核恢复并 flush。恰好只有版本头、没有记录的旧 WAL 可由 ReadWrite 打开流程原子升级，ReadOnly/Immutable 保持零写并要求先由 Writer 完成升级。

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

启动后台守护线程，定时在后台串行化执行数据压缩与全量落盘（包含 `flush` + WAL 截断清理）。

**Python：**
```python
db.enable_auto_compaction(interval_secs=30)  # 间隔必须大于 0
# db.enable_auto_compaction(interval_secs=0)  # ValueError：拒绝忙循环
db.disable_auto_compaction()                 # 停止后台压缩线程
```

**Rust：**
```rust
db.enable_auto_compaction(Duration::from_secs(30));
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

## 内存管理

### set_memory_limit — 内存预算控制

设置 MemTable 内存使用上限。当估算内存超过限额时，写操作完成后自动触发 flush。

**Python：**
```python
db.set_memory_limit(mb=256)  # 限制为 256 MB
db.set_memory_limit(mb=0)    # 取消限制（默认）
```

### estimated_memory — 查询当前内存占用

**Python：**
```python
usage_bytes = db.estimated_memory()
print(f"当前内存占用: {usage_bytes / 1024 / 1024:.1f} MB")
```

---

## 文本索引与稀疏检索

### index_text — 建立全文稀疏索引
对指定节点的长文本内容提取 BM25 特征，用于后续的混合检索召回。需在节点 insert 后调用。

**Python：**
```python
db.index_text(id=42, text="Rust 在嵌入式领域取得突破")
```

### index_keyword — 建立精确关键词索引
建立基于 AC 自动机 (Aho-Corasick) 的精确词汇匹配索引，极速锁定特征锚点。

**Python：**
```python
db.index_keyword(id=42, keyword="Rust")
```

### build_text_index — 编译倒排字典树
在数据初始化批量调用完毕后，**必须调用此方法**完成底层 AC 自动机的编译与全局文本 IDF 频率汇算。之后方可进行 `search_hybrid` 混合检索。

**Python：**
```python
db.build_text_index()
```

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
let ids = db.all_node_ids();     // Vec<NodeId>
```

### QuIVer 自动索引说明

TriviumDB v0.7.0 起采用自研的 **QuIVer** SOTA 级 ANN 图索引，全自动双引擎向量索引路由，无需手动 `rebuild_index()` 接口：

| 条件 | 检索引擎 | 召回行为 |
|------|----------|----------|
| < 1 万节点 或 QuIVer 未就绪 | **BruteForce** | 100% 精确召回，零误差 |
| ≥ 1 万节点 + 索引就绪 | **QuIVer (BQ + Vamana)** | BQ 签名 + 图导航 + f32 精排，Recall@10 > 97% |

QuIVer 索引支持增量 Insert/Delete/Update，无需全量重建。索引以 `.tdb.quiver` 持久化，并通过 `.tdb.quiver.meta` 的主数据代际、文件尺寸、节点数、维度与 CRC 校验后 mmap 加载。TextIndex 对应 `.tdb.text` 和 `.tdb.text.meta`。

批量导入可在构造时设置 `auto_build_quiver=False`，或运行时调用 `set_auto_build_quiver(False)`，避免导入中途因查询触发索引构建；导入完成后重新开启并执行一次查询，或显式调用 Rust `build_quiver_index()`。`flush()` 只负责持久化，不会隐式构建 QuIVer。无状态评测或独立查询之间可调用 `clear_search_state()` 清空疲劳状态。

> 💡 如果你的业务对 100% 召回率有强需求（如金融/医疗），可以通过 `force_brute_force: true` 强制使用 BruteForce。

---

## 维度迁移

> 文件格式兼容边界：当前 `.tdb` 为 v7，可读取 v5–v7；可写句柄在下次 flush/close 原子升级到当前格式。早于 v5 的主文件必须通过旧内核导出迁移；未来版本明确拒绝。sidecar 独立版本化：`.pidx v4`、`.gidx v2`、`.text v2`、`.quiver v1`。

当需要更换 Embedding 模型（维度发生变化）时，使用 `migrate` 将旧库的结构迁移到新维度。

### migrate — 迁移到新维度

将当前数据库的所有节点 Payload、图谱边复制到一个全新的数据库文件中，向量以零向量占位（因为维度变了，旧向量无法直接复用）。

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

## 事务支持（Rust / Python）

TriviumDB 提供轻量级事务，采用**验证前置（Dry-Run）架构**：所有操作先缓冲在内存中，`commit()` 分两阶段执行——首先在纯内存验证全部约束（维度、节点存在性、ID 冲突），全部通过后才一次性写入。

**特性：**
- `commit()` 返回 `Err` 时，**底层数据没有被修改一个字节**，可加入日志后安全重试
- 在同一事务内，`insert_with_id(999)` 后立即 `link(..., 999)` 是完全合法的（虚拟状态叠加给 999 号打过标记）
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
