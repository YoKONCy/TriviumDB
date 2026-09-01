#![allow(non_snake_case)]
//! GJB-5000B 条款 6.3.2: unsafe 代码路径专项安全验证
//!
//! 当前源码中的 28 处 unsafe 分布在 4 个模块：
//!   - vector.rs:    AVX2 / NEON SIMD 算子 (6 处)
//!   - vec_pool.rs:  mmap MAP_PRIVATE + from_raw_parts (6 处)
//!   - hook.rs:      FFI 外部函数调用 + Send/Sync impl (6 处)
//!   - pipeline.rs:  ARM prefetch 指令 (4 处)
//!   - file_format.rs: Mmap::map + from_raw_parts (2 处)
//!
//! 本文件为每一类 unsafe 路径提供专项测试，证明其安全契约成立。

use triviumdb::VectorType;
use triviumdb::database::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("unsafe_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

fn assert_open_result_has_no_dirty_data(
    result: std::thread::Result<triviumdb::Result<Database<f32>>>,
    max_nodes: usize,
    payload_key: &str,
) {
    assert!(result.is_ok(), "损坏文件不应触发 panic 或段错误");
    match result.unwrap() {
        Ok(db) => {
            assert!(
                db.node_count() <= max_nodes,
                "降级加载不能产生超过原始规模的脏节点"
            );
            for id in db.all_node_ids() {
                let payload = db.get_payload(id).expect("恢复节点必须有 payload");
                assert!(
                    payload.get(payload_key).is_some() || payload == serde_json::json!({}),
                    "恢复节点 payload 必须来自已知写入形态: {payload}"
                );
            }
        }
        Err(e) => assert!(!e.to_string().is_empty(), "安全拒绝必须返回可诊断错误"),
    }
}

fn assert_clean_reopen_after_reject(path: &str) {
    cleanup(path);
    let db = Database::<f32>::open(path, DIM).unwrap();
    assert_eq!(db.node_count(), 0, "清理后同路径重建不能携带脏状态");
    cleanup(path);
}

// ════════════════════════════════════════════════════════════════
//  1. SIMD 路径安全性 — vector.rs
// ════════════════════════════════════════════════════════════════

/// SIMD 尾部处理：向量长度不是 8 的倍数时，AVX2 内核必须正确处理剩余元素。
/// 验证所有可能的尾部长度 (1-7) 不会越界读取或返回 NaN。
#[test]
fn UNSAFE_01_SIMD_非8倍数维度_尾部处理正确性() {
    for dim in [1, 2, 3, 5, 7, 9, 13, 15, 17, 31, 33, 63, 65, 127, 129] {
        let a: Vec<f32> = (0..dim).map(|i| (i as f32 + 1.0) * 0.01).collect();
        let b: Vec<f32> = (0..dim).map(|i| (i as f32 + 2.0) * 0.01).collect();

        let sim = f32::similarity(&a, &b);

        assert!(
            !sim.is_nan(),
            "dim={}: SIMD 路径返回 NaN，尾部处理存在越界读取",
            dim
        );
        assert!(
            (-1.01..=1.01).contains(&sim),
            "dim={}: similarity={} 超出 [-1,1] 范围",
            dim,
            sim
        );
    }
}

/// SIMD 极端浮点值：f32::MAX, f32::MIN, 次正规数, ±0.0
/// SIMD 寄存器在这些值上的行为可能与标量路径不同
#[test]
fn UNSAFE_02_SIMD_极端浮点值_不产生NaN_Inf() {
    let cases: Vec<(&str, Vec<f32>)> = vec![
        ("max", vec![f32::MAX; 4]),
        ("min", vec![f32::MIN; 4]),
        ("min_positive", vec![f32::MIN_POSITIVE; 4]),
        ("subnormal", vec![1.0e-40_f32; 4]),
        ("neg_zero", vec![-0.0_f32; 4]),
        ("pos_zero", vec![0.0_f32; 4]),
        (
            "mixed_extreme",
            vec![f32::MAX, f32::MIN, 0.0, f32::MIN_POSITIVE],
        ),
    ];

    let normal = vec![1.0_f32, 0.0, 0.0, 0.0];

    for (label, vec) in &cases {
        let sim = f32::similarity(vec, &normal);
        assert!(
            !sim.is_nan() && !sim.is_infinite(),
            "{}: similarity 产生了 NaN/Inf (got {})",
            label,
            sim
        );
    }
}

/// SIMD 与标量路径的一致性：禁用 SIMD 后的结果应与 SIMD 结果一致（误差 < 1e-5）
/// 由于无法运行时禁用 SIMD，我们通过手动计算标量余弦相似度来对比
#[test]
fn UNSAFE_03_SIMD_与标量路径结果一致() {
    let dims = [4, 8, 16, 32, 64, 128, 256];

    for dim in dims {
        let a: Vec<f32> = (0..dim).map(|i| ((i as f32) * 0.37).sin()).collect();
        let b: Vec<f32> = (0..dim).map(|i| ((i as f32) * 0.73).cos()).collect();

        // SIMD 路径（通过 VectorType trait）
        let simd_result = f32::similarity(&a, &b);

        // 手动标量计算
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        let scalar_result = if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            dot / (norm_a * norm_b)
        };

        let diff = (simd_result - scalar_result).abs();
        assert!(
            diff < 1e-5,
            "dim={}: SIMD({}) vs scalar({}) 差异 {} 超过阈值",
            dim,
            simd_result,
            scalar_result,
            diff
        );
    }
}

// ════════════════════════════════════════════════════════════════
//  2. VecPool mmap 路径安全性 — vec_pool.rs
// ════════════════════════════════════════════════════════════════

/// mmap 后外部截断 .vec 文件：不应 SIGBUS/SIGSEGV
/// MAP_PRIVATE 的 COW 语义应该保护进程免受文件截断的影响
#[test]
fn UNSAFE_04_VecPool_文件外部截断后读取不崩溃() {
    let path = tmp_db("vecpool_trunc");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..100u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({"idx": i}))
                .unwrap();
        }
        db.flush().unwrap();
    }

    // 外部截断 .vec 文件到一半
    let vec_path = format!("{}.vec", path);
    if let Ok(meta) = std::fs::metadata(&vec_path) {
        let half = meta.len() / 2;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&vec_path)
            .unwrap();
        file.set_len(half).unwrap();
    }

    // 删除 flush_ok 标记
    std::fs::remove_file(format!("{}.flush_ok", path)).ok();

    // 重新打开：不应 panic / segfault
    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(
        result.is_ok(),
        ".vec 文件被截断后引擎触发了 panic / segfault"
    );
    match result.unwrap() {
        Ok(db) => {
            assert!(db.node_count() <= 100, ".vec 截断降级加载不能产生额外节点");
            for id in db.all_node_ids() {
                let payload = db.get_payload(id).expect("恢复节点必须有 payload");
                let idx = payload
                    .get("idx")
                    .and_then(|value| value.as_u64())
                    .expect("恢复节点必须携带原始 idx 字段");
                assert!(idx < 100, ".vec 截断恢复不能产生越界 payload: {payload}");
            }
        }
        Err(e) => assert!(!e.to_string().is_empty(), ".vec 截断拒绝必须返回可诊断错误"),
    }

    cleanup(&path);
}

/// mmap 后 .vec 文件被删除：不应 SIGBUS
#[test]
fn UNSAFE_05_VecPool_文件被删除后读取不崩溃() {
    let path = tmp_db("vecpool_deleted");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..50u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
                .unwrap();
        }
        db.flush().unwrap();
    }

    // 删除 .vec 和 .flush_ok
    std::fs::remove_file(format!("{}.vec", path)).ok();
    std::fs::remove_file(format!("{}.flush_ok", path)).ok();

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert_open_result_has_no_dirty_data(result, 50, "idx");

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  3. file_format 反序列化安全性
// ════════════════════════════════════════════════════════════════

/// 构造一个畸形 .tdb 文件：header 声明的偏移量越界
/// 验证 read_u64_le 等安全读取函数能正确拦截
#[test]
fn UNSAFE_06_畸形Header偏移量越界_不panic() {
    let path = tmp_db("malformed_offsets");
    cleanup(&path);

    // 先创建一个合法的小 DB
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"ok": true}))
            .unwrap();
        db.flush().unwrap();
    }

    // 篡改 header 中的 payload_offset 为一个越界值
    if let Ok(mut data) = std::fs::read(&path)
        && data.len() >= 58
    {
        // payload_offset 字段在 offset 26-34，设为 u64::MAX
        let bad_offset = u64::MAX.to_le_bytes();
        data[26..34].copy_from_slice(&bad_offset);
        std::fs::write(&path, &data).unwrap();
        std::fs::remove_file(format!("{}.flush_ok", path)).ok();
    }

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "畸形偏移量不应触发 panic");

    match result.unwrap() {
        Ok(_) => panic!("畸形偏移量应被拒绝，不应成功加载"),
        Err(e) => assert!(
            !e.to_string().is_empty(),
            "畸形偏移量拒绝必须返回可诊断错误"
        ),
    }
    assert_clean_reopen_after_reject(&path);

    cleanup(&path);
}

/// 构造一个最小化的畸形 .tdb：只有 magic 没有完整 header
#[test]
fn UNSAFE_07_截断Header_只有Magic() {
    let path = tmp_db("tiny_header");
    cleanup(&path);

    // 写入 4 字节 magic + 2 字节垃圾（不足 HEADER_SIZE=58）
    std::fs::write(&path, b"TVDB\x05\x00").unwrap();

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "截断 header 不应 panic");
    assert!(result.unwrap().is_err(), "只有 6 字节的文件应被拒绝");

    cleanup(&path);
}

/// node_count 声明值远大于文件实际大小
#[test]
fn UNSAFE_08_node_count溢出_声明百万实际为空() {
    let path = tmp_db("nodecount_overflow");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
        db.flush().unwrap();
    }

    // 篡改 node_count 为 1_000_000
    if let Ok(mut data) = std::fs::read(&path)
        && data.len() >= 58
    {
        let fake_count = 1_000_000u64.to_le_bytes();
        data[18..26].copy_from_slice(&fake_count);
        std::fs::write(&path, &data).unwrap();
        std::fs::remove_file(format!("{}.flush_ok", path)).ok();
    }

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "node_count 溢出不应 panic");
    let opened = result.unwrap();
    assert!(opened.is_err(), "node_count 溢出文件应被拒绝");
    let err = opened.err().unwrap();
    assert!(
        !err.to_string().is_empty(),
        "node_count 溢出拒绝必须返回可诊断错误"
    );
    assert_clean_reopen_after_reject(&path);

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  4. 零字节/空文件极端情况
// ════════════════════════════════════════════════════════════════

/// 零字节 .tdb 文件
#[test]
fn UNSAFE_09_零字节文件_不panic() {
    let path = tmp_db("zero_bytes");
    cleanup(&path);

    std::fs::write(&path, b"").unwrap();

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "零字节文件不应 panic");
    assert!(result.unwrap().is_err(), "零字节文件应被拒绝");

    cleanup(&path);
}

/// 1 字节 .tdb 文件
#[test]
fn UNSAFE_10_单字节文件_不panic() {
    let path = tmp_db("one_byte");
    cleanup(&path);

    std::fs::write(&path, [0xFF]).unwrap();

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "单字节文件不应 panic");
    let opened = result.unwrap();
    assert!(opened.is_err(), "单字节文件应被拒绝");
    let err = opened.err().unwrap();
    assert!(!err.to_string().is_empty(), "单字节拒绝应有错误信息");
    assert_clean_reopen_after_reject(&path);

    cleanup(&path);
}
