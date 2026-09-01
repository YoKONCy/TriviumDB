use super::mutation::{BoundaryValue, Mutation};
use super::spec::{FieldSpec, FileRole};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FormatReplay {
    pub seed: u64,
    pub role: String,
    pub mutation: String,
    pub original_size: usize,
    pub mutated_size: usize,
}

pub fn replay(seed: u64, role: FileRole, mutation: &Mutation, input: &[u8]) -> FormatReplay {
    let output = mutation.apply(input);
    FormatReplay {
        seed,
        role: format!("{role:?}"),
        mutation: format!("{mutation:?}"),
        original_size: input.len(),
        mutated_size: output.len(),
    }
}

pub fn shrink_mutation(mutation: &Mutation) -> Vec<Mutation> {
    match mutation {
        Mutation::SetField { field, value } => {
            if *value == BoundaryValue::Zero {
                Vec::new()
            } else {
                vec![Mutation::SetField {
                    field: *field,
                    value: BoundaryValue::Zero,
                }]
            }
        }
        Mutation::TruncateAt(offset) => [0, 1, offset / 2]
            .into_iter()
            .filter(|candidate| candidate < offset)
            .map(Mutation::TruncateAt)
            .collect(),
        Mutation::FlipBit { offset, bit } => {
            let mut output = Vec::new();
            if *offset > 0 {
                output.push(Mutation::FlipBit {
                    offset: offset / 2,
                    bit: *bit,
                });
            }
            if *bit > 0 {
                output.push(Mutation::FlipBit {
                    offset: *offset,
                    bit: 0,
                });
            }
            output
        }
        Mutation::Append(bytes) if bytes.len() > 1 => {
            vec![Mutation::Append(bytes[..bytes.len() / 2].to_vec())]
        }
        Mutation::Append(_) | Mutation::RepairTrailingCrc | Mutation::RepairFlushMarkerCrc => {
            Vec::new()
        }
    }
}

pub fn greedily_shrink(initial: Mutation, still_fails: impl Fn(&Mutation) -> bool) -> Mutation {
    let mut current = initial;
    loop {
        let Some(next) = shrink_mutation(&current)
            .into_iter()
            .find(|candidate| candidate != &current && still_fails(candidate))
        else {
            return current;
        };
        current = next;
    }
}

#[test]
fn 格式失败记录可序列化且mutation可确定性缩减() {
    let field = FieldSpec {
        name: "length",
        offset: 4,
        width: 8,
        encoding: super::spec::FieldEncoding::U64Le,
    };
    let mutation = Mutation::SetField {
        field,
        value: BoundaryValue::Max,
    };
    let record = replay(42, FileRole::Tdb, &mutation, &[0; 32]);
    let json = serde_json::to_string(&record).unwrap();
    assert!(json.contains("Tdb"));
    let shrunk = greedily_shrink(mutation, |_| true);
    assert_eq!(
        shrunk,
        Mutation::SetField {
            field,
            value: BoundaryValue::Zero
        }
    );
}
