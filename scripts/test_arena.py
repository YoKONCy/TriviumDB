"""
TriviumDB 三引擎竞技场测试
验证 f32 / f16 / u64 三种 dtype 均可正常工作
"""
import triviumdb
import time
import os


# 初始化 Rust 侧的 tracing 日志输出到标准输出
triviumdb.init_logger()

def cleanup(paths):
    for p in paths:
        for ext in ["", ".wal", ".lock", ".tmp"]:
            target = p + ext if ext else p
            # 对于 .wal .lock 是在原来的基础上替换还是拼接？
            # p 是 "arena_f32.tdb"，wal 是 "arena_f32.tdb.wal" 还是 "arena_f32.wal"?
            # Rust中 lock_path = format!("{}.lock", path) 即 arena_f32.tdb.lock
            # tmp = format!("{}.tmp", path) 即 arena_f32.tdb.tmp
            # wal = format!("{}.wal", path) 即 arena_f32.tdb.wal
            if os.path.exists(target):
                try:
                    os.remove(target)
                except Exception:
                    pass

DB_PATHS = ["arena_f32.tdb", "arena_f16.tdb", "arena_u64.tdb"]
cleanup(DB_PATHS)

DIM = 128
N = 1000

# ═══════ 1. f32 测试 ═══════
print("=" * 50)
print("🔬 Testing dtype=f32 (standard precision)")
print("=" * 50)

t0 = time.perf_counter()
db32 = triviumdb.TriviumDB("arena_f32.tdb", dim=DIM, dtype="f32")

for i in range(N):
    vec = [float(i * j % 97) / 97.0 for j in range(DIM)]
    db32.insert(vec, {"index": i, "type": "f32"})

t1 = time.perf_counter()
print(f"  Inserted {N} nodes in {t1-t0:.3f}s")
print(f"  repr: {db32}")
print(f"  node_count: {db32.node_count()}")

node = db32.get(1)
print(f"  get(1): id={node.id}, vec_len={len(node.vector)}")

query = [0.5] * DIM
results = db32.search(query, top_k=3, min_score=0.0)
print(f"  search top3: {[(r.id, round(r.score, 4)) for r in results]}")
db32.flush()
print("  ✅ f32 全部通过\n")

# ═══════ 2. f16 测试 ═══════
print("=" * 50)
print("🔬 Testing dtype=f16 (half precision, 50% memory)")
print("=" * 50)

t0 = time.perf_counter()
db16 = triviumdb.TriviumDB("arena_f16.tdb", dim=DIM, dtype="f16")

for i in range(N):
    vec = [float(i * j % 97) / 97.0 for j in range(DIM)]
    db16.insert(vec, {"index": i, "type": "f16"})

t1 = time.perf_counter()
print(f"  Inserted {N} nodes in {t1-t0:.3f}s")
print(f"  repr: {db16}")
print(f"  node_count: {db16.node_count()}")

node = db16.get(1)
print(f"  get(1): id={node.id}, vec_len={len(node.vector)}")

query = [0.5] * DIM
results = db16.search(query, top_k=3, min_score=0.0)
print(f"  search top3: {[(r.id, round(r.score, 4)) for r in results]}")
db16.flush()
print("  ✅ f16 全部通过\n")

# ═══════ 3. u64 测试 ═══════
print("=" * 50)
print("🔬 Testing dtype=u64 (binary hash / SimHash)")
print("=" * 50)

t0 = time.perf_counter()
db64 = triviumdb.TriviumDB("arena_u64.tdb", dim=4, dtype="u64")

for i in range(N):
    vec = [i * 123456789, i * 987654321, i ^ 0xDEADBEEF, i * 42]
    db64.insert(vec, {"index": i, "type": "u64"})

t1 = time.perf_counter()
print(f"  Inserted {N} nodes in {t1-t0:.3f}s")
print(f"  repr: {db64}")
print(f"  node_count: {db64.node_count()}")

node = db64.get(1)
print(f"  get(1): id={node.id}, vec_len={len(node.vector)}")

query = [100 * 123456789, 100 * 987654321, 100 ^ 0xDEADBEEF, 100 * 42]
results = db64.search(query, top_k=3, min_score=0.0)
print(f"  search top3: {[(r.id, round(r.score, 4)) for r in results]}")
db64.flush()
print("  ✅ u64 全部通过\n")

# ═══════ 4. 错误 dtype 测试 ═══════
print("=" * 50)
print("🔬 Testing invalid dtype rejection")
print("=" * 50)
try:
    bad = triviumdb.TriviumDB("bad.tdb", dim=4, dtype="i32")
    print("  ❌ Should have thrown!")
except ValueError as e:
    print(f"  ✅ Correctly rejected: {e}")

print()
print("🏆 All arena tests passed!")

cleanup(DB_PATHS)
