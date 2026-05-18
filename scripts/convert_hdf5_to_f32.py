#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
将 VIBE HDF5 数据集转换为 TriviumDB 根目录的 f32 分离格式

输入: research/infoNCE/dataset/*.hdf5
输出: {ROOT}/{prefix}_train.f32
      {ROOT}/{prefix}_test.f32
      {ROOT}/{prefix}_groundtruth.i32

转换完成后，utils.py 中的数据集注册表将自动切换为 f32 格式加载。
"""

import sys
import numpy as np
from pathlib import Path

if sys.platform == "win32":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

try:
    import h5py
except ImportError:
    print("需要安装 h5py: pip install h5py")
    sys.exit(1)

ROOT = Path(__file__).resolve().parent.parent  # TriviumDB 根目录
HDF5_DIR = ROOT / "research" / "infoNCE" / "dataset"

# HDF5 文件名 → 输出前缀 的映射
CONVERSIONS = {
    "arxiv-nomic-768-normalized.hdf5":          "arxiv_nomic",
    "ccnews-nomic-768-normalized.hdf5":         "ccnews_nomic",
    "coco-nomic-768-normalized.hdf5":           "coco_nomic",
    "codesearchnet-jina-768-cosine.hdf5":       "codesearch_jina",
    "gooaq-distilroberta-768-normalized.hdf5":  "gooaq_roberta",
    "landmark-nomic-768-normalized.hdf5":       "landmark_nomic",
    "landmark-dino-768-cosine.hdf5":            "landmark_dino",
}


def convert_one(hdf5_name: str, prefix: str, max_train: int = 1_000_000):
    """转换单个 HDF5 文件"""
    src = HDF5_DIR / hdf5_name
    if not src.exists():
        print(f"  [跳过] {hdf5_name} 不存在")
        return False

    train_path = ROOT / f"{prefix}_train.f32"
    test_path = ROOT / f"{prefix}_test.f32"
    gt_path = ROOT / f"{prefix}_groundtruth.i32"

    # 检查是否已转换
    if train_path.exists():
        size_mb = train_path.stat().st_size / (1024 * 1024)
        print(f"  [已存在] {train_path.name} ({size_mb:.0f} MB)")
        return True

    print(f"  转换: {hdf5_name} → {prefix}_*.f32")

    with h5py.File(str(src), 'r') as f:
        # 读取训练数据
        n_total = f['train'].shape[0]
        dim = f['train'].shape[1]

        if n_total > max_train:
            # 取前 max_train 条（连续读取，快）
            train_data = f['train'][:max_train].astype(np.float32)
            print(f"    train: ({n_total}, {dim}) → 截取前 {max_train} 条")
        else:
            train_data = f['train'][:].astype(np.float32)
            print(f"    train: {train_data.shape}")

        # 读取测试数据
        test_data = f['test'][:].astype(np.float32)
        print(f"    test:  {test_data.shape}")

        # 读取 ground truth (top-100 近邻)
        neighbors = f['neighbors'][:].astype(np.int32)
        # 只保留 top-10
        gt_data = neighbors[:, :10]
        print(f"    groundtruth: {gt_data.shape}")

    # 写出 raw binary
    train_data.tofile(str(train_path))
    print(f"    → {train_path.name} ({train_path.stat().st_size / (1024*1024):.0f} MB)")

    test_data.tofile(str(test_path))
    print(f"    → {test_path.name}")

    gt_data.tofile(str(gt_path))
    print(f"    → {gt_path.name}")

    return True


def main():
    print("=" * 60)
    print("  VIBE HDF5 → f32 分离格式转换")
    print("=" * 60)

    success = 0
    for hdf5_name, prefix in CONVERSIONS.items():
        print(f"\n{'─'*50}")
        result = convert_one(hdf5_name, prefix)
        if result:
            success += 1

    print(f"\n{'='*60}")
    print(f"  转换完成: {success}/{len(CONVERSIONS)} 个数据集")
    print(f"{'='*60}")

    # 提示更新 utils.py
    print("\n  ⚠️  记得将 utils.py 中对应数据集的格式从 'hdf5' 改为 'f32'")
    print("  或者直接使用新的前缀名加载。")


if __name__ == "__main__":
    main()
