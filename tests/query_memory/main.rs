//! 按查询线程的实际分配测量快照峰值；按分配时的 epoch 区分测试前已持有的内存。
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use triviumdb::database::{Config, StorageMode};
use triviumdb::{Database, TriviumError};

#[path = "../support/mod.rs"]
mod support;

thread_local! { static METER: Cell<(usize, usize, usize)> = const { Cell::new((0,0,0)) }; }
static EPOCH: AtomicUsize = AtomicUsize::new(1);
struct Metered;
#[global_allocator]
static ALLOCATOR: Metered = Metered;

fn padded(layout: Layout) -> Option<(Layout, usize)> {
    let prefix = layout.align().max(std::mem::size_of::<usize>());
    Some((
        Layout::from_size_align(layout.size().checked_add(prefix)?, layout.align()).ok()?,
        prefix,
    ))
}

// SAFETY: 每次 System 分配均额外保留对齐前缀；用户指针仍满足原 Layout。
// 释放和重分配恢复相同基址及总长度；epoch 头不属于用户可访问的内存范围。
unsafe impl GlobalAlloc for Metered {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some((actual, prefix)) = padded(layout) else {
            return std::ptr::null_mut();
        };
        // SAFETY: actual 来自有效 Layout，成功时可读写整个分配区。
        let base = unsafe { System.alloc(actual) };
        if base.is_null() {
            return base;
        }
        let epoch = METER
            .try_with(|meter| {
                let (epoch, used, peak) = meter.get();
                if epoch != 0 {
                    meter.set((epoch, used + layout.size(), peak.max(used + layout.size())));
                }
                epoch
            })
            .unwrap_or(0);
        // SAFETY: 前缀至少可容纳 usize，非对齐写避免假设小对齐分配的基址对齐。
        unsafe {
            (base as *mut usize).write_unaligned(epoch);
            base.add(prefix)
        }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let (actual, prefix) = padded(layout).expect("已分配对象必须能重建布局");
        // SAFETY: pointer 来自本分配器，前缀及头部仍在有效分配内。
        let base = unsafe { pointer.sub(prefix) };
        let owner = unsafe { (base as *const usize).read_unaligned() };
        let _ = METER.try_with(|meter| {
            let (epoch, used, peak) = meter.get();
            if owner != 0 && owner == epoch {
                meter.set((epoch, used - layout.size(), peak));
            }
        });
        // SAFETY: 基址和 actual 与对应 alloc 完全相同，且只释放一次。
        unsafe { System.dealloc(base, actual) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let Ok(next) = Layout::from_size_align(size, layout.align()) else {
            return std::ptr::null_mut();
        };
        // SAFETY: 新旧布局有效；失败保留旧指针，成功复制双方有效长度后释放旧对象。
        unsafe {
            let replacement = self.alloc(next);
            if !replacement.is_null() {
                std::ptr::copy_nonoverlapping(pointer, replacement, layout.size().min(size));
                self.dealloc(pointer, layout);
            }
            replacement
        }
    }
}

fn measured<T>(action: impl FnOnce() -> T) -> (T, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            METER.with(|meter| meter.set((0, 0, 0)));
        }
    }
    let _reset = Reset;
    METER.with(|meter| meter.set((EPOCH.fetch_add(1, Ordering::Relaxed), 0, 0)));
    let result = action();
    let peak = METER.with(|meter| meter.get().2);
    (result, peak)
}

fn sample(path: &str, mode: StorageMode) -> Database<f32> {
    let mut db = Database::open_with_config(
        path,
        Config {
            dim: 2,
            storage_mode: mode,
            auto_build_quiver: false,
            payload_cache_bytes: 0,
            ..Default::default()
        },
    )
    .unwrap();
    for id in 1..=20 {
        db.insert_with_id(
            id,
            &[1., 0.],
            serde_json::json!({"id":id,"blob":"x".repeat(2048)}),
        )
        .unwrap();
    }
    for source in 1..=20 {
        for target in 1..=20 {
            db.link(source, target, "rel", 1.).unwrap();
        }
    }
    db
}

fn rejected<T: std::fmt::Debug>(result: triviumdb::Result<T>) {
    assert!(
        matches!(
            result,
            Err(TriviumError::ResourceBudgetExceeded {
                resource: "query_memory_bytes",
                ..
            })
        ),
        "{result:?}"
    );
}

#[test]
fn 多变量快照和写匹配在复制前拒绝并限制实际分配峰值() {
    // 512 KiB 是实例查询上限；20×20 条匹配会重复快照 Payload 与整份邻接表。
    const LIMIT: usize = 512 * 1024;
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = support::TestDir::new("tql_heap_admission");
        let mut db = sample(&root.file("data.tdb"), mode);
        db.set_memory_limit(LIMIT).unwrap();
        for query in [
            "MATCH (a)-[:rel]->(b) RETURN a,b",
            "MATCH (a)-[e:rel]->(b) RETURN a,b,e",
            "OPTIONAL MATCH (a)-[:rel]->(b) RETURN a,b",
        ] {
            let (result, peak) = measured(|| db.tql_values(query));
            rejected(result);
            assert!(
                peak <= LIMIT,
                "{mode:?}/{query}: 实际查询线程峰值 {peak} 超过 {LIMIT}"
            );
        }
        let (result, peak) = measured(|| db.tql_mut("MATCH (a)-[:rel]->(b) SET a.touched == true"));
        rejected(result);
        assert!(peak <= LIMIT, "写查询实际峰值 {peak} 超过 {LIMIT}");
        for id in 1..=20 {
            assert!(
                db.get_payload(id)
                    .unwrap()
                    .unwrap()
                    .get("touched")
                    .is_none()
            );
        }
        db.set_memory_limit(0).unwrap();
        let rows = db
            .tql_values("MATCH (a)-[:rel]->(b) RETURN COUNT(*) AS total")
            .unwrap();
        assert!(matches!(
            rows[0]["total"],
            triviumdb::query::tql_executor::TqlValue::Int(400)
        ));
        assert_eq!(
            db.tql_mut("MATCH (a)-[:rel]->(b) SET a.touched == true")
                .unwrap()
                .affected,
            20
        );
        db.close().unwrap();
        let mut reopened = Database::<f32>::open(&root.file("data.tdb"), 2).unwrap();
        for id in 1..=20 {
            assert_eq!(reopened.get_payload(id).unwrap().unwrap()["touched"], true);
        }
        reopened.close().unwrap();
    }
}
