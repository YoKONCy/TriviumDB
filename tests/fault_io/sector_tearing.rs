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
    crate::cleanup_files::database(path);
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
    // 保留有效检查点标记与 Payload 文件，只破坏主文件。
    {
        let db = Database::<f32>::open_read_only(&path, DIM).unwrap();
        assert_eq!(db.node_count().unwrap(), 100);
        let last = db.get(100).unwrap().unwrap();
        assert_eq!(last.vector, [9.0, 9.0, 0.0, 0.0]);
        assert_eq!(last.payload, serde_json::json!({"batch": 9, "val": 9}));
    }

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

    let error = match Database::<f32>::open_read_only(&path, DIM) {
        Ok(_) => panic!("主文件截断后应拒绝打开"),
        Err(error) => error,
    };
    assert_eq!(error.code(), "TDB_CORRUPTED_FILE", "{error}");

    cleanup(&path);
}
