# triviumdb-cli

TriviumDB 的统一命令行工具，提供三种交互模式：

- **非交互命令**：`triviumdb exec <db> "<TQL>"`、`info`、`export`、`import`、`repair`、`compact`
- **REPL**：`triviumdb open <db>` 进入交互式 TQL 终端
- **TUI**：`triviumdb ui <db>` 进入全屏可视化面板（图谱 / 查询 / 结果 / 节点详情）

## 构建

```bash
cargo build -p triviumdb-cli --release
```

产物：`target/release/triviumdb`

## 用法

```bash
# 查看数据库元信息
triviumdb info mydata.tdb

# 非交互执行 TQL
triviumdb exec mydata.tdb "FIND {type: 'person'} RETURN * LIMIT 5"

# 进入 REPL
triviumdb open mydata.tdb

# 进入 TUI 可视化
triviumdb ui mydata.tdb
```

详见 [`docs/cli-design.md`](../docs/cli-design.md)。
