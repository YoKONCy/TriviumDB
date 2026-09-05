# TriviumDB 贡献指南

感谢你愿意为 TriviumDB 做出贡献。

本项目是一个快速迭代中的嵌入式三模数据库（向量 / 图 / 文档），发布链路涉及 Rust crate、Python wheel、Node 原生模块、CLI 与 nightly Server 的多平台构建。因此分支与发布流程比普通项目更严格，请先阅读以下规则。

### 分支模型

项目采用 `master` + `dev` 双长期分支模型：

```text
feature/*、fix/*、docs/*
        │
        ▼
   PR → dev（集成与验收）
        │
        ▼
Release PR：dev → master
        │
        ▼
在 master 合并提交上打 vX.Y.Z → 触发正式 CD
```

两个长期分支都受到分支保护，任何人（包括维护者）都不允许直接 push：

- `master` 只保存可正式发布的状态，只接受来自 `dev` 的 Release PR（由 CI Guard 强制校验 PR 来源分支）；
- `dev` 是集成分支，接受所有功能、修复与文档 PR。

### 目标分支

大多数 Pull Request 应提交到 `dev` 分支。

直接提交到 `master` 的 PR 会被 CI 一律拒绝，没有例外。如果发现影响当前稳定版本的数据安全、损坏或发布阻断问题，请提交 Issue 详细描述影响与复现方式；紧急修复由维护者通过内部流程处理，不接受外部 hotfix PR。

如果不确定目标分支，请优先选择 `dev`，并在 PR 说明中说明背景。

### Tag 与发布

项目采用以下发布成熟度顺序：

```text
nightly < alpha < beta < rc < stable（无后缀） < hotfix
```

这是项目的发布通道与验收等级，不是对 SemVer 预发布标识符排序规则的改写。`nightly`、`alpha` 与 `rc` 可以通过分支、构建元数据或制品通道标识，但**不创建 Git Tag**。`hotfix` 表示稳定版之后的紧急维护等级，必须通过递增补丁版本发布（例如 `v0.8.6` → `v0.8.7`），不得使用 `v0.8.6-hotfix` 作为高于稳定版的 SemVer 版本。

仓库只允许以下两类 Git Tag，它们都必须能被 Cargo/crates.io、NPM 与 PyPI 三端一致识别：

- **Beta Tag**：`vX.Y.Z-beta.N`，只用于已达到公开测试条件的候选版本；发布到三端的预发布通道，不得覆盖或冒充 stable；
- **Stable Tag**：`vX.Y.Z`，只能创建在 `master` 上已通过验收的 Release Commit，并触发三端正式发布；hotfix 使用下一个 stable 补丁 Tag；
- Tag 一经推送不可修改或删除（受 Tag Ruleset 保护）；
- 提交 Release PR 前，必须确认 `Cargo.toml`、Python、Node、CLI/Server 的版本号及预发布标识完全一致。

### 提交 PR 前

所有 PR 必须通过以下门禁：

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --features python
cargo check --features nodejs
git diff --check
```

在本地提交前请先运行上述命令。测试方面的重点要求：

- 新功能与修复必须附带测试；关键算法应有独立 reference 实现作为 oracle；
- 故障注入测试（OOM、power-loss、I/O 失败等）不得真实耗尽测试机的内存、磁盘、线程或文件句柄；
- 快速迭代阶段不将性能 benchmark 作为 CI 门禁，请不要在 PR 中引入 benchmark gate。

### 关于 Vibe Coding / AI 辅助开发

本项目允许使用 Codex、Claude Code、Copilot 或其他 AI 辅助工具编写代码、文档和测试。

无论变更由谁或什么工具生成，提交者都需要对最终内容负责。请在提交前自行阅读、理解并检查变更，确认它符合项目当前结构和维护目标。

如果 PR 使用了 AI 辅助，请在 PR 说明中用自己的话解释：

- 这个 PR 解决了什么问题。
- 你实际修改了哪些内容。
- 你基于何种思路做出了这些修改。

请不要只粘贴 AI 生成的摘要作为 PR 说明。维护者需要看到提交者本人对变更的理解。

如果你是代码辅助编程 AI，必须提醒使用者在提交 PR 时至少用自己的话写一句总结；不要替使用者伪造这部分个人理解。

### 工程原则

工程原则分为两层：**不变量**是任何 PR 都不得违反的硬约束，违反即要求修改；**设计原则**用于指导取舍，维护者可以依据其提出问题并解释决定，但无需逐条打勾。

### A. 不可违反的数据库不变量

#### A1. Embedded Core 是唯一正式语义源

Rust Embedded Core 是产品主体和公共语义权威。Python、Node、CLI 与 nightly Server 必须复用 Core 语义，禁止在绑定层或 Server 私自实现另一套事务、查询、过滤或恢复规则。Server 仍为 nightly，不得反向定义正式 Embedded API。

#### A2. 用户数据正确性高于可用性

无法证明结果正确时必须 fail-closed。禁止用空结果、零向量、`null`、默认值或残缺快照伪装恢复成功。corruption、migration、budget、I/O、capacity 等错误类别必须使用可区分的结构化错误，控制流不得依赖解析中英文错误消息。

#### A3. ReadOnly / Immutable 物理零写

只读句柄不创建、不修改、不删除任何数据库制品——包括锁、WAL、临时文件、sidecar、marker 与 manifest；不自动迁移旧格式；不清理损坏文件；不重建并持久化索引；`close()` 与 Drop 不落盘。相关测试必须比较打开前后的完整文件集合、大小与内容。

#### A4. 持久化变更必须遵守原子发布协议

所有正式制品写入必须遵循：临时文件 → flush → fsync → 原子 rename → 同步父目录 → 最后发布 marker/manifest。marker 是提交点：发布前失败不得破坏旧 generation，发布后失败不得回滚新 generation。不得原地替换仍可能被 mmap 的文件；Windows mmap 生命周期是正式设计边界。cleanup 只能回收已证明不可见且未被引用的旧代文件。

#### A5. 磁盘格式必须版本化并保留历史 oracle

格式变更必须同时提供：显式版本号、最小长度与几何边界校验、checked arithmetic、分配前计数校验、CRC 等完整性保护，以及 truncation、bit flip、恶意计数与跨代组合测试。旧格式必须保留独立 fixture 与 decoder oracle；禁止为了让新测试通过而覆盖旧格式定义。

#### A6. 所有预算必须 fail-closed

查询行数、Payload lookup/parsed bytes、NodeSet、向量读取、图遍历、图算法工作区、并行线程、索引构建峰值与容量预留，都必须在大分配或无界工作开始前检查。不得静默截断需要完整语义的结果、自动扩大预算、先分配后检查、通过真实耗尽机器资源来测试 OOM，也不得让缓存命中与否改变预算约束下的查询结果。

#### A7. 生产可达路径不得 panic

生产代码禁止显式 `panic!`、`unreachable!`、未证明安全的 `unwrap()` / `expect()`、未检查的整数运算，以及通过 unsafe 延长引用生命周期。`unsafe` 必须局部化、附带中文 SAFETY 契约、说明长度/对齐/生命周期/并发条件，并有截断、畸形与边界测试。

#### A8. 查询行为必须确定

相同数据、配置与查询必须产生相同结果，不受 HashMap 遍历顺序、Rayon 线程数、wall clock、cache hit/miss、瞬时 RSS 或同分候选进入顺序影响。所有排序必须有稳定 tie-break（最终通常落在 NodeId）。

#### A9. Cache 与索引只能改变性能

删除 cache 或设置 cache=0 后结果必须完全相同；cache 淘汰不得使进行中的查询持有悬空引用；索引候选必须经完整谓词验证；索引缺失、损坏或重建不得污染事实数据。QuIVer、BQ、属性、文本与图 sidecar 都是加速层，不是事实来源。

#### A10. Late Materialization 是强制不变量

纯向量候选阶段必须满足 `payload_lookup_count = 0` 且 `payload_parsed_bytes = 0`。正确顺序是：BQ/QuIVer/Exact 候选生成 → 精确精排 → Top-K/rerank pool 截断 → 必要时 Payload hydrate。只有 Payload 谓词、字段排序/聚合或公共返回值确实需要时才允许解析。

#### A11. 事务与 WAL 保持全原子语义

尽可能把所有可能失败的检查前置到 WAL 之前；批量操作不得产生半批状态；CAS、Unique、Atomic Delete 与普通 CRUD 共享同一事务事实；未提交事务与截断尾帧不得泄漏；恢复后结果必须确定且可再次打开。

#### A12. 三端必须同步变更

公共结构性变更必须同时覆盖 Rust API、PyO3、napi-rs、`.pyi`、`.d.ts`、错误映射、默认值、参数边界与共享 JSON 契约测试。不得以"稍后再补绑定"为默认做法；若某项能力本期不开放，应在所有端一致地保持不可用。

#### A13. 测试必须能证明正确性

核心算法与格式变更至少考虑：独立 reference/oracle、固定 seed 差分、状态机、metamorphic、历史 fixture、格式 mutation、I/O failpoint、allocator failpoint、子进程 power-loss、ReadOnly/Immutable 零写与跨端契约。测试不得复制生产实现充当 reference——那只能证明两份相同代码得出相同结果。

#### A14. QuIVer 与 ANN 算法的产品边界

QuIVer 是 TriviumDB 的核心 ANN 底座与主要产品卖点。以下规则区分两类变更：

- **新增 ANN 算法：允许。** 作为可选后端或实验路径接入时不触碰 QuIVer 语义，需保证确定性、事务安全、Late Materialization 不变量与完整测试；
- **直接修改 QuIVer 核心：必须提供跨数据集帕累托前沿证据。** 调整 BQ 编码、图导航、精排、构建或持久化语义时，必须在多个数据集与多个 Recall 工作点上同时对比 QPS、延迟与内存，证明新实现整体不劣且在关键指标上明显更优，才可合并。单一工作点或单一数据集的局部胜利不足以替换已发表并经过广泛实测的基线。

无论哪类变更，都不得破坏 Recall 语义、稳定排序、事务安全与确定性；性能数据必须可复现。

### B. 设计与维护原则

#### B1. 最小必要改动

Bug 修复不顺带重构无关模块；不为单次调用创建新抽象；不添加假想兼容层、无用 re-export 或占位 helper；确定无用的代码直接删除；新文件必须有明确且独立的长期职责。

#### B2. 单一事实来源

禁止同时存在两个权威 parsed Payload、两套格式版本解释、Core 与绑定层各自实现的过滤、文档与源码不一致的默认值，或 Planner 与执行器对同一算子的不同语义。

#### B3. 复杂度与证据相称

可维护性与正确性优先于未经测量的局部性能。不以 RSS 单指标证明 mmap 内存收益，需区分堆、映射文件大小、驻留页、PageCache 与临时工作区。真实且测量得到的性能问题可以引入额外复杂度，但应与证据相称并留在合适的边界内。

#### B4. 通用大于特判

先判断若干需求是否共享同一语义，优先扩展一处共同机制；真实差异应当清楚保留。不要为形式统一提前搭建万能抽象，也不要为单一入口增加私有平行实现。

#### B5. 文档必须描述真实能力边界

不夸大单文件、零拷贝、零内存、自动恢复等能力；Rom 与 Mmap 文件集合、Server nightly 与 Embedded 正式能力必须明确区分；格式、默认值与错误码必须与源码一致。README 可以突出卖点，但不能隐藏影响数据安全或迁移的必要条件。

### C. 按变更类型触发的 PR 说明要求

修改以下任一领域时，PR 说明必须列出**不变量、兼容性、故障行为、测试证据**四项，缺失将被要求补充：

- WAL、事务与恢复；
- `.tdb/.vec/.pld/.flush_ok` 磁盘格式；
- mmap 生命周期与 generation；
- Query Planner、预算与执行器；
- QuIVer / BQ / ANN 路径；
- Rust / Python / Node 公共 API。

### Commit 约定

- Commit message 使用详尽的英文，说明"为什么"而不仅是"改了什么"；
- 合并或吸收其他贡献者的有效工作时，必须在 Commit 尾部添加 `Co-authored-by: Name <email>`，并在 Release PR 中链接原 PR/Issue，确保 GitHub 正确记录共同作者；
- 不要在 commit 中包含密钥、临时文件或与变更无关的重排；
- 一次 PR 聚焦一件事；混合了功能、重构与格式化的 PR 难以 review，可能被要求拆分。

---

# Contributing to TriviumDB

Thank you for contributing to TriviumDB.

TriviumDB is a fast-moving embedded tri-model database (vector / graph / document). Its release pipeline spans the Rust crate, Python wheels, native Node modules, the CLI, and the nightly Server across multiple platforms. The branch and release workflow is therefore stricter than a typical project — please read the rules below first.

### Branch Model

The project uses a dual long-lived branch model: `master` + `dev`.

```text
feature/*, fix/*, docs/*
        │
        ▼
   PR → dev (integration and validation)
        │
        ▼
Release PR: dev → master
        │
        ▼
Tag vX.Y.Z on the merged master commit → official CD
```

Both long-lived branches are protected; nobody (including maintainers) may push directly:

- `master` holds only releasable states and accepts Release PRs from `dev` only (a CI guard verifies the source branch of every PR targeting `master`);
- `dev` is the integration branch and accepts all feature, fix, and documentation PRs.

### Target Branch

Most pull requests should target the `dev` branch.

PRs submitted directly against `master` are always rejected by CI, with no exceptions. If you find a data-safety, corruption, or release-blocking issue affecting the current stable version, please open an issue describing the impact and how to reproduce it; urgent fixes are handled by maintainers through an internal process, and external hotfix PRs are not accepted.

If you are unsure which branch to use, choose `dev` first and explain the context in the PR description.

### Tags and Releases

The project uses the following release-maturity order:

```text
nightly < alpha < beta < rc < stable (no suffix) < hotfix
```

This is the project's release-channel and acceptance-level ordering, not a redefinition of SemVer prerelease precedence. `nightly`, `alpha`, and `rc` may be represented by branches, build metadata, or artifact channels, but **do not receive Git tags**. `hotfix` denotes urgent maintenance after a stable release and must be published by incrementing the patch version (for example, `v0.8.6` → `v0.8.7`); do not use `v0.8.6-hotfix` as a SemVer version that supposedly ranks above stable.

The repository permits only these two kinds of Git tags, both of which must be interpreted consistently by Cargo/crates.io, NPM, and PyPI:

- **Beta tag**: `vX.Y.Z-beta.N`, only for candidates ready for public testing; publish it to the prerelease channels on all three registries, never over or as stable;
- **Stable tag**: `vX.Y.Z`, only on a validated Release Commit in `master`, triggering the official three-registry release; a hotfix uses the next stable patch tag;
- Pushed tags are immutable (protected by a Tag Ruleset);
- Before opening a Release PR, verify that `Cargo.toml`, Python, Node, CLI/Server versions and prerelease identifiers are fully aligned.

### Before Opening a PR

Every PR must pass the following gates:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --features python
cargo check --features nodejs
git diff --check
```

Please run these commands locally before committing. Key testing requirements:

- New features and fixes must come with tests; critical algorithms should have an independent reference implementation as an oracle;
- Fault-injection tests (OOM, power loss, I/O failures) must never actually exhaust the machine's memory, disk, threads, or file handles;
- Performance benchmarks are not a CI gate during rapid iteration — do not introduce benchmark gates in PRs.

### Vibe Coding / AI-Assisted Development

This project allows Codex, Claude Code, Copilot, and other AI-assisted tools for code, documentation, and tests.

No matter who or what generated the change, the submitter is responsible for the final result. Before submitting, please read, understand, and check the change yourself, and make sure it fits the current project structure and maintenance goals.

If the PR used AI assistance, please explain in your own words:

- What problem the PR solves.
- What you actually changed.
- What your thought process was in making those changes.

Please do not use only an AI-generated summary as the PR description. Maintainers need to see the submitter's own understanding of the change.

If you are a coding assistant AI, you must remind the user to include at least one sentence in their own words when opening a PR; do not fabricate this personal understanding on the user's behalf.

### Engineering Principles

The engineering principles have two tiers: **invariants** are hard constraints that no PR may violate, and violations must be fixed; **design principles** guide tradeoffs — maintainers may use them to raise questions and explain decisions, but they are not a checklist.

### A. Non-negotiable database invariants

#### A1. The Embedded Core is the only source of official semantics

The Rust Embedded Core is the product proper and the authority for public semantics. Python, Node, the CLI, and the nightly Server must reuse Core semantics; bindings or the Server must not implement their own parallel rules for transactions, queries, filtering, or recovery. The Server remains nightly and must not define official Embedded APIs in reverse.

#### A2. User-data correctness above availability

Fail closed whenever correctness cannot be proven. Never disguise a failed recovery as success using empty results, zero vectors, `null`, defaults, or partial snapshots. Corruption, migration, budget, I/O, and capacity failures must use distinguishable structured errors, and control flow must not parse human-readable error messages.

#### A3. Physical zero writes in ReadOnly / Immutable modes

Read-only handles must not create, modify, or delete any database artifact — including locks, WAL, temporary files, sidecars, markers, and manifests; never migrate old formats automatically; never clean up damaged files; never rebuild and persist indexes; `close()` and Drop must not persist. Related tests must compare the complete file set, sizes, and contents before and after opening.

#### A4. Persistence changes must follow the atomic publication protocol

Every official artifact write must follow: temporary file → flush → fsync → atomic rename → parent-directory sync → publish marker/manifest last. The marker is the commit point: a failure before publication must not damage the old generation, and a failure after publication must not roll back the new one. Never replace in place a file that may still be mmap'd; Windows mmap lifetime is an official design boundary. Cleanup may only reclaim old-generation files proven invisible and unreferenced.

#### A5. Disk formats must be versioned with preserved historical oracles

A format change must provide: an explicit version number, minimum-length and geometry validation, checked arithmetic, pre-allocation count validation, integrity protection such as CRC, and tests covering truncation, bit flips, malicious counts, and cross-generation combinations. Old formats must keep dedicated fixtures and decoder oracles; never overwrite historical format definitions just to make new tests pass.

#### A6. Every budget must fail closed

Query rows, Payload lookup/parsed bytes, NodeSets, vector reads, graph traversal, graph-algorithm workspaces, parallel threads, index-build peaks, and capacity reservations must all be checked before large allocations or unbounded work. Never silently truncate results that require complete semantics, auto-expand budgets, allocate before checking, test OOM by actually exhausting the machine, or let cache hits change query results under a budget constraint.

#### A7. No panics on production-reachable paths

Production code must not contain explicit `panic!`, `unreachable!`, unproven-safe `unwrap()` / `expect()`, unchecked integer arithmetic, or unsafe lifetime extension of references. `unsafe` must be localized, carry a SAFETY contract comment, state length/alignment/lifetime/concurrency conditions, and have truncation, malformed, and boundary tests.

#### A8. Query behavior must be deterministic

The same data, configuration, and query must produce the same results regardless of HashMap iteration order, Rayon thread count, wall clock, cache hit/miss, transient RSS, or the arrival order of tied candidates. Every sort must have a stable tie-break (ultimately, usually NodeId).

#### A9. Caches and indexes may only change performance

Results must be identical with the cache removed or cache=0; eviction must not leave in-flight queries holding dangling references; index candidates must be verified by the full predicate; missing, damaged, or rebuilt indexes must not pollute ground-truth data. QuIVer, BQ, property, text, and graph sidecars are acceleration layers, not sources of truth.

#### A10. Late materialization is a mandatory invariant

Pure vector candidate generation must satisfy `payload_lookup_count = 0` and `payload_parsed_bytes = 0`. The correct order is: BQ/QuIVer/Exact candidate generation → exact rerank → Top-K/rerank-pool truncation → Payload hydration only when needed. Parse only when a Payload predicate, field ordering/aggregation, or a public return value genuinely requires it.

#### A11. Transactions and WAL keep full atomic semantics

Move every fallible check ahead of the WAL write whenever possible; batch operations must never produce half-applied states; CAS, Unique, and Atomic Delete share the same transactional ground truth as plain CRUD; uncommitted transactions and truncated tail frames must not leak; recovery results must be deterministic and reopenable.

#### A12. The three language surfaces must change together

Any public structural change must cover the Rust API, PyO3, napi-rs, `.pyi`, `.d.ts`, error mapping, defaults, parameter bounds, and the shared JSON contract tests. "Bindings later" is not a default practice; if a capability is intentionally unavailable this cycle, keep it unavailable consistently on all surfaces.

#### A13. Tests must prove correctness

For core algorithms and format changes, consider at least: an independent reference/oracle, fixed-seed differential tests, state machines, metamorphic tests, historical fixtures, format mutations, I/O failpoints, allocator failpoints, subprocess power-loss, ReadOnly/Immutable zero-write verification, and cross-language contracts. A test must not copy the production implementation as its reference — that only proves two identical implementations agree.

#### A14. Product boundaries for QuIVer and ANN algorithms

QuIVer is the core ANN foundation and a primary product differentiator of TriviumDB. The rules distinguish two kinds of change:

- **Adding a new ANN algorithm: allowed.** Wiring it in as an optional backend or experimental path must not touch QuIVer semantics, and must preserve determinism, transaction safety, the late-materialization invariant, and complete tests;
- **Directly modifying the QuIVer core: requires cross-dataset Pareto-frontier evidence.** Changes to BQ encoding, graph navigation, reranking, construction, or persistence semantics must compare QPS, latency, and memory across multiple datasets and multiple recall operating points, demonstrating that the new implementation is no worse overall and clearly better on key metrics before merging. A local win at a single operating point or on a single dataset is not sufficient to replace a published, widely benchmarked baseline.

Neither kind of change may break recall semantics, stable ordering, transaction safety, or determinism; performance data must be reproducible.

### B. Design and maintenance principles

#### B1. Minimal necessary change

A bug fix does not refactor unrelated modules; no new abstraction for a single call site; no speculative compatibility layers, unused re-exports, or placeholder helpers; delete code that is confirmed unused; every new file must have a clear, distinct, long-term responsibility.

#### B2. Single source of truth

Never keep two authoritative parsed Payloads, two interpretations of a format version, filtering implemented separately in Core and bindings, defaults that differ between docs and source, or Planner/executor disagreeing about one operator's semantics.

#### B3. Complexity proportional to evidence

Maintainability and correctness come before unmeasured local performance. Do not prove mmap memory gains with RSS alone; distinguish heap, mapped file size, resident pages, page cache, and temporary workspaces. A real, measured performance problem may justify extra complexity, kept proportional to the evidence and contained within the right boundary.

#### B4. Prefer general solutions to special cases

Check whether several requirements share the same semantics and extend one shared mechanism first; preserve genuine differences clearly. Do not build a universal abstraction before a shared need exists, and do not add private parallel implementations for a single entry point.

#### B5. Documentation must describe real capability boundaries

Do not overstate single-file, zero-copy, zero-memory, or auto-recovery capabilities; clearly distinguish Rom vs Mmap file sets and nightly Server vs official Embedded capabilities; formats, defaults, and error codes must match the source. The README may highlight strengths but must not hide conditions that affect data safety or migration.

### C. PR description requirements by change type

When modifying any of the following areas, the PR description must list four items — **invariants, compatibility, failure behavior, and test evidence** — or it will be asked to add them:

- WAL, transactions, and recovery;
- `.tdb/.vec/.pld/.flush_ok` disk formats;
- mmap lifetime and generations;
- Query planner, budgets, and executors;
- QuIVer / BQ / ANN paths;
- Rust / Python / Node public APIs.

### Commit Conventions

- Commit messages use detailed English and explain the "why", not just the "what";
- Do not commit secrets, temporary files, or unrelated churn;
- Keep one PR focused on one thing; PRs mixing features, refactoring, and formatting are hard to review and may be asked to be split.
