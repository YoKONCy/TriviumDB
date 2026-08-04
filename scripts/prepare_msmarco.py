#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""确定性准备 MSMARCO Cohere-v3 的 1M/5M QuIVer 评测数据。"""

import argparse
import hashlib
import itertools
import json
import os
import sys
from pathlib import Path

import numpy as np


ROOT = Path(__file__).resolve().parent.parent
REPO_ID = "Cohere/msmarco-v2.1-embed-english-v3"


def parse_args():
    parser = argparse.ArgumentParser(description="准备 MSMARCO-1M/5M 与精确 cosine Ground truth")
    parser.add_argument("--output-dir", type=Path, default=ROOT)
    parser.add_argument("--repo-id", default=REPO_ID)
    parser.add_argument("--config", default="passages")
    parser.add_argument("--split", default="train")
    parser.add_argument("--revision")
    parser.add_argument("--embedding-column")
    parser.add_argument("--sizes", type=int, nargs="+", default=[1_000_000, 5_000_000])
    parser.add_argument("--queries", type=int, default=10_000)
    parser.add_argument("--dim", type=int, default=1024)
    parser.add_argument("--top-k", type=int, default=10)
    parser.add_argument("--query-batch", type=int, default=16)
    parser.add_argument("--base-batch", type=int, default=50_000)
    parser.add_argument("--force", action="store_true")
    return parser.parse_args()


def choose_embedding(row, explicit, dim):
    if explicit:
        if explicit not in row:
            raise ValueError(f"指定的 embedding 字段不存在: {explicit}; 可用字段: {', '.join(row)}")
        return explicit
    candidates = []
    for key, value in row.items():
        if isinstance(value, (list, tuple, np.ndarray)) and len(value) == dim:
            candidates.append(key)
    if len(candidates) != 1:
        raise ValueError(f"无法唯一识别 {dim} 维 embedding 字段，候选: {candidates}")
    return candidates[0]


def normalized_vector(value, dim):
    vector = np.asarray(value, dtype=np.float32)
    if vector.shape != (dim,) or not np.isfinite(vector).all():
        raise ValueError(f"embedding 必须是有限的 {dim} 维向量，实际 shape={vector.shape}")
    norm = np.linalg.norm(vector)
    if norm <= 1e-12:
        raise ValueError("embedding 不能是零向量")
    return vector / norm


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(8 * 1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def exact_groundtruth(train_path, n_train, queries, dim, top_k, query_batch, base_batch):
    train = np.memmap(train_path, dtype="<f4", mode="r", shape=(n_train, dim))
    result = np.empty((len(queries), top_k), dtype="<i4")
    for query_start in range(0, len(queries), query_batch):
        query = queries[query_start:query_start + query_batch]
        best_scores = np.full((len(query), top_k), -np.inf, dtype=np.float32)
        best_ids = np.full((len(query), top_k), -1, dtype=np.int32)
        for base_start in range(0, n_train, base_batch):
            base = train[base_start:min(base_start + base_batch, n_train)]
            scores = query @ base.T
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
        result[query_start:query_start + len(query)] = np.take_along_axis(best_ids, order, axis=1)
        print(f"  GT 查询进度: {min(query_start + len(query), len(queries))}/{len(queries)}")
    return result


def write_manifest(output_dir, prefix, args, embedding_column, n_train, paths):
    manifest = {
        "schema_version": 1,
        "name": prefix.replace("msmarco", "msmarco-"),
        "prefix": prefix,
        "source": {
            "type": "huggingface",
            "id": args.repo_id,
            "config": args.config,
            "split": args.split,
            "embedding_column": embedding_column,
            "revision": args.revision,
        },
        "metric": "cosine",
        "normalized": True,
        "dim": args.dim,
        "n_train": n_train,
        "n_test": args.queries,
        "groundtruth_k": args.top_k,
        "train_offset": 0,
        "query_offset": max(args.sizes),
        "dtype": {"train": "float32", "test": "float32", "groundtruth": "int32"},
        "byte_order": "little",
        "files": {key: {"name": path.name, "sha256": sha256(path)} for key, path in paths.items()},
    }
    path = output_dir / f"{prefix}_manifest.json"
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def main():
    args = parse_args()
    if min(args.sizes) < args.top_k or args.queries < 1 or args.dim < 1:
        raise ValueError("sizes、queries 和 dim 参数无效")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    prefixes = {size: f"msmarco{size // 1_000_000}m" for size in sorted(set(args.sizes))}
    all_paths = {
        size: {
            "train": args.output_dir / f"{prefixes[size]}_train.f32",
            "test": args.output_dir / f"{prefixes[size]}_test.f32",
            "groundtruth": args.output_dir / f"{prefixes[size]}_groundtruth.i32",
        }
        for size in prefixes
    }
    if not args.force and all(
        all(path.exists() for path in (*paths.values(), args.output_dir / f"{prefixes[size]}_manifest.json"))
        for size, paths in all_paths.items()
    ):
        print("所有 MSMARCO 产物已存在；使用 --force 重新生成")
        return
    try:
        from datasets import load_dataset
    except ImportError as error:
        raise RuntimeError("需要安装 datasets: python -m pip install datasets") from error
    dataset = load_dataset(
        args.repo_id,
        args.config or None,
        split=args.split,
        streaming=True,
        revision=args.revision,
    )
    iterator = iter(dataset)
    first = next(iterator)
    embedding_column = choose_embedding(first, args.embedding_column, args.dim)
    max_size = max(args.sizes)
    temporary_train = args.output_dir / ".msmarco_train.tmp"
    query_path = args.output_dir / ".msmarco_queries.tmp"
    rows_written = 0
    with temporary_train.open("wb") as train_handle, query_path.open("wb") as query_handle:
        for index, row in enumerate(itertools.chain([first], iterator)):
            vector = normalized_vector(row[embedding_column], args.dim).astype("<f4")
            if index < max_size:
                vector.tofile(train_handle)
            elif index < max_size + args.queries:
                vector.tofile(query_handle)
            else:
                break
            rows_written = index + 1
            if (index + 1) % 100_000 == 0:
                print(f"  已读取 {index + 1}/{max_size + args.queries} 条向量")
    if rows_written < max_size + args.queries:
        raise ValueError(f"数据源不足 {max_size + args.queries} 条向量，实际读取 {rows_written} 条")
    queries = np.memmap(query_path, dtype="<f4", mode="r", shape=(args.queries, args.dim))
    for size, prefix in prefixes.items():
        paths = all_paths[size]
        with temporary_train.open("rb") as source, paths["train"].open("wb") as target:
            remaining = size * args.dim * 4
            while remaining:
                block = source.read(min(8 * 1024 * 1024, remaining))
                if not block:
                    raise ValueError("临时训练文件提前结束")
                target.write(block)
                remaining -= len(block)
        np.asarray(queries).astype("<f4").tofile(paths["test"])
        groundtruth = exact_groundtruth(
            paths["train"], size, queries, args.dim, args.top_k, args.query_batch, args.base_batch
        )
        groundtruth.tofile(paths["groundtruth"])
        write_manifest(args.output_dir, prefix, args, embedding_column, size, paths)
        print(f"  [完成] {prefix}: {size}×{args.dim}, queries={args.queries}, K={args.top_k}")
    del queries
    temporary_train.unlink(missing_ok=True)
    query_path.unlink(missing_ok=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"[错误] {error}", file=sys.stderr)
        sys.exit(1)
