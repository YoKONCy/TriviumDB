# TriviumDB 文档来源与结构

> 本文说明 `0.9.1` 的文档身份、修订、UTF-8 证据范围、结构清单与原子更新。文档事实由 Rust Core 管理，Python 和 Node 通过共享 SDK JSON 适配调用。版本说明见[变更说明](changes/0.9.1.md)。

---

## 目录

- [数据模型](#数据模型)
- [来源与位置](#来源与位置)
- [自动解析与分批接入](#自动解析与分批接入)
- [创建与替换](#创建与替换)
- [读取与引用](#读取与引用)
- [分组文本检索](#分组文本检索)
- [字段检索与评分](#字段检索与评分)
- [元数据与集合成员过滤](#元数据与集合成员过滤)
- [文档向量与混合召回](#文档向量与混合召回)
- [位置查询与词项证据](#位置查询与词项证据)
- [上下文展开](#上下文展开)
- [缓存与查询预算](#缓存与查询预算)
- [删除与恢复](#删除与恢复)
- [SDK 与公共错误](#sdk-与公共错误)
- [当前实现范围](#当前实现范围)

---

## 数据模型

文档以根节点的 `NodeId` 为数据库内身份，根节点拥有一个真实向量和调用方提供的 Payload。`source_id` 记录宿主的稳定来源键，`source_revision` 记录宿主的输入版本，`parser_id` 标识解析配置。根节点 Payload 的 `_tdb_document` 保存内联快照或版本化归档引用，`data` 保存调用方的 Payload。归档中的正文、清单与来源映射参与同一主文件发布。

每次成功创建或替换都会生成独立的文档 `revision`。该修订随普通 Payload 进入同一 WAL 事务，检查点与重开后保持原值。其他文档和普通节点的更新沿各自身份执行。

`blocks` 按父级在前的顺序保存逻辑结构。块字段如下。

| 字段 | 含义 |
| --- | --- |
| `key` | 文档内唯一且非空的块身份 |
| `parent` | 父块的 `key`，根层块使用 `null` 或省略 |
| `kind` | 非空的粒度名称，例如 `section`、`paragraph`、`line` |
| `start_byte`、`end_byte` | 同一来源修订中的 UTF-8 半开字节范围 |
| `node` | 可选检索表示，包含全库唯一的节点 ID、向量和 Payload |

纯目录块可以省略 `node`，保存在结构清单中。具有 `node` 的块使用普通三模节点，Payload 保存来源修订、块身份、范围、`text` 与调用方的 `data`。向量由宿主提供，遵守数据库当前的 dtype 和维度。

结构图使用 `tdb.document.contains` 标签。每个具有节点表示的块连接到最近的具有节点表示的祖先，根节点作为默认祖先。边 metadata 保存文档修订、逻辑父块、块身份和清单顺序。完整逻辑父子关系保存在 `blocks` 中。

> **编辑提醒**　文档根节点及片段的完整 Payload 和向量通过文档替换入口统一维护，普通业务关系通过图 API 维护。文档 API 管理 `_tdb_document` 和 `tdb.document.contains`，读取时校验来源摘要、片段修订与结构关系，发现失配时返回错误。跨入口编辑由宿主协调预期版本。

---

## 来源与位置

手工 `DocumentWrite` 的正文按输入字符串原样保存，包括 CRLF、LF、Unicode 和空白。解析接入的正文为解析后的 `text`，来源文本与映射保存于 `source`。`content_hash` 为快照正文 UTF-8 字节的 SHA-256，格式为 `sha256:<小写十六进制>`。数据库内修订承担并发控制，内容摘要用于核对正文。

范围满足 `0 <= start_byte <= end_byte <= content.len()`，两端落在 UTF-8 字符边界。子块范围包含在父块范围内。父级在前的清单顺序同时确定层级的无环关系，块之间可以保留重叠窗口。

引用中的行列为一基 Unicode 标量位置。CRLF 计一个换行，Tab 计一列。字节范围是持久引用的权威位置，行列由对应修订的正文计算。

内置或宿主解析器将材料映射到解析正文。宿主使用 `parser_id` 和 `source_revision` 记录解析规则及输入版本。文档 API 中的 `source_id` 由根节点绑定，替换时保持相同值。多个根节点采用相同来源键时，宿主管理各自的用途和映射。

---

## 自动解析与分批接入

`parse_document` 支持纯文本、Markdown、HTML、JSON 和 JSONL，生成逻辑块与原文映射。`put_parsed_document` 接收统一解析结果，`ingest_document` 串联解析和发布。PDF、Office 和 OCR 由宿主提供相同外部解析结果。证据的 `source_spans` 携带来源文本、位置、页码及区域。

显式上传通过 begin、append、status、publish 和 abort 入口保存候选版本。批次经过 WAL 持久化，查询读取当前已发布版本，最终事务更新根引用、片段节点和结构边。正文和原文各支持 64 MiB，普通表示节点 Payload 继续采用 8 MiB 上限。协议、代码示例、来源精度与资源条件见[文档解析与分批发布](document-ingestion.md)。

---

## 创建与替换

Rust 使用 `document::DocumentWrite<T>`、`DocumentNode<T>` 和 `DocumentBlock<T>`，调用 `Database::put_document(document, expected_revision)`。`expected_revision=None` 表达首次创建，更新时传入最近读取的文档修订。

Python 的 `document_request(request_json)`、Node 的 `documentRequest(requestJson)` 和 Rust 的 `sdk::documents::request(&mut database, request_json)` 使用相同 JSON 协议。以下请求创建文档及一个检索片段。

```json
{
  "operation": "put",
  "document": {
    "source_id": "vault://notes/example.md",
    "source_revision": "file-v1",
    "parser_id": "plain-utf8-v1",
    "content": "alpha\nbeta\n",
    "root": {"id": "10", "vector": [1, 0], "payload": {"title": "Example"}},
    "blocks": [
      {"key": "section", "kind": "section", "start_byte": 0, "end_byte": 11},
      {
        "key": "first",
        "parent": "section",
        "kind": "line",
        "start_byte": 0,
        "end_byte": 5,
        "node": {"id": "11", "vector": [0, 1], "payload": {"role": "evidence"}}
      }
    ]
  }
}
```

成功返回 `DocumentSnapshot`，包括 `schema_version`、`document_id`、`revision`、来源字段、`content_hash`、正文和完整结构清单。返回块中的 `node_id` 为十进制字符串或 `null`。JSON 中的节点 ID 统一采用规范十进制字符串，向量分量采用目标 dtype 的数值语义，f16 使用浮点数值，u64 保留完整整数。

替换请求提供完整的新文档和 `expected_revision`。Core 检查来源身份、范围、节点归属、向量和预期修订，再通过一个普通事务更新根节点、片段、失效节点及结构边。复用节点保留其业务边，向量位模式相同时保留现有向量。移除的表示节点按普通节点删除契约清理相连边。

> **容量提醒**　单批文档替换受普通事务、单节点 Payload 上限、数据库内存额度和共享工作区准入约束。正文及结构清单位于根 Payload，当前单节点编码上限为 8 MiB。宿主先完成解析和向量计算，再进入短提交。

---

## 读取与引用

Rust 使用 `get_document(document_id)` 和 `resolve_document_reference(reference)`。`DatabaseReader` 提供相同读取方法。JSON 请求如下。

```json
{"operation":"get","document_id":"10"}
```

根节点缺失时返回 `null`。读取在同一事实读锁内核对来源摘要、结构范围、片段内容、修订和完整结构边，然后返回拥有型快照。

引用同时指定文档、修订和范围。

```json
{
  "operation": "resolve",
  "reference": {
    "document_id": "10",
    "revision": "<此前读取的文档修订>",
    "start_byte": 0,
    "end_byte": 5
  }
}
```

成功返回 `reference`、来源身份、来源版本、解析器身份、正文摘要、实际 `text` 以及起止行列。快照与证据拥有其返回字符串，生命周期独立于数据库文件资源。

文档更新后，旧引用返回 `TDB_DOCUMENT_REVISION_MISMATCH`，详情包含十进制 `document_id`、`expected` 和 `actual`。文档删除后，引用解析沿节点缺失契约返回 `TDB_NODE_NOT_FOUND`。

---

## 分组文本检索

Rust 调用 `search_documents(&DocumentSearchQuery)`，`DatabaseReader` 提供相同入口。Python、Node 和 Rust SDK 使用 `search` 请求。

```json
{
  "operation": "search",
  "query": {
    "text": "alpha",
    "level": "block",
    "source_ids": ["vault://notes/example.md"],
    "kinds": ["line"],
    "max_documents": 10,
    "hits_per_document": 2,
    "use_cache": true
  }
}
```

`level` 默认为 `block`，检索具有节点表示的块正文。`document` 检索完整来源正文，其命中 `kind` 为 `document`，`block_key` 为 `null`。两种级别分别维护 BM25 语料统计。纯目录块通过结构读取与上下文展开访问。空文本或分词后为空的表示保留在文档事实中，BM25 语料数量统计实际具有词项的表示。

评分复用现有 TextIndex 的分词规则和 Okapi BM25，参数固定为 `k1=1.2`、`b=0.75`。查询词去重后按任一词匹配累加分数。`document_ids`、`source_ids`、`kinds` 各自表示集合内匹配，多种过滤同时满足，省略或空集合表示该字段覆盖完整语料。过滤沿用完整语料的词频和平均长度统计。

所有符合过滤的命中先按文档根身份分组，再截取每组的 `hits_per_document` 个片段和全局的 `max_documents` 组。组分数取最高片段分数，组按分数降序、文档 ID 升序排列，组内按分数降序、节点 ID 升序排列。多个文档根使用相同 `source_id` 时，各根保留自己的组和修订。

返回 `DocumentSearchResult` 包含 `level`、`corpus_documents`、`corpus_units`、`matched_units` 和 `groups`。`corpus_documents` 为登记的文档根数，`corpus_units` 为当前级别的非空词项表示数，`matched_units` 为过滤后、截断前的表示数。每组返回文档及来源身份、修订、组分数和 `hits`。命中包含节点 ID、块身份、粒度、BM25 分数和完整 `evidence`，其引用可直接用于原文解析和上下文展开。

`expected_revisions` 可以提供形如 `{"10":"<已知修订>"}` 的文档修订表。Core 在评分前核对表内全部文档，失配返回 `TDB_DOCUMENT_REVISION_MISMATCH`，缺失返回 `TDB_NODE_NOT_FOUND`。检索、校验与证据物化共享同一事实读视图。

---

## 字段检索与评分

`fields` 在 `level="block"` 下选择具有节点表示的块类型，并为每种类型设置正的有限 `weight`，省略权重时采用 `1`。例如，解析器可以将标题标记为 `kind="title"`，正文标记为 `kind="body"`。字段名与块类型精确匹配，且同一请求中的字段名唯一。空 `fields` 使用当前层级的整体语料。

```json
{
  "operation": "search",
  "query": {
    "text": "memory checkpoint",
    "level": "block",
    "fields": [
      {"kind": "title", "weight": 3},
      {"kind": "body", "weight": 1}
    ],
    "max_documents": 10,
    "hits_per_document": 2
  }
}
```

每个字段分别维护完整语料中的词频、非空表示数、片段词项长度与平均长度，片段得分为该字段 BM25 乘以权重。来源、元数据及成员过滤后继续采用完整字段语料统计。一个块对应一个字段，每个文档组按最高加权片段分数排序。组内采用加权分数和节点 ID 确定顺序。

`fields` 与 `kinds` 同时指定时取交集。未知字段返回空命中，并在 `field_corpora` 中报告零语料。纯逻辑块通过结构与上下文访问。字段选取覆盖具有节点表示的块，片段之间的重叠范围分别参与各自的统计。

启用字段检索时，响应增加按 `kind` 排序的 `field_corpora`，每项包含 `kind`、`weight`、`corpus_units` 与 `average_length`。响应顶层 `corpus_units` 继续统计当前检索级别的全部非空表示。短语、邻近及词项位置以单个块为边界，证据始终映射到该块在原文中的范围。

> **调用提醒**　字段检索采用 `block` 层级。`document` 层级检索完整正文，携带非空 `fields` 时返回 `TDB_INVALID_INPUT`。权重导致分数溢出时，同样返回结构化参数错误。

---

## 元数据与集合成员过滤

`document_filter` 对根节点 Payload 的 `data` 求值，`block_filter` 对当前候选片段的 `data` 求值。两者采用公共 JSON 过滤语法，字段名定位 `data` 中的顶层键，包括包含点号的完整键名。省略或 `null` 表示省略该过滤条件，空对象表示满足全部条件。

`members` 提供数组成员过滤。每项通过 `scope="document"` 或 `"block"` 选择根或片段的 `data`，`field` 指定非空顶层字段，`mode="any"` 要求命中任意查询成员，`mode="all"` 要求包含全部查询成员。多个成员条件、普通元数据过滤及来源限制共同取交集。

```json
{
  "operation": "search",
  "query": {
    "text": "checkpoint",
    "document_filter": {"vault_id": "notes", "updated_at": {"$after": "2026-01-01T00:00:00Z"}},
    "block_filter": {"state": "ready"},
    "members": [
      {"scope": "document", "field": "entities", "mode": "all", "values": ["agent", "storage"]},
      {"scope": "block", "field": "tags", "mode": "any", "values": ["decision", "evidence"]}
    ]
  }
}
```

| 输入情况 | 成员匹配语义 |
| --- | --- |
| 字符串、布尔、数值及 `null` | 按 JSON 标量类型与值精确匹配，整数与浮点保留各自表示 |
| 重复数组元素或查询成员 | 按集合解释，重复次数对结果无影响 |
| 缺失字段、`null`、标量字段 | 成员条件匹配失败 |
| 数组内对象或数组元素 | 标量成员索引跳过这些复合元素，查询成员要求标量 |
| 空 `any` 查询 | 匹配结果为空 |
| 空 `all` 查询 | 匹配该字段为数组的记录，包括空数组 |
| 空 `members` | 省略成员过滤 |

现有 `$in` 比较字段完整值与查询值，`$all` 求数组包含关系。专用成员入口通过派生倒排计算交并集，随后重新读取对应 `data`，复核完整成员与元数据谓词。文档条件向所属片段传播，片段条件仅作用于当前候选片段。`document` 层级接受文档条件；片段条件在该层级返回 `TDB_INVALID_INPUT` 并定位具体字段。

候选交集与事实复核先于文本候选计量、来源分组和截断。成员 posting 与文本 posting 累计进入 `max_postings`，过滤中间集合受 `max_filter_candidates` 约束。通用谓词扫描由节点与 Payload 访问预算约束，成员事实复核同时计入成员值预算。

> **数值提醒**　JSON 入口保留完整整数与浮点表示。JavaScript 宿主可直接构造请求 JSON，以传递超过安全整数范围的元数据数值及需要保留的浮点表示。文档和节点 ID 按现有协议使用十进制字符串。

---

## 文档向量与混合召回

Rust Core 和 Reader 提供 `search_documents_vector(query, vector, options)` 与 `search_documents_hybrid(query, vector, options)`，沿用 `DocumentSearchQuery` 的来源、层级、过滤、分组、修订和预算。Python／Node 的既有文档 JSON 入口分别使用 `search_vector` 与 `search_hybrid`。

```json
{
  "operation": "search_vector",
  "query": {
    "level": "block",
    "document_filter": {"vault_id": "notes"},
    "members": [{"scope": "block", "field": "tags", "mode": "any", "values": ["evidence"]}],
    "max_documents": 10,
    "hits_per_document": 2
  },
  "vector": [1, 0],
  "options": {"strategy": "auto", "candidate_limit": 4096}
}
```

向量维度与 dtype 沿用数据库契约，浮点使用余弦相似度，`u64` 使用汉明匹配位数。JSON 数值按目标类型转换，完整整数位模式通过 SDK 保留。`level="document"` 使用根向量，`block` 使用具有节点表示的块向量，空正文的表示同样参与向量召回。向量入口中的 `query.text` 采用空字符串默认值。文本字段权重、词项明细与位置条件用于文本或混合入口。

`DocumentVectorOptions` 与 JSON `options` 的字段如下。

| 字段 | 默认值 | 含义 |
| --- | --- | --- |
| `strategy` | `auto` | 已有浮点 QuIVer 索引时使用 ANN，其余采用精确评分。`exact` 指定精确评分，`ann` 要求已就绪的浮点 QuIVer 索引 |
| `rerank` | `original` | `original` 使用原始向量评分，`compressed` 使用已配置 QuIVer 运行时的编码与查询精度 |
| `min_score` | `null` | 可选有限向量分数阈值，按大于等于保留 |
| `initial_candidates` | 64 | ANN 初始候选宽度基数 |
| `candidate_limit` | 4096 | 单轮 ANN 结果候选池的宽度上限 |
| `ef_search` | 128 | ANN 导航宽度下限，实际至少覆盖当前候选宽度，并按索引节点数约束 |

三个候选参数须为正数，`initial_candidates <= candidate_limit`。`auto` 采用当前读视图中已就绪的索引，查询通过现有来源事实派生文档缓存。QuIVer 建图沿既有内核维护及自动构建流程执行。显式 `ann` 的前置条件失败时返回 `TDB_INVALID_INPUT`，字段定位到相应 `strategy`。

精确路径对全部合格表示按原生 dtype 评分，随后按文档分组和截断。ANN 复用 QuIVer BQ 查询时评分入口，导航访问现有图，精排通过候选集合筛选并按需读取向量。初始宽度取 `max(initial_candidates, max_documents * hits_per_document)`，随后受候选上限及索引节点数约束。每轮按分数和节点 ID 排序，目标文档组数为请求组数与合格来源数的较小值，组数不足时宽度倍增。最终分组采用停止轮的候选。

`rerank="compressed"` 要求浮点数据库、已就绪的 QuIVer 索引与已配置的压缩运行时，策略可选 `auto` 或 `ann`。参数错误返回 `TDB_INVALID_INPUT`，压缩配置定位为 `options.rerank`，混合入口使用 `options.vector.rerank`。显式 `ann` 的索引前置条件定位到对应 `strategy`。编码、查询精度、驻留和字节缓存预算从 QuIVer 运行时读取，文档请求独立选择 `rerank`，`use_for_ann` 控制普通 ANN 入口。

Python 可先配置运行时，再通过文档入口选择压缩重排。Node 使用相同 JSON 字段，通过 `quiverRequest` 与 `documentRequest` 调用。Rust 使用 `configure_quiver` 和 `DocumentVectorOptions::rerank`。

```python
import json

db.quiver_request(json.dumps({"operation": "configure", "options": {
    "encoding": "centered_int4", "query_precision": "fp32",
    "residency": "ssd", "cache_bytes": 64 * 1024 * 1024,
    "io_mode": "auto", "io_depth": 4, "use_for_ann": False
}}))
result = json.loads(db.document_request(json.dumps({
    "operation": "search_vector",
    "query": {"max_documents": 10, "hits_per_document": 2},
    "vector": query_vector,
    "options": {"strategy": "ann", "rerank": "compressed"}
})))
```

压缩路径在同一事实读视图中复用来源过滤、组覆盖扩展和证据生成，借用数据库主 QuIVer 路由，并通过压缩 Reader 批量预取和评分。Hot 与 SSD 使用相同编码行与评分逻辑，查询精度可选 FP32、INT8、INT4。压缩向量分数为近似余弦相似度，`min_score` 和混合排名作用于实际返回的分数。向量更新后的派生代际由后续压缩查询刷新。

```json
{
  "operation": "search_hybrid",
  "query": {
    "text": "memory checkpoint",
    "fields": [{"kind": "title", "weight": 3}, {"kind": "body"}],
    "max_documents": 10,
    "hits_per_document": 2
  },
  "vector": [1, 0],
  "options": {
    "vector": {"strategy": "exact", "min_score": 0},
    "text_weight": 1
  }
}
```

混合入口使用 `DocumentHybridOptions`，其中 `vector` 包含上述向量配置，`text_weight` 默认 `1`，要求有限正数。字段权重先作用于文本 BM25 排名，字段集合同时约束参与召回的块类型。元数据、成员及来源条件统一约束两路候选，短语或有序邻近条件进一步约束完整候选集合。向量路按 `min_score` 筛选，文本路保留自己的合格命中。`include_matches` 为实际具有词项匹配的入选表示提供窗口证据。

融合复用现有管线的 RRF，排名按原始分数降序、节点 ID 升序确定，排名从一开始。两路均有结果时，每个表示的最终得分为 `60 / (60 + vector_rank) + text_weight * 60 / (60 + text_rank)`，每路只对实际召回的成员贡献。单独向量路保留原始向量分数，单独文本路使用加权 RRF 排名分数。计算时先求平滑比例，再乘文本权重，支持极大有限权重的数值范围。

结果继续使用文档组及原文证据，每个向量／混合命中增加 `scores`，包括可空的 `vector_score`、`vector_rank`、`text_score` 与 `text_rank`。缺少一路召回时，对应值为 `null`。响应增加 `retrieval`。

| 字段 | 含义 |
| --- | --- |
| `hybrid` | 是否使用混合入口 |
| `strategy`、`approximate` | 实际执行的 `exact`／`ann` 路由及 ANN 近似范围 |
| `rerank` | 实际使用的 `original`／`compressed` 评分来源 |
| `compression` | 压缩模式的实际 `encoding`、`query_precision`、`residency`，原始模式为 `null` |
| `eligible_units` | 完成来源、元数据、成员及位置约束后的向量候选表示数 |
| `vector_candidates` | 达到向量分数阈值的最终向量候选数 |
| `vector_scores` | 精确评分或 ANN 精排候选的累计准入次数，多轮访问累计 |
| `ann_visits`、`ann_rounds` | ANN 控制检查点访问量及已执行轮数 |
| `stop` | `exact_complete`、`groups_satisfied`、`candidate_limit` 或 `index_exhausted` |

`groups_satisfied` 记录当前候选覆盖目标组数，ANN 结果同时携带 `approximate=true`。`candidate_limit` 记录宽度达到配置上限，`index_exhausted` 记录宽度覆盖索引或合格候选为空。硬资源预算超额统一返回结构化错误。文档组仍取最高命中分数，组与组内排序沿既有规则执行。

压缩候选先完成过滤和评分预算准入，再进行批量取数，缓存命中与 Hot 驻留采用相同计数。导航控制检查点计入 `max_ann_visits`，候选准入计入 `max_vector_scores`。外层文档工作区与 Reader 内存合并核对数据库额度，共享 Runtime 中的工作区额度只预留一次。预算拒绝、取消及读取失败返回完整错误，临时额度和行租约沿请求退出归还。

顶层 `corpus_units` 继续统计相应层级的非空文本表示，向量候选规模通过 `retrieval.eligible_units` 解释。空正文、纯向量命中及混合命中都返回拥有型来源证据，可直接使用现有引用解析和上下文展开。

> **调用提醒**　示例使用二维向量，调用时按实际数据库维度提供向量。JavaScript 宿主通过原始请求 JSON 传递完整 `u64` 分量。ANN 导航范围及候选上限通过响应字段解释，调用方按自身召回需求配置。

> **精度提醒**　压缩重排的分数与召回质量随编码、查询精度和业务数据变化。业务阈值按实际配置验证，运行时重开后重新配置。配置与制品要求见 [QuIVer 压缩重排](quiver.md#可选压缩重排与查询精度)。

---

## 位置查询与词项证据

`DocumentSearchQuery` 的 `match_mode` 默认为 `any`，按查询词项的并集进行 BM25 召回。`phrase` 要求词项连续出现，`ordered_near` 允许有序词项之间累计跳过至多 `max_gap` 个词项。`max_gap` 默认零，正值用于 `ordered_near`。重复查询词在位置条件中要求不同的出现位置，BM25 评分继续按去重查询词计算。

```json
{
  "operation": "search",
  "query": {
    "text": "alpha beta",
    "level": "document",
    "match_mode": "ordered_near",
    "max_gap": 1,
    "include_matches": true,
    "analyzer_id": "tdb-text-ascii-cjk2-v1"
  }
}
```

`include_matches` 默认 `false`，开启后每个命中增加 `matches`。每项包含完整 `evidence` 和有序 `terms`。`evidence.reference` 指向实际词项窗口的文档修订及原文字节范围；原有命中的 `evidence` 继续表示整个检索单位。`terms` 字段如下。

| 字段 | 含义 |
| --- | --- |
| `term` | 大小写归一化后的分析词项 |
| `position` | 当前整篇或块内从零开始的词项序号，包含中间跳过词项的位置 |
| `start_byte`、`end_byte` | 该词项在来源正文中的 UTF-8 半开字节范围 |

每个起始词项返回最早结束的完整匹配窗口；终点相同时选择词序最靠前的链。正文 `a b b` 对查询 `a b` 的有序邻近结果取第一个 `b`。多个起始词项产生的重叠窗口分别返回，宿主可以交给上下文入口合并。`any` 的命中明细列出每个匹配词项的实际出现，重复查询词合并计入。输出按起始词项序号确定排列。

位置条件作用于当前检索单位。`document` 在完整正文上解释词序，`block` 在每块正文上分别从零计数。块的原文字节范围确定其边界，输出字节位置统一映射回来源正文。位置条件先验证完整候选，再执行来源分组与组内截断。

分析器身份为 `tdb-text-ascii-cjk2-v1`，Rust 常量位于 `index::text::TEXT_ANALYZER_ID`。搜索结果提供 `analyzer_id`，请求可以提供相同字段核对身份。当前规则先做 Unicode 小写转换，再将 ASCII 字母和数字组成词项；CJK、日文和韩文范围的连续字符使用重叠双字词项，单字符段生成单字词项。其他字符作为分隔符。词项记录原始字符范围，覆盖 `İ` 的小写展开以及 `K` 等大小写转换前后的字节宽度差异。

双字字符范围分别为 `U+3400–U+9FFF`、`U+3040–U+30FF` 和 `U+AC00–U+D7AF`。

短语按分析后的词序及重叠关系解释，标点、空格和换行作为分隔符。中文连续双字词项保留共享字符的重叠关系。查询 `甲乙丙` 对应重叠的 `甲乙`、`乙丙`；查询 `甲乙 乙丙` 对应分开的两次出现。有序邻近的跳过额度应用于分开的词项之间，共享字符的双字链按相邻重叠词项连接。

精确证据可以直接传入 `resolve`，也可以与命中的 `block_key` 一起传入 `context`。更新后的旧引用按文档修订契约拒绝。批量证据的行列在同一正文上一次扫描生成，CRLF、Unicode 标量和 Tab 沿统一位置规则解释。

> **资源提醒**　位置明细受访问次数、匹配窗口数和输出预算约束。一次查询按完整结果或结构化错误交付。大量重复词及长查询链需要相应额度。

---

## 上下文展开

Rust 调用 `document_context(&DocumentContextQuery)`，Reader 和共享 SDK 提供相同语义。一个请求可以传入多个文档的种子。

```json
{
  "operation": "context",
  "query": {
    "seeds": [
      {
        "reference": {
          "document_id": "10",
          "revision": "<检索返回的修订>",
          "start_byte": 0,
          "end_byte": 5
        },
        "block_key": "first"
      }
    ],
    "before": 1,
    "after": 1,
    "ancestor_depth": 1
  }
}
```

提供 `block_key` 时，种子范围须包含在该块中。Core 取完整种子块，并按 `before`、`after` 展开同一逻辑父级下的相邻块，默认各一个。相邻块按起点、终点、清单序号排序，粒度可以不同。窗口包含所选同级块之间的原始正文。

`ancestor_depth` 默认零，逐层展开种子块的逻辑祖先，覆盖纯目录块。到达逻辑顶层后再展开一层得到整篇正文。省略 `block_key` 时只取引用给出的精确范围，邻居和祖先参数适用于具有块身份的种子。

每个文档的重叠或相接范围合并成一个证据窗口。结果按文档 ID 升序排列，`block_keys` 按清单顺序列出实际选中的块，`windows` 按字节范围升序返回完整证据。任一来源修订、块身份、UTF-8 范围或预算验证失败时，整个请求返回结构化错误。

---

## 缓存与查询预算

文档文本索引由已保存的来源正文和片段事实派生，首次检索时在内存中构建。节点、Payload、向量或图事实变更后缓存失效，下次检索重新核对来源、片段及结构边。重开后的首次检索采用恢复完成的事实。缓存估算纳入 `estimated_memory()` 和 `index_memory_stats().hot_bytes`，临时构建及结果物化申请共享工作区。

`use_cache=false` 使用同样的构建、评分和排序语义，查询完成后回收派生索引。Rust 的 `clear_document_index()` 和 JSON 的 `{"operation":"clear_index"}` 回收缓存，JSON 成功结果为 `{"cleared":true}`。只读实例可以使用这些内存操作，文件组遵守物理零写契约。

现有 `index_text`、关键词和 `rebuild_text_index` 继续维护独立登记的通用文本索引。文档检索从文档事实恢复自身语料，其缓存与通用 `.text` 文件分别管理。`load_text_index` 控制通用索引的加载，文档查询沿自身入口构建派生语料。本批磁盘布局沿用现有文件组。

两种查询都接受可选 `limits`，省略字段采用下列默认值。零值表示零额度，超额返回现有 `QueueFull` 类别及 `resource`、`requested`、`used`、`limit`。

| 字段 | 默认值 | 计量范围 |
| --- | --- | --- |
| `max_scan_nodes` | 1,000,000 | 事实扫描及完整性校验中的节点访问次数，重复访问累计 |
| `max_payload_bytes` | 256 MiB | 上述访问累计的编码 Payload 字节 |
| `max_text_bytes` | 64 MiB | 构建语料时输入的正文与表示文本字节总和 |
| `max_edges` | 1,000,000 | 校验文档节点时访问的完整邻接表边数，包含业务边 |
| `max_postings` | 1,000,000 | 搜索的文本及成员 posting 累计访问，或上下文的邻居／祖先访问次数 |
| `max_candidates` | 100,000 | 搜索过滤后的表示数，或上下文种子数加目标文档全部逻辑块数 |
| `max_return_bytes` | 1 MiB | 累计输出的保守编码计量，包含正文、转义字符串和结构预留 |
| `max_positions` | 4,000,000 | 位置条件或明细启用时，文档与块语料中累计保存的词项出现数，冷热缓存采用相同逻辑计量 |
| `max_query_terms` | 256 | 位置条件或命中明细请求中的分析词项数，重复查询词累计 |
| `max_position_visits` | 1,000,000 | 位置条件验证、后继表、窗口重建和明细枚举的累计工作量 |
| `max_matches` | 10,000 | 所选命中中实际生成的精确窗口总数，明细开启时计量 |
| `max_member_values` | 1,000,000 | 启用成员条件时，完整派生成员索引的数组元素数，加事实复核的数组元素与去重查询成员访问数；复合数组元素计入构建和复核工作量 |
| `max_filter_candidates` | 100,000 | 每个成员条件、作用域交集、通过文档条件的根集合，以及最终过滤候选集合的大小上限 |
| `max_vector_scores` | 1,000,000 | 文档精确向量评分或 ANN 精排取数累计次数 |
| `max_ann_visits` | 1,000,000 | ANN 上层下降、beam、候选整理及精排控制检查点访问，多轮累计 |

搜索在冷、热缓存下均计入同一派生语料的逻辑构建访问成本，再累计选中文档的证据校验成本。过滤条件在语料完整性校验后应用，预算超额按错误返回。`max_return_bytes` 使用结构预留加实际 JSON 字符串编码长度计量，宿主应为元数据与转义开销保留额度。数据库内存配额和运行时工作区是独立的准入约束。

位置条件或明细首次启用时，在同一派生内存索引中建立位置posting，按词项增长检查预算。普通词频缓存升级时先归还旧缓存额度，再按当前事实完整构建。查询在分配后继表前核对 posting 数与工作区，匹配过程持续观察取消。普通 `any` 请求按词项并集生成候选，位置条件与明细启用时采用位置和查询词项上限。上下文查询按自身展开工作量计量，位置索引的专用计数用于搜索入口。

字段语料与成员倒排同样按请求需要构建。缓存缺少本次能力时，先回收旧缓存，再从当前事实构建本次请求所需的能力组合。成员倒排与位置表纳入索引内存账本；过滤集合、谓词编译、原始事实解码及证据输出使用工作区准入。冷、热查询采用相同的逻辑构建与复核计数。更新、删除和恢复后由事实重建派生状态。

向量和混合请求复用同一来源读视图、文档缓存和完整性校验。合格候选先经统一过滤，再进入原生向量评分。ANN 工作区在查询前按索引节点数和最大导航宽度准入，每轮拥有的访问位图、候选堆和精排缓冲随该轮结束释放。取消或预算失败传播到请求线程，工作区随请求归还。候选分数、两路排名、融合并集及输出元数据均纳入候选或响应额度。

> **资源提醒**　首次文档检索需要扫描活跃节点并验证已登记文档，当前预算按整库事实计量。大库使用显式额度控制构建与物化，宿主可以主动清理缓存。

---

## 删除与恢复

Rust 调用 `delete_document(document_id, expected_revision)`。JSON 请求如下。

```json
{"operation":"delete","document_id":"10","expected_revision":"<此前读取的文档修订>"}
```

删除核对文档修订，并以一个事务删除根节点和文档拥有的检索表示。JSON 成功结果为 `{"deleted":true}`。纯目录信息随根节点清单删除，其他业务节点保留，指向已删除节点的边遵守普通删除契约。

正文、清单、片段和结构边复用已有 WAL 与文件组发布协议。WAL 提交前的验证拒绝保持旧文档。同步失败及结果未知按照内核的实际写入阶段返回，上层通过重新打开和读取文档核对完整结果。后续检查点继续保存同一修订。

> **重试提醒**　检查 `write.phase` 和 `write.outcome`。修订冲突后读取当前快照并重新准备修改，结果未知时核对持久事实和来源版本，再决定后续操作。

---

## SDK 与公共错误

SDK 负责 JSON、字符串 ID、请求借用和错误传递，Core 负责文档事实、修订检查、事务、WAL 和生命周期。Python 与 Node 调用期间采用各自现有请求准入机制。Rust 可通过 `SharedDatabase::try_read` 或 `try_write` 安排共享访问。

| 情况 | 返回契约 |
| --- | --- |
| 来源修订冲突或旧引用 | `TDB_DOCUMENT_REVISION_MISMATCH`，保留文档及预期／实际修订 |
| 根节点或表示节点已被占用 | 沿 `TDB_NODE_ALREADY_EXISTS` 或目标类型诊断返回 |
| 范围、父级、身份或 JSON 非法 | `TDB_INVALID_INPUT` 及已知 `target.field` |
| 向量维度或数值非法 | 沿公共向量校验错误返回 |
| 已保存的来源、片段或边失配 | `TDB_CORRUPTED_FILE` |
| 只读写入、关闭、取消、预算或 I/O | 沿对应公共错误及实际阶段返回 |

JSON 请求在解码前预留工作区。已经识别的写命令在正文转换前核对写权限和 WAL 准入。解码前拒绝保留已知请求信息，写入之后的响应编码失败提供 `response/committed`。内部事务缓冲的消费状态由 SDK 隐去。

---

## 当前实现范围

当前实现提供完整文档来源、逻辑结构、可选块表示、修订保护、原文范围引用、原子替换／删除、分组 BM25、块类型字段权重、文档／片段元数据与集合成员过滤、文档级向量与 RRF 混合分组、位置倒排、短语／有序邻近、词项证据和版本绑定的上下文窗口。现有向量、属性及图查询可以访问已建立的表示节点及边。

文档节点可以同时加入多个[命名图](named-graphs.md)，共享成员通过全库NodeId引用，业务边在所属图内独立保存。文档结构边使用默认图。嵌入宿主通过 Core 和共享 SDK 使用解析、外部映射与分批发布。TQL 的 `DOCUMENTS` 来源复用文本、向量和混合召回，逐命中携带来源与证据，并接入过滤、图扩展、聚合及重排。语法和元数据字段见 [TQL 文档来源](tql-reference.md#documents--文档来源)。
