"""
hnswlib 基准测试：Cohere-1M (768-d, cosine)
用于论文基线对比
"""
import numpy as np
import hnswlib
import time
import os

# === 配置 ===
DIM = 768
NUM_BASE = 1_000_000
NUM_QUERIES = 1_000
K = 10
DATA_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# hnswlib 参数（与 USearch 对齐）
M = 16
EF_CONSTRUCTION = 200
NUM_THREADS = 8  # 构建用多线程

# 搜索时测试的 ef 值
EF_SEARCH_VALUES = [32, 64, 128, 256]

print("=" * 60)
print("hnswlib 基准测试：Cohere-1M (768-d)")
print("=" * 60)

# === 加载数据 ===
print("\n[1/4] 加载数据...")
train_path = os.path.join(DATA_DIR, "cohere_train.f32")
test_path = os.path.join(DATA_DIR, "cohere_test.f32")
gt_path = os.path.join(DATA_DIR, "cohere_groundtruth.i32")

train = np.fromfile(train_path, dtype=np.float32).reshape(NUM_BASE, DIM)
queries = np.fromfile(test_path, dtype=np.float32).reshape(NUM_QUERIES, DIM)
groundtruth = np.fromfile(gt_path, dtype=np.int32).reshape(NUM_QUERIES, -1)[:, :K]
print(f"  训练集: {train.shape}, 查询集: {queries.shape}, GT: {groundtruth.shape}")

# === 构建索引 ===
print(f"\n[2/4] 构建 hnswlib 索引 (M={M}, ef_c={EF_CONSTRUCTION}, threads={NUM_THREADS})...")
index = hnswlib.Index(space='cosine', dim=DIM)
index.init_index(max_elements=NUM_BASE, ef_construction=EF_CONSTRUCTION, M=M)
index.set_num_threads(NUM_THREADS)

t0 = time.perf_counter()
index.add_items(train, np.arange(NUM_BASE))
build_time = time.perf_counter() - t0
print(f"  构建时间: {build_time:.2f}s")

# === 搜索测试 ===
print(f"\n[3/4] 搜索测试 (K={K})...")
print(f"{'ef':>6} | {'Recall@10':>10} | {'QPS':>10} | {'Latency(ms)':>12}")
print("-" * 50)

results = []
for ef in EF_SEARCH_VALUES:
    index.set_ef(ef)
    index.set_num_threads(1)  # 单线程搜索，与 QuIVer 对齐
    
    # 预热
    _ = index.knn_query(queries[:10], k=K)
    
    # 正式测试
    t0 = time.perf_counter()
    labels, distances = index.knn_query(queries, k=K)
    search_time = time.perf_counter() - t0
    
    # 计算 Recall@10
    recall = 0.0
    for i in range(NUM_QUERIES):
        gt_set = set(groundtruth[i].tolist())
        pred_set = set(labels[i].tolist())
        recall += len(gt_set & pred_set) / K
    recall = recall / NUM_QUERIES * 100
    
    qps = NUM_QUERIES / search_time
    latency = search_time / NUM_QUERIES * 1000
    
    print(f"{ef:>6} | {recall:>9.2f}% | {qps:>10,.0f} | {latency:>11.3f}")
    results.append((ef, recall, qps, build_time))

# === 汇总 ===
print(f"\n[4/4] 汇总")
print("=" * 60)
print(f"构建时间: {build_time:.2f}s")
print(f"M={M}, ef_construction={EF_CONSTRUCTION}")
print()
print("可直接用于论文的 LaTeX 表格行：")
print()
for ef, recall, qps, bt in results:
    print(f"hnswlib & ef={ef} & {bt:.0f} & {recall:.2f}\\% & {qps:,.0f} \\\\")
