# 多实例共享资源运行时

> 适用范围：0.9.0-beta.4。

同一进程中的多个嵌入式数据库可以共享 `ResourceRuntime`。每个租户、会话或 Agent 使用独立数据库，运行时统一管理配额、计算线程和任务准入。普通单机应用与容器内的宿主使用同一套 Rust、Python、Node.js 接口。

## 资源准入

节点、Payload 原始字节、图边、属性 posting 与文本索引在变更时更新计数；QuIVer 热内存直接读取结构容量。写入准入和查询观测据此估算堆内存，计量成本随索引定义数量变化。属性 `index_info` 从计数生成统计；Payload 的完整诊断继续按需统计缓存中被外部引用持有的条目。

| 配置 | 含义 | 零值 |
| --- | --- | --- |
| `memory_limit` | 数据库声明配额与已预留临时工作区的总和上限，Rust 单位为字节 | 不限 |
| `maintenance_memory_limit` | 总预算内保护的维护工作区，Rust 单位为字节 | 有限总预算下自动保护八分之一，至少 1 字节 |
| `max_threads` | 同一运行时所有缓存计算池的工作线程总上限，最大 64 | 按可用并行度选择，最多 64 |
| `max_active_tasks` | 同时准入的前台根任务数；同一任务的内部嵌套调用复用名额 | 不限 |

有共享内存上限时，每个数据库通过非零 `Config.memory_limit` 声明配额。打开数据库先登记完整配额，超额在创建数据库制品前返回错误。配额调整成功后更新账本，失败时保留原值；打开失败或成功关闭时归还配额。关闭完成时同时释放本实例的内存存储、WAL writer、mmap、数据库持有的 Hook 与文件锁；共享运行时线程池由运行时所有者管理。

**维护保护额度计入总预算。** 例如总额 256 MiB、保护 32 MiB 维护额度，数据库配额与前台工作区合计最多使用 224 MiB。维护操作使用保护额度，并可借用当前闲置的前台额度。前台可用空间按保护额度与维护实际用量中的较大值扣除。

刷盘、压实、关闭和快照使用独立的维护任务名额，最多同时准入 `max_threads` 个维护根任务。前台根任务由 `max_active_tasks` 单独计数。同一维护任务的嵌套调用复用名额。后台调度优先处理已排空的关闭任务，再处理周期压实。

维护在工作前检查峰值预算。宿主按数据库规模和同时执行的维护任务配置总额与保护额度。额度不足时返回预算错误并保留重试状态；后台维护记录延期或失败原因。详见[关闭、快照与恢复](lifecycle-snapshots.md)。

资源不足立即返回错误，宿主可以限流、退避重试或释放闲置实例。检索参数、结果完整性和排序遵循调用方选择的查询语义。

GraphFirst 与精确检索先对候选评分并保留 Top-K，再为最终 Payload 的解码和返回副本预留峰值空间。可达性的详细、紧凑和并行执行，以及邻域和子图查询，将状态容器、路径、位图、邻接临时区、Metadata 与 Payload 物化纳入共享账本。状态预留随实际探索量增长；并行位图按槽位数和当前 worker 数计算。所有内存准入失败均返回资源预算错误，遍历的 Partial 配置继续服从这一准入规则。

## Rust

TQL 为完整 Payload、向量、邻接边、标签和 Metadata 快照建立查询级预留，返回、分组、DISTINCT 与排序在复制前计算工作区。管线按实际求值列估算，常量、节点 ID 与标量别名保持轻量。DML 匹配继承实例内存预算并完整执行匹配集；读查询的返回行数配置单独生效。预算拒绝发生在事务写入前，失败和完成均归还共享工作区。该准入使用保守峰值估算。

```rust
use triviumdb::{Database, ResourceLimits, ResourceRuntime};
use triviumdb::database::Config;

fn main() -> triviumdb::Result<()> {
    let runtime = ResourceRuntime::new(ResourceLimits {
        memory_limit: 256 * 1024 * 1024,
        maintenance_memory_limit: 32 * 1024 * 1024,
        max_threads: 4,
        max_active_tasks: 8,
    })?;
    let config = Config {
        dim: 768,
        memory_limit: 64 * 1024 * 1024,
        ..Default::default()
    };
    let mut first = Database::<f32>::open_with_runtime("first.tdb", config, runtime.clone())?;
    let mut second = Database::<f32>::open_with_runtime("second.tdb", config, runtime.clone())?;
    println!("共享资源统计：{:?}", runtime.stats());
    first.set_memory_limit(80 * 1024 * 1024)?;
    first.close()?;
    second.close()?;
    Ok(())
}
```

`DatabaseReader`、`DatabaseWriter` 也提供 `open_with_runtime`。现有 `open` / `open_with_config` 入口复用进程默认运行时：不限制配额与任务数，但缓存工作线程总数最多 64。显式创建的运行时相互独立，其上限不会自动合并。

Rust `set_memory_limit(bytes)` 返回 `Result<()>`，成功时同步更新本实例配置与共享配额。运行时资源拒绝返回 `TriviumError::ResourceBudgetExceeded { resource, requested, used, limit }`，可直接按字段分类。

## Python

```python
from triviumdb import TriviumDB, ResourceRuntime, ResourceBudgetError

runtime = ResourceRuntime(memory_limit_mb=256, maintenance_memory_limit_mb=32, max_threads=4, max_active_tasks=8)
first = TriviumDB("first.tdb", dim=768, memory_limit_mb=64, runtime=runtime)
second = TriviumDB("second.tdb", dim=768, memory_limit_mb=64, runtime=runtime)
try:
    first.set_memory_limit(80)
except ResourceBudgetError:
    print("额度不足，稍后重试")
print(runtime.stats())
first.close()
second.close()
```

Python 配额参数单位为 MiB，资源拒绝抛出 `ResourceBudgetError`，它继承 `RuntimeError`。数据库的 `resource_stats()` 和运行时的 `stats()` 返回同一运行时的共享统计。

内核产生的 `ResourceBudgetError` 实例包含 `resource`、`requested`、`used`、`limit` 属性。`resource` 标识预算维度，内存维度单位为字节，线程与任务维度单位为个数。宿主可直接读取这些字段决定退避或释放资源。

## Node.js

```javascript
const { TriviumDB, ResourceRuntime } = require('triviumdb')
const runtime = new ResourceRuntime({ memoryLimitMb: 256, maintenanceMemoryLimitMb: 32, maxThreads: 4, maxActiveTasks: 8 })
const first = new TriviumDB('first.tdb', { dim: 768, memoryLimitMb: 64, runtime })
const second = new TriviumDB('second.tdb', { dim: 768, memoryLimitMb: 64, runtime })
console.log(runtime.stats())
first.close()
second.close()
```

Node.js 配额参数单位为 MiB，运行时资源拒绝的 `error.code` 为 `QueueFull`。宿主按错误码和预算字段处理重试。`resourceStats()` / `stats()` 的 JSON 字段与 Rust 统计保持一致，使用下划线命名。

`QueueFull` 错误还包含与 Python 相同的 `resource`、`requested`、`used`、`limit` 字段，TypeScript 对应 `ResourceBudgetError` 接口。同步调用与 `searchBatch()` 的 Promise 拒绝采用同一映射；JS 错误对象在事件循环线程上创建。

## 实例诊断与可用额度

| Rust | Python | Node.js | 范围 |
| --- | --- | --- | --- |
| `runtime.stats()` | `runtime.stats()` | `runtime.stats()` | 运行时共享总账 |
| `db.resource_stats()` | `db.resource_stats()` | `db.resourceStats()` | 同一份运行时共享总账 |
| `db.instance_stats()` | `db.instance_stats()` | `db.instanceStats()` | 当前数据库实例 |

`InstanceStats` 包含规范化的绝对 `database_path`、配置的 `memory_limit_bytes`、`memory`、`shutdown` 和 `compaction`。`DatabaseReader`、`DatabaseWriter` 也提供实例诊断。宿主可以自行维护实例列表，逐个查询和汇总。

诊断接口独立于工作任务准入，任务名额用满时仍可读取状态。

`memory` 包含本实例的 `reserved_database_bytes`、`estimated_memory_bytes` 与 `node_count`。写锁正被刷盘、压实或其他操作占用时，整个 `memory` 返回 `None` / `null`，表示本次采样暂不可用，可在后续轮询中重试。诊断接口直接读取运行状态，通过尝试获取 MemTable 读锁采样内存。成功关闭后，配置值仍可查看，预留配额与节点数为零；内存估算包含空结构的固定开销。

共享统计还提供三个余额：

- `available_foreground_bytes`：考虑维护保护额度及维护实际借用后，可供前台登记的字节数。
- `available_maintenance_bytes`：当前总预算中尚未登记的字节数，可用于维护工作区。
- `available_task_slots`：尚未占用的前台根任务名额。

未设置对应上限时，余额为 `None` / `null`。两个内存余额分别表示共享总预算下可准入的前台与维护工作区大小；每次操作执行前重新进行原子准入。实例内存、维护状态和生命周期采用分段采样。临时工作区按运行时汇总，数据库声明配额按实例报告。

## 后台维护诊断

`instance_stats().compaction` 提供当前周期压实登记的状态；未启用或已撤销登记时为空。关闭任务的状态通过 `shutdown_status().maintenance` 查看，任务完成并撤销登记后为空。

维护状态包含 `phase`（`scheduled` / `running` / `stopped`）、成功次数 `completed_runs`、失败次数 `failed_runs`、延期次数 `deferred_runs`，以及最近的 `last_defer_reason` 和 `last_failure`：

- `not_ready`：关闭需要等待已有操作或旧后台任务排空。
- `resource_budget`：调度准入被预算拒绝；结构化详情在 `last_failure.budget`。
- `database_busy`：周期压实尝试获取写锁时数据库繁忙，本次跳过。

延期不计作已执行失败。执行阶段发生 I/O 或工作区预算错误则计入 `failed_runs`。`last_failure.kind` 为稳定分类，`budget` 在资源拒绝时包含四个预算字段；`message` 仅供诊断。一次成功执行会清除最近延期原因和故障，但保留累计次数。统计从本次登记开始，重新启用压实会建立新的登记。

## 调度与观测边界

运行时统一管理数据库入口使用的扫描、构图、TQL 计算及后台压实工作池。闲置池的线程退出后才建立替代池，避免缓存多个并行度时累计超额。后台压实共用一个进程计时线程，实际维护进入所属运行时；资源不足或数据库正忙时延期。

`max_threads` 管理所属运行时计算池中的工作线程。宿主调用线程、进程共享的计时线程、宿主创建的其他线程及外部 Hook 资源由宿主单独管理。每个运行时具有独立额度；宿主显式传入预算，并通过操作系统或容器配置进程内存和 CPU 限额。

`stats()` 包含数据库与工作区预留、峰值预留、登记实例数、分配/实际存活/峰值工作线程、前台/维护活动任务、维护工作区预留、拒绝计数和维护任务计数。统计不触发资源准入，可在过载时读取。

内存额度按**显式数据库配额和已接入工作区的预留**核算。进程容量评估还应采集 mmap 驻留、操作系统页缓存、线程栈、语言运行时与 Hook 的内存开销。`estimated_memory`、索引/载荷统计以及进程内存指标提供相应观测信息；操作准入由显式预算决定。

根任务准入覆盖返回 `Result` 的数据库工作入口。统计与读取辅助接口保留各自的直接访问方式。Prepared 构图任务从准备成功起持有任务名额与工作区，执行完成或丢弃时归还。同步 `close()` 等待本库操作完成；`request_shutdown()` 与 `close_timeout()` 提供可观测关闭和有界等待，超时后关闭任务继续持有正在使用的资源。
