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

`--format json` 模式下，内核错误写入 stderr，字段为 `schema_version: 1`、`code`、`message`、`details`，进程退出码为 1。stdout 保留给结果数据；TQL 解析失败同样通过此结构返回。文本模式在诊断中显示稳定错误码。

`tdb exec` 按 TQL 投影返回指定列，列名使用 `AS` 别名。JSON 输出保留数值、布尔值、空值、对象与列表。CSV 输出完整单元格并转义逗号、引号及换行。写入成功后，`--mutate --format json` 在 stdout 返回 `{"affected":1,"created_ids":[3]}` 形式的确认结果。CSV 使用 `affected,created_ids` 两列，后者为 JSON 数组。

`info`、`export` 和未指定 `--mutate` 的 `exec` 使用共享读锁，保持源文件组零写。CSV 查询即使返回零行也输出投影列名。`info` 从 `.idx` 目录读取已持久化的 QuIVer 组件状态。

`repair check` 和 `repair dump` 需要存在主库或 WAL。`repair dump --format json` 在 stdout 按节点 ID 升序输出 JSON 数组，挂载诊断写入 stderr。

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

> **维度提醒**：QuIVer ANN 支持最多 3072 维。更高维度使用精确 BruteForce 检索，显式构建 QuIVer 返回错误。

## JSONL 导入/导出格式

`tdb export` 按节点 ID 升序输出 JSONL，每行包含原始 dtype 的向量、Payload 和默认图的完整出边。

```json
{"id":1,"vector":[1.0,0.0,0.0,0.0],"payload":{"name":"Alice"},"edges":[{"target":2,"label":"knows","weight":1.0,"metadata":{"rank":7,"source":"manual"}}]}
```

`tdb import` 完整解析文件后，将全部节点与边提交为同一笔内核事务，随后执行检查点。解析过程中向量按目标 dtype 的原生表示缓冲到事务，边引用在事务写锁内解析。省略 ID 的记录获得自动分配的真实 ID，其出边参与相同提交；目标可以是已有节点或本批后续节点。

- `vector` 必填，使用符合目标 dtype 与维度的非空数组。f32／f16 按内核规则转换并验证有限性。u64 分量使用 `0..=18446744073709551615` 的无符号整数字面量，以保留完整位模式。
- `id` 可选，显式 ID 使用有效的非零 u64，分配遵循内核的 ID 容量边界。
- `payload` 可选，缺省为 `null`。
- `edges` 可选，每条边的 `target` 引用有效节点。`label` 缺省为空字符串，`weight` 缺省为 `1.0` 并验证有限 f32 范围，`metadata` 保留任意 JSON 值，缺省为 `null`。

维度、重复 ID、整数表示或边端点校验失败时，命令以非零状态退出，整批数据保持提交前内容。进入 WAL 后的 I/O 错误通过核心写入结果报告提交阶段与确认状态。

导出先写入同目录暂存文件，完成序列化和同步后原子替换目标文件。路径检查覆盖主库、WAL、文本及图索引、Payload 代际、发布日志、QuIVer SSD 制品及其临时文件，并检查实际文件身份以识别硬链接和符号链接别名。

> **精度提醒**　u64 JSONL 向量使用整数字面量。浮点和指数表示在解析时可能发生舍入，导入入口会拒绝这些表示。此前经过 f32 舍入的导出文件，需要从原始数据库或完整备份重新导出。

> **备份提醒**　完整数据库备份使用快照接口，保留文档正文、命名图、文本登记、索引定义及关联文件。JSONL 适用于上述节点与默认图边的数据交换。

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

- 导出目标使用独立文件名，例如 `backup.jsonl`。数据库制品名称族由 CLI 路径保护检查维护。
- **导入会写入数据库**，建议对重要数据先备份。
- **创建新库时必须提供 `--dim`**，否则无法确定向量维度。

更多细节请参阅源码及根目录 [README](../../README.md)。
