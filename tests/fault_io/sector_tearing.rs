#![allow(non_snake_case)]
//! 断电扇区撕裂 (Sector Tearing) 模拟测试对标 SQLite VFS 崩溃模拟验证
//! 检测引擎是否对底层 `.tdb` 发生半路截断、页残缺，拥有完备的拒绝与降级恢复能力

use std::fs::OpenOptions;
use triviumdb::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("tear_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok", ".tdb"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

#[test]
#[ignore = "故障注入测试由 fault-injection workflow 显式运行"]
fn 测试_深层页撕裂_模拟写入一半断电截断() {
    let path = tmp_db("sector_torn");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        // 灌入较多数据以拉涨 tdb 文件大小
        for i in 0..10 {
            let mut tx = db.begin_tx();
            for j in 0..10_usize {
                tx.insert(
                    &[i as f32, j as f32, 0.0, 0.0],
                    serde_json::json!({"batch": i, "val": j}),
                );
            }
            tx.commit().unwrap();
        }
        db.flush().unwrap();
    }
    // 删除 WAL 和 flush_ok，隔离变量，只保留被截断的主数据文件
    std::fs::remove_file(format!("{}.wal", path)).ok();
    std::fs::remove_file(format!("{}.flush_ok", path)).ok();

    // TriviumDB 的主数据文件就是 path 本身（不带 .tdb 后缀）
    let tdb_path = path.clone();
    // 强制截断文件到非 4KB 对齐或是正好少了一半的残页状态
    let meta = std::fs::metadata(&tdb_path).unwrap();
    let original_len = meta.len();
    assert!(
        original_len > 1024,
        "主数据文件应有一定体积，实际: {} bytes",
        original_len
    );

    let torn_len = original_len - 256; // 砍掉 256 字节，模拟只写入了部分数据断电

    let file = OpenOptions::new().write(true).open(&tdb_path).unwrap();
    file.set_len(torn_len).expect("系统截断文件长度失败");
    drop(file);

    // 重新打开：撕裂的由于缺少 flush_ok 或元数据校验报错，必须优雅拦截
    // 不能让 Mmap 去映射超出物理文件体积的心智虚假空间从而引发 Linux BusError 甚至导致进程 Segfault
    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));

    assert!(
        result.is_ok(),
        "引擎因为扇区撕裂触发了极危系统崩溃 (Segfault / Panic)！"
    );
    let load_res = result.unwrap();

    // 加载可能 Err(Corrupted)，或者是 fallback 清空
    if let Ok(db) = load_res {
        // 如果它强行加载了，那至少不能是原本的 100个 节点，或者它的节点数应为 0（降级重建）
        if db.node_count() == 100 {
            panic!(
                "引擎毫无察觉地加载了一个被残忍撕裂的文件！存在极大概率访问未初始化的 Mmap 页！"
            );
        }
    }

    cleanup(&path);
}
