#![allow(non_snake_case)]
//! 阶段 A：基于独立 Reference Model 的确定性状态机测试。
//!
//! 测试只通过公共 Database API 观察 TriviumDB，不复用生产 MemTable、索引、事务或查询逻辑。
//! 每个操作后逐项比较节点、向量、Payload、出入边及属性查询；固定 seed 可直接重放失败序列。

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use triviumdb::database::{Config, Database, StorageMode};

const DIM: usize = 3;
const STEPS: usize = 120;
const LABELS: [&str; 3] = ["related", "parent", "tagged"];
const KINDS: [&str; 3] = ["alpha", "beta", "gamma"];

#[derive(Debug, Clone, PartialEq)]
struct ReferenceNode {
    vector: Vec<f32>,
    payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReferenceEdge {
    source: u64,
    target: u64,
    label: String,
    weight_bits: u32,
}

#[derive(Debug, Clone, Default)]
struct ReferenceDb {
    nodes: BTreeMap<u64, ReferenceNode>,
    edges: BTreeSet<ReferenceEdge>,
    indexes: BTreeSet<String>,
    next_id: u64,
}

impl ReferenceDb {
    fn new() -> Self {
        Self {
            next_id: 1,
            ..Self::default()
        }
    }

    fn insert_auto(&mut self, vector: Vec<f32>, payload: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.insert(id, ReferenceNode { vector, payload });
        id
    }

    fn insert_with_id(&mut self, id: u64, vector: Vec<f32>, payload: Value) -> bool {
        if id == 0 || self.nodes.contains_key(&id) {
            return false;
        }
        self.nodes.insert(id, ReferenceNode { vector, payload });
        self.next_id = self.next_id.max(id.saturating_add(1));
        true
    }

    fn delete(&mut self, id: u64) -> bool {
        if self.nodes.remove(&id).is_none() {
            return false;
        }
        self.edges
            .retain(|edge| edge.source != id && edge.target != id);
        true
    }

    fn link(&mut self, source: u64, target: u64, label: &str, weight: f32) -> bool {
        if !self.nodes.contains_key(&source) || !self.nodes.contains_key(&target) {
            return false;
        }
        self.edges.retain(|edge| {
            !(edge.source == source && edge.target == target && edge.label == label)
        });
        self.edges.insert(ReferenceEdge {
            source,
            target,
            label: label.to_owned(),
            weight_bits: weight.to_bits(),
        });
        true
    }

    fn unlink_label(&mut self, source: u64, target: u64, label: &str) -> bool {
        if !self.nodes.contains_key(&source) {
            return false;
        }
        self.edges.remove(&ReferenceEdge {
            source,
            target,
            label: label.to_owned(),
            weight_bits: self
                .edges
                .iter()
                .find(|edge| edge.source == source && edge.target == target && edge.label == label)
                .map_or(0, |edge| edge.weight_bits),
        });
        true
    }

    fn ids_for_kind(&self, kind: &str) -> Vec<u64> {
        self.nodes
            .iter()
            .filter_map(|(&id, node)| (node.payload["kind"] == kind).then_some(id))
            .collect()
    }
}

#[derive(Debug, Clone)]
enum Operation {
    InsertAuto {
        vector: Vec<f32>,
        payload: Value,
    },
    InsertWithId {
        id: u64,
        vector: Vec<f32>,
        payload: Value,
    },
    UpdatePayload {
        id: u64,
        payload: Value,
    },
    UpdateVector {
        id: u64,
        vector: Vec<f32>,
    },
    Delete {
        id: u64,
    },
    Link {
        source: u64,
        target: u64,
        label: String,
        weight: f32,
    },
    UnlinkLabel {
        source: u64,
        target: u64,
        label: String,
    },
    CreateIndex {
        field: String,
    },
    DropIndex {
        field: String,
    },
    Flush,
    Compact,
    Reopen,
    TransactionCommit,
    TransactionRollback,
    TransactionRejected {
        duplicate_id: u64,
    },
}

fn vector_for(value: u64) -> Vec<f32> {
    vec![value as f32, (value % 7) as f32, 1.0]
}

fn payload_for(value: u64) -> Value {
    json!({
        "kind": KINDS[value as usize % KINDS.len()],
        "rank": value as i64,
        "active": value.is_multiple_of(2),
    })
}

fn choose_id(rng: &mut StdRng, model: &ReferenceDb, allow_missing: bool) -> u64 {
    if !model.nodes.is_empty() && (!allow_missing || rng.gen_bool(0.75)) {
        let index = rng.gen_range(0..model.nodes.len());
        *model.nodes.keys().nth(index).expect("非空模型必须有节点")
    } else {
        model.next_id.saturating_add(rng.gen_range(10..30))
    }
}

fn generate_operation(rng: &mut StdRng, model: &ReferenceDb) -> Operation {
    match rng.gen_range(0..100) {
        0..=24 => {
            let value = rng.gen_range(1..10_000);
            Operation::InsertAuto {
                vector: vector_for(value),
                payload: payload_for(value),
            }
        }
        25..=31 => {
            let duplicate = !model.nodes.is_empty() && rng.gen_bool(0.35);
            let id = if duplicate {
                choose_id(rng, model, false)
            } else {
                model.next_id.saturating_add(rng.gen_range(1..8))
            };
            Operation::InsertWithId {
                id,
                vector: vector_for(id),
                payload: payload_for(id),
            }
        }
        32..=41 => Operation::UpdatePayload {
            id: choose_id(rng, model, true),
            payload: payload_for(rng.gen_range(1..10_000)),
        },
        42..=48 => {
            let value = rng.gen_range(1..10_000);
            Operation::UpdateVector {
                id: choose_id(rng, model, true),
                vector: vector_for(value),
            }
        }
        49..=55 => Operation::Delete {
            id: choose_id(rng, model, true),
        },
        56..=66 => Operation::Link {
            source: choose_id(rng, model, true),
            target: choose_id(rng, model, true),
            label: LABELS[rng.gen_range(0..LABELS.len())].to_owned(),
            weight: rng.gen_range(1..=10) as f32 / 10.0,
        },
        67..=72 => Operation::UnlinkLabel {
            source: choose_id(rng, model, true),
            target: choose_id(rng, model, true),
            label: LABELS[rng.gen_range(0..LABELS.len())].to_owned(),
        },
        73..=76 => Operation::CreateIndex {
            field: "kind".to_owned(),
        },
        77..=79 => Operation::DropIndex {
            field: "kind".to_owned(),
        },
        80..=84 => Operation::Flush,
        85..=88 => Operation::Compact,
        89..=92 => Operation::Reopen,
        93..=95 => Operation::TransactionCommit,
        96..=97 => Operation::TransactionRollback,
        _ if !model.nodes.is_empty() => Operation::TransactionRejected {
            duplicate_id: choose_id(rng, model, false),
        },
        _ => Operation::TransactionRollback,
    }
}

fn apply_operation(operation: &Operation, db: &mut Database<f32>, model: &mut ReferenceDb) {
    match operation {
        Operation::InsertAuto { vector, payload } => {
            let expected = model.insert_auto(vector.clone(), payload.clone());
            assert_eq!(db.insert(vector, payload.clone()).unwrap(), expected);
        }
        Operation::InsertWithId {
            id,
            vector,
            payload,
        } => {
            let expected = model.insert_with_id(*id, vector.clone(), payload.clone());
            let actual = db.insert_with_id(*id, vector, payload.clone());
            assert_eq!(actual.is_ok(), expected);
        }
        Operation::UpdatePayload { id, payload } => {
            let expected = if let Some(node) = model.nodes.get_mut(id) {
                node.payload = payload.clone();
                true
            } else {
                false
            };
            assert_eq!(db.update_payload(*id, payload.clone()).is_ok(), expected);
        }
        Operation::UpdateVector { id, vector } => {
            let expected = if let Some(node) = model.nodes.get_mut(id) {
                node.vector = vector.clone();
                true
            } else {
                false
            };
            assert_eq!(db.update_vector(*id, vector).is_ok(), expected);
        }
        Operation::Delete { id } => {
            let expected = model.delete(*id);
            assert_eq!(db.delete(*id).is_ok(), expected);
        }
        Operation::Link {
            source,
            target,
            label,
            weight,
        } => {
            let expected = model.link(*source, *target, label, *weight);
            assert_eq!(db.link(*source, *target, label, *weight).is_ok(), expected);
        }
        Operation::UnlinkLabel {
            source,
            target,
            label,
        } => {
            let expected = model.unlink_label(*source, *target, label);
            assert_eq!(db.unlink_label(*source, *target, label).is_ok(), expected);
        }
        Operation::CreateIndex { field } => {
            db.create_index(field).unwrap();
            model.indexes.insert(field.clone());
        }
        Operation::DropIndex { field } => {
            db.drop_index(field).unwrap();
            model.indexes.remove(field);
        }
        Operation::Flush => db.flush().unwrap(),
        Operation::Compact => db.compact().unwrap(),
        Operation::Reopen => {}
        Operation::TransactionCommit => {
            let first_payload = payload_for(model.next_id.saturating_add(100));
            let second_payload = payload_for(model.next_id.saturating_add(101));
            let first_id = model.next_id;
            let second_id = first_id.saturating_add(1);
            let mut tx = db.begin_tx();
            tx.insert(&vector_for(101), first_payload.clone());
            tx.insert(&vector_for(102), second_payload.clone());
            tx.link(first_id, second_id, "tx", 0.75);
            let ids = tx.commit().unwrap();
            let expected = vec![
                model.insert_auto(vector_for(101), first_payload),
                model.insert_auto(vector_for(102), second_payload),
            ];
            assert!(model.link(first_id, second_id, "tx", 0.75));
            assert_eq!(ids, expected);
        }
        Operation::TransactionRollback => {
            let before = model.nodes.len();
            let mut tx = db.begin_tx();
            tx.insert(&vector_for(999), payload_for(999));
            tx.rollback();
            assert_eq!(model.nodes.len(), before);
        }
        Operation::TransactionRejected { duplicate_id } => {
            let before_ids = db.all_node_ids();
            let before_payload = db.get_payload(*duplicate_id);
            let mut tx = db.begin_tx();
            tx.update_payload(*duplicate_id, payload_for(7_777));
            tx.insert_with_id(*duplicate_id, &vector_for(7_777), payload_for(7_777));
            assert!(tx.commit().is_err(), "非法事务必须整体拒绝");
            assert_eq!(db.all_node_ids(), before_ids, "失败事务不得改变节点集合");
            assert_eq!(
                db.get_payload(*duplicate_id),
                before_payload,
                "失败事务不得提交前序更新"
            );
        }
    }
}

fn canonical_edges(db: &Database<f32>) -> BTreeSet<ReferenceEdge> {
    db.all_node_ids()
        .into_iter()
        .flat_map(|source| {
            db.get_edges(source)
                .into_iter()
                .map(move |edge| ReferenceEdge {
                    source,
                    target: edge.target_id,
                    label: edge.label,
                    weight_bits: edge.weight.to_bits(),
                })
        })
        .collect()
}

fn assert_matches_model(db: &Database<f32>, model: &ReferenceDb, context: &str) {
    let actual_ids = db.all_node_ids().into_iter().collect::<BTreeSet<_>>();
    let expected_ids = model.nodes.keys().copied().collect::<BTreeSet<_>>();
    assert_eq!(actual_ids, expected_ids, "{context}: NodeId 集合不一致");
    assert_eq!(
        db.node_count(),
        model.nodes.len(),
        "{context}: node_count 不一致"
    );

    for (&id, expected) in &model.nodes {
        let actual = db
            .get(id)
            .unwrap_or_else(|| panic!("{context}: 缺少节点 {id}"));
        assert_eq!(
            actual.vector, expected.vector,
            "{context}: 节点 {id} 向量不一致"
        );
        assert_eq!(
            actual.payload, expected.payload,
            "{context}: 节点 {id} Payload 不一致"
        );
    }
    assert_eq!(
        canonical_edges(db),
        model.edges,
        "{context}: 出边集合不一致"
    );

    for &target in model.nodes.keys() {
        let actual = db
            .get_incoming_edges(target, None)
            .into_iter()
            .map(|edge| {
                (
                    edge.source_id,
                    edge.target_id,
                    edge.label,
                    edge.weight.to_bits(),
                )
            })
            .collect::<BTreeSet<_>>();
        let expected = model
            .edges
            .iter()
            .filter(|edge| edge.target == target)
            .map(|edge| {
                (
                    edge.source,
                    edge.target,
                    edge.label.clone(),
                    edge.weight_bits,
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected, "{context}: 节点 {target} 入边不一致");
    }

    for kind in KINDS {
        let query = format!(r#"FIND {{kind: "{kind}"}} RETURN *"#);
        let actual = db
            .tql(&query)
            .unwrap()
            .into_iter()
            .map(|row| row["_"].id)
            .collect::<BTreeSet<_>>();
        let expected = model
            .ids_for_kind(kind)
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected, "{context}: kind={kind} 查询不一致");
    }
}

fn cleanup(path: &str) {
    for suffix in [
        "",
        ".wal",
        ".vec",
        ".lock",
        ".flush_ok",
        ".propidx",
        ".graph",
        ".textidx",
        ".textidx.meta",
        ".quiver",
    ] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

fn run_state_machine(seed: u64, mode: StorageMode) {
    let path = std::env::temp_dir()
        .join("triviumdb_test")
        .join(format!("model_state_{seed}_{mode:?}.tdb"))
        .to_string_lossy()
        .to_string();
    std::fs::create_dir_all(std::env::temp_dir().join("triviumdb_test")).unwrap();
    cleanup(&path);
    let config = Config {
        dim: DIM,
        storage_mode: mode,
        auto_build_quiver: false,
        ..Default::default()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    let mut model = ReferenceDb::new();
    let mut rng = StdRng::seed_from_u64(seed);
    let mut history = Vec::new();

    for step in 0..STEPS {
        let operation = generate_operation(&mut rng, &model);
        history.push(operation.clone());
        apply_operation(&operation, &mut db, &mut model);
        if matches!(operation, Operation::Reopen) {
            drop(db);
            db = Database::<f32>::open_with_config(&path, config).unwrap();
        }
        let context = format!(
            "seed={seed:#x}, mode={mode:?}, step={step}, operation={operation:?}, history={history:?}"
        );
        assert_matches_model(&db, &model, &context);
    }

    db.flush().unwrap();
    drop(db);
    let reopened = Database::<f32>::open_with_config(&path, config).unwrap();
    assert_matches_model(&reopened, &model, "最终 flush/reopen");
    drop(reopened);
    cleanup(&path);
}

#[test]
fn 阶段A_固定种子状态机覆盖_Mmap与Rom() {
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        for seed in [0xA11C_E001, 0xA11C_E002, 0xA11C_E003] {
            run_state_machine(seed, mode);
        }
    }
}
