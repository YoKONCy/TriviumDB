#![allow(non_snake_case)]
//! 跨平台字节序兼容 (Cross-Endian) 与 32 位寻址边界模拟
//! 对标 SQLite 在跨平台/端序异常时安全降级而不是 Panic

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::panic::catch_unwind;
use triviumdb::Database;
use triviumdb::database::Config;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("endian_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".tdb", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

#[test]
fn 测试_人为端序颠倒污染_魔数与长度域应有效拦截不崩溃() {
    let path = tmp_db("endianness");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"test": true}));
        tx.commit().unwrap();
        db.flush().unwrap();
    }

    // 删除 WAL 和 flush_ok，隔离变量，只保留被篡改的 .tdb
    std::fs::remove_file(format!("{}.wal", path)).ok();
    std::fs::remove_file(format!("{}.flush_ok", path)).ok();

    // 打开写入的主数据文件，手动将文件头部的 u32/u64 元数据反转
    let tdb_path = path.clone();
    if let Ok(mut file) = OpenOptions::new().read(true).write(true).open(&tdb_path) {
        let mut buffer = [0u8; 16];
        if file.read(&mut buffer).is_ok() {
            // 将前四个字节粗暴反转
            buffer[0..4].reverse();
            let _ = file.seek(SeekFrom::Start(0));
            let _ = file.write_all(&buffer);
        }
    }

    // 核心断言：绝不能因为读取被篡改的文件而发生 Segfault 或 Panic
    let result = catch_unwind(|| Database::<f32>::open(&path, DIM));

    assert!(
        result.is_ok(),
        "引擎读取了端序颠倒的文件，触发了极危 Panic 崩溃！"
    );
    // 文件头 magic 被篡改且无 WAL/flush_ok 灾备：引擎必须拒绝加载
    assert!(
        result.unwrap().is_err(),
        "端序颠倒导致 magic 失效后，引擎应拒绝加载，而不是静默接受损坏文件"
    );
    eprintln!("  ✅ 端序颠倒: 引擎正确拒绝加载被篡改的文件");

    cleanup(&path);
}

#[test]
fn 测试_恶意BQ计数溢出_应拒绝且不崩溃() {
    let path = tmp_db("bq_count_overflow");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"test": true}))
            .unwrap();
        db.flush().unwrap();
    }

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.seek(SeekFrom::Start(50)).unwrap();
    let mut offset_bytes = [0u8; 8];
    file.read_exact(&mut offset_bytes).unwrap();
    let bq_offset = u64::from_le_bytes(offset_bytes);
    assert!(bq_offset > 0, "测试文件必须包含 BQ 块");
    file.seek(SeekFrom::Start(bq_offset)).unwrap();
    file.write_all(&u64::MAX.to_le_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);

    let result = catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "恶意 BQ 计数不得触发 panic");
    assert!(result.unwrap().is_err(), "恶意 BQ 计数必须被明确拒绝");

    cleanup(&path);
}

#[test]
fn 测试_恶意BQ计数字段截断_应拒绝且不崩溃() {
    let path = tmp_db("bq_count_truncated");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"test": true}))
            .unwrap();
        db.flush().unwrap();
    }

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.seek(SeekFrom::Start(50)).unwrap();
    let mut offset_bytes = [0u8; 8];
    file.read_exact(&mut offset_bytes).unwrap();
    let bq_offset = u64::from_le_bytes(offset_bytes);
    file.set_len(bq_offset + 4).unwrap();
    file.sync_all().unwrap();
    drop(file);

    let result = catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "截断 BQ 计数字段不得触发 panic");
    assert!(result.unwrap().is_err(), "截断 BQ 计数字段必须被明确拒绝");

    cleanup(&path);
}

#[test]
fn 测试_超大维数模拟拦截_防平台指针溢出() {
    let path = tmp_db("huge_dim");
    cleanup(&path);

    // 模拟恶意输入一个导致 u64 会溢出或撑爆 32位 内存寻址极限的维度
    let huge_dim = 1 << 30; // 给定一个约 10 亿的向量维度

    // 不应 Panic 崩溃
    let result = catch_unwind(|| {
        let config = Config {
            dim: huge_dim,
            ..Default::default()
        };
        Database::<f32>::open_with_config(&path, config)
    });

    assert!(result.is_ok(), "加载非法超巨维度触发了 Panic 崩溃");
    // 引擎必须拒绝超巨维度
    let open_result = result.unwrap();
    match open_result {
        Ok(_) => panic!(
            "引擎接受了 {} 维的超巨向量维度！这将导致内存分配溢出或指针越界",
            huge_dim
        ),
        Err(e) => {
            let err_msg = e.to_string();
            assert!(
                err_msg.contains("exceeds maximum") || err_msg.contains("Invalid input"),
                "错误信息应明确指出维度越界，实际: {}",
                err_msg
            );
            eprintln!(
                "  ✅ 超大维度: 引擎正确拒绝 {} 维向量: {}",
                huge_dim, err_msg
            );
        }
    }

    cleanup(&path);
}
