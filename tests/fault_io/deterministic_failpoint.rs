use triviumdb::Database;
use triviumdb::test_hooks::{IoPoint, fail_io_at};

const DIM: usize = 4;

fn path(name: &str) -> String {
    let directory = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&directory).unwrap();
    directory
        .join(format!("io_failpoint_{name}"))
        .to_string_lossy()
        .into_owned()
}

fn cleanup(path: &str) {
    for suffix in [
        "",
        ".wal",
        ".vec",
        ".lock",
        ".flush_ok",
        ".tmp",
        ".vec.tmp",
        ".flush_ok.tmp",
    ] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

fn seed(path: &str) {
    cleanup(path);
    let mut database = Database::<f32>::open(path, DIM).unwrap();
    for id in 1..=4u64 {
        database
            .insert_with_id(
                id,
                &[id as f32, 1.0, 2.0, 3.0],
                serde_json::json!({"id": id}),
            )
            .unwrap();
    }
    database.flush().unwrap();
}

#[test]
fn marker_io各操作定点失败均返回错误且不发布伪成功generation() {
    for point in [
        IoPoint::MarkerMetadata,
        IoPoint::MarkerCreate,
        IoPoint::MarkerWrite,
        IoPoint::MarkerSync,
        IoPoint::MarkerRename,
    ] {
        let path = path(&format!("{point:?}"));
        seed(&path);
        let old_marker = std::fs::read(format!("{path}.flush_ok")).unwrap();
        let mut database = Database::<f32>::open(&path, DIM).unwrap();
        database
            .insert_with_id(5, &[5.0, 1.0, 2.0, 3.0], serde_json::json!({"id": 5}))
            .unwrap();
        let guard = fail_io_at(point);
        let result = database.flush();
        drop(guard);
        assert!(result.is_err(), "{point:?} 故障必须传播到调用方");
        drop(database);

        let marker = std::fs::read(format!("{path}.flush_ok")).unwrap();
        assert_eq!(marker, old_marker, "{point:?} 故障不得发布新的提交标记");
        assert!(
            Database::<f32>::open_read_only(&path, DIM).is_err(),
            "跨文件撕裂必须 fail-closed"
        );
        cleanup(&path);
    }
}

#[test]
fn io_failpoint_guard作用域结束后恢复正常发布() {
    let path = path("guard_scope");
    seed(&path);
    let mut database = Database::<f32>::open(&path, DIM).unwrap();
    database
        .insert_with_id(5, &[5.0, 1.0, 2.0, 3.0], serde_json::json!({"id": 5}))
        .unwrap();
    {
        let _guard = fail_io_at(IoPoint::MarkerCreate);
        assert!(database.flush().is_err());
    }
    database.flush().unwrap();
    drop(database);
    let reader = Database::<f32>::open_read_only(&path, DIM).unwrap();
    assert_eq!(reader.node_count(), 5);
    cleanup(&path);
}
