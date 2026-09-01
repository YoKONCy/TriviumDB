"""严格统计 TriviumDB 业务源码与有效测试资产。

统计排除空行、纯注释、构建产物、benchmark、文档和二进制 corpus；src 内
``#[cfg(test)] mod`` 之后的内联测试计入测试资产而非业务源码。
"""

from __future__ import annotations

import argparse
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TEST_SUFFIXES = {".rs", ".py", ".js"}


def is_effective(line: str) -> bool:
    value = line.strip()
    return bool(value) and not value.startswith(("//", "//!", "///", "# "))


def count_file(path: Path) -> int:
    return sum(
        is_effective(line)
        for line in path.read_text(encoding="utf-8", errors="ignore").splitlines()
    )


def split_rust_source(path: Path) -> tuple[int, int]:
    """按顶层 ``#[cfg(test)] mod`` 边界近似拆分生产代码与内联测试。"""
    production = 0
    tests = 0
    pending_test_cfg = False
    test_depth: int | None = None
    depth = 0
    for line in path.read_text(encoding="utf-8", errors="ignore").splitlines():
        stripped = line.strip()
        if stripped.startswith("#[cfg(test)]"):
            pending_test_cfg = True
        opens = line.count("{")
        closes = line.count("}")
        if pending_test_cfg and "mod " in stripped and opens:
            test_depth = depth + opens - closes
            pending_test_cfg = False
        if is_effective(line):
            if test_depth is None:
                production += 1
            else:
                tests += 1
        depth += opens - closes
        if test_depth is not None and depth < test_depth:
            test_depth = None
    return production, tests


def measure() -> tuple[int, int]:
    production = 0
    inline_tests = 0
    for path in (ROOT / "src").rglob("*.rs"):
        source, tests = split_rust_source(path)
        production += source
        inline_tests += tests
    external_tests = sum(
        count_file(path)
        for path in (ROOT / "tests").rglob("*")
        if path.is_file() and path.suffix in TEST_SUFFIXES
    )
    return production, inline_tests + external_tests


def main() -> int:
    parser = argparse.ArgumentParser(description="统计严格有效测试资产比率")
    parser.add_argument("--target", type=float, default=5.0)
    parser.add_argument("--enforce", action="store_true")
    arguments = parser.parse_args()
    production, tests = measure()
    ratio = tests / production if production else 0.0
    required = max(0, int(production * arguments.target) - tests)
    print(f"业务有效行: {production}")
    print(f"测试有效行: {tests}")
    print(f"当前比率: {ratio:.3f}:1")
    print(f"目标比率: {arguments.target:.3f}:1")
    print(f"仍需有效测试行: {required}")
    if arguments.enforce and ratio < arguments.target:
        print("测试资产比率尚未达到目标，门禁拒绝通过")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
