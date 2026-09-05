## 变更说明

<!-- 请用自己的话说明问题、实际修改和设计思路。不要只粘贴 AI 生成的摘要。 -->

### 解决的问题


### 实际修改


### 设计思路


## 高风险领域检查

<!-- 若涉及 WAL/事务/恢复、磁盘格式、mmap generation、Planner/预算、QuIVer/BQ/ANN 或三端公共 API，请填写以下四项；不涉及可写“不适用”。 -->

- **不变量：**
- **兼容性：**
- **故障行为：**
- **测试证据：**

## 提交前确认

- [ ] PR 的目标分支是 `dev`。
- [ ] 我已阅读 `CONTRIBUTING.md`。
- [ ] 我已亲自阅读并理解最终变更，并在上方至少用自己的一句话作出说明。
- [ ] 变更保持聚焦，没有包含无关重构或生成物。
- [ ] 已补充或更新必要测试。
- [ ] 已运行与本次变更相关的格式、Clippy、测试、存根及跨端检查。
- [ ] 若修改公共 API，Rust、Python、Node、`.pyi`、`.d.ts` 与契约测试已同步。

---

## Change Description

<!-- Explain the problem, actual changes, and reasoning in your own words. Do not submit only an AI-generated summary. -->

### Problem solved


### Actual changes


### Reasoning


## High-risk area checks

<!-- If this PR touches WAL/transactions/recovery, disk formats, mmap generations, Planner/budgets, QuIVer/BQ/ANN, or public language APIs, complete all four items. Otherwise write “Not applicable.” -->

- **Invariants:**
- **Compatibility:**
- **Failure behavior:**
- **Test evidence:**

## Pre-submission checklist

- [ ] This PR targets `dev`.
- [ ] I have read `CONTRIBUTING.md`.
- [ ] I personally reviewed and understand the final change, and wrote at least one sentence above in my own words.
- [ ] The change is focused and contains no unrelated refactoring or generated artifacts.
- [ ] Necessary tests were added or updated.
- [ ] Relevant formatting, Clippy, tests, stubs, and cross-language checks were run.
- [ ] If the public API changed, Rust, Python, Node, `.pyi`, `.d.ts`, and contract tests were updated together.
