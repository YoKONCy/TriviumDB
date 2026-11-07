# 嵌入式 Console 宿主接入

> 本文说明嵌入式 Console 的宿主协议、权限、文件锁、生命周期和前端接入。适用范围为以 `0.9.0` 为基线的当前开发版本，新增编辑协议见[未发布变更](changes/unreleased.md)。

---

## 目录

- [接入概述](#接入概述)
- [文件锁、借用和关闭](#文件锁借用和关闭)
- [Rust](#rust)
- [Python](#python)
- [Node.js](#nodejs)
- [请求、权限和数据表示](#请求权限和数据表示)
- [编辑快照、冲突与原事务确认](#编辑快照冲突与原事务确认)
- [错误与写入确认](#错误与写入确认)
- [宿主自定义桥接](#宿主自定义桥接)
- [资源交付与组件挂载](#资源交付与组件挂载)

---

## 接入概述

版本保护事务与回执确认属于[未发布更新](changes/unreleased.md)，使用时需构建包含这些更新的开发版本。Rust、Python、Node 的宿主协议属于嵌入式接口；HTTP 适配及现有 WebUI 继续采用 nightly 开发通道。

Console v1 为 Rust、Python 和 Node.js 提供同一份 JSON 请求协议，直接操作宿主已经打开的 Embedded Database。HTTP 适配位于独立的 nightly Server crate，桌面 IPC 可原样转发字符串；宿主按需引入相应框架。

宿主接入包含协议、权限、生命周期、HTTP 适配和资源打包。HTML 保留独立 Server HTTP v1 模式，并通过内置客户端支持 Console RPC v1。宿主使用 `console-embed.js` 将页面挂载为同源 iframe；容器、会话和数据库由宿主管理。

---

## 文件锁、借用和关闭

### 实例与文件锁

- 文件锁属于宿主 Core 实例及其仍存活的存储、已准入任务；文件租约持续到相关 mmap、句柄和任务全部释放。`ConsoleSession` 复用已有实例；Rust `ConsoleHost::attach_shared` 接收 `SharedDatabase<T>`，`attach` 兼容已有的 `Arc<Mutex<Database<T>>>`，两者均保存弱引用。
- Rust 宿主的业务访问与 Console 共用同一 SDK 句柄或同一个外层 `Mutex`。锁竞争返回 `TDB_BUSY`，详情通过 `resource` 标识资源；SDK 识别出的同线程重入返回 `TDB_REENTRANT_ACCESS`。宿主锁中毒返回 `TDB_HOST_LOCK_POISONED`。所有权和分层见[内核 SDK 与存储内核](kernel-sdk.md)。
- 会话在第一次请求时绑定具体实例身份，Rust `ConsoleHost` 在 `attach` 时完成绑定。绑定身份由实例弱引用确定；重新打开数据库后需创建新会话。

> **文件锁提醒**：宿主和 Console 应复用同一实例。对同一文件另开可写 Server 会产生锁冲突。

### 请求调度与取消

- detach 阻止新请求并发出取消信号。数据库持久化与关闭由宿主调度。多个独立会话各自管理状态；同一会话的克隆共享卸载及取消状态。
- `cancel(id)` 的布尔值确认取消信号是否发给活动请求。`is_drained()`／`isDrained()` 在会话已 detach 且所有 Core 请求实际退出后返回 `true`，新建的开放会话返回 `false`。状态查询仅访问会话自身的锁。
- TQL 读取使用内核 QueryControl 协作取消和 deadline。其余操作在支持的检查点检查取消；向量 Hook、索引构建及已进入提交的写入不能被强制中断。`info.transport.cooperativeCancellation` 明确列出协作取消的操作。
- HTTP 客户端退出会传递取消信号。blocking 任务持有的并发许可在实际执行退出后归还。

主动取消使用 `TDB_QUERY_CANCELLED`，期限到达使用 `TDB_QUERY_DEADLINE_EXCEEDED`，两者详情为空对象。同一检查点同时观察到取消与期限到达时，期限优先。HTTP 请求体期限返回 408，执行中的期限返回 504。命令分发前的写请求终止提供 `admission/not_committed`，四类事务／导入在 WAL 前准备完成处终止提供 `validation/not_committed`。资源归还以真实执行退出为完成点，详见[公共错误契约](error-contracts.md#取消与期限)。

### 宿主关闭顺序

宿主先停止入口并调用 `detach`，再在自己的串行调度中调用 `Database.close`。关闭成功后释放文件锁，调用方可继续持有会话、Router、Prepared 或关闭后的 Database 对象。

宿主可用 `is_drained()` 确认会话执行排空，嵌入 HTTP 对应 `GET /v1/console/status` 和 `{"drained": bool}`。HTTP 请求体、适配任务与响应消费按传输层任务记录完成点，数据库文件资源以显式关闭结果确认。

> **关闭提醒**：准入或持久化失败时保留锁和原实例，处理原因后重试。持久化完成后的 Hook 析构异常仍完成资源释放，此时实例已为 `closed`。宿主结合关闭阶段判断资源状态，详见[生命周期契约](lifecycle-snapshots.md)。

### 绑定调用与借用

- Python 请求执行期间释放 GIL，PyO3 继续保持实例的独占借用。宿主用同一个锁或队列协调业务访问和关闭。Node 请求同步执行；公共 `index.js` 在进入 N-API 前以 `TDB_REENTRANT_ACCESS` 拒绝同步回调期间的方法及 getter 重入。
- 数据库互斥锁的借用范围限定为同步内核调用，在 await、网络发送或等待对端 IPC 前释放。数据库路径、关闭及文件操作由宿主直接管理。

Python 数据库方法与 Console 请求共用显式借用准入：同库回调重入返回 `TDB_REENTRANT_ACCESS`，其他线程的借用争用返回 `TDB_BUSY`。失败请求退出后，原持有者继续执行；事务提交的准入拒绝保留 builder。

> **绑定提醒**：回调中的同库访问由宿主在当前调用结束后调度。Node 通过公共加载器使用原生模块；Python 参数转换及其他包装对象的借用错误继续按各自契约处理，范围见[SDK 指南](kernel-sdk.md#现有入口与后续收敛)。

---

## Rust

```rust
use triviumdb::{ConsoleHost, ConsoleOptions, SharedDatabase};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database = SharedDatabase::<f32>::open("app.tdb", 768)?;
    let console = ConsoleHost::attach_shared(&database, ConsoleOptions::default())?;
    let raw = r#"{"version":1,"requestId":"q1","command":{"operation":"info"}}"#;
    let response = console.request(raw)?;
    console.session().detach();
    database.close()?;
    Ok(())
}
```

Rust 也可在自己的同步借用范围内直接调用 `database.console_request(&session, raw)`。支持 f32、f16、u64 以及 Database 的 ReadWrite、ReadOnly、Immutable 模式；会话不能提升数据库本身的访问权限。

### HTTP 适配

HTTP 接入复用相同的 `ConsoleHost`：

```rust
use triviumdb_server::console::{build_console_app, ConsoleHttpOptions};
let app = build_console_app(console, ConsoleHttpOptions::default())?;
// 可通过宿主 Router::nest 设置前缀，并在外层添加自己的认证和跨域策略。
```

可执行示例见 [embedded_console.rs](../crates/triviumdb-server/examples/embedded_console.rs)。适配器复用宿主实例，提供以下路由：

| 路由 | 用途 |
| --- | --- |
| `POST /v1/console` | 执行 JSON RPC，成功返回 `application/json` |
| `POST /v1/console/cancel/{requestId}` | 请求取消，返回 `cancelled` 布尔值 |
| `GET /v1/console/contract` | 获取版本化操作清单与传输约定 |

HTTP 并发额度默认 8，允许 1..=1024；满额返回 `503/TDB_BUSY`，`details.resource=console_in_flight`，先准入再读取请求体。请求体上限使用会话的 maxRequestBytes，读取请求体与内核工作共享同一个 deadline。宿主保持 Database 或 SharedDatabase 的强引用；最后一个强引用释放后 Router 返回 `410/TDB_DATABASE_CLOSED`。

---

## Python

```python
import json
import threading
import triviumdb

db = triviumdb.TriviumDB("app.tdb", dim=768)
session = triviumdb.ConsoleSession()  # 默认只读策略
gate = threading.Lock()

def handle_console(raw: str) -> str:
    if not gate.acquire(blocking=False):
        raise BlockingIOError("宿主数据库繁忙 (Host database is busy)")
    try:
        return db.console_request(session, raw)
    finally:
        gate.release()

response = handle_console(json.dumps({"version": 1, "requestId": "q1", "command": {"operation": "info"}}))
session.detach()
with gate:
    db.close()
```

---

## Node.js

```javascript
const { TriviumDB, ConsoleSession } = require('triviumdb')
const db = new TriviumDB('app.tdb', { dim: 768 })
const session = new ConsoleSession()
const response = db.consoleRequest(session, JSON.stringify({
  version: 1, requestId: 'q1', command: { operation: 'info' },
}))
session.detach()
db.close()
```

Python/Node 提供同步 JSON 调用入口。HTTP 或 IPC 包装、端口监听、对象生命周期、异步任务调度及应用级请求授权由宿主负责。

---

## 请求、权限和数据表示

### 会话配置

会话配置由可信宿主创建，不从前端请求读取。三端使用相同字段，Python/Node 构造器接收配置 JSON 字符串：

| 字段 | 默认值 | 边界 |
| --- | --- | --- |
| `readOnly` | `true` | `false` 仍受 Database AccessMode 限制 |
| `features` | `query`、`vector`、`graph`、`indexes`、`transactions`、`import` | 枚举，未知值拒绝 |
| `maxRequestBytes` | `4194304` | `1..=67108864`，单位为字节 |
| `maxResponseBytes` | `16777216` | `1..=67108864`，单位为字节 |
| `maxOperations` | `1000` | `1..=1000`，限制读取工作集行数、批量记录、写计划、显式 `topK` 及图预算 |
| `queryTimeoutMs` | `30000` | `1..=300000`，单位为毫秒；受外部 deadline 约束 |

`features` 控制宿主开放的操作入口。例如开放 `query` 后，读 TQL 可以包含向量和图算子。只读权限由会话与内核共同执行。

> **权限提醒**：表级、算子级或更细粒度的数据权限由宿主控制，包括限制可执行查询。

### 请求结构

请求必须包含 `version=1`、非空且最多 128 UTF-8 字节的 `requestId`，以及 `command` 对象。`requestId` 用于响应关联和取消；同一活跃 ID 会被拒绝。未知字段、命令、协议版本和不合法参数均拒绝。

> **重试提醒**：普通写入口的重复请求会再次执行。需要版本保护和幂等处理时，使用下述 `guardedTransaction`，并提供独立的 `idempotencyKey`。

```json
{
  "version": 1,
  "requestId": "batch-1",
  "command": {
    "operation": "transaction",
    "operations": [
      {
        "op": "insert",
        "id": "9007199254740993",
        "vector": [1, 0],
        "payload": { "name": "示例" }
      }
    ]
  }
}
```

上面的写入要求宿主显式使用 `{"readOnly":false}`。协议显式节点 ID 字段以十进制字符串传入和返回，避免 JavaScript 精度丢失；TQL 整数字面量由内核解析，parameters 保留 TQL 的原生 JSON 参数规则。u64 向量分量以十进制字符串返回，大整数输入也必须使用字符串；f32/f16 向量使用 JSON 数字，入站统一经过内核 f32 转换通道，f16 再按 half 舍入；出站返回实际数值。Payload 保持 JSON 类型。

> **精度提醒**：Payload 或参数包含大整数时，前端应采用无损 JSON 编解码。

### 查询结果与操作

`query` 支持 `parameters` 对象，返回 `rows`、`rowCount` 和 `columns`；节点列包含 `id`、`vector`、`payload`、`edges`。`getNodes` 保持输入顺序，并以 `missingIds` 报告缺失项。查询中的 `UInt` 和超出 JavaScript 安全范围的 `Int` 标量返回十进制字符串，`Path` 的 ID 同样使用字符串。

操作及参数见资源包 `console-contract.json`。`transaction` 的 `op` 包含 `insert`、`updatePayload`、`updateVector`、`delete`、`link`、`unlink`；`insert` 的 `id` 可省略，`link` 接收 `source`、`target`、`label`、`weight` 和可选 `metadata`，`unlink` 的 `label` 可省略。`importNodes` / `importEdges` 分别以同样的节点/边记录整请求原子提交。索引 `kind` 支持 `hash`、`ordered`、`bitmap`、`ngram`、`unique`、`composite`、`uniqueComposite`。

本协议复用内核默认排序、事务、持久化和关闭语义。读取行工作集预算取数据库与 maxOperations 的较小有效值，超限明确拒绝，不返回静默截断结果。写计划按模式每行的操作上界约束匹配集，在匹配集和写向量物化前准入，复杂模式可能需要拆分批次；确认响应预算在 WAL 前检查。TQL 读取与写计划可协作取消，已经开始的提交遵循原子完成语义。

Console RPC v1 使用整请求事务、全库版本校验和有界持久回执。请求与结果协议在 Rust、Python、Node 和嵌入 HTTP 之间共享。

> **能力范围**　NDJSON、交互式事务、节点级 OCC/ETag 和无限期幂等历史需要单独的协议设计。

---

## 编辑快照、冲突与原事务确认

### 读取版本与提交条件

`getNodes` 设置 `withRevision:true` 后，在同一个数据读锁内返回节点、缺失 ID 和 `data.revision`。此令牌标识当前实例的全库事实版本，客户端应原样保存，并在后续提交中使用。节点、Payload、向量和边的宿主直接写入、CAS、TQL、普通事务与 Console 写入都会使旧版本失效；删除后以相同 ID 和内容重建会生成新版本。不同节点的修改同样会触发冲突；派生索引构建及普通 flush 保留当前节点编辑版本。

> **令牌提醒**：Console revision 与 Server ETag 分属不同协议，请使用读取接口返回的完整原令牌。

编辑提交使用 `guardedTransaction`，必须同时提供读取时的 `expectedRevision`、1..=128 UTF-8 字节的 `idempotencyKey` 以及非空 `operations`。写操作集合与普通 transaction 一致，并遵守 maxOperations。Core 在事务持有的数据写锁内、写 WAL 前检查版本，冲突返回 `TDB_WRITE_CONFLICT`，整笔事务均不生效。

> **冲突处理**：重新读取最新数据，合并并审阅修改，再提交新事务。guardedTransaction 应在宿主 group_commit 之外调用。

```javascript
// 使用当前开发版本的 Node 宿主；前端通过自己的 HTTP/IPC 桥接调用。
const { ConsoleSession } = require('triviumdb')
const session = new ConsoleSession('{"readOnly":false}')
const rpc = (requestId, command) => JSON.parse(db.consoleRequest(session,
  JSON.stringify({ version: 1, requestId, command }))).data
const snapshot = rpc('read-1', {
  operation: 'getNodes', ids: ['1'], withRevision: true,
})
const original = {
  operation: 'guardedTransaction',
  expectedRevision: snapshot.revision,
  idempotencyKey: require('node:crypto').randomUUID(),
  operations: [{ op: 'updatePayload', id: '1', payload: { body: '新正文' } }],
}
// 从草稿确认提交时冻结并保留 original；后续编辑不能改写它。
const result = rpc('write-1', original)
// 结果不确定时查询原事务回执；此请求为只读操作。
const status = rpc('confirm-1', { ...original, operation: 'transactionStatus' })
```

### 回执与结果确认

成功响应为 `{createdIds, revision, replayed:false}`。以原键、原版本和完整原操作重试，命中保留的回执后返回原 createdIds/revision 和 `replayed:true`，直接复用原结果。新的 requestId 可以不同。回执与事实共用 WAL 事务封条，并随完整检查点保存，正常关闭、恢复和重开后继续提供确认。键应由调用方生成并保持全局唯一。键命中但原版本或操作内容不同，返回 `TDB_IDEMPOTENCY_CONFLICT`。transactionStatus 使用相同的完整事务身份进行校验。

`transactionStatus` 接收与原事务相同的 expectedRevision、idempotencyKey、operations，需要 transactions 能力，允许只读会话查询。确认成功时返回 `{outcome:"committed", createdIds, revision}`，其持久性服从数据库 SyncMode。缺少完整成功回执时返回 `{outcome:"unknown"}`，此接口读取当前保留的回执。

> **结果确认**　unknown 表示当前缺少完整成功回执，实际提交结果需要另行核对。若实例因 WAL/应用故障要求恢复，宿主需先处理恢复错误。

回执最多保留 64 条，记录内容合计最多 8 MiB，先入先出淘汰。回执保存原键、原版本、完整事务指纹字节、创建 ID 及提交版本，加载计入实例堆预算，运行期及编码暂存按共享资源预算准入。只有完整提交封条中的回执参与恢复，缺少封条的事务及回执一起丢弃。检查点中的回执与事实一同原子发布。

重新打开会生成新的编辑版本身份。保留的原事务回执继续确认历史提交，后续编辑从当前快照取得版本令牌。回执缺失或已淘汰时返回 unknown，旧版本条件按当前实例校验。无事实变化的受保护事务也消耗一个版本并保存回执，进入 WAL 后的未知结果通过恢复完成后再确认。

> **恢复提醒**　回执受容量和淘汰约束，unknown 的事务按业务身份核对完整事实。当前开发格式为主文件 v11、WAL v4，切换内核前保留完整文件组备份，旧内核环境通过升级前备份恢复。

`info.transport` 与 console-contract.json 公布这些能力及上限。旧的 transaction、mutate、importNodes/importEdges 保持无版本前置条件的原语义；这些写入会使旧编辑令牌失效。版本条件和幂等处理由 guardedTransaction 提供。

### 嵌入页面的编辑队列

嵌入模式下，HTML 的文档编辑、删除和导入统一进入审阅队列。文档从 `getNodes(withRevision:true)` 读取基线，删除与导入在暂存时捕获版本。一个批次中的版本必须一致；混合了不同基线时，需重新读取并审阅整批操作。提交冻结完整操作、版本和幂等键；响应丢失后，“确认原事务”仅调用 `transactionStatus`。`unknown` 保留原请求与队列，由宿主和使用者核对实际数据。继续编辑的草稿与已发送内容分别保存。

> **写入范围**：TQL 写语句使用明确的写入开关并直接执行。索引管理使用对应管理接口。上述入口遵守宿主权限及内核事务约束，文档审阅队列的版本前置条件适用于 `guardedTransaction`。

---

## 错误与写入确认

### 结构化错误

Rust/Python/Node 保留内核错误码与 details；HTTP 将它们放入 error.code、error.message、error.details，并把详情中超出 JavaScript 安全范围的整数转换为十进制字符串。卸载后的会话返回 TDB_CONSOLE_SESSION_CLOSED，关闭的实例返回 TDB_DATABASE_CLOSED；同路径的新实例仍需新会话。

HTTP 数据适配器发送 `X-TDB-Request-ID`，值为原 requestId 的 ASCII JSON 字符串，非 ASCII 字符按 UTF-16 的 `\uXXXX` 转义。服务端在读取正文之前验证单一身份头，允许的原始 ID 为 1—128 个 UTF-8 字节，头编码最多 770 字节。请求体读取、期限和并发拒绝沿该身份返回，响应头保留相同编码，错误信封附加原 requestId。进入 Core 之前核对正文身份，相异身份按 TDB_INVALID_INPUT 拒绝。

现有直接调用 HTTP 的宿主可继续发送原正文协议。提供身份头时遵守上述编码及正文匹配要求。HTTP 读取正文前的错误仅提供此时已知的事实，已解析命令的写入结果继续由 Core 返回。认证及跨域配置由宿主维护，跨域接入时将该请求头纳入允许列表。

### 响应预算与提交状态

响应编码严格受 maxResponseBytes 限制，超限返回 TDB_CONSOLE_RESPONSE_TOO_LARGE，details 包含 max_bytes 和 committed。响应缓冲按扩容峰值参与 ResourceRuntime 共享工作区准入，返回拥有型字符串后由调用方管理其内存。所有写入口在提交前验证确认响应的编码上界，并保留已分配的确认缓冲及额度跨越提交，避免提交后额度被其他实例抢走；TQL 在内核生成实际写计划后、写 WAL 前准入，预算不足时在提交前拒绝。编码器保留提交后检查；committed=true 表示写入已经提交，响应交付失败。

Console 数据适配器将连接中断、JSON 解码、响应身份及回执结构错误按 `TDB_CONSOLE_TRANSPORT` 返回，保留原宿主异常 cause。已交给桥接的写请求提供 `response/unknown`，本地排队取消提供 `admission/not_committed`。内核及 HTTP 错误信封沿用原类别、实际 HTTP 状态和已有写入事实。HTTP 的 AbortSignal 传入 fetch，原事务确认失败继续保留待确认结果。

`dispose()` 等待 detach 回调、本地请求队列和已经发出的取消调用结束，再调用已配置的 drain 桥接。取消调用按请求去重，Core 会话状态及数据库关闭由宿主各自确认。详细完成点见[公共错误契约](error-contracts.md#请求与传输完成点)。

> **写入确认**　客户端退出、超时或传输失败后，应查询完整原事务回执或核对实际数据，再决定后续操作。

---

## 宿主自定义桥接

`create(options)` 在创建时捕获 request、cancel、detach 和 drain 回调，并保留 options 作为回调 receiver。request 接收完整 JSON 字符串及冻结的 `{requestId, operation, write, signal}` 元数据。前三个字段来自排队前的请求快照，signal 由当前进程处理。

```javascript
const client = create({
  request(raw, { requestId, signal }) {
    const context = new RequestContext(requestId);
    const abort = () => context.cancel();
    signal?.addEventListener('abort', abort, { once: true });
    if (signal?.aborted) context.cancel();
    try {
      return context.run(() => db.consoleRequest(session, raw));
    } finally {
      signal?.removeEventListener('abort', abort);
    }
  },
  cancel(requestId) { return session.cancel(requestId); },
  detach() { session.detach(); },
  drain() { return session.isDrained(); },
});
```

Core 将外层控制器、会话期限及会话取消合并。宿主使用 Rust、Python 或 Node 的原生控制句柄时，取消和期限继续进入相同检查点。IPC 跨进程通过宿主消息传递身份及控制事件，接收端建立本地控制器并执行实际调用。

drain 可返回 Promise，完成值为 undefined 或 true 时确认宿主排空。false 返回 `TDB_BUSY`，`details.resource` 为 `console_bridge_drain`。其他完成值返回 `TDB_CONSOLE_TRANSPORT`。detach 或 drain 失败后，客户端保持停止准入，后续 dispose 可重试清理。HTTP 适配器接受同样的 drain 回调。

任意宿主拒绝值，包括 null 和原始值，按传输错误保存 cause。已分发写入仍按未知响应结果处理。完成点分别覆盖请求执行、传输消费、宿主排空和数据库关闭。

> **完成提醒**　宿主的 drain 回调确认其实际持有的工作。同步原生调用使用宿主执行线程，异步任务及跨进程消息的调度和取消由对应桥接管理。数据库文件资源通过显式关闭完成点确认释放。

---

## 资源交付与组件挂载

### 资源包

```text
python scripts/package_console.py --output .tmp/console/triviumdb-console.zip
```

ZIP 包含 `index.html`、`console-client.js`、`console-embed.js`、`console-contract.json`、`manifest.json`、接入文档及 Apache-2.0 LICENSE。manifest 记录版本、资源 SHA-256、挂载入口和外部资源 URL。将三个页面资源放在宿主同源静态目录，例如 `/tdb-console/`。

Rust Server crate 同时提供 `console_html()`、`console_client_script()` 和 `console_embed_script()`，可由宿主路由交付。独立 Server 已提供对应脚本路由。

### IPC 挂载

宿主页面加载 `console-embed.js` 后，提供自己的容器与会话桥接：

```html
<div id="database-console" style="height: 800px"></div>
<script src="/tdb-console/console-embed.js"></script>
```

```javascript
// hostConsole 为应用提供的 IPC 桥接；绑定到本次挂载专属的 ConsoleSession。
const component = await TriviumConsoleEmbed.mount(
  document.getElementById('database-console'),
  {
    url: '/tdb-console/index.html',
    storageKey: 'workspace-42/database-main',
    request: raw => hostConsole.request(raw),
    cancel: requestId => hostConsole.cancel(requestId),
    detach: () => hostConsole.detach(),
  },
)

// 卸载会等待桥接中的请求退出；宿主随后可按自身关闭流程释放数据库。
await component.dispose()
```

`request(raw)` 接收完整 JSON 字符串，可同步返回或通过 Promise 返回响应字符串/对象。IPC 错误应保留 `schema_version`、`code`、`message`、`details` 及已提供的 `stage`、`target`；也可返回 HTTP 适配器相同的 `error` 信封。TQL 的阶段和源码锚点见[结构化诊断](tql-reference.md#结构化查询诊断)。`cancel(requestId)` 为可选的尽力取消入口；`detach()` 为必需回调，用于阻止该会话继续准入。

### HTTP 挂载

使用 HTTP 适配时省略 `request`，配置已有的 Console 路由：

```javascript
const component = await TriviumConsoleEmbed.mount(container, {
  url: '/tdb-console/index.html',
  storageKey: 'workspace-42/database-main',
  endpoint: '/database/main/v1/console',
  credentials: 'same-origin',
  detach: () => hostConsole.detach(),
})
```

`endpoint` 支持宿主嵌套路由；`headers` 可传递宿主所需认证头。静态资源与宿主页面同源，跨域 API 访问由宿主配置 CORS 与认证。每次挂载使用独立 ConsoleSession，避免一个组件卸载影响其他组件。

### 实例隔离与卸载

iframe 隔离页面 DOM 和 CSS。`storageKey` 按应用及数据库稳定区分，主题、语言、历史、AI 配置和文档草稿使用该命名空间。`component.info` 返回实际权限、dtype、预算和协议能力，`component.frame` 提供容器引用。对于自定义 WebView，加载带 `embedded=1&storageKey=...` 的页面后，可直接调用页面的 `TriviumConsole.mount({request,cancel,detach})`。

> **身份提醒**：`storageKey` 随页面 URL 传递，请使用非敏感标识。切换数据库时先卸载旧组件，再使用新标识和新会话挂载。

`component.dispose()` 在存在未提交草稿或队列时保留组件并返回错误。宿主处理这些内容后重试；显式 `dispose({discard:true})` 可以退出页面，浏览器草稿按原命名空间保存。调用 `component.pendingTransaction()` 可取得结果待确认的完整原请求，供宿主在卸载前导出或交给自己的恢复流程。卸载入口停止新请求、调用取消与 detach，并等待已准入桥接调用完成；文件锁和持久化由数据库生命周期契约保证。

> **恢复提醒**：未知事务的原请求保存在页面内存中。强制退出、刷新或导航前，需保存原事务并核对其完整结果。`pagehide` 的 detach 通知属于尽力操作，宿主应在 WebView/窗口销毁时补充会话卸载。

### 能力范围与离线资源

传输适配覆盖 TQL、节点与边读取、向量检索、子图、最短路径、索引和审阅事务。Console 的邻域沿出边扩展；边详情读取入边与出边；路径入口返回一条最短路径。Console 查询使用 JSON 整体响应。向量工作区支持 f32/f16；u64 数据库通过 TQL 使用二进制向量，节点 ID 在 Console 路径以十进制字符串完整传递。Server 专用剖析与队列指标在嵌入模式中关闭或标记为未提供。

现有页面包含按需加载的 GSAP、Sigma 等 CDN 资源；基础查询、文档和 Canvas 图功能使用随包资源。底层数据库与会话配置由宿主掌握。

> **离线使用**：需将页面依赖打包到宿主，并调整资源加载路径。
