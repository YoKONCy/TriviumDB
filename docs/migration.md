# 外部迁移工具

> 本文说明 `0.9.2` 的外部迁移流程。`scripts/migrate.py` 调用独立的 `triviumdb-migrate` 原生执行器，在副本中恢复旧数据，核对完整事实后发布新目录。版本变更见[发布说明](changes/0.9.2.md)。

---

## 目录

- [支持范围](#支持范围)
- [构建与执行](#构建与执行)
- [事实与资源](#事实与资源)
- [锁、发布与重试](#锁发布与重试)
- [结果与错误](#结果与错误)
- [验收样本](#验收样本)

---

## 支持范围

| 项目 | 当前执行器 |
| --- | --- |
| 工具身份 | `tdb-migrate-2`，编排协议 `1` |
| 来源 | 主格式 `5` 至 `15`，`f32`、`f16`、`u64`，Rom/Mmap |
| 目标 | 当前主格式 `15`、文档结构 `2` 与统一 `.idx` 文件组，Rom/Mmap 可选 |
| 数据范围 | 节点、向量位模式、Payload、默认图边及元数据、登记文本与关键词、索引定义和唯一约束、文档及上传状态、命名图、持久回执 |
| 交付位置 | 新目标目录内的 `database.tdb` 及配套文件 |

历史解码与来源恢复后端位于 [`crates/triviumdb-migrate/source`](../crates/triviumdb-migrate/source/Cargo.toml)，源码身份记录于 [`provenance.json`](../crates/triviumdb-migrate/source/provenance.json)。来源后端拥有独立源码与模块装配，恢复副本后通过版本化逻辑事实协议交付数据。当前内核负责目标写入及核验，生产源码维护当前格式的读取、写入和恢复。

格式 10 起使用 VTYP 类型头核对 dtype。格式 5–9 的非空检查点按节点槽数、维度和完整向量块长度确定元素宽度，`--source-dtype` 可额外核对宿主记录。旧空检查点要求显式提供 `--source-dtype f32/f16/u64`，已确认 WAL 按指定类型恢复。

> **来源提醒**：格式 5 以前、未来格式、无法确定的类型以及缺少可恢复事实的来源会被拒绝。旧属性格式中无法无歧义恢复字段列表的普通复合索引需要先通过来源版本整理，唯一约束使用主文件中保存的完整字段定义。

---

## 构建与执行

从与目标内核相同的源码检出构建迁移工具，保留该检出的独立锁文件：

```bash
cargo build --locked --manifest-path crates/triviumdb-migrate/Cargo.toml --target-dir target/migration
python scripts/migrate.py --source /data/old.tdb --destination /data/current --check
python scripts/migrate.py --source /data/old.tdb --destination /data/current --execute
```

Windows 可以使用带引号的绝对路径，目标父目录须已存在。脚本按 `--tool`、PATH、仓库 `target/migration/release`、`target/migration/debug` 查找执行器，调用前核对协议身份。默认操作为 `--check`。独立构建目录保存迁移器与来源后端的构建制品。

```powershell
python scripts/migrate.py --source 'D:\data\old.tdb' --destination 'D:\data\current' --execute --storage mmap --memory-mib 256 --max-source-mib 1024 --max-work-mib 8192
```

旧空检查点及其 WAL 可以按宿主保存的 dtype 显式执行检查和迁移。

```bash
python scripts/migrate.py --source /data/legacy.tdb --source-dtype f32 --destination /data/current --check
python scripts/migrate.py --source /data/legacy.tdb --source-dtype f32 --destination /data/current --execute
```

检查模式在目标父目录的独占临时目录完成同样的恢复、转换和内容核验，成功后清理临时副本。来源文件集合、长度和内容保持不变。执行模式在核验后发布新目录，原来源数据保留。

> **操作提醒**：停止持有来源的 Writer 和 ReadOnly 实例，暂停上层切换来源的任务。检查模式要求来源已经有 `.lock`；缺少时先用来源版本正常打开并关闭数据库。执行模式允许创建来源协调锁文件，数据制品仍保持不变。

---

## 事实与资源

工具先获取来源排他租约，记录完整制品的大小与 SHA-256，再复制至自己的暂存目录。旧 WAL 与发布日志只在该副本中恢复。转换前后按稳定顺序比较节点与边、索引定义、文本、命名图、文档和回执摘要，并用当前内核正常重开目标核验；发布前再次核对来源制品摘要。

来源后端把普通文档及片段的受管理标签统一为结构 2，保留正文、来源键、范围、NodeId、修订、业务 Payload 和原事务确认结果。来源摘要按统一后的逻辑事实计算，再与目标的完整事实逐组比较。当前内核通过逻辑事实接口建立目标文件组，来源文件组与目标文件组分别位于独立暂存子目录。

`u64` 向量按原始整数位模式保留，浮点向量按实际 dtype 字节保留。登记文本与关键词属于持久事实，随 `.idx` 交付。压缩热内存/SSD 制品属于派生缓存，迁移目标清除旧压缩缓存；宿主按自己的编码、查询精度和缓存预算重新配置。原始向量事实始终保留。

目标是普通数据库文件组。来源快照 manifest 留在来源，目标需要作为 Immutable 代际分发时，再通过当前 SDK 发布新快照或代际。

| 限制 | 默认值与行为 |
| --- | --- |
| 内存准入 | `--memory-mib 256`，最小 `16`；复用内核加载与摘要工作区预算 |
| 来源文件字节 | `--max-source-mib 1024`；按真实制品累计 |
| 来源文件数 | 最多 `4096` |
| 工作磁盘预检 | `--max-work-mib 8192`；保守估计为来源大小六倍、内存预算八倍与 64 MiB 固定余量之和，同时检查可用空间 |
| 文件复制 | 固定大小缓冲，核对长度与摘要 |
| 工作线程 | 原生命令固定两个 Rayon 工作线程 |

> **资源提醒**：磁盘预检给出迁移准入估计，实际文件系统 I/O 错误仍会返回。为来源、候选副本、发布备份及目标保留空间。内核加载预算约束托管数据与工作区，进程运行时和操作系统页缓存另计。

---

## 锁、发布与重试

迁移全程持有来源排他锁；执行模式同时持有目标父目录中的 `.<目标名>.migration.lock`。目标暂存目录保留 `owner.json`，记录工具身份、规范来源路径、目标路径和来源摘要。重试仅清理归属完全匹配、且后代均为普通文件/目录的暂存区。

提交点是将已同步的候选目录原子、排他地重命名为目标目录。候选包含 `database.tdb.migration` 发布标记，普通 SDK 看到它会拒绝打开。目录发布后，工具取得目标数据库共享锁、复核制品清单、移除并同步标记，再用当前 SDK 重开核验。该顺序适配 Windows 目录重命名与文件锁规则。

发布前中断保留来源和归属明确的暂存目录；发布后中断保留已提交目标及发布标记。使用相同来源和目标重新执行原命令，工具重新转换来源副本、核对归属和目标事实后完成确认。已经成功发布且事实相同的重试返回 `reused: true`。

> **重试提醒**：已存在的其他目录、归属不匹配、目标已被业务修改或目标制品损坏均保留现场并返回错误。保留协调锁文件；直接删除正在使用的锁文件会破坏进程间互斥。

---

## 结果与错误

成功 JSON 写到 stdout，包括 `protocol`、`tool`、`source_format`、`target_format`、`dtype`、`checked`、`committed`、`reused`、`database` 和 `facts`。`facts` 含计数、下一节点 ID、各事实组 SHA-256 及总摘要。目标目录保存 `migration-source.json` 和 `migration-report.json`；报告中的提交状态对应目录提交点，SDK 可打开性还取决于发布标记完成确认。

失败 JSON 写到 stderr 的 `error` 字段，包含 `code`、`message`、`phase`、`outcome` 和 `details`，命令退出码为 `1`。发布后的失败标记 `outcome: "committed"`，调用方按重试流程核验目标。

| 工具错误码 | 含义 |
| --- | --- |
| `TDB_MIGRATION_LEASE_REQUIRED` | 检查模式缺少既有来源锁文件 |
| `TDB_MIGRATION_BUDGET` | 来源数量、字节或工作空间预算不足 |
| `TDB_MIGRATION_TARGET_EXISTS` | 目标存在且归属不匹配 |
| `TDB_MIGRATION_CONFLICT` | 来源摘要变化、暂存归属冲突或已交付事实变化 |
| `TDB_MIGRATION_UNSUPPORTED_SOURCE` | 工具支持范围外的来源格式 |
| `TDB_MIGRATION_DTYPE_REQUIRED` | 无类型头的旧空检查点需要宿主显式确认 dtype |
| `TDB_DATABASE_LOCKED` | 来源/目标正在使用或等待发布确认 |
| 内核错误码 | 保留原始错误类别与必要字段，例如损坏、I/O、容量和唯一约束 |

普通 SDK 的 `TDB_MIGRATION_REQUIRED` 只提供迁移指引。`--check`、转换和目标发布均由用户显式调用外部脚本完成。

---

## 验收样本

[`release_090`](../tests/fixtures/release_090/manifest.json) 来自不可变 Tag `v0.9.0` 的源码，保存生成程序、提交身份和逐文件摘要，覆盖三种 dtype、两种存储模式、检查点及已确认 WAL。[`release_091`](../tests/fixtures/release_091/manifest.json) 来自不可变 `v0.9.1` 源码，包含 Rom/Mmap 的解析文档、来源映射、命名图和持久回执。

| 来源格式 | 独立样本与来源 | 数据与验证范围 |
| --- | --- | --- |
| 5–9 | 独立小端布局样本，另含 `v0810_text_mmap` 原生制品 | 三种 dtype、Rom/Mmap 目标、默认边与元数据、旧空检查点及 WAL、显式正文与关键词 |
| 10 | `release_090`，`v0.9.0` 原始源码 | 三种 dtype、Rom/Mmap、检查点与已确认 WAL、文本、索引约束、边元数据 |
| 11 | [`pre_named_graph_format11`](../tests/fixtures/pre_named_graph_format11/provenance.json)，命名图实装前的原生 wheel | 三种 dtype、Mmap 检查点与 WAL 4、默认图边，另含 f32 事务回执及原确认令牌 |
| 12 | [`pre_document_publication_format12`](../tests/fixtures/pre_document_publication_format12/provenance.json)，文档分批发布实装前的原生 wheel | 三种 dtype、Mmap 检查点与 WAL 5、命名图成员、边元数据及图修订 |
| 13 | `release_091`，`v0.9.1` 原始源码 | f32、Rom/Mmap、解析文档、来源映射、命名图及持久回执 |
| 13 | [`document_schema1`](../tests/fixtures/document_schema1/manifest.json)，`v0.9.1` 原始源码 | 普通文档结构 1、片段、唯一约束和旧回执，升级后核对结构 2 与原确认令牌 |
| 14 | [`pre_externalization_format14`](../tests/fixtures/pre_externalization_format14/manifest.json)，本轮实施前的独立工作区源码快照 | Rom/Mmap 统一容器、普通文档结构 1、片段、唯一约束及原确认令牌 |
| 15 | 当前公开 SDK 生成 | 文档、命名图、回执及完整事实往返 |

格式 11/12 验收逐文件核对既有来源记录中的大小与 SHA-256，历史 `case.json` 通过 Git 属性保留捕获时的换行字节。各 dtype 分别迁入 Rom/Mmap 目标，经过检查、执行及同源重试，再以只读和可写实例核对全部节点、向量、Payload、边与图身份。后续写入验证自动 ID 水位、唯一约束及再次重开。旧回执的状态查询和原命令幂等重试均保留原确认结果。脚本入口另以两组 u64 样本核对检查、发布、重试和源文件零写。

迁移验收核对向量、Payload、边元数据、文本、索引约束及再次重开，另覆盖当前文档/命名图事实、未提交事务与截断 WAL 尾帧、租约冲突、预算、损坏、发布中断和重试。

```bash
cargo test --locked --manifest-path crates/triviumdb-migrate/Cargo.toml --target-dir target/migration -- --test-threads=1
```

各平台运行结果以对应提交的 CI 为准。本地与 CI 的验收范围见[测试实践](testing.md)。
