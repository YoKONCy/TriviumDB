#![allow(non_snake_case)]
//! GJB-5000B 长时间稳定性浸泡测试
//!
//! 验证有界数据量下的内核配额、资源归还、文件大小与完整持久化内容，并记录 RSS。
//! 长时用例标记为 #[ignore]，通过 `cargo test --test long_running soak -- --ignored` 运行。

use triviumdb::database::{Config, Database};
use triviumdb::resources::{ResourceLimits, ResourceRuntime};

const DIM: usize = 4;
// 一百条小记录采用明确的宿主配额，解析缓存属于这八 MiB 内核预算。
const DATABASE_BUDGET: usize = 8 * 1024 * 1024;
const PAYLOAD_CACHE_BUDGET: usize = 256 * 1024;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test").join(format!(
        "soak_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("sample.tdb").to_string_lossy().to_string()
}

fn cleanup(path: &str) {
    let root = std::path::Path::new(path).parent().unwrap();
    assert_eq!(
        root.parent().unwrap(),
        std::env::temp_dir().join("triviumdb_test")
    );
    assert!(
        root.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("soak_")
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// 读取操作系统进程观测；采样失败和零值都使验收失败。
fn get_rss_bytes() -> u64 {
    #[cfg(target_os = "windows")]
    let rss = {
        use std::os::windows::process::CommandExt;
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("(Get-Process -Id {}).WorkingSet64", std::process::id()),
            ])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW：独立采样进程使用隐藏窗口。
            .output()
            .expect("无法启动 Windows 进程内存采样命令");
        assert!(output.status.success(), "Windows 进程内存采样命令执行失败");
        std::str::from_utf8(&output.stdout)
            .expect("RSS 观测编码错误")
            .trim()
            .parse::<u64>()
            .expect("RSS 观测缺失或格式错误")
    };
    #[cfg(target_os = "linux")]
    let rss = {
        let status = std::fs::read_to_string("/proc/self/status").expect("进程内存观测文件不可读");
        let mut fields = status
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))
            .expect("进程内存观测缺少 VmRSS")
            .split_whitespace();
        let kib = fields
            .next()
            .expect("RSS 观测缺少数值")
            .parse::<u64>()
            .expect("RSS 数值格式错误");
        assert_eq!(fields.next(), Some("kB"), "RSS 观测单位必须为 KiB");
        kib.checked_mul(1024).expect("RSS 单位换算溢出")
    };
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    let rss = {
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .expect("无法启动进程内存采样命令");
        assert!(output.status.success(), "进程内存采样命令执行失败");
        std::str::from_utf8(&output.stdout)
            .expect("RSS 观测编码错误")
            .trim()
            .parse::<u64>()
            .expect("RSS 观测缺失或格式错误")
            .checked_mul(1024)
            .expect("RSS 单位换算溢出")
    };
    assert!(rss > 0, "RSS 观测必须为正数");
    rss
}

#[test]
fn RSS采样必须取得有效进程观测() {
    assert!(get_rss_bytes() > 0);
}

// ════════════════════════════════════════════════════════════════
//  1. 五分钟有界数据量配额、资源归还与重开验收
// ════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn SOAK_01_连续运行5分钟_配额归还与完整重开() {
    let path = tmp_db("mem_leak");
    let runtime = ResourceRuntime::new(ResourceLimits {
        memory_limit: 2 * DATABASE_BUDGET,
        max_threads: 2,
        ..Default::default()
    })
    .unwrap();
    let config = Config {
        dim: DIM,
        memory_limit: DATABASE_BUDGET,
        payload_cache_bytes: PAYLOAD_CACHE_BUDGET,
        payload_cache_entry_bytes: 4096,
        ..Default::default()
    };
    // 工作负载及其全部持有者在闭包内退出，清理后再传播原始断言失败。
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut db = Database::<f32>::open_with_runtime(&path, config, runtime.clone()).unwrap();
        let mut live = std::collections::VecDeque::new();
        write_round(&mut db, &mut live, 0);
        db.compact().unwrap();
        db.flush().unwrap();
        let initial_rss = get_rss_bytes();
        let start = std::time::Instant::now();
        let duration = std::time::Duration::from_secs(300);
        let mut round = 1u64;
        let mut peak_rss = initial_rss;

        while start.elapsed() < duration {
            write_round(&mut db, &mut live, round);
            if round.is_multiple_of(10) {
                db.compact().unwrap();
                db.flush().unwrap();
                assert_quiescent_budget(&db);
            }
            peak_rss = peak_rss.max(get_rss_bytes());
            round += 1;
        }
        db.compact().unwrap();
        db.flush().unwrap();
        assert_quiescent_budget(&db);
        assert_live_facts(&db, &live);
        let final_rss = get_rss_bytes();
        eprintln!(
            "浸泡资源观测 (Soak resource observation): rounds={round}, initial_rss_bytes={initial_rss}, peak_rss_bytes={peak_rss}, final_rss_bytes={final_rss}, estimated_memory_bytes={}",
            db.estimated_memory()
        );
        db.close().unwrap();
        assert_eq!(runtime.stats().reserved_database_bytes, 0);
        assert_eq!(runtime.stats().attached_databases, 0);

        let mut reopened =
            Database::<f32>::open_with_runtime(&path, config, runtime.clone()).unwrap();
        assert_live_facts(&reopened, &live);
        reopened.flush().unwrap();
        reopened.close().unwrap();
        let mut readonly = Database::<f32>::open_read_only(&path, DIM).unwrap();
        assert_live_facts(&readonly, &live);
        readonly.close().unwrap();
        let resources = runtime.stats();
        assert_eq!(resources.reserved_database_bytes, 0);
        assert_eq!(resources.reserved_workspace_bytes, 0);
        assert_eq!(resources.reserved_maintenance_bytes, 0);
        assert_eq!(resources.active_tasks, 0);
        assert_eq!(resources.attached_databases, 0);
    }));
    cleanup(&path);
    if let Err(failure) = outcome {
        std::panic::resume_unwind(failure);
    }
}

fn assert_quiescent_budget(db: &Database<f32>) {
    assert!(db.estimated_memory() <= DATABASE_BUDGET);
    assert!(db.payload_memory_stats().parsed_cache_bytes <= PAYLOAD_CACHE_BUDGET);
    let resources = db.resource_stats();
    assert_eq!(resources.reserved_database_bytes, DATABASE_BUDGET);
    assert_eq!(resources.reserved_workspace_bytes, 0);
    assert_eq!(resources.reserved_maintenance_bytes, 0);
    assert_eq!(resources.active_tasks, 0);
    assert_eq!(resources.active_maintenance_tasks, 0);
}

fn assert_live_facts(db: &Database<f32>, live: &std::collections::VecDeque<u64>) {
    assert_eq!(
        db.all_node_ids().unwrap(),
        live.iter().copied().collect::<Vec<_>>()
    );
    for &id in live {
        let node = db.get(id).unwrap().unwrap();
        assert_eq!(node.vector, vec![id as f32, 1.0, 0.0, 0.0]);
        assert_eq!(node.payload, serde_json::json!({"id": id}));
        assert!(db.get_edges(id).unwrap().is_empty());
    }
}

fn write_round(db: &mut Database<f32>, live: &mut std::collections::VecDeque<u64>, round: u64) {
    // 每轮写入一百条，保留最新一百条，使逻辑数据量保持有界。
    for offset in 1..=100 {
        let id = round * 100 + offset;
        db.insert_with_id(
            id,
            &[id as f32, 1.0, 0.0, 0.0],
            serde_json::json!({"id":id}),
        )
        .unwrap();
        live.push_back(id);
    }
    while live.len() > 100 {
        db.delete(live.pop_front().unwrap()).unwrap();
    }
    for _ in 0..10 {
        let hits = db.search(&[1.0, 0.0, 0.0, 0.0], 10, 0, 0.0).unwrap();
        assert_eq!(hits.len(), 10, "有界负载的向量查询必须返回 Top 10");
        for hit in hits {
            assert!(live.contains(&hit.id), "查询返回已删除或未写入节点");
            assert_eq!(hit.payload, serde_json::json!({"id":hit.id}));
        }
    }
}

// ════════════════════════════════════════════════════════════════
//  2. 文件大小有界浸泡
// ════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn SOAK_02_连续flush_500轮_文件大小不膨胀() {
    let path = tmp_db("file_bloat");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let mut live = std::collections::VecDeque::new();
    write_round(&mut db, &mut live, 0);
    db.compact().unwrap();
    db.flush().unwrap();

    let baseline_size = file_total_size(&path);
    eprintln!("  基准文件大小: {}KB", baseline_size / 1024);

    // 500 轮: insert → delete → compact → flush
    for round in 1..=500 {
        write_round(&mut db, &mut live, round);
        db.compact().unwrap();
        db.flush().unwrap();
    }

    let final_size = file_total_size(&path);
    eprintln!(
        "  500 轮后文件大小: {}KB (基准 {}KB)",
        final_size / 1024,
        baseline_size / 1024
    );

    // 五倍空间界限覆盖递增序号与文件组的页面分配，逻辑节点数始终为一百。
    assert!(
        final_size < baseline_size * 5,
        "文件大小膨胀: baseline={}KB, final={}KB",
        baseline_size / 1024,
        final_size / 1024
    );

    let mut actual = db.all_node_ids().unwrap();
    actual.sort_unstable();
    assert_eq!(actual, live.into_iter().collect::<Vec<_>>());
    db.close().unwrap();
    cleanup(&path);
}

fn file_total_size(path: &str) -> u64 {
    assert!(
        std::fs::metadata(path).unwrap().len() > 0,
        "主文件观测必须存在且包含内容"
    );
    // 每个样本独占目录，统计全部文件组，包含独立 Payload 和索引文件。
    std::fs::read_dir(std::path::Path::new(path).parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().metadata().unwrap().len())
        .sum()
}
