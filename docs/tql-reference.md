# TQL (Trivium Query Language) 完整参考

> **版本**: v0.8.5
> **定位**: 统一查询 DSL — 融合文档过滤、图模式匹配、向量检索于一体  
> **前置依赖**: 零外部依赖，纯 Rust 实现

---

## 目录

- [概述](#概述)
- [快速入门](#快速入门)
- [FIND — 文档过滤查询](#find--文档过滤查询)
- [MATCH — 图模式匹配](#match--图模式匹配)
- [SEARCH — 向量检索](#search--向量检索)
- [WHERE — 统一谓词系统](#where--统一谓词系统)
- [RETURN / ORDER BY / LIMIT / OFFSET](#return--order-by--limit--offset)
- [操作符速查表](#操作符速查表)
- [WITH 可组合管线](#with-可组合管线)
- [表达式、聚合与空值](#表达式聚合与空值)
- [Prepared TQL](#prepared-tql)
- [路径与集合代数](#路径与集合代数)
- [形式语法与内部架构](#形式语法与内部架构)
- [当前边界](#当前边界)

---

## 概述

TQL 是 TriviumDB 的统一查询语言，也是面向三模数据的 **自由 DIY 混合查询管线**。它不是把几条预设 RAG 流程包装成语法糖，而是让开发者把向量召回、属性过滤、图模式、图扩展、图算法、路径、集合运算、迭代、聚合和重排作为算子，按照自己的业务语义自由编排。

这种自由不是无边界的字符串拼接：每个阶段都有明确输入/输出类型、作用域、确定性规则和预算切片，完整查询仍由 Parser、Cascades 与 Pipeline 统一验证和优化。

| 入口 | 对应能力 |
|------|---------|
| `FIND` | MongoDB 风格文档过滤与属性索引规划 |
| `MATCH` / `OPTIONAL MATCH` | 图模式、可变长路径与 GraphFirst |
| `SEARCH` | 向量源、确定性扩展与跨模管线 |
| `WITH` | 命名 NodeSet、图算法、集合、路径、迭代与聚合组合 |

**设计哲学**：

- **文档-图-向量 三位一体**：一条 TQL 语句可以同时表达文档过滤、图遍历和向量检索
- **渐进式复杂度**：简单查询极简，高级功能按需叠加
- **与存储引擎深度集成**：内联节点过滤支持完整的 MongoDB `$op` 语法，WHERE 子句同时支持 Cypher 比较和 `MATCHES` 文档断言
- **零成本抽象**：解析器和执行器完全内联，无运行时反射开销
- **索引交集**：`AND` 中多个已建索引的等值条件会先按候选集大小排序并求交，再执行完整谓词校验
- **有界扩展**：`SEARCH ... EXPAND` 与底层确定性遍历共享访问节点、结果和边扫描预算语义

### 调用方式

**Rust：**
```rust
let results = db.tql(r#"FIND {type: "event", heat: {$gte: 0.7}} RETURN * LIMIT 10"#)?;
for row in &results {
    let node = &row["_"];
    println!("[{}] {:?}", node.id, node.payload);
}
```

**Python：**
```python
results = db.tql('FIND {type: "event", heat: {$gte: 0.7}} RETURN * LIMIT 10')
for result in results:
    print(result.row["_"]["payload"])
```

---

## 快速入门

### 30 秒上手

```sql
-- 文档过滤：查找所有 type=person 的节点
FIND {type: "person"} RETURN *

-- 图遍历：沿 knows 边找到 Alice 的朋友
MATCH (a {name: "Alice"})-[:knows]->(b) RETURN b

-- 向量检索：找最相似的 5 个节点
SEARCH VECTOR [0.1, 0.2, 0.3] TOP 5 RETURN *
```

### 完整能力示例

```sql
-- 复杂图遍历 + 文档过滤 + 排序分页
MATCH (a {region: "cn"})-[:knows|works_with*1..3]->(b)
WHERE b.age > 25 AND b MATCHES {role: {$in: ["engineer", "manager"]}}
RETURN a, b
ORDER BY b.age DESC
LIMIT 20 OFFSET 10

-- 向量检索 + 图扩散 + 过滤
SEARCH VECTOR [0.1, -0.2, 0.8, ...] TOP 10
EXPAND BOTH [:related*1..2]
WHERE {type: "event"}
RETURN *

-- GraphFirst：先匹配合法图结构，再在 doc 集合内精确向量排名
MATCH (doc)-[:cites]->(ref)
RANK doc BY VECTOR [0.1, -0.2, 0.8, ...] TOP 10
RETURN doc, ref
```

> 💡 TQL 支持行注释：以 `--` 开头的内容到行尾会被忽略。

---

## FIND — 文档过滤查询

`FIND` 入口对全库节点的 JSON Payload 进行条件过滤，功能完全覆盖旧的 `db.filter()` 和 `db.filter_where()` API。

### 基础语法

```
FIND {文档过滤条件} RETURN ...
```

### 等值匹配

```sql
-- 精确匹配单字段
FIND {type: "person"} RETURN *

-- 多字段隐式 AND
FIND {type: "person", region: "cn"} RETURN *
```

### 操作符过滤

```sql
-- 数值比较
FIND {age: {$gt: 18}} RETURN *
FIND {score: {$gte: 0.8, $lt: 1.0}} RETURN *

-- 集合匹配
FIND {role: {$in: ["admin", "mod"]}} RETURN *
FIND {status: {$nin: ["deleted", "banned"]}} RETURN *

-- 字段存在性
FIND {avatar: {$exists: true}} RETURN *

-- 数组操作
FIND {tags: {$all: ["rust", "database"]}} RETURN *
FIND {tags: {$size: 3}} RETURN *

-- 类型匹配
FIND {metadata: {$type: "object"}} RETURN *
```

### 逻辑组合

```sql
-- 显式 $or
FIND {$or: [{age: {$lt: 18}}, {role: "admin"}]} RETURN *

-- 显式 $and（等价于隐式多字段）
FIND {$and: [{age: {$gte: 18}}, {region: "cn"}]} RETURN *
```

### 排序与分页

```sql
-- ORDER BY + LIMIT + OFFSET
FIND {type: "event"} RETURN *
ORDER BY _.heat DESC
LIMIT 10 OFFSET 20
```

> 💡 `FIND` 场景下节点绑定到隐式变量 `_`，在 `ORDER BY` 中通过 `_.field` 引用字段。

### 支持的全部操作符

| 操作符 | 含义 | 值类型 | TQL 示例 |
|--------|------|--------|----------|
| `$eq` | 等于 | 任意 | `{name: {$eq: "Alice"}}` 或 `{name: "Alice"}` |
| `$ne` | 不等于 | 任意 | `{status: {$ne: "deleted"}}` |
| `$gt` | 大于 | 数字 | `{age: {$gt: 18}}` |
| `$gte` | 大于等于 | 数字 | `{score: {$gte: 0.8}}` |
| `$lt` | 小于 | 数字 | `{age: {$lt: 30}}` |
| `$lte` | 小于等于 | 数字 | `{price: {$lte: 99.9}}` |
| `$in` | 包含于列表 | 数组 | `{role: {$in: ["admin", "mod"]}}` |
| `$nin` | 不包含于列表 | 数组 | `{status: {$nin: ["deleted"]}}` |
| `$exists` | 字段存在性 | 布尔 | `{avatar: {$exists: true}}` |
| `$size` | 数组长度 | 整数 | `{tags: {$size: 3}}` |
| `$all` | 数组全包含 | 数组 | `{tags: {$all: ["a", "b"]}}` |
| `$type` | 字段类型 | 字符串 | `{data: {$type: "object"}}` |
| `$and` | 逻辑与 | 条件数组 | `{$and: [{...}, {...}]}` |
| `$or` | 逻辑或 | 条件数组 | `{$or: [{...}, {...}]}` |

---

## MATCH — 图模式匹配

`MATCH` 入口沿图谱边进行模式匹配遍历，完全覆盖旧的 `db.query()` API，并新增可变长路径、多标签边等高级能力。

### 基础语法

```
MATCH (节点模式)(-[边模式]->(节点模式))* (WHERE 谓词)? RETURN ...
```

### 节点模式

```sql
-- 裸节点（无条件，匹配所有节点）
MATCH (a) RETURN a

-- 内联等值属性过滤
MATCH (a {name: "Alice"}) RETURN a

-- 内联 MongoDB 操作符（Q1-B 决策）
MATCH (a {age: {$gte: 30}}) RETURN a

-- 空属性大括号（等价于无条件）
MATCH (a {}) RETURN a

-- 按 ID 精确查找（O(1) 短路优化）
MATCH (a {id: 42}) RETURN a
```

> 💡 当内联属性包含 `{id: N}` 时，执行器会启用 **O(1) 主键哈希短路**，跳过全表扫描直接定位节点。

### 边模式

```sql
-- 按标签过滤
MATCH (a)-[:knows]->(b) RETURN b

-- 通配边（匹配任意标签）
MATCH (a)-[]->(b) RETURN b

-- 多标签 OR（管道符分隔，Q2-A 决策）
MATCH (a)-[:knows|works_with]->(b) RETURN b
```

### 多跳路径

```sql
-- 两跳路径
MATCH (a {name: "Alice"})-[:knows]->(b)-[:likes]->(c) RETURN c

-- 三跳路径
MATCH (a)-[:next]->(b)-[:next]->(c)-[:next]->(d) RETURN d
```

### 可变长路径

```sql
-- 1 到 3 跳的 knows 关系
MATCH (a {name: "Alice"})-[:knows*1..3]->(b) RETURN b

-- 任意边 2 到 5 跳
MATCH (a)-[*2..5]->(b) RETURN b

-- 多标签 + 可变长组合
MATCH (a)-[:knows|works_with*1..2]->(b) RETURN b
```

**可变长路径执行机制**：

```
DFS 遍历 + 环检测（HashSet visited）
├── depth < min_depth: 继续展开，不收敛
├── min_depth <= depth <= max_depth: 收敛到下一层 + 继续展开
└── depth == max_depth: 仅收敛，停止展开
```

> ⚠️ 可变长路径内置环检测：同一条路径上不会重复访问已经到达过的节点，防止无限循环。

### WHERE 条件

```sql
-- Cypher 风格比较
MATCH (a)-[:knows]->(b) WHERE b.age > 25 RETURN b

-- AND / OR 组合
MATCH (a)-[:knows]->(b)
WHERE b.age > 18 AND (b.role == "admin" OR b.role == "mod")
RETURN b

-- MATCHES 文档断言（将 MongoDB Filter 绑定到变量）
MATCH (a)-[:authored]->(e)
WHERE e MATCHES {heat: {$gte: 0.5}, type: "event"}
RETURN a, e

-- NOT 取反
MATCH (a)-[:knows]->(b)
WHERE NOT b.role == "banned"
RETURN b
```

### 变量绑定规则

| 规则 | 说明 |
|------|------|
| 路径中间/末尾节点**必须**指定变量名 | `(a)-[]->(b)` ✅ / `(a)-[]->()` ❌ |
| 包含边的路径中起始节点**必须**指定变量名 | `(a)-[]->(b)` ✅ / `()-[]->(b)` ❌ |
| 纯单节点查询允许匿名 | `MATCH (n) RETURN n` ✅ |
| 变量在 RETURN 中引用 | `RETURN a, b` |
| `RETURN *` 返回所有绑定变量 | `RETURN *` |

### 执行器安全机制

| 机制 | 配置 | 说明 |
|------|------|------|
| 预算熔断 | 100,000 步 | 单次查询最多评估 10 万步，防止内存爆炸 |
| 行数上限 | LIMIT 或默认 5,000 | 结果行数达标后立即停止所有 DFS 分支 |
| 环路检测 | 可变长路径 | `HashSet<u64>` 跟踪已访问节点 |

---

## SEARCH — 向量检索

`SEARCH` 入口执行向量相似度检索，并可选通过 `EXPAND` 子句沿图谱扩散，将语义锚点与结构关系融合。

### 基础语法

```
SEARCH VECTOR [v1, v2, ...] TOP k (EXPAND [...])? (WHERE 谓词)? RETURN ...
```

### 基础向量检索

```sql
-- 找最相似的 5 个节点
SEARCH VECTOR [0.1, 0.2, 0.3] TOP 5 RETURN *

-- 支持负数分量
SEARCH VECTOR [0.1, -0.48, 0.8] TOP 10 RETURN *
```

### 带图扩散 (EXPAND)

```sql
-- 向量锚点 + 1 跳 related 扩散
SEARCH VECTOR [0.1, 0.2, 0.3] TOP 5
EXPAND [:related*1..2]
RETURN *

-- 多标签扩散
SEARCH VECTOR [0.1, 0.2] TOP 3
EXPAND [:knows|works_with*1..3]
RETURN *

-- 反向遍历入边
SEARCH VECTOR [0.1, 0.2] TOP 3
EXPAND INCOMING [:cites*1..2]
RETURN *

-- 同时遍历出边和入边
SEARCH VECTOR [0.1, 0.2] TOP 3
EXPAND BOTH [*1..2]
RETURN *
```

方向可为 `OUTGOING`、`INCOMING` 或 `BOTH`；省略时默认 `OUTGOING`。标签列表支持 `|` 分隔，省略标签表示全部边。EXPAND 使用确定性最短路径 Reachability 收集候选；候选或访问预算超限时明确报错，不静默截断。

**SEARCH + EXPAND 逻辑流程**：

```text
查询向量 → Cascades 选择向量访问路径 → 稳定 Top-K 锚点
                                      │
                                      ▼
                             确定性 Reachability
                                      │
                                      ▼
                             候选去重 → WHERE → 投影
```

实际物理路径可使用可用的向量索引或 exact fallback，并受统计信息、访问模式和预算约束。

### 带 WHERE 过滤

```sql
-- 向量检索 + 文档过滤
SEARCH VECTOR [0.5, 0.5] TOP 10
WHERE {type: "event"}
RETURN *

-- 向量检索 + Cypher 比较
SEARCH VECTOR [0.5, 0.5] TOP 10
WHERE _.score > 0.8
RETURN *
```

> 💡 `SEARCH` 的 WHERE 过滤在向量打分和 EXPAND 之后执行，作为最终的候选集筛选。

> `SEARCH` 进入统一 NodeSet/Cascades 管线；物理访问路径由统计、索引可用性和预算共同决定。需要认知检索、文本双路召回或 Hook 时仍使用 `search_advanced()`；两者语义不同，不应仅按“快/慢入口”区分。

---

## MATCH + RANK — GraphFirst 约束检索

`RANK` 将 `MATCH` 产生的某个绑定变量视为 anchor，在去重后的 anchor 集合内执行精确向量评分：

```sql
MATCH (doc)-[:cites|mentions]->(ref)
WHERE ref.type == "paper"
RANK doc BY VECTOR [0.1, 0.2, 0.3] TOP 20
RETURN doc, ref
LIMIT 10 OFFSET 5
```

执行语义固定为：

1. `MATCH` 与 `WHERE` 先生成合法绑定行；
2. 按 `RANK` 变量的 NodeId 去重，同一 anchor 的多条路径只保留变量名顺序下 NodeId 元组最小的规范行；
3. 在 anchor 集合内精确评分，按 score 降序、NodeId 升序稳定取 Top-K；
4. 执行聚合或 DISTINCT、显式 `ORDER BY`、`OFFSET`、`LIMIT` 和投影裁剪。

因此显式 `ORDER BY` 会覆盖向量排名顺序；不写 `ORDER BY` 时保留 RANK 顺序。RANK score 当前仅用于排序，不作为可投影字段暴露。anchor 超过 100,000 个、变量未绑定或向量维度不匹配时明确报错。

`EXPLAIN MATCH ... RANK ...` 会在 `optimizations` 中报告 `GraphFirst exact anchor ranking`。

---

## WHERE — 统一谓词系统

TQL 的 WHERE 子句统一了两种过滤范式，可在同一条件中自由组合：

### Cypher 比较表达式

```sql
WHERE a.age > 25
WHERE b.name == "Alice"
WHERE a.score >= 0.8 AND a.score < 1.0
```

**支持的比较运算符**：

| 运算符 | 含义 |
|--------|------|
| `==` | 等于 |
| `!=` | 不等于 |
| `>` | 大于 |
| `>=` | 大于等于 |
| `<` | 小于 |
| `<=` | 小于等于 |

**属性访问**: `变量名.字段名`，特殊字段 `id` 引用节点的结构主键。

### MATCHES 文档断言

```sql
-- 将完整的 MongoDB 过滤器绑定到变量
WHERE b MATCHES {age: {$gte: 18}, role: {$in: ["admin", "mod"]}}

-- 无变量绑定（FIND/SEARCH 场景）
WHERE {type: "event"}
```

### 逻辑组合

```sql
-- AND
WHERE a.age > 18 AND b.name == "Bob"

-- OR
WHERE a.role == "admin" OR a.role == "mod"

-- NOT
WHERE NOT a.status == "banned"

-- 括号优先级
WHERE (a.age > 18 OR a.role == "admin") AND b.active == true

-- 混合 Cypher + MATCHES
WHERE a.age > 25 AND b MATCHES {tags: {$all: ["rust"]}}
```

### 类型安全

| 场景 | 行为 |
|------|------|
| 字段不存在 | 比较结果为 `false`，不报错 |
| 类型不匹配（如 `age > "text"`）| 比较结果为 `false`，不报错 |
| `Int` vs `Float` 跨类型比较 | 自动提升为 `f64` 比较 |
| `Null` 值 | 与任何值比较均为 `false` |

---

## RETURN / ORDER BY / LIMIT / OFFSET

### RETURN

```sql
RETURN *
RETURN a, b
RETURN a.score * 1.2 + 3 AS adjusted
RETURN COALESCE(a.nickname, a.name, "unknown") AS display
RETURN COUNT(*) AS total, AVG(a.score) AS avg_score
```

- `FIND` / `SEARCH` 场景下，`RETURN *` 将节点绑定到隐式变量 `_`
- `MATCH` 场景下，`RETURN *` 返回模式中所有具名节点变量

### ORDER BY

```sql
ORDER BY b.age ASC          -- 升序（默认）
ORDER BY b.age DESC         -- 降序
ORDER BY a.name, b.age DESC -- 多字段排序
ORDER BY _.heat DESC        -- FIND/SEARCH 场景
```

### LIMIT / OFFSET

```sql
LIMIT 10              -- 最多返回 10 条
LIMIT 10 OFFSET 20    -- 跳过前 20 条，返回 10 条
```

**执行顺序**：`WHERE 过滤 → RANK（如有）→ 聚合/DISTINCT → ORDER BY → OFFSET → LIMIT`

---

## WITH 可组合管线

`WITH` 把每个阶段的 NodeSet 绑定到显式 alias，作用域在解析期校验；未定义变量和错误阶段输入会在执行前拒绝，不产生部分结果。

```sql
SEARCH VECTOR [0.1, 0.2] TOP 100 AS seed
WITH seed
EXPAND seed [:cites*1..2] AS related
WITH related
WHERE similarity(related) > 0.5
RETURN related, similarity(related) AS sim
ORDER BY sim DESC LIMIT 10
```

可组合阶段包括 `EXPAND`、`FILTER/WHERE`、`RANK`、PageRank、WCC、Degree/Betweenness、Leiden、Label Propagation、SA-PPR、`ALL_PATHS`、`SHORTEST_PATHS`、`UNION/INTERSECT/EXCEPT` 与 `ITERATE`。每阶段接受预算切片，并在 EXPLAIN 中暴露成本、预计行数、临时字节和物理实现。

## 表达式、聚合与空值

表达式支持 `+ - * /`、括号优先级、参数、属性、一等分数、`COALESCE`、`IS NULL/IS NOT NULL`、`path()` 与 `path_length()`。除零、非数值算术和非有限结果返回 Null，不 panic。

聚合支持 `COUNT/SUM/AVG/MIN/MAX/COLLECT` 与 aggregate `DISTINCT`。RETURN 中非聚合表达式构成隐式分组键；空输入 `COUNT(*)=0`，其他无值聚合返回 Null。Rust 的 `tql()` 与 Python/Node 动态语言入口均返回统一一等值；`tql_nodes()` 保留给只需要节点绑定的 Rust 调用方，`tql_values()` 是兼容别名。

## Prepared TQL

```python
prepared = db.prepare_tql(
    "SEARCH VECTOR [$x, $y] TOP 10 AS seed WITH seed RETURN seed, $bonus + 1 AS score"
)
print(prepared.parameter_names())
rows = db.execute_prepared_tql(prepared, {"x": 0.2, "y": 0.8, "bonus": 4})
```

`SEARCH VECTOR` 的方括号内可混合有限数字字面量与 Prepared 数值参数，例如 `[0.1, $y, -0.3]`。参数只在 bind 阶段写入连续向量，绑定完成后复用与字面量完全相同的 QuIVer/精确检索热路径，不增加候选打分开销。

Node 使用 `prepareTql/executePreparedTql`，Rust 使用 `prepare_tql/execute_prepared_tql`。缺参、额外参数、非数值向量参数和非有限数值全部 fail-closed；同一 Prepared 对象可重复绑定执行。

## 路径与集合代数

```sql
SEARCH VECTOR [1, 0] TOP 1 AS seed
WITH seed
SHORTEST_PATHS seed TO [42] LABEL cites AS route
WITH route
RETURN path(route) AS nodes, path_length(route) AS hops
```

Path 当前是一等 NodeId 序列；`path_length` 返回边数。集合阶段公开 `UNION/INTERSECT/EXCEPT`，结果按 NodeId 稳定归一化并保留确定性 provenance/score 合并语义。路径与集合均受节点数、字节和遍历预算约束。

## 历史 API 迁移

### db.query() → db.tql()

| 旧 `db.query()` 写法 | TQL 等价写法 | 新增能力 |
|---|---|---|
| `MATCH (n {name: "alice"}) RETURN n` | 完全相同 | — |
| `MATCH (n {id: 42}) RETURN n` | 完全相同 | O(1) 短路 |
| `MATCH (a)-[:knows]->(b) RETURN b` | 完全相同 | — |
| `WHERE b.age < 27` | 完全相同 | — |
| `WHERE a AND (b OR c)` | 完全相同 | +NOT 支持 |
| — | `(a)-[:knows\|likes]->(b)` | **多标签边** |
| — | `(a)-[:knows*1..3]->(b)` | **可变长路径** |
| — | `ORDER BY b.age DESC` | **排序** |
| — | `LIMIT 10 OFFSET 5` | **分页** |
| — | `WHERE b MATCHES {$op}` | **混合谓词** |

### db.filter_where() → db.tql()

| 旧 `Filter` 枚举 | TQL FIND 写法 |
|---|---|
| `Filter::Eq("name", json!("Alice"))` | `FIND {name: "Alice"}` |
| `Filter::Gt("age", 18.0)` | `FIND {age: {$gt: 18}}` |
| `Filter::In("role", vec![...])` | `FIND {role: {$in: [...]}}` |
| `Filter::And(vec![...])` | `FIND {a: x, b: y}` |
| `Filter::Or(vec![...])` | `FIND {$or: [{...}, {...}]}` |
| `Filter::Exists("f", true)` | `FIND {f: {$exists: true}}` |
| `Filter::Size("arr", 3)` | `FIND {arr: {$size: 3}}` |
| `Filter::All("tags", vec![...])` | `FIND {tags: {$all: [...]}}` |
| `Filter::TypeMatch("f", "object")` | `FIND {f: {$type: "object"}}` |

---

## 形式语法与内部架构

以下 EBNF 仅展示顶层骨架；WITH 图算法阶段和 Mongo 文档过滤的完整细节以 Parser 为准。

```ebnf
Query       := Entry (WHERE Predicate)? (RankClause)? RETURN ReturnClause
               (ORDER BY OrderList)? (LIMIT Int)? (OFFSET Int)?

Entry       := MatchEntry | FindEntry | SearchEntry

MatchEntry  := MATCH Pattern
FindEntry   := FIND DocFilter
SearchEntry := SEARCH VECTOR '[' NumList ']' TOP Int (ExpandClause)?

Pattern     := NodePat (EdgePat NodePat)*
NodePat     := '(' Ident? ('{' DocBody '}')? ')'
EdgePat     := '-[' (':' LabelList)? ('*' Int '..' Int)? ']->'
LabelList   := Ident ('|' Ident)*

DocFilter   := '{' DocBody '}'
DocBody     := (LogicOp | FieldEntry) (',' (LogicOp | FieldEntry))*
LogicOp     := ('$and' | '$or') ':' '[' DocFilter (',' DocFilter)* ']'
FieldEntry  := FieldName ':' (Value | OpObject)
OpObject    := '{' '$op' ':' Value (',' '$op' ':' Value)* '}'

RankClause   := RANK Ident BY VECTOR '[' NumList ']' TOP Int
ExpandClause := EXPAND (OUTGOING | INCOMING | BOTH)?
                '[' (':' LabelList)? '*' Int '..' Int ']'

Predicate   := PredOr
PredOr      := PredAnd (OR PredAnd)*
PredAnd     := PredAtom (AND PredAtom)*
PredAtom    := NOT PredAtom
             | '(' Predicate ')'
             | DocFilter
             | Ident MATCHES DocFilter
             | Ident '.' Ident CompOp Expr

CompOp      := '==' | '!=' | '>' | '>=' | '<' | '<='
Expr        := Ident '.' Ident | Literal
Literal     := Int | Float | String | Bool | null

ReturnClause := '*' | Ident (',' Ident)*
OrderList    := OrderExpr (',' OrderExpr)*
OrderExpr    := Expr (ASC | DESC)?
```

---

### 内部架构

TQL 由七个协作模块组成，延续项目既有的模块化拆分：

| 模块 | 文件 | 行数 | 职责 |
|------|------|------|------|
| **AST** | `query/tql_ast.rs` | 查询、管线、表达式、聚合、Path |
| **Lexer** | `query/tql_lexer.rs` | Token、参数、位置诊断 |
| **Parser** | `query/tql_parser.rs` | 递归下降、作用域与语义验证 |
| **Cascades** | `query/cascades.rs` | Memo、物理候选、成本与预算 |
| **Pipeline** | `query/pipeline.rs` | NodeSet 与图/集合/路径算子 |
| **Executor** | `query/tql_executor.rs` | 一等值、聚合和结果投影 |
| **Prepared** | `query/tql_prepared.rs` | 严格参数绑定 |

### 执行流程

```text
TQL 文本
  → TqlLexer
  → TqlParser / TqlQuery AST
  → Cascades Memo（统计、成本、预算与确定性 tie-break）
  → PipelineOperator（索引访问、NodeSet、图、路径、集合与迭代）
  → 表达式/聚合/排序/分页
  → TqlValueResult（Node、标量、Path、List、Null）
```

`FIND` 可选择主键、Hash/Ordered/Composite/Bitmap、索引交集或 Fast Tags + 精确校验；`SEARCH` 可选择可用的向量访问路径与 exact fallback；`MATCH`、图算法和路径阶段复用 `.gidx`/内存图目录。Cascades 是确定性、有界、统计感知且成本驱动的优化器，不宣称穷举意义上的数学全局最优。

---

## 当前边界

### TQL 与认知检索 API 的关系

TQL 和 `search_advanced()` 共用底层数据、QuIVer、图与预算设施，但提供不同语义：

| 维度 | `search_advanced()` | TQL Pipeline |
|---|---|---|
| 主要目标 | 固定工业 RAG 检索管线 | 任意三模算子组合 |
| 向量访问 | QuIVer / exact fallback | Cascades 选择向量源与重排 |
| 图能力 | SA-PPR 扩散 | EXPAND、路径、PageRank/WCC/Leiden 等 |
| 文本/认知 | AC+BM25、FISTA、DPP、Hook | 通过 TQL 算子与属性/向量/图组合 |
| 结果 | SearchHit | Node/标量/Path/List/Null 绑定行 |

两者长期共存：固定低开销 RAG 流程优先使用 `search*`，需要跨阶段优化、聚合、路径或图算法组合时使用 TQL。

### 当前限制

| 限制 | 说明 | 计划 |
|------|------|------|
| MATCH 模式方向 | MATCH 边模式仍以显式有向模式为主；SEARCH EXPAND 已支持 OUTGOING/INCOMING/BOTH | 后续扩展 |
| RANK score 投影 | GraphFirst score 仅参与排序，不作为 RETURN 字段暴露 | 后续扩展 |
| 子查询组合 | 不支持将任意 SEARCH 结果作为 MATCH 子查询输入 | 后续扩展 |
| GraphFirst 大集合 ANN | RANK 对合法 anchor 集合执行精确评分，不切换 bitmap-filtered ANN | 按真实需求评估 |

TQL 已支持聚合、OPTIONAL MATCH 与 DML；后续工作聚焦子查询组合、更多 MATCH 方向表达式和可选的排名分数投影。
