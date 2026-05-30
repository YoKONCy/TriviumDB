#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
跑 VIBE 数据集在 QuIVer 索引下的真实 Recall
只跑 1d 子实验（ef_search recall 曲线，默认参数 m=32, ef_c=128, α=1.2）
"""
import os, sys, subprocess, time
from pathlib import Path

if sys.platform == "win32":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

ROOT = Path(__file__).resolve().parent.parent

DATASETS = [
    "arxiv-nomic",
    "ccnews-nomic",
    "coco-nomic",
    "codesearch-jina",
    "gooaq-roberta",
    "landmark-nomic",
    "landmark-dino",
]

LOG_DIR = ROOT / "research" / "bench_logs"
LOG_DIR.mkdir(parents=True, exist_ok=True)

for name in DATASETS:
    log_path = LOG_DIR / f"{name}.log"
    if log_path.exists() and "全部实验完成" in log_path.read_text(encoding="utf-8", errors="replace"):
        print(f"  [跳过] {name}: 已完成")
        continue

    print(f"\n{'='*60}")
    print(f"  跑 {name}...")
    print(f"{'='*60}")

    env = os.environ.copy()
    env["TRIVIUM_ANN_NAME"] = name
    env["TRIVIUM_SENSITIVITY_MODE"] = "params"
    env["TRIVIUM_SENSITIVITY_START"] = "1d"
    env["TRIVIUM_SENSITIVITY_END"] = "1d"

    t0 = time.time()
    with open(log_path, "w", encoding="utf-8") as f:
        proc = subprocess.Popen(
            ["cargo", "bench", "--bench", "bench_sensitivity"],
            cwd=str(ROOT), env=env,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            text=True, encoding="utf-8", errors="replace",
        )
        for line in proc.stdout:
            sys.stdout.write(line)
            f.write(line)
        proc.wait()

    elapsed = time.time() - t0
    status = "✅" if proc.returncode == 0 else "❌"
    print(f"  {status} {name} 完成, 耗时 {elapsed:.0f}s")

print("\n全部跑完!")
