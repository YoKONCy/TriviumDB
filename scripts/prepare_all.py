#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
QuIVer 数据集统一准备脚本。

将所有数据集下载/生成并转换为标准 ANN benchmark 三文件格式：
  {prefix}_train.f32   — 训练向量 (N × D, float32 raw binary)
  {prefix}_test.f32    — 查询向量 (Q × D, float32 raw binary)
  {prefix}_groundtruth.i32 — Ground truth (Q × K, int32 raw binary)

支持三种数据源：
  1. ann-benchmarks HDF5 文件 (SIFT, GIST, GloVe)
  2. HuggingFace datasets 库 (MiniLM, DBpedia, BGE-M3, Wolt-CLIP, Cohere)
  3. 本地合成生成 (Random, Synthetic-LR)

用法：
  python scripts/prepare_all.py                    # 准备所有数据集
  python scripts/prepare_all.py cohere minilm      # 只准备指定数据集
  python scripts/prepare_all.py --list              # 列出所有可用数据集

依赖（按需安装）：
  pip install numpy tqdm                # 必需
  pip install h5py                      # HDF5 数据集需要
  pip install datasets                  # HuggingFace 数据集需要
"""

import sys
import os
import time
import numpy as np
from pathlib import Path

if sys.platform == "win32":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")

ROOT = Path(__file__).resolve().parent.parent
TOP_K = 10


# ══════════════════════════════════════════════════════════════════════
#  数据集注册表
# ══════════════════════════════════════════════════════════════════════

DATASETS = {
    # ── ann-benchmarks HDF5 ──────────────────────────────────────────
    "sift128": {
        "source": "hdf5",
        "hdf5": "sift-128-euclidean.hdf5",
        "prefix": "sift128",
        "dim": 128,
        "metric": "euclidean",
        "desc": "SIFT-1M (128-d, euclidean)",
    },
    "gist960": {
        "source": "hdf5",
        "hdf5": "gist-960-euclidean.hdf5",
        "prefix": "gist960",
        "dim": 960,
        "metric": "euclidean",
        "desc": "GIST-1M (960-d, euclidean)",
    },
    "glove100": {
        "source": "hdf5",
        "hdf5": "glove-100-angular.hdf5",
        "prefix": "glove100",
        "dim": 100,
        "metric": "angular",
        "desc": "GloVe-100 (100-d, angular/cosine)",
    },

    # ── HuggingFace datasets ─────────────────────────────────────────
    "minilm": {
        "source": "huggingface",
        "hf_id": "maloyan/wikipedia-22-12-en-embeddings-all-MiniLM-L6-v2",
        "embedding_col": "emb",
        "dim": 384,
        "n_train": 1_000_000,
        "n_test": 1_000,
        "prefix": "minilm",
        "streaming": True,
        "desc": "Wikipedia all-MiniLM-L6-v2 (384-d)",
    },
    "cohere": {
        "source": "hf_bin",
        "hf_id": "YoKONCy/Cohere-1M-wikipedia-768d",
        "prefix": "cohere",
        "desc": "Cohere Wikipedia embed-english-v2.0 (768-d)",
    },
    "bge_m3": {
        "source": "huggingface",
        "hf_id": "Qdrant/BGE-m3-1-million-ads",
        "embedding_col": "bgem3_dense_vecs",
        "dim": 1024,
        "n_train": 1_000_000,
        "n_test": 1_000,
        "prefix": "bge_m3",
        "streaming": False,
        "desc": "BGE-M3 1M ads (1024-d)",
    },
    "dbpedia1536": {
        "source": "huggingface",
        "hf_id": "Qdrant/dbpedia-entities-openai3-text-embedding-3-large-1536-1M",
        "embedding_col": "text-embedding-3-large-1536-embedding",
        "dim": 1536,
        "n_train": 990_000,
        "n_test": 10_000,
        "prefix": "dbpedia_openai",
        "streaming": False,
        "desc": "DBpedia OpenAI text-embedding-3-large (1536-d)",
    },
    "dbpedia3072": {
        "source": "huggingface",
        "hf_id": "Qdrant/dbpedia-entities-openai3-text-embedding-3-large-3072-1M",
        "embedding_col": "text-embedding-3-large-3072-embedding",
        "dim": 3072,
        "n_train": 990_000,
        "n_test": 10_000,
        "prefix": "dbpedia_openai_3072",
        "streaming": False,
        "desc": "DBpedia OpenAI text-embedding-3-large (3072-d)",
    },
    "wolt_clip": {
        "source": "huggingface",
        "hf_id": "Qdrant/wolt-food-clip-ViT-B-32-embeddings",
        "embedding_col": "vector",
        "dim": 512,
        "n_train": 1_000_000,
        "n_test": 1_000,
        "prefix": "wolt_clip",
        "streaming": True,
        "binary_vector": True,
        "desc": "Wolt Food CLIP ViT-B/32 (512-d, multimodal)",
    },

    # ── 合成数据集 ───────────────────────────────────────────────────
    "random": {
        "source": "synthetic",
        "generator": "random_uniform",
        "dim": 768,
        "n_train": 1_000_000,
        "n_test": 1_000,
        "prefix": "random",
        "desc": "Uniform random on S^{D-1} (768-d, baseline)",
    },
    "sphere": {
        "source": "synthetic",
        "generator": "low_rank",
        "dim": 768,
        "rank": 64,
        "n_train": 1_000_000,
        "n_test": 1_000,
        "prefix": "sphere",
        "desc": "Synthetic low-rank (768-d, rank-64, control)",
    },
}


# ══════════════════════════════════════════════════════════════════════
#  通用工具函数
# ══════════════════════════════════════════════════════════════════════

def l2_normalize(vecs):
    """L2 归一化到单位球面"""
    norms = np.linalg.norm(vecs, axis=1, keepdims=True)
    return vecs / np.maximum(norms, 1e-12)


def compute_groundtruth(train, test, top_k=TOP_K):
    """基于 cosine 计算 ground truth（分块矩阵乘法避免 OOM）"""
    from tqdm import tqdm

    print(f"  计算 ground truth (top-{top_k})...")
    train_n = l2_normalize(train)
    test_n = l2_normalize(test)
    n_test = len(test)
    gt = np.zeros((n_test, top_k), dtype=np.int32)
    batch_size = 100

    for start in tqdm(range(0, n_test, batch_size), desc="  GT 计算"):
        end = min(start + batch_size, n_test)
        sims = test_n[start:end] @ train_n.T
        if top_k < sims.shape[1]:
            indices = np.argpartition(-sims, top_k, axis=1)[:, :top_k]
            for i in range(end - start):
                order = np.argsort(-sims[i, indices[i]])
                indices[i] = indices[i, order]
        else:
            indices = np.argsort(-sims, axis=1)[:, :top_k]
        gt[start:end] = indices.astype(np.int32)

    return gt


def save_binary(path, data, dtype_str):
    """保存 raw binary 文件"""
    data.tofile(str(path))
    mb = path.stat().st_size / 1024 / 1024
    print(f"  [OK] {path.name}: shape={data.shape}, {mb:.1f} MB")


def check_exists(prefix):
    """检查三文件是否都已存在"""
    train = ROOT / f"{prefix}_train.f32"
    test = ROOT / f"{prefix}_test.f32"
    gt = ROOT / f"{prefix}_groundtruth.i32"
    return train.exists() and test.exists() and gt.exists()


def save_dataset(prefix, train, test, gt):
    """保存三文件"""
    train_path = ROOT / f"{prefix}_train.f32"
    test_path = ROOT / f"{prefix}_test.f32"
    gt_path = ROOT / f"{prefix}_groundtruth.i32"

    save_binary(train_path, train.astype(np.float32), "f32")
    save_binary(test_path, test.astype(np.float32), "f32")
    save_binary(gt_path, gt.astype(np.int32), "i32")


# ══════════════════════════════════════════════════════════════════════
#  数据源处理器
# ══════════════════════════════════════════════════════════════════════

def process_hdf5(cfg):
    """从 ann-benchmarks HDF5 文件转换"""
    import h5py

    hdf5_path = ROOT / cfg["hdf5"]
    if not hdf5_path.exists():
        print(f"  [错误] HDF5 文件不存在: {hdf5_path.name}")
        print(f"         请从 http://ann-benchmarks.com/ 下载后放到项目根目录")
        return

    with h5py.File(str(hdf5_path), "r") as f:
        train = np.array(f["train"], dtype=np.float32)
        test = np.array(f["test"], dtype=np.float32)
        neighbors = np.array(f["neighbors"], dtype=np.int32)

    print(f"  训练集: {train.shape}, 测试集: {test.shape}")

    # 所有数据集都做 L2 归一化（QuIVer 统一使用 cosine）
    train = l2_normalize(train)
    test = l2_normalize(test)

    if cfg["metric"] == "euclidean":
        # 归一化后需要重新计算 GT
        print(f"  [重算 GT] euclidean 数据集归一化后用 cosine 重新计算...")
        gt = compute_groundtruth(train, test, TOP_K)
    else:
        # angular 数据集直接用 HDF5 里的 GT
        gt = neighbors[:, :TOP_K].astype(np.int32)

    save_dataset(cfg["prefix"], train, test, gt)


def process_huggingface(cfg):
    """从 HuggingFace datasets 库下载并转换"""
    from datasets import load_dataset
    from tqdm import tqdm

    n_total = cfg["n_train"] + cfg["n_test"]
    dim = cfg["dim"]
    emb_col = cfg["embedding_col"]
    is_binary = cfg.get("binary_vector", False)

    if cfg.get("streaming", False):
        # Streaming 模式：逐条读取，适合超大数据集
        print(f"  [Streaming] 取前 {n_total:,} 条...")
        ds = load_dataset(cfg["hf_id"], split="train", streaming=True)
        all_emb = np.zeros((n_total, dim), dtype=np.float32)
        count = 0

        for row in tqdm(ds, total=n_total, desc="  提取向量"):
            emb = row[emb_col]
            if is_binary:
                import json
                all_emb[count] = np.array(json.loads(emb), dtype=np.float32)
            elif isinstance(emb, list):
                all_emb[count] = np.array(emb, dtype=np.float32)
            else:
                all_emb[count] = emb.astype(np.float32)
            count += 1
            if count >= n_total:
                break

        if count < n_total:
            print(f"  [!] 实际只取到 {count} 条")
            all_emb = all_emb[:count]
    else:
        # 完整下载模式：一次性加载
        print(f"  [完整下载] 加载数据集...")
        ds = load_dataset(cfg["hf_id"], split="train")
        total_rows = len(ds)
        print(f"  总行数: {total_rows:,}")
        n_total = min(n_total, total_rows)

        all_emb = np.zeros((n_total, dim), dtype=np.float32)
        np.random.seed(42)
        indices = np.random.permutation(total_rows)[:n_total]

        for i, idx in enumerate(tqdm(indices, desc="  提取向量")):
            emb = ds[int(idx)][emb_col]
            if isinstance(emb, list):
                all_emb[i] = np.array(emb, dtype=np.float32)
            else:
                all_emb[i] = emb.astype(np.float32)

    # 随机打乱 → 切分 train/test
    np.random.seed(42)
    all_emb = all_emb[np.random.permutation(len(all_emb))]

    n_test = min(cfg["n_test"], len(all_emb) // 10)
    n_train = len(all_emb) - n_test
    train = all_emb[:n_train]
    test = all_emb[n_train:n_train + n_test]

    print(f"  训练集: {train.shape}, 测试集: {test.shape}")

    gt = compute_groundtruth(train, test, TOP_K)
    save_dataset(cfg["prefix"], train, test, gt)


def process_synthetic(cfg):
    """生成合成数据集"""
    dim = cfg["dim"]
    n_train = cfg["n_train"]
    n_test = cfg["n_test"]

    np.random.seed(42)

    if cfg["generator"] == "random_uniform":
        # 均匀随机：在 S^{D-1} 上均匀采样
        print(f"  生成均匀随机向量: {n_train + n_test} × {dim}")
        all_vecs = np.random.randn(n_train + n_test, dim).astype(np.float32)
    elif cfg["generator"] == "low_rank":
        # 低秩合成：先在 rank 维子空间采样，再嵌入高维
        rank = cfg["rank"]
        print(f"  生成低秩向量: {n_train + n_test} × {dim}, rank={rank}")
        low = np.random.randn(n_train + n_test, rank).astype(np.float32)
        # 随机正交投影矩阵
        proj = np.random.randn(rank, dim).astype(np.float32)
        proj = np.linalg.qr(proj.T)[0].T[:rank]  # rank × dim 正交行
        all_vecs = low @ proj
    else:
        raise ValueError(f"未知生成器: {cfg['generator']}")

    # L2 归一化到单位球面
    all_vecs = l2_normalize(all_vecs)

    train = all_vecs[:n_train]
    test = all_vecs[n_train:n_train + n_test]

    print(f"  训练集: {train.shape}, 测试集: {test.shape}")

    gt = compute_groundtruth(train, test, TOP_K)
    save_dataset(cfg["prefix"], train, test, gt)


def process_hf_bin(cfg):
    """从 HuggingFace Hub 下载预计算好的 raw binary 格式数据集"""
    from huggingface_hub import hf_hub_download
    import shutil

    repo_id = cfg["hf_id"]
    prefix = cfg["prefix"]

    # 预估要下载的三个文件映射
    files_to_download = {
        f"{prefix}_train.f32": ROOT / f"{prefix}_train.f32",
        f"{prefix}_test.f32": ROOT / f"{prefix}_test.f32",
        f"{prefix}_groundtruth.i32": ROOT / f"{prefix}_groundtruth.i32",
    }

    for repo_file, local_path in files_to_download.items():
        print(f"  正在下载 {repo_file} 并保存到 {local_path}...")
        downloaded_path = hf_hub_download(
            repo_id=repo_id,
            filename=repo_file,
            repo_type="dataset"
        )
        # 将缓存文件拷贝到目标位置
        shutil.copy(downloaded_path, local_path)
        mb = local_path.stat().st_size / 1024 / 1024
        print(f"  [OK] {repo_file} ({mb:.1f} MB)")


# ══════════════════════════════════════════════════════════════════════
#  主入口
# ══════════════════════════════════════════════════════════════════════

PROCESSORS = {
    "hdf5": process_hdf5,
    "huggingface": process_huggingface,
    "synthetic": process_synthetic,
    "hf_bin": process_hf_bin,
}


def process_one(name, cfg):
    """处理单个数据集"""
    prefix = cfg["prefix"]

    print(f"\n{'=' * 70}")
    print(f"  {cfg['desc']}")
    print(f"  源: {cfg['source']} | 前缀: {prefix}")
    print(f"{'=' * 70}")

    if check_exists(prefix):
        # 打印已有文件大小
        train_mb = (ROOT / f"{prefix}_train.f32").stat().st_size / 1024 / 1024
        print(f"  [跳过] 三文件已存在 (train: {train_mb:.0f} MB)")
        return True

    processor = PROCESSORS.get(cfg["source"])
    if not processor:
        print(f"  [错误] 未知数据源类型: {cfg['source']}")
        return False

    t0 = time.time()
    processor(cfg)
    print(f"  [完成] 耗时 {time.time() - t0:.1f}s")
    return True


def main():
    # 处理命令行参数
    args = sys.argv[1:]

    if "--list" in args or "-l" in args:
        print("可用数据集:")
        for name, cfg in DATASETS.items():
            exists = "✓" if check_exists(cfg["prefix"]) else " "
            print(f"  [{exists}] {name:<14} {cfg['desc']}")
        return

    if "--help" in args or "-h" in args:
        print(__doc__)
        return

    names = args if args else list(DATASETS.keys())

    # 验证名称
    for name in names:
        if name not in DATASETS:
            print(f"[错误] 未知数据集: {name}")
            print(f"  可选: {', '.join(DATASETS.keys())}")
            print(f"  用 --list 查看所有数据集")
            sys.exit(1)

    print(f"将处理 {len(names)} 个数据集: {', '.join(names)}")
    print(f"输出目录: {ROOT}")

    ok, fail = 0, 0
    for name in names:
        try:
            if process_one(name, DATASETS[name]):
                ok += 1
            else:
                fail += 1
        except Exception as e:
            print(f"\n  [错误] {name}: {e}")
            import traceback
            traceback.print_exc()
            fail += 1

    print(f"\n{'=' * 70}")
    print(f"  完成! 成功: {ok}, 失败: {fail}")
    print(f"{'=' * 70}")


if __name__ == "__main__":
    main()
