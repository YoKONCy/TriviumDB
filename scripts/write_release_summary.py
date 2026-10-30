#!/usr/bin/env python3
"""根据真实门禁与上传作业结果生成双语发布摘要。"""

from __future__ import annotations

import json
import os
from pathlib import Path


def registry_status(gate: dict, job: dict, flag: str) -> str:
    result = job.get("result")
    if result == "failure":
        return "上传失败 (Upload failed)"
    if result == "cancelled":
        return "上传已取消 (Upload cancelled)"
    if gate.get("result") != "success":
        return "未执行：版本门禁未成功 (Not run: version gate did not succeed)"
    should_publish = gate.get("outputs", {}).get(flag)
    if result == "success" and should_publish == "true":
        return "上传作业成功 (Upload job succeeded)"
    if result == "skipped" and should_publish == "false":
        return "版本已存在，跳过上传 (Version already exists; upload skipped)"
    if result == "skipped" and should_publish == "true":
        return "未执行：上游门禁或构建未成功 (Not run: upstream gate or build did not succeed)"
    return "发布状态不一致，请检查作业 (Inconsistent release state; inspect jobs)"


def render(needs: dict) -> str:
    gate = needs.get("version-gate", {})
    version = gate.get("outputs", {}).get("version") or "未知 (unknown)"
    python_version = version.replace("-beta.", "b")
    return "\n".join([
        f"## 发布摘要 (Release Summary): {version}",
        "",
        "- **crates.io**: 由维护者在本地手动发布 (Requires manual publication by the maintainer)",
        f"- **PyPI**: triviumdb=={python_version} — "
        + registry_status(gate, needs.get("release", {}), "pypi_should_publish"),
        f"- **NPM**: triviumdb@{version} — "
        + registry_status(gate, needs.get("release-npm", {}), "npm_should_publish"),
        "",
    ])


if __name__ == "__main__":
    with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as output:
        output.write(render(json.loads(os.environ["RELEASE_NEEDS"])))
