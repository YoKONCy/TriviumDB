# triviumdb-cli

TriviumDB 的统一命令行工具（包名 `triviumdb-cli`，命令名 `tdb`），提供三种交互模式：

- **非交互命令**：`tdb exec <db> "<TQL>"`、`info`、`export`、`import`、`repair`、`compact`
- **REPL**：`tdb open <db>` 进入交互式 TQL 终端
- **TUI**：`tdb ui <db>` 进入全屏可视化面板（图谱 / 查询 / 结果 / 节点详情）

## 构建

```bash
cargo build -p triviumdb-cli --release
```

产物：`target/release/tdb`

## 用法

```bash
# 查看数据库元信息
tdb info mydata.tdb

# 非交互执行 TQL
tdb exec mydata.tdb 'FIND {type: "person"} RETURN * LIMIT 5'

# 进入 REPL
tdb open mydata.tdb

# 进入 TUI 可视化
tdb ui mydata.tdb
```

详见 [`docs/cli-design.md`](../docs/cli-design.md)。
