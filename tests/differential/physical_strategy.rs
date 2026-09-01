#![cfg(feature = "test-hooks")]

use super::canonical::canonicalize_rows;
use triviumdb::database::Database;
use triviumdb::test_hooks::{QueryExecutionStrategy, force_query_strategy};

fn database() -> (String, Database<f32>) {
    let root = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&root).unwrap();
    let path = root
        .join("physical_strategy.tdb")
        .to_string_lossy()
        .to_string();
    super::matrix::cleanup(&path);
    let reference = super::model::RefDatabase::fixture(48);
    let mut database = Database::<f32>::open(&path, 3).unwrap();
    super::matrix::seed(&mut database, &reference);
    (path, database)
}

#[test]
fn ForceSerial与ForceParallel执行结果一致() {
    let (path, database) = database();
    let query = "FIND {rank: {$gte: 0}} AS seed WITH seed WHERE seed.active == true RETURN seed, seed.rank AS rank ORDER BY seed.rank DESC LIMIT 12";
    let serial = {
        let _guard = force_query_strategy(QueryExecutionStrategy::ForceSerial);
        canonicalize_rows(database.tql_values(query).unwrap(), true)
    };
    let parallel = {
        let _guard = force_query_strategy(QueryExecutionStrategy::ForceParallel);
        canonicalize_rows(database.tql_values(query).unwrap(), true)
    };
    assert_eq!(parallel, serial);
    super::matrix::cleanup(&path);
}

#[test]
fn ExpandRank融合与拆分物理路径结果一致() {
    let (path, database) = database();
    let query = "FIND {rank: 0} AS seed WITH seed EXPAND seed [:related*1..4] AS related RANK related BY VECTOR [1, 1, 0] TOP 3 RETURN related";
    let fused = canonicalize_rows(database.tql_values(query).unwrap(), true);
    let split = {
        let _guard = force_query_strategy(QueryExecutionStrategy::DisableFusion);
        canonicalize_rows(database.tql_values(query).unwrap(), true)
    };
    assert_eq!(split, fused);
    super::matrix::cleanup(&path);
}
