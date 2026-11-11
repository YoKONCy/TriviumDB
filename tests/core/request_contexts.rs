//! 普通类型化入口与包装对象共享请求控制及真实提交边界。
use super::support::TestDir;
use serde_json::json;
use triviumdb::Database;
use triviumdb::database::{DatabaseReader, DatabaseWriter};
use triviumdb::query::tql_executor::QueryControl;
use triviumdb::sdk::RequestContext;

#[cfg(feature = "test-hooks")]
#[test]
fn 精确扫描与图计算在内部检查点取消后归还资源并可重开() {
    use triviumdb::database::Config;
    use triviumdb::test_hooks::{
        ConcurrencyPoint, QueryExecutionStrategy, force_query_strategy, pause_at,
    };
    for strategy in [
        QueryExecutionStrategy::ForceSerial,
        QueryExecutionStrategy::ForceParallel,
    ] {
        for operation in ["exact", "leiden", "pagerank", "label_propagation"] {
            let root = TestDir::new("algorithm-cancel");
            let path = root.file("db.tdb");
            let mut database = Database::<f32>::open_with_config(
                &path,
                Config {
                    dim: 2,
                    auto_build_quiver: false,
                    ..Default::default()
                },
            )
            .unwrap();
            for id in 1..=4 {
                database
                    .insert_with_id(id, &[1., 0.], json!({"id":id,"body":"完整事实"}))
                    .unwrap();
            }
            for (source, target) in [(1, 2), (2, 3), (3, 4), (4, 1)] {
                database.link(source, target, "related", 1.).unwrap();
            }
            let before = std::fs::read(format!("{path}.wal")).unwrap();
            let query = format!(
                "SEARCH VECTOR [1, 0] TOP 4 AS seed WITH seed {operation} seed AS scored WITH scored RETURN scored"
            );
            if matches!(operation, "pagerank" | "label_propagation") {
                database.prepare_tql(&query).unwrap();
            }
            let context = RequestContext::new("algorithm", QueryControl::default(), false).unwrap();
            let running_context = context.clone();
            let hook = pause_at(if operation == "exact" {
                ConcurrencyPoint::ExactScanStep
            } else {
                ConcurrencyPoint::GraphAlgorithmIteration
            });
            let worker = std::thread::spawn(move || {
                let _strategy = force_query_strategy(strategy);
                let result = running_context.run(|| match operation {
                    "exact" => database.search_exact(&[1., 0.], 2).map(|_| ()),
                    "leiden" => database.leiden_cluster(1, Some(15), Some(true)).map(|_| ()),
                    _ => database.tql(&query).map(|_| ()),
                });
                (database, result.unwrap_err())
            });
            hook.wait_until_arrived();
            context.cancel();
            hook.release();
            let (mut database, error) = worker.join().unwrap();
            assert_eq!(
                error.code(),
                "TDB_QUERY_CANCELLED",
                "{operation} {strategy:?}"
            );
            assert!(error.info().write.is_none());
            assert_eq!(database.resource_stats().reserved_workspace_bytes, 0);
            assert_eq!(database.resource_stats().active_tasks, 0);
            assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
            assert_eq!(
                database
                    .search_exact(&[1., 0.], 2)
                    .unwrap()
                    .iter()
                    .map(|hit| (hit.id, hit.score, hit.payload.clone()))
                    .collect::<Vec<_>>(),
                vec![
                    (1, 1., json!({"id":1,"body":"完整事实"})),
                    (2, 1., json!({"id":2,"body":"完整事实"}))
                ]
            );
            database.close().unwrap();
            let mut reopened = Database::<f32>::open_read_only(&path, 2).unwrap();
            assert_eq!(reopened.all_node_ids().unwrap(), vec![1, 2, 3, 4]);
            for id in 1..=4 {
                let node = reopened.get(id).unwrap().unwrap();
                assert_eq!(node.vector, vec![1., 0.]);
                assert_eq!(node.payload, json!({"id":id,"body":"完整事实"}));
                assert_eq!(node.edges.len(), 1);
                let edge = &node.edges[0];
                assert_eq!(
                    (
                        edge.target_id,
                        edge.label.as_str(),
                        edge.weight,
                        edge.metadata.clone()
                    ),
                    (if id == 4 { 1 } else { id + 1 }, "related", 1., json!(null))
                );
            }
            reopened.close().unwrap();
        }
    }
}

#[test]
fn 类型化门面嵌套取消和异常退出保留事实与资源() {
    let root = TestDir::new("typed-request");
    let path = root.file("db.tdb");
    let mut writer = DatabaseWriter::<f32>::open(&path, 2).unwrap();
    writer
        .insert_with_id(7, &[1., 0.], json!({"body":"请求原文","stable":true}))
        .unwrap();
    let parent = RequestContext::new("parent", QueryControl::default(), true).unwrap();
    let child = RequestContext::new("child", QueryControl::default(), true).unwrap();
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    let error = writer
        .with_request_context(&parent, |writer| {
            parent.cancel();
            writer.with_request_context(&child, |writer| writer.delete(7))
        })
        .unwrap_err();
    assert_eq!(error.code(), "TDB_QUERY_CANCELLED");
    assert_eq!(
        serde_json::to_value(error.info().write).unwrap(),
        json!({"phase":"admission","outcome":"not_committed"})
    );
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        child.run::<()>(|| panic!("模拟宿主异常"))
    }));
    assert!(panic.is_err());
    writer
        .insert_with_id(8, &[0., 1.], json!({"body":"后续写入"}))
        .unwrap();
    writer.close().unwrap();
    let mut reader = DatabaseReader::<f32>::open_read_only(&path, 2).unwrap();
    let revision = RequestContext::new("read", QueryControl::default(), false).unwrap();
    let rows = reader
        .with_request_context(&revision, |reader| reader.all_node_ids())
        .unwrap();
    assert_eq!(rows, vec![7, 8]);
    let node = reader.get(7).unwrap().unwrap();
    assert_eq!(node.vector, vec![1., 0.]);
    assert_eq!(node.payload, json!({"body":"请求原文","stable":true}));
    assert!(node.edges.is_empty());
    revision.cancel();
    assert_eq!(
        reader
            .with_request_context(&revision, |reader| reader.get(7))
            .unwrap_err()
            .code(),
        "TDB_QUERY_CANCELLED"
    );
    reader.close().unwrap();
}

#[test]
fn 嵌套控制保留外层期限且错误后清空当前作用域() {
    let expired = RequestContext::new(
        "expired",
        QueryControl::with_deadline(std::time::Instant::now()),
        false,
    )
    .unwrap();
    let parent = RequestContext::new("parent", QueryControl::default(), false).unwrap();
    let root = TestDir::new("typed-deadline");
    let mut database = Database::<f32>::open(&root.file("db.tdb"), 2).unwrap();
    let error = parent
        .run(|| {
            parent.cancel();
            expired.run(|| database.all_node_ids())
        })
        .unwrap_err();
    assert_eq!(error.code(), "TDB_QUERY_DEADLINE_EXCEEDED");
    assert_eq!(
        expired.run(|| database.all_node_ids()).unwrap_err().code(),
        "TDB_QUERY_DEADLINE_EXCEEDED"
    );
    assert!(database.all_node_ids().unwrap().is_empty());
    let child = RequestContext::new("child", QueryControl::default(), false).unwrap();
    let error = expired
        .run(|| child.run(|| database.all_node_ids()))
        .unwrap_err();
    assert_eq!(error.code(), "TDB_QUERY_DEADLINE_EXCEEDED");
    fn nested(context: &RequestContext, depth: usize) -> triviumdb::Result<()> {
        if depth == 0 {
            return Ok(());
        }
        context.run(|| nested(context, depth - 1))
    }
    // Core 的 64 帧包含用户作用域与适配帧，超限拒绝发生在业务执行前。
    let bounded = RequestContext::new("bounded", QueryControl::default(), true).unwrap();
    nested(&bounded, 64).unwrap();
    let error = nested(&bounded, 65).unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    assert_eq!(
        serde_json::to_value(error.info().write).unwrap(),
        json!({"phase":"admission","outcome":"not_committed"})
    );
    database.close().unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn 普通共享提交继承请求控制并在WAL前取消() {
    use triviumdb::SharedDatabase;
    use triviumdb::database::TxBuilder;
    use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};
    let root = TestDir::new("typed-commit");
    let path = root.file("db.tdb");
    let database = SharedDatabase::<f32>::open(&path, 2).unwrap();
    let context = RequestContext::new("commit", QueryControl::default(), true).unwrap();
    let worker_context = context.clone();
    let worker = database.clone();
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    let hook = pause_at(ConcurrencyPoint::TransactionWalLockAcquired);
    let running = std::thread::spawn(move || {
        let mut transaction = TxBuilder::new();
        transaction.insert(&[1., 0.], json!({"body":"取消写入"}));
        let error = worker_context
            .run(|| worker.commit_tx(&mut transaction))
            .unwrap_err();
        (error, transaction.pending_count())
    });
    hook.wait_until_arrived();
    context.cancel();
    hook.release();
    let (error, pending) = running.join().unwrap();
    assert_eq!(error.code(), "TDB_QUERY_CANCELLED");
    assert_eq!(
        serde_json::to_value(error.info().write).unwrap(),
        json!({"phase":"validation","outcome":"not_committed","transaction_consumed":true})
    );
    assert_eq!(pending, 0);
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert!(
        database
            .try_read(|db| db.all_node_ids())
            .unwrap()
            .is_empty()
    );
    database.close().unwrap();
    let mut reopened = Database::<f32>::open_read_only(&path, 2).unwrap();
    assert!(reopened.all_node_ids().unwrap().is_empty());
    reopened.close().unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn 直接写入在取得WAL锁后观察取消且保留自动ID() {
    use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};
    let root = TestDir::new("typed-direct-wal");
    let path = root.file("db.tdb");
    let mut database = Database::<f32>::open(&path, 2).unwrap();
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    let context = RequestContext::new("direct", QueryControl::default(), true).unwrap();
    let worker_context = context.clone();
    let hook = pause_at(ConcurrencyPoint::DirectWriteWalLockAcquired);
    let running = std::thread::spawn(move || {
        let error = worker_context
            .run(|| database.insert(&[1., 0.], json!({"body":"取消"})))
            .unwrap_err();
        (database, error)
    });
    hook.wait_until_arrived();
    context.cancel();
    hook.release();
    let (mut database, error) = running.join().unwrap();
    assert_eq!(error.code(), "TDB_QUERY_CANCELLED");
    assert_eq!(
        serde_json::to_value(error.info().write).unwrap(),
        json!({"phase":"validation","outcome":"not_committed"})
    );
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert!(database.all_node_ids().unwrap().is_empty());
    assert_eq!(
        database
            .insert(&[0., 1.], json!({"body":"已提交"}))
            .unwrap(),
        1
    );
    database.close().unwrap();
    let mut reopened = Database::<f32>::open_read_only(&path, 2).unwrap();
    assert_eq!(reopened.all_node_ids().unwrap(), vec![1]);
    let node = reopened.get(1).unwrap().unwrap();
    assert_eq!(node.vector, vec![0., 1.]);
    assert_eq!(node.payload, json!({"body":"已提交"}));
    assert!(node.edges.is_empty());
    reopened.close().unwrap();
}
