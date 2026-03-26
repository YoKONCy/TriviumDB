<div align="center">

# TriviumDB

**向量 × 图谱 × 关系型 —— 三位一体的 AI 原生嵌入式数据库**

> *Trivium*：拉丁语，意为"三条道路的交汇"。

[![Rust](https://img.shields.io/badge/Rust-stable-orange?logo=rust)](https://www.rust-lang.org/)
[![Python](https://img.shields.io/badge/Python-3.9+-blue?logo=python)](https://pypi.org/)
[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)

</div>

---

## 一句话介绍

TriviumDB 是一个用纯 Rust 编写的**嵌入式单文件数据库引擎**，将**向量检索（Vector）**、**属性图谱（Graph）**和**关系型元数据（Relational）**原生融合在同一个存储内核中。

它的目标是成为 **AI 应用领域的 SQLite**：
- 🗃️ **单文件带走** —— 所有数据打包进一个 `.tdb` 文件，复制即迁移
- 🔗 **节点即一切** —— 每个节点天然同时拥有向量、元数据和图关系，ID 全局唯一，绝不错位
- 🧠 **为 AI 而生** —— 支持"先向量锚定、再沿图谱扩散"的混合检索范式
- 🐍 **Python 原生** —— `pip install` 后直接 `import triviumdb`，类 MongoDB 查询语法
- ⚡ **多核并行** —— rayon 并行向量扫描 + mmap 零拷贝加载 + 可选 HNSW 索引
- 💾 **SSD 友好** —— Append-Only WAL + 后台 Compaction 线程，杜绝随机写入磨损

---

## 为什么需要 TriviumDB？

### 当前 AI 应用的「三库割裂」困境

几乎所有的 AI 应用（Agent / RAG / 推荐系统）都同时需要三种数据能力，但市面上没有一个引擎能同时原生支持它们：

```
┌──────────────────── 现状：三套系统缝合 ────────────────────┐
│                                                            │
│   PostgreSQL / SQLite      ← 存文本、属性、时间戳          │
│   Qdrant / Milvus / Pinecone  ← 存向量，独立进程/服务      │
│   Neo4j / NetworkX         ← 存图谱关系，又一套运行时       │
│                                                            │
│   痛点：                                                   │
│   ① 三套 ID 空间，需手写胶水代码保持同步                    │
│   ② 删一条记录 → 要操作三个地方，任何一步失败就不一致       │
│   ③ 「先向量检索，再沿关系扩散」需要跨库 JOIN，延迟爆炸     │
│   ④ 部署一个 Agent 却要装三套数据库运行时                   │
│   ⑤ 想把数据发给别人 → 要导出三份文件再合并                 │
│                                                            │
├──────────────────── TriviumDB：一库统一 ──────────────────┤
│                                                            │
│   单一引擎 · 单一文件 · 单一 ID 空间                       │
│                                                            │
│   insert() → 向量 + 元数据 + 图谱就绪（原子写入）          │
│   search() → 向量锚定 + 图谱扩散    （一次调用）           │
│   delete() → 三层联删               （绝不残留）           │
│   flush()  → 一个 .tdb 文件         （复制即迁移）          │
│                                                            │
└────────────────────────────────────────────────────────────┘
```

### 一个具体的例子

假设你在做一个 **AI 对话记忆系统**，用户说了一句「我昨天和小红去了咖啡馆」：

| 步骤 | 传统三库方案 | TriviumDB |
|------|-------------|------------|
| ① 存语义向量 | 调 Qdrant API 写入 embedding | `db.insert(vec, payload)` 一步完成 |
| ② 存元数据 | 调 SQLite 写入时间、场景 | ↑ 同一步，payload 里就是 JSON |
| ③ 存关系 | 调 Neo4j: 用户→地点→人物 | `db.link(user, cafe, "went_to")` |
| ④ 后续召回 | 3 次跨库查询 + 手写合并 | `db.search(vec, expand_depth=2)` |
| ⑤ 迁移数据 | 导出 3 份 + 写转换脚本 | 复制 `memory.tdb` 一个文件 |

### 适用场景

| 场景 | 怎么用 TriviumDB |
|------|------------------|
| 🤖 **AI Agent 长期记忆** | 每条对话存为节点（embedding + 原文 + 时间戳），人物/地点/事件之间建边，召回时先向量匹配再沿关系链扩散 |
| 🎮 **游戏 NPC 认知引擎** | NPC 观察到的事件存为带向量的节点，NPC 之间的关系用图谱表达，对话时检索相关记忆自动生成回应 |
| 📚 **个人知识库** | Markdown 笔记切片后存入，概念之间手动或自动连边，语义搜索 + 知识图谱导航双模式浏览 |
| 🔬 **小型推荐系统** | 用户和物品各为节点，交互行为存为带权边，混合检索实现「相似用户喜欢的 + 你的社交圈在看的」|
| 🧬 **生物信息学** | 基因/蛋白质序列的 embedding + 互作关系网络，一库搜到相似序列并自动追溯代谢通路 |

---

## 快速上手

### 安装

```bash
# Python（需要 Rust 工具链）
pip install maturin
cd TriviumDB && maturin develop --features python

# Rust
cargo add triviumdb
```

### Python 30 秒入门

```python
import triviumdb

# 打开数据库（with 语句退出时自动 flush 持久化）
with triviumdb.TriviumDB("memory.tdb", dim=1536) as db:

    # 插入节点（向量 + 元数据一步到位）
    id1 = db.insert([0.12, -0.45, 0.78, ...], {"text": "小明喜欢吃苹果", "ts": 1711440000})
    id2 = db.insert([0.08, -0.52, 0.81, ...], {"text": "小红送了小明一箱苹果", "ts": 1711450000})

    # 建立图谱关系
    db.link(id1, id2, label="caused_by", weight=0.95)

    # 混合检索：向量锚定 + 图谱扩散
    results = db.search([0.10, -0.48, 0.80, ...], top_k=5, expand_depth=2, min_score=0.6)
    for hit in results:
        print(f"[{hit.id}] score={hit.score:.3f} | {hit.payload}")

    # 类 MongoDB 高级过滤
    young_admins = db.filter_where({
        "$and": [
            {"age": {"$lt": 30}},
            {"role": {"$in": ["admin", "mod"]}}
        ]
    })

    # 批量插入
    ids = db.batch_insert(
        vectors=[[0.1, 0.2, ...], [0.3, 0.4, ...]],
        payloads=[{"name": "A"}, {"name": "B"}]
    )
```

### Rust 示例

```rust
use triviumdb::Database;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Database::open("memory.tdb", 1536)?;

    let id1 = db.insert(&[0.12, -0.45, 0.78], json!({"text": "Hello"}))?;
    let id2 = db.insert(&[0.08, -0.52, 0.81], json!({"text": "World"}))?;
    db.link(id1, id2, "related", 0.95)?;

    let results = db.search(&[0.10, -0.48, 0.80], 5, 2, 0.6)?;
    for hit in &results {
        println!("[{}] score={:.3} {:?}", hit.id, hit.score, hit.payload);
    }

    db.flush()?;
    Ok(())
}
```

---

## 核心特性

### 🔍 混合检索引擎

```
查询向量 ──→ [向量索引层] ──→ Top-K 锚点
                                  │
                                  ▼
              [图谱扩散层] ──→ N 跳邻居节点（Spreading Activation）
                                  │
                                  ▼
                           最终排序结果
```

### 📦 统一数据模型

每个节点同时持有三种数据，共享全局 `u64` 主键：

| 层 | 存储 | 说明 |
|----|------|------|
| 📊 Payload | JSON | 任意 Key-Value 元数据 |
| 📐 Vector | f32 × dim | 用户传入的外部 Embedding |
| 🔗 Edges | 邻接表 | `(target_id, label, weight)` |

### ⚡ 性能优化

| 优化 | 技术 | 效果 |
|------|------|------|
| 并行向量扫描 | rayon `par_chunks` | 多核线性加速 |
| 零拷贝加载 | memmap2 mmap | 微秒级冷启动 |
| HNSW 索引 | instant-distance (可选) | O(log N) 近似检索 |
| WAL 顺序写入 | Append-Only + fsync | SSD 零磨损 |
| 后台 Compaction | 守护线程定时合并 | 自动垃圾回收 |

---

## Python API 完整参考

```python
db = triviumdb.TriviumDB(path, dim=1536)    # 打开/创建

# ── CRUD ──
id = db.insert(vector, payload_dict)         # 插入
ids = db.batch_insert(vectors, payloads)     # 批量插入
node = db.get(id)                            # 获取 → NodeView(id, vector, payload, num_edges)
db.update_payload(id, new_dict)              # 更新元数据
db.update_vector(id, new_vec)                # 更新向量
db.delete(id)                                # 三层原子联删

# ── 图谱 ──
db.link(src, dst, label="rel", weight=1.0)   # 连边
db.unlink(src, dst)                          # 断边
db.neighbors(id, depth=2)                    # N 跳邻居

# ── 检索 ──
db.search(query_vec, top_k=5, expand_depth=2, min_score=0.5)

# ── 高级过滤（类 MongoDB 语法）──
db.filter_where({"age": {"$gt": 20}})
db.filter_where({"role": {"$in": ["admin", "mod"]}})
db.filter_where({"$or": [{"age": {"$lt": 18}}, {"role": "admin"}]})
# 支持: $eq, $ne, $gt, $gte, $lt, $lte, $in, $and, $or

# ── Pythonic 魔术方法 ──
len(db)                                      # 节点数
id in db                                     # 是否存在
print(db)                                    # TriviumDB(nodes=100, dim=1536)
with triviumdb.TriviumDB("x.tdb") as db:     # 上下文管理器（退出自动 flush）
    ...

# ── 持久化 ──
db.flush()                                   # 手动落盘
db.enable_auto_compaction(interval_secs=30)   # 后台自动落盘
db.disable_auto_compaction()
```

---

## 向量索引策略

通过 Cargo Features 在编译期选择索引后端：

| 后端 | 特点 | 适用规模 | Feature Flag |
|------|------|----------|-------------|
| **BruteForce** | 100% 精确召回，rayon 多核并行 | < 10 万节点 | 默认启用 |
| **HNSW** | 亚毫秒级近似搜索，O(log N) 复杂度 | 10 万 ~ 千万节点 | `hnsw` |

```toml
# 启用 HNSW
cargo build --features hnsw

# 启用 Python 绑定
maturin develop --features python
```

---

## 项目结构

```
TriviumDB/
├── src/
│   ├── lib.rs              # 库入口 + 公开 API
│   ├── database.rs         # Database 核心（Arc<Mutex> 并发安全）
│   ├── node.rs             # Node / Edge / SearchHit 数据结构
│   ├── filter.rs           # 高级过滤引擎 ($gt/$lt/$in/$and/$or)
│   ├── error.rs            # 统一错误类型
│   ├── storage/
│   │   ├── memtable.rs     # 内存工作区 (SoA 向量池 + HashMap)
│   │   ├── wal.rs          # Write-Ahead Log（崩溃恢复）
│   │   ├── file_format.rs  # .tdb 单文件读写（mmap 零拷贝）
│   │   └── compaction.rs   # 后台 Compaction 守护线程
│   ├── index/
│   │   ├── brute_force.rs  # rayon 并行暴力精确搜索
│   │   └── hnsw.rs         # HNSW 近似搜索 (feature-gated)
│   ├── graph/
│   │   └── traversal.rs    # Spreading Activation 图扩散
│   └── python.rs           # PyO3 绑定（完整 Pythonic API）
├── Cargo.toml
├── pyproject.toml          # Maturin 构建配置
└── README.md
```

---

## 路线图

### v0.1 — MVP ✅
- [x] Node / Edge 核心数据结构
- [x] 内存 MemTable（SoA 向量池 + HashMap + 邻接表）
- [x] BruteForce 向量检索
- [x] `insert` / `link` / `search` / `delete` 基础 API
- [x] 单文件 `.tdb` 序列化/反序列化

### v0.2 — 工业可用 ✅
- [x] WAL 日志 + 崩溃恢复
- [x] 后台 Compaction 线程
- [x] HNSW 索引集成 (`instant-distance`, feature-gated)
- [x] 高级 Payload 过滤 ($eq/$ne/$gt/$gte/$lt/$lte/$in/$and/$or)
- [x] PyO3 Python 绑定 + Maturin 打包
- [x] rayon 并行向量扫描
- [x] mmap 零拷贝文件加载

### v0.3 — 生态拓展（进行中）
- [ ] 子图导出 / 批量导入
- [ ] CLI 工具 (`triviumdb-cli`)
- [ ] 性能基准测试套件 (benchmark)
- [ ] npm / WASM 绑定（可选）
- [ ] 分布式分片存储

---

## 与现有方案深度对比

### 能力矩阵

| 维度 | SQLite | Qdrant | Milvus | Neo4j | SurrealDB | **TriviumDB** |
|------|--------|--------|--------|-------|-----------|---------------|
| 关系型数据 | ✅ SQL | ❌ 仅过滤 | ❌ 仅过滤 | ⚠️ 属性 | ✅ SurrealQL | ✅ JSON + $gt/$in |
| 向量检索 | ❌ 需外挂 | ✅ HNSW | ✅ 多索引 | ❌ 需插件 | ✅ ANN | ✅ 可插拔 HNSW |
| 图谱遍历 | ❌ JOIN 模拟 | ❌ | ❌ | ✅ Cypher | ✅ 图查询 | ✅ 原生邻接表 |
| 嵌入式 | ✅ 单文件 | ❌ 独立服务 | ❌ 集群 | ❌ JVM 服务 | ⚠️ RocksDB | ✅ 单 .tdb |
| Python 体验 | ✅ 内置 | ✅ gRPC | ✅ SDK | ⚠️ Bolt | ⚠️ HTTP | ✅ 原生 PyO3 |
| 混合检索 | ❌ | ❌ | ❌ | ❌ | ⚠️ 手动 | ✅ 向量+图扩散 |
| 崩溃恢复 | ✅ WAL | ✅ Raft | ✅ | ✅ | ✅ | ✅ WAL |
| 零 C/C++ 依赖 | ❌ 自身是 C | ✅ | ❌ | ❌ JVM | ❌ RocksDB | ✅ 纯 Rust |

### 代码量对比：实现同一个 AI 记忆系统

**传统方案：SQLite + Qdrant + NetworkX（52 行，三套运行时）**

```python
# 需要: pip install sqlite3 qdrant-client networkx
import sqlite3, networkx as nx
from qdrant_client import QdrantClient
from qdrant_client.models import VectorParams, PointStruct

# 初始化三套系统
sql = sqlite3.connect("meta.db")
sql.execute("CREATE TABLE IF NOT EXISTS nodes (id INT, text TEXT, ts INT)")
qd = QdrantClient(":memory:")
qd.create_collection("mem", VectorParams(size=3, distance="Cosine"))
G = nx.DiGraph()

# 插入：写三次
sql.execute("INSERT INTO nodes VALUES (1, 'Alice likes coffee', 1711440000)")
qd.upsert("mem", [PointStruct(id=1, vector=[0.1, 0.2, 0.7], payload={"text": "..."})])
G.add_node(1)

# 检索：查三次，手动合并
hits = qd.search("mem", [0.1, 0.2, 0.7], limit=5)
for h in hits:
    row = sql.execute("SELECT * FROM nodes WHERE id=?", (h.id,)).fetchone()
    neighbors = list(G.neighbors(h.id))
    # ... 手动拼装结果
```

**TriviumDB 方案（12 行，零外部服务）**

```python
import triviumdb

with triviumdb.TriviumDB("memory.tdb", dim=3) as db:
    # 插入：一步到位
    id1 = db.insert([0.1, 0.2, 0.7], {"text": "Alice likes coffee", "ts": 1711440000})

    # 检索：向量 + 图谱一次完成
    results = db.search([0.1, 0.2, 0.7], top_k=5, expand_depth=2)
    for hit in results:
        print(f"[{hit.id}] {hit.score:.3f} | {hit.payload}")
```

### 关键差异详解

#### vs SQLite
SQLite 是关系型数据库的标杆，但它**没有向量检索和图谱遍历**。虽然可以通过 `sqlite-vss` 扩展加向量检索，但那需要额外的 C 扩展编译，且图谱遍历只能靠递归 CTE（性能差、写法复杂）。TriviumDB 在保持 SQLite 级别的嵌入式单文件体验的同时，原生集成了向量和图谱能力。

#### vs Qdrant / Milvus
这两个是优秀的向量数据库，但它们**需要独立部署服务进程**，且完全没有图谱遍历能力。当你需要「找到相似内容后，再沿关系链扩散」时，必须自己写应用层逻辑跨库 JOIN。TriviumDB 的 `search(expand_depth=N)` 一个调用就搞定。

#### vs Neo4j
Neo4j 是图数据库的王者，但它**需要 JVM 运行时**（体积上百 MB），且原生不支持向量检索。更重要的是，Neo4j 不是嵌入式的——你的 Python 应用必须通过 Bolt 协议连接一个独立的 Neo4j Server。TriviumDB 是纯嵌入式，`import` 即用。

#### vs SurrealDB
SurrealDB 理念相近（多模型统一），但它**底层依赖 RocksDB (C++)**，不是真正的零依赖；它的向量检索和图谱查询需要通过 SurrealQL 语法组合，学习曲线陡峭。TriviumDB 提供的是**函数级 API**——`insert()`, `search()`, `link()`，五分钟上手。

---

## 设计哲学

1. **三合一原子性**：一个 `u64` ID 同时映射到向量、Payload、边表。插入原子、删除原子，永不出现 ID 不一致。

2. **嵌入式优先**：没有 Server、没有端口、没有配置文件。`import triviumdb` 或 `use triviumdb::Database` 就是全部。

3. **渐进式复杂度**：小数据集用 BruteForce 暴搜（精确、零配置）；数据量上去后加一个 `--features hnsw` 即可切换到近似索引。

4. **可预测的性能**：顺序 I/O only（WAL 追加写 + Compaction 顺序重写），绝不发起随机小写。SSD 寿命安全。

5. **Rust 安全边界**：所有公开 API 均为安全代码。内部仅在 mmap 向量块对齐 cast 处存在 1 处 `unsafe`，附完整 SAFETY 注释和对齐检查守卫。

---

## 许可证

Apache-2.0

---

## 作者 (Author)

[YoKONCy](https://github.com/YoKONCy)
