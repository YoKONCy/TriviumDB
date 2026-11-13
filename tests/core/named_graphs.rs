//! 命名图的独立事实、共享引用、事务恢复与非UI公共契约。
use super::support::TestDir;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use triviumdb::database::{AccessMode, Config, StorageMode, TxBuilder};
use triviumdb::named_graph::*;
use triviumdb::{Database, VectorType};

const A: u64 = 9007199254740993;
const B: u64 = 9007199254740995;
const C: u64 = 9007199254740997;

fn fixture() -> Value {
    serde_json::from_str(include_str!("../contracts/named_graph_cases.json")).unwrap()
}
fn graph(index: usize) -> NamedGraphWrite {
    serde_json::from_value(fixture()["graphs"][index].clone()).unwrap()
}
fn config(mode: StorageMode) -> Config {
    Config {
        dim: 2,
        storage_mode: mode,
        auto_build_quiver: false,
        ..Default::default()
    }
}
fn scope(ids: &[&str]) -> NamedGraphScope {
    NamedGraphScope {
        graph_ids: ids.iter().map(|id| (*id).into()).collect(),
        ..Default::default()
    }
}
fn seed<T: VectorType + Serialize + DeserializeOwned>(db: &mut Database<T>) {
    for (id, vector) in [(A, [1., 0.]), (B, [0., 1.]), (C, [1., 1.])] {
        db.insert_with_id(id, &vector.map(T::from_f32), json!({"id":id.to_string()}))
            .unwrap();
    }
    db.upsert_edge(A, C, "same", 9., json!({"owner":"default"}))
        .unwrap();
}
fn matrix<T: VectorType + Serialize + DeserializeOwned>() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let dir = TestDir::new("named_graph_matrix");
        let path = dir.file("db.tdb");
        let mut db = Database::<T>::open_with_config(&path, config(mode)).unwrap();
        seed(&mut db);
        let a = db.put_named_graph(graph(0), None).unwrap();
        let b = db.put_named_graph(graph(1), None).unwrap();
        let limits = NamedGraphLimits::default();
        let one = db
            .traverse_named_graph(&scope(&["词级图"]), A, 2, &limits)
            .unwrap();
        assert_eq!(one.members, [A, B]);
        assert_eq!(one.edges.len(), 1);
        assert_eq!(one.edges[0].edge.metadata, json!({"owner":"terms"}));
        let all = db
            .traverse_named_graph(&scope(&["词级图", "事件图"]), A, 2, &limits)
            .unwrap();
        assert_eq!(
            serde_json::to_value(all.members.iter().map(u64::to_string).collect::<Vec<_>>())
                .unwrap(),
            fixture()["expected"]["union_members"]
        );
        assert_eq!(
            serde_json::to_value(&all.edges).unwrap(),
            fixture()["expected"]["union_edges"]
        );
        let expected_all = json!({"revisions":{"词级图":a.revision,"事件图":b.revision},"members":fixture()["expected"]["union_members"],"edges":fixture()["expected"]["union_edges"]});
        assert_eq!(serde_json::to_value(&all).unwrap(), expected_all);
        let exact_bytes = serde_json::to_vec(&expected_all).unwrap().len();
        let exact_limits = NamedGraphLimits {
            max_return_bytes: exact_bytes,
            ..Default::default()
        };
        assert_eq!(
            db.named_graph_scope(&scope(&["词级图", "事件图"]), &exact_limits)
                .unwrap(),
            all
        );
        let short_limits = NamedGraphLimits {
            max_return_bytes: exact_bytes - 1,
            ..Default::default()
        };
        let error = db
            .named_graph_scope(&scope(&["词级图", "事件图"]), &short_limits)
            .unwrap_err();
        assert_eq!(error.code(), "QueueFull");
        assert_eq!(error.details()["resource"], "named_graph_return_bytes");
        assert_eq!(db.get_edges(A).unwrap().len(), 1);
        assert_eq!(
            db.get_edges(A).unwrap()[0].metadata,
            json!({"owner":"default"})
        );
        let default = db
            .named_graph_scope(
                &NamedGraphScope {
                    include_default: true,
                    ..Default::default()
                },
                &limits,
            )
            .unwrap();
        assert_eq!(default.members, [A, B, C]);
        assert_eq!(default.edges.len(), 1);
        assert_eq!(default.edges[0].graph_id, None);
        db.update_payload(B, json!({"shared":"changed"})).unwrap();
        assert_eq!(
            db.get_named_graph("词级图", &limits).unwrap(),
            Some(a.clone())
        );
        assert_eq!(
            db.get_named_graph("事件图", &limits).unwrap(),
            Some(b.clone())
        );
        db.flush().unwrap();
        db.close().unwrap();
        let mut db = Database::<T>::open_with_config(&path, config(mode)).unwrap();
        assert_eq!(
            db.get_named_graph("词级图", &limits).unwrap(),
            Some(a.clone())
        );
        assert_eq!(
            db.get_named_graph("事件图", &limits).unwrap(),
            Some(b.clone())
        );
        assert_eq!(
            db.get_payload(B).unwrap(),
            Some(json!({"shared":"changed"}))
        );
        assert_eq!(
            db.named_graph_scope(&scope(&["词级图", "事件图"]), &limits)
                .unwrap(),
            all
        );
        let changed = db
            .edit_named_graph(
                "词级图",
                &a.revision,
                vec![NamedGraphEdit::RemoveMember { node_id: B }],
            )
            .unwrap();
        assert_eq!(changed.graph.members, [A]);
        assert!(changed.graph.edges.is_empty());
        assert_eq!(
            db.get_named_graph("事件图", &limits).unwrap(),
            Some(b.clone())
        );
        assert!(db.get(B).unwrap().is_some());
        let error = db.put_named_graph(graph(0), Some(&a.revision)).unwrap_err();
        assert_eq!(error.code(), "TDB_GRAPH_REVISION_MISMATCH");
        assert_eq!(
            error.details(),
            json!({"graph_id":"词级图","expected":a.revision,"actual":changed.revision})
        );
        db.delete_named_graph("词级图", &changed.revision).unwrap();
        let recreated = db.put_named_graph(graph(0), None).unwrap();
        assert_ne!(recreated.revision, a.revision);
        assert_eq!(
            db.delete_named_graph("词级图", &a.revision)
                .unwrap_err()
                .code(),
            "TDB_GRAPH_REVISION_MISMATCH"
        );
        db.delete(B).unwrap();
        let a = db.get_named_graph("词级图", &limits).unwrap().unwrap();
        let b_after = db.get_named_graph("事件图", &limits).unwrap().unwrap();
        assert_eq!(a.graph.members, [A]);
        assert!(a.graph.edges.is_empty());
        assert_eq!(b_after.graph.members, [A, C]);
        assert!(b_after.graph.edges.is_empty());
        assert_ne!(b_after.revision, b.revision);
        assert_eq!(db.get_edges(A).unwrap()[0].target_id, C);
        db.close().unwrap();
        let mut db = Database::<T>::open_with_config(&path, config(mode)).unwrap();
        assert_eq!(
            db.get_named_graph("事件图", &limits).unwrap(),
            Some(b_after)
        );
        assert!(db.get(B).unwrap().is_none());
        db.close().unwrap();
    }
}
#[test]
fn 三种向量类型与两种存储模式共享同一图契约() {
    matrix::<f32>();
    matrix::<half::f16>();
    matrix::<u64>();
}

#[test]
fn 混合节点图事务按顺序预检且失败保持原图与日志() {
    let dir = TestDir::new("named_graph_tx");
    let path = dir.file("db.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    let mut tx = TxBuilder::new();
    tx.insert_with_id(A, &[1., 0.], json!({"tx":true}));
    tx.insert_with_id(B, &[0., 1.], json!({"tx":true}));
    let snapshot = tx.put_named_graph(graph(0), None).unwrap();
    assert_eq!(db.commit_tx(tx).unwrap(), [A, B]);
    assert_eq!(
        db.get_named_graph("词级图", &Default::default()).unwrap(),
        Some(snapshot.clone())
    );
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    let mut tx = TxBuilder::new();
    tx.delete(A);
    tx.put_named_graph(graph(0), Some(&snapshot.revision))
        .unwrap();
    assert_eq!(db.commit_tx(tx).unwrap_err().code(), "TDB_NODE_NOT_FOUND");
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert!(db.get(A).unwrap().is_some());
    assert_eq!(
        db.get_named_graph("词级图", &Default::default()).unwrap(),
        Some(snapshot)
    );
    let mut tx = TxBuilder::new();
    tx.delete(A);
    tx.delete(B);
    db.commit_tx(tx).unwrap();
    let empty = db
        .get_named_graph("词级图", &Default::default())
        .unwrap()
        .unwrap();
    assert!(empty.graph.members.is_empty());
    assert!(empty.graph.edges.is_empty());
    db.close().unwrap();
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    assert_eq!(
        db.get_named_graph("词级图", &Default::default()).unwrap(),
        Some(empty)
    );
    assert!(db.all_node_ids().unwrap().is_empty());
    db.close().unwrap();
}

#[test]
fn 非法成员重复边修订和零预算在写日志前结构化拒绝() {
    let dir = TestDir::new("named_graph_reject");
    let path = dir.file("db.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Rom)).unwrap();
    seed(&mut db);
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    for input in [
        json!({"graph_id":"bad","members":["1"],"edges":[]}),
        json!({"graph_id":"bad","members":["0"],"edges":[]}),
        json!({"graph_id":"bad","members":[A.to_string(),A.to_string()],"edges":[]}),
        json!({"graph_id":"bad","members":[A.to_string()],"edges":[{"source_id":A.to_string(),"target_id":B.to_string(),"label":"x","weight":1}]}),
    ] {
        let error = triviumdb::sdk::named_graphs::request(
            &mut db,
            &json!({"operation":"put","graph":input}).to_string(),
        )
        .unwrap_err();
        assert!(["TDB_NODE_NOT_FOUND", "TDB_INVALID_INPUT"].contains(&error.code()));
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
        assert!(
            db.list_named_graphs(&Default::default())
                .unwrap()
                .is_empty()
        );
    }
    let a = db.put_named_graph(graph(0), None).unwrap();
    let mut bad = graph(0);
    bad.edges.push(bad.edges[0].clone());
    assert_eq!(
        db.put_named_graph(bad, Some(&a.revision))
            .unwrap_err()
            .code(),
        "TDB_INVALID_INPUT"
    );
    for limits in [
        NamedGraphLimits {
            max_nodes: 0,
            ..Default::default()
        },
        NamedGraphLimits {
            max_edges: 0,
            ..Default::default()
        },
        NamedGraphLimits {
            max_return_bytes: 0,
            ..Default::default()
        },
        NamedGraphLimits {
            max_graphs: 0,
            ..Default::default()
        },
    ] {
        assert_eq!(
            db.named_graph_scope(&scope(&["词级图"]), &limits)
                .unwrap_err()
                .code(),
            "QueueFull"
        );
    }
    assert_eq!(
        db.named_graph_scope(&Default::default(), &Default::default())
            .unwrap_err()
            .code(),
        "TDB_INVALID_INPUT"
    );
    assert_eq!(
        db.named_graph_scope(&scope(&["missing"]), &Default::default())
            .unwrap_err()
            .code(),
        "TDB_GRAPH_NOT_FOUND"
    );
    assert_eq!(
        db.get_named_graph("词级图", &Default::default()).unwrap(),
        Some(a)
    );
    db.close().unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn 图写入与全库删除的应用故障通过日志恢复精确修订() {
    use triviumdb::test_hooks::{IoPoint, fail_io_at};
    for delete in [false, true] {
        let dir = TestDir::new("named_graph_recover");
        let path = dir.file("db.tdb");
        let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
        seed(&mut db);
        let original = db.put_named_graph(graph(0), None).unwrap();
        db.flush().unwrap();
        let failure = fail_io_at(IoPoint::TransactionApply);
        let result = if delete {
            let mut tx = TxBuilder::new();
            tx.delete(B);
            db.commit_tx(tx).map(|_| ())
        } else {
            db.edit_named_graph(
                "词级图",
                &original.revision,
                vec![NamedGraphEdit::SetMetadata {
                    metadata: json!({"recovered":true}),
                }],
            )
            .map(|_| ())
        };
        let error = result.unwrap_err();
        assert_eq!(
            error.info().write.unwrap().outcome,
            triviumdb::error::WriteOutcome::Unknown
        );
        drop(failure);
        drop(db);
        let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
        let recovered = db
            .get_named_graph("词级图", &Default::default())
            .unwrap()
            .unwrap();
        let incarnation = original.revision.rsplit_once(':').unwrap().0;
        assert_eq!(recovered.revision, format!("{incarnation}:2"));
        if delete {
            assert_eq!(recovered.graph.members, [A]);
            assert!(recovered.graph.edges.is_empty());
            assert!(db.get(B).unwrap().is_none());
        } else {
            assert_eq!(recovered.graph.members, [A, B]);
            assert_eq!(recovered.graph.edges, original.graph.edges);
            assert_eq!(recovered.graph.metadata, json!({"recovered":true}));
        }
        db.flush().unwrap();
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
        assert_eq!(
            db.get_named_graph("词级图", &Default::default()).unwrap(),
            Some(recovered)
        );
        db.close().unwrap();
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn 检查点发布后日志清理失败保留跨重建和节点删除的最终图修订() {
    use triviumdb::test_hooks::{IoPoint, fail_io_at};
    let dir = TestDir::new("named_graph_checkpoint");
    let path = dir.file("db.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    seed(&mut db);
    let original = db.put_named_graph(graph(0), None).unwrap();
    db.delete_named_graph("词级图", &original.revision).unwrap();
    let current = db.put_named_graph(graph(0), None).unwrap();
    db.delete(B).unwrap();
    let expected = db
        .get_named_graph("词级图", &Default::default())
        .unwrap()
        .unwrap();
    assert_ne!(current.revision, expected.revision);
    let failure = fail_io_at(IoPoint::CheckpointWalClear);
    assert_eq!(db.flush().unwrap_err().code(), "TDB_IO");
    drop(db);
    drop(failure);
    assert!(std::fs::metadata(format!("{path}.wal")).unwrap().len() > 6);
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    assert_eq!(
        db.get_named_graph("词级图", &Default::default()).unwrap(),
        Some(expected.clone())
    );
    assert!(db.get(B).unwrap().is_none());
    assert_eq!(
        db.get_payload(A).unwrap(),
        Some(json!({"id":A.to_string()}))
    );
    db.close().unwrap();
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    assert_eq!(
        db.get_named_graph("词级图", &Default::default()).unwrap(),
        Some(expected)
    );
    db.close().unwrap();
}

#[test]
fn 旧十一版检查点和四版日志保留默认图事实() {
    fn run<T: VectorType + Serialize + DeserializeOwned>(dtype: &str) {
        let dir = TestDir::new("named_graph_history");
        let path = dir.file("db.tdb");
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/pre_named_graph_format11")
            .join(dtype);
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let name = entry
                .file_name()
                .to_string_lossy()
                .replacen("sample.tdb", "db.tdb", 1);
            std::fs::copy(
                entry.path(),
                std::path::Path::new(&path).parent().unwrap().join(name),
            )
            .unwrap();
        }
        let mut db = Database::<T>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
        assert_eq!(db.all_node_ids().unwrap(), [11, 22, 33]);
        assert_eq!(
            db.get(11).unwrap().unwrap().vector,
            vec![T::from_f32(1.), T::zero()]
        );
        assert_eq!(
            db.get_payload(11).unwrap(),
            Some(json!({"name":"共享节点"}))
        );
        assert_eq!(
            db.get_payload(33).unwrap(),
            Some(json!({"confirmed":"wal-v4"}))
        );
        let edges = db.get_edges(11).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(
            (edges[0].target_id, edges[0].label.as_str(), edges[0].weight),
            (22, "same", 0.5)
        );
        assert!(
            db.list_named_graphs(&Default::default())
                .unwrap()
                .is_empty()
        );
        db.flush().unwrap();
        db.close().unwrap();
        let mut db = Database::<T>::open_with_config(&path, config(StorageMode::Rom)).unwrap();
        assert_eq!(db.all_node_ids().unwrap(), [11, 22, 33]);
        assert_eq!(db.get_edges(11).unwrap(), edges);
        db.close().unwrap();
    }
    run::<f32>("f32");
    run::<half::f16>("f16");
    run::<u64>("u64");
}

#[test]
fn 只读查询保持完整文件组字节且关闭后拥有结果不占用映射() {
    let dir = TestDir::new("named_graph_readonly");
    let path = dir.file("db.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    seed(&mut db);
    let a = db.put_named_graph(graph(0), None).unwrap();
    db.close().unwrap();
    fn files(path: &str) -> std::collections::BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(std::path::Path::new(path).parent().unwrap())
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read(entry.path()).unwrap(),
                )
            })
            .collect()
    }
    let before = files(&path);
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            access_mode: AccessMode::ReadOnly,
            ..config(StorageMode::Mmap)
        },
    )
    .unwrap();
    let result = db
        .named_graph_scope(&scope(&["词级图"]), &Default::default())
        .unwrap();
    assert_eq!(
        db.put_named_graph(graph(1), None).unwrap_err().code(),
        "TDB_READ_ONLY"
    );
    assert_eq!(
        db.get_named_graph("词级图", &Default::default()).unwrap(),
        Some(a)
    );
    db.close().unwrap();
    assert_eq!(files(&path), before);
    assert_eq!(result.members, [A, B]);
    for entry in std::fs::read_dir(std::path::Path::new(&path).parent().unwrap()).unwrap() {
        let entry = entry.unwrap();
        let target = entry.path().with_extension("moved");
        std::fs::rename(entry.path(), &target).unwrap();
        std::fs::remove_file(target).unwrap();
    }
    assert_eq!(result.edges[0].edge.metadata, json!({"owner":"terms"}));
}

#[cfg(feature = "test-hooks")]
#[test]
fn 图读视图取消释放工作区并保留后续完整查询() {
    let dir = TestDir::new("named_graph_cancel");
    let path = dir.file("db.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    seed(&mut db);
    db.put_named_graph(graph(0), None).unwrap();
    let expected = db
        .named_graph_scope(&scope(&["词级图"]), &Default::default())
        .unwrap();
    let db = std::sync::Arc::new(db);
    let context =
        triviumdb::sdk::RequestContext::new("named-graph-cancel", Default::default(), false)
            .unwrap();
    let gate =
        triviumdb::test_hooks::pause_at(triviumdb::test_hooks::ConcurrencyPoint::NamedGraphRead);
    let worker_db = db.clone();
    let worker_context = context.clone();
    let worker = std::thread::spawn(move || {
        worker_context.run(|| worker_db.named_graph_scope(&scope(&["词级图"]), &Default::default()))
    });
    gate.wait_until_arrived();
    context.cancel();
    drop(gate);
    assert_eq!(
        worker.join().unwrap().unwrap_err().code(),
        "TDB_QUERY_CANCELLED"
    );
    assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
    assert_eq!(
        db.named_graph_scope(&scope(&["词级图"]), &Default::default())
            .unwrap(),
        expected
    );
    let mut db = std::sync::Arc::into_inner(db).unwrap();
    db.close().unwrap();
}

#[test]
fn 快照恢复压实及维度迁移保留命名图身份和完整边() {
    let dir = TestDir::new("named_graph_transfer");
    let path = dir.file("source.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    seed(&mut db);
    let a = db.put_named_graph(graph(0), None).unwrap();
    let b = db.put_named_graph(graph(1), None).unwrap();
    db.compact().unwrap();
    let snapshot_dir = dir.file("snapshot");
    db.create_snapshot(&snapshot_dir, "named-graphs").unwrap();
    let restored = dir.file("restored.tdb");
    triviumdb::storage::snapshot::restore_snapshot(&snapshot_dir, &restored).unwrap();
    let restored_path = std::path::Path::new(&restored).join("database.tdb");
    let mut restored = Database::<f32>::open_with_config(
        restored_path.to_str().unwrap(),
        config(StorageMode::Mmap),
    )
    .unwrap();
    assert_eq!(
        restored
            .get_named_graph("词级图", &Default::default())
            .unwrap(),
        Some(a.clone())
    );
    assert_eq!(
        restored
            .get_named_graph("事件图", &Default::default())
            .unwrap(),
        Some(b.clone())
    );
    assert_eq!(
        restored.get_payload(B).unwrap(),
        Some(json!({"id":B.to_string()}))
    );
    restored.close().unwrap();
    let (mut migrated, ids) = db.migrate_to(&dir.file("migrated.tdb"), 3).unwrap();
    assert_eq!(ids, [A, B, C]);
    assert_eq!(
        migrated
            .get_named_graph("词级图", &Default::default())
            .unwrap(),
        Some(a)
    );
    assert_eq!(
        migrated
            .get_named_graph("事件图", &Default::default())
            .unwrap(),
        Some(b)
    );
    assert_eq!(migrated.get(B).unwrap().unwrap().vector, [0., 0., 0.]);
    migrated.close().unwrap();
    db.close().unwrap();
}

#[test]
fn 边更新自环标签删除及作用域修订保持图内唯一性() {
    let dir = TestDir::new("named_graph_edge_edits");
    let path = dir.file("db.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Rom)).unwrap();
    seed(&mut db);
    let a = db.put_named_graph(graph(0), None).unwrap();
    let b = db.put_named_graph(graph(1), None).unwrap();
    let edge = NamedGraphEdge {
        source_id: A,
        target_id: B,
        label: "same".into(),
        weight: 7.,
        metadata: json!({"changed":true}),
    };
    let loop_edge = NamedGraphEdge {
        source_id: A,
        target_id: A,
        label: "loop".into(),
        weight: -1.,
        metadata: json!({"loop":true}),
    };
    let updated = db
        .edit_named_graph(
            "词级图",
            &a.revision,
            vec![
                NamedGraphEdit::UpsertEdge { edge: edge.clone() },
                NamedGraphEdit::UpsertEdge {
                    edge: loop_edge.clone(),
                },
            ],
        )
        .unwrap();
    assert_eq!(updated.graph.edges, vec![loop_edge.clone(), edge]);
    assert_eq!(
        db.get_named_graph("事件图", &Default::default()).unwrap(),
        Some(b)
    );
    let removed = db
        .edit_named_graph(
            "词级图",
            &updated.revision,
            vec![NamedGraphEdit::RemoveEdge {
                source_id: A,
                target_id: B,
                label: "same".into(),
            }],
        )
        .unwrap();
    assert_eq!(removed.graph.edges, vec![loop_edge]);
    let stale = NamedGraphScope {
        expected_revisions: std::collections::BTreeMap::from([("词级图".into(), a.revision)]),
        ..scope(&["词级图"])
    };
    assert_eq!(
        db.named_graph_scope(&stale, &Default::default())
            .unwrap_err()
            .code(),
        "TDB_GRAPH_REVISION_MISMATCH"
    );
    assert_eq!(
        db.edit_named_graph(
            "词级图",
            &removed.revision,
            vec![NamedGraphEdit::RemoveMember { node_id: 0 }]
        )
        .unwrap_err()
        .code(),
        "TDB_INVALID_INPUT"
    );
    assert_eq!(
        db.get_named_graph("词级图", &Default::default()).unwrap(),
        Some(removed)
    );
    db.close().unwrap();
}

#[test]
fn 旧十一版检查点与空四版日志只读打开保持所有字节() {
    let dir = TestDir::new("named_graph_history_readonly");
    let path = dir.file("db.tdb");
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pre_named_graph_format11/f32");
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry
            .file_name()
            .to_string_lossy()
            .replacen("sample.tdb", "db.tdb", 1);
        std::fs::copy(
            entry.path(),
            std::path::Path::new(&path).parent().unwrap().join(name),
        )
        .unwrap();
    }
    std::fs::OpenOptions::new()
        .write(true)
        .open(format!("{path}.wal"))
        .unwrap()
        .set_len(6)
        .unwrap();
    // 只读打开使用已有协调锁，空WAL保持历史版本。
    std::fs::write(format!("{path}.lock"), []).unwrap();
    let read_files = || {
        std::fs::read_dir(std::path::Path::new(&path).parent().unwrap())
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), std::fs::read(entry.path()).unwrap())
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let before = read_files();
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            access_mode: AccessMode::ReadOnly,
            ..config(StorageMode::Mmap)
        },
    )
    .unwrap();
    assert_eq!(db.all_node_ids().unwrap(), [11, 22]);
    assert_eq!(db.get_edges(11).unwrap()[0].target_id, 22);
    assert!(
        db.list_named_graphs(&Default::default())
            .unwrap()
            .is_empty()
    );
    db.close().unwrap();
    assert_eq!(read_files(), before);
}

#[test]
fn 类型化图与边元数据深度在编码克隆和写日志之前拒绝() {
    fn deep() -> Value {
        // 200层超过现有JSON契约的127层容器上限。
        (0..200).fold(Value::Null, |value, _| Value::Array(vec![value]))
    }
    let dir = TestDir::new("named_graph_json_depth");
    let path = dir.file("db.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Rom)).unwrap();
    seed(&mut db);
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    let mut bad = graph(0);
    bad.metadata = deep();
    assert_eq!(
        db.put_named_graph(bad, None).unwrap_err().code(),
        "TDB_INVALID_INPUT"
    );
    let mut bad = graph(0);
    bad.edges[0].metadata = deep();
    assert_eq!(
        db.put_named_graph(bad, None).unwrap_err().code(),
        "TDB_INVALID_INPUT"
    );
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert!(
        db.list_named_graphs(&Default::default())
            .unwrap()
            .is_empty()
    );
    let original = db.put_named_graph(graph(0), None).unwrap();
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    assert_eq!(
        db.edit_named_graph(
            "词级图",
            &original.revision,
            vec![NamedGraphEdit::SetMetadata { metadata: deep() }]
        )
        .unwrap_err()
        .code(),
        "TDB_INVALID_INPUT"
    );
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert_eq!(
        db.get_named_graph("词级图", &Default::default()).unwrap(),
        Some(original)
    );
    db.close().unwrap();
}

#[test]
fn 缺失图的空值响应遵守四字节编码额度() {
    let dir = TestDir::new("named_graph_null_budget");
    let path = dir.file("db.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Rom)).unwrap();
    assert!(
        db.get_named_graph(
            "missing",
            &NamedGraphLimits {
                max_return_bytes: 4,
                ..Default::default()
            }
        )
        .unwrap()
        .is_none()
    );
    for max_return_bytes in [0, 3] {
        let error = db
            .get_named_graph(
                "missing",
                &NamedGraphLimits {
                    max_return_bytes,
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert_eq!(error.code(), "QueueFull");
        assert_eq!(error.details()["resource"], "named_graph_return_bytes");
    }
    assert!(
        db.list_named_graphs(&Default::default())
            .unwrap()
            .is_empty()
    );
    db.close().unwrap();
}
