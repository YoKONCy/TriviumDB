//! 多实例资源准入、回调线程与维护调度的回归。

use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};
use triviumdb::database::Config;
use triviumdb::hook::{HookContext, SearchHook};
use triviumdb::{Database, ResourceLimits, ResourceRuntime, SearchConfig, TriviumError};

const MB: usize = 1024 * 1024;

fn path(label: &str) -> String {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir()
        .join(format!(
            "tdb_resources_{}_{nonce}_{label}.tdb",
            std::process::id()
        ))
        .to_string_lossy()
        .into_owned()
}

fn runtime(memory: usize, tasks: usize) -> ResourceRuntime {
    ResourceRuntime::new(ResourceLimits {
        maintenance_memory_limit: 0,
        memory_limit: memory,
        max_threads: 1,
        max_active_tasks: tasks,
    })
    .unwrap()
}

fn open(path: &str, runtime: &ResourceRuntime, memory: usize) -> Database<f32> {
    Database::open_with_runtime(
        path,
        Config {
            dim: 3,
            auto_build_quiver: false,
            memory_limit: memory,
            ..Default::default()
        },
        runtime.clone(),
    )
    .unwrap()
}

struct Gate {
    entered: mpsc::Sender<std::thread::ThreadId>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl SearchHook for Gate {
    fn on_pre_search(&self, _: &mut Vec<f32>, _: &mut SearchConfig, _: &mut HookContext) {
        self.entered.send(std::thread::current().id()).unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(20))
            .unwrap();
    }
}

#[test]
fn 活动任务跨库拒绝且回调保持调用线程() {
    let shared = runtime(64 * MB, 1);
    let mut first = open(&path("first"), &shared, 8 * MB);
    let mut second = open(&path("second"), &shared, 8 * MB);
    let independent = runtime(32 * MB, 1);
    let mut third = open(&path("independent"), &independent, 8 * MB);
    first
        .insert_with_id(7, &[1.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    let (entered, arrived) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    first
        .set_hook(Gate {
            entered,
            release: Mutex::new(wait),
        })
        .unwrap();
    let worker = std::thread::spawn(move || {
        let id = std::thread::current().id();
        let result = first.search(&[1.0, 0.0, 0.0], 1, 0, 0.0);
        (first, id, result)
    });
    let callback_thread = arrived.recv_timeout(Duration::from_secs(20)).unwrap();
    let rejected = second.insert(&[0.0, 1.0, 0.0], serde_json::json!({}));
    let independent_result = third.insert(&[0.0, 1.0, 0.0], serde_json::json!({}));
    release.send(()).unwrap();
    let (mut first, caller_thread, result) = worker.join().unwrap();
    assert_eq!(callback_thread, caller_thread);
    assert_eq!(result.unwrap()[0].id, 7);
    assert!(matches!(
        rejected.unwrap_err().underlying(),
        TriviumError::ResourceBudgetExceeded {
            resource: "active_tasks",
            ..
        }
    ));
    independent_result.unwrap();
    assert_eq!(second.node_count().unwrap(), 0);
    assert_eq!(shared.stats().active_tasks, 0);
    second
        .insert(&[0.0, 1.0, 0.0], serde_json::json!({}))
        .unwrap();
    first.close().unwrap();
    second.close().unwrap();
    third.close().unwrap();
    assert_eq!(shared.stats().reserved_database_bytes, 0);
}

#[test]
fn 工作区拒绝关闭时保留配额文件锁与可重试数据() {
    let shared = runtime(32 * MB, 1);
    let first_path = path("close-retry");
    let mut first = open(&first_path, &shared, 16 * MB);
    let mut second = open(&path("quota-holder"), &shared, 12 * MB);
    first
        .enable_auto_compaction(Duration::from_secs(3600))
        .unwrap();
    first
        .insert_with_id(
            11,
            &[1.0, 0.0, 0.0],
            serde_json::json!({"padding": "x".repeat(2 * MB)}),
        )
        .unwrap();
    assert!(matches!(
        first.close(),
        Err(TriviumError::ResourceBudgetExceeded {
            resource: "maintenance_memory_bytes",
            ..
        })
    ));
    assert_eq!(shared.stats().reserved_database_bytes, 28 * MB);
    assert_eq!(shared.stats().reserved_workspace_bytes, 0);
    assert_eq!(shared.stats().maintenance_jobs, 1);
    assert_eq!(
        first.instance_stats().compaction.unwrap().phase,
        triviumdb::MaintenancePhase::Scheduled
    );
    assert!(first.contains(11).unwrap());
    assert!(Database::<f32>::open(&first_path, 3).is_err());
    second.set_memory_limit(4 * MB).unwrap();
    first.close().unwrap();
    assert_eq!(shared.stats().maintenance_jobs, 0);
    let mut reopened = Database::<f32>::open(&first_path, 3).unwrap();
    assert!(reopened.contains(11).unwrap());
    reopened.close().unwrap();
    second.close().unwrap();
    assert_eq!(shared.stats().attached_databases, 0);
}

#[test]
fn 开库失败回收登记且有限运行时要求显式数据库配额() {
    let shared = runtime(32 * MB, 1);
    let target = path("invalid");
    for config in [
        Config {
            dim: 0,
            memory_limit: 8 * MB,
            ..Default::default()
        },
        Config {
            dim: 3,
            ..Default::default()
        },
    ] {
        assert!(Database::<f32>::open_with_runtime(&target, config, shared.clone()).is_err());
        assert_eq!(shared.stats().attached_databases, 0);
        assert_eq!(shared.stats().reserved_database_bytes, 0);
        assert_eq!(shared.stats().active_tasks, 0);
        assert!(!std::path::Path::new(&target).exists());
    }
}

#[test]
fn 受限运行时的只读打开查询关闭不改写制品() {
    use std::collections::BTreeMap;
    use triviumdb::database::AccessMode;
    let target = path("reader");
    let shared = runtime(64 * MB, 1);
    let mut writer = open(&target, &shared, 8 * MB);
    writer
        .insert_with_id(1, &[1.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    writer.close().unwrap();
    triviumdb::storage::snapshot::write_manifest(
        &target,
        "resource-reader",
        std::any::type_name::<f32>(),
        3,
        1,
    )
    .unwrap();
    let snapshot = || {
        [
            "",
            ".vec",
            ".wal",
            ".lock",
            ".pidx",
            ".quiver",
            ".text",
            ".manifest.json",
            ".flush_ok",
            ".pld.1",
            ".pld.2",
        ]
        .into_iter()
        .map(|suffix| (suffix, std::fs::read(format!("{target}{suffix}")).ok()))
        .collect::<BTreeMap<_, _>>()
    };
    let before = snapshot();
    for access_mode in [AccessMode::ReadOnly, AccessMode::Immutable] {
        let mut reader = Database::<f32>::open_with_runtime(
            &target,
            Config {
                dim: 3,
                memory_limit: 8 * MB,
                auto_build_quiver: false,
                access_mode,
                ..Default::default()
            },
            shared.clone(),
        )
        .unwrap();
        assert_eq!(reader.search_exact(&[1.0, 0.0, 0.0], 1).unwrap()[0].id, 1);
        reader.close().unwrap();
        assert_eq!(snapshot(), before);
        assert_eq!(shared.stats().attached_databases, 0);
    }
}

#[test]
fn 单线程共享维护不会阻断前台检索且关闭撤销登记() {
    let shared = runtime(64 * MB, 2);
    let mut databases = (0..3)
        .map(|index| {
            let mut db = open(&path(&format!("maintenance-{index}")), &shared, 8 * MB);
            db.insert_with_id(index + 1, &[1.0, 0.0, 0.0], serde_json::json!({}))
                .unwrap();
            db.enable_auto_compaction(Duration::from_millis(5)).unwrap();
            db
        })
        .collect::<Vec<_>>();
    assert_eq!(shared.stats().maintenance_jobs, 3);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        for (index, db) in databases.iter().enumerate() {
            match db.search_exact(&[1.0, 0.0, 0.0], 1) {
                Ok(hits) => assert_eq!(hits[0].id, index as u64 + 1),
                Err(TriviumError::ResourceBudgetExceeded { .. }) => {}
                Err(error) => panic!("前台检索失败：{error}"),
            }
        }
        if shared.stats().maintenance_completed > 0 {
            break;
        }
        assert!(Instant::now() < deadline, "共享维护未在期限内完成");
        std::thread::sleep(Duration::from_millis(2));
    }
    for db in &mut databases {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match db.close() {
                Ok(()) => break,
                Err(TriviumError::ResourceBudgetExceeded { .. }) => {
                    assert!(Instant::now() < deadline, "关闭未在额度归还后完成");
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("关闭失败：{error}"),
            }
        }
    }
    let stats = shared.stats();
    assert_eq!(stats.maintenance_jobs, 0);
    assert_eq!(stats.active_tasks, 0);
    assert_eq!(stats.reserved_database_bytes, 0);
    assert!(stats.peak_worker_threads <= 1);
}
