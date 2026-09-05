# TriviumDB Server（HTTP 服务端版）

> **版本**: v0.8.6 开发分支（nightly 预览）
> **语言**: Rust（Axum + Tokio，仅服务端 crate，嵌入式核心零依赖侵入）
> **许可**: Apache-2.0

---

## 状态声明：nightly 预览

**TriviumDB 的正式产品形态仍是嵌入式数据库（Embedded Database）。** Python、Node.js 和 Rust 用户应优先使用正式嵌入式包：

```bash
pip install triviumdb
npm install triviumdb
cargo add triviumdb
```

`triviumdb-server` 目前处于 nightly 预览状态：

- HTTP 路由、字段和错误码仍可能调整；
- 尚未发布到 crates.io，也未承诺长期兼容性；
- 一切核心 API、存储格式和查询语义仍以嵌入式版本为准；
- 当前没有认证和 TLS，不应直接暴露到公网；
- 生产环境仍不建议依赖 nightly Server。

Server 适合提前体验多客户端访问、Writer Actor、Group Commit、OCC、健康监管、流式响应和批量导入。

---

## 架构与并发模型

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

- Server 和 Embedded 共用同一个 `triviumdb` crate，没有第二套存储引擎；
- Axum/Tokio 等依赖只存在于 `crates/triviumdb-server`，不会进入嵌入式用户依赖树；
- Core 的单写多读契约不变，Server 将并发客户端写入串行化；
- Writer 等待时新读不会插队；读等待使用通知唤醒，不做忙轮询；
- 读、写、预热和手工 QuIVer 构建均在受监管的 blocking 任务中运行；
- 请求断开或 deadline 到期会传播取消信号；已经进入 durable commit 的写入不会被中断。

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

---

## 配置参考

所有参数同时支持命令行和环境变量，优先级为 **命令行 > 环境变量 > 默认值**。

| CLI 参数 | 环境变量 | 默认值 | 说明 |
|---|---|---:|---|
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
|---|---|---|
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

请求头设置 `Accept: application/x-ndjson` 时，响应按 `meta → row... → summary` 输出。HTTP 输出逐行生成，但排序、聚合和 Top-K 等 Core 算子仍可能先物化结果。

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
|---|---:|---|
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
- 导入通过 Writer Actor 和 Core transaction/WAL 提交，不是绕过持久化的快速旁路。

---

## OCC、幂等与错误模型

- 全局 ETag 为 `"<epoch>-g<n>"`，成功写入后递增；
- `GET /v1/nodes/{id}` 返回节点和边 ETag；
- `If-Match`、`expectedGeneration`、`expectedNodes` 和 `expectedEdges` 提供 OCC；
- 版本不匹配返回 `409 WRITE_CONFLICT`，事务不会部分提交；
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

Writer Actor 由 supervisor 监管。异常退出会设置失败状态、关闭读取容量并唤醒等待者，后续请求快速返回明确错误，而不是永久挂起。

请求 `profile: true` 时，JSON 响应或 NDJSON summary 会提供：

- `elapsedMicros`：HTTP 请求总耗时；
- `queueWaitMicros`：队列或 read semaphore 等待；
- `executionMicros`：Core/blocking 执行；
- `preparedCacheHit`：是否命中 Prepared 缓存。

`/metrics` 公开写队列、合批、WAL fsync、活动/等待读、等待写、blocking 任务、Writer 状态、取消、超时和 OCC 冲突指标。access log 记录 method、path、status、elapsed、queue wait、execution、response bytes 和 cancelled；不记录完整查询参数、向量、Payload、Prepared 参数、Authorization 或幂等键。

---

## 边界与已知限制

- Server 仍是 nightly，不提供协议兼容承诺；
- 当前没有认证和 TLS；
- NDJSON 导入是有界整请求原子导入，并非无限流式、逐批提交协议；
- TQL mutation 当前保守失效此前签发的全部细粒度节点/边 ETag；
- 幂等缓存与 Server generation 不持久化；
- QuIVer 手工构建仍遵守 Core 的自动构建资格，数据量不足、已就绪或配置禁用时返回 `published: false`；
- JSON 响应受 16 MiB 上限约束；NDJSON 只流式编码 HTTP 输出，Core 算子仍受自己的内存与查询预算约束；
- 安装方式和跨平台二进制以 GitHub Release 的 nightly 说明为准。
