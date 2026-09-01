use super::spec::FileRole;
use std::collections::BTreeMap;
use triviumdb::database::Database;

pub const DIM: usize = 4;
pub const ALL_ROLES: &[FileRole] = &[
    FileRole::Tdb,
    FileRole::Vec,
    FileRole::FlushMarker,
    FileRole::Wal,
    FileRole::PropertyIndex,
    FileRole::GraphIndex,
    FileRole::Quiver,
    FileRole::QuiverMeta,
    FileRole::Text,
    FileRole::TextMeta,
    FileRole::Manifest,
];

pub fn path(name: &str) -> String {
    let root = std::env::temp_dir().join("triviumdb_format_spec");
    std::fs::create_dir_all(&root).unwrap();
    root.join(format!("{name}_{}", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

pub fn cleanup(path: &str) {
    for role in ALL_ROLES {
        std::fs::remove_file(format!("{path}{}", role.suffix())).ok();
    }
    for suffix in [
        ".lock",
        ".tmp",
        ".vec.tmp",
        ".pidx.tmp",
        ".gidx.tmp",
        ".ready",
    ] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

pub fn seed(name: &str) -> String {
    let path = path(name);
    cleanup(&path);
    let mut database = Database::<f32>::open(&path, DIM).unwrap();
    for id in 1..=8 {
        database
            .insert_with_id(
                id,
                &[id as f32, 1.0, 0.0, -1.0],
                serde_json::json!({"kind": if id % 2 == 0 { "even" } else { "odd" }, "rank": id}),
            )
            .unwrap();
    }
    database
        .upsert_edge(1, 2, "next", 1.0, serde_json::json!({"index": 1}))
        .unwrap();
    database
        .upsert_edge(2, 3, "next", 0.5, serde_json::json!({"index": 2}))
        .unwrap();
    database.create_index("kind").unwrap();
    database.create_ordered_index("rank").unwrap();
    database.flush().unwrap();
    drop(database);
    path
}

pub fn copy_roles(source: &str, target: &str, roles: &[FileRole]) {
    cleanup(target);
    for role in roles {
        let source_file = format!("{source}{}", role.suffix());
        if std::path::Path::new(&source_file).exists() {
            std::fs::copy(source_file, format!("{target}{}", role.suffix())).unwrap();
        }
    }
}

pub fn directory_snapshot(path: &str) -> BTreeMap<String, Vec<u8>> {
    ALL_ROLES
        .iter()
        .filter_map(|role| {
            let file = format!("{path}{}", role.suffix());
            std::fs::read(&file).ok().map(|bytes| (file, bytes))
        })
        .collect()
}

pub fn assert_read_only_zero_write(path: &str) {
    let before = directory_snapshot(path);
    if let Ok(database) = Database::<f32>::open_read_only(path, DIM) {
        let _ = database.node_count();
        let _ = database.get_payload(1);
        let _ = database.search(&[1.0, 1.0, 0.0, -1.0], 3, 0, -1.0);
    }
    assert_eq!(
        directory_snapshot(path),
        before,
        "ReadOnly 修改了格式 fixture"
    );
}
