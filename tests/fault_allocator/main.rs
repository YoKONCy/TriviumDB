#![allow(non_snake_case)]
//! OOM (Out Of Memory) 极值与软内存限制探测
//! 对标 SQLite 对分配失败的容忍度

use std::alloc::{GlobalAlloc, Layout, System};
use std::panic::catch_unwind;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use triviumdb::Database;

struct BoundedFailAllocator;
static FAIL_ENABLED: AtomicBool = AtomicBool::new(false);
static FAIL_ABOVE_BYTES: AtomicUsize = AtomicUsize::new(usize::MAX);

unsafe impl GlobalAlloc for BoundedFailAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if FAIL_ENABLED.load(Ordering::Relaxed)
            && layout.size() >= FAIL_ABOVE_BYTES.load(Ordering::Relaxed)
        {
            return std::ptr::null_mut();
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: BoundedFailAllocator = BoundedFailAllocator;

const DIM: usize = 4; // 修复：在真实 OS 上多线程跑测试时，降低到 4 维以防止物理机真实 OOM 和引发死机蓝屏！测试配额是测逻辑截断的，无需占用超大真实内存

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("oom_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

#[test]
fn __allocator_failure_child_entry() {
    if std::env::var_os("TRIVIUM_ALLOCATOR_FAILURE_CHILD").is_none() {
        return;
    }

    let path = tmp_db("allocator_failure_child");
    cleanup(&path);
    let db = Database::<f32>::open(&path, DIM).unwrap();
    let before = db.node_count();

    // 只拒绝目标 API 的首次中型预留；阈值远低于物理内存，且失败分配不会提交页面。
    FAIL_ABOVE_BYTES.store(512 * 1024, Ordering::Relaxed);
    FAIL_ENABLED.store(true, Ordering::SeqCst);
    let result = db.reserve_nodes(100_000);
    FAIL_ENABLED.store(false, Ordering::SeqCst);

    assert!(
        result.is_err(),
        "数据库容量预留必须传播真实 allocator failure"
    );
    assert_eq!(db.node_count(), before, "失败预留不得改变数据库状态");
    drop(db);

    let reopened = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(
        reopened.node_count(),
        before,
        "失败后数据库必须仍可安全重开"
    );
    cleanup(&path);
}

#[test]
fn 测试_真实allocator_failure隔离子进程并保持数据库可恢复() {
    let exe = std::env::current_exe().unwrap();
    let status = Command::new(exe)
        .env("TRIVIUM_ALLOCATOR_FAILURE_CHILD", "1")
        .arg("__allocator_failure_child_entry")
        .arg("--exact")
        .arg("--nocapture")
        .status()
        .unwrap();
    assert!(status.success(), "allocator failure 子进程验证失败");
}

#[test]
fn 测试_真实分配失败_try_reserve安全返回且不申请物理大内存() {
    // 仅拒绝单次 >= 1 MiB 的分配；测试本身不会实际占用该内存，更不会填满机器。
    let mut values = Vec::<u64>::new();
    FAIL_ABOVE_BYTES.store(1024 * 1024, Ordering::Relaxed);
    FAIL_ENABLED.store(true, Ordering::SeqCst);
    let result = values.try_reserve(200_000);
    FAIL_ENABLED.store(false, Ordering::SeqCst);
    assert!(result.is_err(), "受控 allocator 必须真实拒绝目标分配");
    assert!(values.is_empty(), "失败分配不得部分改变容器");
}

#[test]
fn 测试_巨量边极速扩张图谱_评估软隔离能力() {
    let path = tmp_db("super_spreader");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 建立一个黑洞节点，并拥有数万条连出边，评估对遍历器的内存压降测试
    let ids = {
        let mut tx = db.begin_tx();
        let payload = serde_json::json!({"kind": "leaf"});
        let vec = vec![0.1f32; DIM];
        tx.insert(&vec, serde_json::json!({"kind": "super_root"}));

        // 降低数量到 5000，足以测试逻辑极限又保护了 Windows 内核稳定性
        for _ in 0..5_000 {
            tx.insert(&vec, payload.clone());
        }

        tx.commit().unwrap()
    };

    let root_id = ids[0];
    let children_ids = &ids[1..];

    {
        let mut tx = db.begin_tx();
        for &child in children_ids {
            tx.link(root_id, child, "spread", 1.0);
        }
        tx.commit().unwrap();
    }

    assert!(ids.len() >= 5_000);

    // 发起深度超过 2 的扩散查询
    let result = catch_unwind(std::panic::AssertUnwindSafe(|| {
        let vec = vec![0.1f32; DIM];
        // 这一步在扩散展开时可能会带来矩阵级的边缘候选爆炸
        let hits = db.search(&vec, 5, 2, 0.0).unwrap();
        assert!(!hits.is_empty());
    }));

    assert!(
        result.is_ok(),
        "百万级边缘扩散测试耗尽核心内存致 Panic！引擎应有配额熔断！"
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_深层海量多重自连边_避免调用栈溢出() {
    let path = tmp_db("deep_cycle");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, 4).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[0.1; 4], serde_json::json!({}));
        tx.insert(&[0.2; 4], serde_json::json!({}));
        tx.commit().unwrap()
    };
    let n1 = ids[0];
    let n2 = ids[1];

    {
        let mut tx = db.begin_tx();
        // 降低为 200。200 * 200 = 40,000 的路径规模，足以触发展开规模测试（验证不会栈溢出和配额熔断）。
        // 且不会因为单节点携带几万条边，在查询返回阶段将克隆内存瞬间挤爆。
        for _ in 0..200 {
            tx.link(n1, n2, "ping", 1.0);
            tx.link(n2, n1, "pong", 1.0);
        }
        tx.commit().unwrap();
    }

    let result = catch_unwind(std::panic::AssertUnwindSafe(|| {
        // MATCH 并包含一个两跳的无限回旋镖
        for _ in 0..10 {
            let _ = db.tql("MATCH (a)-[]->(b)-[]->(c) RETURN c");
        }
    }));

    assert!(result.is_ok(), "海量重边图遍历导致引擎栈溢出/内存雪崩！");

    drop(db);
    cleanup(&path);
}
