#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
理论验证实验：对比 Proposition 3 (Bernstein) 的理论上界与实际 misranking 概率。

实验 1: Misranking rate vs Δθ
  - 在每个数据集上采样三元组 (u, v, w)，按 Δθ 分桶
  - 测量实际 misranking 概率
  - 与 Bernstein/Hoeffding 理论上界对比

实验 2: p_s 和 ν₂ across datasets
  - 在所有数据集上测量 p_s（强 bit 概率）
  - 计算对应的 ν₂ = (1+3p_s)²
  - 与 Recall@10 (ef=64) 定性对比
"""

import sys
import numpy as np
from pathlib import Path
import time

if sys.platform == "win32":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")

ROOT = Path(__file__).resolve().parent.parent


# ==================== 数据加载 ====================

def load_vectors(prefix, dim, max_n=None):
    """加载 raw float32 向量"""
    path = ROOT / f"{prefix}_train.f32"
    if not path.exists():
        raise FileNotFoundError(f"{path} 不存在")
    data = np.fromfile(str(path), dtype=np.float32)
    n = len(data) // dim
    if max_n and n > max_n:
        n = max_n
    data = data[:n * dim].reshape(n, dim)
    # L2 归一化
    norms = np.linalg.norm(data, axis=1, keepdims=True)
    norms = np.maximum(norms, 1e-12)
    data = data / norms
    return data


# ==================== BQ2 编码 ====================

def encode_bq2(vectors):
    """
    2-bit Sign-Magnitude 编码。
    返回: sign_bits (bool), magnitude_bits (bool)
    """
    # 符号位: x_i >= 0
    sign_bits = vectors >= 0
    # 幅度位: |x_i| > mean(|x_i|) (逐向量阈值)
    abs_vals = np.abs(vectors)
    thresholds = np.mean(abs_vals, axis=1, keepdims=True)
    magnitude_bits = abs_vals > thresholds
    return sign_bits, magnitude_bits


def bq2_distance(sign_u, mag_u, sign_v, mag_v):
    """
    计算 BQ2 加权汉明距离。
    权重: both-strong=4, one-strong=2, both-weak=1
    """
    # 符号不一致
    disagree = sign_u != sign_v  # (n_pairs, D)
    # 幅度类别
    both_strong = mag_u & mag_v
    one_strong = mag_u ^ mag_v  # XOR: 恰好一个 strong
    # both_weak = ~mag_u & ~mag_v  # 隐含在 weight=1 中

    # 加权距离 = 4 * (disagree & both_strong) + 2 * (disagree & one_strong) + 1 * (disagree & both_weak)
    # = disagree * (1 + 2*one_strong + 3*both_strong)
    # 简化: weight = 1 + mag_u + mag_v + 2*(mag_u & mag_v)
    #   both_weak: 1, one_strong: 2, both_strong: 4 ✓
    weight = 1 + mag_u.astype(np.int32) + mag_v.astype(np.int32) + 2 * both_strong.astype(np.int32)
    distance = np.sum(disagree.astype(np.int32) * weight, axis=-1)
    return distance


# ==================== 实验 1: Misranking vs Δθ ====================

def experiment_misranking(vectors, n_triplets=50000, seed=42):
    """
    采样三元组，按 Δθ 分桶，统计实际 misranking 率。
    返回: 每个 Δθ 桶的 (Δθ_center, empirical_rate, n_samples, hoeffding_bound, bernstein_bound)
    """
    rng = np.random.RandomState(seed)
    n = len(vectors)

    # 编码
    sign_bits, mag_bits = encode_bq2(vectors)

    # 测量 p_s
    ps = np.mean(mag_bits.astype(np.float64))
    nu2 = (1 + 3 * ps) ** 2
    w_bar = (1 + ps) ** 2
    D = vectors.shape[1]
    B = 8  # 范围

    print(f"  维度 D={D}, 向量数 n={n}")
    print(f"  p_s={ps:.4f}, nu2={nu2:.4f}, w_bar={w_bar:.4f}")

    # 采样三元组索引
    idx_u = rng.randint(0, n, size=n_triplets)
    idx_v = rng.randint(0, n, size=n_triplets)
    idx_w = rng.randint(0, n, size=n_triplets)

    # 排除相同索引
    mask = (idx_u != idx_v) & (idx_u != idx_w) & (idx_v != idx_w)
    idx_u, idx_v, idx_w = idx_u[mask], idx_v[mask], idx_w[mask]

    # 计算真实角度 (余弦 → 角度)
    # 分批计算避免内存爆炸
    batch_size = 5000
    all_theta_uv = []
    all_theta_uw = []
    all_bq_uv = []
    all_bq_uw = []

    for start in range(0, len(idx_u), batch_size):
        end = min(start + batch_size, len(idx_u))
        u = vectors[idx_u[start:end]]
        v = vectors[idx_v[start:end]]
        w = vectors[idx_w[start:end]]

        # 余弦相似度 → 角度
        cos_uv = np.clip(np.sum(u * v, axis=1), -1, 1)
        cos_uw = np.clip(np.sum(u * w, axis=1), -1, 1)
        theta_uv = np.arccos(cos_uv)
        theta_uw = np.arccos(cos_uw)

        # BQ2 距离
        su, mu = sign_bits[idx_u[start:end]], mag_bits[idx_u[start:end]]
        sv, mv = sign_bits[idx_v[start:end]], mag_bits[idx_v[start:end]]
        sw, mw = sign_bits[idx_w[start:end]], mag_bits[idx_w[start:end]]
        bq_uv = bq2_distance(su, mu, sv, mv)
        bq_uw = bq2_distance(su, mu, sw, mw)

        all_theta_uv.append(theta_uv)
        all_theta_uw.append(theta_uw)
        all_bq_uv.append(bq_uv)
        all_bq_uw.append(bq_uw)

    theta_uv = np.concatenate(all_theta_uv)
    theta_uw = np.concatenate(all_theta_uw)
    bq_uv = np.concatenate(all_bq_uv)
    bq_uw = np.concatenate(all_bq_uw)

    # 确保 theta_uv < theta_uw（v 是真正的近邻）
    # 交换使得 v 总是更近
    swap = theta_uv > theta_uw
    theta_uv[swap], theta_uw[swap] = theta_uw[swap].copy(), theta_uv[swap].copy()
    bq_uv[swap], bq_uw[swap] = bq_uw[swap].copy(), bq_uv[swap].copy()

    delta_theta = theta_uw - theta_uv

    # Misranking: BQ 距离排序错误（d̂(u,v) >= d̂(u,w) 当 θ_uv < θ_uw）
    misranked = bq_uv >= bq_uw

    # 按 Δθ 分桶
    bins = [0.01, 0.05, 0.10, 0.15, 0.20, 0.30, 0.50, 0.80, 1.20]
    results = []

    for i in range(len(bins) - 1):
        lo, hi = bins[i], bins[i + 1]
        mask = (delta_theta >= lo) & (delta_theta < hi)
        n_in_bin = np.sum(mask)
        if n_in_bin < 50:
            continue

        emp_rate = np.mean(misranked[mask])
        dt_center = (lo + hi) / 2

        # 理论上界（理想化模型）
        mu_model = D * w_bar * dt_center / np.pi

        # Hoeffding
        hoeff = np.exp(-2 * mu_model ** 2 / (D * B ** 2))
        hoeff = min(hoeff, 1.0)

        # Bernstein (理想化)
        denom = 2 * 2 * D * nu2 + (2 / 3) * B * mu_model
        bern = np.exp(-mu_model ** 2 / denom)
        bern = min(bern, 1.0)

        results.append({
            "dtheta_lo": lo,
            "dtheta_hi": hi,
            "dtheta_center": dt_center,
            "n_samples": int(n_in_bin),
            "empirical": emp_rate,
            "hoeffding": hoeff,
            "bernstein": bern,
        })

    return results, ps, nu2, w_bar


# ==================== 实验 2: p_s across datasets ====================

def measure_ps(vectors):
    """测量一个数据集的 p_s"""
    abs_vals = np.abs(vectors)
    thresholds = np.mean(abs_vals, axis=1, keepdims=True)
    mag_bits = abs_vals > thresholds
    ps = np.mean(mag_bits.astype(np.float64))
    return ps


# ==================== 主函数 ====================

DATASETS = {
    "cohere":       {"prefix": "cohere",                  "dim": 768,  "type": "contrastive"},
    "minilm":       {"prefix": "minilm",                  "dim": 384,  "type": "contrastive"},
    "bge_m3":       {"prefix": "bge_m3",                  "dim": 1024, "type": "contrastive"},
    "dbpedia_1536": {"prefix": "dbpedia_openai",          "dim": 1536, "type": "contrastive"},
    "dbpedia_3072": {"prefix": "dbpedia_openai_3072",     "dim": 3072, "type": "contrastive"},
    "wolt_clip":    {"prefix": "wolt_clip",               "dim": 512,  "type": "multimodal"},
    "sift128":      {"prefix": "sift128",                 "dim": 128,  "type": "euclidean"},
    "gist960":      {"prefix": "gist960",                 "dim": 960,  "type": "euclidean"},
    "glove100":     {"prefix": "glove100",                "dim": 100,  "type": "word-vec"},
    "random":       {"prefix": "random",                  "dim": 768,  "type": "random"},
    "sphere":       {"prefix": "sphere",                  "dim": 768,  "type": "synthetic-LR"},
}


def main():
    print("=" * 70)
    print("  QuIVer 理论验证实验")
    print("=" * 70)

    # ── 实验 1: Misranking vs Δθ（在 Cohere-1M 上）──
    print("\n" + "=" * 70)
    print("  实验 1: Misranking Rate vs Δθ (Cohere-1M)")
    print("=" * 70)

    t0 = time.time()
    # 加载前 100K 向量即可（加速）
    vectors = load_vectors("cohere", 768, max_n=100000)
    print(f"  加载 {len(vectors)} 向量, 耗时 {time.time() - t0:.1f}s")

    results, ps, nu2, w_bar = experiment_misranking(vectors, n_triplets=100000)

    print(f"\n  {'Δθ 范围':<14} {'样本数':>7} {'实际':>8} {'Hoeffding':>10} {'Bernstein':>10} {'松弛倍数':>10}")
    print("  " + "-" * 62)
    for r in results:
        ratio = r["bernstein"] / max(r["empirical"], 1e-10)
        print(f"  [{r['dtheta_lo']:.2f}, {r['dtheta_hi']:.2f})  "
              f"{r['n_samples']:>6}  "
              f"{r['empirical']:>7.4f}  "
              f"{r['hoeffding']:>9.4f}  "
              f"{r['bernstein']:>9.4f}  "
              f"{ratio:>9.1f}x")

    print(f"\n  总耗时: {time.time() - t0:.1f}s")

    # ── 实验 2: p_s across datasets ──
    print("\n" + "=" * 70)
    print("  实验 2: p_s 和 ν₂ Across All Datasets")
    print("=" * 70)

    print(f"\n  {'数据集':<16} {'类型':<14} {'维度':>5} {'p_s':>7} {'ν₂':>7} {'w_bar':>7} {'ν₂/16':>7}")
    print("  " + "-" * 68)

    for name in ["cohere", "minilm", "bge_m3", "dbpedia_1536", "dbpedia_3072",
                  "wolt_clip", "sift128", "gist960", "glove100", "random", "sphere"]:
        cfg = DATASETS[name]
        try:
            # 只加载前 50K 节省时间
            vecs = load_vectors(cfg["prefix"], cfg["dim"], max_n=50000)
            ps_val = measure_ps(vecs)
            nu2_val = (1 + 3 * ps_val) ** 2
            wbar_val = (1 + ps_val) ** 2
            print(f"  {name:<16} {cfg['type']:<14} {cfg['dim']:>5} "
                  f"{ps_val:>6.4f}  {nu2_val:>6.2f}  {wbar_val:>6.2f}  {nu2_val/16:>6.3f}")
        except Exception as e:
            print(f"  {name:<16} [错误] {e}")

    print("\n  完成!")


if __name__ == "__main__":
    main()
