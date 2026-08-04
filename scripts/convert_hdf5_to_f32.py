#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""将 VIBE HDF5 数据集转换为 QuIVer 使用的裸二进制格式。"""

import argparse
import hashlib
import json
import os
import sys
from pathlib import Path

import numpy as np

if sys.platform == "win32":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

try:
    import h5py
except ImportError:
    print("需要安装 h5py: pip install h5py")
    sys.exit(1)

ROOT = Path(__file__).resolve().parent.parent

CONVERSIONS = {
    "arxiv-nomic-768-normalized.hdf5":          "arxiv_nomic",
    "ccnews-nomic-768-normalized.hdf5":         "ccnews_nomic",
    "coco-nomic-768-normalized.hdf5":           "coco_nomic",
    "codesearchnet-jina-768-cosine.hdf5":       "codesearch_jina",
    "gooaq-distilroberta-768-normalized.hdf5":  "gooaq_roberta",
    "landmark-nomic-768-normalized.hdf5":       "landmark_nomic",
    "landmark-dino-768-cosine.hdf5":            "landmark_dino",
}


def parse_args():
    parser = argparse.ArgumentParser(description="转换并校验 VIBE HDF5 数据集")
    parser.add_argument("--input-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, default=ROOT)
    parser.add_argument("--datasets", nargs="+", choices=sorted(CONVERSIONS), metavar="HDF5")
    parser.add_argument("--max-train", type=int, default=1_000_000)
    parser.add_argument("--force", action="store_true")
    return parser.parse_args()


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(8 * 1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def exact_groundtruth(train, test, top_k=10, query_batch=32, base_batch=100_000):
    result = np.empty((len(test), top_k), dtype=np.int32)
    for query_start in range(0, len(test), query_batch):
        queries = test[query_start:query_start + query_batch]
        best_scores = np.full((len(queries), top_k), -np.inf, dtype=np.float32)
        best_ids = np.full((len(queries), top_k), -1, dtype=np.int32)
        for base_start in range(0, len(train), base_batch):
            base = train[base_start:base_start + base_batch]
            scores = queries @ base.T
            local_k = min(top_k, len(base))
            local = np.argpartition(-scores, local_k - 1, axis=1)[:, :local_k]
            local_scores = np.take_along_axis(scores, local, axis=1)
            local_ids = local.astype(np.int32) + base_start
            merged_scores = np.concatenate((best_scores, local_scores), axis=1)
            merged_ids = np.concatenate((best_ids, local_ids), axis=1)
            keep = np.argpartition(-merged_scores, top_k - 1, axis=1)[:, :top_k]
            best_scores = np.take_along_axis(merged_scores, keep, axis=1)
            best_ids = np.take_along_axis(merged_ids, keep, axis=1)
        order = np.argsort(-best_scores, axis=1)
        result[query_start:query_start + len(queries)] = np.take_along_axis(best_ids, order, axis=1)
    return result


def validate_arrays(train, test, groundtruth):
    if train.ndim != 2 or test.ndim != 2 or groundtruth.ndim != 2:
        raise ValueError("train、test 和 neighbors 必须是二维数组")
    if train.shape[1] != test.shape[1]:
        raise ValueError("train 与 test 的向量维度不一致")
    if len(test) == 0 or len(train) < 10:
        raise ValueError("数据集必须至少包含一个查询和十个训练向量")
    if groundtruth.shape != (len(test), 10):
        raise ValueError("Ground truth 必须是 Q×10")
    if not np.isfinite(train).all() or not np.isfinite(test).all():
        raise ValueError("向量包含 NaN 或 Inf")
    if np.any(groundtruth < 0) or np.any(groundtruth >= len(train)):
        raise ValueError("Ground truth 包含越界 ID")


def atomic_write(path, array):
    temporary = path.with_name(path.name + ".tmp")
    array.tofile(temporary)
    os.replace(temporary, path)


def convert_one(src, prefix, output_dir, max_train, force):
    paths = {
        "train": output_dir / f"{prefix}_train.f32",
        "test": output_dir / f"{prefix}_test.f32",
        "groundtruth": output_dir / f"{prefix}_groundtruth.i32",
    }
    manifest_path = output_dir / f"{prefix}_manifest.json"
    if not force and all(path.exists() for path in (*paths.values(), manifest_path)):
        print(f"  [已存在] {prefix} 的完整产物")
        return
    with h5py.File(src, "r") as source:
        missing = {"train", "test", "neighbors"} - set(source.keys())
        if missing:
            raise ValueError(f"{src.name} 缺少字段: {', '.join(sorted(missing))}")
        source_train_count = source["train"].shape[0]
        train_count = min(source_train_count, max_train)
        train = source["train"][:train_count].astype("<f4")
        test = source["test"][:].astype("<f4")
        neighbors = source["neighbors"][:, :10].astype("<i4")
    if source_train_count > train_count or np.any(neighbors >= train_count):
        print(f"  原始 Ground truth 不适用于前 {train_count} 条训练向量，正在重新计算")
        train_norm = train / np.maximum(np.linalg.norm(train, axis=1, keepdims=True), 1e-12)
        test_norm = test / np.maximum(np.linalg.norm(test, axis=1, keepdims=True), 1e-12)
        neighbors = exact_groundtruth(train_norm, test_norm)
    validate_arrays(train, test, neighbors)
    output_dir.mkdir(parents=True, exist_ok=True)
    atomic_write(paths["train"], train)
    atomic_write(paths["test"], test)
    atomic_write(paths["groundtruth"], neighbors)
    manifest = {
        "schema_version": 1,
        "name": prefix.replace("_", "-"),
        "prefix": prefix,
        "source": {"type": "vibe-hdf5", "file": src.name},
        "metric": "cosine",
        "normalized": bool(np.allclose(np.linalg.norm(train[:min(len(train), 1000)], axis=1), 1.0, atol=1e-3)),
        "dim": train.shape[1],
        "n_train": len(train),
        "n_test": len(test),
        "groundtruth_k": 10,
        "dtype": {"train": "float32", "test": "float32", "groundtruth": "int32"},
        "byte_order": "little",
        "files": {key: {"name": path.name, "sha256": sha256(path)} for key, path in paths.items()},
    }
    temporary_manifest = manifest_path.with_name(manifest_path.name + ".tmp")
    temporary_manifest.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary_manifest, manifest_path)
    print(f"  [完成] {prefix}: train={train.shape}, test={test.shape}, GT={neighbors.shape}")


def main():
    args = parse_args()
    selected = args.datasets or list(CONVERSIONS)
    failed = []
    for hdf5_name in selected:
        src = args.input_dir / hdf5_name
        if not src.exists():
            print(f"  [错误] 输入文件不存在: {src}")
            failed.append(hdf5_name)
            continue
        try:
            convert_one(src, CONVERSIONS[hdf5_name], args.output_dir, args.max_train, args.force)
        except Exception as error:
            print(f"  [错误] {hdf5_name}: {error}")
            failed.append(hdf5_name)
    if failed:
        print(f"转换失败: {', '.join(failed)}")
        sys.exit(1)
    print(f"全部转换完成: {len(selected)}/{len(selected)}")


if __name__ == "__main__":
    main()
