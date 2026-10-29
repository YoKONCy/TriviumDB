use std::path::PathBuf;
use triviumdb::database::{Config, Database, SearchConfig, StorageMode};
use triviumdb::storage::memtable::auto_quiver_node_threshold;
use triviumdb::{VectorType, hook::HookContext};

fn directory() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "tdb-quiver-threshold-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).unwrap();
    path
}

fn route(context: &HookContext) -> u64 {
    context
        .observations
        .iter()
        .find(|(name, _)| name == "vector_route_quiver")
        .expect("查询必须报告实际向量路由")
        .1
}

fn vector<T: VectorType>(id: usize, dim: usize) -> Vec<T> {
    let mut state = (id as u32).wrapping_mul(747_796_405).wrapping_add(1);
    (0..dim)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            T::from_f32((state % 1024 + 1) as f32)
        })
        .collect()
}

fn threshold_route<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(dim: usize) {
    for storage_mode in [StorageMode::Mmap, StorageMode::Rom] {
        let root = directory();
        let path = root.join("sample.tdb");
        let mut db = Database::<T>::open_with_config(
            &path.to_string_lossy(),
            Config {
                dim,
                storage_mode,
                ..Default::default()
            },
        )
        .unwrap();
        let threshold = auto_quiver_node_threshold(dim);
        for id in 1..threshold {
            db.insert_with_id(
                id as u64,
                &vector::<T>(id, dim),
                serde_json::json!({"id":id}),
            )
            .unwrap();
        }
        let query = vector::<T>(1, dim);
        let config = SearchConfig {
            top_k: 5,
            expand_depth: 0,
            min_score: -1.0,
            ..Default::default()
        };
        let (before, context) = db
            .search_hybrid_with_context(None, Some(&query), &config)
            .unwrap();
        assert_eq!(route(&context), 0);
        assert_eq!(before.len(), 5);
        db.insert_with_id(
            threshold as u64,
            &vector::<T>(threshold, dim),
            serde_json::json!({"id":threshold}),
        )
        .unwrap();
        db.set_auto_build_quiver(false).unwrap();
        let (_, context) = db
            .search_hybrid_with_context(None, Some(&query), &config)
            .unwrap();
        assert_eq!(route(&context), 0);
        db.set_auto_build_quiver(true).unwrap();
        let (hits, context) = db
            .search_hybrid_with_context(None, Some(&query), &config)
            .unwrap();
        let binary = std::any::TypeId::of::<T>() == std::any::TypeId::of::<u64>();
        assert_eq!(route(&context), if binary { 0 } else { 1 });
        assert_eq!(hits.len(), 5);
        let mut ids = std::collections::HashSet::new();
        for hit in &hits {
            assert!(ids.insert(hit.id));
            assert!(db.contains(hit.id).unwrap());
            assert!(hit.score.is_finite());
        }
        // 显式精确检索保持各 dtype 的公开评分契约。
        let exact = db.search_exact(&query, 1).unwrap();
        let expected = if std::any::TypeId::of::<T>() == std::any::TypeId::of::<u64>() {
            (dim * 64) as f32
        } else {
            1.0
        };
        assert!((exact[0].score - expected).abs() < 1e-5);
        db.close().unwrap();
        let mut reopened = Database::<T>::open_with_config(
            &path.to_string_lossy(),
            Config {
                dim,
                storage_mode,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        let (_, context) = reopened
            .search_hybrid_with_context(None, Some(&query), &config)
            .unwrap();
        assert_eq!(route(&context), if binary { 0 } else { 1 });
        assert_eq!(reopened.node_count().unwrap(), threshold);
        reopened.close().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn 浮点dtype按阈值切换且二进制dtype保持汉明路由并可重载() {
    threshold_route::<f32>(128);
    threshold_route::<half::f16>(768);
    threshold_route::<u64>(1536);
}

#[test]
fn 最高合法维度在新阈值实际切换() {
    threshold_route::<f32>(3072);
}
