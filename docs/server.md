# TriviumDB Server（HTTP 服务端版）

> 本文说明 `0.9.0` 源码中的 nightly Server，覆盖 HTTP 协议、并发调度、部署与关闭。服务端使用 Rust、Axum 和 Tokio，许可为 Apache-2.0。版本记录见[变更说明](changes/0.9.0.md)。

---

## 目录

- [状态声明：nightly 预览](#状态声明nightly-预览)
- [架构与并发模型](#架构与并发模型)
- [构建与运行](#构建与运行)
- [配置参考](#配置参考)
- [HTTP API](#http-api)
- [OCC、幂等与错误模型](#occ幂等与错误模型)
- [健康、任务监管与可观测性](#健康任务监管与可观测性)
- [边界与已知限制](#边界与已知限制)
- [按需开发自测](#按需开发自测)

---

## 状态声明：nightly 预览

TriviumDB 提供 Rust、Python 和 Node.js 嵌入式包：

```bash
pip install triviumdb
npm install triviumdb
cargo add triviumdb
```

`triviumdb-server` 目前处于 nightly 预览状态：

- HTTP 路由、字段和错误码仍可能调整；
- 目前从仓库源码构建，兼容性随 nightly 开发调整；
- 一切核心 API、存储格式和查询语义仍以嵌入式版本为准；
- 认证和 TLS 由部署方通过反向代理或上层服务配置；
- 当前适用范围为开发和集成评估。

Server 支持多客户端访问、Writer Actor、Group Commit、OCC、健康监管、流式响应和批量导入。

> **部署提醒**：nightly Server 用于开发和集成评估。将监听端口限定在可信网络，需要远程访问时由上层服务或反向代理提供认证与 TLS。

宿主已经打开嵌入式数据库时，使用独立的 `console::build_console_app` 接入 Console RPC v1，复用现有实例和文件锁。该适配器通过 ConsoleHost 直接调用宿主实例。当前开发版本复用 Core 的全库快照条件事务及实例内回执；Console 使用独立的版本令牌和整请求响应协议。编辑版本冲突、幂等键错配和结果未知映射为 HTTP 409，并保留 Core 错误码与详情。前端传输适配与完整契约见[嵌入式 Console 宿主接入](embedded-console.md)。现有 `build_app(ServerConfig)` 与 `/ui` 的 HTTP v1 控制台继续保留原有行为。

---

## 架构与并发模型

职责划分见[内核 SDK 与存储内核](kernel-sdk.md)。独立 Server 继续使用自己的有界 Writer 队列与合批；嵌入宿主可使用 `SharedDatabase` 和 `ConsoleHost::attach_shared` 复用实例。

当前开发版将内核错误的 `code`、`details` 和 `schema_version: 1` 原样映射到 HTTP，`message` 与 RFC Problem Details 的 `detail` 同义。原 HTTP 分类名通过 `legacyCode` 提供迁移信息，例如 `QUERY_PARSE_ERROR` → `TDB_QUERY_PARSE`。超出 JavaScript 安全整数域的详情字段编码为十进制字符串。期限到达使用 `TDB_QUERY_DEADLINE_EXCEEDED`、HTTP 504 和 `legacyCode=REQUEST_TIMEOUT`，主动取消使用 `TDB_QUERY_CANCELLED` 和 HTTP 408。携带写入结果的期限错误保持 `retryable=false`。队列与其他传输错误按各自入口继续收敛，详见[公共错误契约](error-contracts.md#取消与期限)。

开始关闭后，新读写请求使用 HTTP 503 和 `TDB_DATABASE_CLOSED`，`details` 为空对象，保留 `legacyCode: "SERVICE_UNAVAILABLE"`，`retryable` 为 `false`。调用方按实例生命周期处理该错误；已经准入的请求继续遵守排空与提交契约。

TQL 词法／语法拒绝使用 HTTP 400 和 `TDB_QUERY_PARSE`，保留 `legacyCode: "QUERY_PARSE_ERROR"`。`stage` 区分 `lex`、`parse`，`target.query_span` 按内核提供的准确位置可选返回。嵌入 Console HTTP 的 `error` 信封保留相同字段；完整定义见 [TQL 结构化诊断](tql-reference.md#结构化查询诊断)。

```text
HTTP / JSON / NDJSON / 二进制 f32
        │
┌───────┴──────────────────────────────┐
│ triviumdb-server（Axum + Tokio）     │
│  ├─ liveness / readiness / metrics  │
│  ├─ 并发读：有界 semaphore + blocking│
│  ├─ 写：bounded queue → Writer Actor │
│  │                       → Group Commit│
│  ├─ OCC / 幂等 / deadline / cancellation│
│  └─ QuIVer 后台预热与任务监管       │
├──────────────────────────────────────┤
│ triviumdb（同一份嵌入式核心）        │
└──────────────────────────────────────┘
```

- Server 和 Embedded 共用同一个 `triviumdb` crate 的存储与查询实现；
- Axum/Tokio 等依赖只存在于 `crates/triviumdb-server`，嵌入式用户按需引入服务端 crate；
- Core 的单写多读契约不变，Server 将并发客户端写入串行化；
- Writer 等待时暂停新读准入；读等待使用通知唤醒；
- 读、写、预热和手工 QuIVer 构建均在受监管的 blocking 任务中运行；
- 请求断开或 deadline 到期会传播取消信号；已经进入 durable commit 的写入继续完成原子提交。

写请求错误的可选 `write` 字段记录阶段与结果。队列满、入队前过期和 Worker 明确拒绝的待执行请求返回 `admission/not_committed`；入队后等待响应超时或通道丢失返回 `response/unknown`。分组同步失败向已经执行成功的等待请求返回 `group_sync/unknown`，独立操作错误保留自身字段。携带写入上下文时 `retryable=false`，调用方依据回执和实际事实决定重新准入、重放或对账。字段说明见[公共错误契约](error-contracts.md#写入结果字段)。

---

## 构建与运行

```bash
cargo build --release -p triviumdb-server

./target/release/triviumdb-server \
  --database /var/lib/triviumdb/main.tdb \
  --listen 0.0.0.0:8080 \
  --dim 1536
```

健康检查：

```bash
curl http://127.0.0.1:8080/health/live
curl http://127.0.0.1:8080/health/ready
curl http://127.0.0.1:8080/health/details
```

优雅关闭使用 `Ctrl+C` 或 `SIGTERM`。进程后台化交给操作系统服务管理器，Server 自身不 daemon 化。

独立 Server 在 HTTP 停止后排空 Writer 队列及真实 blocking 工作，再等待 Core 完成持久化和文件资源释放。关闭开始后拒绝新数据操作；尚未执行的请求按取消和 deadline 状态决定执行或拒绝。调用方取消等待后，已启动的关闭工作继续运行；持久化失败会报告错误并保留文件锁，宿主可排障后重试。

嵌入 HTTP Server 的 Rust 宿主可使用 `build_app_with_shutdown(config)`，获取 `(Router, ServerShutdown)`。停止 HTTP 接入后调用 `shutdown.shutdown().await?`，完成后即使保留 Router 和关闭句柄，也可以重新打开数据库。等待期间保持 Tokio runtime 存活。原有 `build_app(config)` 保持兼容，但只返回 Router，不提供可等待的关闭完成点。复用宿主现有数据库的 `build_console_app` 仍由宿主负责关闭数据库。

```rust,no_run
use triviumdb_server::{ServerConfig, build_app_with_shutdown};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let listener = tokio::net::TcpListener::bind("127.0.0.1:8080").await?;
let (app, shutdown) = build_app_with_shutdown(ServerConfig::default()).await?;
let serving = axum::serve(listener, app)
    .with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; })
    .await;
let closed = shutdown.shutdown().await;
serving?;
closed?;
# Ok(())
# }
```

---

## 配置参考

所有参数同时支持命令行和环境变量，优先级为 **命令行 > 环境变量 > 默认值**。

| CLI 参数 | 环境变量 | 默认值 | 说明 |
| --- | --- | ---: | --- |
| `--log-format` | `TRIVIUMDB_LOG_FORMAT` | `pretty` | `pretty` 或 `json` |
| `--database` | `TRIVIUMDB_DATABASE` | `triviumdb-server.tdb` | 数据库路径 |
| `--listen` | `TRIVIUMDB_LISTEN` | `127.0.0.1:8080` | 监听地址 |
| `--dim` | `TRIVIUMDB_DIM` | `1536` | 向量维度 |
| `--max-query-rows` | `TRIVIUMDB_MAX_QUERY_ROWS` | `10000` | 查询行上限，0 为不限 |
| `--memory-limit` | `TRIVIUMDB_MEMORY_LIMIT` | `0` | Core 内存上限（字节），0 为不限 |
| `--write-queue-capacity` | `TRIVIUMDB_WRITE_QUEUE_CAPACITY` | `256` | 有界写队列容量 |
| `--max-concurrent-reads` | `TRIVIUMDB_MAX_CONCURRENT_READS` | 系统并行度 | 最大并发读 |
| `--idempotency-capacity` | `TRIVIUMDB_IDEMPOTENCY_CAPACITY` | `4096` | 进程内幂等缓存容量 |
| `--max-write-batch-size` | `TRIVIUMDB_MAX_WRITE_BATCH_SIZE` | `64` | Group Commit 合批上限 |
| `--max-write-batch-delay-us` | `TRIVIUMDB_MAX_WRITE_BATCH_DELAY_US` | `500` | 动态合批窗口（微秒） |
| `--prepared-cache-capacity` | `TRIVIUMDB_PREPARED_CACHE_CAPACITY` | `1024` | Prepared TQL 缓存容量 |
| `--request-timeout-ms` | `TRIVIUMDB_REQUEST_TIMEOUT_MS` | `30000` | 请求 deadline（毫秒） |
| `--max-body-bytes` | `TRIVIUMDB_MAX_BODY_BYTES` | `4194304` | HTTP 请求体硬上限 |

```bash
RUST_LOG=triviumdb_server=info,triviumdb=warn ./triviumdb-server
```

---

## HTTP API

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/` 或 `/ui` | 内置 Web Console 前端视窗（Navicat 风格三模管理与可视化教程） |
| GET | `/health/live` | 纯事件循环存活探针，不获取 Database 锁 |
| GET | `/health/ready` | Writer/read capacity 就绪状态；不可服务时返回 503 |
| GET | `/health/details` | 脱敏运行状态、队列、读写等待和 QuIVer 预热状态 |
| GET | `/metrics` | Prometheus 文本指标 |
| POST | `/v1/tql` | TQL 查询或 mutation |
| POST | `/v1/prepared` | 创建 Prepared TQL |
| POST | `/v1/prepared/{id}/execute` | 执行 Prepared TQL |
| POST | `/v1/search/vector` | little-endian f32 二进制向量搜索 |
| POST | `/v1/transactions` | 多操作原子事务和 OCC precondition |
| GET | `/v1/nodes/{id}` | 节点详情及节点/边 ETag |
| POST | `/v1/nodes/batch-get` | 按 ID 批量读取节点并报告缺失 ID |
| GET | `/v1/nodes/{id}/neighbors` | 有界多跳邻域子图，返回真实节点、边和截断状态 |
| GET | `/v1/nodes/{id}/edges` | 按方向、标签和分页列举真实边 |
| GET | `/v1/nodes/{from}/paths/to/{to}` | 有界最短路径及逐跳边标签 |
| GET | `/v1/indexes` | 查询四类属性索引及统计 |
| POST | `/v1/indexes` | 创建属性索引 |
| DELETE | `/v1/indexes/delete` | 删除属性索引 |
| GET | `/v1/indexes/quiver` | 查询 QuIVer 预热状态 |
| POST | `/v1/indexes/quiver` | 按 Core 资格和预算启动 QuIVer 构建 |
| POST | `/v1/import/nodes` | 有界、整请求原子的节点 NDJSON 导入 |
| POST | `/v1/import/edges` | 有界、整请求原子的边 NDJSON 导入 |

### TQL

```bash
curl -X POST http://127.0.0.1:8080/v1/tql \
  -H 'content-type: application/json' \
  -d '{"query":"CREATE ({name: \"Alice\"})","mutation":true}'

curl -X POST http://127.0.0.1:8080/v1/tql \
  -H 'content-type: application/json' \
  -d '{"query":"FIND {name: \"Alice\"} RETURN *","profile":true}'

curl -X POST http://127.0.0.1:8080/v1/tql \
  -H 'content-type: application/json' \
  -d '{"query":"TEXT HYBRID \"vector database\" TOP 100 AS seed WITH seed DIVERSIFY seed TOP 10 AS result WITH result RETURN result, text_score(result) AS text, diversity_score(result) AS diversity","profile":true}'
```

Server 不为认知算子维护另一套 HTTP API；`TEXT`、`RESIDUAL`、`DIVERSIFY`、`TOPICS` 和 `SA_PPR_CONFIG` 均通过 `/v1/tql` 使用 Core 的相同 Parser、Cascades、预算和确定性语义。Refractory Fatigue 未开放为普通 TQL 算子，因为它会修改进程内检索状态。

普通 JSON 查询响应有 16 MiB 硬上限。超限返回 `413 RESPONSE_TOO_LARGE`；应改用属性投影、`LIMIT` 或 NDJSON 响应。

请求头设置 `Accept: application/x-ndjson` 时，响应按 `meta → row... → summary` 输出；`meta.columns` 明确声明每个投影列的 `node`、`edge`、`path` 或标量类型。普通 JSON 响应也在顶层 `columns` 返回相同元数据。HTTP 输出逐行生成，但排序、聚合和 Top-K 等 Core 算子仍可能先物化结果。

### 属性索引

```bash
curl -X POST http://127.0.0.1:8080/v1/indexes \
  -H 'content-type: application/json' \
  -d '{"kind":"hash","fields":["type"]}'

curl -X POST http://127.0.0.1:8080/v1/indexes \
  -H 'content-type: application/json' \
  -d '{"kind":"composite","fields":["tenant","type"]}'

curl -X DELETE http://127.0.0.1:8080/v1/indexes/delete \
  -H 'content-type: application/json' \
  -d '{"kind":"hash","fields":["type"]}'
```

可用 kind 为 `hash`、`ordered`、`bitmap` 和 `composite`。单字段索引必须恰好提供一个字段；复合索引至少两个字段。修改操作统一进入 Writer Actor，并推进 Server generation。

### 二进制向量搜索

请求体必须为连续 little-endian f32，媒体类型为 `application/vnd.triviumdb.vector+f32`：

```text
POST /v1/search/vector?top_k=10&recall_k=80&rerank_k=40&min_score=0.1
Content-Type: application/vnd.triviumdb.vector+f32
```

支持参数：

| 参数 | 默认值 | 服务端约束 |
| --- | ---: | --- |
| `top_k` | `10` | `1..=10000` |
| `recall_k` | `0`（Core 自动） | 不超过 `100000` |
| `rerank_k` | `0`（Core 自动） | 不超过 `100000` |
| `min_score` | `-1.0` | 必须为有限值 |
| `force_brute_force` | `false` | true 时请求精确暴力搜索 |

查询向量中的 NaN/Infinity 会被拒绝。

### NDJSON 批量导入

节点行：

```json
{"id":1,"vector":[1.0,0.0],"payload":{"type":"event"}}
```

边行：

```json
{"source":1,"target":2,"label":"related","weight":1.0}
```

请求体使用 `Content-Type: application/x-ndjson`。当前协议边界：

- 整个 HTTP body 受 `max_body_bytes` 限制；
- 单行最大 1 MiB；
- 单请求最多 10,000 条；
- 空请求、未知字段、非法 JSON、非法维度和非有限数值均 fail-closed；
- 当前模式为**整请求原子提交**：任意记录失败时整批不写入；
- 导入通过 Writer Actor 和 Core transaction/WAL 完成原子提交与持久化。

---

## OCC、幂等与错误模型

- 全局 ETag 为 `"<epoch>-g<n>"`，成功写入后递增；
- `GET /v1/nodes/{id}` 返回节点和边 ETag；
- `If-Match`、`expectedGeneration`、`expectedNodes` 和 `expectedEdges` 提供 OCC；
- 版本不匹配返回 `409 WRITE_CONFLICT`，整笔事务拒绝；
- `Idempotency-Key` 同键同请求重放首次结果，同键不同请求返回 `409 IDEMPOTENCY_KEY_REUSED`；
- 幂等缓存和 Server generation 是进程内状态，重启后 epoch 改变。

统一错误响应包含 `code`、双语 `title`、`detail` 和 `retryable`。常见状态码包括 400、404、409、413、415、422、423、500、503 和 504。

---

## 健康、任务监管与可观测性

`/health/details` 不获取 Database 锁，可在数据库操作被阻塞时继续诊断：

- `writeQueueDepth` / `writeQueueCapacity`
- `activeReads` / `waitingReads` / `waitingWriters`
- `activeBlockingTasks`
- `writerAlive` / `writerFailed`
- `quiverWarmup`

Writer Actor 由 supervisor 监管。异常退出会设置失败状态、关闭读取容量并唤醒等待者，后续请求立即返回明确错误。

请求 `profile: true` 时，JSON 响应或 NDJSON summary 会提供：

- `elapsedMicros`：HTTP 请求总耗时；
- `queueWaitMicros`：队列或 read semaphore 等待；
- `executionMicros`：Core/blocking 执行；
- `preparedCacheHit`：是否命中 Prepared 缓存。

`/metrics` 公开写队列、合批、WAL fsync、活动/等待读、等待写、blocking 任务、Writer 状态、取消、超时和 OCC 冲突指标。access log 记录 method、path、status、elapsed、queue wait、execution、response bytes 和 cancelled；不记录完整查询参数、向量、Payload、Prepared 参数、Authorization 或幂等键。

---

## 边界与已知限制

- 内置控制台的查询、表格、JSON 与 Canvas 图视图可离线使用；WebGL、社区检测和动画按需加载 CDN，断网时回退基础显示；
- Server 仍是 nightly，不提供协议兼容承诺；
- 当前没有认证和 TLS；
- NDJSON 导入按请求预算接收数据，并以整请求为单位原子提交；
- TQL mutation 当前保守失效此前签发的全部细粒度节点/边 ETag；
- 幂等缓存与 Server generation 不持久化；
- QuIVer 手工构建仍遵守 Core 的自动构建资格，数据量不足、已就绪或配置禁用时返回 `published: false`；
- JSON 响应受 16 MiB 上限约束；NDJSON 只流式编码 HTTP 输出，Core 算子仍受自己的内存与查询预算约束；
- 安装方式和跨平台二进制以 GitHub Release 的 nightly 说明为准。

---

## 按需开发自测

Server 执行非 UI 的引擎、HTTP 数据契约与嵌入生命周期验收：

```bash
cargo test -p triviumdb-server --features test-hooks --lib engine::tests -- --test-threads=1
cargo test -p triviumdb-server --test console_embedding -- --test-threads=1
cargo test -p triviumdb-server --test http_contract -- --test-threads=1 --skip webui --skip if_none_match
```

禁止编写、新增或执行 UI/UX、样式、布局、视觉基线、动画、开屏时间及界面耗时阈值测试，也不将其用作本地、CI、合并或发布门禁。上述命令排除现存的页面测试；前端改动只做代码审查、语法检查及非 UI 数据契约验证。常规嵌入式验收覆盖 Core、SDK 与 CLI 的非 UI 公共契约。
