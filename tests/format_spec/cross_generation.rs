use super::fixture::*;
use super::mutation::Mutation;
use super::spec::FileRole;
use triviumdb::database::Database;

fn generation(name: &str, count: u64) -> String {
    let path = path(name);
    cleanup(&path);
    let mut database = Database::<f32>::open(&path, DIM).unwrap();
    for id in 1..=count {
        database
            .insert_with_id(
                id,
                &[id as f32, 1.0, 0.0, 0.0],
                serde_json::json!({"generation": name, "id": id}),
            )
            .unwrap();
    }
    if count >= 2 {
        database
            .upsert_edge(1, 2, "next", 1.0, serde_json::Value::Null)
            .unwrap();
    }
    database.create_index("generation").unwrap();
    database.flush().unwrap();
    drop(database);
    path
}

#[test]
fn tdb_vec_marker所有跨代际组合均不得拼出混合快照() {
    let old = generation("generation_old", 4);
    let new = generation("generation_new", 7);
    let roles = [FileRole::Tdb, FileRole::Vec, FileRole::FlushMarker];
    for mask in 0..8u8 {
        let candidate = path(&format!("mixed_generation_{mask}"));
        let main_source = if mask & 1 == 0 { &old } else { &new };
        copy_roles(
            main_source,
            &candidate,
            &[FileRole::Tdb, FileRole::Vec, FileRole::FlushMarker],
        );
        for (bit, role) in roles.iter().enumerate() {
            let source = if mask & (1 << bit) == 0 { &old } else { &new };
            std::fs::copy(
                format!("{source}{}", role.suffix()),
                format!("{candidate}{}", role.suffix()),
            )
            .unwrap();
        }
        if mask == 0 || mask == 7 {
            let mut db = Database::<f32>::open_read_only(&candidate, DIM).unwrap();
            let count = if mask == 0 { 4 } else { 7 };
            assert_eq!(db.all_node_ids().unwrap(), (1..=count).collect::<Vec<_>>());
            for id in 1..=count {
                assert_eq!(db.get(id).unwrap().unwrap().vector, [id as f32, 1., 0., 0.]);
            }
            db.close().unwrap();
        } else {
            assert!(
                Database::<f32>::open_read_only(&candidate, DIM).is_err(),
                "混代 mask={mask} 被错误接受"
            );
        }
        assert_read_only_zero_write(&candidate);
        cleanup(&candidate);
    }
    cleanup(&old);
    cleanup(&new);
}

#[test]
fn sidecar跨代际不能产生幽灵节点或错误索引结果() {
    let old = generation("sidecar_old", 4);
    let new = generation("sidecar_new", 7);
    for role in [FileRole::PropertyIndex, FileRole::GraphIndex] {
        let candidate = path(&format!("mixed_sidecar_{role:?}"));
        copy_roles(
            &new,
            &candidate,
            &[
                FileRole::Tdb,
                FileRole::Vec,
                FileRole::FlushMarker,
                FileRole::Wal,
            ],
        );
        crate::artifact::copy(
            format!("{old}{}", role.suffix()),
            format!("{candidate}{}", role.suffix()),
        )
        .unwrap();
        let error = Database::<f32>::open_read_only(&candidate, DIM)
            .err()
            .expect("错配的索引定义或图目录必须拒绝");
        assert_eq!(error.code(), "TDB_CORRUPTED_FILE", "{role:?}");
        assert_read_only_zero_write(&candidate);
        cleanup(&candidate);
    }
    cleanup(&old);
    cleanup(&new);
}

#[test]
fn 修复marker_crc不能掩盖tdb和vec内容损坏() {
    let source = generation("cross_crc", 5);
    for role in [FileRole::Tdb, FileRole::Vec] {
        let candidate = path(&format!("cross_crc_{role:?}"));
        copy_roles(
            &source,
            &candidate,
            &[FileRole::Tdb, FileRole::Vec, FileRole::FlushMarker],
        );
        let file = format!("{candidate}{}", role.suffix());
        let original = std::fs::read(&file).unwrap();
        let changed = Mutation::FlipBit {
            offset: original.len() / 2,
            bit: 4,
        }
        .apply(&original);
        std::fs::write(&file, changed).unwrap();
        let marker_path = format!("{candidate}.flush_ok");
        let marker = std::fs::read(&marker_path).unwrap();
        std::fs::write(&marker_path, Mutation::RepairFlushMarkerCrc.apply(&marker)).unwrap();
        assert!(Database::<f32>::open_read_only(&candidate, DIM).is_err());
        assert_read_only_zero_write(&candidate);
        cleanup(&candidate);
    }
    cleanup(&source);
}
