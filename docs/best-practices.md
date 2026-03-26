# TriviumDB 最佳实践

> 高效使用 TriviumDB 的实战指南：从项目集成到性能调优，从数据建模到避坑指南。

---

## 目录

- [快速集成](#快速集成)
- [数据建模范式](#数据建模范式)
- [性能调优](#性能调优)
- [可靠性保障](#可靠性保障)
- [常见使用模式](#常见使用模式)
- [避坑指南](#避坑指南)
- [模型升级与维度迁移](#模型升级与维度迁移)
- [HNSW 索引重建策略](#hnsw-索引重建策略)

---

## 快速集成

TriviumDB 核心使用极致性能的 Rust 编写，但官方已经为各类平台环境预先编译并发布了底层扩展包。**无需在本地配置和折腾任何编译工具链！**

### Node.js / TypeScript 环境搭建

NPM 包内部自带 `triviumdb.d.ts` 提供全量的 TS 类型注解，支持主流全栈框架。

```bash
# 推荐使用 NPM 或者 PNPM 一键提包：
npm install triviumdb

# 验证安装
node -e "console.log(require('triviumdb').TriviumDB.name)"
```

### Python 环境搭建

直接通过 PyPI 拉取原生交叉编译后的 Wheel 包。

```bash
# 推荐使用超快速的 uv (支持 3.9 ~ 3.12)
uv pip install triviumdb

# 或者传统 pip
pip install triviumdb

# 验证安装
python -c "import triviumdb; print('OK')"
```

### Rust 项目集成（原生模式）

```toml
# Cargo.toml
[dependencies]
triviumdb = "0.3.0"  # 或者填本地路径 path = "../TriviumDB"
# 启用 HNSW 索引
# triviumdb = { version = "0.3.0", features = ["hnsw"] }
```

### 30 秒入门模板

```python
import triviumdb

with triviumdb.TriviumDB("my_app.tdb", dim=768) as db:
    # 插入
    id1 = db.insert([0.1] * 768, {"text": "第一条记忆", "ts": 1711440000})
    id2 = db.insert([0.2] * 768, {"text": "第二条记忆", "ts": 1711450000})
    
    # 建立关系
    db.link(id1, id2, label="caused_by", weight=0.9)
    
    # 混合检索
    results = db.search([0.15] * 768, top_k=3, expand_depth=1)
    for hit in results:
        print(f"[{hit.id}] {hit.score:.3f} | {hit.payload}")
# 退出 with 块时自动 flush 落盘
```

---

## 数据建模范式

### 范式一：扁平记忆（最简单）

每条记忆/事件作为独立节点，不建边。适合纯语义搜索场景。

```python
db.insert(encode("今天天气真好"), {"type": "memory", "date": "2024-03-26"})
db.insert(encode("吃了一碗拉面"), {"type": "memory", "date": "2024-03-26"})
# 查询时只用向量检索
results = db.search(encode("天气"), top_k=5, expand_depth=0)
```

### 范式二：实体-关系图（推荐）

为每类实体创建节点，用边表达它们之间的关系。发挥 TriviumDB 的图谱扩散优势。

```python
# 实体节点
person_a  = db.insert(encode("小明"), {"type": "person", "name": "小明"})
person_b  = db.insert(encode("小红"), {"type": "person", "name": "小红"})
place     = db.insert(encode("星巴克三里屯店"), {"type": "place", "name": "星巴克"})
event     = db.insert(encode("小明和小红在星巴克喝咖啡"), {
    "type": "event", "date": "2024-03-26", "summary": "喝咖啡"
})

# 关系边
db.link(event, person_a, label="participant", weight=1.0)
db.link(event, person_b, label="participant", weight=1.0)
db.link(event, place,    label="location",    weight=1.0)
db.link(person_a, person_b, label="friend",   weight=0.8)
db.link(person_b, person_a, label="friend",   weight=0.8)  # 双向

# 检索："谁在喝咖啡？" → 找到事件 → 扩散到人物和地点
results = db.search(encode("喝咖啡"), top_k=3, expand_depth=2)
```

### 范式三：关系具象化（高级）

当关系本身也需要被语义检索时，将关系升格为节点。

```python
# "小明擅长编程" 这个关系本身也是一条可搜索的知识
relation = db.insert(
    encode("擅长编程"),
    {"type": "relation", "label": "skill", "confidence": 0.95}
)
db.link(person_a, relation, label="rel_src", weight=1.0)
db.link(relation, skill_node, label="rel_dst", weight=1.0)
```

### 节点设计 Checklist

| 决策点 | 建议 |
|--------|------|
| `type` 字段 | **必加**。方便按类型过滤：`filter_where({"type": "person"})` |
| `created_at` 字段 | **推荐**。支持时间范围过滤和数据清理 |
| 向量维度 | 与你使用的 embedding 模型对齐（如 OpenAI = 1536, BGE = 768） |
| 边的 label | 使用清晰的英文标签。保持项目内一致 |
| 边的 weight | `0.0~1.0` 表示强弱关联；负值表示抑制关系 |

---

## 性能调优

### 选择合适的 dtype

```python
# 标准场景（推荐）
db = triviumdb.TriviumDB("data.tdb", dim=768, dtype="f32")

# 大规模场景：内存减半，精度损失 < 1%
db = triviumdb.TriviumDB("data.tdb", dim=768, dtype="f16")

# SimHash / 指纹类场景
db = triviumdb.TriviumDB("data.tdb", dim=64, dtype="u64")
```

### 批量写入优化

批量导入大量数据时，临时关闭 WAL 同步可大幅提速：

```python
with triviumdb.TriviumDB("data.tdb", dim=768, sync_mode="off") as db:
    # 批量插入（sync_mode=off 跳过每次写入的 fsync）
    ids = db.batch_insert(
        vectors=all_vectors,
        payloads=all_payloads
    )
    # 全部写完后手动 flush 一次（退出 with 块时也会自动 flush）
    db.flush()
```

> ⚠️ `sync_mode="off"` 期间如果进程崩溃，未 flush 的数据**可能丢失**。仅在初始批量导入时使用。

### 内存预算控制

长时间运行的服务应设置内存上限，避免 MemTable 无限膨胀：

```python
db.set_memory_limit(mb=512)  # 超过 512MB 自动触发 flush
```

### 搜索参数调优

| 参数 | 调大效果 | 调小效果 |
|------|----------|----------|
| `top_k` | 更多候选 → 更高召回率 | 更少候选 → 更快响应 |
| `expand_depth` | 更深扩散 → 发现更远关联 | 更浅扩散 → 避免噪声 |
| `min_score` | 更严格 → 结果更精准 | 更宽松 → 结果更多 |

**推荐起步参数：**
```python
db.search(query_vec, top_k=10, expand_depth=1, min_score=0.5)
```

### Compaction 策略

| 场景 | 推荐配置 |
|------|----------|
| 生产服务（持续写入） | `db.enable_auto_compaction(interval_secs=60)` |
| 批量导入（一次性） | 不启用，导入完成后手动 `flush()` |
| 低频写入的查询服务 | `db.enable_auto_compaction(interval_secs=300)` |

---

## 可靠性保障

### 数据安全等级选择

| 场景 | sync_mode | auto_compaction | 说明 |
|------|-----------|-----------------|------|
| 金融级零丢失 | `"full"` | 60s | 每次写入 fsync，性能最低 |
| 生产服务（推荐） | `"normal"` | 60s | 均衡方案，OS 崩溃可能丢最近几条 |
| 开发/测试 | `"off"` | 关闭 | 最快，不保证持久化 |
| 批量导入 | `"off"` → `"normal"` | 关闭 | 导入时提速，完成后切回 |

### 正确关闭数据库

```python
# ✅ 推荐：使用 with 语句
with triviumdb.TriviumDB("data.tdb", dim=768) as db:
    # ... 操作 ...
    pass  # 退出时自动 flush

# ✅ 手动关闭
db = triviumdb.TriviumDB("data.tdb", dim=768)
# ... 操作 ...
db.flush()  # 必须手动调用！

# ❌ 错误：不调用 flush 就退出 → WAL 里的数据下次重启才会回放
```

### 文件锁冲突处理

TriviumDB 使用独占文件锁防止多进程并发写入。如果遇到锁冲突：

```
RuntimeError: Database 'data.tdb' is already opened by another process.
If this is unexpected, delete 'data.tdb.lock'
```

**解决方法：**
1. 确认没有其他进程在使用该数据库
2. 如果进程异常退出残留了锁文件，手动删除 `data.tdb.lock`

---

## 常见使用模式

### 模式一：AI Agent 长期记忆

```python
def store_memory(db, text, embedding_model, metadata=None):
    """将一段对话/观察存入记忆库"""
    vec = embedding_model.encode(text).tolist()
    payload = {"text": text, "ts": time.time(), "type": "memory"}
    if metadata:
        payload.update(metadata)
    return db.insert(vec, payload)

def recall(db, query_text, embedding_model, top_k=5, depth=2):
    """根据查询文本召回相关记忆"""
    vec = embedding_model.encode(query_text).tolist()
    return db.search(vec, top_k=top_k, expand_depth=depth, min_score=0.4)
```

### 模式二：知识库 + 图谱导航

```python
# 两种检索方式并存
# 方式 A：语义搜索 → 精准定位
results = db.search(encode("Python 异步编程"), top_k=5)

# 方式 B：图谱查询 → 结构化导航
rows = db.query('MATCH (a {type: "concept"})-[:related]->(b) RETURN b')
```

### 模式三：更新边权而非覆盖

由于 `unlink` 会断开源节点到目标节点的**所有**边，更新特定类型的边权重需要谨慎：

```python
def update_edge_weight(db, src, dst, label, new_weight):
    """安全地更新特定边的权重"""
    # 先查看当前所有边（通过 get 获取 node 的 edges 信息）
    db.unlink(src, dst)
    db.link(src, dst, label=label, weight=new_weight)
```

> ⚠️ 如果同一对 (src, dst) 之间有多种 label 的边，`unlink` 会全部断开。
> 建议同一对节点之间只建立一条边，用 label 区分类型。

### 模式四：定期清理过期数据

```python
import time

# 找出 7 天前的旧数据
cutoff = time.time() - 7 * 86400
old_nodes = db.filter_where({"ts": {"$lt": cutoff}})

# 逐个删除（三层联删，自动清理关联的边和向量）
for node in old_nodes:
    db.delete(node.id)

db.flush()  # 落盘
```

---

## 避坑指南

### ❌ 坑 1：维度不匹配

```python
db = triviumdb.TriviumDB("data.tdb", dim=768)
db.insert([0.1] * 512, {"text": "hello"})  # 💥 DimensionMismatch!
```

**规则**：`dim` 在创建数据库时确定，之后所有 insert / update_vector / search 的向量长度**必须等于 dim**。

### ❌ 坑 2：忘记 flush

```python
db = triviumdb.TriviumDB("data.tdb", dim=768)
db.insert([0.1] * 768, {"text": "important data"})
# 程序退出... 数据可能只在 WAL 里，下次重启才回放！
```

**解决**：始终使用 `with` 语句，或在程序退出前调用 `db.flush()`。

### ❌ 坑 3：对已删除节点建边

```python
db.delete(42)
db.link(1, 42, label="ref")  # 💥 NodeNotFound!
```

**规则**：`link` 要求两个端点都必须存在。已删除的节点不能作为边的端点。

### ❌ 坑 4：unlink 的范围比预期大

```python
db.link(1, 2, label="friend", weight=0.8)
db.link(1, 2, label="colleague", weight=0.5)
db.unlink(1, 2)  # ⚠️ 两条边都被断开了！
```

**规则**：`unlink(src, dst)` 会移除 src → dst 之间的**全部**边，不区分 label。

### ❌ 坑 5：多进程同时打开

```python
# 进程 A
db_a = triviumdb.TriviumDB("shared.tdb", dim=768)

# 进程 B（同时）
db_b = triviumdb.TriviumDB("shared.tdb", dim=768)  # 💥 文件锁冲突!
```

**规则**：TriviumDB 是嵌入式数据库，同一个 `.tdb` 文件同一时刻只能被一个进程打开。如需多进程访问，请在应用层实现读写代理。

---

## 模型升级与维度迁移

当你需要切换 Embedding 模型（导致向量维度变化）时，不必重建整个数据库——使用 `migrate()` 保留全部 Payload 和图谱结构。

### 标准迁移流程

```python
import triviumdb

OLD_DIM = 768    # 旧模型维度（如 BGE-small）
NEW_DIM = 1536   # 新模型维度（如 OpenAI text-embedding-3-small）

# 第一步：迁移结构（payload + 图谱边保留，向量置零）
with triviumdb.TriviumDB("knowledge.tdb", dim=OLD_DIM) as old_db:
    node_ids = old_db.migrate("knowledge_v2.tdb", new_dim=NEW_DIM)
    print(f"结构迁移完成，共 {len(node_ids)} 个节点待更新向量")

# 第二步：打开新库，用新模型逐节点重新编码
with triviumdb.TriviumDB("knowledge_v2.tdb", dim=NEW_DIM) as new_db:
    for nid in node_ids:
        node = new_db.get(nid)
        if node and "text" in node.payload:
            new_vec = new_model.encode(node.payload["text"]).tolist()
            new_db.update_vector(new_vec, nid)
    print("向量更新完成")
# 退出 with 自动 flush
```

### 迁移注意事项

| 注意点 | 说明 |
|--------|------|
| 原库不受影响 | `migrate()` 只读原库，不会修改或删除任何数据 |
| 新库初始不可检索 | 迁移后所有向量为零，必须先更新向量才能进行语义搜索 |
| 图谱关系完整保留 | 所有 `link()` 建立的边、label、weight 全部复制到新库 |
| 同时换 dtype | 建以 `TriviumDB("new.tdb", dim=NEW_DIM, dtype="f16")` 创建新库后再迁移 |
| 大库分批更新向量 | 每更新 1000 个节点手动 `flush()` 一次，避免内存积压 |

---

## HNSW 索引重建策略

*仅在使用 `--features hnsw` 时相关，BruteForce 模式下忽略此章节。*

### 何时需要 rebuild_index？

| 场景 | 建议 |
|------|------|
| 初次批量导入几十万条数据 | 导入完成后调用一次 `rebuild_index()` |
| 增量插入少量数据（< 5%） | 通常不需要重建，HNSW 增量维护即可 |
| 删除了大量节点（> 20%） | 建议重建，清理已失效的图层引用 |
| 完成维度迁移并更新完向量后 | 必须重建，旧索引对新库无效 |

### 批量导入 + 重建的完整模板

```python
# 最优批量导入流程（HNSW 模式）
with triviumdb.TriviumDB("data.tdb", dim=768, sync_mode="off") as db:
    # 1. 批量写入（关闭 WAL 同步提速）
    db.batch_insert(all_vectors, all_payloads)
    # 2. 切回安全同步模式
    db.set_sync_mode("normal")
    # 3. 重建索引（同步阻塞，完成后立即生效）
    db.rebuild_index()
    # 4. 落盘（with 退出时也会自动执行）
    db.flush()
```

> 💡 `rebuild_index()` 是同步操作，重建期间会持有 MemTable 读锁。对于百万级节点建议在低峰期执行。
