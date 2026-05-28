# TriviumDB CLI & TUI 设计文档

> **版本**: v0.1 (设计稿)  
> **状态**: 待评审  
> **负责人**: TBD  
> **关联路线图**: README.md — "CLI 工具 (triviumdb-cli)" + "数据库可视化 UI 工具"

---

## 目录

- [1. 概述](#1-概述)
- [2. 架构决策](#2-架构决策)
- [3. Workspace 结构](#3-workspace-结构)
- [4. 命令体系设计](#4-命令体系设计)
- [5. 共享层设计](#5-共享层设计)
- [6. REPL 模式设计](#6-repl-模式设计)
- [7. TUI 模式设计](#7-tui-模式设计)
- [8. 技术栈与依赖](#8-技术栈与依赖)
- [9. 开发计划与分工](#9-开发计划与分工)
- [10. 未来扩展](#10-未来扩展)

---

## 1. 概述

`triviumdb-cli` 是 TriviumDB 的统一命令行工具，提供三种交互模式：

| 模式 | 入口 | 适用场景 |
|------|------|---------|
| **非交互** | `triviumdb-cli exec db.tdb "TQL语句"` | 脚本、CI、管道 |
| **REPL** | `triviumdb-cli open db.tdb` | 快速查询、调试 |
| **TUI** | `triviumdb-cli ui db.tdb` | 图谱浏览、搜索探索、可视化 |

设计哲学：
- **单一二进制**：一个可执行文件涵盖所有模式
- **共享核心**：非交互命令逻辑被 REPL 和 TUI 共同复用
- **渐进复杂度**：新用户用 REPL 快速上手，高级用户用 TUI 深度探索

---

## 2. 架构决策

| 决策项 | 结论 | 理由 |
|--------|------|------|
| UI 技术 | TUI (终端全屏) | 纯 Rust、零外部依赖、开发者友好 |
| 代码组织 | Cargo workspace | CLI 依赖不污染 lib，编译更快 |
| 开发节奏 | 共享层先行，CLI + TUI 并行 | 适合多人分工 |
| 图谱渲染 | ASCII/Unicode in terminal | 无需 GPU/浏览器，终端内即可可视化 |

---

## 3. Workspace 结构

```
TriviumDB/
├── Cargo.toml              ← workspace root (同时保留 triviumdb lib package)
├── src/                    ← 现有 triviumdb lib 源码 (不动)
├── cli/                    ← 新增: triviumdb-cli bin crate
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs             ── clap App 定义 + 模式分发
│       ├── db_handle.rs        ── Database<T> 打开 / dtype 动态分发
│       ├── formatter.rs        ── table / json / csv 输出格式化
│       ├── commands/           ── 非交互子命令 (共享层)
│       │   ├── mod.rs
│       │   ├── info.rs         ── 数据库元信息
│       │   ├── exec.rs         ── 非交互 TQL 执行
│       │   ├── export.rs       ── 导出 JSON/JSONL
│       │   ├── import.rs       ── 从 JSON/JSONL 批量导入
│       │   ├── repair.rs       ── 检查/修复 (吸收 trivium_repair)
│       │   └── compact.rs      ── 手动压缩
│       ├── repl/               ── REPL 模式
│       │   ├── mod.rs          ── REPL 主循环
│       │   └── completer.rs    ── Tab 补全 (TQL 关键词 + 元命令)
│       └── tui/                ── TUI 模式
│           ├── mod.rs          ── TUI 入口
│           ├── app.rs          ── App state + 事件循环
│           ├── ui.rs           ── 整体布局渲染
│           ├── event.rs        ── 键盘/鼠标事件处理
│           └── widgets/        ── 面板组件
│               ├── mod.rs
│               ├── graph_view.rs    ── 图谱 ASCII 可视化
│               ├── query_editor.rs  ── TQL 编辑器
│               ├── results_table.rs ── 查询结果表格
│               ├── node_detail.rs   ── 节点详情面板
│               └── status_bar.rs    ── 底部状态栏
├── tests/                  ← 现有测试 (不动)
├── docs/                   ← 文档
└── ...
```

### 根 Cargo.toml 修改

```toml
[workspace]
members = ["cli"]

# 现有 [package] 部分保持不变
[package]
name = "triviumdb"
# ...
```

### cli/Cargo.toml

```toml
[package]
name = "triviumdb-cli"
version = "0.1.0"
edition = "2024"
description = "CLI & TUI tool for TriviumDB"
license = "Apache-2.0"

[[bin]]
name = "triviumdb"
path = "src/main.rs"

[dependencies]
triviumdb = { path = ".." }

# CLI 基础
clap = { version = "4", features = ["derive", "env"] }
colored = "2"
tabled = "0.17"
serde_json = "1.0"

# REPL
rustyline = "14"

# TUI
ratatui = "0.29"
crossterm = "0.28"
```

---

## 4. 命令体系设计

### 顶层命令

```
triviumdb [全局选项] <子命令>

全局选项:
  --format <table|json|csv>    输出格式 (默认: table)
  --color <auto|always|never>  彩色输出 (默认: auto)
  -v, --verbose                详细日志

子命令:
  open <PATH>                  打开数据库进入 REPL
  ui <PATH>                    打开数据库进入 TUI 可视化面板
  info <PATH>                  显示数据库元信息
  exec <PATH> <TQL>            非交互执行 TQL 语句
  repair <check|dump> <PATH>   数据库诊断与修复
  export <PATH> <OUTPUT>       导出数据
  import <PATH> <INPUT>        导入数据
  compact <PATH>               手动压缩
  migrate <PATH> <NEW_PATH> --dim <N>  维度迁移
  version                      版本信息
```

### info 子命令输出示例

```
$ triviumdb info mydata.tdb

┌───────────────────────────────────────────┐
│ TriviumDB Instance Info                   │
├───────────────────┬───────────────────────┤
│ Path              │ mydata.tdb            │
│ Version           │ v5 (file format)      │
│ Dimension         │ 1536                  │
│ Data Type         │ f32                   │
│ Node Count        │ 12,847                │
│ Storage Mode      │ Mmap                  │
│ File Size (.tdb)  │ 4.2 MB                │
│ File Size (.vec)  │ 75.1 MB               │
│ WAL Size          │ 128 KB (pending)      │
│ QuIVer Index      │ Active (12,847 nodes) │
│ Text Index        │ 8,231 docs indexed    │
└───────────────────┴───────────────────────┘
```

### exec 子命令

```bash
# 单条查询
$ triviumdb exec mydata.tdb "FIND {type: 'person'} RETURN * LIMIT 3"

# 管道输出
$ triviumdb exec mydata.tdb "FIND {} RETURN *" --format json | jq '.[] | .id'

# 写操作
$ triviumdb exec mydata.tdb --mut "CREATE (n {name: 'Alice', type: 'person'})"
```

---

## 5. 共享层设计

### db_handle.rs — dtype 动态分发

TriviumDB 的 `Database<T>` 是泛型的 (`f32` / `f16` / `u64`)，CLI 需要在运行时根据用户参数或文件嗅探决定类型。

```rust
/// 封装 dtype 动态分发，避免在每个命令中重复 match
pub enum DbHandle {
    F32(Database<f32>),
    F16(Database<half::f16>),
    U64(Database<u64>),
}

impl DbHandle {
    pub fn open(path: &str, dim: Option<usize>, dtype: &str) -> Result<Self>;
    
    // 统一入口方法 — 内部 match dispatch
    pub fn node_count(&self) -> usize;
    pub fn dim(&self) -> usize;
    pub fn tql(&self, query: &str) -> Result<TqlOutput>;
    pub fn tql_mut(&mut self, query: &str) -> Result<TqlMutOutput>;
    pub fn info(&self) -> DbInfo;
    pub fn flush(&mut self) -> Result<()>;
    // ... 更多按需添加
}
```

> **设计要点**: 使用 `macro_rules!` 类似 Python 绑定中的 `dispatch!` 宏减少重复代码。

### formatter.rs — 输出格式化

```rust
pub enum OutputFormat { Table, Json, Csv }

pub fn format_tql_result(result: &TqlOutput, format: OutputFormat) -> String;
pub fn format_info(info: &DbInfo, format: OutputFormat) -> String;
pub fn format_search_hits(hits: &[SearchHit], format: OutputFormat) -> String;
```

### commands/ — 子命令逻辑

每个子命令是一个纯函数，接收 `DbHandle` + 参数，返回格式化后的输出或错误。这些函数可以被 REPL（元命令 `.info`）和 TUI（快捷键触发）直接调用。

```rust
// commands/info.rs
pub fn run_info(handle: &DbHandle) -> DbInfo { ... }

// commands/exec.rs
pub fn run_exec(handle: &mut DbHandle, tql: &str, is_mut: bool) -> Result<TqlOutput> { ... }

// commands/export.rs  
pub fn run_export(handle: &DbHandle, output_path: &str, format: ExportFormat) -> Result<usize> { ... }
```

---

## 6. REPL 模式设计

### 交互体验

```
$ triviumdb open mydata.tdb

TriviumDB v0.7.1 | dim=1536 | nodes=12,847 | f32 | mmap
Type .help for commands, or enter TQL directly.

tql> FIND {type: "person", age: {$gt: 25}} RETURN * LIMIT 5
┌────┬───────────────────────────────────────┐
│ id │ payload                               │
├────┼───────────────────────────────────────┤
│ 42 │ {"name":"Alice","type":"person",...}   │
│ 87 │ {"name":"Bob","type":"person",...}     │
└────┴───────────────────────────────────────┘
2 rows (3.2ms)

tql> MATCH (a {name:"Alice"})-[:knows]->(b) RETURN a, b
┌──────────┬────────────────────┬──────────┬────────────────────┐
│ a.id     │ a.payload          │ b.id     │ b.payload          │
├──────────┼────────────────────┼──────────┼────────────────────┤
│ 42       │ {"name":"Alice"}   │ 87       │ {"name":"Bob"}     │
└──────────┴────────────────────┴──────────┴────────────────────┘
1 row (1.8ms)

tql> .info
  Nodes: 12,847 | Memory: 82.3 MB | WAL: 128 KB | QuIVer: active

tql> .flush
  ✓ Flushed to disk (took 45ms)

tql> .help
  .info          数据库概览
  .stats         详细统计 (内存、QuIVer、WAL)
  .schema        Payload 字段分布 (采样)
  .flush         手动落盘
  .compact       触发压缩
  .export <file> 导出为 JSONL
  .format <fmt>  切换输出格式 (table/json/csv)
  .quit / .exit  退出

tql> .quit
```

### 设计细节

- **无点号** → 进 TQL 解析（支持多行，以 `;` 或空行结束）
- **点号开头** → 元命令
- **Tab 补全**: TQL 关键词 (`FIND`, `MATCH`, `SEARCH`, `RETURN`, `WHERE`, ...) + 元命令 (`.info`, `.flush`, ...)
- **历史记录**: 保存至 `~/.triviumdb_history`
- **错误处理**: TQL 解析错误高亮出错位置

---

## 7. TUI 模式设计

### 布局

```
┌─ TriviumDB ─ mydata.tdb ─ 12,847 nodes ─ dim:1536 ─ f32 ───────────────┐
│                                                                          │
│  ┌─ [1] Graph View ─────────────────┐  ┌─ [2] Node Detail ───────────┐ │
│  │                                   │  │ ID: 42                      │ │
│  │     ●Alice ──knows──► ●Bob        │  │                             │ │
│  │       │                  │        │  │ Payload:                    │ │
│  │    works_with        colleague    │  │   name: "Alice"             │ │
│  │       │                  │        │  │   type: "person"            │ │
│  │       ▼                  ▼        │  │   age: 28                   │ │
│  │     ●Charlie          ●David      │  │                             │ │
│  │                                   │  │ Edges (out): 3              │ │
│  │   [WASD] Navigate  [Enter] Focus  │  │   → 87 (knows, w=0.9)      │ │
│  │                                   │  │   → 91 (works_with, w=0.7)  │ │
│  └───────────────────────────────────┘  │   → 103 (friend, w=0.5)    │ │
│                                          │                             │ │
│                                          │ Vector: [0.12, -0.34, ...]  │ │
│                                          └─────────────────────────────┘ │
│                                                                          │
│  ┌─ [3] TQL Query ──────────────────────────────────────────────────────┐│
│  │ FIND {type: "person", age: {$gt: 25}} RETURN * LIMIT 10             ││
│  └──────────────────────────────────────────────────────────────────────┘│
│                                                                          │
│  ┌─ [4] Results ────────────────────────────────────────────────────────┐│
│  │  # │ id  │ name    │ type   │ age │ edges                           ││
│  │  1 │  42 │ Alice   │ person │  28 │ 3 out                           ││
│  │► 2 │  87 │ Bob     │ person │  31 │ 1 out                           ││
│  │  3 │  91 │ Charlie │ person │   27 │ 2 out                           ││
│  └──────────────────────────────────────────────────────────────────────┘│
│                                                                          │
│ [Tab] Panel  [F5] Run  [/] Filter  [?] Help  [q] Quit      3.2ms  12847n│
└──────────────────────────────────────────────────────────────────────────┘
```

### 面板功能详细设计

#### 7.1 Graph View（图谱面板）

- **渲染方式**: 基于力导向布局的 ASCII 图（简化版 Fruchterman-Reingold）
- **交互**:
  - `W/A/S/D` 或方向键：平移视口
  - `+/-`：缩放
  - `Enter`：选中节点 → 联动 Node Detail
  - `E`：展开选中节点的 1-hop 邻居
  - `L`：切换边标签显示
- **颜色编码**:
  - 不同 label 类型的边用不同颜色
  - 选中节点高亮
  - 搜索命中节点标记

#### 7.2 Node Detail（节点详情）

- 显示选中节点的完整信息
- Payload JSON 树形可折叠展开
- 出边/入边列表（可翻页）
- 向量摘要（维度、L2 范数、前 8 维值）

#### 7.3 TQL Query（查询编辑器）

- 单行/多行编辑
- TQL 关键词语法高亮
- `F5` 或 `Ctrl+Enter`：执行
- `↑/↓`：历史浏览
- 执行结果自动填充 Results 面板

#### 7.4 Results Table（结果表格）

- 分页浏览（`PageUp/PageDown`）
- `↑/↓` 选中行 → 联动 Graph View 和 Node Detail
- `Enter` 在 Graph View 中聚焦该节点
- 列宽自适应

#### 7.5 Status Bar（底部状态栏）

- 左侧：快捷键提示
- 右侧：查询耗时 + 节点总数 + 内存占用

### 键绑定总览

| 按键 | 全局/面板 | 功能 |
|------|-----------|------|
| `Tab` | 全局 | 切换活跃面板 |
| `q` / `Ctrl+C` | 全局 | 退出 |
| `?` | 全局 | 帮助浮层 |
| `F5` | Query | 执行 TQL |
| `/` | 全局 | 聚焦 Query 面板 |
| `Enter` | Results | 选中节点联动 |
| `E` | Graph | 展开邻居 |
| `R` | Graph | 重置视图 |
| `Ctrl+S` | 全局 | flush 落盘 |

---

## 8. 技术栈与依赖

### CLI crate 依赖

| 依赖 | 版本 | 用途 |
|------|------|------|
| `triviumdb` | path = ".." | 核心数据库引擎 |
| `clap` | 4.x (derive) | 命令行解析 |
| `rustyline` | 14.x | REPL 历史/补全/多行 |
| `ratatui` | 0.29.x | TUI 终端渲染框架 |
| `crossterm` | 0.28.x | 跨平台终端控制 |
| `tabled` | 0.17.x | 表格输出 (REPL/非交互) |
| `colored` | 2.x | 彩色文本 |
| `serde_json` | 1.0 | JSON 格式化 |
| `unicode-width` | 0.2.x | Unicode 字符宽度 (表格对齐) |

### 可选未来依赖

| 依赖 | 用途 |
|------|------|
| `syntect` | TQL 语法高亮 (REPL) |
| `indicatif` | 进度条 (导入/导出) |
| `dialoguer` | 交互式确认 (危险操作) |

---

## 9. 开发计划与分工

### Phase 0: 基础设施 (1-2 天)

- [ ] 根 Cargo.toml 添加 `[workspace]`
- [ ] 创建 `cli/` 目录和 `cli/Cargo.toml`
- [ ] `main.rs` + clap 骨架，确保 `cargo build -p triviumdb-cli` 通过
- [ ] `db_handle.rs` — 实现 DbHandle enum + open + dtype 分发宏
- [ ] 移除或 deprecate 旧的 `src/bin/trivium_repair.rs`（逻辑迁入 `commands/repair.rs`）

### Phase 1: 共享层 (3-5 天)

- [ ] `commands/info.rs` — 读取文件头 + 统计信息
- [ ] `commands/exec.rs` — TQL 执行 + 结果收集
- [ ] `commands/repair.rs` — check / dump (迁移自 trivium_repair)
- [ ] `commands/export.rs` — JSONL 导出
- [ ] `commands/import.rs` — JSONL 导入
- [ ] `commands/compact.rs` — 手动压缩
- [ ] `formatter.rs` — table / json / csv 三种格式化

### Phase 2a: REPL (2-3 天) — 可与 Phase 2b 并行

- [ ] REPL 主循环 (rustyline)
- [ ] 元命令解析 (`.info`, `.flush`, `.help`, `.quit`, ...)
- [ ] TQL 多行输入
- [ ] Tab 补全
- [ ] 历史记录持久化
- [ ] 错误位置高亮

### Phase 2b: TUI (5-8 天) — 可与 Phase 2a 并行

- [ ] App state 设计 + event loop (crossterm)
- [ ] 布局框架 (ratatui constraints)
- [ ] Status Bar
- [ ] Results Table (分页、选择、联动)
- [ ] Query Editor (单行编辑、历史、执行)
- [ ] Node Detail (JSON 渲染、边列表)
- [ ] Graph View (ASCII 力导向布局)
- [ ] 面板切换 + 联动逻辑

### Phase 3: 打磨 (持续)

- [ ] TQL 语法高亮
- [ ] 大数据集导入进度条
- [ ] Graph View 交互优化（展开/折叠/缩放）
- [ ] 搜索 Playground (向量输入 + 可视化命中)
- [ ] 配置文件支持 (`~/.triviumdb.toml`)

### 分工建议

| 角色 | 负责模块 |
|------|---------|
| 开发者 A | Phase 0 + 共享层 + REPL |
| 开发者 B | TUI app/event + widgets |
| 共同 | db_handle 接口定义、代码 review |

---

## 10. 未来扩展

当 CLI + TUI 稳定后，可以低成本扩展：

1. **Web UI (Phase 远期)**
   - 在 CLI 中添加 `serve` 子命令，启动 HTTP server (axum)
   - 前端复用 TUI 的交互设计，用 React + Cytoscape.js 重新渲染
   - TUI 和 Web 共享相同的 commands 层

2. **Tauri 桌面应用**
   - 把 Web 前端打包到 Tauri
   - Rust 后端直接复用 commands 层

3. **LSP-like TQL 支持**
   - TQL 补全 / 诊断可以抽成独立模块
   - 供 REPL、TUI、Web Editor 共同使用

4. **插件系统**
   - 自定义 TUI widget (如 embedding 可视化)
   - 自定义导出格式

---

## 附录: 参考项目

| 项目 | 亮点 |
|------|------|
| [lazygit](https://github.com/jesseduffield/lazygit) | TUI 交互设计、面板切换 |
| [k9s](https://github.com/derailed/k9s) | Kubernetes TUI、实时刷新 |
| [gobang](https://github.com/TaKO8Ki/gobang) | 数据库 TUI、SQL 编辑器 |
| [bottom](https://github.com/ClementTsang/bottom) | ratatui 架构、事件循环 |
| [pgcli](https://github.com/dbcli/pgcli) | CLI 补全、语法高亮 |
| [litecli](https://github.com/dbcli/litecli) | SQLite CLI、极致体验 |
