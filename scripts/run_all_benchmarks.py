#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
QuIVer 多数据集批量 benchmark 挂机脚本

自动遍历所有数据集，运行 bench_sensitivity，输出保存到日志文件。
跳过已经跑过的数据集（检测日志文件是否存在且包含"全部实验完成"标记）。

用法：
  python scripts/run_all_benchmarks.py              # 跑 1d+1e+实验2（推荐，每数据集约5-15min）
  python scripts/run_all_benchmarks.py --full        # 跑全部子实验（1a~1f+实验2，非常耗时）
  python scripts/run_all_benchmarks.py --force       # 强制重跑所有（不跳过已完成的）
  python scripts/run_all_benchmarks.py --only minilm-384 sift-128  # 只跑指定的
  python scripts/run_all_benchmarks.py --mode params  # 只跑参数敏感性（不跑线程扩展）
  python scripts/run_all_benchmarks.py --start 1e    # 自定义起始子实验

日志输出目录: research/bench_logs/
"""

import os
import sys
import subprocess
import time
from pathlib import Path
from datetime import datetime

# Windows 终端编码
if sys.platform == "win32":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")

ROOT = Path(__file__).resolve().parent.parent
LOG_DIR = ROOT / "research" / "bench_logs"

# 所有数据集（按预估耗时从短到长排序）
ALL_DATASETS = [
    # (环境变量名,   描述,                    维度)
    ("glove-100",    "GloVe-1.18M",           100),
    ("sift-128",     "SIFT-1M",               128),
    ("minilm-384",   "MiniLM-1M",             384),
    ("wolt-clip-512","Wolt CLIP-1M",           512),
    ("random-1m",    "Synthetic-LR-1M",        768),
    ("sphere-1m",    "Random-Sphere-1M",       768),
    ("bge-m3-1024",  "BGE-M3-1M",             1024),
    ("gist-960",     "GIST-1M",               960),
    ("dbpedia-1536", "DBpedia-OpenAI-1M",     1536),
    ("dbpedia-3072", "DBpedia-OpenAI-3072-1M",3072),
]

# 跳过已跑完的（cohere-1m）
SKIP_DEFAULT = {"cohere-1m"}

COMPLETE_MARKER = "全部实验完成"


def is_completed(log_path: Path) -> bool:
    """检查日志文件是否包含完成标记"""
    if not log_path.exists():
        return False
    try:
        text = log_path.read_text(encoding="utf-8", errors="replace")
        return COMPLETE_MARKER in text
    except Exception:
        return False


def run_one(name: str, desc: str, dim: int, mode: str, start: str, log_dir: Path) -> bool:
    """运行单个数据集的 bench_sensitivity，返回是否成功"""
    log_path = log_dir / f"{name}.log"

    print(f"\n{'='*70}")
    print(f"  数据集: {desc} ({name}, {dim}-d)")
    print(f"  模式: {mode}, 起始: {start}")
    print(f"  日志: {log_path}")
    print(f"  开始时间: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    print(f"{'='*70}")

    env = os.environ.copy()
    env["TRIVIUM_ANN_NAME"] = name
    env["TRIVIUM_SENSITIVITY_MODE"] = mode
    if start:
        env["TRIVIUM_SENSITIVITY_START"] = start

    cmd = ["cargo", "bench", "--bench", "bench_sensitivity"]

    with open(log_path, "w", encoding="utf-8") as log_file:
        # 写入日志头
        log_file.write(f"# QuIVer bench_sensitivity 日志\n")
        log_file.write(f"# 数据集: {name} ({desc}, {dim}-d)\n")
        log_file.write(f"# 模式: {mode}, 起始: {start}\n")
        log_file.write(f"# 开始时间: {datetime.now().isoformat()}\n")
        log_file.write(f"# 命令: {' '.join(cmd)}\n")
        log_file.write(f"{'='*70}\n\n")
        log_file.flush()

        t0 = time.time()
        try:
            proc = subprocess.Popen(
                cmd,
                cwd=str(ROOT),
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
            )

            # 实时输出到终端和日志
            for line in proc.stdout:
                sys.stdout.write(line)
                sys.stdout.flush()
                log_file.write(line)
                log_file.flush()

            proc.wait()
            elapsed = time.time() - t0

            log_file.write(f"\n{'='*70}\n")
            log_file.write(f"# 退出码: {proc.returncode}\n")
            log_file.write(f"# 总耗时: {elapsed:.1f}s ({elapsed/60:.1f}min)\n")
            log_file.write(f"# 结束时间: {datetime.now().isoformat()}\n")

            if proc.returncode == 0:
                print(f"\n  ✅ {name} 完成! 耗时 {elapsed:.0f}s ({elapsed/60:.1f}min)")
                return True
            else:
                print(f"\n  ❌ {name} 失败! 退出码 {proc.returncode}, 耗时 {elapsed:.0f}s")
                return False

        except Exception as e:
            elapsed = time.time() - t0
            print(f"\n  ❌ {name} 异常: {e}")
            log_file.write(f"\n# 异常: {e}\n")
            return False


def main():
    import argparse
    parser = argparse.ArgumentParser(description="QuIVer 多数据集批量 benchmark")
    parser.add_argument("--full", action="store_true", help="跑全部子实验（1a~1f+实验2），默认只跑 1d+1e+实验2")
    parser.add_argument("--force", action="store_true", help="强制重跑所有（不跳过已完成的）")
    parser.add_argument("--only", nargs="+", help="只跑指定的数据集名称")
    parser.add_argument("--mode", default="all", help="bench_sensitivity 模式: all/params/threads")
    parser.add_argument("--start", default="", help="自定义起始子实验: 1a/1b/1c/1d/1e/1f")
    args = parser.parse_args()

    # 默认从 1d 开始（跳过耗时的 1a/1b/1c 参数扫描）
    # bench_sensitivity.rs 默认 END=1e（跳过 1f m×α 交叉）
    if not args.start and not args.full:
        args.start = "1d"

    # --full 模式：从 1a 跑到 1f
    if args.full:
        if not args.start:
            args.start = "1a"
        os.environ["TRIVIUM_SENSITIVITY_END"] = "1f"

    LOG_DIR.mkdir(parents=True, exist_ok=True)

    # 确定要跑的数据集
    if args.only:
        datasets = [(n, d, dim) for n, d, dim in ALL_DATASETS if n in args.only]
        if not datasets:
            print(f"[错误] 没有匹配的数据集: {args.only}")
            print(f"  可选: {[n for n, _, _ in ALL_DATASETS]}")
            sys.exit(1)
    else:
        datasets = [(n, d, dim) for n, d, dim in ALL_DATASETS if n not in SKIP_DEFAULT]

    # 检查跳过
    to_run = []
    for name, desc, dim in datasets:
        log_path = LOG_DIR / f"{name}.log"
        if not args.force and is_completed(log_path):
            print(f"  [跳过] {name}: 已有完成的日志 ({log_path.name})")
        else:
            to_run.append((name, desc, dim))

    if not to_run:
        print("\n所有数据集都已完成！使用 --force 重跑。")
        return

    print(f"\n{'='*70}")
    print(f"  QuIVer 批量 benchmark")
    print(f"  待跑数据集: {len(to_run)} 个")
    for name, desc, dim in to_run:
        print(f"    - {name}: {desc} ({dim}-d)")
    print(f"  模式: {args.mode}")
    if args.start:
        print(f"  起始子实验: {args.start}")
    print(f"  日志目录: {LOG_DIR}")
    print(f"  开始时间: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    print(f"{'='*70}")

    total_t0 = time.time()
    results = []

    for i, (name, desc, dim) in enumerate(to_run):
        print(f"\n  [{i+1}/{len(to_run)}] 开始 {name}...")
        ok = run_one(name, desc, dim, args.mode, args.start, LOG_DIR)
        results.append((name, desc, ok))

    total_elapsed = time.time() - total_t0

    # 汇总
    print(f"\n\n{'='*70}")
    print(f"  批量 benchmark 完成!")
    print(f"  总耗时: {total_elapsed:.0f}s ({total_elapsed/60:.1f}min, {total_elapsed/3600:.1f}h)")
    print(f"{'='*70}")
    print(f"\n  {'数据集':<20} {'状态':<10}")
    print(f"  {'-'*30}")
    for name, desc, ok in results:
        status = "✅ 成功" if ok else "❌ 失败"
        print(f"  {name:<20} {status}")

    failed = [n for n, _, ok in results if not ok]
    if failed:
        print(f"\n  ⚠️ 失败的数据集: {failed}")
        print(f"  查看日志: {LOG_DIR}")
        sys.exit(1)
    else:
        print(f"\n  🎉 全部成功!")


if __name__ == "__main__":
    main()
