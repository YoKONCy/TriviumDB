use serde_json::json;
use triviumdb::{
    Database, IndustrialSearchConfig, QueryMemoryBudget, TsngBudget, TsngQuery, TsngWeights,
};

const BYTE_BUDGET: usize = 24 * 1024 * 1024;

#[test]
fn 相同字节预算按任意维度精确推导候选上限() {
    let budget = QueryMemoryBudget {
        max_rerank_vector_bytes: BYTE_BUDGET,
        ..Default::default()
    };
    for dim in [384usize, 400, 768, 900, 1536, 3072] {
        assert_eq!(
            budget.max_rerank_vectors::<f32>(dim),
            BYTE_BUDGET / (dim * std::mem::size_of::<f32>())
        );
    }
}

#[test]
fn 候选上限随维度单调下降且非常规维度没有分档() {
    let config = IndustrialSearchConfig {
        memory: QueryMemoryBudget {
            max_rerank_vector_bytes: BYTE_BUDGET,
            ..Default::default()
        },
        direct_rerank_bytes: 6 * 1024 * 1024,
        union_rerank_bytes: 12 * 1024 * 1024,
        ..IndustrialSearchConfig::for_top_k(10)
    };
    let dimensions = [384usize, 400, 768, 900, 1536, 3072];
    let limits = dimensions.map(|dim| config.direct_candidate_limit::<f32>(dim, 10));
    assert!(limits.windows(2).all(|pair| pair[0] > pair[1]));
    for dim in dimensions {
        assert_eq!(
            config.direct_candidate_limit::<f32>(dim, 10),
            config.direct_rerank_bytes / (dim * std::mem::size_of::<f32>())
        );
        assert_eq!(
            config.union_candidate_limit::<f32>(dim, 10),
            config.union_rerank_bytes / (dim * std::mem::size_of::<f32>())
        );
    }
}

#[test]
fn 候选与并集预算按真实元素字节推导() {
    let budget = QueryMemoryBudget {
        max_candidate_id_bytes: 80,
        max_union_bytes: 160,
        ..Default::default()
    };
    assert_eq!(budget.max_candidate_ids(), 10);
    assert_eq!(budget.max_union_ids(), 10);
}

#[test]
fn 各维度超预算均在查询前拒绝且不写盘() {
    for dim in [384usize, 768, 1536, 3072] {
        let path = std::env::temp_dir().join(format!(
            "triviumdb_dimension_budget_{dim}_{}.tdb",
            std::process::id()
        ));
        let path = path.to_string_lossy().into_owned();
        cleanup(&path);
        let mut db = Database::<f32>::open(&path, dim).unwrap();
        let vector = vec![1.0f32; dim];
        for id in 1..=12 {
            db.insert_with_id(id, &vector, json!({"id": id})).unwrap();
        }
        let query = TsngQuery {
            vector: &vector,
            payload_filter: None,
            graph: None,
            top_k: 4,
            weights: TsngWeights::default(),
            budget: TsngBudget::default(),
        };
        let before = db.storage_write_stats();
        let mut config = IndustrialSearchConfig::for_top_k(4);
        config.memory.max_rerank_vector_bytes = 3 * dim * std::mem::size_of::<f32>();
        config.direct_rerank_bytes = config.memory.max_rerank_vector_bytes;
        config.union_rerank_bytes = config.memory.max_rerank_vector_bytes;
        let error = db
            .search_tsng_industrial(&query, config)
            .expect_err("十二个精排候选必须超过三个向量的字节预算");
        assert!(error.to_string().contains("字节预算"));
        assert_eq!(db.storage_write_stats(), before);
        assert_eq!(db.storage_write_stats().temporary_spill_bytes, 0);
        drop(db);
        cleanup(&path);
    }
}

fn cleanup(path: &str) {
    for suffix in ["", ".wal", ".vec", ".lock", ".flush_ok", ".pidx", ".gidx"] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}
