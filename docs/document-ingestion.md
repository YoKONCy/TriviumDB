# TriviumDB 文档解析与分批发布

> 本文说明当前开发版的自动解析、原文证据映射、外部解析接入和持久化候选版本。文档查询与结构模型见[文档来源与结构](documents.md)。

---

## 目录

- [解析范围](#解析范围)
- [解析与接入](#解析与接入)
- [原文证据](#原文证据)
- [外部解析接入](#外部解析接入)
- [分批发布协议](#分批发布协议)
- [配额与存储](#配额与存储)
- [故障与恢复](#故障与恢复)
- [公共错误](#公共错误)

---

## 解析范围

`DocumentParseInput` 包含 `source_id`、`source_revision`、UTF-8 `content`，以及可选的 `format`、`media_type`、`config`。`format` 默认 `auto`，支持 `plain`、`markdown`、`html`、`json` 和 `jsonl`。选择顺序为显式格式、已知媒体类型、来源键扩展名，普通文本作为默认格式。媒体类型忽略大小写及分号后的参数，PDF 与 Office 类型通过外部解析结果接入。

| 格式 | 解析正文与逻辑块 |
| --- | --- |
| 纯文本 | 移除 UTF-8 BOM，默认统一 CRLF／CR 为 LF，按空行形成段落 |
| Markdown | 提取正文、标题、代码、列表项与表格单元，标题组织层级 section，解析行内实体与格式文本 |
| HTML | 按词法顺序提取文本、块级边界及实体，忽略 script、style、template 内容 |
| JSON | 按源文件字段顺序提取解码后的键、字符串和标量，保留 record、field、value 块 |
| JSONL | 每个非空行形成独立 record，并按 JSON 规则提取内容 |

Markdown 使用 `pulldown-cmark` 的带字节范围事件。HTML 使用 `html5gum` 的带来源范围词法事件，块层级按实际标签开闭组织。HTML 输入由宿主提供 UTF-8 字符串，浏览器 DOM 树修正和版面信息由上层解析适配器提供。[Markdown 解析器](https://docs.rs/pulldown-cmark/latest/pulldown_cmark/struct.Parser.html)、[HTML 词法解析器](https://docs.rs/html5gum/0.9.1/html5gum/)

`config` 的默认值如下。

| 配置 | 默认值 | 作用 |
| --- | --- | --- |
| `max_input_bytes` | 64 MiB | 输入 UTF-8 字节数 |
| `max_output_bytes` | 64 MiB | 解析正文 UTF-8 字节数 |
| `max_blocks` | 100,000 | 逻辑块数量 |
| `max_mappings` | 1,000,000 | 来源映射条目数量 |
| `max_block_bytes` | 65,536 | 超长叶块按 UTF-8 边界拆分，支持 4 至 1,048,576 |
| `include_lines` | `false` | 为叶块添加非空行级子块 |
| `normalize_newlines` | `true` | 纯文本换行标准化 |

字节数、块数和映射数还受上表默认值对应的格式硬上限约束。零额度按字面执行。`parser_id` 包含内置解析版本和完整配置摘要，相同输入内容与配置生成稳定的块键。

---

## 解析与接入

Rust 的 `Database::parse_document(&input)` 返回 `ParsedDocument`。`ingest_document(&input, root, representations, expected_revision)` 串联解析和发布。宿主可以先解析并生成向量，再调用 `put_parsed_document(ParsedDocumentWrite, expected_revision)`。

Python 使用 `document_request`，Node 使用 `documentRequest`。二者共享以下 JSON 协议，所有节点 ID 使用十进制字符串。

```python
import json
import triviumdb

db = triviumdb.TriviumDB("notes.tdb", dim=2)

def request(command):
    return json.loads(db.document_request(json.dumps(command, ensure_ascii=False)))

input_document = {
    "source_id": "vault://notes/cat.md",
    "source_revision": "file-1",
    "content": "# 猫\n\n猫喜欢休息。\n",
    "config": {"include_lines": True},
}
parsed = request({"operation": "parse", "input": input_document})
snapshot = request({
    "operation": "put_parsed",
    "document": {
        "parsed": parsed,
        "root": {"id": "10", "vector": [1, 0], "payload": {"title": "猫"}},
        "representations": [],
    },
    "expected_revision": None,
})
db.close()
```

`ParsedDocument` 保存 `text`、解析正文 `content_hash`、来源 `source`、逻辑 `blocks` 和三项来源身份。内置解析的块只有结构信息。`representations` 按块 `key` 关联宿主提供的真实 `DocumentNode`，每个节点遵守数据库的 dtype、维度和全库 ID 唯一性。

整篇文本检索使用 `level: "document"`。块级文本、向量与混合检索使用具有节点表示的块。`operation: "ingest"` 接受 `input`、`root`、可选 `representations` 和 `expected_revision`，适用于宿主已经确定表示所对应块键的接入流程。

> **更新提醒**　替换使用此前读取的数据库 `revision` 作为 `expected_revision`。来源文件版本填入 `source_revision`，同一根节点保留稳定 `source_id`。

---

## 原文证据

解析文档的快照使用结构版本 2。`content` 保存解析正文，块范围、查询位置和 `DocumentReference` 均采用该正文的 UTF-8 半开字节位置。来源字段如下。

| 字段 | 含义 |
| --- | --- |
| `source.media_type` | 来源媒体类型，内置解析采用所选格式的规范值，外部结果使用宿主声明 |
| `source.content_hash` | 来源文本 UTF-8 字节的 SHA-256 |
| `source.original_text` | 与解析正文不同的原始文本，相同时使用 `null` |
| `source.artifact_hash` | 可选的 PDF、Office、图片等原始二进制摘要，由宿主提供 |
| `source.mappings` | 按解析位置顺序完整连续覆盖正文的映射 |

每条映射包含 `text_start`、`text_end`、`source_start`、`source_end`、`kind` 和 `location`。`exact` 对应相同文本，可对任意子范围做一对一映射。`cover` 返回涵盖转换内容的完整来源范围，适用于实体、转义和文本标准化。`synthetic` 将新增分隔符映射到零宽来源锚点。来源范围可以重叠或重排。

`resolve`、检索证据和上下文证据返回 `source_spans`。每项携带解析范围、来源范围、来源摘要、原文、来源行列和位置元数据。HTML 实体等转换可覆盖同一词法文本片段。调用方根据 `kind` 展示精度，行列按对应文本的一基 Unicode 标量计算，CRLF 计一个换行。

> **预算提醒**　来源片段、位置元数据和字符串转义计入证据响应额度。整篇正文作为命中证据时，需要相应提高 `limits.max_return_bytes`。默认额度为 1 MiB。

---

## 外部解析接入

PDF、Office 和 OCR 由宿主解析器生成同一 `ParsedDocument` 后调用 `put_parsed`。宿主提供稳定的 `parser_id`、来源文本与摘要、完整映射、父级在前的块清单，以及可选的二进制 `artifact_hash`。

`location` 支持 `page`、`paragraph` 和 `region`。页码从 1 开始。区域使用左上角原点和整数定点坐标，包含 `x`、`y`、`width`、`height`、`scale` 和 `unit`，实际坐标为对应整数除以 `scale`。`unit` 支持 `point`、`pixel`、`normalized`。缩放因子及宽高须为正，归一化区域位于单位平面内。

```json
{"page":2,"paragraph":"p-7","region":{"x":100,"y":200,"width":300,"height":400,"scale":1000,"unit":"normalized"}}
```

Core 在创建候选版本前校验解析摘要、来源摘要、UTF-8 范围、映射覆盖、块层级及位置元数据。二进制材料由宿主存储，宿主同时维护 `artifact_hash` 与实际制品的对应关系。

---

## 分批发布协议

Rust 提供 `begin_document_upload`、`append_document_upload`、`document_upload_status`、`list_document_uploads`、`publish_document_upload` 和 `abort_document_upload`。JSON 操作如下。

| 操作 | 请求字段 | 成功结果 |
| --- | --- | --- |
| `begin_upload` | `upload` | 上传状态 |
| `append_upload` | `upload_id`、`batch` | 上传状态 |
| `upload_status` | `upload_id` | 上传状态 |
| `list_uploads` | 可选 `document_id` | 按上传身份排序的状态数组 |
| `publish_upload` | `upload_id` | 完整文档快照 |
| `abort_upload` | `upload_id` | `{"aborted":true}` |

`upload` 包含调用方 UUID `upload_id`、`source_id`、`source_revision`、`parser_id`、完整解析正文的 `content_hash`、根节点 `root` 和可选 `expected_revision`。解析来源还提供 `source` 描述，包含 `media_type`、`content_hash`、可选 `artifact_hash` 与 `has_original_text`。

```json
{
  "operation":"begin_upload",
  "upload":{
    "upload_id":"ae01c329-a7e3-4f8c-88bd-7cfab66f26da",
    "source_id":"vault://notes/cat.txt",
    "source_revision":"file-2",
    "parser_id":"host-parser-v1",
    "content_hash":"<完整解析正文的sha256摘要>",
    "root":{"id":"10","vector":[1,0]},
    "expected_revision":"<当前文档修订>"
  }
}
```

`batch.sequence` 从 0 连续递增。批次可包含 `content`、`original_text`、`mappings`、`blocks` 中的任意组合，范围统一使用完整文本的位置。正文和原文分别按批次拼接，映射与块分别按批次连接。每个字符串批次都须独立满足 UTF-8 边界。

```json
{
  "operation":"append_upload",
  "upload_id":"ae01c329-a7e3-4f8c-88bd-7cfab66f26da",
  "batch":{"sequence":0,"content":"猫喜欢休息。","blocks":[
    {"key":"p-1","kind":"paragraph","start_byte":0,"end_byte":18}
  ]}
}
```

创建和追加使用请求内容摘要判定重试。相同上传身份及相同创建内容返回已有状态。已确认序号的相同批次返回已有确认，相同序号的不同内容返回冲突。状态保留独立的数据库文档修订、`next_sequence`、正文与原文字节数、块数、映射数、`state` 和 `visible`。

候选版本通过独立事实存储持久化，查询继续读取当前已发布文档。`publish_upload` 校验完整摘要、结构、来源映射、节点占用及根节点预期修订，通过一个事务安装根引用、检索表示、结构边和发布状态。竞争候选按同一根修订决定发布结果。相同已发布上传再次发布返回同一可见快照。

成功替换时回收该根节点此前的已发布归档。终止入口用于候选或已失去根引用的归档。删除根节点时回收该根的归档与候选。普通根节点 Payload 编辑导致归档失去引用时，可以通过列表及 `visible` 核对并终止该归档。

---

## 配额与存储

| 范围 | 上限 |
| --- | --- |
| 单文档解析正文 | 64 MiB |
| 单文档原始文本 | 64 MiB |
| 单文档逻辑块 | 100,000 |
| 单文档来源映射 | 1,000,000 |
| 单上传批次 | 4,096 |
| 单库待发布候选 | 128 |
| 单库候选与已发布归档合计 | 1,000,000 |
| 创建输入及单批次的公共 JSON 编码 | 16 MiB |
| 持久化批次记录 JSON 编码 | 32 MiB |
| 普通节点 Payload | 8 MiB |

内置接入以 256 KiB UTF-8 正文片段、每批 1,024 条映射及有界结构批次执行。手工 `put_document` 在正文或输入 JSON 编码超过 4 MiB，或块数超过 4,096 时使用同一分批路径。解析接入统一采用分批路径。

当前开发 `.tdb` 为 v13，WAL 为 v6。归档块随主文件和既有文件组原子发布。v12 检查点及 v5 命名图 WAL 使用独立历史布局读取，Writer 按既有流程升级。归档包含正文、清单、映射及候选表示，发布后释放候选根节点和向量副本。当前归档事实采用内存拥有型记录，打开、读取和最终发布均受实例内存与共享工作区准入约束。

> **资源提醒**　分批协议提供持久续传与可见版本的事务发布。最终发布需要组装完整快照及节点事务，宿主按文档大小和表示数量配置内存额度。超长表示块受普通 Payload 上限约束，可使用内置叶块拆分或宿主分块。

---

## 故障与恢复

确认点沿 `Config.sync_mode` 的 WAL 持久化契约执行。请求返回的序号与摘要用于重试核对。关闭和重新打开保留候选及已确认批次，`upload_status` 返回后续序号。

最终发布前的验证拒绝保留候选和当前可见版本。WAL 同步或应用故障沿 `write.phase`、`write.outcome` 返回。结果未知时重新打开，按原上传身份查询状态、核对可见修订，再恢复追加或发布。共享请求取消通过 Core 控制点传播，已提交的批次保留其持久化事实。

检查点记录发布序号水位，覆盖已发布归档的旧 WAL 重放保持幂等。ReadOnly／Immutable 可以解析、读取上传状态和查询已发布文档，并遵守物理零写。快照随文件组保存候选与归档。维度迁移前完成或终止候选，已发布归档随迁移目标保存。

> **接入提醒**　一键接入在中途失败时可能留下已确认候选，可按根节点调用 `list_uploads` 核对。需要应用级恢复身份时，由宿主生成并保存 `upload_id`，使用显式分批协议。

---

## 公共错误

| 错误码 | 条件与字段 |
| --- | --- |
| `TDB_DOCUMENT_UPLOAD_NOT_FOUND` | 上传身份不存在，`details.upload_id` 保留身份 |
| `TDB_DOCUMENT_UPLOAD_CONFLICT` | 创建内容、批次序号、摘要或状态冲突，`details.field` 与参数定位保留字段 |
| `TDB_DOCUMENT_REVISION_MISMATCH` | 根节点预期修订冲突，保留文档 ID 和预期／实际修订 |
| `TDB_INVALID_INPUT` | 来源范围、逻辑块、解析内容或配置无效 |
| `TDB_NODE_ALREADY_EXISTS` | 新表示的 ID 已被占用 |
| `QueueFull` | 文档数量、字节、批次或查询额度超限，或共享工作区准入拒绝 |
| `TDB_READ_ONLY` | 只读句柄请求修改候选或发布事实 |
| `TDB_CORRUPTED_FILE` | 持久记录、归档指针或文档事实损坏 |

其他维度、向量、取消、关闭与 I/O 错误沿既有统一错误契约返回。完整字段说明见[错误契约](error-contracts.md)。
