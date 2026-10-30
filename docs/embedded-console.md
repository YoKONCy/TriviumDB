# 嵌入式 Console 宿主接入

Console v1 为 Rust、Python 和 Node.js 提供同一份 JSON 请求协议，直接操作宿主已经打开的 Embedded Database。内核不引入 HTTP 或桌面框架依赖；HTTP 适配位于独立的 nightly Server crate，桌面 IPC 可原样转发字符串。

本次交付范围是宿主协议、权限、生命周期、HTTP 适配和资源打包。现有 HTML、CSS、界面状态及交互逻辑保持不变。当前 HTML 仍使用 Server HTTP v1；前端开发者需要把数据访问适配到 Console RPC v1，才能使用新的嵌入式入口。资源包的 manifest 明确记录这一条件，不能把当前 HTML 的复制视为已经完成组件挂载。

## 文件锁、借用和关闭

- 唯一文件锁由原 Database 持有。创建 ConsoleSession 不打开路径；Rust ConsoleHost::attach 接收已有的 Arc<Mutex<Database<T>>>，只保存弱引用。禁止另外启动 Server 打开同一可写文件。
- Rust 宿主的业务访问与 Console 必须使用同一个 Mutex。Console 使用 try_lock；竞争或重入返回 `TDB_IO`，`details.io_kind=WouldBlock`，不会等待锁。锁中毒时拒绝访问，交由宿主执行恢复处理。
- 会话在第一次请求时绑定具体实例身份，Rust ConsoleHost 在 attach 时就完成绑定。绑定身份使用弱引用而非路径；旧会话不能绑定同路径的新实例。
- detach 阻止新请求并发出取消信号，不获取宿主数据库锁、不执行 flush 或 close。多个独立会话互不影响；同一会话的克隆共享卸载及取消状态。
- TQL 读取使用内核 QueryControl 协作取消和 deadline。其余操作在支持的检查点检查取消；向量 Hook、索引构建及已进入提交的写入不能被强制中断。`info.transport.cooperativeCancellation` 明确列出协作取消的操作。
- HTTP 客户端退出会传递取消信号。blocking 任务持有的并发许可直到实际执行退出才归还；响应 Future 消失不代表资源已经释放。
- 宿主关闭前先停止入口并 detach，再在自己的串行调度中调用 Database.close。关闭成功会释放文件锁，即使会话、Router、Prepared 或关闭后的 Database 对象仍被持有。关闭失败沿用 Core 契约：保留锁和原实例，处理原因后重试关闭。
- Python 请求执行期间释放 GIL，但 PyO3 继续保持实例的独占借用。宿主用同一个锁或队列协调所有业务访问和关闭；不要在 Hook 中重入该实例。Node 请求同步执行；公共 index.js 在进入 N-API 前拒绝 Console 期间的方法及 getter 重入，也拒绝宿主调用的 Hook 反向进入 Console。不要绕过公共加载器直接调用内部 .node 文件。
- 不持有数据库互斥锁跨越 await、网络发送或等待对端 IPC；只在同步内核调用期间借用。宿主关闭是宿主权限，RPC 不提供打开路径、关闭数据库或移动文件的命令。

## Rust

```rust,no_run
use std::sync::{Arc, Mutex};
use triviumdb::{ConsoleHost, ConsoleOptions, Database};

let database = Arc::new(Mutex::new(Database::<f32>::open("app.tdb", 768)?));
let console = ConsoleHost::attach(&database, ConsoleOptions::default())?;
let raw = r#"{"version":1,"requestId":"q1","command":{"operation":"info"}}"#;
let response = console.request(raw)?;
console.session().detach();
database.lock().map_err(|_| std::io::Error::other("宿主锁中毒 (Host lock poisoned)"))?.close()?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Rust 也可在自己的同步借用范围内直接调用 `database.console_request(&session, raw)`。支持 f32、f16、u64 以及 Database 的 ReadWrite、ReadOnly、Immutable 模式；会话不能提升数据库本身的访问权限。

HTTP 接入复用相同的 ConsoleHost：

```rust,ignore
use triviumdb_server::console::{build_console_app, ConsoleHttpOptions};
let app = build_console_app(console, ConsoleHttpOptions::default())?;
// 可通过宿主 Router::nest 设置前缀，并在外层添加自己的认证和跨域策略。
```

可执行示例见仓库 `crates/triviumdb-server/examples/embedded_console.rs`。适配器只提供以下路由，不额外启动 Writer Actor 或 QuIVer 预热任务：

| 路由 | 用途 |
| --- | --- |
| POST /v1/console | 执行 JSON RPC，成功返回 application/json |
| POST /v1/console/cancel/{requestId} | 请求取消，返回 cancelled 布尔值 |
| GET /v1/console/contract | 获取版本化操作清单与传输约定 |

HTTP 并发额度默认 8，允许 1..=1024；满额返回 503/WouldBlock，先准入再读取请求体。请求体上限使用会话的 maxRequestBytes，读取请求体与内核工作共享同一个 deadline。宿主必须保持 Database 的强引用，否则 Router 返回 410/TDB_DATABASE_CLOSED。

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

Python/Node 的 HTTP 或 IPC 包装由宿主提供。该入口是同步 JSON 调用，不创建监听端口、不拥有宿主对象；异步任务调度及应用级请求授权仍由宿主负责。

## 请求、权限和数据表示

会话配置由可信宿主创建，不从前端请求读取。三端使用相同字段，Python/Node 构造器接收配置 JSON 字符串：

| 字段 | 默认值 | 边界 |
| --- | --- | --- |
| readOnly | true | false 仍受 Database AccessMode 限制 |
| features | query、vector、graph、indexes、transactions、import | 枚举，未知值拒绝 |
| maxRequestBytes | 4194304 | 1..=67108864 |
| maxResponseBytes | 16777216 | 1..=67108864 |
| maxOperations | 1000 | 1..=1000，限制读取工作集行数、批量记录、写计划、显式 topK 及图预算 |
| queryTimeoutMs | 30000 | 1..=300000，不延长外部 deadline |

features 是宿主开放的操作入口，不是 TQL 的表级或算子级权限沙箱。例如开放 query 后，读 TQL 本身可以包含向量和图算子。需要细粒度数据权限时由宿主选择可执行查询；只读权限始终由会话与内核共同执行。

请求必须包含 version=1、非空且最多 128 UTF-8 字节的 requestId，以及 command 对象。requestId 仅用于关联和取消，不是幂等键；同一活跃 ID 会被拒绝，完成后重复发送写入会再次执行。未知字段、命令、协议版本和不合法参数均拒绝。

```json
{"version":1,"requestId":"batch-1","command":{"operation":"transaction","operations":[{"op":"insert","id":"9007199254740993","vector":[1,0],"payload":{"name":"示例"}}]}}
```

上面的写入要求宿主显式使用 `{"readOnly":false}`。协议显式节点 ID 字段以十进制字符串传入和返回，避免 JavaScript 精度丢失；TQL 整数字面量由内核解析，parameters 保留 TQL 的原生 JSON 参数规则。u64 向量分量以十进制字符串返回，大整数输入也必须使用字符串；f32/f16 向量使用 JSON 数字，入站统一经过内核 f32 转换通道，f16 再按 half 舍入；出站返回实际数值而非 half 的存储位模式。Payload 的 JSON 类型不改变；Payload 或参数本身包含大整数时，前端应采用无损 JSON 编解码。

query 支持 parameters 对象，返回 rows、rowCount 和 columns；节点列包含 id、vector、payload、edges。getNodes 保持输入顺序，并以 missingIds 报告缺失项。查询中的 UInt 和超出 JavaScript 安全范围的 Int 标量返回十进制字符串，Path 的 ID 同样使用字符串。

操作及参数见资源包 console-contract.json。transaction 的 op 包含 insert、updatePayload、updateVector、delete、link、unlink；insert 的 id 可省略，link 接收 source、target、label、weight 和可选 metadata，unlink 的 label 可省略。importNodes/importEdges 分别以同样的节点/边记录整请求原子提交。索引 kind 支持 hash、ordered、bitmap、ngram、unique、composite、uniqueComposite。

本协议复用内核默认排序、事务、持久化和关闭语义。读取行工作集预算取数据库与 maxOperations 的较小有效值，超限明确拒绝，不返回静默截断结果。写计划按模式每行的操作上界约束匹配集，在匹配集和写向量物化前准入，复杂模式可能需要拆分批次；确认响应预算在 WAL 前检查。TQL 读取与写计划可协作取消，已经开始的提交遵循原子完成语义。当前不提供 NDJSON、交互式事务、Server OCC/ETag 和幂等缓存，不生成可被误认为 OCC 令牌的虚假 generation。

## 错误与写入确认

Rust/Python/Node 保留内核错误码与 details；HTTP 将它们放入 error.code、error.message、error.details，并把详情中超出 JavaScript 安全范围的整数转换为十进制字符串。卸载后的会话返回 TDB_CONSOLE_SESSION_CLOSED，关闭的实例返回 TDB_DATABASE_CLOSED；同路径的新实例仍需新会话。

响应编码严格受 maxResponseBytes 限制，超限返回 TDB_CONSOLE_RESPONSE_TOO_LARGE，details 包含 max_bytes 和 committed。响应缓冲按扩容峰值参与 ResourceRuntime 共享工作区准入，返回拥有型字符串后由调用方管理其内存。所有写入口在提交前验证确认响应的编码上界，并保留已分配的确认缓冲及额度跨越提交，避免提交后额度被其他实例抢走；TQL 在内核生成实际写计划后、写 WAL 前准入，因此预算不足不会产生写入事实。编码器仍保留提交后防御性检查：若返回 committed=true，表示写入已经提交而响应无法交付，不得盲目重试。客户端退出、超时或传输失败同样不能被视为写入回滚，调用方应重新查询确认事实。

## 资源交付与前端分工

```text
python scripts/package_console.py --output .tmp/console/triviumdb-console.zip
```

ZIP 包含原样 index.html、console-contract.json、manifest.json、接入文档及 Apache-2.0 LICENSE。manifest 记录版本、完整 HTML SHA-256 和现有外部资源 URL。脚本不修改 HTML 或前端依赖，不包含数据库、凭据或测试数据。

现有页面包含按需加载的 GSAP、Sigma 等 CDN 资源，本包不宣称完整离线运行。前端负责人负责传输适配、容器挂载/卸载、CSS 隔离、主题/语言、偏好命名空间和离线依赖处理；组件卸载时应通知宿主 detach。底层数据库与会话配置由宿主掌握。
