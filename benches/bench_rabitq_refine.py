"""
==========================================
FAISS IVF+RaBitQfs+Refine(SQ8) Benchmark
==========================================
RaBitQfs = FastScan (SIMD优化) + SQ8 Refine (高recall)
这是 RaBitQ 的最强配置：快速粗筛 + 精确重排。
"""
import numpy as np
import time
import os
import faiss

def p(msg=""):
    print(msg, flush=True)

DIM = 768
K = 10
PROJECT_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TRAIN_PATH = os.path.join(PROJECT_DIR, "cohere_train.f32")
TEST_PATH = os.path.join(PROJECT_DIR, "cohere_test.f32")
GT_PATH = os.path.join(PROJECT_DIR, "cohere_groundtruth.i32")
N_THREADS = os.cpu_count() or 8

def load_data():
    p("加载数据...")
    train = np.fromfile(TRAIN_PATH, dtype=np.float32).reshape(-1, DIM)
    queries = np.fromfile(TEST_PATH, dtype=np.float32).reshape(-1, DIM)
    gt_raw = np.fromfile(GT_PATH, dtype=np.int32)
    n_queries = queries.shape[0]
    k_gt = gt_raw.shape[0] // n_queries
    gt = gt_raw.reshape(n_queries, k_gt)[:, :K]
    faiss.normalize_L2(train)
    faiss.normalize_L2(queries)
    p(f"  训练: {train.shape}, 查询: {queries.shape}, GT: {gt.shape}")
    return train, queries, gt

def compute_recall(pred, gt):
    n = gt.shape[0]
    hits = 0
    for i in range(n):
        hits += len(set(pred[i].tolist()) & set(gt[i].tolist()))
    return hits / (n * K)

def measure_qps(search_fn, queries, warmup=5):
    search_fn(queries[:warmup])
    t0 = time.perf_counter()
    labels = search_fn(queries)
    elapsed = time.perf_counter() - t0
    return labels, len(queries)/elapsed, elapsed/len(queries)*1e6

def main():
    train, queries, gt = load_data()
    p(f"硬件: {N_THREADS} 核, FAISS {faiss.__version__}\n")

    configs = [
        ("IVF1024,RaBitQfs,Refine(SQ8)", [16,32,64,128,256], [1,2,4,10,20]),
        ("IVF4096,RaBitQfs,Refine(SQ8)", [32,64,128,256,512], [1,2,4,10,20]),
        ("IVF1024,RaBitQfs,Refine(Flat)", [16,32,64,128], [1,2,4,10]),
    ]

    for factory_str, nprobe_values, k_factor_values in configs:
        p("=" * 65)
        p(f"  {factory_str}")
        p("=" * 65)

        try:
            idx = faiss.index_factory(DIM, factory_str, faiss.METRIC_INNER_PRODUCT)
            t0 = time.perf_counter()
            idx.train(train)
            idx.add(train)
            build_time = time.perf_counter() - t0
            p(f"构建: {build_time:.1f}s")

            header = f"{'nprobe':>8} {'k_fac':>6} {'R@10':>9} {'1T-QPS':>10} {'MT-QPS':>10} {'1T-lat(us)':>12}"
            p(header)
            p("-" * len(header))

            for nprobe in nprobe_values:
                faiss.ParameterSpace().set_index_parameter(idx, "nprobe", nprobe)
                for k_factor in k_factor_values:
                    idx.k_factor = k_factor

                    faiss.omp_set_num_threads(1)
                    labels_1t, qps_1t, lat_us = measure_qps(lambda q: idx.search(q, K)[1], queries)
                    recall = compute_recall(labels_1t, gt)

                    faiss.omp_set_num_threads(N_THREADS)
                    _, qps_mt, _ = measure_qps(lambda q: idx.search(q, K)[1], queries)

                    p(f"{nprobe:>8} {k_factor:>6} {recall*100:>8.2f}% {qps_1t:>10,.0f} {qps_mt:>10,.0f} {lat_us:>12,.1f}")

            del idx
        except Exception as e:
            p(f"[错误] {factory_str}: {e}")
            import traceback; traceback.print_exc()

    p("\n全部完成!")

if __name__ == "__main__":
    main()
