# triviumdb-cli

TriviumDB 的统一命令行工具（包名 `triviumdb-cli`，命令名 `tdb`），提供三种交互模式：

- **非交互命令**：`tdb info`、`tdb exec`、`tdb export`、`tdb import`、`tdb repair`、`tdb compact`
- **REPL**：`tdb open <db>` 进入交互式 TQL 终端
- **TUI**：`tdb ui <db>` 进入全屏可视化面板（图谱 / 查询 / 结果 / 节点详情）

## 构建

```bash
cargo build -p triviumdb-cli --release
```

产物：`target/release/tdb`

## 命令参考

```bash
# 查看数据库元信息
tdb info mydata.tdb

# 非交互执行只读 TQL
tdb exec mydata.tdb 'MATCH (n) RETURN n LIMIT 5'

# 非交互执行写入 TQL
tdb exec mydata.tdb 'CREATE (n {name: "Alice"})' --mutate

# 导出全部节点为 JSONL
tdb export mydata.tdb backup.jsonl

# 从 JSONL 导入；创建新库时需要指定维度
tdb import newdata.tdb backup.jsonl --dim 4

# 快速检查数据库文件头与 WAL 状态
tdb repair check mydata.tdb

# 强制挂载并输出全部节点
tdb repair dump mydata.tdb --format json

# 手动压缩
tdb compact mydata.tdb

# 进入 REPL
tdb open mydata.tdb

# 进入 TUI 可视化
tdb ui mydata.tdb
```

全局参数：

- **`--format <table|json|csv>`**：控制输出格式。
- **`--color <auto|always|never>`**：控制彩色输出。
- **`--dtype <f32|f16|u64>`**：控制数据库向量元素类型。
- **`--dim <N>`**：创建新库或无法嗅探文件头时指定向量维度。

> ⚠️ **强烈建议 `--dim` 不超过 3072。** 更高维度仍可使用精确 BruteForce 检索，但不会启用 QuIVer ANN 加速；显式构建 QuIVer 会被安全拒绝。

## JSONL 导入/导出格式

`tdb export` 每行输出一个 JSON 对象：

```json
{"id":1,"vector":[1.0,0.0,0.0,0.0],"payload":{"name":"Alice"},"edges":[{"target":2,"label":"knows","weight":1.0}]}
```

`tdb import` 会严格校验：

- **`vector`**：必填，必须是非空数字数组。
- **`id`**：可选，必须是非负整数。
- **`payload`**：可选，缺省为 `null`。
- **`edges`**：可选，必须是数组；每条边的 `target` 必须是非负整数。

## REPL 元命令

在 `tdb open <db>` 中可用：

- **`.info`**：显示数据库元信息。
- **`.stats`**：显示实时统计。
- **`.schema`**：采样 payload 字段分布。
- **`.flush`**：手动落盘。
- **`.compact`**：手动压缩。
- **`.export <file.jsonl>`**：导出全部节点。
- **`.format <table|json|csv>`**：切换输出格式。
- **`.help`**：显示帮助。
- **`.quit` / `.exit` / `.q`**：退出。

REPL 支持多行 TQL；普通 TQL 语句需以分号结束。

## TUI 快捷键

在 `tdb ui <db>` 中可用：

查询编辑器（多行）：

- **`Enter`**：换行。
- **`Ctrl+Enter`**：执行当前查询。
- **`Tab` / `Esc`**：切换到结果面板。
- **`←/→/↑/↓`**：移动光标；`Home/End` 跳到行首/尾。
- **`Backspace` / `Delete`**：删除字符；行首处合并行。

结果面板：

- **`/`**：跳到查询区。
- **`g`**：在表格视图与图视图之间切换。
- **`s`**：基于当前节点执行相似搜索。
- **`?`**：显示 / 隐藏帮助。
- **`q` / `Ctrl-C`**：退出。

图视图（`g` 进入）：

- **`e` / `c`**：展开 / 折叠选中节点邻居。
- **`+` / `-`**：缩放；**`Shift + 方向键`** 平移；**`f`** 复位视图。
- **`m`**：循环切换字符渲染（Braille → Dot → Block → HalfBlock）。

## TQL 错误位置高亮

执行非法查询时：

- **REPL / `tdb exec`**：打印带 caret 的多行错误信息（仿 rustc）。
- **TUI**：查询编辑器边框变红、状态栏显示 `line/col`，错误列字符红色高亮。
- 编辑查询会自动清除错误标记。

## 配置文件

可选的 `~/.triviumdb.toml` 提供默认值（优先级：**命令行参数 > 配置 > 内置默认**）：

```toml
[defaults]
dtype  = "f32"      # f32 | f16 | u64
format = "table"    # table | json | csv

[tui]
default_limit = 50  # TUI 启动默认 MATCH (n) ... LIMIT N
graph_marker  = "auto"  # auto | braille | dot | block | half_block
```

`graph_marker = "auto"`（默认）会根据终端环境自动选择：

- **Windows Terminal / VS Code / JetBrains** → Braille（最高密度）
- **传统 cmd.exe / 老 PowerShell**（Braille 字体支持差） → 自动降级到 Dot
- **其他平台** → 默认 Braille

如果发现图视图字符渲染异常，可在 TUI 内按 `m` 实时切换；或在配置中显式指定。

## 安全注意事项

- **不要把导出文件写到数据库路径或 sidecar 文件路径**，例如 `mydata.tdb`、`mydata.tdb.vec`、`mydata.tdb.wal`。CLI 会拒绝这些危险路径。
- **导入会写入数据库**，建议对重要数据先备份。
- **创建新库时必须提供 `--dim`**，否则无法确定向量维度。

更多细节请参阅源码及根目录 [README](../README.md)。
