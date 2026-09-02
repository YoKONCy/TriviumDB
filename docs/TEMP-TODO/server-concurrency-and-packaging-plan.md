# TriviumDB Server 与并发写演进设计（TEMP）

> 状态：临时规划文档，尚未进入实现阶段。本文记录 TriviumDB Embedded 与 TriviumDB Server 的产品边界、发包结构，以及从单写多读逐步演进到服务端并发写的路线。

## 一、目标与基本判断

TriviumDB 当前核心采用单写多读模型。嵌入式调用方负责串行化写操作，这一模型具有实现简单、确定性强、恢复路径清晰、常驻开销低等优点。

未来若需要面向多客户端提供并发写能力，不应立即把完整 MVCC 强加给所有嵌入式用户。更合理的路线是：

1. Embedded 永久保留轻量的单写多读模型；
2. 新增独立的 TriviumDB Server 包；
3. Server 初期接收并发写请求，内部通过 Writer Actor 串行提交；
4. 随后加入 Group Commit 与乐观并发控制（OCC）；
5. 仅在真实业务证明 Snapshot Isolation 必不可少时，再实现 Server 专享的完整 MVCC。

必须准确区分：

- **多客户端并发写请求**：多个客户端可以同时发起写入，由服务端协调为确定性提交序列；
- **存储核心并行多写/MVCC**：多个事务维护独立版本与快照，并在提交时进行冲突检测。

初版 Server 提供前者，不应宣传为 MVCC 或并行提交。

## 二、同一 master、同一 workspace、独立发包

Embedded 与 Server 使用同一个 Git `master` 分支和 Cargo workspace，但作为独立 crate 发布，不维护长期代码分支，也不复制数据库核心。

建议目录：

```text
TriviumDB/
├─ Cargo.toml
├─ src/                         # triviumdb 核心库
├─ crates/
│  └─ triviumdb-server/
│     ├─ Cargo.toml
│     └─ src/
│        ├─ main.rs
│        ├─ writer.rs
│        ├─ routes.rs
│        ├─ middleware.rs
│        └─ protocol.rs
├─ python/                      # Python Embedded binding
├─ tests/
└─ docs/
```

根 workspace 建议：

```toml
[workspace]
members = [
    ".",
    "crates/triviumdb-server",
]
default-members = ["."]
resolver = "2"
```

`default-members = ["."]` 保证仓库根目录的普通 `cargo build`、`cargo test` 仍默认只处理核心；维护者使用 `--workspace` 时才构建和测试 Server。

服务端 crate 单向依赖 Core：

```toml
[dependencies]
triviumdb = { path = "../..", version = "0.8.5" }
axum = "..."
tokio = { version = "...", features = ["rt-multi-thread", "macros"] }
tower-http = "..."
```

发布时，本地开发通过 `path` 使用同仓代码，crates.io 消费者通过 `version` 解析已发布 Core。

## 三、依赖隔离

依赖关系必须保持：

```text
triviumdb-server → triviumdb core
```

禁止 Core 反向依赖 Server。核心 crate 不应新增：

- Axum；
- Tokio；
- Tower/Hyper；
- TLS/JWT；
- Prometheus/OpenTelemetry Server 集成；
- OpenAPI Web 框架。

Server 不应启用 Python/Node binding feature，也不应依赖 PyO3 或 napi-rs。

最终依赖图：

```text
                   ┌────────────────────┐
                   │  triviumdb core    │
                   │ WAL/TQL/QuIVer/... │
                   └─────────┬──────────┘
                 ┌───────────┼────────────┐
                 │           │            │
       ┌─────────▼───┐ ┌─────▼─────┐ ┌────▼──────────────┐
       │ Python wheel│ │ Node addon │ │ triviumdb-server │
       │ PyO3        │ │ napi-rs    │ │ Axum/Tokio       │
       └─────────────┘ └───────────┘ └────────┬──────────┘
                                              │ HTTP/gRPC
                                    ┌─────────┼──────────┐
                                    │         │          │
                                  Python     Node       Go/Java
                                  client     client     client
```

因此：

- Rust Embedded 用户不会编译 Server 依赖；
- Python/Node Embedded 包不会包含 Server；
- Server 不会包含 Python/Node 原生绑定；
- 位于同一个 workspace 不会让不同成员自动共享依赖。

不建议把 Server 做成 Core 的 Cargo feature。否则 `--all-features`、绑定构建、发布元数据和 CI feature 矩阵都会被不必要地扩大。

## 四、发包与版本策略

初期建议锁步版本：

```text
crates.io: triviumdb        0.8.x
crates.io: triviumdb-server 0.8.x
```

使用同一个正式 tag，例如 `v0.8.5`，以便明确 Server 与 Core 的兼容关系。发布顺序必须为：

1. 完成 Core 与 Server 的全部 Release Gate；
2. 发布 `triviumdb`；
3. 等待 crates.io 索引可见；
4. 发布 `triviumdb-server`；
5. 构建并发布 Server 预编译二进制；
6. 可选发布 Docker/OCI 镜像。

Server 用户通常更需要预编译二进制与容器镜像，而不只是 Rust crate：

```bash
cargo install triviumdb-server
triviumdb-server --database ./data.tdb --listen 127.0.0.1:8080
```

后续若 Server 发布节奏与 Core 明显分离，再改为独立版本与 tag，例如：

```text
core-v0.9.0
server-v0.3.0
```

初期不增加这项复杂度。

发布系统必须避免再次发生同名 tag 移动后包内容不一致的问题：

- 正式 tag 不允许移动；
- CD 必须等待对应 commit 的完整 CI 成功；
- Release Summary 必须区分“已发布”“已有版本而跳过”“前置 Gate 未运行”；
- Server 二进制记录 Server 版本、Core 版本、Git commit 与格式版本；
- 部分 registry 发布失败时明确报告状态，不得显示误导性的全绿成功。

## 五、Server 初版并发模型

### 5.1 Writer Actor

初版 Server 对外接收多个并发写请求，内部使用有界 channel 和专用 Writer Actor 串行提交：

```text
多个 HTTP/gRPC 客户端
        ↓
认证、配额、超时、请求校验
        ↓
有界写队列与背压
        ↓
Writer Actor
        ↓
TriviumDB 单写事务与 WAL
```

Writer Actor 负责：

1. 解析并验证事务命令；
2. 建立 Core 事务；
3. 写 WAL；
4. 更新 MemTable、Payload、向量、边与属性索引；
5. 维护 QuIVer 增量状态；
6. 触发 Hook；
7. 返回事务结果。

服务端可以准确宣传：

> 支持多客户端并发写请求，由服务端协调为确定性的事务提交序列。

不得称为 MVCC 或存储层并行写。

### 5.2 Group Commit

多个相邻事务可在不破坏事务原子性的前提下，共享批量 WAL append/fsync：

```text
Tx A ─┐
Tx B ─┼─ 批量 WAL 写入 + 单次 fsync → 分别提交/响应
Tx C ─┘
```

Group Commit 需要保持：

- 每个事务独立的 TxBegin/TxCommit；
- 稳定提交顺序；
- 单个事务失败不污染其他事务；
- power-loss 后恢复结果与已确认响应一致；
- 不绕过 Hook、索引和 generation 语义。

### 5.3 并发读

初版可选两种内部结构：

1. 所有操作由一个 Owner Actor 执行：简单但读请求也排队；
2. Writer Actor + 并发只读 handle/snapshot：更符合单写多读，应作为正式方向。

如果 Core 当前缺少安全的共享读句柄，只增加最薄的通用 Core 接口，不把 HTTP/Server 逻辑放入 Core。

## 六、OCC 中间阶段

在完整 MVCC 前，Server 可实现乐观并发控制：

```text
BEGIN/读取
→ 记录数据库或节点 generation
→ 客户端并行计算写集合
→ COMMIT 时校验版本
→ 无冲突则串行提交
→ 有冲突则返回 SerializationFailure/409
```

节点可维护：

```text
last_modified_generation
```

HTTP 可映射为：

```text
GET /v1/nodes/42
ETag: "generation-183"

PATCH /v1/nodes/42
If-Match: "generation-183"
```

OCC 的性质：

- 多个客户端可以并行读取和准备事务；
- 最终提交仍按单一序列执行；
- 可以检测 write-write 和部分 read-write 冲突；
- 不保留任意历史版本；
- 不承诺长生命周期 Snapshot Isolation。

这能覆盖大量 Agent 场景中的“读取、思考、条件更新”需求，复杂度远低于完整 MVCC。

## 七、完整 MVCC 的远期范围

完整 MVCC 并非 Axum 中间件可以实现，它必须深入存储核心。即使只由 Server 对外启用，以下能力仍需进入 Core 下层或独立 VersionStore：

### 7.1 Payload 版本链

```text
NodeId -> [
    {begin_ts, end_ts, payload_v1},
    {begin_ts, end_ts, payload_v2},
    ...
]
```

删除必须形成 tombstone version，不能立即物理移除。

### 7.2 向量版本

旧 reader 必须继续读取旧向量；VecPool 不能立即原地覆盖、清零或复用仍被 snapshot 引用的 slot。

### 7.3 QuIVer 可见性

ANN 候选需要版本/提交时间信息。过滤不可见候选后可能不足 K 个结果，因此需要超额召回、继续探索或版本感知重建，不能只在最终结果上加一次时间戳判断。

### 7.4 图边版本

边创建、删除、标签与权重更新都需要 `begin_ts/end_ts`。同一个 MATCH 必须在一致 snapshot 中遍历节点和边。

### 7.5 属性索引版本

Hash、Ordered、Composite 与 Bitmap Posting 需要版本化，或采用 Base + Delta + Tombstone，并在查询时执行 snapshot visibility filter。

### 7.6 WAL 与恢复

WAL 需要事务 ID、snapshot timestamp、commit timestamp、版本链、冲突结果和 vacuum publication。恢复必须保证：

- 未提交版本不可见；
- 已确认提交原子可见；
- commit timestamp 不复用；
- 数据、索引和向量版本属于同一代际。

### 7.7 Vacuum/GC

只有满足以下条件的旧版本才能回收：

```text
end_ts < oldest_active_snapshot
```

需要同时清理 Payload、向量 slot、属性 Posting、图边版本、QuIVer 节点、WAL 历史与 mmap 旧块。

因此完整 MVCC 属于远期大型存储项目，不应作为 Server MVP 的前置条件。

## 八、网络协议与多语言访问

Server 本身是 Rust 后端，不需要 Python、Node、Go、Java 原生数据库绑定。多语言统一通过 HTTP/gRPC 协议访问。

初版可只提供：

- HTTP JSON；
- OpenAPI；
- curl 示例。

后续网络 SDK 是纯语言客户端，不包含 TriviumDB Core、QuIVer、WAL、mmap 或原生 addon：

```text
Rust:   triviumdb-client
PyPI:   triviumdb-client
NPM:    @triviumdb/client
```

嵌入式与网络客户端必须显式分离：

```python
from triviumdb import TriviumDB              # Embedded
from triviumdb_client import TriviumClient   # Server
```

禁止通过 `TriviumDB("http://...")` 静默切换本地和网络模式。

网络协议需要明确：

- API version；
- TQL 请求与 Prepared 参数格式；
- `TqlValue` 中 Node、Path、List、Null 的 JSON 表示；
- 错误代码与可重试性；
- timeout/cancellation；
- row/memory/step budget；
- transaction ID；
- idempotency key；
- pagination/cursor；
- streaming export；
- 鉴权与 namespace。

NodeId 是 `u64`，在 JSON 中必须使用字符串，避免 JavaScript 超过 `2^53 - 1` 后丢失精度：

```json
{
  "id": "18446744073709551615"
}
```

## 九、数据格式兼容

初版 Server 与 Embedded 使用完全相同的数据库格式：

```text
Embedded 创建 → 完全关闭 → Server 可打开
Server 创建   → 完全停止 → Embedded 可打开
```

文件锁继续禁止两个进程同时持有写权限。Writer Actor 不应引入新的磁盘格式。

Server 不能直接编辑 WAL、MemTable、`.tdb/.vec`、`.pidx` 或 QuIVer sidecar，所有存储操作必须经过 Core 公共接口。

远期 Server 专享 MVCC 可考虑在稳定 Base 上增加版本化 sidecar：

```text
.tdb/.vec   稳定基础代际
.mvcc       Payload/Vector 版本增量
.midx       版本化索引增量
```

任何格式升级必须显式、可检测、可迁移，不允许 Server 静默把 Embedded 数据库升级为不可回退格式。

## 十、Server MVP 边界

建议第一版只包含：

- Axum HTTP JSON；
- 单数据库实例；
- TQL 查询；
- 基础节点/边写接口或 TQL mutation；
- Writer Actor；
- 有界队列与背压；
- request timeout/cancellation；
- graceful shutdown；
- health/readiness；
- 基础 tracing/metrics；
- 请求级 row/memory/step budget；
- 幂等写请求键；
- 完整故障恢复与跨进程测试。

第一版明确不包含：

- 完整 MVCC；
- 长事务 Snapshot Isolation；
- 多节点集群；
- 分片与复制；
- 自动主从切换；
- Server 专用磁盘格式；
- Python/Node 原生 Server 绑定；
- 在 Core 中引入 Axum/Tokio。

## 十一、推荐实施顺序

1. 将仓库声明为 workspace，并保持 Core 为 default member；
2. 创建独立 `triviumdb-server` crate；
3. 定义稳定的协议值模型和错误模型；
4. 实现单 Owner Actor 的最小端到端原型；
5. 拆分 Writer Actor 与并发只读路径；
6. 加入有界队列、背压、超时与取消；
7. 加入 Group Commit，并完成 power-loss 验证；
8. 完成 HTTP 公共契约、OpenAPI 与跨平台 Server CI；
9. 独立发布 Server crate、二进制和容器镜像；
10. 基于真实冲突负载加入 OCC；
11. 基于真实 Snapshot Isolation 需求评估完整 MVCC。

## 十二、验收原则

Server 的新增能力不得破坏现有核心约束：

- Embedded 依赖树不引入 Server 库；
- ReadOnly/Immutable 继续保持零写；
- 单一 Writer 的提交顺序确定；
- 查询与容量预算 fail-closed；
- 队列满时明确背压，不允许无限内存增长；
- 客户端断开不得留下幽灵事务；
- 已确认写入在 power-loss 恢复后必须存在；
- 未确认或未提交事务不得部分可见；
- Group Commit 不改变事务原子性；
- NodeId 网络编码无精度损失；
- Embedded 与初版 Server 数据格式可互换；
- 故障测试不得耗尽宿主机物理内存或磁盘。

## 十三、最终决策摘要

- Embedded 与 Server 放在同一个 `master` 和 Cargo workspace；
- 分别发布 `triviumdb` 与 `triviumdb-server`；
- Server 是独立 crate，不是 Core feature；
- Server 不需要其他语言原生绑定；
- Embedded 不引入 Axum/Tokio 等 Server 依赖；
- 初版 Server 通过 Writer Actor 提供多客户端并发写请求；
- 下一阶段优先 OCC，而不是直接实现完整 MVCC；
- 完整 MVCC 仅在真实需求证明必要时作为 Server 专享能力推进；
- 即使 Server 专享 MVCC，其版本存储、索引可见性、WAL 与 Vacuum 仍属于存储层工程，不能只靠外部 Web 壳实现。
