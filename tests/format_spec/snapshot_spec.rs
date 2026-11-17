use super::fixture::*;
use super::mutation::{Mutation, field_boundary_mutations, truncation_mutations};
use super::spec::{FLUSH_MARKER, FileRole, TDB_HEADER};
use triviumdb::database::Database;

#[test]
fn 当前提交标记字段边界_crc及未知版本矩阵() {
    let source = seed("flush_marker_source");
    let marker = std::fs::read(format!("{source}.flush_ok")).unwrap();
    assert_eq!(marker.len(), 53);
    assert_eq!(marker[4], 3);
    let control = Database::<f32>::open_read_only(&source, DIM).unwrap();
    assert_eq!(control.all_node_ids().unwrap(), [1, 2, 3, 4, 5, 6, 7, 8]);
    drop(control);

    let mutations = field_boundary_mutations(FLUSH_MARKER)
        .into_iter()
        .chain(truncation_mutations(marker.len(), FLUSH_MARKER))
        .chain([
            Mutation::FlipBit { offset: 49, bit: 0 },
            Mutation::Append(vec![0xa5; 8]),
        ])
        // 边界值可能恰好等于原字段；原始合法输入由上面的正向对照验证。
        .filter(|mutation| mutation.apply(&marker) != marker);
    for (index, mutation) in mutations.enumerate() {
        let candidate = path(&format!("flush_marker_case_{index}"));
        copy_roles(
            &source,
            &candidate,
            &[FileRole::Tdb, FileRole::Vec, FileRole::Payload],
        );
        std::fs::write(format!("{candidate}.flush_ok"), mutation.apply(&marker)).unwrap();
        assert!(
            Database::<f32>::open_read_only(&candidate, DIM).is_err(),
            "变异应被拒绝: {mutation:?}"
        );
        assert_read_only_zero_write(&candidate);
        cleanup(&candidate);
    }
    cleanup(&source);
}

#[test]
fn marker修复自身crc后_错误尺寸和文件crc仍被语义校验拒绝() {
    let source = seed("marker_repair_source");
    let marker = std::fs::read(format!("{source}.flush_ok")).unwrap();
    for field_name in [
        "tdb_size",
        "vec_size",
        "payload_size",
        "tdb_crc",
        "vec_crc",
        "payload_crc",
    ] {
        let field = *FLUSH_MARKER
            .iter()
            .find(|field| field.name == field_name)
            .unwrap();
        let candidate = path(&format!("marker_repair_{field_name}"));
        copy_roles(
            &source,
            &candidate,
            &[FileRole::Tdb, FileRole::Vec, FileRole::Payload],
        );
        let changed = Mutation::RepairFlushMarkerCrc.apply(
            &Mutation::SetField {
                field,
                value: super::mutation::BoundaryValue::Max,
            }
            .apply(&marker),
        );
        std::fs::write(format!("{candidate}.flush_ok"), changed).unwrap();
        assert!(Database::<f32>::open_read_only(&candidate, DIM).is_err());
        assert_read_only_zero_write(&candidate);
        cleanup(&candidate);
    }
    cleanup(&source);
}

#[test]
fn tdb头部每个字段边界与逐字段截断均不panic且只读零写() {
    let source = seed("tdb_header_source");
    let tdb = std::fs::read(&source).unwrap();
    let mutations = field_boundary_mutations(TDB_HEADER)
        .into_iter()
        .chain(truncation_mutations(tdb.len(), TDB_HEADER));
    for (index, mutation) in mutations.enumerate() {
        let candidate = path(&format!("tdb_header_case_{index}"));
        copy_roles(
            &source,
            &candidate,
            &[FileRole::Vec, FileRole::Payload, FileRole::FlushMarker],
        );
        std::fs::write(&candidate, mutation.apply(&tdb)).unwrap();
        let _ = Database::<f32>::open_read_only(&candidate, DIM);
        assert_read_only_zero_write(&candidate);
        cleanup(&candidate);
    }
    cleanup(&source);
}

#[test]
fn vec长度元素边界和非有限值由跨文件校验拒绝() {
    let source = seed("vec_spec_source");
    let bytes = std::fs::read(format!("{source}.vec")).unwrap();
    let variants = [
        Vec::new(),
        bytes[..bytes.len() - 1].to_vec(),
        bytes[..bytes.len() - 4].to_vec(),
        [bytes.clone(), vec![0]].concat(),
        [bytes.clone(), vec![0; 4]].concat(),
    ];
    for (index, variant) in variants.into_iter().enumerate() {
        let candidate = path(&format!("vec_length_case_{index}"));
        copy_roles(
            &source,
            &candidate,
            &[FileRole::Tdb, FileRole::Payload, FileRole::FlushMarker],
        );
        std::fs::write(format!("{candidate}.vec"), variant).unwrap();
        assert!(Database::<f32>::open_read_only(&candidate, DIM).is_err());
        assert_read_only_zero_write(&candidate);
        cleanup(&candidate);
    }
    cleanup(&source);
}
