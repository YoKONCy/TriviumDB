"""TriviumDB Pythonic API 完整演示"""
import triviumdb
import os

DB_PATH = "demo_pythonic.tdb"

# ══════ 1. Context Manager: with 语句自动 flush ══════
print("=== Context Manager ===")
with triviumdb.TriviumDB(DB_PATH, dim=3) as db:
    db.insert([1.0, 0.0, 0.0], {"name": "Alice", "age": 25, "role": "admin"})
    db.insert([0.0, 1.0, 0.0], {"name": "Bob", "age": 30, "role": "user"})
    db.insert([0.0, 0.0, 1.0], {"name": "Charlie", "age": 22, "role": "admin"})
    db.insert([0.5, 0.5, 0.0], {"name": "Diana", "age": 28, "role": "mod"})
    db.insert([0.0, 0.5, 0.5], {"name": "Eve", "age": 35, "role": "user"})
    print(f"  Created: {db}")  # __repr__
    # with 退出时自动 flush，不需要手动调 db.flush()

# ══════ 2. 重新打开验证 ══════
print("\n=== Reopen & Pythonic ops ===")
db = triviumdb.TriviumDB(DB_PATH, dim=3)
print(f"  {db}")
print(f"  len(db) = {len(db)}")      # __len__
print(f"  1 in db = {1 in db}")      # __contains__
print(f"  999 in db = {999 in db}")

# ══════ 3. Batch Insert: 批量插入 ══════
print("\n=== Batch Insert ===")
vectors = [[0.1, 0.2, 0.7], [0.3, 0.3, 0.4], [0.9, 0.05, 0.05]]
payloads = [
    {"name": "Frank", "age": 19, "role": "user"},
    {"name": "Grace", "age": 42, "role": "admin"},
    {"name": "Hank", "age": 27, "role": "mod"},
]
ids = db.batch_insert(vectors, payloads)
print(f"  Batch inserted {len(ids)} nodes: {ids}")
print(f"  Total now: {len(db)}")

# ══════ 4. filter_where: 高级过滤 ══════
print("\n=== filter_where ===")

# 简写精确匹配
admins = db.filter_where({"role": "admin"})
print(f"  role == 'admin': {[n.payload['name'] for n in admins]}")

# $gt 运算符
older = db.filter_where({"age": {"$gt": 25}})
names = [n.payload["name"] + "(" + str(n.payload["age"]) + ")" for n in older]
print(f"  age > 25: {names}")

# $in 运算符
mods_or_admins = db.filter_where({"role": {"$in": ["admin", "mod"]}})
print(f"  role in [admin, mod]: {[n.payload['name'] for n in mods_or_admins]}")

# $and 组合
young_admins = db.filter_where({
    "$and": [
        {"role": {"$eq": "admin"}},
        {"age": {"$lt": 30}}
    ]
})
print(f"  admin AND age<30: {[n.payload['name'] for n in young_admins]}")

# $or 组合
special = db.filter_where({
    "$or": [
        {"age": {"$lte": 20}},
        {"age": {"$gte": 40}}
    ]
})
print(f"  age<=20 OR age>=40: {[n.payload['name'] for n in special]}")

# ══════ 5. 混合检索 ══════
print("\n=== Hybrid Search ===")
results = db.search([0.9, 0.1, 0.0], top_k=3)
for r in results:
    print(f"  [ID:{r.id}] score={r.score:.4f} | {r.payload}")

# Cleanup
del db
os.remove(DB_PATH)
if os.path.exists(DB_PATH + ".wal"):
    os.remove(DB_PATH + ".wal")

print("\n=== All Pythonic API tests passed! ===")
