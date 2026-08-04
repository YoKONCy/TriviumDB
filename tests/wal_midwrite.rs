#![allow(non_snake_case)]
//! WAL 逐字节截断 + 解析器模糊测试
//!
//! 覆盖此前测试体系中最大的安全盲区：
//!   1. WAL 帧写入中途断电（每个字节边界的截断）
//!   2. WAL 解析器面对任意垃圾字节的鲁棒性
//!   3. 事务边界的部分写入（TxBegin 写了但 TxCommit 未写）
//!
//! 与 hw_crash.rs 的区别：
//!   hw_crash.rs  — 在"数据已完整写入 WAL 之后"崩溃，验证回放
//!   wal_midwrite — 在"WAL 帧写到一半"时模拟断电，验证解析器的截断容错

use std::io::Cursor;
use triviumdb::Database;
use triviumdb::storage::wal::{Wal, WalEntry};

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("midwrite_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

/// 构造一个包含多条有效 WAL 记录的字节流
fn build_valid_wal_bytes() -> Vec<u8> {
    let entries: Vec<WalEntry<f32>> = vec![
        WalEntry::Insert {
            id: 1,
            vector: vec![1.0, 0.0, 0.0, 0.0],
            payload: r#"{"name":"alice"}"#.to_string(),
        },
        WalEntry::Insert {
            id: 2,
            vector: vec![0.0, 1.0, 0.0, 0.0],
            payload: r#"{"name":"bob"}"#.to_string(),
        },
        WalEntry::Link {
            src: 1,
            dst: 2,
            label: "knows".to_string(),
            weight: 1.0,
        },
        WalEntry::Insert {
            id: 3,
            vector: vec![0.0, 0.0, 1.0, 0.0],
            payload: r#"{"name":"charlie"}"#.to_string(),
        },
        WalEntry::UpdatePayload {
            id: 1,
            payload: r#"{"name":"alice","age":30}"#.to_string(),
        },
    ];

    let mut buf = Vec::new();
    for entry in &entries {
        let data = bincode::serialize(entry).unwrap();
        let crc = crc32fast::hash(&data);
        let len = data.len() as u32;
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(&data);
        buf.extend_from_slice(&crc.to_le_bytes());
    }
    buf
}

// ════════════════════════════════════════════════════════════════
//  测试 1: WAL 帧逐字节截断 — 覆盖每个断电时间点
// ════════════════════════════════════════════════════════════════

/// 将一个有效的 WAL 字节流在每个可能的字节偏移处截断，
/// 验证 `read_entries_from_reader` 在任意截断点都不 panic，
/// 且总是返回有效条目的前缀子集。
///
/// 这覆盖了 hw_crash.rs 未测试的最危险时间窗口：
///   - len 字段只写了 1/2/3 字节
///   - bincode data 写到一半
///   - CRC 字段只写了 1/2/3 字节
#[test]
fn WAL_逐字节截断_覆盖每个断电时间点() {
    let full_wal = build_valid_wal_bytes();
    let full_len = full_wal.len();

    // 完整 WAL 应该能解析出 5 条记录
    let (full_entries, _) = Wal::read_entries_from_reader::<f32>(Cursor::new(&full_wal)).unwrap();
    assert_eq!(
        full_entries.len(),
        5,
        "完整 WAL 应包含 5 条记录，实际 {}",
        full_entries.len()
    );

    // 在每个字节偏移处截断
    for cut_at in 0..full_len {
        let truncated = &full_wal[..cut_at];
        let result = std::panic::catch_unwind(|| {
            Wal::read_entries_from_reader::<f32>(Cursor::new(truncated))
        });

        assert!(
            result.is_ok(),
            "WAL 在偏移 {}/{} 处截断后 read_entries_from_reader panic 了！",
            cut_at,
            full_len
        );

        let (entries, _) = result.unwrap().unwrap();
        assert!(
            entries.len() <= 5,
            "截断在 {}/{} 处，条目数 {} 超过了原始 5 条",
            cut_at,
            full_len,
            entries.len()
        );

        // 核心断言：截断后恢复的条目数必须单调递增
        // （更长的前缀不可能比更短的前缀恢复出更少的条目）
        if cut_at > 0 {
            let prev_truncated = &full_wal[..cut_at - 1];
            let (prev_entries, _) =
                Wal::read_entries_from_reader::<f32>(Cursor::new(prev_truncated)).unwrap();
            assert!(
                entries.len() >= prev_entries.len(),
                "单调性违反：截断在 {} 处恢复 {} 条，但在 {} 处恢复了 {} 条",
                cut_at,
                entries.len(),
                cut_at - 1,
                prev_entries.len()
            );
        }
    }
    eprintln!(
        "  ✅ WAL 逐字节截断: 在 {}/{} 个断电点上均安全恢复，零 panic",
        full_len, full_len
    );
}

// ════════════════════════════════════════════════════════════════
//  测试 2: 事务边界部分写入 — TxBegin 写了但 TxCommit 未写
// ════════════════════════════════════════════════════════════════

/// 模拟事务写入中途断电：
///   - 先写 2 条独立 Insert（已提交）
///   - 再写一个事务：TxBegin + 3 条 Insert（但 TxCommit 被截掉）
///
/// 预期：只恢复前 2 条独立 Insert，事务中的 3 条 Insert 全部丢弃。
#[test]
fn WAL_事务边界截断_未提交事务必须被丢弃() {
    let mut buf = Vec::new();

    // 2 条独立 Insert（无事务包裹）
    for id in 1..=2u64 {
        let entry = WalEntry::Insert::<f32> {
            id,
            vector: vec![id as f32, 0.0, 0.0, 0.0],
            payload: format!(r#"{{"id":{}}}"#, id),
        };
        let data = bincode::serialize(&entry).unwrap();
        let crc = crc32fast::hash(&data);
        buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&data);
        buf.extend_from_slice(&crc.to_le_bytes());
    }

    let committed_boundary = buf.len(); // 前 2 条独立 Insert 的边界

    // 事务：TxBegin + 3 条 Insert + TxCommit
    let tx_entries: Vec<WalEntry<f32>> = vec![
        WalEntry::TxBegin { tx_id: 42 },
        WalEntry::Insert {
            id: 3,
            vector: vec![3.0, 0.0, 0.0, 0.0],
            payload: r#"{"id":3}"#.to_string(),
        },
        WalEntry::Insert {
            id: 4,
            vector: vec![4.0, 0.0, 0.0, 0.0],
            payload: r#"{"id":4}"#.to_string(),
        },
        WalEntry::Insert {
            id: 5,
            vector: vec![5.0, 0.0, 0.0, 0.0],
            payload: r#"{"id":5}"#.to_string(),
        },
        WalEntry::TxCommit { tx_id: 42 },
    ];

    for entry in &tx_entries {
        let data = bincode::serialize(entry).unwrap();
        let crc = crc32fast::hash(&data);
        buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&data);
        buf.extend_from_slice(&crc.to_le_bytes());
    }

    let full_len = buf.len();

    // 完整 WAL：应恢复 2 + 3 = 5 条记录
    let (full, _) = Wal::read_entries_from_reader::<f32>(Cursor::new(&buf)).unwrap();
    assert_eq!(full.len(), 5, "完整事务应恢复 5 条记录");

    // 在事务区域的每个字节处截断（跳过 TxCommit）
    for cut_at in (committed_boundary + 1)..full_len {
        let truncated = &buf[..cut_at];
        let (entries, _) = Wal::read_entries_from_reader::<f32>(Cursor::new(truncated)).unwrap();

        // 核心断言：未提交事务中的条目绝不能出现在恢复结果中
        // 只有前 2 条独立 Insert 应被恢复，或者如果 TxCommit 恰好完整则恢复 5 条
        assert!(
            entries.len() == 2 || entries.len() == 5,
            "截断在 {}/{} 处（事务区域内），恢复了 {} 条记录。\
             应该只有 2（丢弃未提交事务）或 5（事务完整提交）",
            cut_at,
            full_len,
            entries.len()
        );
    }

    eprintln!(
        "  ✅ 事务边界截断: 在事务区域 {} 个断电点上，未提交事务均被正确丢弃",
        full_len - committed_boundary - 1
    );
}

// ════════════════════════════════════════════════════════════════
//  测试 3: 单帧内每个字段边界的截断
// ════════════════════════════════════════════════════════════════

/// 精确测试单条 WAL 帧内的 3 个关键截断点：
///   - len 字段内部（前 1/2/3 字节）
///   - data 字段内部（bincode 写到一半）
///   - CRC 字段内部（1/2/3 字节）
#[test]
fn WAL_单帧字段边界截断_len_data_crc各字段() {
    let entry = WalEntry::Insert::<f32> {
        id: 1,
        vector: vec![1.0, 2.0, 3.0, 4.0],
        payload: r#"{"key":"value","nested":{"a":1}}"#.to_string(),
    };
    let data = bincode::serialize(&entry).unwrap();
    let crc = crc32fast::hash(&data);
    let len = data.len() as u32;

    let mut frame = Vec::new();
    frame.extend_from_slice(&len.to_le_bytes()); // 4 bytes: len
    frame.extend_from_slice(&data); // N bytes: data
    frame.extend_from_slice(&crc.to_le_bytes()); // 4 bytes: crc

    let total = frame.len();
    let data_start = 4;
    let crc_start = 4 + data.len();

    eprintln!(
        "  📊 帧结构: total={} bytes, len=[0..4), data=[4..{}), crc=[{}..{})",
        total, crc_start, crc_start, total
    );

    // 在 len 字段内部截断 (偏移 1, 2, 3)
    for cut in 1..4 {
        let truncated = &frame[..cut];
        let (entries, _) = Wal::read_entries_from_reader::<f32>(Cursor::new(truncated)).unwrap();
        assert_eq!(
            entries.len(),
            0,
            "len 字段内截断(offset={})应返回 0 条记录",
            cut
        );
    }

    // 在 data 字段内部截断
    for cut in (data_start + 1)..crc_start {
        let truncated = &frame[..cut];
        let (entries, _) = Wal::read_entries_from_reader::<f32>(Cursor::new(truncated)).unwrap();
        assert_eq!(
            entries.len(),
            0,
            "data 字段内截断(offset={})应返回 0 条记录",
            cut
        );
    }

    // 在 CRC 字段内部截断
    for cut in (crc_start + 1)..total {
        let truncated = &frame[..cut];
        let (entries, _) = Wal::read_entries_from_reader::<f32>(Cursor::new(truncated)).unwrap();
        assert_eq!(
            entries.len(),
            0,
            "CRC 字段内截断(offset={})应返回 0 条记录",
            cut
        );
    }

    // 完整帧应恢复 1 条
    let (entries, _) = Wal::read_entries_from_reader::<f32>(Cursor::new(&frame)).unwrap();
    assert_eq!(entries.len(), 1, "完整帧应恢复 1 条记录");

    eprintln!(
        "  ✅ 单帧字段边界: len/data/crc 每个截断点均安全（共 {} 个点）",
        total - 1
    );
}

// ════════════════════════════════════════════════════════════════
//  测试 4: WAL 解析器对随机垃圾字节的鲁棒性
// ════════════════════════════════════════════════════════════════

/// 向 read_entries_from_reader 灌入大量随机字节，
/// 验证解析器在面对任意输入时绝不 panic。
#[test]
fn WAL_解析器_10000轮随机字节绝不panic() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let rounds = 10_000;
    let mut panic_count = 0;

    for seed in 0..rounds {
        // 简单确定性 PRNG（避免引入 rand 依赖）
        let mut hasher = DefaultHasher::new();
        seed.hash(&mut hasher);
        let h = hasher.finish();

        // 生成 0~1024 字节的随机数据
        let len = (h % 1024) as usize;
        let garbage: Vec<u8> = (0..len)
            .map(|i| {
                let mut h2 = DefaultHasher::new();
                (seed, i).hash(&mut h2);
                h2.finish() as u8
            })
            .collect();

        let result = std::panic::catch_unwind(|| {
            Wal::read_entries_from_reader::<f32>(Cursor::new(&garbage))
        });

        if result.is_err() {
            panic_count += 1;
            eprintln!(
                "  ❌ Panic at seed={}, len={}, bytes={:?}",
                seed,
                len,
                &garbage[..garbage.len().min(32)]
            );
        }
    }

    assert_eq!(
        panic_count, 0,
        "{}/{} 轮随机字节触发了 WAL 解析器 panic！",
        panic_count, rounds
    );
    eprintln!(
        "  ✅ WAL 解析器: {}/{} 轮随机字节，零 panic",
        rounds, rounds
    );
}

// ════════════════════════════════════════════════════════════════
//  测试 5: 有效前缀 + 垃圾尾部 — 验证前缀恢复
// ════════════════════════════════════════════════════════════════

/// 在有效的 WAL 字节流后追加各种垃圾尾部，
/// 验证引擎总是能恢复有效前缀的所有条目。
#[test]
fn WAL_有效前缀加垃圾尾部_前缀条目必须完整恢复() {
    let valid_wal = build_valid_wal_bytes();
    let (baseline, _) = Wal::read_entries_from_reader::<f32>(Cursor::new(&valid_wal)).unwrap();
    let baseline_count = baseline.len();

    // 各种垃圾尾部 pattern
    let garbage_patterns: Vec<Vec<u8>> = vec![
        vec![0xFF; 64],                                     // 全 FF
        vec![0x00; 64],                                     // 全零
        vec![0xDE, 0xAD, 0xBE, 0xEF],                       // 4 字节（恰好是一个 len 字段）
        b"INVALID WAL ENTRY GARBAGE DATA HERE!!!".to_vec(), // ASCII 垃圾
        {
            // 一个 len 字段指向超大值
            let mut v = Vec::new();
            v.extend_from_slice(&(0xFFFFFFFFu32).to_le_bytes());
            v
        },
        {
            // 一个合理的 len 但后面数据不足
            let mut v = Vec::new();
            v.extend_from_slice(&(100u32).to_le_bytes());
            v.extend_from_slice(&[0xAA; 50]); // 只有 50 字节，不够 100
            v
        },
        {
            // CRC 故意错误
            let entry = WalEntry::Insert::<f32> {
                id: 999,
                vector: vec![9.0, 9.0, 9.0, 9.0],
                payload: r#"{"poison":true}"#.to_string(),
            };
            let data = bincode::serialize(&entry).unwrap();
            let bad_crc = 0xDEADBEEFu32;
            let mut v = Vec::new();
            v.extend_from_slice(&(data.len() as u32).to_le_bytes());
            v.extend_from_slice(&data);
            v.extend_from_slice(&bad_crc.to_le_bytes());
            v
        },
    ];

    for (i, garbage) in garbage_patterns.iter().enumerate() {
        let mut corrupted = valid_wal.clone();
        corrupted.extend_from_slice(garbage);

        let result = std::panic::catch_unwind(|| {
            Wal::read_entries_from_reader::<f32>(Cursor::new(&corrupted))
        });

        assert!(result.is_ok(), "垃圾 pattern #{} 导致解析器 panic", i);

        let (entries, _) = result.unwrap().unwrap();
        assert_eq!(
            entries.len(),
            baseline_count,
            "垃圾 pattern #{}: 有效前缀的 {} 条记录应完整恢复，实际 {}",
            i,
            baseline_count,
            entries.len()
        );
    }

    eprintln!(
        "  ✅ 有效前缀+垃圾尾部: {} 种 pattern 下 {} 条前缀记录均完整恢复",
        garbage_patterns.len(),
        baseline_count
    );
}

// ════════════════════════════════════════════════════════════════
//  测试 6: 端到端 — WAL 文件物理截断后 Database::open 恢复
// ════════════════════════════════════════════════════════════════

/// 完整的端到端测试：
///   1. 创建数据库，写入 5 个节点并 flush
///   2. 继续写入 3 个节点（仅在 WAL 中）
///   3. 手动将 .wal 文件在每个关键偏移处截断
///   4. 用 Database::open 重新加载，验证恢复
#[test]
fn WAL_端到端_文件物理截断后Database重新加载() {
    let path = tmp_db("e2e_truncate");
    cleanup(&path);

    // Step 1: 写入 5 个节点并 flush
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..5u32 {
            db.insert(
                &[i as f32, 0.0, 0.0, 0.0],
                serde_json::json!({"phase": "flushed", "seq": i}),
            )
            .unwrap();
        }
        db.flush().unwrap();

        // Step 2: 继续写入 3 个节点（不 flush）
        for i in 5..8u32 {
            db.insert(
                &[i as f32, 0.0, 0.0, 0.0],
                serde_json::json!({"phase": "wal_only", "seq": i}),
            )
            .unwrap();
        }
        // Drop 会 flush BufWriter
    }

    // 读取完整 WAL 的大小
    let wal_path = format!("{}.wal", path);
    let wal_bytes = std::fs::read(&wal_path).unwrap_or_default();
    let wal_len = wal_bytes.len();

    assert_ne!(wal_len, 0, "WAL 文件为空，无法执行物理截断恢复测试");

    // Step 3: 在关键偏移处截断 WAL，每次截断后重新加载
    // 采样截断点：每 10 字节取一个 + 关键位置
    let mut cut_points: Vec<usize> = (0..wal_len).step_by(10).collect();
    // 补充关键位置
    for offset in [
        1,
        2,
        3,
        4,
        5,
        wal_len / 4,
        wal_len / 2,
        wal_len * 3 / 4,
        wal_len - 1,
    ] {
        if offset < wal_len && !cut_points.contains(&offset) {
            cut_points.push(offset);
        }
    }
    cut_points.sort();

    for &cut_at in &cut_points {
        // 复制原始 WAL 并截断
        let truncated = &wal_bytes[..cut_at];
        std::fs::write(&wal_path, truncated).unwrap();

        // 删除 flush_ok 以强制恢复路径
        std::fs::remove_file(format!("{}.flush_ok", path)).ok();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Database::<f32>::open(&path, DIM)
        }));

        assert!(
            result.is_ok(),
            "WAL 截断到 {}/{} 字节后 Database::open panic 了！",
            cut_at,
            wal_len
        );

        match result.unwrap() {
            Ok(db) => {
                // 至少 5 个已 flush 的节点应存活
                assert!(
                    db.node_count() >= 5,
                    "WAL 截断到 {}/{} 后节点数 {} < 5（已 flush 的节点丢失了）",
                    cut_at,
                    wal_len,
                    db.node_count()
                );
                assert!(
                    db.node_count() <= 8,
                    "WAL 截断到 {}/{} 后节点数 {} > 8（凭空多出节点）",
                    cut_at,
                    wal_len,
                    db.node_count()
                );
            }
            Err(e) => {
                // 如果加载失败也是可接受的（极端截断可能破坏了必要的元数据）
                eprintln!(
                    "  ⚠️ WAL 截断到 {}/{} 后加载失败（可接受）: {}",
                    cut_at, wal_len, e
                );
            }
        }
    }

    eprintln!(
        "  ✅ 端到端 WAL 截断: 在 {} 个采样截断点上均安全恢复或优雅拒绝",
        cut_points.len()
    );

    // 恢复原始 WAL 以便清理
    std::fs::write(&wal_path, &wal_bytes).ok();
    cleanup(&path);
}
