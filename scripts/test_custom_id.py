import triviumdb
import os

DB_PATH = "test_custom_id.tdb"

# 先清理可能残留的文件
for f in [DB_PATH, DB_PATH + ".wal"]:
    if os.path.exists(f):
        os.remove(f)

# 打开数据库
db = triviumdb.TriviumDB(DB_PATH, dim=2)

try:
    print("=== Testing Single insert_with_id ===")
    
    # PEDSA 的 id 可能是特定数值，比如 1001, 1002
    db.insert_with_id(1001, [0.1, 0.2], {"source": "PEDSA", "type": "Memory"})
    print("Inserted custom ID = 1001")
    
    node = db.get(1001)
    assert node, "Node not found!"
    assert node.id == 1001
    print(f"Retrieved: ID={node.id}, payload={node.payload}")
    
    # 测试防止覆盖
    try:
        db.insert_with_id(1001, [0.0, 0.0], {"test": "override"})
        print("FAIL: Should not allow duplicate ID")
    except Exception as e:
        print(f"Duplicate ID blocked successfully: {e}")

    print("\n=== Testing batch_insert_with_ids ===")
    
    ids = [2001, 2002, 2003]
    vectors = [[1.0, 1.0], [2.0, 2.0], [3.0, 3.0]]
    payloads = [
        {"name": "Feature 1"},
        {"name": "Feature 2"},
        {"name": "Feature 3"}
    ]
    
    db.batch_insert_with_ids(ids, vectors, payloads)
    print(f"Batch inserted IDs: {ids}")
    
    assert db.get(2002).payload["name"] == "Feature 2"
    
    # 测试自增分配器推进
    # 我们刚刚插入的最大 ID 是 2003，下一个普通的 insert 应该自增从 2004 开始
    auto_id = db.insert([0.5, 0.5], {"source": "Auto"})
    print(f"Next auto ID: {auto_id}")
    assert auto_id == 2004, f"Expected 2004, got {auto_id}"

    db.flush()
    print("\n✅ All Custom ID Tests Passed!")

finally:
    del db
    for f in [DB_PATH, DB_PATH + ".wal"]:
        if os.path.exists(f):
            os.remove(f)
