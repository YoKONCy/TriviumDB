use super::fixture::*;
use super::mutation::{BoundaryValue, Mutation, field_boundary_mutations, truncation_mutations};
use super::spec::{FileRole, GRAPH_HEADER, PROPERTY_HEADER};
use triviumdb::database::Database;

fn mutate_sidecar(
    source: &str,
    role: FileRole,
    header: &[super::spec::FieldSpec],
    repair_crc: bool,
    prefix: &str,
) {
    let original = std::fs::read(format!("{source}{}", role.suffix())).unwrap();
    let mutations = field_boundary_mutations(header)
        .into_iter()
        .chain(truncation_mutations(original.len(), header))
        .chain([
            Mutation::FlipBit {
                offset: original.len() / 2,
                bit: 3,
            },
            Mutation::Append(vec![0x5a; 13]),
        ]);
    for (index, mutation) in mutations.enumerate() {
        let candidate = path(&format!("{prefix}_{index}"));
        copy_roles(
            source,
            &candidate,
            &[
                FileRole::Tdb,
                FileRole::Vec,
                FileRole::FlushMarker,
                FileRole::Wal,
            ],
        );
        let mut changed = mutation.apply(&original);
        if repair_crc && !matches!(mutation, Mutation::TruncateAt(_)) {
            changed = Mutation::RepairTrailingCrc.apply(&changed);
        }
        std::fs::write(format!("{candidate}{}", role.suffix()), changed).unwrap();
        let _ = Database::<f32>::open_read_only(&candidate, DIM);
        assert_read_only_zero_write(&candidate);
        cleanup(&candidate);
    }
}

#[test]
fn 属性索引字段边界_逐点截断_crc损坏和修复crc后语义非法均安全() {
    let source = seed("property_sidecar_source");
    mutate_sidecar(
        &source,
        FileRole::PropertyIndex,
        PROPERTY_HEADER,
        false,
        "property_raw",
    );
    mutate_sidecar(
        &source,
        FileRole::PropertyIndex,
        PROPERTY_HEADER,
        true,
        "property_repaired",
    );
    cleanup(&source);
}

#[test]
fn 图索引字段边界_逐点截断_crc损坏和修复crc后语义非法均安全() {
    let source = seed("graph_sidecar_source");
    mutate_sidecar(
        &source,
        FileRole::GraphIndex,
        GRAPH_HEADER,
        false,
        "graph_raw",
    );
    mutate_sidecar(
        &source,
        FileRole::GraphIndex,
        GRAPH_HEADER,
        true,
        "graph_repaired",
    );
    cleanup(&source);
}

#[test]
fn sidecar未知版本与恶意计数不得触发危险分配() {
    let source = seed("sidecar_budget_source");
    for (role, fields) in [
        (FileRole::PropertyIndex, PROPERTY_HEADER),
        (FileRole::GraphIndex, GRAPH_HEADER),
    ] {
        let original = std::fs::read(format!("{source}{}", role.suffix())).unwrap();
        for field_name in [
            "version",
            "format_version",
            "node_count",
            "field_count",
            "block_count",
        ] {
            let Some(field) = fields
                .iter()
                .find(|field| field.name == field_name)
                .copied()
            else {
                continue;
            };
            let candidate = path(&format!("sidecar_budget_{role:?}_{field_name}"));
            copy_roles(
                &source,
                &candidate,
                &[
                    FileRole::Tdb,
                    FileRole::Vec,
                    FileRole::FlushMarker,
                    FileRole::Wal,
                ],
            );
            let changed = Mutation::RepairTrailingCrc.apply(
                &Mutation::SetField {
                    field,
                    value: BoundaryValue::Max,
                }
                .apply(&original),
            );
            std::fs::write(format!("{candidate}{}", role.suffix()), changed).unwrap();
            let _ = Database::<f32>::open_read_only(&candidate, DIM);
            assert_read_only_zero_write(&candidate);
            cleanup(&candidate);
        }
    }
    cleanup(&source);
}
