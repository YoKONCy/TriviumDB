# TriviumDB Server（HTTP 服务端版）

> **版本**: v0.8.5（nightly 预览）
> **语言**: Rust（Axum + Tokio，仅服务端 crate，嵌入式核心零依赖侵入）
> **许可**: Apache-2.0

---

## ⚠️ 状态声明：nightly 预览

**TriviumDB 的正式产品形态是嵌入式数据库（Embedded Database）。** Python / Node.js / Rust 用户应继续以嵌入式方式使用：

```bash
pip install triviumdb     # Python
npm install triviumdb     # Node.js
cargo add triviumdb       # Rust
```

`triviumdb-server` 目前处于 **nightly 预览状态**：

- 协议（路由、字段、错误码）可能在不通知的情况下变更；
- 并发模型仍在迭代，尚未承诺任何稳定性保证；
- 尚未发布到 crates.io，也未承诺长期支持；
- **一切 API、语义与用法说明仍以嵌入式版本为准**，本文档仅为尝鲜者提供上手参考；
- 生产环境请勿依赖 nightly Server。

它适合两类读者：

1. 想提前体验「多客户端并发读写一个 TriviumDB 文件」的开发者；
2. 想跟踪 Server 演进方向（Writer Actor、Group Commit、OCC、流式协议）的社区成员。

---

## 目录

- [架构与定位](#架构与定位)
- [构建与运行](#构建与运行)
- [配置参考（CLI 参数与环境变量）](#配置参考cli-参数与环境变量)
- [HTTP API 一览](#http-api-一览)
- [并发模型](#并发模型)
- [OCC 与 ETag/If-Match](#occ-与-etagif-match)
- [幂等写](#幂等写)
- [可观测性（日志 / 指标 / request ID）](#可观测性日志--指标--request-id)
- [边界与已知限制](#边界与已知限制)

---

## 架构与定位

```text
HTTP / JSON / NDJSON / 二进制向量
        │
┌───────┴────────────────────────────┐
│ triviumdb-server (Axum + Tokio)    │
│  ├─ 并发读：semaphore + blocking   │
│  ├─ 写：bounded queue → Writer     │
│  │        Actor → Group Commit     │
│  └─ OCC / 幂等 / 指标 / 日志       │
├────────────────────────────────────┤
│ triviumdb（嵌入式核心，同一份代码） │
└────────────────────────────────────┘
```

要点：

- Server 与嵌入式核心共用同一个 `triviumdb` crate，没有第二套存储实现；
- Server 依赖（Axum/Tokio 等）**只存在于 `crates/triviumdb-server`**，嵌入式用户 `cargo add triviumdb` 不会引入任何服务端依赖；
- 嵌入式核心保持「单写多读」契约不变；Server 通过 Writer Actor 把多个客户端写请求串行化为确定性的提交序列。

---

## 构建与运行

需要 Rust 工具链：

```bash
cargo build --release -p triviumdb-server
```

运行：

```bash
./target/release/triviumdb-server \
  --database /var/lib/triviumdb/main.tdb \
  --listen 0.0.0.0:8080 \
  --dim 1536
```

健康检查：

```bash
curl http://127.0.0.1:8080/health/live
curl http://127.0.0.1:8080/health/ready
```

优雅关闭：`Ctrl+C` 或 `SIGTERM`。

---

## 配置参考（CLI 参数与环境变量）

所有参数同时支持命令行与环境变量，优先级为 **命令行 > 环境变量 > 默认值**。

| CLI 参数 | 环境变量 | 默认值 | 说明 |
|---|---|---|---|
| `--log-format` | `TRIVIUMDB_LOG_FORMAT` | `pretty` | 日志格式：`pretty` / `json` |
| `--database` | `TRIVIUMDB_DATABASE` | `triviumdb-server.tdb` | 数据库文件路径 |
| `--listen` | `TRIVIUMDB_LISTEN` | `127.0.0.1:8080` | 监听地址 |
| `--dim` | `TRIVIUMDB_DIM` | `1536` | 向量维度 |
| `--max-query-rows` | `TRIVIUMDB_MAX_QUERY_ROWS` | `10000` | 查询行上限，0 为不限 |
| `--memory-limit` | `TRIVIUMDB_MEMORY_LIMIT` | `0` | 内核内存上限（字节），0 为不限 |
| `--write-queue-capacity` | `TRIVIUMDB_WRITE_QUEUE_CAPACITY` | `256` | 有界写队列容量 |
| `--max-concurrent-reads` | `TRIVIUMDB_MAX_CONCURRENT_READS` | `8` | 最大并发读 |
| `--idempotency-capacity` | `TRIVIUMDB_IDEMPOTENCY_CAPACITY` | `4096` | 幂等缓存容量，0 为关闭 |
| `--max-write-batch-size` | `TRIVIUMDB_MAX_WRITE_BATCH_SIZE` | `64` | Group Commit 批量上限 |
| `--max-write-batch-delay-us` | `TRIVIUMDB_MAX_WRITE_BATCH_DELAY_US` | `500` | 动态合批等待窗口（微秒） |
| `--prepared-cache-capacity` | `TRIVIUMDB_PREPARED_CACHE_CAPACITY` | `1024` | Prepared 缓存容量 |
| `--request-timeout-ms` | `TRIVIUMDB_REQUEST_TIMEOUT_MS` | `30000` | 请求 deadline（毫秒） |
| `--max-body-bytes` | `TRIVIUMDB_MAX_BODY_BYTES` | `4194304` | HTTP 请求体上限（字节） |

日志级别使用 tracing 标准变量：

```bash
RUST_LOG=triviumdb_server=info,triviumdb=warn ./triviumdb-server
```

---

## HTTP API 一览

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/health/live` | 存活检查 |
| GET | `/health/ready` | 就绪检查（Writer Actor 故障时返回 503） |
| GET | `/metrics` | Prometheus 文本指标 |
| POST | `/v1/tql` | TQL 查询 / 写入（`mutation: true`） |
| POST | `/v1/prepared` | 创建 Prepared 查询，返回 `preparedId` |
| POST | `/v1/prepared/{id}/execute` | 执行 Prepared 查询 |
| POST | `/v1/search/vector` | 二进制 f32 向量检索 |
| POST | `/v1/transactions` | 多操作原子事务（含 OCC precondition） |
| GET | `/v1/nodes/{id}` | 节点详情 + 节点/边 ETag |

示例——写入与查询：

```bash
curl -X POST http://127.0.0.1:8080/v1/tql \
  -H 'content-type: application/json' \
  -d '{"query": "CREATE ({name: \"Alice\"})", "mutation": true}'

curl -X POST http://127.0.0.1:8080/v1/tql \
  -H 'content-type: application/json' \
  -d '{"query": "FIND {name: \"Alice\"} RETURN *"}'
```

流式响应：请求头加 `Accept: application/x-ndjson`，逐行返回 `meta → row... → summary` 帧。

请求级 profile：请求体加 `"profile": true`，响应携带 `elapsedMicros` 等字段；`EXPLAIN ANALYZE` 语句可直接通过 `/v1/tql` 执行。

错误模型：统一 JSON 结构（`code` / 双语 `title` / `detail` / `retryable`），使用标准 HTTP 状态码（400 / 404 / 409 / 413 / 415 / 422 / 423 / 500 / 503 / 504）。

---

## 并发模型

- **并发读**：请求在 blocking worker 中执行，read semaphore 限制并发上限；
- **串行写**：所有写进入有界队列，由唯一 Writer Actor 依序提交，饱和时返回 `503 WRITE_QUEUE_FULL`；
- **公平性**：写者等待时新读不插队，写者拿到全部读许可后才提交；
- **deadline**：排队、等许可、执行全程计入请求超时，过期返回 `504 REQUEST_TIMEOUT`；
- **取消**：客户端断开且写尚未开始时不产生副作用；已开始的 durable 提交不会被中断；
- **Group Commit**：相邻写请求动态合批，整批共享一次 WAL fsync，单请求业务失败不影响同批其他请求。

---

## OCC 与 ETag/If-Match

- 全局 generation ETag：`"<epoch>-g<n>"`，任何成功写入后递增；
- 节点/边 ETag：从 `GET /v1/nodes/{id}` 获取；
- 写请求可带 `If-Match`（或事务 body 中的 `expectedGeneration` / `expectedNodes` / `expectedEdges`）做乐观并发控制；
- 版本不匹配返回 `409 WRITE_CONFLICT`，冲突事务不会部分写入；
- 进程重启后 epoch 变化，旧 ETag 自动失效（返回 409），不会误判为匹配。

---

## 幂等写

写请求可携带 `Idempotency-Key` Header：

- 同键同请求：重放首次结果（响应 `replayed: true`）；
- 同键不同请求：返回 `409 IDEMPOTENCY_KEY_REUSED`；
- 缓存有界（FIFO 淘汰），仅进程内，重启不保留。

---

## 可观测性（日志 / 指标 / request ID）

- `--log-format json` 输出结构化日志，便于采集；
- 每个请求自动生成或透传 `X-Request-ID`，并回写到响应头；
- access log 记录 method / path / status / elapsed / queue wait / execution / response bytes / cancelled，**不含**查询文本、向量、Payload、参数或幂等键；
- `/metrics` 暴露队列深度、合批、fsync、取消、超时、OCC 冲突等计数，可直接被 Prometheus 抓取；
- 进程后台化交给 systemd / Docker / 服务管理器，Server 自身不 daemon 化。

---

## 边界与已知限制

- TQL mutation 目前保守失效所有细粒度节点/边 ETag；真正的细粒度写集追踪在路线图中；
- 幂等缓存与 generation 均为进程内状态，不持久化；
- NDJSON 为 HTTP 层流式，排序 / 聚合 / Top-K 算子仍需先物化；
- 尚无认证与 TLS，请勿直接暴露到公网；
- 交叉编译与二进制发布流水线尚在 nightly 演进中，安装方式以 GitHub Release 说明为准。
