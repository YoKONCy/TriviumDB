#![allow(non_snake_case)]
//! GJB-5000B 长时间稳定性浸泡测试
//!
//! 验证 TriviumDB 在持续高负载下内存不泄漏、文件大小有界。
//! 这些测试标记为 #[ignore]，仅在 CI 中通过 `cargo test -- --ignored` 运行。

use triviumdb::database::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("soak_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

/// 获取当前进程的近似堆内存占用
/// 使用命令行工具避免引入额外依赖
fn get_rss_bytes() -> usize {
    #[cfg(windows)]
    {
        // 使用 Rust 的 GlobalAlloc 统计不可用，回退为粗略估计
        // 通过 tasklist 获取 Working Set（仅用于趋势检测，不需精确值）
        let pid = std::process::id();
        if let Ok(output) = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/FO", "CSV", "/NH"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            // 格式: "name","pid","session","session#","mem usage"
            // mem usage 格式: "123,456 K"
            if let Some(last) = text.trim().split('"').rev().nth(1) {
                let cleaned: String = last.chars().filter(|c| c.is_ascii_digit()).collect();
                return cleaned.parse::<usize>().unwrap_or(0) * 1024; // KB → bytes
            }
        }
        0
    }
    #[cfg(not(windows))]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
            let rss_pages: usize = content
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            rss_pages * 4096
        } else {
            0
        }
    }
}

// ════════════════════════════════════════════════════════════════
//  1. 内存不泄漏浸泡（5 分钟版本，CI 跑 30 分钟可调参数）
// ════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn SOAK_01_连续运行5分钟_内存不泄漏() {
    let path = tmp_db("mem_leak");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let start = std::time::Instant::now();
    let duration = std::time::Duration::from_secs(300); // 5 分钟

    let initial_rss = get_rss_bytes();
    let mut round = 0u64;
    let mut peak_rss = initial_rss;

    while start.elapsed() < duration {
        // 插入 100 个节点
        {
            let mut tx = db.begin_tx();
            for j in 0..100u32 {
                let v = [round as f32, j as f32, 0.0, 0.0];
                tx.insert(&v, serde_json::json!({"r": round, "j": j}));
            }
            tx.commit().unwrap();
        }

        // 搜索 10 次
        for _ in 0..10 {
            let _ = db.search(&[round as f32, 0.0, 0.0, 0.0], 10, 0, 0.0);
        }

        // 删除前半部分
        let ids = db.all_node_ids();
        let to_delete: Vec<u64> = ids.iter().take(ids.len() / 2).copied().collect();
        for id in to_delete {
            db.delete(id).unwrap();
        }

        // 每 10 轮 compact + flush
        if round.is_multiple_of(10) {
            db.compact().unwrap();
            db.flush().unwrap();
        }

        // 采样 RSS
        let current_rss = get_rss_bytes();
        if current_rss > peak_rss {
            peak_rss = current_rss;
        }

        round += 1;
    }

    let final_rss = get_rss_bytes();

    eprintln!("  浸泡测试完成: {} 轮", round);
    eprintln!(
        "  内存: initial={}MB, peak={}MB, final={}MB",
        initial_rss / 1024 / 1024,
        peak_rss / 1024 / 1024,
        final_rss / 1024 / 1024
    );

    // 最终 RSS 不超过初始值的 3 倍（考虑到 Rust allocator 的 fragmentation）
    if initial_rss > 0 {
        assert!(
            final_rss < initial_rss * 3,
            "疑似内存泄漏: initial={}MB, final={}MB",
            initial_rss / 1024 / 1024,
            final_rss / 1024 / 1024,
        );
    }

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  2. 文件大小有界浸泡
// ════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn SOAK_02_连续flush_500轮_文件大小不膨胀() {
    let path = tmp_db("file_bloat");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 先建立基准大小
    for i in 0..100u32 {
        db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({"idx": i}))
            .unwrap();
    }
    db.flush().unwrap();

    let baseline_size = file_total_size(&path);
    eprintln!("  基准文件大小: {}KB", baseline_size / 1024);

    // 500 轮: insert → delete → compact → flush
    for round in 0..500u32 {
        // 插入 50 个
        {
            let mut tx = db.begin_tx();
            for j in 0..50u32 {
                tx.insert(
                    &[round as f32, j as f32, 0.0, 0.0],
                    serde_json::json!({"r": round}),
                );
            }
            tx.commit().unwrap();
        }

        // 删除最旧的 50 个
        let ids = db.all_node_ids();
        let to_delete: Vec<u64> = ids.iter().take(50.min(ids.len())).copied().collect();
        for id in to_delete {
            let _ = db.delete(id);
        }

        db.compact().unwrap();
        db.flush().unwrap();
    }

    let final_size = file_total_size(&path);
    eprintln!(
        "  500 轮后文件大小: {}KB (基准 {}KB)",
        final_size / 1024,
        baseline_size / 1024
    );

    // 文件大小不应超过基准的 5 倍（节点总数有界）
    assert!(
        final_size < baseline_size * 5,
        "文件大小膨胀: baseline={}KB, final={}KB",
        baseline_size / 1024,
        final_size / 1024
    );

    cleanup(&path);
}

fn file_total_size(path: &str) -> u64 {
    let mut total = 0u64;
    for ext in &["", ".wal", ".vec"] {
        let p = format!("{}{}", path, ext);
        if let Ok(meta) = std::fs::metadata(&p) {
            total += meta.len();
        }
    }
    total
}
