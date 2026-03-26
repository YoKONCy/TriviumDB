"""TriviumDB 图谱查询语言（类 Cypher）完整测试"""
import triviumdb
import os

DB_PATH = "test_query.tdb"

db = triviumdb.TriviumDB(DB_PATH, dim=3)

# 建立一个小型社交图谱
alice = db.insert([1.0, 0.0, 0.0], {"name": "Alice", "age": 25, "role": "admin"})
bob   = db.insert([0.0, 1.0, 0.0], {"name": "Bob", "age": 30, "role": "user"})
carol = db.insert([0.0, 0.0, 1.0], {"name": "Carol", "age": 22, "role": "admin"})
dave  = db.insert([0.5, 0.5, 0.0], {"name": "Dave", "age": 35, "role": "mod"})
eve   = db.insert([0.0, 0.5, 0.5], {"name": "Eve", "age": 28, "role": "user"})

db.link(alice, bob, label="knows", weight=0.9)
db.link(alice, carol, label="knows", weight=0.8)
db.link(bob, carol, label="knows", weight=0.7)
db.link(bob, dave, label="works_with", weight=0.6)
db.link(carol, eve, label="knows", weight=0.9)
db.link(dave, eve, label="manages", weight=1.0)

print("=== TriviumDB Query Language Test ===\n")
print(f"Graph: {len(db)} nodes, Alice={alice} Bob={bob} Carol={carol} Dave={dave} Eve={eve}\n")

# ═══════ Test 1: 基础路径匹配 ═══════
print("--- Test 1: MATCH (a)-[:knows]->(b) RETURN b ---")
results = db.query("MATCH (a)-[:knows]->(b) RETURN b")
names = [r["b"]["payload"]["name"] for r in results]
print(f"  All 'knows' targets: {sorted(names)}")
assert sorted(names) == ["Bob", "Carol", "Carol", "Eve"], f"FAIL: {names}"

# ═══════ Test 2: WHERE 过滤 ═══════
print("\n--- Test 2: WHERE b.age > 25 ---")
results = db.query("MATCH (a)-[:knows]->(b) WHERE b.age > 25 RETURN b")
names = [r["b"]["payload"]["name"] for r in results]
print(f"  knows targets with age>25: {sorted(names)}")
assert "Bob" in names

# ═══════ Test 3: 两跳链路 ═══════
print("\n--- Test 3: Two-hop path ---")
results = db.query("MATCH (a)-[:knows]->(b)-[:knows]->(c) RETURN a, c")
pairs = [(r["a"]["payload"]["name"], r["c"]["payload"]["name"]) for r in results]
print(f"  Two-hop knows chains: {pairs}")

# ═══════ Test 4: 带 WHERE 的两跳 ═══════
print("\n--- Test 4: Two-hop + WHERE ---")
results = db.query("MATCH (a)-[:knows]->(b)-[:knows]->(c) WHERE c.age < 25 RETURN a, b, c")
for r in results:
    a_name = r["a"]["payload"]["name"]
    b_name = r["b"]["payload"]["name"]
    c_name = r["c"]["payload"]["name"]
    c_age  = r["c"]["payload"]["age"]
    print(f"  {a_name} -knows-> {b_name} -knows-> {c_name} (age={c_age})")

# ═══════ Test 5: 不同边标签 ═══════
print("\n--- Test 5: Mixed edge labels ---")
results = db.query("MATCH (a)-[:works_with]->(b) RETURN a, b")
for r in results:
    print(f"  {r['a']['payload']['name']} -works_with-> {r['b']['payload']['name']}")

# ═══════ Test 6: 通配边（不限标签）═══════
print("\n--- Test 6: Wildcard edges (no label filter) ---")
results = db.query("MATCH (a)-[]->(b) WHERE b.role == 'admin' RETURN a, b")
for r in results:
    print(f"  {r['a']['payload']['name']} --> {r['b']['payload']['name']} (admin)")

# ═══════ Test 7: AND 条件 ═══════
print("\n--- Test 7: AND condition ---")
results = db.query("MATCH (a)-[:knows]->(b) WHERE b.age > 20 AND b.role == 'admin' RETURN b")
names = [r["b"]["payload"]["name"] for r in results]
print(f"  knows + age>20 AND admin: {names}")
assert "Carol" in names

# Cleanup
del db
for f in [DB_PATH, DB_PATH + ".wal"]:
    if os.path.exists(f):
        os.remove(f)

print("\n=== All Query Language tests passed! ===")
