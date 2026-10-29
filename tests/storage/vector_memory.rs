#![allow(non_snake_case)]

use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use triviumdb::database::{Config, Database, SearchConfig, StorageMode};
use triviumdb::index::bq::BqSignature;
use triviumdb::storage::{file_format, memtable::MemTable, vec_pool::VecPool};

struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "triviumdb_vector_memory_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        // 只清理本测试独占目录的直接文件，不遍历其他测试数据。
        if let Ok(entries) = std::fs::read_dir(&self.0) {
            for entry in entries.flatten() {
                std::fs::remove_file(entry.path()).ok();
            }
        }
        std::fs::remove_dir(&self.0).ok();
    }
}

#[test]
fn 旧BQ视图按需物化并计入预算_写操作使其失效() {
    let mut mt = MemTable::<f32>::new(65);
    for id in 1..=128 {
        mt.insert_with_id(id, &vec![1.; 65], json!({})).unwrap();
    }
    mt.prepare_search_cache(false);
    mt.prepare_persistence_cache(false);
    assert_eq!(mt.bq_cache_bytes(), 0);
    assert_eq!(
        mt.get_bq_signature(0),
        Some(BqSignature::from_vector(&vec![1.; 65]))
    );
    assert_eq!(mt.bq_cache_bytes(), 0, "单条请求不能分配整库旧缓存");
    let before = mt.estimated_memory_bytes();
    assert_eq!(mt.bq_signatures_slice().len(), 128);
    assert!(mt.bq_cache_bytes() >= 128 * 384);
    assert_eq!(mt.estimated_memory_bytes() - before, mt.bq_cache_bytes());
    mt.update_vector(1, &vec![-1.; 65]).unwrap();
    assert_eq!(mt.bq_cache_bytes(), 0);
    assert_eq!(
        mt.get_bq_signature(0),
        Some(BqSignature::from_vector(&vec![-1.; 65]))
    );
}

#[test]
fn 两种持久化模式均不写每节点旧BQ_重载不物化旧数组() {
    let dir = TestDir::new();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = dir.file(&format!("{mode:?}.tdb"));
        let mut mt = MemTable::<f32>::new(65);
        for id in 1..=64 {
            mt.insert_with_id(id, &vec![1.; 65], json!({"id":id,"group":id%3}))
                .unwrap();
        }
        file_format::save(&mut mt, &path, mode).unwrap();
        assert_eq!(mt.bq_cache_bytes(), 0);
        let bytes = std::fs::read(&path).unwrap();
        let offset = u64::from_le_bytes(bytes[50..58].try_into().unwrap()) as usize;
        assert_eq!(&bytes[offset..offset + 4], b"TBQF");
        assert_eq!(
            u64::from_le_bytes(bytes[offset + 8..offset + 16].try_into().unwrap()),
            0
        );
        drop(mt);
        let loaded = file_format::load::<f32>(
            &path,
            mode,
            false,
            false,
            triviumdb::database::MissingIndexPolicy::Fallback,
            false,
        )
        .unwrap();
        assert_eq!(loaded.node_count(), 64);
        assert_eq!(loaded.bq_cache_bytes(), 0);
    }
}

#[test]
fn mmap直接连续视图零堆副本_混合层按需合并后正确失效和转Rom() {
    let dir = TestDir::new();
    let path = dir.file("vectors.vec");
    let mut pool = VecPool::<f32>::new(3);
    pool.push(&[1., 2., 3.]);
    pool.push(&[4., 5., 6.]);
    pool.flush(std::path::Path::new(&path)).unwrap();
    pool.ensure_cache();
    assert_eq!(pool.flat_vectors(), &[1., 2., 3., 4., 5., 6.]);
    assert_eq!(pool.heap_memory_bytes(), 0);
    pool.push(&[7., 8., 9.]);
    let (base, delta) = pool.vector_segments();
    assert_eq!(base, &[1., 2., 3., 4., 5., 6.]);
    assert_eq!(delta, &[7., 8., 9.]);
    let delta_heap = pool.heap_memory_bytes();
    pool.ensure_cache();
    assert!(pool.heap_memory_bytes() > delta_heap);
    pool.update(0, &[9., 2., 3.]);
    assert_eq!(pool.heap_memory_bytes(), delta_heap, "写入应释放已失效副本");
    assert_eq!(pool.vector_segments().0[0], 9.);
    let disk = std::fs::read(&path).unwrap();
    assert_eq!(
        f32::from_le_bytes(disk[..4].try_into().unwrap()),
        1.,
        "COW 写入不得修改磁盘"
    );
    pool.ensure_cache();
    let expected = pool.flat_vectors().to_vec();
    pool.detach_mmap();
    assert_eq!(pool.delta_raw(), expected);
    assert_eq!(pool.heap_memory_bytes(), expected.len() * 4);
}

#[test]
fn mmap与增量及墓碑复用_精确查询等同Rom且不创建全库缓存() {
    let dir = TestDir::new();
    let mut mapped = Database::<f32>::open(&dir.file("mapped.tdb"), 65).unwrap();
    let mut owned = Database::<f32>::open_with_config(
        &dir.file("owned.tdb"),
        Config {
            dim: 65,
            storage_mode: StorageMode::Rom,
            ..Default::default()
        },
    )
    .unwrap();
    for id in 1..=128u64 {
        let vector: Vec<_> = (0..65)
            .map(|j| ((id * 13 + j * 7) % 71) as f32 / 71.)
            .collect();
        for db in [&mut mapped, &mut owned] {
            db.insert_with_id(id, &vector, json!({"id":id,"group":id%3}))
                .unwrap();
        }
    }
    mapped.flush().unwrap();
    for db in [&mut mapped, &mut owned] {
        db.delete(5).unwrap();
        db.insert_with_id(500, &vec![0.3; 65], json!({"id":500}))
            .unwrap();
        db.insert_with_id(501, &vec![0.7; 65], json!({"id":501}))
            .unwrap();
        db.update_vector(1, &vec![0.9; 65]).unwrap();
    }
    mapped.build_quiver_index(None).unwrap();
    owned.build_quiver_index(None).unwrap();
    let before = mapped.estimated_memory();
    let mut config = SearchConfig {
        top_k: 12,
        recall_k: 24,
        rerank_k: 24,
        force_brute_force: true,
        enable_advanced_pipeline: false,
        min_score: -1.,
        ..Default::default()
    };
    for (force, filter) in [(true, false), (true, true), (false, false), (false, true)] {
        config.force_brute_force = force;
        config.payload_filter = filter.then(|| triviumdb::Filter::eq("group", json!(1)));
        for i in 1..=7u64 {
            let query: Vec<_> = (0..65)
                .map(|j| ((i * 17 + j * 3) % 67) as f32 / 67.)
                .collect();
            let a = mapped.search_advanced(&query, &config).unwrap();
            let b = owned.search_advanced(&query, &config).unwrap();
            assert_eq!(
                a.iter().map(|h| (h.id, h.score)).collect::<Vec<_>>(),
                b.iter().map(|h| (h.id, h.score)).collect::<Vec<_>>()
            );
        }
    }
    let after = mapped.estimated_memory();
    // Payload 解析可增加少量缓存，但不能多出整库向量或旧 BQ 数组。
    assert!(
        after.saturating_sub(before) < 128 * 65 * 4,
        "查询意外物化全库缓存: {before} -> {after}"
    );
}

#[test]
fn 分段映射支持FP16和u64的长度与对齐() {
    fn check<T: triviumdb::VectorType>() {
        let dir = TestDir::new();
        let mut pool = VecPool::<T>::new(65);
        let vector = vec![T::from_f32(1.); 65];
        pool.push(&vector);
        let path = dir.file("typed.vec");
        pool.flush(std::path::Path::new(&path)).unwrap();
        pool.ensure_cache();
        assert_eq!(
            bytemuck::cast_slice::<T, u8>(pool.flat_vectors()),
            bytemuck::cast_slice::<T, u8>(&vector)
        );
        assert_eq!(pool.heap_memory_bytes(), 0);
        pool.push(&vector);
        assert_eq!(pool.vector_segments().0.len(), 65);
        assert_eq!(pool.vector_segments().1.len(), 65);
    }
    check::<half::f16>();
    check::<u64>();
}

#[test]
fn 未物化混合层直接转Rom保留COW和增量() {
    let dir = TestDir::new();
    let path = dir.file("detach.vec");
    let mut pool = VecPool::<f32>::new(3);
    pool.push(&[1., 2., 3.]);
    pool.flush(std::path::Path::new(&path)).unwrap();
    pool.update(0, &[4., 5., 6.]);
    pool.push(&[7., 8., 9.]);
    pool.detach_mmap();
    assert_eq!(pool.flat_vectors(), &[4., 5., 6., 7., 8., 9.]);
    assert!(pool.vector_segments().0.is_empty());
    assert_eq!(pool.heap_memory_bytes(), 6 * 4);
    assert_eq!(pool.mmap_bytes(), 0);
}

#[test]
fn 映射几何溢出及零维在分配前拒绝() {
    let dir = TestDir::new();
    let path = dir.file("invalid.vec");
    std::fs::write(&path, [0u8; 8]).unwrap();
    for (dim, count) in [(usize::MAX, 2), (2, usize::MAX), (0, 1), (3, 1)] {
        assert!(matches!(
            VecPool::<f32>::open(std::path::Path::new(&path), dim, count),
            Err(triviumdb::TriviumError::CorruptedFile(_))
        ));
    }
}

#[test]
fn 冷页回收保留COW更新与增量并可落盘重开() {
    let dir = TestDir::new();
    let path = dir.file("cold-pages.vec");
    let mut pool = VecPool::<f32>::new(3);
    pool.push(&[1., 2., 3.]);
    pool.push(&[4., 5., 6.]);
    pool.flush(std::path::Path::new(&path)).unwrap();
    pool.advise_dontneed();
    assert_eq!(pool.get(0).unwrap(), &[1., 2., 3.]);
    pool.update(0, &[7., 8., 9.]);
    pool.push(&[10., 11., 12.]);
    pool.advise_dontneed();
    assert_eq!(pool.get(0).unwrap(), &[7., 8., 9.]);
    assert_eq!(pool.get(1).unwrap(), &[4., 5., 6.]);
    assert_eq!(pool.get(2).unwrap(), &[10., 11., 12.]);
    let disk = std::fs::read(&path).unwrap();
    assert_eq!(f32::from_le_bytes(disk[..4].try_into().unwrap()), 1.);
    pool.flush(std::path::Path::new(&path)).unwrap();
    drop(pool);
    let pool = VecPool::<f32>::open(std::path::Path::new(&path), 3, 3).unwrap();
    pool.advise_dontneed();
    assert_eq!(pool.get(0).unwrap(), &[7., 8., 9.]);
    assert_eq!(pool.get(2).unwrap(), &[10., 11., 12.]);
}

#[test]
fn wal与检查点索引恢复从开始遵守禁用解析缓存的配置() {
    fn check<T: triviumdb::VectorType + serde::Serialize + serde::de::DeserializeOwned>() {
        use triviumdb::storage::wal::{SyncMode, Wal, WalEntry};
        let dir = TestDir::new();
        for mode in [StorageMode::Rom, StorageMode::Mmap] {
            for checkpoint in [false, true] {
                let path = dir.file(&format!("{mode:?}-{checkpoint}.tdb"));
                let config = Config {
                    dim: 4,
                    storage_mode: mode,
                    auto_build_quiver: false,
                    payload_cache_bytes: 0,
                    ..Default::default()
                };
                let vector = [T::from_f32(1.); 4];
                if checkpoint {
                    let mut db = Database::<T>::open_with_config(&path, config).unwrap();
                    for id in 1..=128 {
                        db.insert_with_id(id, &vector, json!({"id":id})).unwrap();
                    }
                    db.create_unique_index("id").unwrap();
                    db.close().unwrap();
                } else {
                    Database::<T>::open_with_config(&path, config)
                        .unwrap()
                        .close()
                        .unwrap();
                    let mut wal = Wal::open_with_sync(&path, SyncMode::Off).unwrap();
                    for id in 1..=128 {
                        wal.append(&WalEntry::Insert {
                            id,
                            vector: vector.to_vec(),
                            payload: json!({"id":id}).to_string(),
                        })
                        .unwrap();
                    }
                    wal.flush_writer();
                }
                let mut db = Database::<T>::open_with_config(&path, config).unwrap();
                assert_eq!(db.node_count().unwrap(), 128);
                let stats = db.payload_memory_stats();
                assert_eq!(stats.parsed_cache_entries, 0);
                assert_eq!(stats.parsed_cache_bytes, 0);
                assert_eq!(
                    stats.cache_evictions, 0,
                    "禁用缓存时，恢复不得先填充默认缓存再淘汰"
                );
                for id in 1..=128 {
                    assert_eq!(
                        db.get_payload(id).unwrap().unwrap().get("id"),
                        Some(&json!(id))
                    );
                }
                if checkpoint {
                    assert!(
                        db.insert(&vector, json!({"id":1})).is_err(),
                        "唯一约束必须恢复"
                    );
                }
                db.close().unwrap();
            }
        }
    }
    check::<f32>();
    check::<half::f16>();
    check::<u64>();
}
