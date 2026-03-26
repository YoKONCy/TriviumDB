<div align="center">

# TriviumDB

**向量 × 图谱 × 关系型 —— 三位一体的 AI 原生嵌入式数据库**

> _Trivium_：拉丁语，意为"三条道路的交汇"。

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

| 步骤         | 传统三库方案                 | TriviumDB                          |
| ------------ | ---------------------------- | ---------------------------------- |
| ① 存语义向量 | 调 Qdrant API 写入 embedding | `db.insert(vec, payload)` 一步完成 |
| ② 存元数据   | 调 SQLite 写入时间、场景     | ↑ 同一步，payload 里就是 JSON      |
| ③ 存关系     | 调 Neo4j: 用户→地点→人物     | `db.link(user, cafe, "went_to")`   |
| ④ 后续召回   | 3 次跨库查询 + 手写合并      | `db.search(vec, expand_depth=2)`   |
| ⑤ 迁移数据   | 导出 3 份 + 写转换脚本       | 复制 `memory.tdb` 一个文件         |

### 适用场景

| 场景                     | 怎么用 TriviumDB                                                                                      |
| ------------------------ | ----------------------------------------------------------------------------------------------------- |
| 🤖 **AI Agent 长期记忆** | 每条对话存为节点（embedding + 原文 + 时间戳），人物/地点/事件之间建边，召回时先向量匹配再沿关系链扩散 |
| 🎮 **游戏 NPC 认知引擎** | NPC 观察到的事件存为带向量的节点，NPC 之间的关系用图谱表达，对话时检索相关记忆自动生成回应            |
| 📚 **个人知识库**        | Markdown 笔记切片后存入，概念之间手动或自动连边，语义搜索 + 知识图谱导航双模式浏览                    |
| 🔬 **小型推荐系统**      | 用户和物品各为节点，交互行为存为带权边，混合检索实现「相似用户喜欢的 + 你的社交圈在看的」             |
| 🧬 **生物信息学**        | 基因/蛋白质序列的 embedding + 互作关系网络，一库搜到相似序列并自动追溯代谢通路                        |

---

## 快速上手

### 安装
> 💡 TriviumDB 核心使用 Rust 编写，但我们已经在云端为您提前交叉编译了所有平台的二进制，**无需在本地安装任何编译环境即可秒速安装！**

### 🐍 Python 用户

推荐使用超快的 [uv](https://github.com/astral-sh/uv) （只需毫秒级）：
```bash
uv pip install triviumdb
```
或者使用传统 pip：
```bash
pip install triviumdb
```

### 🌐 Node.js / 前端用户

跨平台包已自带 `*.node` 预编译拓展，并含有完整的 TypeScript 补全：
```bash
npm install triviumdb
# 或者
pnpm add triviumdb
```

### 🦀 Rust 原生用户

直接把我们当成 Library 依赖：
```bash
cargo add triviumdb
```

### 30 秒入门

```python
import triviumdb

with triviumdb.TriviumDB("memory.tdb", dim=3) as db:
    id1 = db.insert([0.12, -0.45, 0.78], {"text": "小明喜欢吃苹果"})
    id2 = db.insert([0.08, -0.52, 0.81], {"text": "小红送了小明一箱苹果"})
    db.link(id1, id2, label="caused_by", weight=0.95)

    results = db.search([0.10, -0.48, 0.80], top_k=5, expand_depth=2, min_score=0.6)
    for hit in results:
        print(f"[{hit.id}] score={hit.score:.3f} | {hit.payload}")
```

> 📖 完整 API 参考、高级用法和 Rust 示例请查看 **[API 参考文档](docs/api-reference.md)**。

---

## 核心特性

| 特性                | 说明                                                                        |
| ------------------- | --------------------------------------------------------------------------- |
| 🔍 **混合检索**     | 向量锚定 → Top-K → 图谱扩散（Spreading Activation）→ 最终排序               |
| 📦 **统一数据模型** | 每个节点同时持有向量（f32×dim）、JSON 元数据和图谱边，共享全局 `u64` 主键   |
| ⚡ **多核并行**     | rayon 并行向量扫描 + mmap 零拷贝加载 + 可选 HNSW 索引                       |
| 💾 **SSD 友好**     | Append-Only WAL + 后台 Compaction，杜绝随机写入                             |
| 🛡️ **崩溃恢复**     | WAL + CRC32 校验 + 原子写入，断电不丢数据                                   |
| 🔎 **高级过滤**     | 类 MongoDB 语法：`$eq/$ne/$gt/$lt/$in/$and/$or`                             |
| 📝 **图谱查询**     | 内置类 Cypher 查询引擎：`MATCH (a)-[:knows]->(b) WHERE b.age > 18 RETURN b` |
| 🐍 **Python 原生**  | PyO3 绑定，`pip install` 后直接 `import triviumdb`                          |

> 📖 深入了解架构设计和技术细节请查看 **[支持特性详解](docs/features.md)**。

---

## 向量索引策略

通过 Cargo Features 在编译期选择索引后端：

| 后端           | 特点                              | 适用规模         | Feature Flag |
| -------------- | --------------------------------- | ---------------- | ------------ |
| **BruteForce** | 100% 精确召回，rayon 多核并行     | < 10 万节点      | 默认启用     |
| **HNSW**       | 亚毫秒级近似搜索，O(log N) 复杂度 | 10 万 ~ 千万节点 | `hnsw`       |

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

### v0.3 — 生态拓展 ✅

- [x] Node.js 扩展绑定 (napi-rs)
- [ ] 子图导出 / 批量导入
- [ ] CLI 工具 (`triviumdb-cli`)
- [ ] 性能基准测试套件 (benchmark)
- [ ] 分布式分片存储

---

## 与现有方案对比

| 维度          | SQLite       | Qdrant      | Neo4j       | SurrealDB    | **TriviumDB**     |
| ------------- | ------------ | ----------- | ----------- | ------------ | ----------------- |
| 关系型数据    | ✅ SQL       | ❌ 仅过滤   | ⚠️ 属性     | ✅ SurrealQL | ✅ JSON + $gt/$in |
| 向量检索      | ❌ 需外挂    | ✅ HNSW     | ❌ 需插件   | ✅ ANN       | ✅ 可插拔 HNSW    |
| 图谱遍历      | ❌ JOIN 模拟 | ❌          | ✅ Cypher   | ✅ 图查询    | ✅ 原生邻接表     |
| 嵌入式单文件  | ✅           | ❌ 独立服务 | ❌ JVM 服务 | ⚠️ RocksDB   | ✅ 单 .tdb        |
| 混合检索      | ❌           | ❌          | ❌          | ⚠️ 手动      | ✅ 向量+图扩散    |
| 零 C/C++ 依赖 | ❌           | ✅          | ❌ JVM      | ❌ RocksDB   | ✅ 纯 Rust        |

---

## 设计哲学

1. **三合一原子性**：一个 `u64` ID 同时映射到向量、Payload、边表。插入原子、删除原子，永不出现 ID 不一致。
2. **嵌入式优先**：没有 Server、没有端口、没有配置文件。`import triviumdb` 就是全部。
3. **渐进式复杂度**：小数据集用 BruteForce 暴搜；数据量上去后 `--features hnsw` 一键切换近似索引。
4. **可预测的性能**：顺序 I/O only（WAL 追加写 + Compaction 顺序重写），SSD 寿命安全。
5. **Rust 安全边界**：所有公开 API 均为安全代码。内部仅存在 1 处 `unsafe`（mmap 对齐 cast），附完整 SAFETY 注释。

---

## 📖 文档

| 文档                                      | 说明                                             |
| ----------------------------------------- | ------------------------------------------------ |
| **[API 完整参考](docs/api-reference.md)** | 全部 Python / Rust API、参数说明、返回值类型     |
| **[支持特性详解](docs/features.md)**      | 架构设计、存储引擎、索引策略、崩溃恢复等技术细节 |
| **[最佳实践](docs/best-practices.md)**    | 数据建模范式、性能调优、可靠性保障、避坑指南     |

---

## 许可证

Apache-2.0
作者：[YoKONCy](https://github.com/YoKONCy)

---
