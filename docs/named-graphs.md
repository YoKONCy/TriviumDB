# 命名图、共享成员与独立边

> 本文说明 `0.9.1` 的命名图事实、显式图作用域、修订保护和持久化契约。版本记录见[变更说明](changes/0.9.1.md)。

---

## 目录

- [数据身份](#数据身份)
- [创建和批量编辑](#创建和批量编辑)
- [作用域与遍历](#作用域与遍历)
- [删除与修订](#删除与修订)
- [预算和存储](#预算和存储)
- [SDK 与验收](#sdk-与验收)

---

## 数据身份

命名图以调用方提供的 `graph_id` 标识，长度为1至1024字节，使用可打印UTF-8文本。每个图保存自己的 Metadata、成员集合、边集合和修订令牌。图本身作为独立存储事实保存。

节点继续使用全库 `NodeId`，向量和 Payload 随节点共享。一个节点可以同时属于词级图、事件图及其他命名图。成员关系允许孤立节点，边的两个端点均须在所属图的成员集合中。

命名边的唯一键为 `(graph_id, source_id, target_id, label)`。同一图、同一键的 `upsert_edge` 编辑替换权重及 Metadata。不同图保存的同键边分别具有独立内容。自环和不同标签的平行边均可保存，权重接受有限 `f32`。

`link`、`upsert_edge`、`get_edges` 及省略作用域的图查询访问默认图。默认图的节点域包含全库节点，边键采用 `(src, dst, label)`。TQL 通过 `GRAPH` 前缀选择命名图和显式默认图。显式作用域响应中，默认边的 `graph_id` 为 `null`，命名边携带所属身份。字符串 `"default"` 可以用作普通命名图身份。

> **宿主提醒**　命名图提供逻辑数据组织，宿主负责用户鉴权及可访问图的选择。TQL 的查询、管线和规划说明见 [GRAPH 作用域](tql-reference.md#graph--命名图作用域)。

---

## 创建和批量编辑

Rust 使用 `named_graph` 模块的 `NamedGraphWrite`、`NamedGraphEdge`、`NamedGraphSnapshot` 和 `NamedGraphEdit`。

| Core 入口 | 语义 |
| --- | --- |
| `put_named_graph(graph, expected_revision)` | `None` 创建，已有图按预期修订完整替换 |
| `edit_named_graph(graph_id, expected_revision, edits)` | 按顺序编辑成员、边及 Metadata，整批提交一次修订 |
| `get_named_graph(graph_id, limits)` | 返回拥有数据的完整快照，身份缺失返回 `None` |
| `list_named_graphs(limits)` | 按图身份排序，返回修订、成员数和边数 |
| `delete_named_graph(graph_id, expected_revision)` | 按预期修订删除该图 |
| `named_graph_scope(scope, limits)` | 返回显式选择图的联合快照 |
| `traverse_named_graph(scope, start, max_depth, limits)` | 对所选图执行出边广度遍历 |

Python 调用 `graph_request(request_json)`，Node 调用 `graphRequest(requestJson)`，两者复用 `sdk::named_graphs::request`。JSON 中的所有节点ID均采用规范十进制字符串。下面的输入要求节点11、22已存在。

> **数值提醒**
>
> Node普通数值API沿既有安全整数范围处理节点ID，完整u64身份由文档／图JSON协议传输。Metadata中的大整数由宿主选择支持完整整数的JSON解析方式。

```json
{
  "operation": "put",
  "graph": {
    "graph_id": "terms",
    "metadata": {"kind": "word"},
    "members": ["11", "22"],
    "edges": [
      {"source_id": "11", "target_id": "22", "label": "related", "weight": 0.5, "metadata": {"source": "note.md"}}
    ]
  }
}
```

响应包含 `schema_version=1`、`graph_id`、`revision`、`metadata`、`members` 和 `edges`。成员按ID升序排列，边按起点、终点、标签排序。完整替换请求中的 `graph` 使用上述写入字段，另将响应修订放入 `expected_revision`。

```json
{
  "operation": "edit",
  "graph_id": "terms",
  "expected_revision": "8b34f182-5c2e-4ed2-823c-09a4e0be8946:1",
  "edits": [
    {"operation": "add_member", "node_id": "33"},
    {"operation": "upsert_edge", "edge": {"source_id": "22", "target_id": "33", "label": "next", "weight": 1}},
    {"operation": "remove_edge", "source_id": "11", "target_id": "22", "label": "related"},
    {"operation": "set_metadata", "metadata": {"kind": "word", "generation": 2}}
  ]
}
```

`remove_member` 编辑会同时清理该图中与目标成员关联的边。重复添加已有成员、移除缺失成员及移除缺失边按集合操作执行，成功编辑仍产生下一修订。完整写入中的重复成员和重复边键返回参数错误。

Rust `TxBuilder::put_named_graph` 返回准备好的图快照，`delete_named_graph` 添加条件删除。它们可以和节点插入、删除、默认图边操作放在同一事务中，提交时按操作顺序核对成员存在性和图修订。整批失败保留提交前事实。

---

## 作用域与遍历

```json
{
  "operation": "scope",
  "scope": {
    "graph_ids": ["terms", "events"],
    "include_default": false,
    "expected_revisions": {}
  }
}
```

`scope.graph_ids` 显式指定命名图，`include_default=true` 纳入默认图。空作用域和重复图身份返回参数错误。`expected_revisions` 可固定所选命名图的修订，失配返回图修订错误。

联合响应包含 `revisions`、去重后的 `members` 和保留图身份的 `edges`。边按起点、终点、标签、所属图身份排序，默认图在同键命名边之前。独立边在联合结果中分别保留。

`traverse` 请求增加十进制字符串 `start` 和整数 `max_depth`。零深度返回起点及空边集合，起点须在作用域内。结果保留所有已展开节点的出边，深度边界上的节点作为终点保留。显式联合允许沿共享节点连接多个图。所有响应数据来自同一事实视图，返回后由调用方拥有。

---

## 删除与修订

图修订由创建时的UUIDv4身份和递增计数组成。删除后重建图会产生新的创建身份，旧修订持续返回冲突。计数耗尽时在写入WAL前拒绝变更。

普通节点 Payload 或向量更新对所有引用该节点的图可见，图结构修订保持原值。全库节点删除会在同一提交中清理所有命名图的成员关系和关联边，并推进受影响图的修订。单图删除和成员移除保留全库节点，以及其他图的关系。

`TDB_GRAPH_REVISION_MISMATCH` 的 `details` 提供 `graph_id`、`expected` 和 `actual`，缺失状态用 `"absent"` 表示。编辑不存在的图、选择不存在的作用域身份返回 `TDB_GRAPH_NOT_FOUND`。缺失成员节点返回 `TDB_NODE_NOT_FOUND`，非法边端点、重复键及未知JSON字段返回 `TDB_INVALID_INPUT`。

> **冲突处理提醒**
>
> 读取当前快照，合并调用方编辑，再携带新修订提交。写入错误的 `write.phase` 和 `write.outcome` 描述实际完成点。结果为 `unknown` 时释放失效实例并重新打开，核对图修订与成员、边内容后决定后续操作。

---

## 预算和存储

单库最多保存10000个命名图，每图最多100万个成员和100万条边。每个图写入结构的JSON编码上限为64 MiB，请求JSON采用相同上限。成员和边在复制、事务投影及恢复前使用内存准入，命名图事实堆内存纳入实例估算。

`NamedGraphLimits` 默认提供 `max_graphs=10000`、`max_nodes=1000000`、`max_edges=1000000`、`max_return_bytes=67108864`。零值表示零额度，超额使用 `QueueFull` 和对应 `named_graph_*` 资源字段。联合预算按所读取的成员关系和边累加，返回节点再按ID去重。遍历深度上限为1000，执行检查点观察当前请求取消。

命名图当前采用内存成员集合及有序边数组。一次替换或编辑将目标图的完整快照写入WAL，读取作用域物化相应成员和边。实际资源消耗受图规模、Metadata及输出图身份长度影响。

遍历先物化并核对完整所选范围，再按深度展开和裁剪结果，返回字节额度同时约束该中间范围视图。缺失图的JSON空值响应计入4字节返回额度。作用域输出在复制边Metadata及图身份之前核对完整编码长度。

`.tdb` 从 v12 起在唯一约束块之后保存命名图块，包含逐记录长度、整体长度、CRC32 和 `NGRF` 标识。当前 v13 在其后保存文档批次块与回执块。WAL 从 v5 起追加命名图记录，当前 v6 继续使用相同命名图布局和事务封条。图事实随主文件参与已有原子发布，快照、恢复和维度迁移保留图身份、结构修订及完整成员和边。

历史 `.tdb` v11及WAL v4具有独立样例和解码布局。Writer按现有打开及检查点流程升级，旧图事实归入默认图。ReadOnly／Immutable保留物理零写契约，恢复由Writer执行。

> **格式提醒**
>
> v12检查点和v5日志要求支持这些格式的内核。升级前保留完整文件组备份，回退使用该备份。

---

## SDK 与验收

Core拥有图事实、修订、成员约束、事务预检、WAL恢复及作用域语义。SDK负责JSON解码、语言借用、错误传递和响应编码。Rust Reader提供读取、列举、作用域及遍历，Writer复用Core写入口。CLI／Server宿主可调用相同Core入口。

共享样例位于 `tests/contracts/named_graph_cases.json`。Core及实际Python／Node制品核对独立边、共享节点、完整结果、修订冲突和跨重开内容。内核用例覆盖Rom／Mmap、三种dtype、节点级联删除、混合事务、受控取消、应用故障、检查点与WAL重叠恢复、只读文件组字节保持及Windows关闭后文件移动。格式用例覆盖独立编码、截断、逐字节损坏和恶意长度／数量。
