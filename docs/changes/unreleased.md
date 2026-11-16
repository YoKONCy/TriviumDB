# 未发布变更

> 后续开发以 `0.9.1` 为基线，已交付能力见 [0.9.1 中英文说明](0.9.1.md)。

---

## 中文说明

### Node 包的命名导出与 Prepared 类型

- 修复 CommonJS 加载器的动态导出使 ESM 命名导入在加载时抛出 `SyntaxError` 的问题。显式提供数据库、Prepared、运行时、会话、请求上下文及快照函数的公共导出。
- 原生 Prepared 类统一为 `PreparedTql`，与 TypeScript 声明一致。JavaScript 的 `JsPreparedTql` 作为同一构造器的兼容名称，Rust 保留原有 `JsPreparedTql` 导出路径，返回句柄可通过两种 JavaScript 名称执行 `instanceof`。
- 实际 npm tarball 消费验收增加 `.cts` 与 `.mts`，按 NodeNext 编译后分别执行 CommonJS／ESM，核对公开值身份、Prepared 参数及完整查询结果。

---

## English

Subsequent development builds on `0.9.1`. See the [bilingual release notes](0.9.1.md) for delivered capabilities.

### Node named exports and Prepared types

- Fixed ESM named imports throwing `SyntaxError` when loading the dynamic CommonJS entry point. The loader explicitly exposes public database, Prepared, runtime, session, request-context, and snapshot-function exports.
- The native Prepared class uses `PreparedTql`, matching the TypeScript declaration. JavaScript retains `JsPreparedTql` as an alias of the same constructor, and Rust retains its existing `JsPreparedTql` export path. Returned handles support `instanceof` with either JavaScript name.
- Installed npm tarball validation includes `.cts` and `.mts` consumers compiled with NodeNext and executed as CommonJS and ESM. Checks cover public value identity, Prepared parameters, and complete query results.
