"""
统一 Baseline 参数扫描 Benchmark
=================================
覆盖: hnswlib / FAISS HNSW / FAISS IVF-PQ / USearch / VSAG / VSAG-HGraph
数据集: Cohere-1M (768-d, cosine) 或通过环境变量指定

用法:
  pip install hnswlib faiss-cpu usearch pyvsag   # 按需安装
  python bench_baselines.py                                # 跑全部已安装的
  BASELINES=hnswlib,faiss_hnsw python bench_baselines.py   # 只跑指定的

环境变量:
  TRIVIUM_ANN_DIM          向量维度 (默认 768)
  TRIVIUM_ANN_TRAIN        训练集 .f32 文件路径
  TRIVIUM_ANN_TEST         测试集 .f32 文件路径
  TRIVIUM_ANN_GT           GroundTruth .i32 文件路径
  BASELINES                逗号分隔的 baseline 名
  BASELINE_START           从指定 baseline 开始 (跳过之前的)
"""

import numpy as np
import time
import os
import sys
import json
from typing import List, Tuple, Dict, Optional, Callable
from concurrent.futures import ThreadPoolExecutor

# ============================================================
#  配置
# ============================================================

DIM = int(os.environ.get("TRIVIUM_ANN_DIM", "768"))
K = 10
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECT_DIR = os.path.dirname(SCRIPT_DIR)

TRAIN_PATH = os.environ.get("TRIVIUM_ANN_TRAIN", os.path.join(PROJECT_DIR, "cohere_train.f32"))
TEST_PATH = os.environ.get("TRIVIUM_ANN_TEST", os.path.join(PROJECT_DIR, "cohere_test.f32"))
GT_PATH = os.environ.get("TRIVIUM_ANN_GT", os.path.join(PROJECT_DIR, "cohere_groundtruth.i32"))

# === HNSW 类参数 (hnswlib / FAISS HNSW / USearch) ===
#   - M × ef_c 交叉扫描：覆盖低/中/高构图质量
#   - ef_search 密扫，画 Recall-QPS Pareto 曲线
# 注意: HNSW 的最大度数 = 2*M，所以 M=32 ↔ QuIVer m=32（最大度 64）
EF_CONSTRUCTION_VALUES = [64, 128, 168]
M_VALUES = [8, 16, 32, 48]
EF_SEARCH_VALUES = [10, 20, 40, 80, 120, 200, 400, 600, 800]

# === FAISS IVF-PQ 参数 ===
NLIST_VALUES = [256, 1024, 4096]
M_PQ_VALUES = [32, 48, 64, 96]  # PQ 子空间数，必须整除 DIM(768)
NPROBE_VALUES = [1, 4, 8, 16, 32, 64, 128, 256]

# === VSAG 参数 ===
# VSAG Python 包当前官方示例以 HNSW 为稳定入口；max_degree 对齐 HNSW 的 M。
VSAG_MAX_DEGREE_VALUES = [8, 16, 32, 48]
VSAG_EF_CONSTRUCTION_VALUES = [64, 128, 168]
VSAG_EF_SEARCH_VALUES = [10, 20, 40, 80, 120, 200, 400, 600, 800]
VSAG_BATCH_THREADS = int(os.environ.get("VSAG_BATCH_THREADS", str(os.cpu_count() or 8)))
VSAG_HGRAPH_BASE_QUANTIZATION_VALUES = ["sq8", "sq8_reorder_fp32"]

# ============================================================
#  数据加载
# ============================================================

def load_data():
    """加载训练集、测试集和 GroundTruth"""
    print("=" * 70)
    print("加载数据...")
    print(f"  训练集: {TRAIN_PATH}")
    print(f"  测试集: {TEST_PATH}")
    print(f"  GT: {GT_PATH}")
    print(f"  维度: {DIM}")

    train = np.fromfile(TRAIN_PATH, dtype=np.float32).reshape(-1, DIM)
    queries = np.fromfile(TEST_PATH, dtype=np.float32).reshape(-1, DIM)
    gt_raw = np.fromfile(GT_PATH, dtype=np.int32)
    n_queries = queries.shape[0]
    k_gt = gt_raw.shape[0] // n_queries
    gt = gt_raw.reshape(n_queries, k_gt)[:, :K]

    print(f"  训练集: {train.shape}")
    print(f"  测试集: {queries.shape}")
    print(f"  GT: {gt.shape}")
    print("=" * 70)
    return train, queries, gt


def compute_recall(pred_labels: np.ndarray, gt: np.ndarray) -> float:
    """计算 Recall@K"""
    n = gt.shape[0]
    hits = 0
    for i in range(n):
        hits += len(set(pred_labels[i].tolist()) & set(gt[i].tolist()))
    return hits / (n * K)


def normalize_rows(data: np.ndarray) -> np.ndarray:
    """L2 归一化（cosine 度量需要）"""
    norms = np.linalg.norm(data, axis=1, keepdims=True)
    norms = np.maximum(norms, 1e-12)
    return data / norms


def measure_qps(search_fn: Callable, queries: np.ndarray, warmup: int = 10) -> Tuple[np.ndarray, float, float]:
    """测量 QPS，返回 (labels, qps, latency_ms_per_query)"""
    # 预热
    search_fn(queries[:min(warmup, len(queries))])
    # 正式
    t0 = time.perf_counter()
    labels = search_fn(queries)
    elapsed = time.perf_counter() - t0
    qps = len(queries) / elapsed
    latency_ms = elapsed / len(queries) * 1000.0
    return labels, qps, latency_ms


# 全局 brute-force baseline QPS（main 中设置）
BRUTE_FORCE_QPS: float = 0.0


def measure_brute_force(train: np.ndarray, queries: np.ndarray, gt: np.ndarray) -> float:
    """测量 brute-force（精确搜索）QPS 作为加速比基准"""
    train_norm = normalize_rows(train)
    queries_norm = normalize_rows(queries)
    # 用 numpy 矩阵乘法模拟 exact search
    t0 = time.perf_counter()
    sims = queries_norm @ train_norm.T  # (n_query, n_train)
    # 取 top-K
    indices = np.argpartition(-sims, K, axis=1)[:, :K]
    for i in range(len(queries)):
        top_idx = indices[i]
        top_sims = sims[i, top_idx]
        indices[i] = top_idx[np.argsort(-top_sims)]
    elapsed = time.perf_counter() - t0
    qps = len(queries) / elapsed
    latency_ms = elapsed / len(queries) * 1000.0
    recall = compute_recall(indices, gt)
    print(f"  Brute-force QPS: {qps:,.0f}  (latency: {latency_ms:.2f} ms/q, recall: {recall*100:.1f}%)")
    return qps


def estimate_hnsw_memory(n: int, dim: int, M: int, stores_vectors: bool = True) -> Tuple[float, str]:
    """
    估算 HNSW 索引的内存占用。
    返回 (bytes, 可读字符串)。
    
    结构：
      - 邻接表 (L0): n * (2*M) * 4 bytes (每个节点最多 2M 个邻居)
      - 邻接表 (上层): 忽略不计，仅少数节点
      - 向量存储 (如果 stores_vectors): n * dim * 4
      - 其他元数据 (每节点 label + 链接列表头): n * 16
    """
    adj_bytes = n * (2 * M) * 4  # L0 邻接表
    vec_bytes = n * dim * 4 if stores_vectors else 0  # 向量存储
    overhead = n * 16  # label + 链接列表头 + 指针
    total = adj_bytes + vec_bytes + overhead
    mb = total / 1024 / 1024
    if stores_vectors:
        adj_mb = adj_bytes / 1024 / 1024
        vec_mb = vec_bytes / 1024 / 1024
        detail = f"{mb:,.0f} MB (向量: {vec_mb:,.0f} MB + 邻接: {adj_mb:,.0f} MB)"
    else:
        detail = f"{mb:,.0f} MB"
    return total, detail


# ============================================================
#  Baseline: hnswlib
# ============================================================

def bench_hnswlib(train: np.ndarray, queries: np.ndarray, gt: np.ndarray):
    """hnswlib 全参数扫描"""
    try:
        import hnswlib
    except ImportError:
        print("[跳过] hnswlib 未安装 (pip install hnswlib)")
        return

    n_train = train.shape[0]
    print("\n" + "=" * 70)
    print("Baseline: hnswlib")
    print("=" * 70)

    for ef_c in EF_CONSTRUCTION_VALUES:
        for M in M_VALUES:
            print(f"\n--- hnswlib M={M}, ef_c={ef_c} ---")
            idx = hnswlib.Index(space='cosine', dim=DIM)
            idx.init_index(max_elements=n_train, ef_construction=ef_c, M=M)
            idx.set_num_threads(os.cpu_count() or 8)

            t0 = time.perf_counter()
            idx.add_items(train, np.arange(n_train))
            build_time = time.perf_counter() - t0
            vecs_per_sec = n_train / build_time
            _, mem_str = estimate_hnsw_memory(n_train, DIM, M, stores_vectors=True)
            print(f"构建: {build_time:.1f}s ({vecs_per_sec:,.0f} vecs/s)")
            print(f"内存: {mem_str}")

            header = f"{'ef':>6} {'R@10':>9} {'1T-QPS':>10} {'MT-QPS':>10} {'lat(ms)':>9} {'1T/BF':>8} {'MT/BF':>8}"
            print(header)
            print("-" * len(header))

            for ef in EF_SEARCH_VALUES:
                idx.set_ef(ef)

                # 单线程
                idx.set_num_threads(1)
                labels_1t, qps_1t, lat_ms = measure_qps(
                    lambda q: idx.knn_query(q, k=K)[0], queries
                )
                recall = compute_recall(labels_1t, gt)

                # 多线程
                idx.set_num_threads(os.cpu_count() or 8)
                _, qps_mt, _ = measure_qps(
                    lambda q: idx.knn_query(q, k=K)[0], queries
                )

                sp_1t = qps_1t / BRUTE_FORCE_QPS if BRUTE_FORCE_QPS > 0 else 0
                sp_mt = qps_mt / BRUTE_FORCE_QPS if BRUTE_FORCE_QPS > 0 else 0
                print(f"{ef:>6} {recall*100:>8.2f}% {qps_1t:>10,.0f} {qps_mt:>10,.0f} {lat_ms:>9.2f} {sp_1t:>7.1f}x {sp_mt:>7.1f}x")


# ============================================================
#  Baseline: FAISS HNSW
# ============================================================

def bench_faiss_hnsw(train: np.ndarray, queries: np.ndarray, gt: np.ndarray):
    """FAISS IndexHNSWFlat 全参数扫描"""
    try:
        import faiss
    except ImportError:
        print("[跳过] faiss 未安装 (pip install faiss-cpu)")
        return

    # FAISS HNSW 使用 inner product (对归一化向量等价于 cosine)
    train_norm = normalize_rows(train)
    queries_norm = normalize_rows(queries)
    n_train = train.shape[0]

    print("\n" + "=" * 70)
    print("Baseline: FAISS HNSW (IndexHNSWFlat)")
    print("=" * 70)

    for ef_c in EF_CONSTRUCTION_VALUES:
        for M in M_VALUES:
            print(f"\n--- FAISS HNSW M={M}, ef_c={ef_c} ---")

            idx = faiss.IndexHNSWFlat(DIM, M, faiss.METRIC_INNER_PRODUCT)
            idx.hnsw.efConstruction = ef_c

            t0 = time.perf_counter()
            idx.add(train_norm)
            build_time = time.perf_counter() - t0
            vecs_per_sec = n_train / build_time
            _, mem_str = estimate_hnsw_memory(n_train, DIM, M, stores_vectors=True)
            print(f"构建: {build_time:.1f}s ({vecs_per_sec:,.0f} vecs/s)")
            print(f"内存: {mem_str}")

            header = f"{'ef':>6} {'R@10':>9} {'1T-QPS':>10} {'MT-QPS':>10} {'lat(ms)':>9} {'1T/BF':>8} {'MT/BF':>8}"
            print(header)
            print("-" * len(header))

            for ef in EF_SEARCH_VALUES:
                idx.hnsw.efSearch = ef

                # 单线程
                faiss.omp_set_num_threads(1)
                labels_1t, qps_1t, lat_ms = measure_qps(
                    lambda q: idx.search(q, K)[1], queries_norm
                )
                recall = compute_recall(labels_1t, gt)

                # 多线程
                faiss.omp_set_num_threads(os.cpu_count() or 8)
                _, qps_mt, _ = measure_qps(
                    lambda q: idx.search(q, K)[1], queries_norm
                )

                sp_1t = qps_1t / BRUTE_FORCE_QPS if BRUTE_FORCE_QPS > 0 else 0
                sp_mt = qps_mt / BRUTE_FORCE_QPS if BRUTE_FORCE_QPS > 0 else 0
                print(f"{ef:>6} {recall*100:>8.2f}% {qps_1t:>10,.0f} {qps_mt:>10,.0f} {lat_ms:>9.2f} {sp_1t:>7.1f}x {sp_mt:>7.1f}x")


# ============================================================
#  Baseline: FAISS IVF-PQ
# ============================================================

def bench_faiss_ivfpq(train: np.ndarray, queries: np.ndarray, gt: np.ndarray):
    """FAISS IVF-PQ 参数扫描（含 OPQ + Refine）"""
    try:
        import faiss
    except ImportError:
        print("[跳过] faiss 未安装 (pip install faiss-cpu)")
        return

    train_norm = normalize_rows(train)
    queries_norm = normalize_rows(queries)
    n_train = train.shape[0]

    print("\n" + "=" * 70)
    print("Baseline: FAISS IVF-PQ")
    print("=" * 70)

    # ---- 第一部分：裸 IVF-PQ（作为参考） ----
    print("\n>>> 模式 A: 裸 IVF-PQ（无 OPQ / 无 Refine）<<<")
    for nlist in [1024]:
        for m_pq in [96]:
            if DIM % m_pq != 0:
                continue
            print(f"\n--- 裸 IVF-PQ nlist={nlist}, m_pq={m_pq} ---")
            quantizer = faiss.IndexFlatIP(DIM)
            idx = faiss.IndexIVFPQ(quantizer, DIM, nlist, m_pq, 8,
                                   faiss.METRIC_INNER_PRODUCT)
            t0 = time.perf_counter()
            idx.train(train_norm)
            idx.add(train_norm)
            build_time = time.perf_counter() - t0
            pq_bytes = n_train * m_pq
            print(f"构建(含训练): {build_time:.1f}s")
            print(f"内存: {pq_bytes/1024/1024:,.0f} MB (仅 PQ 码)")

            header = f"{'nprobe':>8} {'R@10':>9} {'1T-QPS':>10} {'MT-QPS':>10} {'lat(ms)':>9} {'1T/BF':>8} {'MT/BF':>8}"
            print(header)
            print("-" * len(header))
            for nprobe in NPROBE_VALUES:
                idx.nprobe = nprobe
                faiss.omp_set_num_threads(1)
                labels_1t, qps_1t, lat_ms = measure_qps(
                    lambda q: idx.search(q, K)[1], queries_norm
                )
                recall = compute_recall(labels_1t, gt)
                faiss.omp_set_num_threads(os.cpu_count() or 8)
                _, qps_mt, _ = measure_qps(
                    lambda q: idx.search(q, K)[1], queries_norm
                )
                sp_1t = qps_1t / BRUTE_FORCE_QPS if BRUTE_FORCE_QPS > 0 else 0
                sp_mt = qps_mt / BRUTE_FORCE_QPS if BRUTE_FORCE_QPS > 0 else 0
                print(f"{nprobe:>8} {recall*100:>8.2f}% {qps_1t:>10,.0f} {qps_mt:>10,.0f} {lat_ms:>9.2f} {sp_1t:>7.1f}x {sp_mt:>7.1f}x")

    # ---- 第二部分：OPQ + IVF-PQ + Refine（完整流水线） ----
    print("\n>>> 模式 B: OPQ + IVF-PQ + Refine（生产级配置）<<<")

    OPQ_M_PQ_VALUES = [64, 96, 128, 192]
    OPQ_NLIST_VALUES = [1024, 4096]

    for nlist in OPQ_NLIST_VALUES:
        for m_pq in OPQ_M_PQ_VALUES:
            if DIM % m_pq != 0:
                print(f"[跳过] nlist={nlist}, m_pq={m_pq}: {DIM} 不能被 {m_pq} 整除")
                continue

            print(f"\n--- OPQ+IVF-PQ+Refine nlist={nlist}, m_pq={m_pq} ---")

            try:
                # 构建 OPQ 预处理 + IVF-PQ
                opq_matrix = faiss.OPQMatrix(DIM, m_pq)
                quantizer = faiss.IndexFlatIP(DIM)
                sub_index = faiss.IndexIVFPQ(quantizer, DIM, nlist, m_pq, 8,
                                             faiss.METRIC_INNER_PRODUCT)
                # OPQ 预旋转 + IVF-PQ
                index_opq = faiss.IndexPreTransform(opq_matrix, sub_index)

                t0 = time.perf_counter()
                index_opq.train(train_norm)
                index_opq.add(train_norm)
                train_time = time.perf_counter() - t0

                # 加 Refine（用原始 f32 向量精排）
                # IndexRefineFlat 会保存一份原始向量用于精排
                refine_index = faiss.IndexRefineFlat(index_opq, faiss.swig_ptr(train_norm))

                pq_bytes = n_train * m_pq
                vec_bytes = n_train * DIM * 4
                total_mb = (pq_bytes + vec_bytes) / 1024 / 1024
                print(f"构建(含 OPQ 训练): {train_time:.1f}s")
                print(f"内存: {total_mb:,.0f} MB (PQ码: {pq_bytes/1024/1024:,.0f} MB + f32精排: {vec_bytes/1024/1024:,.0f} MB)")
            except Exception as e:
                # IndexRefineFlat 构造方式可能因 FAISS 版本不同
                # 备用方案：手动加载
                print(f"[备用] 尝试 IndexRefineFlat 标准构造...")
                try:
                    opq_matrix = faiss.OPQMatrix(DIM, m_pq)
                    quantizer = faiss.IndexFlatIP(DIM)
                    sub_index = faiss.IndexIVFPQ(quantizer, DIM, nlist, m_pq, 8,
                                                 faiss.METRIC_INNER_PRODUCT)
                    index_opq = faiss.IndexPreTransform(opq_matrix, sub_index)

                    t0 = time.perf_counter()
                    index_opq.train(train_norm)
                    index_opq.add(train_norm)
                    train_time = time.perf_counter() - t0

                    refine_index = faiss.IndexRefineFlat(index_opq)
                    refine_index.add(train_norm)

                    pq_bytes = n_train * m_pq
                    vec_bytes = n_train * DIM * 4
                    total_mb = (pq_bytes + vec_bytes) / 1024 / 1024
                    print(f"构建(含 OPQ 训练): {train_time:.1f}s")
                    print(f"内存: {total_mb:,.0f} MB (PQ码: {pq_bytes/1024/1024:,.0f} MB + f32精排: {vec_bytes/1024/1024:,.0f} MB)")
                except Exception as e2:
                    print(f"[错误] OPQ+Refine 构建失败: {e2}")
                    continue

            # 搜索时用 k_factor 控制候选数量
            header = f"{'nprobe':>8} {'k_fac':>6} {'R@10':>9} {'1T-QPS':>10} {'MT-QPS':>10} {'lat(ms)':>9} {'1T/BF':>8} {'MT/BF':>8}"
            print(header)
            print("-" * len(header))

            for nprobe in [4, 16, 32, 64, 128, 256]:
                # 设置 IVF-PQ 底层的 nprobe
                faiss.ParameterSpace().set_index_parameter(refine_index, "nprobe", nprobe)

                for k_factor in [1, 4, 10, 20]:
                    refine_index.k_factor = k_factor

                    faiss.omp_set_num_threads(1)
                    labels_1t, qps_1t, lat_ms = measure_qps(
                        lambda q: refine_index.search(q, K)[1], queries_norm
                    )
                    recall = compute_recall(labels_1t, gt)

                    faiss.omp_set_num_threads(os.cpu_count() or 8)
                    _, qps_mt, _ = measure_qps(
                        lambda q: refine_index.search(q, K)[1], queries_norm
                    )

                    sp_1t = qps_1t / BRUTE_FORCE_QPS if BRUTE_FORCE_QPS > 0 else 0
                    sp_mt = qps_mt / BRUTE_FORCE_QPS if BRUTE_FORCE_QPS > 0 else 0
                    print(f"{nprobe:>8} {k_factor:>6} {recall*100:>8.2f}% {qps_1t:>10,.0f} {qps_mt:>10,.0f} {lat_ms:>9.2f} {sp_1t:>7.1f}x {sp_mt:>7.1f}x")


# ============================================================
#  Baseline: USearch
# ============================================================

def bench_usearch(train: np.ndarray, queries: np.ndarray, gt: np.ndarray):
    """USearch Python 全参数扫描"""
    try:
        from usearch.index import Index
    except ImportError:
        print("[跳过] usearch 未安装 (pip install usearch)")
        return

    n_train = train.shape[0]
    print("\n" + "=" * 70)
    print("Baseline: USearch HNSW")
    print("=" * 70)

    for ef_c in EF_CONSTRUCTION_VALUES:
        for M in M_VALUES:
            print(f"\n--- USearch M={M}, ef_c={ef_c} ---")

            idx = Index(ndim=DIM, metric='cos', connectivity=M,
                        expansion_add=ef_c, dtype='f32')

            t0 = time.perf_counter()
            idx.add(np.arange(n_train), train)
            build_time = time.perf_counter() - t0
            vecs_per_sec = n_train / build_time
            # USearch 内存估算：和 hnswlib 类似，但用 uint40_t (5 bytes) 作为节点 ID
            _, mem_str = estimate_hnsw_memory(n_train, DIM, M, stores_vectors=True)
            print(f"构建: {build_time:.1f}s ({vecs_per_sec:,.0f} vecs/s)")
            print(f"内存: {mem_str}")

            header = f"{'ef':>6} {'R@10':>9} {'1T-QPS':>10} {'MT-QPS':>10} {'lat(ms)':>9} {'1T/BF':>8} {'MT/BF':>8}"
            print(header)
            print("-" * len(header))

            n_threads = os.cpu_count() or 8
            for ef in EF_SEARCH_VALUES:
                idx.expansion_search = ef

                # 单线程
                def search_fn_1t(q):
                    results = idx.search(q, K, exact=False, threads=1)
                    return results.keys

                labels_1t, qps_1t, lat_ms = measure_qps(search_fn_1t, queries)
                recall = compute_recall(labels_1t, gt)

                # 多线程
                def search_fn_mt(q):
                    results = idx.search(q, K, exact=False, threads=n_threads)
                    return results.keys

                _, qps_mt, _ = measure_qps(search_fn_mt, queries)

                sp_1t = qps_1t / BRUTE_FORCE_QPS if BRUTE_FORCE_QPS > 0 else 0
                sp_mt = qps_mt / BRUTE_FORCE_QPS if BRUTE_FORCE_QPS > 0 else 0
                print(f"{ef:>6} {recall*100:>8.2f}% {qps_1t:>10,.0f} {qps_mt:>10,.0f} {lat_ms:>9.2f} {sp_1t:>7.1f}x {sp_mt:>7.1f}x")


# ============================================================
#  Baseline: VSAG (pyvsag)
# ============================================================

def _vsag_batch_search(index, queries: np.ndarray, ef_search: int, threads: int, index_name: str = "hnsw") -> np.ndarray:
    """用 pyvsag 单点 knn_search 组成批量查询。"""
    search_params = json.dumps({index_name: {"ef_search": ef_search}})

    def search_one(q: np.ndarray) -> np.ndarray:
        ids, _ = index.knn_search(vector=np.asarray(q, dtype=np.float32), k=K, parameters=search_params)
        return np.asarray(ids, dtype=np.int64)

    if threads <= 1:
        return np.vstack([search_one(q) for q in queries])

    with ThreadPoolExecutor(max_workers=threads) as pool:
        return np.vstack(list(pool.map(search_one, queries)))


def bench_vsag(train: np.ndarray, queries: np.ndarray, gt: np.ndarray):
    """VSAG/pyvsag HNSW 参数扫描。"""
    try:
        import pyvsag
    except ImportError:
        print("[跳过] pyvsag 未安装 (pip install pyvsag)")
        return

    n_train = train.shape[0]
    train_norm = normalize_rows(train).astype(np.float32, copy=False)
    queries_norm = normalize_rows(queries).astype(np.float32, copy=False)
    ids = np.arange(n_train, dtype=np.int64)

    print("\n" + "=" * 70)
    print("Baseline: VSAG (pyvsag HNSW)")
    print("=" * 70)

    for ef_c in VSAG_EF_CONSTRUCTION_VALUES:
        for max_degree in VSAG_MAX_DEGREE_VALUES:
            print(f"\n--- VSAG HNSW max_degree={max_degree}, ef_c={ef_c} ---")
            index_params = json.dumps({
                "dtype": "float32",
                "metric_type": "l2",
                "dim": DIM,
                "hnsw": {
                    "max_degree": max_degree,
                    "ef_construction": ef_c,
                },
            })
            index = pyvsag.Index("hnsw", index_params)

            try:
                t0 = time.perf_counter()
                index.build(vectors=train_norm, ids=ids, num_elements=n_train, dim=DIM)
                build_time = time.perf_counter() - t0
                vecs_per_sec = n_train / build_time
                _, mem_str = estimate_hnsw_memory(n_train, DIM, max_degree, stores_vectors=True)
                print(f"构建: {build_time:.1f}s ({vecs_per_sec:,.0f} vecs/s)")
                print(f"内存: {mem_str}")

                header = f"{'ef':>6} {'R@10':>9} {'1T-QPS':>10} {'MT-QPS':>10} {'lat(ms)':>9} {'1T/BF':>8} {'MT/BF':>8}"
                print(header)
                print("-" * len(header))

                for ef in VSAG_EF_SEARCH_VALUES:
                    labels_1t, qps_1t, lat_ms = measure_qps(
                        lambda q, _ef=ef: _vsag_batch_search(index, q, _ef, 1), queries_norm
                    )
                    recall = compute_recall(labels_1t, gt)

                    _, qps_mt, _ = measure_qps(
                        lambda q, _ef=ef: _vsag_batch_search(index, q, _ef, VSAG_BATCH_THREADS), queries_norm
                    )

                    sp_1t = qps_1t / BRUTE_FORCE_QPS if BRUTE_FORCE_QPS > 0 else 0
                    sp_mt = qps_mt / BRUTE_FORCE_QPS if BRUTE_FORCE_QPS > 0 else 0
                    print(f"{ef:>6} {recall*100:>8.2f}% {qps_1t:>10,.0f} {qps_mt:>10,.0f} {lat_ms:>9.2f} {sp_1t:>7.1f}x {sp_mt:>7.1f}x")

            except Exception as e:
                print(f"[错误] VSAG HNSW max_degree={max_degree}, ef_c={ef_c}: {e}")
                continue


def _vsag_hgraph_index_param(base_quantization: str, max_degree: int, ef_construction: int) -> Dict:
    """生成 VSAG HGraph 的 index_param。"""
    index_param = {
        "base_quantization_type": base_quantization,
        "max_degree": max_degree,
        "ef_construction": ef_construction,
        "alpha": 1.2,
    }
    if base_quantization == "sq8_reorder_fp32":
        index_param["base_quantization_type"] = "sq8"
        index_param["use_reorder"] = True
        index_param["precise_quantization_type"] = "fp32"
    return index_param


def bench_vsag_hgraph(train: np.ndarray, queries: np.ndarray, gt: np.ndarray):
    """VSAG/pyvsag HGraph 参数扫描。"""
    try:
        import pyvsag
    except ImportError:
        print("[跳过] pyvsag 未安装或当前平台无法加载 (pip install pyvsag)")
        return

    n_train = train.shape[0]
    train_norm = normalize_rows(train).astype(np.float32, copy=False)
    queries_norm = normalize_rows(queries).astype(np.float32, copy=False)
    ids = np.arange(n_train, dtype=np.int64)

    print("\n" + "=" * 70)
    print("Baseline: VSAG (pyvsag HGraph)")
    print("=" * 70)

    for base_quantization in VSAG_HGRAPH_BASE_QUANTIZATION_VALUES:
        for ef_c in VSAG_EF_CONSTRUCTION_VALUES:
            for max_degree in VSAG_MAX_DEGREE_VALUES:
                print(f"\n--- VSAG HGraph quant={base_quantization}, max_degree={max_degree}, ef_c={ef_c} ---")
                index_params = json.dumps({
                    "dtype": "float32",
                    "metric_type": "l2",
                    "dim": DIM,
                    "index_param": _vsag_hgraph_index_param(base_quantization, max_degree, ef_c),
                })
                index = pyvsag.Index("hgraph", index_params)

                try:
                    t0 = time.perf_counter()
                    index.build(vectors=train_norm, ids=ids, num_elements=n_train, dim=DIM)
                    build_time = time.perf_counter() - t0
                    vecs_per_sec = n_train / build_time
                    _, mem_str = estimate_hnsw_memory(n_train, DIM, max_degree, stores_vectors=True)
                    print(f"构建: {build_time:.1f}s ({vecs_per_sec:,.0f} vecs/s)")
                    print(f"内存: {mem_str}")

                    header = f"{'ef':>6} {'R@10':>9} {'1T-QPS':>10} {'MT-QPS':>10} {'lat(ms)':>9} {'1T/BF':>8} {'MT/BF':>8}"
                    print(header)
                    print("-" * len(header))

                    for ef in VSAG_EF_SEARCH_VALUES:
                        labels_1t, qps_1t, lat_ms = measure_qps(
                            lambda q, _ef=ef: _vsag_batch_search(index, q, _ef, 1, "hgraph"), queries_norm
                        )
                        recall = compute_recall(labels_1t, gt)

                        _, qps_mt, _ = measure_qps(
                            lambda q, _ef=ef: _vsag_batch_search(index, q, _ef, VSAG_BATCH_THREADS, "hgraph"), queries_norm
                        )

                        sp_1t = qps_1t / BRUTE_FORCE_QPS if BRUTE_FORCE_QPS > 0 else 0
                        sp_mt = qps_mt / BRUTE_FORCE_QPS if BRUTE_FORCE_QPS > 0 else 0
                        print(f"{ef:>6} {recall*100:>8.2f}% {qps_1t:>10,.0f} {qps_mt:>10,.0f} {lat_ms:>9.2f} {sp_1t:>7.1f}x {sp_mt:>7.1f}x")

                except Exception as e:
                    print(f"[错误] VSAG HGraph quant={base_quantization}, max_degree={max_degree}, ef_c={ef_c}: {e}")
                    continue


# ============================================================
#  主入口
# ============================================================

ALL_BASELINES = {
    "hnswlib": bench_hnswlib,
    "faiss_hnsw": bench_faiss_hnsw,
    "faiss_ivfpq": bench_faiss_ivfpq,
    "usearch": bench_usearch,
    "vsag": bench_vsag,
    "vsag_hgraph": bench_vsag_hgraph,
}

def main():
    global BRUTE_FORCE_QPS
    train, queries, gt = load_data()

    # 先测 brute-force baseline
    print("\n" + "=" * 70)
    print("Brute-Force Baseline (numpy exact search)")
    print("=" * 70)
    BRUTE_FORCE_QPS = measure_brute_force(train, queries, gt)

    # 选择要跑的 baseline
    requested = os.environ.get("BASELINES", "").strip()
    start_from = os.environ.get("BASELINE_START", "").strip()

    if requested:
        names = [n.strip() for n in requested.split(",")]
    else:
        names = list(ALL_BASELINES.keys())

    # 跳过支持
    if start_from:
        try:
            idx = names.index(start_from)
            names = names[idx:]
            print(f"从 {start_from} 开始，跳过之前的 baseline")
        except ValueError:
            print(f"[警告] BASELINE_START={start_from} 不在列表中，忽略")

    print(f"\n将运行: {', '.join(names)}")
    print(f"硬件: {os.cpu_count()} 核")

    for name in names:
        fn = ALL_BASELINES.get(name)
        if fn is None:
            print(f"[警告] 未知 baseline: {name}，跳过")
            continue
        fn(train, queries, gt)

    print("\n" + "=" * 70)
    print("全部完成！")
    print("=" * 70)


if __name__ == "__main__":
    main()
