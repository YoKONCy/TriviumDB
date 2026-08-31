//! 数据库公共查询路径的 Criterion 微基准。
//!
//! 覆盖向量检索、Payload 过滤与图扩散的热路径，固定维度和数据规模以观察代码回归。
//! 绝对数值只在相同硬件、构建配置和数据生成参数下可比较。

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use rand::Rng;
use serde_json::json;
use std::fs;
use triviumdb::Database;

const DIM: usize = 128;

fn generate_vector(dim: usize) -> Vec<f32> {
    let mut rng = rand::thread_rng();
    (0..dim).map(|_| rng.gen_range(-1.0..1.0)).collect()
}

fn db_path(name: &str) -> String {
    format!("bench_query_{}.tdb", name)
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

/// 创建带丰富 payload + 图结构的测试数据库
fn build_test_db(name: &str, node_count: usize) -> Database<f32> {
    let path = db_path(name);
    cleanup(&path);
    let mut db = Database::open(&path, DIM).unwrap();
    db.disable_auto_compaction();

    let categories = ["event", "person", "concept", "document", "task"];
    let mut rng = rand::thread_rng();
    let mut ids = Vec::with_capacity(node_count);

    for i in 0..node_count {
        let cat = categories[i % categories.len()];
        let id = db
            .insert(
                &generate_vector(DIM),
                json!({
                    "type": cat,
                    "name": format!("node_{}", i),
                    "score": rng.gen_range(0.0..1.0_f64),
                    "priority": rng.gen_range(1..=10_u32),
                    "active": rng.gen_bool(0.7),
                }),
            )
            .unwrap();
        ids.push(id);
    }

    // 建边 — 每节点平均 3 条出边
    for i in 0..node_count {
        for _ in 0..3 {
            let target = ids[rng.gen_range(0..node_count)];
            if ids[i] != target {
                db.link(
                    ids[i],
                    target,
                    categories[rng.gen_range(0..categories.len())],
                    rng.gen_range(0.1..1.0),
                )
                .unwrap();
            }
        }
    }

    db.flush().unwrap();
    drop(db);
    Database::open(&path, DIM).unwrap()
}

// ════════════════════════════════════════════════════════════════
//  TQL 只读查询 Benchmarks
// ════════════════════════════════════════════════════════════════

fn bench_tql_match_by_id(c: &mut Criterion) {
    let db = build_test_db("tql_match_id", 5000);
    let ids = db.all_node_ids();
    let anchor = ids[ids.len() / 2];

    c.bench_function("tql_match_by_id_1hop", |b| {
        let q = format!("MATCH (a {{id: {}}})-[]->(b) RETURN b", anchor);
        b.iter(|| db.tql(black_box(&q)).unwrap())
    });

    cleanup(&db_path("tql_match_id"));
}

fn bench_tql_match_by_prop(c: &mut Criterion) {
    let db = build_test_db("tql_match_prop", 5000);

    c.bench_function("tql_match_by_prop_no_index", |b| {
        b.iter(|| {
            db.tql(black_box("MATCH (a {type: \"event\"})-[]->(b) RETURN b"))
                .unwrap()
        })
    });

    cleanup(&db_path("tql_match_prop"));
}

fn bench_tql_match_with_where(c: &mut Criterion) {
    let db = build_test_db("tql_match_where", 5000);
    let ids = db.all_node_ids();
    let anchor = ids[0];

    c.bench_function("tql_match_where_clause", |b| {
        let q = format!(
            "MATCH (a {{id: {}}})-[]->(b) WHERE b.priority > 5 RETURN b",
            anchor
        );
        b.iter(|| db.tql(black_box(&q)).unwrap())
    });

    cleanup(&db_path("tql_match_where"));
}

fn bench_tql_find_no_index(c: &mut Criterion) {
    let db = build_test_db("tql_find_noidx", 5000);

    c.bench_function("tql_find_eq_no_index_5k", |b| {
        b.iter(|| {
            db.tql(black_box("FIND {type: \"person\"} RETURN *"))
                .unwrap()
        })
    });

    cleanup(&db_path("tql_find_noidx"));
}

fn bench_tql_find_with_index(c: &mut Criterion) {
    let path = db_path("tql_find_idx");
    cleanup(&path);

    // 先建 DB 再关闭重开（Windows 文件锁）
    {
        let db = build_test_db("tql_find_idx", 5000);
        drop(db);
    }

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.create_index("type").unwrap();

    c.bench_function("tql_find_eq_with_index_5k", |b| {
        b.iter(|| {
            db.tql(black_box("FIND {type: \"person\"} RETURN *"))
                .unwrap()
        })
    });

    drop(db);
    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  TQL DML 写操作 Benchmarks
// ════════════════════════════════════════════════════════════════

fn bench_tql_create(c: &mut Criterion) {
    let path = db_path("tql_create");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.disable_auto_compaction();

    let mut counter = 0u64;
    c.bench_function("tql_mut_create", |b| {
        b.iter(|| {
            counter += 1;
            let q = format!("CREATE (a {{name: \"bench_{}\", type: \"temp\"}})", counter);
            db.tql_mut(black_box(&q)).unwrap()
        })
    });

    drop(db);
    cleanup(&path);
}

fn bench_tql_set(c: &mut Criterion) {
    let path = db_path("tql_set");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.disable_auto_compaction();

    // 预插入节点
    for i in 0..1000u32 {
        db.insert(
            &generate_vector(DIM),
            json!({"name": format!("n{}", i), "val": 0}),
        )
        .unwrap();
    }

    let ids = db.all_node_ids();
    let mut idx = 0usize;

    c.bench_function("tql_mut_set_by_id", |b| {
        b.iter(|| {
            let id = ids[idx % ids.len()];
            idx += 1;
            let q = format!("MATCH (a {{id: {}}}) SET a.val == {}", id, idx);
            db.tql_mut(black_box(&q)).unwrap()
        })
    });

    drop(db);
    cleanup(&path);
}

fn bench_tql_delete(c: &mut Criterion) {
    let path = db_path("tql_delete");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.disable_auto_compaction();

    // 预灌一批一次性节点
    for i in 0..100u32 {
        db.insert(
            &generate_vector(DIM),
            json!({"type": "disposable", "seq": i}),
        )
        .unwrap();
    }

    c.bench_function("tql_mut_detach_delete", |b| {
        b.iter(|| {
            // 删除后重新插入以保持可重复
            let _ = db.tql_mut(black_box(
                "MATCH (a {type: \"disposable\"}) DETACH DELETE a",
            ));
            for i in 0..10u32 {
                db.insert(
                    &generate_vector(DIM),
                    json!({"type": "disposable", "seq": i}),
                )
                .unwrap();
            }
        })
    });

    drop(db);
    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  索引操作 Benchmarks
// ════════════════════════════════════════════════════════════════

fn bench_create_index(c: &mut Criterion) {
    let path = db_path("idx_create");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.disable_auto_compaction();

    // 预插入 5000 节点
    for i in 0..5000u32 {
        db.insert(
            &generate_vector(DIM),
            json!({"category": format!("cat_{}", i % 50), "val": i}),
        )
        .unwrap();
    }

    c.bench_function("create_index_5k_nodes", |b| {
        b.iter(|| {
            db.create_index(black_box("category")).unwrap();
            db.drop_index("category").unwrap();
        })
    });

    drop(db);
    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  CRUD 快速路径 Benchmarks
// ════════════════════════════════════════════════════════════════

fn bench_get_payload(c: &mut Criterion) {
    let db = build_test_db("get_payload", 5000);
    let ids = db.all_node_ids();

    c.bench_function("get_payload_5k", |b| {
        let mut idx = 0usize;
        b.iter(|| {
            let id = ids[idx % ids.len()];
            idx += 1;
            black_box(db.get_payload(id))
        })
    });

    cleanup(&db_path("get_payload"));
}

fn bench_get_edges(c: &mut Criterion) {
    let db = build_test_db("get_edges", 5000);
    let ids = db.all_node_ids();

    c.bench_function("get_edges_5k", |b| {
        let mut idx = 0usize;
        b.iter(|| {
            let id = ids[idx % ids.len()];
            idx += 1;
            black_box(db.get_edges(id))
        })
    });

    cleanup(&db_path("get_edges"));
}

fn bench_contains(c: &mut Criterion) {
    let db = build_test_db("contains", 5000);
    let ids = db.all_node_ids();

    c.bench_function("contains_5k", |b| {
        let mut idx = 0usize;
        b.iter(|| {
            let id = ids[idx % ids.len()];
            idx += 1;
            black_box(db.contains(id))
        })
    });

    cleanup(&db_path("contains"));
}

// ════════════════════════════════════════════════════════════════
//  过滤查询 Benchmarks
// ════════════════════════════════════════════════════════════════

fn bench_filter_via_tql(c: &mut Criterion) {
    let db = build_test_db("filter_tql", 5000);

    c.bench_function("tql_find_eq_5k", |b| {
        b.iter(|| {
            db.tql(black_box("FIND {type: \"event\"} RETURN *"))
                .unwrap()
        })
    });

    cleanup(&db_path("filter_tql"));
}

fn bench_filter_complex_via_tql(c: &mut Criterion) {
    let db = build_test_db("filter_complex_tql", 5000);

    c.bench_function("tql_find_complex_5k", |b| {
        b.iter(|| {
            db.tql(black_box("FIND {type: \"event\", active: true} RETURN *"))
                .unwrap()
        })
    });

    cleanup(&db_path("filter_complex_tql"));
}

// ════════════════════════════════════════════════════════════════
//  TQL vs 原始 API 对比组
// ════════════════════════════════════════════════════════════════

fn bench_tql_vs_raw_search(c: &mut Criterion) {
    let db = build_test_db("tql_vs_raw", 5000);
    let query = generate_vector(DIM);

    let mut group = c.benchmark_group("search_comparison_5k");

    group.bench_function("raw_search_top10", |b| {
        b.iter(|| db.search(black_box(&query), 10, 0, 0.0).unwrap())
    });

    group.bench_function("raw_search_top10_expand2", |b| {
        b.iter(|| db.search(black_box(&query), 10, 2, 0.0).unwrap())
    });

    group.finish();
    cleanup(&db_path("tql_vs_raw"));
}

fn bench_index_vs_no_index(c: &mut Criterion) {
    let path = db_path("idx_cmp");
    cleanup(&path);

    {
        let db = build_test_db("idx_cmp", 10000);
        drop(db);
    }

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let mut group = c.benchmark_group("index_comparison_10k");

    group.bench_function("find_type_no_index", |b| {
        b.iter(|| {
            db.tql(black_box("FIND {type: \"event\"} RETURN *"))
                .unwrap()
        })
    });

    db.create_index("type").unwrap();

    group.bench_function("find_type_with_index", |b| {
        b.iter(|| {
            db.tql(black_box("FIND {type: \"event\"} RETURN *"))
                .unwrap()
        })
    });

    group.finish();
    drop(db);
    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════

criterion_group!(
    query_benches,
    // TQL 只读
    bench_tql_match_by_id,
    bench_tql_match_by_prop,
    bench_tql_match_with_where,
    bench_tql_find_no_index,
    bench_tql_find_with_index,
    // TQL DML
    bench_tql_create,
    bench_tql_set,
    bench_tql_delete,
    // 索引
    bench_create_index,
    // CRUD 快速路径
    bench_get_payload,
    bench_get_edges,
    bench_contains,
    // 过滤
    bench_filter_via_tql,
    bench_filter_complex_via_tql,
    // 对比组
    bench_tql_vs_raw_search,
    bench_index_vs_no_index,
);
criterion_main!(query_benches);
