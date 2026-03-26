# TriviumDB API 完整参考

> **版本**: v0.3  
> **语言**: Rust 核心 + Python 绑定 (PyO3)  
> **许可**: Apache-2.0

---

## 目录

- [数据库生命周期](#数据库生命周期)
- [节点 CRUD](#节点-crud)
- [图谱操作](#图谱操作)
- [向量检索](#向量检索)
- [元数据过滤](#元数据过滤)
- [Cypher 图谱查询](#cypher-图谱查询)
- [持久化与压缩](#持久化与压缩)
- [内存管理](#内存管理)
- [索引维护](#索引维护)
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
    dtype="f32",           # 向量类型："f32" | "f16" | "u64"
    sync_mode="normal"     # WAL 同步模式："full" | "normal" | "off"
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
| `dim` | `int` | `1536` | 向量维度，必须与后续插入的向量长度一致 |
| `dtype` | `str` | `"f32"` | 向量存储精度：`f32`（标准）、`f16`（省内存）、`u64`（SimHash） |
| `sync_mode` | `str` | `"normal"` | WAL 写入安全级别，详见[持久化与压缩](#持久化与压缩) |

### Rust

```rust
use triviumdb::Database;
use triviumdb::storage::wal::SyncMode;

// 基础打开
let mut db = Database::<f32>::open("my_data.tdb", 1536)?;

// 指定同步模式
let mut db = Database::<f32>::open_with_sync("my_data.tdb", 1536, SyncMode::Full)?;

// 运行时切换同步模式
db.set_sync_mode(SyncMode::Off);
```

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

### batch_insert — 批量插入

一次性插入多个节点，返回所有新 ID 的列表。

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

---

## 图谱操作

### link — 建立有向边

在两个节点之间建立一条有向带权边。两个端点必须已存在，否则返回错误。

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

### unlink — 断开边

移除从 `src` 到 `dst` 的**所有**边（无论 label 是什么）。

**Python：**
```python
db.unlink(src=1, dst=2)
```

**Rust：**
```rust
db.unlink(1, 2)?;
```

### neighbors — N 跳邻居

从指定节点出发，沿有向边进行广度优先遍历（BFS），返回 N 跳以内所有可达节点的 ID。

**Python：**
```python
neighbor_ids = db.neighbors(id=1, depth=2)  # 2 跳以内的所有邻居
```

**Rust：**
```rust
let ids = db.neighbors(1, 2);
```

---

## 向量检索

### search — 混合检索

TriviumDB 的核心能力：**先用向量相似度找到锚点，再沿图谱关系向外扩散**。

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
| `$gt` | 大于 | 数字 | `{"age": {"$gt": 18}}` |
| `$gte` | 大于等于 | 数字 | `{"score": {"$gte": 0.8}}` |
| `$lt` | 小于 | 数字 | `{"age": {"$lt": 30}}` |
| `$lte` | 小于等于 | 数字 | `{"price": {"$lte": 99.9}}` |
| `$in` | 包含于列表 | 数组 | `{"role": {"$in": ["admin", "mod"]}}` |
| `$and` | 逻辑与 | 条件数组 | `{"$and": [{...}, {...}]}` |
| `$or` | 逻辑或 | 条件数组 | `{"$or": [{...}, {...}]}` |

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

## Cypher 图谱查询

### query — 类 Cypher 语法查询

使用类似 Neo4j Cypher 的语法，沿图谱路径模式进行匹配查询。

**Python：**
```python
# 沿 knows 边查找
rows = db.query("MATCH (a)-[:knows]->(b) RETURN b")
for row in rows:
    node = row.row["b"]    # {"id": ..., "payload": {...}, "num_edges": ...}
    print(node["payload"])

# 带内联属性过滤
rows = db.query("MATCH (a {id: 1})-[]->(b) RETURN a, b")

# 带 WHERE 条件
rows = db.query('MATCH (a)-[:knows]->(b) WHERE b.age > 18 RETURN b')

# AND / OR 复合条件
rows = db.query('MATCH (a)-[]->(b) WHERE b.age > 18 AND b.role == "admin" RETURN b')
```

**Rust：**
```rust
let rows = db.query("MATCH (a)-[:knows]->(b) WHERE b.age > 20 RETURN b")?;
for row in &rows {
    if let Some(node) = row.get("b") {
        println!("{}: {:?}", node.id, node.payload);
    }
}
```

**语法规范：**

```
Query     := MATCH Pattern (WHERE Condition)? RETURN ReturnList
Pattern   := NodePat (EdgePat NodePat)*
NodePat   := '(' Ident? ('{' PropList '}')? ')'
EdgePat   := '-[' (':' Ident)? ']->'
Condition := CompareExpr ((AND | OR) CompareExpr)*
ReturnList:= Ident (',' Ident)*
```

**返回值 `QueryRow`：**

| 属性 | 类型 | 说明 |
|------|------|------|
| `row` | `dict[str, dict]` | 变量名 → `{"id": int, "payload": dict, "num_edges": int}` |

> 💡 当前仅支持**有向**边模式 `-[]->`，不支持无向匹配或反向匹配。

---

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

启动后台守护线程，定时执行 flush + WAL 清理。

**Python：**
```python
db.enable_auto_compaction(interval_secs=30)  # 每 30 秒自动落盘
db.disable_auto_compaction()                  # 停止
```

**Rust：**
```rust
db.enable_auto_compaction(Duration::from_secs(30));
db.disable_auto_compaction();
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

## 索引维护

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

### rebuild_index — 重建 HNSW 向量索引

将当前 MemTable 中所有活跃节点的向量重新构建为 HNSW 图索引，以获得 O(log N) 的近似搜索性能。

- **BruteForce 模式**（默认）：调用此方法为空操作（no-op），无副作用
- **HNSW 模式**（`--features hnsw`）：从向量池重建完整 HNSW 图，完成后立即生效

**典型使用场景**：批量导入大量数据后调用一次，避免每次插入都触发索引更新。

**Python：**
```python
# 批量导入（临时关闭 WAL 同步提速）
with triviumdb.TriviumDB("data.tdb", dim=768, sync_mode="off") as db:
    ids = db.batch_insert(vectors, payloads)
    db.rebuild_index()   # 导入完毕后一次性重建索引
```

**Rust：**
```rust
db.rebuild_index();  // 同步调用，完成前阻塞
```

---

## 维度迁移

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

## 事务支持 (Rust Only)

TriviumDB 提供轻量级事务：所有操作先缓冲在内存中，`commit()` 后一次性原子写入。

```rust
let mut tx = db.begin_tx();
tx.insert(&vec1, json!({"type": "event"}));
tx.insert(&vec2, json!({"type": "person"}));
tx.link(1, 2, "attended", 1.0);

// 原子提交 → 一次性持锁写入 memtable + WAL
let ids = tx.commit()?;

// 或显式回滚（丢弃所有操作）
// tx.rollback();
```

> ⚠️ 事务目前仅在 Rust API 中可用，Python 侧暂未暴露。

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
| `num_edges` | `edges.len()` | `int` / `usize` | 出边数量 |

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
