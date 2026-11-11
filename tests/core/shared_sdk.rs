//! 共享 SDK 的请求拒绝、所有权与真实数据恢复契约。

use super::support::TestDir;
use serde_json::{Value, json};
use std::sync::{Arc, Barrier, Mutex, OnceLock, Weak};
use triviumdb::database::TxBuilder;
use triviumdb::{
    ConsoleHost, ConsoleOptions, Database, HookContext, SearchConfig, SearchHook, SharedDatabase,
    ShutdownPhase, TriviumError,
};

fn assert_error<T>(result: triviumdb::Result<T>, code: &str, details: Value) {
    let error = result.err().expect("请求必须被明确拒绝");
    assert_eq!(error.code(), code);
    assert_eq!(error.details(), details);
}

#[cfg(feature = "test-hooks")]
#[test]
fn 上下文取消贯通内核预检后的WAL边界并保留完整事实() {
    use triviumdb::query::tql_executor::QueryControl;
    use triviumdb::sdk::RequestContext;
    use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};
    let root = TestDir::new("sdk_late_cancel");
    let path = root.file("db.tdb");
    let database = SharedDatabase::<f32>::open(&path, 2).unwrap();
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    let control = QueryControl::default();
    let context = RequestContext::new("late", control.clone(), true).unwrap();
    let pause = pause_at(ConcurrencyPoint::BeforeTransactionWal);
    let worker = database.clone();
    let running = std::thread::spawn(move || {
        let mut transaction = TxBuilder::new();
        transaction.insert(&[1., 0.], json!({"state":"cancelled"}));
        let error = worker
            .commit_tx_with_context(&context, &mut transaction)
            .unwrap_err();
        (error, transaction.pending_count())
    });
    pause.wait_until_arrived();
    control.cancel();
    pause.release();
    let (error, pending) = running.join().unwrap();
    assert_eq!(error.code(), "TDB_QUERY_CANCELLED");
    assert_eq!(
        serde_json::to_value(error.info().write).unwrap(),
        json!({"phase":"validation","outcome":"not_committed","transaction_consumed":true})
    );
    assert_eq!(pending, 0);
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert_eq!(
        database.try_read(|core| core.all_node_ids()).unwrap(),
        Vec::<u64>::new()
    );
    let mut transaction = TxBuilder::new();
    transaction.insert(&[0., 1.], json!({"state":"saved"}));
    assert_eq!(database.commit_tx(&mut transaction).unwrap(), vec![1]);
    database.close().unwrap();
    let mut reopened = Database::<f32>::open(&path, 2).unwrap();
    assert_eq!(reopened.all_node_ids().unwrap(), vec![1]);
    let node = reopened.get(1).unwrap().unwrap();
    assert_eq!(node.vector, vec![0., 1.]);
    assert_eq!(node.payload, json!({"state":"saved"}));
    assert!(node.edges.is_empty());
    reopened.close().unwrap();
}

#[test]
fn 请求上下文在借用前拒绝取消并保留原始内核结果() {
    use triviumdb::query::tql_executor::QueryControl;
    use triviumdb::sdk::RequestContext;
    let root = TestDir::new("sdk_context");
    let path = root.file("db.tdb");
    let database = SharedDatabase::<f32>::open(&path, 2).unwrap();
    let control = QueryControl::default();
    let context = RequestContext::new("write-1", control.clone(), true).unwrap();
    assert_eq!(context.request_id(), "write-1");
    assert!(context.is_write());
    control.cancel();
    let error = database
        .try_write_with_context(&context, |_| -> triviumdb::Result<()> {
            panic!("取消请求必须在调用业务闭包之前拒绝");
        })
        .unwrap_err();
    assert_eq!(error.code(), "TDB_QUERY_CANCELLED");
    assert_eq!(
        serde_json::to_value(error.info().write).unwrap(),
        json!({"phase":"admission","outcome":"not_committed"})
    );
    let mut transaction = TxBuilder::new();
    transaction.insert_with_id(7, &[1., 0.], json!({"state":"saved"}));
    let error = database
        .commit_tx_with_context(&context, &mut transaction)
        .unwrap_err();
    assert_eq!(error.code(), "TDB_QUERY_CANCELLED");
    assert_eq!(transaction.pending_count(), 1);
    assert_eq!(
        serde_json::to_value(error.info().write).unwrap(),
        json!({"phase":"admission","outcome":"not_committed","transaction_consumed":false})
    );
    let context = RequestContext::new("write-2", QueryControl::default(), true).unwrap();
    assert_eq!(
        database
            .commit_tx_with_context(&context, &mut transaction)
            .unwrap(),
        vec![7]
    );
    assert_eq!(transaction.pending_count(), 0);
    let error = database
        .try_read(|_| database.try_write_with_context(&context, |_| Ok(())))
        .unwrap_err();
    assert_eq!(error.code(), "TDB_REENTRANT_ACCESS");
    assert_eq!(
        serde_json::to_value(error.info().write).unwrap(),
        json!({"phase":"admission","outcome":"not_committed"})
    );
    let read = RequestContext::new("read-1", QueryControl::default(), false).unwrap();
    assert_eq!(
        database
            .try_read_with_context(&read, |core| core.all_node_ids())
            .unwrap(),
        vec![7]
    );
    database.close().unwrap();
    let mut reopened = Database::<f32>::open(&path, 2).unwrap();
    let node = reopened.get(7).unwrap().unwrap();
    assert_eq!(node.vector, vec![1., 0.]);
    assert_eq!(node.payload, json!({"state":"saved"}));
    assert!(node.edges.is_empty());
    reopened.close().unwrap();
}

#[test]
fn 并发借用允许读取且繁忙关闭保持实例开放() {
    let root = TestDir::new("sdk_readers");
    let database = SharedDatabase::<f32>::open(&root.file("db.tdb"), 2).unwrap();
    database
        .try_write(|core| core.insert_with_id(1, &[1., 2.], json!({"live":true})))
        .unwrap();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        let worker = database.clone();
        let entered_worker = entered.clone();
        let release_worker = release.clone();
        let reader = scope.spawn(move || {
            worker.try_read(|core| {
                entered_worker.wait();
                release_worker.wait();
                core.get_payload(1)
            })
        });
        entered.wait();
        let read = database.try_read(|core| core.get_payload(1));
        let close = database.close();
        let status = database.shutdown_status();
        release.wait();
        assert_eq!(read.unwrap(), Some(json!({"live":true})));
        assert_error(close, "TDB_BUSY", json!({"resource":"shared_database"}));
        assert_eq!(status.unwrap().phase, ShutdownPhase::Open);
        assert_eq!(reader.join().unwrap().unwrap(), Some(json!({"live":true})));
    });
    database.close().unwrap();
}

#[test]
fn 宿主异常后锁中毒明确拒绝并由释放后重开恢复() {
    let root = TestDir::new("sdk_poison");
    let path = root.file("db.tdb");
    let database = SharedDatabase::<f32>::open(&path, 2).unwrap();
    let worker = database.clone();
    let panic = std::thread::spawn(move || {
        let _: triviumdb::Result<()> = worker.try_write(|core| {
            core.insert_with_id(1, &[1., 2.], json!({"confirmed":true}))?;
            panic!("模拟宿主闭包异常展开");
        });
    })
    .join();
    assert!(panic.is_err());
    assert_error(
        database.try_read(|core| core.get(1)),
        "TDB_HOST_LOCK_POISONED",
        json!({"resource":"shared_database"}),
    );
    assert!(matches!(
        Database::<f32>::open(&path, 2),
        Err(TriviumError::DatabaseLocked(_))
    ));
    drop(database);
    let mut reopened = Database::<f32>::open(&path, 2).unwrap();
    let recovered = reopened.get(1).unwrap().unwrap();
    assert_eq!(recovered.vector, vec![1., 2.]);
    assert_eq!(recovered.payload, json!({"confirmed":true}));
    reopened.close().unwrap();
}

#[test]
fn 并发准备锁忙拒绝保留事务且释放后完整提交() {
    let root = TestDir::new("sdk_commit");
    let path = root.file("db.tdb");
    let database = SharedDatabase::<f32>::open(&path, 2).unwrap();
    let worker = database.clone();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        let entered_worker = entered.clone();
        let release_worker = release.clone();
        let writer = scope.spawn(move || {
            let mut transaction = TxBuilder::new();
            transaction.insert_with_id(1, &[1., 0.], json!({"source":"first"}));
            worker
                .try_write(|core| {
                    entered_worker.wait();
                    release_worker.wait();
                    core.commit_tx(transaction)
                })
                .unwrap()
        });
        entered.wait();
        let mut transaction = TxBuilder::new();
        transaction.insert_with_id(2, &[0., 1.], json!({"source":"second"}));
        transaction.link(2, 1, "next", 0.5);
        let rejected = database.commit_tx(&mut transaction);
        // 先解除屏障再断言；断言失败也允许写入者退出。
        release.wait();
        assert_eq!(writer.join().unwrap(), vec![1]);
        assert_error(rejected, "TDB_BUSY", json!({"resource":"shared_database"}));
        assert_eq!(transaction.pending_count(), 2);
        assert_eq!(database.commit_tx(&mut transaction).unwrap(), vec![2]);
        assert_eq!(transaction.pending_count(), 0);
    });
    database.close().unwrap();
    let mut reopened = Database::<f32>::open(&path, 2).unwrap();
    assert_eq!(reopened.all_node_ids().unwrap(), vec![1, 2]);
    let first = reopened.get(1).unwrap().unwrap();
    assert_eq!(first.vector, vec![1., 0.]);
    assert_eq!(first.payload, json!({"source":"first"}));
    let second = reopened.get(2).unwrap().unwrap();
    assert_eq!(second.vector, vec![0., 1.]);
    assert_eq!(second.payload, json!({"source":"second"}));
    assert_eq!(second.edges.len(), 1);
    assert_eq!(second.edges[0].target_id, 1);
    assert_eq!(second.edges[0].label, "next");
    assert_eq!(second.edges[0].weight, 0.5);
    reopened.close().unwrap();
}

#[test]
fn 同线程重入关闭和重复打开只拒绝请求并保留原实例() {
    let root = TestDir::new("sdk_reentry");
    let path = root.file("db.tdb");
    let database = SharedDatabase::<f32>::open(&path, 2).unwrap();
    let other = database.clone();
    database
        .try_write(|core| {
            assert_error(
                other.try_read(|core| core.get(1)),
                "TDB_REENTRANT_ACCESS",
                json!({"operation":"shared_read"}),
            );
            assert_error(
                other.close(),
                "TDB_REENTRANT_ACCESS",
                json!({"operation":"shared_write"}),
            );
            assert!(matches!(
                Database::<f32>::open(&path, 2),
                Err(TriviumError::DatabaseLocked(_))
            ));
            core.insert_with_id(1, &[1., 2.], json!({"state":"alive"}))
        })
        .unwrap();
    drop(other);
    assert_eq!(
        database.try_read(|core| core.get_payload(1)).unwrap(),
        Some(json!({"state":"alive"}))
    );
    let mut invalid = TxBuilder::new();
    invalid.insert_with_id(2, &[1.], json!({}));
    assert_error(
        database.commit_tx(&mut invalid),
        "TDB_DIMENSION_MISMATCH",
        json!({"expected":2,"got":1}),
    );
    assert_eq!(
        database.try_read(|core| core.all_node_ids()).unwrap(),
        vec![1]
    );
    database
        .try_write(|core| core.insert_with_id(3, &[3., 4.], json!({"state":"after_error"})))
        .unwrap();
    database.close().unwrap();
    assert_eq!(
        database.shutdown_status().unwrap().phase,
        ShutdownPhase::Closed
    );
    let mut pending = TxBuilder::new();
    pending.insert_with_id(4, &[0., 1.], json!({}));
    assert_error(
        database.commit_tx(&mut pending),
        "TDB_DATABASE_CLOSED",
        json!({}),
    );
    assert_eq!(pending.pending_count(), 1);
    assert_error(
        database.try_read::<()>(|_| panic!("关闭后不得进入调用方闭包")),
        "TDB_DATABASE_CLOSED",
        json!({}),
    );
    let mut reopened = Database::<f32>::open(&path, 2).unwrap();
    assert_eq!(reopened.all_node_ids().unwrap(), vec![1, 3]);
    assert_eq!(reopened.get(3).unwrap().unwrap().vector, vec![3., 4.]);
    assert_eq!(
        reopened.get_payload(3).unwrap(),
        Some(json!({"state":"after_error"}))
    );
    reopened.close().unwrap();
}

#[test]
fn 控制台共享宿主弱所有权和卸载保持一致() {
    let root = TestDir::new("sdk_console");
    let path = root.file("db.tdb");
    let database = SharedDatabase::<f32>::open(&path, 2).unwrap();
    database
        .try_write(|core| core.insert_with_id(1, &[1., 2.], json!({"key":"value"})))
        .unwrap();
    let host = ConsoleHost::attach_shared(&database, ConsoleOptions::default()).unwrap();
    let request =
        json!({"version":1,"requestId":"snapshot","command":{"operation":"getNodes","ids":["1"]}})
            .to_string();
    database
        .try_read(|_| {
            assert_error(
                host.request(&request),
                "TDB_REENTRANT_ACCESS",
                json!({"operation":"shared_write"}),
            );
            Ok(())
        })
        .unwrap();
    let snapshot: Value = serde_json::from_str(&host.request(&request).unwrap()).unwrap();
    assert_eq!(
        snapshot["data"]["nodes"],
        json!([{"id":"1","vector":[1.,2.],"payload":{"key":"value"},"edges":[]}])
    );
    host.session().detach();
    assert_error(
        host.request(&request),
        "TDB_CONSOLE_SESSION_CLOSED",
        json!({}),
    );
    let attached = ConsoleHost::attach_shared(&database, ConsoleOptions::default()).unwrap();
    let weak = database.downgrade();
    database.close().unwrap();
    drop(database);
    assert!(weak.upgrade().is_none());
    assert_error(attached.request(&request), "TDB_DATABASE_CLOSED", json!({}));
    // 已关闭的组件仍存活时，Windows 主文件也必须可以移动并重新打开。
    let renamed = root.file("renamed.tdb");
    std::fs::rename(&path, &renamed).unwrap();
    std::fs::rename(&renamed, &path).unwrap();
    let mut reopened = Database::<f32>::open(&path, 2).unwrap();
    assert_eq!(
        reopened.get_payload(1).unwrap(),
        Some(json!({"key":"value"}))
    );
    reopened.close().unwrap();
}

#[test]
fn 内核hook按实例拒绝重入且其他实例可继续写入() {
    struct Probe {
        owner: Arc<OnceLock<Weak<Database<f32>>>>,
        other: Arc<Mutex<Database<f32>>>,
        observations: Arc<Mutex<Vec<&'static str>>>,
    }
    impl SearchHook for Probe {
        fn on_pre_search(&self, _: &mut Vec<f32>, _: &mut SearchConfig, _: &mut HookContext) {
            let owner = self.owner.get().unwrap().upgrade().unwrap();
            let read = owner.get(1).unwrap_err();
            let write = owner.update_vector(1, &[9., 9.]).unwrap_err();
            let build = owner
                .quiver_build_handle()
                .prepare_auto_build()
                .err()
                .expect("重入构建应拒绝");
            self.observations
                .lock()
                .unwrap()
                .extend([read.code(), write.code(), build.code()]);
            self.other
                .lock()
                .unwrap()
                .insert_with_id(7, &[7., 8.], json!({"other":true}))
                .unwrap();
        }
    }
    let root = TestDir::new("sdk_hook");
    let path = root.file("db.tdb");
    let owner = Arc::new(OnceLock::new());
    let observations = Arc::new(Mutex::new(Vec::new()));
    let other = Arc::new(Mutex::new(
        Database::<f32>::open(&root.file("other.tdb"), 2).unwrap(),
    ));
    let mut core = Database::<f32>::open(&path, 2).unwrap();
    core.insert_with_id(1, &[1., 0.], json!({"source":true}))
        .unwrap();
    core.set_hook(Probe {
        owner: owner.clone(),
        other: other.clone(),
        observations: observations.clone(),
    })
    .unwrap();
    let core = Arc::new(core);
    owner.set(Arc::downgrade(&core)).unwrap();
    let hits = core.search(&[1., 0.], 1, 0, 0.).unwrap();
    assert_eq!(hits.iter().map(|hit| hit.id).collect::<Vec<_>>(), vec![1]);
    assert_eq!(
        *observations.lock().unwrap(),
        vec!["TDB_REENTRANT_ACCESS"; 3]
    );
    assert_eq!(core.get(1).unwrap().unwrap().vector, vec![1., 0.]);
    core.update_vector(1, &[2., 3.]).unwrap();
    let mut core = Arc::try_unwrap(core).ok().expect("Hook 只持有弱引用");
    core.close().unwrap();
    assert_eq!(
        other.lock().unwrap().get_payload(7).unwrap(),
        Some(json!({"other":true}))
    );
    other.lock().unwrap().close().unwrap();
    let mut reopened = Database::<f32>::open(&path, 2).unwrap();
    assert_eq!(reopened.get(1).unwrap().unwrap().vector, vec![2., 3.]);
    reopened.close().unwrap();
}

#[test]
fn 预先准备的后台构建在hook内执行也拒绝重入并归还租约() {
    struct BuildHook {
        build: Mutex<Option<triviumdb::database::PreparedQuiverBuild<f32>>>,
        result: Arc<Mutex<Option<&'static str>>>,
    }
    impl SearchHook for BuildHook {
        fn on_pre_search(&self, _: &mut Vec<f32>, _: &mut SearchConfig, _: &mut HookContext) {
            let build = self
                .build
                .lock()
                .unwrap()
                .take()
                .expect("必须持有已准备的真实任务");
            let error = build.execute().unwrap_err();
            *self.result.lock().unwrap() = Some(error.code());
        }
    }
    let root = TestDir::new("sdk_prepared_hook");
    let path = root.file("db.tdb");
    let mut core = Database::<f32>::open(&path, 3).unwrap();
    // 834 为低维默认自动构建门槛，确保测试持有真实快照与活动操作租约。
    let mut transaction = TxBuilder::new();
    for id in 1..=834 {
        transaction.insert_with_id(id, &[id as f32, 1., 0.], json!({"id":id}));
    }
    core.commit_tx(transaction).unwrap();
    let build = core
        .prepare_auto_quiver_build()
        .unwrap()
        .expect("必须准备真实构建任务");
    core.set_auto_build_quiver(false).unwrap();
    let result = Arc::new(Mutex::new(None));
    core.set_hook(BuildHook {
        build: Mutex::new(Some(build)),
        result: result.clone(),
    })
    .unwrap();
    assert_eq!(core.search(&[1., 1., 0.], 1, 0, 0.).unwrap()[0].id, 1);
    assert_eq!(*result.lock().unwrap(), Some("TDB_REENTRANT_ACCESS"));
    assert_eq!(core.shutdown_status().active_operations, 0);
    core.clear_hook();
    core.set_auto_build_quiver(true).unwrap();
    assert!(
        core.prepare_auto_quiver_build()
            .unwrap()
            .expect("拒绝后构建登记必须释放")
            .execute()
            .unwrap()
    );
    core.update_payload(1, json!({"after":"reentry"})).unwrap();
    core.close().unwrap();
    let mut reopened = Database::<f32>::open(&path, 3).unwrap();
    assert_eq!(reopened.node_count().unwrap(), 834);
    assert_eq!(
        reopened.get_payload(1).unwrap(),
        Some(json!({"after":"reentry"}))
    );
    let last = reopened.get(834).unwrap().unwrap();
    assert_eq!(last.vector, vec![834., 1., 0.]);
    assert_eq!(last.payload, json!({"id":834}));
    reopened.close().unwrap();
}
