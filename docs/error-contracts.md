# 公共错误与请求完成契约

> 本文记录以 `0.9.0` 为基线的当前开发版本，覆盖公共字段、查询控制、写入结果与传输完成点。错误定义见 `src/error.rs`，SDK 职责见[内核 SDK 与存储内核](kernel-sdk.md)。

---

## 目录

- [公共表示](#公共表示)
- [公开调用形状](#公开调用形状)
- [构造参数值](#构造参数值)
- [普通 CRUD 参数与成员定位](#普通-crud-参数与成员定位)
- [检索、图与文本参数](#检索图与文本参数)
- [入口与字段目录](#入口与字段目录)
- [写入结果字段](#写入结果字段)
- [操作与输入字段](#操作与输入字段)
- [提交阶段与事实来源](#提交阶段与事实来源)
- [取消与期限](#取消与期限)
- [请求与传输完成点](#请求与传输完成点)
- [语言及传输边界](#语言及传输边界)
- [重试与兼容目录](#重试与兼容目录)
- [文档接入错误](#文档接入错误)
- [后续收敛](#后续收敛)

---

## 公共表示

| 字段 | 当前契约 |
| --- | --- |
| `schema_version` | 当前为 `1` |
| `code` | Core 稳定类别，具体详情与对应语言声明配套使用 |
| `message` | 面向调用方的诊断说明，项目上下文使用中英文双语 |
| `details` | 错误类别对应的结构化事实，空详情使用空对象 |
| `stage` | TQL 的 `lex`、`parse`、`bind`、`plan`、`execute` 阶段，由实际检查点提供 |
| `target.query_span` | 原始查询的 UTF-8 半开字节范围与一基 Unicode 标量行列；可与参数名同时提供 |
| `target.parameter` | Prepared 参数的原始名称，与传入映射中的键一致 |
| `target.field` | 已覆盖普通 API 校验的输入字段名，采用当前入口的参数命名 |
| `write` | 已覆盖写入口的失败阶段、结果及操作缓冲消费状态 |

`target` 根据错误来源提供查询位置、Prepared 参数名或普通 API 字段名。Prepared 参数名可与该引用的源码范围同时提供。JSON／Node 省略缺失的扩展字段，Python 使用 `None`。参数名中的点、斜杠、方括号及 Unicode 字符按原始键传递。Prepared 的缺参和多参分别按参数名排序，缺参检查先执行。

Rust `at_field()` 为错误附加普通字段定位，保留原始类别、详情、cause、已知阶段与写入结果。外层准入透过字段和 Prepared 包装识别既有写入结果。当前容量预留使用 `additional`；内核资源额度使用 `bytes`，Python／Node 的 MiB 接口使用 `mb`；运行期同步模式使用 `mode`。Node 的显式安全整数转换使用对应的公开参数名。

参数转换与绑定定位保留原始 `code`、`details` 和错误链。Rust 可通过 `TriviumError::underlying()` 取得原始错误变体，Python 按原始变体保留异常继承关系。Prepared 参数 JSON 容器值沿用 `TDB_INVALID_INPUT`，向量槽位中的非数值沿用 `TDB_QUERY_EXECUTION`，两者增加 `stage=bind` 与参数定位。

TQL 的可靠解析诊断使用真实 token 范围，输入结束及 token 预算边界使用零长度锚点。数据库准备的 Prepared 参数错误同时提供名称与首个真实引用的源码范围，包含 `$` 前缀。字符串、注释和文档过滤操作符按解析语义区分。额外参数、独立 `TqlParamValue::from_named_json` 转换及手工 AST 准备沿各自已知信息提供参数名。Core 保存与附加位置，SDK、绑定及传输层复用这些事实。错误分类、cause、详情和已知写入结果沿原链保留。

AST 构造来源覆盖查询、表达式、谓词、模式、各类管线与写入节点。作用域及列名检查使用 `plan`，按 AST 路径定位真实引用或冲突返回项。实际算子、投影、事务执行与运行期预算使用 `execute`。算子融合保留参与节点的最小包围范围，生成的子查询沿原入口追溯。Rust 的 `QuerySource` 保留原始变体，`underlying()` 提供统一的分类入口。手工 AST 的阶段按执行点提供，源码范围依据实际持有的来源信息提供。

字段与参数包装保留已有查询范围。公开目标可使用 `field` 与 `query_span`，或 `parameter` 与 `query_span` 的组合。查询阶段和写入阶段分别描述查询检查点与事务完成事实，`execute` 阶段的唯一约束拒绝仍提供 `validation/not_committed` 及实际缓冲消费状态。

Rust 与 Python 保留错误整数的完整精度。Node 与 HTTP 将超出 JavaScript 安全整数域的详情编码为十进制字符串。全部 Core 错误码与详情类型见 npm 公共声明 `ErrorDetailsByCode`，真实行为由共享契约与各语言实际制品验证。

> **处理提醒**：根据 `code`、`details` 与已提供的扩展字段处理错误。第三方与 OS 的原始 cause 用于诊断。

---

## 公开调用形状

Python 包依据原生公开签名，在值转换和方法执行之前检查四个公开构造器、实例方法、选定的魔术方法及模块函数的实参结构。调用错误保留 `TypeError` 继承，并附加 `TDB_INVALID_INPUT`、`target.field` 和 `details.argument_kind`。正常调用继续交给原生绑定，方法体和宿主协议中的原异常、cause 及已有属性沿原路径传播。类身份、签名、默认值和上下文管理入口继续保留。

| `argument_kind` | 定位规则 |
| --- | --- |
| `missing` | 签名顺序中的首个缺失必需参数 |
| `unknown_keyword` | 原始关键字按 Unicode 字符顺序排列后的首项 |
| `multiple_values` | 同一参数由位置和关键字重复提供，按签名顺序定位 |
| `positional_only` | 以关键字提供的位置专用参数，按签名顺序定位 |
| `too_many_positional` | `target.field=arguments` |
| `invalid_arguments` | 其余已识别的签名绑定拒绝，`target.field=arguments` |

Python 同次调用包含多个问题时，依次检查位置数量上限、位置专用／重复赋值、未知关键字和缺参。关键字内容中的 Unicode、NUL 和孤立 surrogate 在 Python 目标字段中完整保留。诊断使用实参数量、原始键名及占位值，随后才进入原生值转换。

Node 公共加载器按当前绑定契约检查必需位置参数，缺参使用 `TypeError`、`argument_kind=missing` 及对应 camelCase 字段。`undefined` 占据参数位置后继续按既有类型规则处理，额外位置参数与配置对象按各入口原规则处理。同步回调重入先于缺参返回 `TDB_REENTRANT_ACCESS`。

Python 构造器在原生入口收集实参后按 SDK 的公开签名元数据绑定和补齐默认值，随后执行原有值转换与内核打开。相同元数据提供四个原生类的 __signature__，兼容最低支持的 Python 3.9。Node 的数据库构造器在缺少 path 时提供相同的缺参类别，构造转发保留原生 prototype、instanceof、子类及 newTarget。

已登记写入口的调用形状拒绝提供 `validation/not_committed`。Python 事务缓冲、提交和回滚调用同时提供 `transaction_consumed=false`，此前操作保留；Node `commitTransaction` 同样报告尚未消费。读取与关闭类调用省略写入结果，TQL 调用形状错误省略 `stage`。

> **宿主边界**　错误 receiver、解释器在进入 callable 前执行 `*`／`**` 展开时产生的错误，以及 JavaScript 的非构造调用，遵循宿主运行时的异常契约。上述调用形状契约通过已安装 Python 包和 Node 公共加载器提供。

---

## 构造参数值

四个构造器的已覆盖值转换使用 `TDB_INVALID_INPUT` 和 `target.field`，`stage` 与 `write` 省略。Python 的内置提取错误保留 TypeError、OverflowError 或 UnicodeEncodeError 继承，自定义整数协议异常保留对象身份、cause 和已有诊断。枚举拒绝保留 ValueError 继承。Node 对已覆盖的配置字段检查原始数值、字符串和布尔值，属性 getter 的原异常沿既有路径传播。

| 入口 | Python 已覆盖字段 | Node 已覆盖字段 |
| --- | --- | --- |
| 数据库 | path、dim、dtype、sync_mode、load_text_index、auto_build_quiver、expected_nodes、memory_limit_mb、payload_cache_mb、payload_cache_entry_mb、access_mode、missing_index_policy、max_query_rows、row_overflow、runtime | path、options、dim、dtype、syncMode、storageMode、autoBuildQuiver、loadTextIndex、expectedNodes、memoryLimitMb、payloadCacheMb、payloadCacheEntryMb、accessMode、missingIndexPolicy、maxQueryRows、rowOverflow、runtime |
| 资源运行时 | memory_limit_mb、max_threads、max_active_tasks、maintenance_memory_limit_mb | options、memoryLimitMb、maxThreads、maxActiveTasks、maintenanceMemoryLimitMb |
| Console 会话 | options_json 的类型、编码与 JSON 配置 | optionsJson 的类型与 JSON 配置 |
| 请求作用域 | request_id、timeout_ms | requestId、timeoutMs |

MiB 换算在打开或登记资源前执行，超出当前语言表示范围时定位原输入字段。Node 运行时配额的字节数继续受 JavaScript 安全整数范围约束，数据库配置的字节换算按 usize 范围处理。Python 可选数量参数的 None 与 Node 配置字段的省略／undefined 使用既有默认值，Node 配置字段中的 null 继续按原类型边界拒绝。Node 会话构造的省略／null 配置使用默认会话。

存储内核拒绝零维度和超过 65536 的维度，字段为 dim，拒绝诊断直接记录输入与上限。资源内核的线程上限拒绝定位 max_threads，维护保护额度与总额冲突定位 maintenance_memory_limit。绑定按各自参数名映射字段及单位。打开失败后的请求与数据库配额由内核所有权释放，保留原始锁、维度身份、预算和 I/O 错误类别。

Node 的 options 按非数组对象校验。省略、undefined 和 null 使用默认配置，具名属性沿既有原型链读取，未知属性保持各入口的处理规则。数组、函数、布尔和字符串等非法配置返回 TDB_INVALID_INPUT 并定位 options；数据库旧数字位置构造使用既有 TDB_API_MIGRATION_REQUIRED 类别，附带 options 与可执行的新调用形式。资源运行时先前接受的原始值及数组配置在此入口统一按对象契约拒绝。

Python 的布尔参数复用 PyO3 提取，内置错误附加字段，宿主转换异常保留原对象。runtime 按原生 ResourceRuntime 类型提取，类型拒绝保留 TypeError 并定位 runtime。Node runtime 继续使用现有类型标记句柄校验，已知类型拒绝提供同一字段，getter 与 Proxy 的原异常及 cause 沿原路径传播。

数据库配置成员 runtime 的 null 在 Node 按类型拒绝，省略／undefined 使用默认运行时。Python 数据库及两端快照模块函数的可选 runtime 以 None／null 表示默认运行时。verify_snapshot／verifySnapshot 的句柄拒绝省略 write；restore_snapshot／restoreSnapshot 的已知句柄拒绝发生在目标操作之前，提供 validation/not_committed。宿主自行抛出的异常保留已有属性。

Node 构造字符串、普通 CRUD 字符串和 JSON 字符串使用严格 UTF-16 解码，孤立 surrogate 按输入错误拒绝。有效 Unicode 与嵌入 NUL 按原内容传递。Python 字符串沿原 UTF-8 提取规则处理。

---

## 普通 CRUD 参数与成员定位

十五个直接写入口覆盖插入、带 ID 插入与替换、两类批量插入、两类删除、Payload 替换、补丁、字段 CAS、向量更新，以及 link、upsert_edge、update_edge、unlink。Python 的 ID、数值协议、向量与 JSON 转换在数据库请求借用内完成。Node 的数组元素及 JSON 属性读取沿加载器的同库重入准入执行。

已知的绑定类型、表示范围与 JSON 转换拒绝提供 `TDB_INVALID_INPUT`、`target.field` 和 `write={phase:"validation",outcome:"not_committed"}`，省略事务消费字段。Python 保留相应 TypeError、ValueError、OverflowError、UnicodeEncodeError 或 IndexError 继承。业务约束和进入 WAL 后的结果由存储内核提供。

批量插入内部临时事务的 transaction_consumed 在公开错误中省略，原内核阶段、结果及 cause 保留。Node 自动 ID 分配在超出 JavaScript 安全整数范围时于提交前拒绝，提供 validation/not_committed。

顶层 target.field 保留当前公开参数名。嵌套成员采用 RFC 6901 JSON Pointer，数组下标从 0 开始，键中的 `~` 编码为 `~0`，`/` 编码为 `~1`。例如 `/vectors/1/0` 表示第二条向量的首个分量，`/payload/a~1b~0c/1` 表示键 `a/b~c` 下数组的第二项。Unicode、点号、方括号与 NUL 保留原内容。无法表示为有效 JSON 字符串的对象键定位到所属容器。

Python 单向量和 ID 集合沿既有非字符串 Sequence 协议接收，批量向量与 Payload 的外层容器沿既有 list 规则接收。长度提示异常沿 PyO3 既有规则忽略，实际迭代及元素提取决定内容。缓冲按实际元素增长，避免按宿主给出的巨大长度提示预留。Node 向量和批量容器继续要求 Array，读取时保留索引 getter 与 JSON 属性 getter 的原调用规则。

Python 批量外层在开始转换时固定下标范围，随后逐项读取。转换回调缩短向量或 Payload 列表导致后续成员缺失时，按对应成员定位拒绝整批，WAL 与既有事实保留；回调追加的外层成员留待后续调用处理。

> **宿主异常**　自定义整数、浮点、迭代、getitem、getter 和 Proxy 自行抛出的异常保留原对象、cause 及已有属性。同库重入由请求准入拒绝。Node 可选边参数的 null／undefined 与 Python None 按各自现有默认或保留值规则处理，Node upsertEdge 的 weight 继续为必需参数。

---

## 检索、图与文本参数

普通、分组、批量、精确、图优先、上下文、高级和混合检索共用参数转换的字段契约。Python 的浮点、布尔、向量、锚点及标签转换在读请求借用内执行。Node 读取原始数组、标量和配置属性，保留 getter 原异常及现有重入准入。已知的语言转换拒绝使用 TDB_INVALID_INPUT，读取错误省略 write。

顶层字段使用各语言公开参数名，例如 query_vector／queryVector、min_score／minScore、anchor_ids／anchorIds。嵌套成员采用 JSON Pointer，例如 `/query_vectors/1/0`、`/queryVectors/1/0`、`/labels/1`、`/payloadFilter/a~1b~0c/0`。配置中的已登记标量沿现有平面字段名提供定位。长度提示异常沿 Python 既有容错规则忽略，真实迭代决定内容，缓冲按实际元素增长。

Node 的本批文本、标签、字段和重建记录字符串严格解码 UTF-16，孤立 surrogate 按输入错误拒绝，有效 Unicode 和 NUL 按原内容传递。

检索配置的有限性、扩散偏置维度和 min_edge_weight 约束由存储内核验证，错误提供对应字段。批量查询在返回空集合前验证同一配置。混合入口的 hybrid_alpha／hybridAlpha 由共享 SDK 换算，非有限输入或换算溢出提供该字段。Rust 通过 underlying() 匹配原错误类别，并从 info() 读取字段。

Node 的 searchAdvanced／searchWithContext 配置和三个可达性入口的 options 使用非数组对象。省略或 null 使用默认配置，已登记属性沿原型链读取，未知配置属性保留原处理规则。配置成员中的 null 按相应成员类型拒绝。可选位置参数的 null 继续使用既有默认值，必需位置参数沿各自类型规则拒绝。searchBatch 的数组／类型拒绝发生在任务创建前，u64 数值范围与内核执行错误通过 Promise 返回，范围错误保留批次和分量位置。

文本维护覆盖 index_text／index_keyword、完整重建和构建入口。已知值转换拒绝提供 validation/not_committed，重入或借用争用提供 admission/not_committed，消费字段省略。完整重建记录包含 id、text 和可选 keywords，缺参、未知字段、类型和成员拒绝定位到 `/documents/0/id` 等路径。Python 记录接受声明中的 Mapping，先固定本批记录对象，再在写请求借用内物化映射和提取字段。Node 固定数组下标范围，校验自有可枚举记录键，声明字段沿既有属性读取路径提取。宿主异常和失败的整批转换保留原文本及索引事实。

indexed_lookup／substring_lookup 的数量、对象、字段和文本类型使用同一字段契约。数量的 1—1000000 范围、空等值条件及空子串字段／内容由内核判断，分别定位 max_results／maxResults、equalities、field 和 needle。

> **调用提醒**　宿主转换、迭代、映射和 getter 抛出的异常保留原对象、cause 与已有诊断。文本维护成功后的耐久性依照 flush 或可写关闭的检查点完成点。

---

## 入口与字段目录

命名图JSON入口 `graph_request`／`graphRequest`与Core图方法共享错误类别。`TDB_GRAPH_NOT_FOUND`包含 `graph_id`，`TDB_GRAPH_REVISION_MISMATCH`包含 `graph_id`、`expected`、`actual`，条件写入冲突定位 `expected_revision`。预算资源以 `named_graph_*`命名，JSON结构拒绝定位 `request_json`。Core保留节点缺失、只读、取消、恢复及实际写入完成点，字段与用法见[命名图契约](named-graphs.md)。

文档入口新增 `put_document`、`get_document`、`resolve_document_reference` 和 `delete_document`，SDK JSON 入口提供相同语义。修订拒绝使用 `TDB_DOCUMENT_REVISION_MISMATCH`，`details` 包含字符串 `document_id`、`expected` 和 `actual`。已知来源、范围、父级和向量拒绝定位到 `target.field`；JSON 结构拒绝定位到 `request_json`。写入遵守本表的阶段及结果字段，完整字段、节点归属和重试说明见[文档来源与结构](documents.md)。

文档分组检索与上下文使用同一修订错误，预期修订表定位到 `query.expected_revisions`，上下文旧引用定位到 `query.seeds.reference.revision`。查询预算采用现有 `QueueFull` 和结构化额度字段，`resource` 区分 `document_scan_nodes`、`document_payload_bytes`、`document_text_bytes`、`document_edges`、`document_postings`、`document_candidates`、`document_return_bytes`。读取拒绝保留查询类别，完整性失配返回 `TDB_CORRUPTED_FILE`。

文档位置查询增加 `document_positions`、`document_query_terms`、`document_position_visits`、`document_matches` 预算资源。分析器身份失配、适用模式之外的正 `max_gap`、位置请求分词后为空分别返回 `TDB_INVALID_INPUT`，定位到 `query.analyzer_id`、`query.max_gap`、`query.text`。JSON 的未知模式、类型与字段拒绝定位到 `request_json`。取消、内存准入、修订和完整性错误沿原公共类别传递。

下表登记当前维护的公共入口及参数族。Rust 使用类型系统表达语言类型，语义校验和执行错误由 Core 提供。Python 使用 snake_case，Node 使用 camelCase。普通字段使用公开参数名，集合成员使用 JSON Pointer。调用形状、默认值和精确类型分别由原生签名、Python 存根及 npm 公共声明维护。

| 入口 | 输入字段与边界 | 拒绝层与结果 |
| --- | --- | --- |
| Database、ResourceRuntime、ConsoleSession、RequestContext 构造 | 数据库配置见构造参数表，会话为 options_json，请求为 request_id、timeout_ms | SDK 表示转换后进入 Core 的维度、配额、策略和打开校验，构造失败省略 write |
| get、get_payload、contains、get_edges、get_incoming_edges、get_edge | id，或 src、dst、label | SDK 在请求范围内转换，Core 判定存在性和生命周期，读取省略 write |
| insert、insert_with_id、upsert_with_id、update_vector | vector、payload、id，Python update_vector 沿自身公开参数顺序 | SDK 校验语言表示及成员位置，Core 验证维度、有限值、身份和约束 |
| batch_insert、batch_insert_with_ids、delete_many_atomic | vectors、payloads、ids | 完整转换后进入 Core 原子事务，普通批次省略内部 builder 的消费字段 |
| delete、update_payload、patch_payload、compare_and_set_payload_field | id、payload、patch、field、expected、replacement | SDK 保留原宿主异常，Core 维护 CAS、唯一约束、WAL 和提交结果 |
| link、upsert_edge、update_edge、unlink | src、dst、label、weight、metadata | SDK 转换 ID、权重、JSON，可选值沿各端既有规则，Core 维护完整正反向边 |
| search、search_grouped、search_batch、search_exact、search_graph_first | query_vector、query_vectors、anchor_ids、top_k、recall_k、rerank_k、expand_depth、min_score、payload_filter、parallelism | SDK 转换及成员定位，Core 验证配置、预算、候选、评分和稳定排序，Node 批量任务按 Promise 完成 |
| search_with_context、search_advanced、search_hybrid | 上述参数及高级配置表中的浮点、布尔、标签、文本、偏置和边约束 | SDK 做语言表示和混合权重换算，Core 执行同一检索语义 |
| neighbors、reachable、reachable_detailed、query_subgraph | ID、深度、标签、方向、节点／边／结果／前沿预算 | SDK 提取公开字段，Core 验证遍历预算及方向语义 |
| leiden_cluster | Python 的 min_community_size、max_iterations、compute_centroids，Node config 中的 minCommunitySize、maxIterations、withCentroids | SDK 在请求范围内转换，省略配置使用原默认值，Core 继续使用既有聚类配置语义 |
| indexed_lookup、substring_lookup | equalities、max_results，或 field、needle、max_results | SDK 转换成员，Core 判定空条件及数量范围 |
| create_index、drop_index、create_ordered_index、drop_ordered_index、create_bitmap_index、drop_bitmap_index、create_ngram_index、drop_ngram_index、create_unique_index | field | SDK 使用严格字符串及字段诊断，Core 维护定义、预算、候选安装和发布 |
| create_composite_index、drop_composite_index、create_unique_composite_index | fields 及逐项成员 | Python 接受非字符串 Sequence，Node 接受 Array，完整转换后进入 Core 维护入口 |
| index_text、index_keyword、build_text_index、rebuild_text_index | id、text、keyword、documents 中的 id／text／keywords | SDK 转换完整记录，Core 维护正文、关键词、预算和索引安装 |
| Python Transaction 的 insert、insert_with_id、link、delete、unlink、update_payload、update_vector | vector、payload、id、src、dst、label、weight | 转换在事务对象借用内执行，已知转换拒绝为 validation/not_committed、transaction_consumed=false，此前缓冲保留 |
| Python Transaction 的 commit、rollback、上下文退出 | 活动 builder 与数据库请求借用 | SDK 准入拒绝保留 builder，Core 提交按真实阶段和消费事实返回，异常退出走回滚清理 |
| Node commitTransaction | operations，各项 type、id、src、dst、vector、payload、label、weight、metadata | SDK 转换及构造拒绝定位 /operations/0/vector/1 等路径，transaction_consumed=false，Core 提交后沿实际消费状态返回 |
| tql、tql_mut、prepare_tql、execute_prepared_tql | query、prepared、parameters | SDK 处理语言表示，Core 提供 lex／parse／bind／plan／execute 及原始查询范围 |
| set_sync_mode、set_memory_limit、reserve_nodes、set_auto_build_quiver、enable_auto_compaction | mode、mb、additional、enabled、interval_secs | SDK 提取和单位换算，Core 维护生命周期、WAL、资源额度及调度登记 |
| flush、compact、disable_auto_compaction、repair_graph_indexes、Rust QuIVer 构建及模式调整 | 无参或各 Rust 类型化配置 | Core 提供准入、执行和发布结果，资源诊断按实际后台任务完成点返回 |
| create_snapshot、publish_generation_manifest、migrate | destination、generation_id，或 new_path、new_dim | SDK 转换后进入 Core 目标发布，已知转换拒绝为 validation/not_committed，目标完成事实由 Core 提供 |
| verify_snapshot、restore_snapshot 模块函数 | directory，或 source、destination，以及可选 runtime | SDK 转换路径及句柄，verify 省略 write，restore 的结果描述目标目录发布 |
| load_ffi_hook、set_hook、clear_hook、clear_search_state | lib_path、宿主 Hook 对象或无参 | SDK 管理语言引用和动态库载入，Core 维护 Hook 安装、回调重入和状态清理，错误沿实际来源传递 |
| console_request、ConsoleSession.cancel | session、request_json，或 request_id | SDK 验证句柄和字符串，Core 解析权限、登记活动请求、执行命令与取消，未知命令内容的语言拒绝省略 write |
| close、close_timeout、request_shutdown、shutdown_status、dispose | timeout_ms 或无参 | SDK 管理借用，Core 决定关闭、排空和资源完成点，已取消请求仍可调用清理入口 |
| 统计、列表、返回对象与元数据 | 无参读取或公开只读属性，包括节点／边／结果、Prepared 参数名和 pending_count | 拥有型结果沿自身生命周期读取，数据库入口继续按公开生命周期契约准入 |

Node 事务操作省略 payload／metadata 时使用原 null 默认，省略连边 label／weight 时使用 related／1。显式提供错误类型的 label／weight 按对应字段拒绝。未知记录成员沿既有忽略规则处理，操作 type 继续按支持列表判定。ConsoleSession 与 PreparedTql 的公共句柄在调用进入原生方法前按来源核对，已知拒绝分别定位 session 与 prepared。Python 的对应原生类型提取在数据库请求借用内完成。

> **宿主提醒** 自定义转换、迭代、getter、Proxy 和 Hook 抛出的原异常沿宿主契约传播。错误 receiver、解释器实参展开和绕过公共加载器的原生调用需要按各自宿主接口处理。

---

## 写入结果字段

已覆盖的写入口错误提供可选 `write` 对象。Core 在真实执行阶段提供事实，SDK 补充进入 Core 前的拒绝状态。外层准入保留内层已经提供的写入上下文。Rust 使用 `WriteFailure` 承载上下文，通过 `underlying()` 匹配原错误；规范错误码、详情、message 与 cause 继续传递。

| 字段 | 契约 |
| --- | --- |
| `phase` | `admission`、`validation`、`wal`、`apply`、`publish`、`group_sync`、`response` |
| `outcome` | `not_committed` 表示本请求尚未提交事实。`unknown` 表示需要恢复或对账。`committed` 表示全部事实已完成应用，耐久性依照同步模式与组尾完成点 |
| `transaction_consumed` | 适用时提供布尔值，表示本次请求是否已消费待提交操作缓冲。直接 CRUD、聚合组结果及回执响应省略此字段 |

Rust `Database::commit_tx` 和 `Transaction::commit` 按值取得操作，进入提交入口后失败时 `transaction_consumed=true`。`SharedDatabase::commit_tx(&mut builder)` 与 Python 活动事务在 SDK 准入拒绝时保留操作，返回 `false`；Core 预检拒绝与后续失败返回 `true`。已结束 Python 对象再提交时，当前请求在准入阶段拒绝，消费字段为 `false`。

Node `commitTransaction` 的 Core 拒绝保留相同字段，绑定内已知的构造校验与同步重入拒绝提供相应阶段和 `transaction_consumed=false`。Node 参数数组由调用方继续持有，消费字段描述绑定生成的内核操作缓冲。参数自动提取和宿主自行抛出的异常继续使用各自语言契约。

分组提交在组尾完成同步。组尾失败返回 `group_sync/unknown`，消费字段省略。Server 将该错误逐一送回本组已经成功执行的等待请求，独立操作已返回的校验或执行错误保持原字段。分组准入失败将 `admission/not_committed` 送回尚未执行的请求。

Server 入队前的关闭、过期和队列满，以及 Worker 明确拒绝的待执行请求，提供 `admission/not_committed`。入队后等待响应超时或响应通道丢失，提供 `response/unknown`。Server 省略这些边界的消费字段，具体事务的执行结果由有效回执或事实对账确认。

Console 已提交事务的重放响应若超过预算，保留 `details.committed=true`，增加 `response/committed`。调用方可继续用原幂等键及原事务查询回执。有界回执随 WAL 和检查点持久化，确认结果受容量及淘汰规则约束。

文本登记、关键词、文本预热与重建、属性索引创建与删除、Rust 显式 QuIVer 构建，以及 `flush`／`compact` 提供相应的 `write`。这些入口省略 `transaction_consumed`。权限、生命周期和 WAL 可写性拒绝为 `admission/not_committed`；候选构建、字段与预算拒绝为 `validation/not_committed`。

文件组发布阶段使用 `publish`。提交标记及其同步完成前的失败为 `unknown`，标记完成后的 WAL 清理失败为 `committed`。唯一约束先在隔离候选中验证，再安装并立即发布；已安装约束的发布准备失败可返回 `apply/unknown`。约束发布失败后实例进入恢复保护，重新打开按真实提交点恢复。普通文本和派生索引修改通过后续 `flush` 或可写关闭持久化。

> **重试提醒** 先读取 `outcome`，再结合 `transaction_consumed`、错误类别与原事务回执决定重试。`not_committed` 的输入错误需要修正输入，`unknown` 需要对账或恢复，`committed` 需要处理响应或读取结果。分组内的结果同时依照组尾完成点。HTTP 携带 `write` 时使用 `retryable=false`，由调用方明确安排重新准入和重放。

---

## 操作与输入字段

| 操作与字段 | 校验或准入位置 | 当前错误与完成边界 |
| --- | --- | --- |
| 打开数据库的 `path` | Core 文件身份、文件租约与文件组打开 | `TDB_DATABASE_LOCKED`、`TDB_IO`；失败打开清理本次取得的资源 |
| 打开的 `dim`、`dtype`、访问模式与配置 | 语言提取、配置构造、Core 打开与文件身份校验 | `TDB_INVALID_INPUT`、`TDB_DIMENSION_MISMATCH`、`TDB_DTYPE_MISMATCH`、格式与恢复类别；语言提取仍按各端现有行为 |
| 共享句柄的读、写、关闭请求 | SDK 请求身份及 try-lock | `TDB_BUSY` 的 `resource`、`TDB_REENTRANT_ACCESS` 的 `operation`、`TDB_HOST_LOCK_POISONED` 的 `resource` |
| Python 数据库及事务对象的方法入口 | 显式借用与请求身份 | 同线程回调重入返回重入类别，跨线程借用冲突返回忙碌类别；原持有者继续执行 |
| 事务缓冲的 ID、向量、Payload、边及权重 | 语言转换、事务构造、Core 预检 | 维度、有限值、保留 ID、节点引用、唯一性、大小、容量与权限分别验证；Core 事务按完整操作集预检 |
| 已结束的 Python 事务对象 | builder 状态 | 缓冲、提交、回滚及进入上下文返回 `TDB_TRANSACTION_FINISHED`；`pending_count()` 为 0 |
| 事务提交及上下文退出 | SDK 准入、Core WAL 与原子应用 | SDK 准入拒绝保留 builder，进入 Core 后消费事务；宿主异常按上下文契约传播 |
| 节点及边的直接 CRUD、Payload 补丁与 CAS | Core 准入、校验、单条 WAL 与应用 | 准入或校验失败为 `not_committed`，WAL 或应用失败为 `unknown`；直接入口省略 `transaction_consumed` |
| 文本维护的 `id`、正文、关键词与完整重建集合 | Core 节点引用、输入、延期加载、候选构建及预算准入 | 写入前拒绝保留原文本事实，成功登记由后续检查点持久化 |
| 属性索引的 `field`／`fields`、唯一性与构建预算 | Core 隔离候选及公共维护准入 | 失败保留既有定义；唯一冲突按活跃 NodeId 升序提供确定的节点身份 |
| 索引删除、`flush` 与 `compact` | Core 权限、WAL 可写性与文件组发布 | WAL 关闭后拒绝维护；发布阶段按标记完成点提供 `unknown` 或 `committed` |
| 自动构建捕获、执行、运行期 QuIVer 模式与自动维护配置 | Core 准入、预算和发布前重检 | 捕获与发布前分别核对 WAL 和恢复保护，已失效 writer 返回结构化拒绝；候选失效沿用原来的丢弃结果 |
| `set_sync_mode`、`reserve_nodes` 与图索引修复 | Core 权限、WAL 可写性和修改前准入 | writer 失效时先拒绝；修复进入内存应用后失败按 `apply` 或文件组 `publish` 阶段提供结果，并隔离实例 |
| `set_memory_limit(bytes)` 与绑定的 `mb` | Core 资源登记及绑定的显式单位转换 | 额度调整失败保留原配置，已知输入失败提供 `target.field`；只读实例可调整自身运行时额度 |
| `create_snapshot`、`restore_snapshot`、`publish_generation_manifest` 与 `migrate_to` | 独立目标的校验、暂存和发布 | 最外层 `write` 描述目标快照、恢复目录、manifest 或迁移库；内部源检查点及暂存库的错误链继续保留 |
| TQL 写语句构造及计划准入 | Core 解析、写计划、操作预算与事务提交 | 事务构造前失败为 `validation/not_committed`；进入事务后保留内层提交阶段及消费状态 |
| 条件编辑的版本与幂等键 | Core 编辑状态与事务预检 | `TDB_WRITE_CONFLICT`、`TDB_IDEMPOTENCY_CONFLICT`、`TDB_WRITE_OUTCOME_UNKNOWN`，编辑版本按实例生成，有界提交回执随 WAL／检查点持久化 |
| TQL 的 `query` | Core 词法与解析入口 | `TDB_QUERY_PARSE`，`stage` 为 `lex` 或 `parse`，可靠位置使用 `target.query_span` |
| TQL 读取、准备及写入的 `query` 类型 | 绑定字符串提取 | 已覆盖类型错误提供 `TDB_INVALID_INPUT`、`target.field=query`，阶段省略；写入口提供 `validation/not_committed` |
| 普通、分组及批量检索的已覆盖整数参数 | 绑定整数提取及安全数值转换 | 提供输入字段定位，读取错误省略 `write`；Python 转换在请求借用后执行 |
| 检索的 `payload_filter`／`payloadFilter` | 绑定 JSON 表示及 Core Filter 校验 | Python 接受外层 `Mapping`；已覆盖表示及语义拒绝提供 `TDB_INVALID_INPUT` 与字段定位 |
| Prepared 的参数名与必需集合 | Core 参数绑定 | `TDB_MISSING_QUERY_PARAMETER`、`TDB_UNEXPECTED_QUERY_PARAMETER`，`details.parameter` 与 `target.parameter` 保持一致 |
| Prepared 的 `parameters` 容器 | Python 映射收集与 Node 原始对象检查 | 错误容器或 Python 非字符串键提供 `TDB_INVALID_INPUT` 和 `target.field=parameters`，阶段省略；Python 接受 `Mapping` |
| 同步模式 `mode` 和 manifest 的 generation 标识 | 绑定字符串提取及 Core 配置／发布校验 | 已覆盖类型拒绝提供字段定位及 `validation/not_committed`；Python 使用 `generation_id`，Node 使用 `generationId` |
| Prepared 的参数值 | 共用命名 JSON 转换与 Core 绑定 | 容器、有限值及向量槽位类型按原始错误类别返回，已知参数名写入 `target.parameter` |
| 关闭、关闭超时与状态读取 | SDK、Core 生命周期和任务排空 | `TDB_DATABASE_CLOSED`、`TDB_SHUTDOWN_FAILED`、WAL 与 I/O 类别；`close_timeout` 返回 `false` 时关闭任务继续执行 |

共享契约入口包括 `tests/contracts/error_cases.json`、`query_diagnostics.json`、`prepared_diagnostics.json` 和 `console_editing_cases.json`。这些用例同时核对对应字段、拒绝后的数据库事实及重开内容。

---

## 提交阶段与事实来源

下表记录现有实现能够判定的事实及事务错误中的相应字段。

| 阶段 | 现有实现提供的事实 | 调用方处理依据 |
| --- | --- | --- |
| SDK 借用与数据库准入 | 锁忙、重入、关闭或恢复保护在取走 builder 前拒绝 | `admission/not_committed`，待提交操作保留 |
| 参数转换与缓冲 | 完整转换后才追加本次操作 | 转换异常保留此前缓冲操作，调用方可修正当前输入 |
| Core 事务预检与预留 | 权限、引用、维度、唯一性、预算及 WAL 条目构造在追加前完成 | 权限与生命周期准入为 `admission`，其余预检为 `validation`；结果为 `not_committed`，Core 已消费操作 |
| WAL 追加前检查 | writer 可用性、事务 ID 和编辑回执准备先于追加 | 需要依据具体失败位置判定本次写入是否开始 |
| WAL 追加、flush 与 sync | I/O 失败可能留下部分帧或完整事务帧；writer 进入保护状态 | 单次事务追加失败为 `wal/unknown`，组尾同步失败为 `group_sync/unknown` |
| MemTable 应用 | WAL 已完成本次追加；应用失败由保护状态隔离部分内存事实 | `apply/unknown`，重新打开后核对完整事务与恢复结果 |
| 文件组发布 | 发布准备与替换进入 `publish`，提交标记及对应同步完成后记录已提交 | 常规发布返回 `publish/unknown` 或 `publish/committed`；迁移在提交前回滚并完成清理时返回 `publish/not_committed` |
| 普通提交成功 | 按配置的同步模式完成该次事务 | 持久化保证依照 `SyncMode`；分组提交以组尾同步完成点确认 |
| 响应交付 | 内核已知的响应编码或预算失败保留写入事实，宿主传输可能丢失响应 | 已知全部应用完成时提供 `response/committed`；其他传输错误依照原事务回执或事实对账 |

`SyncMode::Normal`、`Full` 和 `Off` 的耐久性分别遵守其同步契约。分组内的单次事务成功、组尾同步和传输响应具有各自完成点。实例内编辑回执的容量、淘汰与重开行为见[嵌入式控制台](embedded-console.md)。

直接节点和边写入保留单条 WAL 编码及原有索引同步方式。WAL 追加后的应用失败复用事务恢复保护，隔离当前实例及已准入的共享查询；释放句柄并重新打开后核对恢复事实。资源诊断通过原始错误类别提取预算和 I/O 信息。

快照、恢复、manifest 和迁移以各自的目标发布作为结果范围。源检查点或暂存库失败且目标尚未发布时，最外层返回 `validation/not_committed`。快照目录重命名后的父目录同步失败返回 `publish/unknown`。迁移的提交标记与目录同步完成后，清理或返回实例失败提供 `publish/committed`。调用方结合当前操作、错误类别和目标路径处理恢复。

后台维护的 `ResourceFailure` 保留 `kind`、`code`、`message`、`details`、可选 `write` 和预算字段。已执行压实按真实文件组进度报告结果，调度预算拒绝使用 `admission/not_committed`。关闭等复用该诊断结构的任务按已知执行点提供可选字段。

Python／Node 的同步模式字符串拒绝、容量预留、MiB 额度、自动压实间隔和自动构建开关的已覆盖输入拒绝提供 `validation/not_committed` 与字段定位。Python 的内置类型提取错误保留 `TypeError`／`OverflowError` 继承及规范属性，管理参数在数据库请求借用后转换。Python 自定义整数协议和映射协议产生的宿主异常保留原对象、cause 与已有诊断。SDK 对上述管理操作及图修复的回调重入拒绝提供 `admission/not_committed`。

Python Prepared 接受字符串键 `Mapping`，转换值失败时提供 `stage=bind` 与原始 `target.parameter`。映射容器和键错误使用 `target.field=parameters`；映射自身的 `items`、迭代和取值异常沿宿主路径传播。

Node Prepared 在方法内读取原始参数，错误容器提供 `target.field=parameters`。NaN、无穷、undefined、BigInt、Symbol、函数及复合参数值提供 `TDB_INVALID_INPUT`、`stage=bind` 与原始 `target.parameter`，键名保留 NUL 和 Unicode。数组和对象值在标量边界拒绝。容器属性枚举与 getter 抛出的宿主异常保留原对象和 cause，既有属性枚举及标量数值转换继续复用原路径。

Python 的同步模式与 generation 标识也在请求借用后提取字符串，已覆盖内置类型错误保留原异常继承及规范字段。Node 对这两个字段执行原始字符串校验。manifest 的 SDK 回调拒绝提供 `admission/not_committed`，目标发布结果继续按 Core 文件组完成点描述。

Python 普通及分组检索的 `top_k`、`recall_k`、`rerank_k`、`expand_depth`、`max_edges_per_node`，以及批量检索的 `top_k`、`recall_k`、`rerank_k`、`expand_depth`、`parallelism` 在请求借用后提取。已覆盖内置类型拒绝保留原类型异常、字段定位及空 `write`；自定义整数协议异常保留原对象。Node 普通／分组检索的 `topK`、`expandDepth` 和批量检索的 `topK`、`parallelism` 提供对应类型及安全整数诊断，可选参数的 null／省略值沿用原默认行为。

检索字段继续覆盖以下入口。Python 的提取均在数据库请求借用内执行，已覆盖的输入拒绝保留类型异常和字段定位，读取错误省略 `write`。

| 入口 | Python 字段 | Node 字段 |
| --- | --- | --- |
| 精确检索 | `top_k` | `topK` |
| 图优先检索 | `top_k`、`max_anchor_nodes` | `topK`、`maxAnchorNodes` |
| 上下文检索 | `top_k`、`recall_k`、`rerank_k`、`expand_depth` | 配置中的 `topK`、`recallK`、`rerankK`、`expandDepth`、`maxEdgesPerNode` |
| 高级检索 | `top_k`、`recall_k`、`rerank_k`、`expand_depth`、`max_edges_per_node` | 配置中的 `topK`、`recallK`、`rerankK`、`expandDepth`、`maxEdgesPerNode` |
| 混合检索 | `top_k`、`expand_depth`、`query_text` | `topK`、`expandDepth`、`queryText` |

Node 配置对象中的上述数字字段以省略或 `undefined` 使用默认值，`null` 按既有类型边界拒绝。方法的可选数字实参继续以省略或 `null` 使用默认值，必需数量参数拒绝 `null`。配置 getter 抛出的宿主异常原样传播。

Python 图优先检索的锚点序列在请求借用后通过既有 PyO3 Sequence 路径提取，序列取值及元素整数协议中的同库写入按重入契约拒绝。混合检索文本的类型错误定位 `query_text`／`queryText`，Python 字符串编码错误同样定位 `query_text`，阶段省略。序列成员路径与框架长度提示错误策略继续单独收敛。

图读取的字段诊断覆盖以下入口，读取错误省略 `write` 和 `stage`。

| 入口 | Python 字段 | Node 字段 |
| --- | --- | --- |
| `neighbors` | `id`、`depth`、`labels` | `id`、`depth` |
| `reachable`、`reachable_detailed`、`query_subgraph` | `id`、`min_depth`、`max_depth`、`max_visited_nodes`、`max_results`、`max_edges`、`direction`、`labels` | `id` 与配置中的 `minDepth`、`maxDepth`、`maxVisitedNodes`、`maxResults`、`maxEdges`、`direction` |

Python 在读请求借用内提取这些参数，保留内置类型错误的异常继承、自定义整数及标签序列协议的原异常。方向枚举拒绝保留 `ValueError`，并增加规范字段。Node 数字及方向字段执行原始类型校验，配置 getter 的原异常直接传播。省略配置字段使用默认值，配置字段的 `null` 按原类型边界拒绝；可选方法实参继续以 `null` 使用默认值。范围组合、零预算及遍历截断由既有 Core 契约判定，标签及其他嵌套成员路径继续逐项登记。

十五个直接 CRUD 入口的 SDK 借用争用和重入拒绝补齐 `admission/not_committed`，省略 `transaction_consumed`。覆盖插入、带 ID 插入／替换、两类批量插入、单个／原子批量删除、Payload 替换／补丁／条件替换、向量更新和四类边写入。Python 在数据库借用边界附加结果，Node 公共加载器在原生调用前附加结果。绑定框架在此之前执行的自动参数提取继续按对应入口收敛。

基础读取的参数诊断覆盖以下入口，读取拒绝省略写入结果和解析阶段。

| Python 入口 | Node 入口 | 字段 |
| --- | --- | --- |
| `get`、`get_payload`、`get_edges`、`id in db` | `get`、`getPayload`、`getEdges`、`contains` | `id` |
| `get_incoming_edges` | `getIncomingEdges` | `id`、`label` |
| `get_edge` | `getEdge` | `src`、`dst`、`label` |

Python 在数据库读请求借用内提取整数和标签，内置提取错误保留原异常类别并附加字段，自定义整数协议原异常保持对象身份与 cause。存在性检查通过 `__contains__` 的普通成员操作进入同一边界。Node 校验原始数值／字符串及安全整数范围，数值对象和字符串对象按类型拒绝。入边读取的省略标签与 None／null 表示全部标签，空字符串作为实际标签精确匹配；精确边读取要求标签实参。Python 标签编码失败同样定位 `label`。

`tql`、`prepare_tql` 和 `tql_mut` 的已覆盖字符串提取错误定位 `query`，发生在词法解析之前，阶段省略。写查询的类型拒绝提供 `validation/not_committed`，SDK 回调拒绝提供 `admission/not_committed`。读取查询省略写入结果。

Python 五个公开 Filter 入口共用外层 Mapping 转换，涵盖普通、分组、上下文、高级和混合检索，其中上下文入口为 `search_with_context`。Mapping 协议在数据库请求范围内执行，宿主异常原样传播；JSON 表示错误保留原异常类型并定位 `payload_filter`。Python／Node 的 Core Filter 语义拒绝提供 `TDB_INVALID_INPUT` 与相应字段。其余 JSON 转换前错误和嵌套成员定位继续逐入口登记。

> **覆盖范围** 当前字段覆盖 Core 事务、直接节点及边 CRUD、Payload 补丁与 CAS、TQL 写计划、文本与属性索引维护、Rust 显式及已捕获 QuIVer 构建、运行期模式、自动维护配置与压实诊断、快照／恢复／manifest／迁移目标、共享 SDK、语言事务的已知绑定校验、分组提交、Server 队列与响应等待及已知 Console 写响应。语言实参提取、其他管理入口、HTTP／IPC 外部传输及宿主取消继续按入口收敛。

---

## 取消与期限

`QueryControl::cancel()` 发出协作取消信号，检查点观察到该信号时返回 `TDB_QUERY_CANCELLED`。`QueryControl::with_deadline()` 的期限到达时返回 `TDB_QUERY_DEADLINE_EXCEEDED`。两者详情均为空对象。TQL 执行检查提供 `execute` 及已知来源范围，进入 TQL 前的请求检查提供该层已经确定的字段。同一检查点同时观察到两种条件时，期限优先。克隆继续共享取消信号，Console 的会话期限与外部期限取较早者。

Console 在命令分发前观察到写请求终止时提供 `admission/not_committed`。`transaction`、`guardedTransaction`、`importNodes` 和 `importEdges` 在 WAL 前准备完成处终止时提供 `validation/not_committed`，省略事务消费字段。TQL 写计划沿用 Core 的提交前校验结果。进入 WAL 的提交按既有原子完成契约执行。

| 入口与情况 | 返回与结果 |
| --- | --- |
| Core 主动取消 | `TDB_QUERY_CANCELLED`，空详情 |
| Core 期限到期 | `TDB_QUERY_DEADLINE_EXCEEDED`，空详情 |
| 独立 Server 期限到期 | HTTP 504，规范期限码，`legacyCode=REQUEST_TIMEOUT`；读取沿用可重新发起请求的 `retryable=true`，携带 `write` 时为 `false` |
| 嵌入 Console 执行期限到期 | HTTP 504，规范期限码及 Core 已知写入结果 |
| 嵌入 Console 请求体读取期限到期 | HTTP 408，规范期限码，命令尚未解析时省略 `write` |
| HTTP 主动查询取消 | HTTP 408，`TDB_QUERY_CANCELLED` |
| 普通类型化请求作用域 | Python ContextVar／Node AsyncLocalStorage 传递控制，原生工作线程接续同一控制器 |
| 自定义桥接 drain 返回 false | `TDB_BUSY`，`details.resource=console_bridge_drain`，停止准入并允许重试 dispose |

Server 入队前期限拒绝提供 `admission/not_committed`，入队后响应等待到期提供 `response/unknown`。取消信号和请求等待结束各有完成点，工作区及文件资源在实际执行退出后归还。保留的持久回执按完整原事务身份提供跨重开确认。

> **接入提醒** 期限错误从原 `TDB_QUERY_CANCELLED` 或 Server 自有码 `REQUEST_TIMEOUT` 收敛到独立规范码。Rust 穷尽匹配需处理新增 `QueryDeadlineExceeded`，Node 错误联合已增加对应空详情分支。Python 保留 `TriviumDBError` 的既有继承关系。

---

## 请求与传输完成点

| 完成点 | 确认依据 | 写入结果与资源范围 |
| --- | --- | --- |
| 客户端本地排队 | 尚未调用请求桥接，取消在本地结束 | 已识别写请求为 `admission/not_committed` |
| 桥接分发 | 请求交给 IPC 或 HTTP 适配器 | 网络中断、响应解码及身份校验失败时，写请求为 `response/unknown` |
| Console 准入 | Core 已解析命令，检查会话、能力及数据库写权限 | 已知写命令的拒绝提供 `admission/not_committed`，原错误码及详情保留 |
| Server 单条执行准入 | 取得实际写锁后，在各条操作前核对响应接收端和期限 | 已退出的等待者对应操作跳过，到期请求返回 `admission/not_committed`，后续有效操作继续处理 |
| Core 执行退出 | 请求 Guard 释放，已完成本次执行及响应编码 | 工作区按实际所有权归还，事务结果遵守 WAL、应用和同步契约 |
| Console 会话排空 | `detach()` 关闭准入，`is_drained()`／`isDrained()` 确认全部 Core 请求退出 | `true` 是该会话的 Core 执行完成点 |
| 客户端适配器卸载 | `dispose()` 等待 detach 回调、本地请求队列及已经发出的取消调用全部 settle | 完成范围为适配器拥有的 Promise；Core 会话及数据库由宿主分别确认 |
| 数据库关闭 | Core 关闭流程成功完成 | 持久化、后台任务、mmap 和文件租约按生命周期契约完成 |

`ConsoleSession.cancel(id)` 返回 `true` 时，活动请求已经收到取消信号。调用方通过请求结果确认该次操作，通过会话排空状态确认 detach 后的整体执行完成。嵌入 HTTP 的 `GET /v1/console/status` 返回 `{"drained": bool}`，使用 `Cache-Control: no-store`，查询复用 Core 状态，并独立于数据库锁和请求并发许可。

`is_drained()` 描述 Core 会话。HTTP 请求体读取、blocking 适配任务、响应消费及数据库关闭各自记录完成点。进入 Core 提交的工作按照原子提交协议结束，响应等待结束后的写入状态依据返回的 `write`、原事务回执和实际数据确认。

Console 数据适配器将宿主连接、解码、协议和响应身份错误归类为 `TDB_CONSOLE_TRANSPORT`，保留原异常 `cause`。HTTP 错误信封保留实际状态、内核错误字段与已有写入结果。活动 HTTP 请求的 AbortSignal 传给 fetch，提前结束的响应等待为写请求提供 `response/unknown`。本地排队取消提供 `TDB_QUERY_CANCELLED`、`AbortError` 和已知未提交结果。后台适配任务失败同样按已知信息提供未知写入结果。

原事务确认发生传输失败或收到无效回执时，`submit(..., true)` 保留 `response/unknown`。请求身份与命令内容取自排队前的序列化快照。活动请求的取消调用按请求去重，卸载等待已经发出的取消调用结束。

> **重试提醒**　`unknown` 的写入先使用完整原事务确认或数据对账。保留的提交回执支持恢复及跨重开确认，缺失或淘汰后的事务按应用持久化身份核对完整事实。

---

## 语言及传输边界

| 入口 | 已覆盖的结构 | 继续收敛的边界 |
| --- | --- | --- |
| Rust Core／SDK | 共享准入、类型化作用域、WAL 前检查、精确扫描、图算法和遍历检查点、TQL 来源及持久回执 | 平台与实际负载验收 |
| Python | 数据库及事务借用、字段目录、完整成员路径、Prepared 来源和任务局部作用域 | 宿主自行创建的线程和专用任务系统显式传递上下文 |
| Node | 字段目录、无损整数、同步重入、事务操作、异步上下文及工作线程控制 | 自建 Worker／进程显式传递消息与控制 |
| CLI | JSON 模式的 Core、命令解析、命令错误、I/O 和配置警告 | 外部管道及宿主进程退出按各自完成点处理 |
| 独立 Server | 规范码、legacyCode、路径／查询拒绝、请求体前身份、队列和响应结果、重试目录 | 缓存容量与进程生命周期遵守 Server 当前配置 |
| 嵌入 Console／HTTP／IPC | 请求头身份、正文核对、会话排空、未知响应结果、有界回执、自定义桥接 drain | 宿主专用桥接按协议确认其实际任务完成 |

HTTP 状态与旧分类遵守各适配器当前协议，规范 `code`、`details`、`stage` 和 `target` 从 Core 传递。Server 的 `legacyCode` 用于旧调用方迁移。

---

## 重试与兼容目录

| 条件 | 规范类别与兼容字段 | 调用方动作 |
| --- | --- | --- |
| SDK 锁忙或 Server 写队列满 | TDB_BUSY，队列 details.resource=write_queue，legacyCode=WRITE_QUEUE_FULL | admission/not_committed 时可按宿主有界队列重新准入，保留 builder 的入口继续使用原缓冲 |
| Server 停止准入 | TDB_SERVER_UNAVAILABLE，legacyCode=SERVICE_UNAVAILABLE | 等待服务可用，写入按 write 及业务身份安排下一步 |
| 主动取消和期限 | TDB_QUERY_CANCELLED、TDB_QUERY_DEADLINE_EXCEEDED，Server 保留 REQUEST_TIMEOUT 旧类别 | 读取可重新发起，写入区分 admission、validation 与 response 结果 |
| 已知校验拒绝 | TDB_INVALID_INPUT、维度／向量／唯一约束等 Core 类别 | 修正输入或前置条件，已消费 builder 需重新构造 |
| 版本或幂等键冲突 | TDB_WRITE_CONFLICT、TDB_IDEMPOTENCY_CONFLICT，旧 Server 幂等码保存在 legacyCode | 读取事实并处理冲突，保留待确认事务的原身份 |
| HTTP 前置头条件冲突 | TDB_HTTP_PRECONDITION_FAILED，legacyCode=WRITE_CONFLICT | 刷新基线并明确处理版本差异 |
| 响应未知 | write.outcome=unknown，或 TDB_WRITE_OUTCOME_UNKNOWN | 使用完整原事务查询／重放保留回执，回执缺失或淘汰后按业务身份核对完整事实 |
| 已知应用完成 | write.outcome=committed | 按 SyncMode 和组尾同步点确认耐久性，处理响应或清理后续步骤 |
| 会话或数据库关闭 | TDB_CONSOLE_SESSION_CLOSED、TDB_DATABASE_CLOSED | 根据新实例和权限建立新句柄／会话 |
| Prepared 缓存未命中 | TDB_PREPARED_NOT_FOUND，legacyCode=PREPARED_NOT_FOUND | 在当前实例重新 prepare，再使用对应句柄 |
| HTTP 路由、方法、媒体类型、请求体及响应预算 | TDB_HTTP_ROUTE_NOT_FOUND、TDB_HTTP_METHOD_NOT_ALLOWED、TDB_HTTP_UNSUPPORTED_MEDIA_TYPE、TDB_HTTP_REQUEST_TOO_LARGE、TDB_HTTP_RESPONSE_TOO_LARGE，legacyCode 保留对应旧类别 | 修正端点、方法、编码或预算，流式接口按其既有完成点使用 |
| Server 适配任务异常 | TDB_SERVER_INTERNAL，legacyCode=INTERNAL_ERROR | 保留请求身份，写入按已提供事实或原事务对账 |
| CLI 参数、非 Core 命令错误与配置警告 | TDB_CLI_ARGUMENT、TDB_CLI_COMMAND、TDB_CLI_CONFIG，I/O 使用 TDB_IO | 显式 --format json／--format=json 时 stderr 输出逐行 JSON，配置解析失败沿既有默认配置策略并输出 level=warning |

Server 的 retryable 是适配器提示。带 write 的错误统一为 false，调用方依据上表的结果事实决定重新准入、重建或确认。读取的期限、锁忙及服务暂不可用继续按其实际映射提供提示。Core 与 SDK 保留结构化事实，重试调度归宿主管理。

精确扫描和已接入的图算法按候选、节点、边或迭代检查控制器，Rayon 任务携带同一信号，错误返回时归还工作区。不可中断的有限库调用结束后继续检查。WAL 开始后的事务继续依照原子协议完成。原有直接返回集合的低层图辅助函数保持自身同步返回契约，数据库和 TQL 调用使用可失败的受控路径。

---

## 文档接入错误

文档分批接入使用 `TDB_DOCUMENT_UPLOAD_NOT_FOUND` 和 `TDB_DOCUMENT_UPLOAD_CONFLICT`。前者的 `details.upload_id` 标识缺失候选，后者的 `details.field` 与 `target.field` 标识创建内容、批次序号、摘要或状态冲突。发布时的根修订竞争继续采用 `TDB_DOCUMENT_REVISION_MISMATCH`。候选状态通过原上传身份及 `next_sequence` 核对，读取命令省略写入结果，写命令沿 Core 的实际阶段返回。完整协议见[文档解析与分批发布](document-ingestion.md)。

---

## 后续收敛

- 当前字段和重试目录随公共 API 同步维护，新增入口同时登记校验层、位置、完成点与兼容处理。
- 宿主专用任务系统按请求及 drain 协议接入，应用持久化身份用于回执淘汰后的事实对账。
- AST 节点范围、规划与执行来源按 TQL 来源契约交付。参数引用使用首个真实引用范围，普通 API 的字段路径按各入口维护。
- 整批验收按实际提交、平台、特性和安装制品记录，当前实现与历史验收证据分别保留。
