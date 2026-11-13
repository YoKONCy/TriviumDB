//! 持有数据库对象时，连续发布失败、关闭重试及存储模式转换的状态回归。

use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use triviumdb::database::{Config, StorageMode};
use triviumdb::index::text::TextIndexDocument;
use triviumdb::resources::{ResourceLimits, ResourceRuntime};
use triviumdb::test_hooks::{IoPoint, fail_io_at};
use triviumdb::{Database, SearchHook, TriviumError, VectorType};

fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "tdb 持有与发布-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    root
}

fn released(root: &Path) {
    #[cfg(target_os = "linux")]
    {
        let prefix = format!("{}/", root.canonicalize().unwrap().display());
        let maps = std::fs::read("/proc/self/maps").unwrap();
        assert!(
            !String::from_utf8_lossy(&maps).contains(&prefix),
            "关闭后仍保留数据库文件映射：{prefix}"
        );
    }
    #[cfg(windows)]
    for file in std::fs::read_dir(root).unwrap() {
        use std::os::windows::fs::OpenOptionsExt;
        let file = file.unwrap().path();
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&file)
            .unwrap_or_else(|error| panic!("文件仍被占用 {}：{error}", file.display()));
    }
    let moved = root.with_extension("moved");
    std::fs::rename(root, &moved).unwrap();
    std::fs::rename(moved, root).unwrap();
}

fn cleanup(root: &Path) {
    for file in std::fs::read_dir(root).unwrap() {
        std::fs::remove_file(file.unwrap().path()).unwrap();
    }
    std::fs::remove_dir(root).unwrap();
}

fn seed<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    path: &str,
    mode: StorageMode,
) {
    let mut db = Database::<T>::open_with_config(
        path,
        Config {
            dim: 4,
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        },
    )
    .unwrap();
    db.insert_with_id(1, &[T::from_f32(1.); 4], json!({"kind":"original"}))
        .unwrap();
    db.insert_with_id(2, &[T::from_f32(2.); 4], json!({"kind":"second"}))
        .unwrap();
    db.create_index("kind").unwrap();
    db.index_text(1, "originaltoken").unwrap();
    db.index_keyword(1, "originalalias").unwrap();
    db.index_text(2, "secondtoken").unwrap();
    db.index_keyword(2, "secondalias").unwrap();
    db.close().unwrap();
}

fn text_ids<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    db: &Database<T>,
    kind: &str,
    text: &str,
) -> Vec<u64> {
    query_ids(
        db,
        &format!("TEXT {kind} \"{text}\" TOP 20 AS hits WITH hits RETURN hits"),
    )
}

fn query_ids<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    db: &Database<T>,
    query: &str,
) -> Vec<u64> {
    let mut ids = db
        .tql_nodes(query)
        .unwrap()
        .into_iter()
        .map(|row| {
            assert_eq!(row.len(), 1, "查询必须返回唯一节点列");
            row.into_values().next().unwrap().id
        })
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

struct HookOwner(Arc<AtomicUsize>);
impl SearchHook for HookOwner {}
impl Drop for HookOwner {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn close<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    db: &mut Database<T>,
    asynchronous: bool,
) -> triviumdb::Result<()> {
    if asynchronous {
        db.close_timeout(Duration::from_secs(10))
            .map(|done| assert!(done))
    } else {
        db.close()
    }
}

fn run_failed_close<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>() {
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        for asynchronous in [false, true] {
            for point in [IoPoint::TextMetadataWrite, IoPoint::GraphSidecarWrite] {
                let root = root();
                let path = root.join("sample.tdb").to_string_lossy().into_owned();
                seed::<T>(&path, mode);
                let runtime = ResourceRuntime::new(ResourceLimits {
                    memory_limit: 64 * 1024 * 1024,
                    maintenance_memory_limit: 8 * 1024 * 1024,
                    ..Default::default()
                })
                .unwrap();
                let mut db = Database::<T>::open_with_runtime(
                    &path,
                    Config {
                        dim: 4,
                        storage_mode: mode,
                        memory_limit: 8 * 1024 * 1024,
                        auto_build_quiver: false,
                        ..Default::default()
                    },
                    runtime.clone(),
                )
                .unwrap();
                let drops = Arc::new(AtomicUsize::new(0));
                db.set_hook(HookOwner(drops.clone())).unwrap();
                let node = db.get(1).unwrap().unwrap();
                let mut tx = db.begin_tx();
                tx.delete(2);
                tx.insert_with_id(2, &[T::from_f32(4.); 4], json!({"kind":"reused"}));
                tx.insert_with_id(3, &[T::from_f32(3.); 4], json!({"kind":"new"}));
                tx.commit().unwrap();
                db.link(1, 3, "新关系", 2.).unwrap();
                {
                    let _failure = fail_io_at(point);
                    assert!(matches!(
                        db.flush().unwrap_err().underlying(),
                        TriviumError::Io(_)
                    ));
                }
                db.index_text(1, "finaltoken").unwrap();
                db.index_keyword(1, "finalalias").unwrap();
                {
                    let _failure = fail_io_at(point);
                    assert!(matches!(
                        close(&mut db, asynchronous),
                        Err(TriviumError::Io(_))
                    ));
                }
                assert_eq!(drops.load(Ordering::SeqCst), 0);
                assert_eq!(runtime.stats().attached_databases, 1);
                assert_eq!(runtime.stats().reserved_database_bytes, 8 * 1024 * 1024);
                assert!(matches!(
                    Database::<T>::open(&path, 4),
                    Err(TriviumError::DatabaseLocked(_))
                ));
                close(&mut db, !asynchronous).unwrap();
                assert_eq!(drops.load(Ordering::SeqCst), 1);
                assert_eq!(runtime.stats().attached_databases, 0);
                assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
                assert_eq!(runtime.stats().reserved_maintenance_bytes, 0);
                released(&root);
                let mut reopened = Database::<T>::open_with_config(
                    &path,
                    Config {
                        dim: 4,
                        load_text_index: true,
                        auto_build_quiver: false,
                        ..Default::default()
                    },
                )
                .unwrap();
                assert_eq!(reopened.node_count().unwrap(), 3);
                assert_eq!(
                    reopened.get_payload(2).unwrap().unwrap(),
                    json!({"kind":"reused"})
                );
                assert_eq!(
                    reopened.get(2).unwrap().unwrap().vector,
                    vec![T::from_f32(4.); 4]
                );
                assert_eq!(reopened.get_edges(1).unwrap()[0].target_id, 3);
                assert_eq!(text_ids(&reopened, "BM25", "finaltoken"), vec![1]);
                assert_eq!(text_ids(&reopened, "AC", "finalalias"), vec![1]);
                assert_eq!(
                    text_ids(&reopened, "BM25", "secondtoken"),
                    Vec::<u64>::new()
                );
                assert_eq!(text_ids(&reopened, "AC", "secondalias"), Vec::<u64>::new());
                assert!(matches!(db.close(), Err(TriviumError::DatabaseClosed)));
                assert!(matches!(
                    Database::<T>::open(&path, 4),
                    Err(TriviumError::DatabaseLocked(_))
                ));
                reopened.close().unwrap();
                released(&root);
                cleanup(&root);
                std::hint::black_box((db, reopened, node));
            }
        }
    }
}

#[test]
fn 连续保存与关闭失败保留锁配额和Hook_跨关闭方式重试释放文件() {
    run_failed_close::<f32>();
    run_failed_close::<half::f16>();
    run_failed_close::<u64>();
}

#[test]
fn 跨存储模式连续失败后恢复WAL事实和原始正文_支持ID复用与索引清空() {
    for initial in [StorageMode::Mmap, StorageMode::Rom] {
        for target in [StorageMode::Mmap, StorageMode::Rom] {
            for loaded in [false, true] {
                for clear in [false, true] {
                    let root = root();
                    let path = root.join("sample.tdb").to_string_lossy().into_owned();
                    seed::<f32>(&path, initial);
                    let open = || {
                        Database::<f32>::open_with_config(
                            &path,
                            Config {
                                dim: 4,
                                storage_mode: target,
                                load_text_index: loaded,
                                auto_build_quiver: false,
                                ..Default::default()
                            },
                        )
                        .unwrap()
                    };
                    let mut db = open();
                    db.insert_with_id(3, &[3.; 4], json!({"kind":"third"}))
                        .unwrap();
                    {
                        let _failure = fail_io_at(IoPoint::TextMetadataWrite);
                        assert!(db.flush().is_err());
                    }
                    let mut tx = db.begin_tx();
                    tx.delete(2);
                    tx.insert_with_id(2, &[4.; 4], json!({"kind":"reused"}));
                    tx.insert_with_id(4, &[5.; 4], json!({"kind":"fourth"}));
                    tx.commit().unwrap();
                    db.update_payload(1, json!({"kind":"updated"})).unwrap();
                    db.update_vector(1, &[6.; 4]).unwrap();
                    db.link(1, 4, "恢复边", 3.).unwrap();
                    if clear {
                        db.rebuild_text_index(&[]).unwrap();
                    } else {
                        db.rebuild_text_index(&[TextIndexDocument {
                            id: 4,
                            text: "uncommittedtoken".into(),
                            keywords: vec!["uncommittedalias".into()],
                        }])
                        .unwrap();
                    }
                    {
                        let _failure = fail_io_at(IoPoint::GraphSidecarWrite);
                        assert!(db.flush().is_err());
                    }
                    drop(db);
                    let mut db = open();
                    assert_eq!(db.node_count().unwrap(), 4);
                    for (id, value, kind) in [
                        (1, 6., "updated"),
                        (2, 4., "reused"),
                        (3, 3., "third"),
                        (4, 5., "fourth"),
                    ] {
                        assert_eq!(db.get(id).unwrap().unwrap().vector, vec![value; 4]);
                        assert_eq!(db.get_payload(id).unwrap().unwrap(), json!({"kind":kind}));
                        assert_eq!(
                            query_ids(&db, &format!("FIND {{kind: \"{kind}\"}} RETURN *")),
                            vec![id]
                        );
                    }
                    assert_eq!(db.get_edges(1).unwrap()[0].target_id, 4);
                    db.build_text_index().unwrap();
                    assert_eq!(text_ids(&db, "BM25", "originaltoken"), vec![1]);
                    assert_eq!(text_ids(&db, "AC", "originalalias"), vec![1]);
                    assert_eq!(text_ids(&db, "BM25", "secondtoken"), Vec::<u64>::new());
                    assert_eq!(text_ids(&db, "AC", "secondalias"), Vec::<u64>::new());
                    assert_eq!(text_ids(&db, "BM25", "uncommittedtoken"), Vec::<u64>::new());
                    assert_eq!(text_ids(&db, "AC", "uncommittedalias"), Vec::<u64>::new());
                    db.close().unwrap();
                    let mut again = open();
                    again.build_text_index().unwrap();
                    assert_eq!(text_ids(&again, "BM25", "originaltoken"), vec![1]);
                    assert_eq!(again.node_count().unwrap(), 4);
                    again.close().unwrap();
                    released(&root);
                    cleanup(&root);
                    std::hint::black_box((db, again));
                }
            }
        }
    }
}

fn arrived(pause: &triviumdb::test_hooks::WaitHandle) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !pause.is_arrived() {
        assert!(
            std::time::Instant::now() < deadline,
            "发布工作未在期限内到达屏障"
        );
        std::thread::yield_now();
    }
}

#[test]
fn 同阶段按数据库路径选择的屏障可独立释放() {
    use triviumdb::test_hooks::{ConcurrencyPoint, hit_for_path, pause_at_path};
    let point = ConcurrencyPoint::AfterSidecarsPersisted;
    let first = pause_at_path(point, "独立样本甲");
    let second = pause_at_path(point, "独立样本乙");
    let (done, receive) = std::sync::mpsc::channel();
    let first_done = done.clone();
    let one = std::thread::spawn(move || {
        hit_for_path(point, "独立样本甲");
        first_done.send(1).unwrap();
    });
    let two = std::thread::spawn(move || {
        hit_for_path(point, "独立样本乙");
        done.send(2).unwrap();
    });
    arrived(&first);
    arrived(&second);
    hit_for_path(point, "独立样本丙");
    assert!(receive.try_recv().is_err());
    first.release();
    assert_eq!(receive.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
    assert!(receive.try_recv().is_err());
    second.release();
    assert_eq!(receive.recv_timeout(Duration::from_secs(5)).unwrap(), 2);
    one.join().unwrap();
    two.join().unwrap();
}

#[test]
fn 重复注册屏障拒绝后原有屏障仍可控制目标线程() {
    use triviumdb::test_hooks::{ConcurrencyPoint, hit_for_path, pause_at_path};
    let point = ConcurrencyPoint::AfterPublicationCommitted;
    let pause = pause_at_path(point, "重复注册样本");
    assert!(std::panic::catch_unwind(|| pause_at_path(point, "重复注册样本")).is_err());
    let (send, receive) = std::sync::mpsc::channel();
    let work = std::thread::spawn(move || {
        hit_for_path(point, "重复注册样本");
        send.send(()).unwrap();
    });
    arrived(&pause);
    assert!(receive.try_recv().is_err());
    pause.release();
    receive.recv_timeout(Duration::from_secs(5)).unwrap();
    work.join().unwrap();
}

#[test]
fn 恢复已替换正文后再次IO失败保留撤销组并可再次恢复() {
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        let root = root();
        let path = root.join("sample.tdb").to_string_lossy().into_owned();
        seed::<f32>(&path, mode);
        let original_main = std::fs::read(&path).unwrap();
        let original_text = std::fs::read(format!("{path}.text")).unwrap();
        let open = || {
            Database::<f32>::open_with_config(
                &path,
                Config {
                    dim: 4,
                    storage_mode: mode,
                    load_text_index: true,
                    auto_build_quiver: false,
                    ..Default::default()
                },
            )
        };
        let mut db = open().unwrap();
        db.insert_with_id(3, &[3.; 4], json!({"kind":"new"}))
            .unwrap();
        db.update_payload(1, json!({"kind":"changed"})).unwrap();
        db.index_text(1, "uncommittedtoken").unwrap();
        {
            let _failure = fail_io_at(IoPoint::GraphSidecarWrite);
            assert!(db.flush().is_err());
        }
        assert_ne!(std::fs::read(&path).unwrap(), original_main);
        assert_ne!(
            std::fs::read(format!("{path}.text")).unwrap(),
            original_text
        );
        drop(db);
        let original_journal = std::fs::read(format!("{path}.publish")).unwrap();
        // 元数据恢复临时文件被占位，确定在主数据与正文恢复后中断后续恢复。
        let obstacle = format!("{path}.text.meta.tmp");
        std::fs::create_dir(&obstacle).unwrap();
        assert!(matches!(open(), Err(TriviumError::Io(_))));
        assert_eq!(std::fs::read(&path).unwrap(), original_main);
        assert_eq!(
            std::fs::read(format!("{path}.text")).unwrap(),
            original_text
        );
        assert_eq!(
            std::fs::read(format!("{path}.publish")).unwrap(),
            original_journal
        );
        std::fs::remove_dir(&obstacle).unwrap();
        let before = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let data = std::fs::read(&path).unwrap();
                (path, data)
            })
            .collect::<Vec<_>>();
        assert!(matches!(
            Database::<f32>::open_read_only(&path, 4),
            Err(TriviumError::RecoveryRequired { .. })
        ));
        assert!(matches!(
            Database::<f32>::open_immutable(&path, 4),
            Err(TriviumError::RecoveryRequired { .. })
        ));
        for (file, data) in before {
            assert_eq!(std::fs::read(file).unwrap(), data);
        }
        let mut db = open().unwrap();
        assert_eq!(db.node_count().unwrap(), 3);
        assert_eq!(
            db.get_payload(1).unwrap().unwrap(),
            json!({"kind":"changed"})
        );
        assert_eq!(query_ids(&db, r#"FIND {kind: "new"} RETURN *"#), vec![3]);
        assert_eq!(
            query_ids(&db, r#"FIND {kind: "changed"} RETURN *"#),
            vec![1]
        );
        assert_eq!(text_ids(&db, "BM25", "originaltoken"), vec![1]);
        assert_eq!(text_ids(&db, "BM25", "uncommittedtoken"), Vec::<u64>::new());
        assert_eq!(text_ids(&db, "AC", "originalalias"), vec![1]);
        db.close().unwrap();
        let mut again = open().unwrap();
        assert_eq!(again.node_count().unwrap(), 3);
        assert_eq!(text_ids(&again, "BM25", "originaltoken"), vec![1]);
        again.close().unwrap();
        released(&root);
        cleanup(&root);
        std::hint::black_box((db, again));
    }
}

#[test]
fn 发布阶段关闭超时保留自身资源且共享实例可继续操作() {
    use triviumdb::test_hooks::{ConcurrencyPoint, pause_at_path};
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        for point in [
            ConcurrencyPoint::AfterSidecarsPersisted,
            ConcurrencyPoint::AfterPublicationCommitted,
        ] {
            let parent = root();
            let first_root = parent.join("关闭样本");
            let peer_root = parent.join("同运行时样本");
            std::fs::create_dir(&first_root).unwrap();
            std::fs::create_dir(&peer_root).unwrap();
            let path = first_root
                .canonicalize()
                .unwrap()
                .join("sample.tdb")
                .to_string_lossy()
                .into_owned();
            let peer_path = peer_root.join("sample.tdb").to_string_lossy().into_owned();
            let runtime = ResourceRuntime::new(ResourceLimits {
                memory_limit: 64 * 1024 * 1024,
                maintenance_memory_limit: 8 * 1024 * 1024,
                ..Default::default()
            })
            .unwrap();
            let config = || Config {
                dim: 4,
                storage_mode: mode,
                memory_limit: 8 * 1024 * 1024,
                auto_build_quiver: false,
                ..Default::default()
            };
            let mut db =
                Database::<f32>::open_with_runtime(&path, config(), runtime.clone()).unwrap();
            let mut peer =
                Database::<f32>::open_with_runtime(&peer_path, config(), runtime.clone()).unwrap();
            db.insert_with_id(1, &[1.; 4], json!({"kind":"closing"}))
                .unwrap();
            db.index_text(1, "closingtoken").unwrap();
            let drops = Arc::new(AtomicUsize::new(0));
            db.set_hook(HookOwner(drops.clone())).unwrap();
            let pause = pause_at_path(point, &path);
            assert!(!db.close_timeout(Duration::ZERO).unwrap());
            arrived(&pause);
            assert!(!db.close_timeout(Duration::ZERO).unwrap());
            assert_eq!(runtime.stats().attached_databases, 2);
            assert_eq!(runtime.stats().reserved_database_bytes, 16 * 1024 * 1024);
            assert_eq!(drops.load(Ordering::SeqCst), 0);
            assert!(matches!(
                Database::<f32>::open(&path, 4),
                Err(TriviumError::DatabaseLocked(_))
            ));
            peer.insert_with_id(9, &[9.; 4], json!({"kind":"peer"}))
                .unwrap();
            // 同一发布阶段的另一个数据库仍可完成持久化，屏障只匹配当前关闭实例。
            peer.flush().unwrap();
            assert_eq!(
                peer.get_payload(9).unwrap().unwrap(),
                json!({"kind":"peer"})
            );
            pause.release();
            assert!(db.close_timeout(Duration::from_secs(10)).unwrap());
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            assert_eq!(runtime.stats().attached_databases, 1);
            assert_eq!(runtime.stats().reserved_database_bytes, 8 * 1024 * 1024);
            released(&first_root);
            let mut reader = Database::<f32>::open_with_config(
                &path,
                Config {
                    dim: 4,
                    load_text_index: true,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(text_ids(&reader, "BM25", "closingtoken"), vec![1]);
            reader.close().unwrap();
            peer.close().unwrap();
            assert_eq!(runtime.stats().attached_databases, 0);
            assert_eq!(runtime.stats().reserved_maintenance_bytes, 0);
            released(&first_root);
            released(&peer_root);
            cleanup(&first_root);
            cleanup(&peer_root);
            std::fs::remove_dir(parent).unwrap();
            std::hint::black_box((db, peer, reader));
        }
    }
}

#[test]
fn 关闭进行中或关闭失败后丢弃对象_后台任务与错误记录不泄漏库资源() {
    use triviumdb::test_hooks::{ConcurrencyPoint, pause_at_path};
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        for outcome in 0..3 {
            let root = root();
            let path = root
                .canonicalize()
                .unwrap()
                .join("sample.tdb")
                .to_string_lossy()
                .into_owned();
            seed::<f32>(&path, mode);
            let runtime = ResourceRuntime::new(ResourceLimits {
                memory_limit: 64 * 1024 * 1024,
                maintenance_memory_limit: 8 * 1024 * 1024,
                ..Default::default()
            })
            .unwrap();
            let mut db = Database::<f32>::open_with_runtime(
                &path,
                Config {
                    dim: 4,
                    storage_mode: mode,
                    memory_limit: 8 * 1024 * 1024,
                    load_text_index: true,
                    auto_build_quiver: false,
                    ..Default::default()
                },
                runtime.clone(),
            )
            .unwrap();
            let saved = db.get(1).unwrap().unwrap();
            let drops = Arc::new(AtomicUsize::new(0));
            db.set_hook(HookOwner(drops.clone())).unwrap();
            db.insert_with_id(3, &[3.; 4], json!({"kind":"new"}))
                .unwrap();
            db.update_payload(1, json!({"kind":"changed"})).unwrap();
            db.index_text(1, "finaltoken").unwrap();
            if outcome == 0 {
                let pause = pause_at_path(ConcurrencyPoint::AfterSidecarsPersisted, &path);
                assert!(!db.close_timeout(Duration::ZERO).unwrap());
                arrived(&pause);
                drop(db);
                assert_eq!(runtime.stats().attached_databases, 1);
                assert_eq!(drops.load(Ordering::SeqCst), 0);
                assert!(matches!(
                    Database::<f32>::open(&path, 4),
                    Err(TriviumError::DatabaseLocked(_))
                ));
                pause.release();
            } else {
                {
                    let _failure = fail_io_at(IoPoint::GraphSidecarWrite);
                    assert!(matches!(
                        close(&mut db, outcome == 2),
                        Err(TriviumError::Io(_))
                    ));
                }
                drop(db);
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut reopened = loop {
                match Database::<f32>::open_with_config(
                    &path,
                    Config {
                        dim: 4,
                        storage_mode: mode,
                        load_text_index: true,
                        auto_build_quiver: false,
                        ..Default::default()
                    },
                ) {
                    Ok(db) => break db,
                    Err(TriviumError::DatabaseLocked(_)) => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "丢弃对象后库资源未在期限内释放"
                        );
                        std::thread::yield_now();
                    }
                    Err(error) => panic!("丢弃对象后的可写恢复失败：{error}"),
                }
            };
            assert_eq!(runtime.stats().attached_databases, 0);
            assert_eq!(runtime.stats().reserved_database_bytes, 0);
            assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            assert_eq!(saved.payload, json!({"kind":"original"}));
            assert_eq!(reopened.node_count().unwrap(), 3);
            assert_eq!(
                reopened.get_payload(1).unwrap().unwrap(),
                json!({"kind":"changed"})
            );
            assert_eq!(
                text_ids(&reopened, "BM25", "finaltoken"),
                if outcome == 0 { vec![1] } else { vec![] }
            );
            assert_eq!(
                text_ids(&reopened, "BM25", "originaltoken"),
                if outcome != 0 { vec![1] } else { vec![] }
            );
            reopened.close().unwrap();
            released(&root);
            cleanup(&root);
            std::hint::black_box((reopened, runtime, saved));
        }
    }
}
