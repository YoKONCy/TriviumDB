use criterion::{black_box, criterion_group, criterion_main, Criterion, BatchSize};
use rand::Rng;
use triviumdb::Database;
use triviumdb::database::SearchConfig;
use serde_json::json;
use std::fs;

// 辅助方法：生成随机向量
fn generate_vector(dim: usize) -> Vec<f32> {
    let mut rng = rand::thread_rng();
    (0..dim).map(|_| rng.gen_range(-1.0..1.0)).collect()
}

// 辅助方法：生成一组点
fn generate_dataset(size: usize, dim: usize) -> Vec<Vec<f32>> {
    (0..size).map(|_| generate_vector(dim)).collect()
}

fn bench_inserts(c: &mut Criterion) {
    let dim = 128;
    let _ = fs::remove_file("bench_insert.tdb");
    let _ = fs::remove_file("bench_insert.tdb.wal");

    let mut db = Database::open("bench_insert.tdb", dim).unwrap();
    db.disable_auto_compaction();

    let payload = json!({"field": "benchmarking insert latency", "val": 42});

    c.bench_function("insert_with_wal", |b| {
        b.iter_batched(
            || generate_vector(dim),
            |vec| {
                db.insert(black_box(&vec), payload.clone()).unwrap()
            },
            BatchSize::SmallInput,
        )
    });

    let _ = fs::remove_file("bench_insert.tdb");
    let _ = fs::remove_file("bench_insert.tdb.wal");
}

fn bench_search(c: &mut Criterion) {
    let dim = 128;
    let db_path = "bench_search.tdb";
    let _ = fs::remove_file(db_path);
    let _ = fs::remove_file(format!("{}.wal", db_path));

    let mut db = Database::open(db_path, dim).unwrap();
    db.disable_auto_compaction();

    // 预加载 10000 个节点
    let data_size = 10000;
    println!("Pre-loading {} nodes for search benchmark...", data_size);
    let dataset = generate_dataset(data_size, dim);
    for vec in &dataset {
        db.insert(vec, json!({"test": 1})).unwrap();
    }
    // 强制落盘，让搜索测试走 mmap（如果有相关逻辑）或者纯内存
    db.flush().unwrap();
    drop(db); // Windows 下必需释放文件系统锁，否则后续 open 会崩溃
    
    // 打开只读模式测试读取
    let db_read = Database::open(db_path, dim).unwrap();

    let query = generate_vector(dim);

    c.bench_function(&format!("search_10k_dim{}", dim), |b| {
        b.iter(|| {
            db_read.search(black_box(&query), 10, 0, 0.0).unwrap()
        })
    });

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_file(format!("{}.wal", db_path));
}

fn bench_hybrid_search(c: &mut Criterion) {
    let dim = 128;
    let db_path = "bench_hybrid.tdb";
    let _ = fs::remove_file(db_path);
    let _ = fs::remove_file(format!("{}.wal", db_path));

    let mut db = Database::open(db_path, dim).unwrap();
    db.disable_auto_compaction();

    let data_size = 5000;
    let mut ids = Vec::with_capacity(data_size);
    for _ in 0..data_size {
        let id = db.insert(&generate_vector(dim), json!({"tag": "base"})).unwrap();
        ids.push(id);
    }
    
    // 生成一些网络边以测试扩线能力
    let mut rng = rand::thread_rng();
    for i in 0..data_size {
        // 每个节点随即连 2 个节点
        for _ in 0..2 {
            let target = ids[rng.gen_range(0..data_size)];
            db.link(ids[i], target, "related", 1.0).unwrap();
        }
    }
    db.flush().unwrap();
    drop(db);

    let query = generate_vector(dim);

    let db_read = Database::open(db_path, dim).unwrap();

    c.bench_function("hybrid_search_expand_2", |b| {
        b.iter(|| {
            // 图扩散深度 = 2
            db_read.search(black_box(&query), 5, 2, 0.0).unwrap()
        })
    });

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_file(format!("{}.wal", db_path));
}

fn bench_cognitive_pipeline(c: &mut Criterion) {
    let dim = 128;
    let db_path = "bench_cognitive.tdb";
    let _ = fs::remove_file(db_path);
    let _ = fs::remove_file(format!("{}.wal", db_path));

    let mut db = Database::open(db_path, dim).unwrap();
    db.disable_auto_compaction();

    let data_size = 5000;
    println!("Pre-loading {} nodes for cognitive pipeline benchmark...", data_size);
    let dataset = generate_dataset(data_size, dim);
    let mut ids = Vec::with_capacity(data_size);
    for vec in &dataset {
        ids.push(db.insert(vec, json!({"cognitive": "bench"})).unwrap());
    }
    
    // 生成关联网络以测试漫游
    let mut rng = rand::thread_rng();
    for _ in 0..(data_size * 2) {
        let src = ids[rng.gen_range(0..data_size)];
        let dst = ids[rng.gen_range(0..data_size)];
        db.link(src, dst, "synapse", 0.8).unwrap();
    }
    db.flush().unwrap();
    drop(db);

    let query = generate_vector(dim);
    let config = SearchConfig {
        top_k: 10,
        expand_depth: 2,
        min_score: 0.1,
        teleport_alpha: 0.2, // 启用 PPR 返回跳散
        enable_advanced_pipeline: true,
        enable_sparse_residual: true, // 启用 FISTA 影子查询
        fista_lambda: 0.1,
        fista_threshold: 0.2, // 容易触发残差查询
        enable_dpp: true, // 启用多样性采样
        dpp_quality_weight: 1.0,
        ..Default::default()
    };

    let db_read = Database::open(db_path, dim).unwrap();

    c.bench_function("cognitive_search_advanced", |b| {
        b.iter(|| {
            db_read.search_advanced(black_box(&query), &config).unwrap()
        })
    });

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_file(format!("{}.wal", db_path));
}

criterion_group!(benches, bench_inserts, bench_search, bench_hybrid_search, bench_cognitive_pipeline);
criterion_main!(benches);
