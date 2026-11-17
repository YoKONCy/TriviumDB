#![allow(non_snake_case)]
//! GJB-5000B 条款 6.3.2: unsafe 代码路径专项安全验证
//!
//! 本文件覆盖向量算子的数值/尾部边界、关闭后的向量文件损坏与畸形文件加载。
//! unsafe 清单随源码演进，不以固定数量代表审计完成；平台与 feature 决定实际执行路径。
//!
//! BQ 裸指针与并发邻接表另由 memory_safety_ 子集及 TSan/Miri 检查。
//! 这些都是特定输入和执行路径的回归验证，不构成所有 unsafe 安全契约的证明，
//! 也不覆盖全部 SIMD 指令、FFI 回调、手工 Send/Sync 或所有线程交错。

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
    crate::cleanup_files::database(path);
}

fn assert_corruption_rejected(result: std::thread::Result<triviumdb::Result<Database<f32>>>) {
    assert!(result.is_ok(), "损坏文件不应触发 panic 或段错误");
    assert!(
        matches!(
            result.unwrap(),
            Err(triviumdb::TriviumError::CorruptedFile(_))
        ),
        "无可验证的恢复基础时，必须返回结构化损坏错误，不能以空库或部分数据伪装成功"
    );
}

fn assert_empty_database_after_explicit_removal(path: &str) {
    // 这里只验证显式删除后新建数据库，不能将它计入原数据恢复证据。
    cleanup(path);
    let db = Database::<f32>::open(path, DIM).unwrap();
    assert_eq!(
        db.node_count().unwrap(),
        0,
        "清理后同路径重建不能携带脏状态"
    );
    cleanup(path);
}

// ════════════════════════════════════════════════════════════════
//  1. SIMD 路径安全性 — vector.rs
// ════════════════════════════════════════════════════════════════

/// 八元素 SIMD 分块的所有尾部长度；用独立 f64 余弦验证每一维都参与计算。
#[test]
fn UNSAFE_01_SIMD_非8倍数维度_尾部处理正确性() {
    for dim in (1..=17).chain([31, 33, 63, 65, 127, 129]) {
        let a: Vec<f32> = (0..dim).map(|i| (i as f32 + 1.0) * 0.01).collect();
        let b: Vec<f32> = (0..dim).map(|i| (i as f32 + 2.0) * 0.01).collect();

        let sim = f32::similarity(&a, &b);

        let dot = a
            .iter()
            .zip(&b)
            .map(|(&x, &y)| f64::from(x) * f64::from(y))
            .sum::<f64>();
        let norm = |values: &[f32]| {
            values
                .iter()
                .map(|&value| f64::from(value).powi(2))
                .sum::<f64>()
                .sqrt()
        };
        let expected = dot / (norm(&a) * norm(&b));
        assert!(
            (f64::from(sim) - expected).abs() < 1e-5,
            "维度 {dim} 的向量尾部结果不符：实际={sim}，独立余弦={expected}"
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

/// 关闭数据库后截断向量文件，保留完整提交标记；重开时必须拒绝不完整文件组。
#[test]
fn UNSAFE_04_关闭后向量文件截断_重开返回损坏错误() {
    let path = tmp_db("vecpool_trunc");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..100u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({"idx": i}))
                .unwrap();
        }
        db.flush().unwrap();
        db.close().unwrap();
    }

    // 外部截断 .vec 文件到一半
    let vec_path = format!("{}.vec", path);
    let size = std::fs::metadata(&vec_path).unwrap().len();
    assert!(size > 0, "样本必须包含向量文件内容");
    let marker = std::fs::read(format!("{}.flush_ok", path)).unwrap();
    // 截去一半向量数据，保证文件长度小于该检查点要求的完整长度。
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&vec_path)
        .unwrap();
    file.set_len(size / 2).unwrap();
    drop(file);

    // 重新打开：不应 panic / segfault
    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert_corruption_rejected(result);
    assert_eq!(std::fs::read(format!("{}.flush_ok", path)).unwrap(), marker);

    cleanup(&path);
}

/// 关闭数据库后删除向量文件，保留完整提交标记；重开时必须拒绝缺失文件组。
#[test]
fn UNSAFE_05_关闭后向量文件缺失_重开返回损坏错误() {
    let path = tmp_db("vecpool_deleted");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..50u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
                .unwrap();
        }
        db.flush().unwrap();
        db.close().unwrap();
    }

    let marker = std::fs::read(format!("{}.flush_ok", path)).unwrap();
    std::fs::remove_file(format!("{}.vec", path)).unwrap();

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert_corruption_rejected(result);
    assert_eq!(std::fs::read(format!("{}.flush_ok", path)).unwrap(), marker);

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
    {
        let mut data = std::fs::read(&path).unwrap();
        assert!(data.len() >= 58, "样本必须包含完整磁盘头");
        // payload_offset 字段在 offset 26-34，设为 u64::MAX
        let bad_offset = u64::MAX.to_le_bytes();
        data[26..34].copy_from_slice(&bad_offset);
        std::fs::write(&path, &data).unwrap();
        std::fs::remove_file(format!("{}.flush_ok", path)).unwrap();
    }

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "畸形偏移量不应触发 panic");

    assert_corruption_rejected(result);
    assert_empty_database_after_explicit_removal(&path);

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
    {
        let mut data = std::fs::read(&path).unwrap();
        assert!(data.len() >= 58, "样本必须包含完整磁盘头");
        let fake_count = 1_000_000u64.to_le_bytes();
        data[18..26].copy_from_slice(&fake_count);
        std::fs::write(&path, &data).unwrap();
        std::fs::remove_file(format!("{}.flush_ok", path)).unwrap();
    }

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "node_count 溢出不应 panic");
    assert_corruption_rejected(result);
    assert_empty_database_after_explicit_removal(&path);

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
    assert_corruption_rejected(result);
    assert_empty_database_after_explicit_removal(&path);

    cleanup(&path);
}
