use super::spec::{FieldEncoding, FieldSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryValue {
    Zero,
    One,
    Max,
    BigEndianOne,
    AllA5,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mutation {
    SetField {
        field: FieldSpec,
        value: BoundaryValue,
    },
    TruncateAt(usize),
    FlipBit {
        offset: usize,
        bit: u8,
    },
    Append(Vec<u8>),
    RepairTrailingCrc,
    RepairFlushMarkerCrc,
}

fn encoded(field: FieldSpec, value: BoundaryValue) -> Vec<u8> {
    let width = field.width;
    match value {
        BoundaryValue::Zero => vec![0; width],
        BoundaryValue::One => {
            let mut output = vec![0; width];
            output[0] = 1;
            output
        }
        BoundaryValue::Max => vec![0xff; width],
        BoundaryValue::BigEndianOne => {
            let mut output = vec![0; width];
            output[width - 1] = 1;
            output
        }
        BoundaryValue::AllA5 => vec![0xa5; width],
    }
}

impl Mutation {
    pub fn apply(&self, input: &[u8]) -> Vec<u8> {
        let mut output = input.to_vec();
        match self {
            Self::SetField { field, value } => {
                if field.end() <= output.len() {
                    output[field.offset..field.end()].copy_from_slice(&encoded(*field, *value));
                }
            }
            Self::TruncateAt(offset) => output.truncate((*offset).min(output.len())),
            Self::FlipBit { offset, bit } => {
                if let Some(byte) = output.get_mut(*offset) {
                    *byte ^= 1 << (bit % 8);
                }
            }
            Self::Append(bytes) => output.extend_from_slice(bytes),
            Self::RepairTrailingCrc => {
                if output.len() >= 4 {
                    let checksum_offset = output.len() - 4;
                    let checksum = crc32fast::hash(&output[..checksum_offset]);
                    output[checksum_offset..].copy_from_slice(&checksum.to_le_bytes());
                }
            }
            Self::RepairFlushMarkerCrc => {
                if output.len() == 41 {
                    let checksum = crc32fast::hash(&output[..37]);
                    output[37..41].copy_from_slice(&checksum.to_le_bytes());
                }
            }
        }
        output
    }
}

pub fn field_boundary_mutations(fields: &[FieldSpec]) -> Vec<Mutation> {
    fields
        .iter()
        .flat_map(|field| {
            let values: &[BoundaryValue] = match field.encoding {
                FieldEncoding::Bytes => &[
                    BoundaryValue::Zero,
                    BoundaryValue::Max,
                    BoundaryValue::AllA5,
                ],
                _ => &[
                    BoundaryValue::Zero,
                    BoundaryValue::One,
                    BoundaryValue::Max,
                    BoundaryValue::BigEndianOne,
                ],
            };
            values.iter().map(|value| Mutation::SetField {
                field: *field,
                value: *value,
            })
        })
        .collect()
}

pub fn truncation_mutations(length: usize, fields: &[FieldSpec]) -> Vec<Mutation> {
    let mut offsets = vec![0, 1, length / 2, length.saturating_sub(1)];
    for field in fields {
        offsets.extend([
            field.offset,
            field.offset.saturating_add(1),
            field.end().saturating_sub(1),
        ]);
    }
    offsets.sort_unstable();
    offsets.dedup();
    offsets.into_iter().map(Mutation::TruncateAt).collect()
}

#[test]
fn 字段变异只修改目标范围且checksum修复有界() {
    let field = FieldSpec {
        name: "value",
        offset: 4,
        width: 4,
        encoding: FieldEncoding::U32Le,
    };
    let original = vec![0x11; 16];
    let changed = Mutation::SetField {
        field,
        value: BoundaryValue::Max,
    }
    .apply(&original);
    assert_eq!(&changed[..4], &original[..4]);
    assert_eq!(&changed[4..8], &[0xff; 4]);
    assert_eq!(&changed[8..], &original[8..]);
    let repaired = Mutation::RepairTrailingCrc.apply(&original);
    assert_eq!(
        u32::from_le_bytes(repaired[12..16].try_into().unwrap()),
        crc32fast::hash(&repaired[..12])
    );
}
