use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq)]
pub struct RefNode {
    pub id: u64,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RefEdge {
    pub source: u64,
    pub target: u64,
    pub label: String,
}

#[derive(Debug, Clone, Default)]
pub struct RefDatabase {
    pub nodes: BTreeMap<u64, RefNode>,
    pub edges: BTreeSet<RefEdge>,
}

impl RefDatabase {
    pub fn fixture(size: usize) -> Self {
        let mut database = Self::default();
        for index in 0..size {
            let id = index as u64 + 1;
            let kind = ["alpha", "beta", "gamma"][index % 3];
            let group = ["north", "south"][index % 2];
            database.nodes.insert(
                id,
                RefNode {
                    id,
                    payload: json!({
                        "kind": kind,
                        "rank": index as i64,
                        "active": index % 2 == 0,
                        "group": group,
                    }),
                },
            );
        }
        for source in 1..size as u64 {
            database.edges.insert(RefEdge {
                source,
                target: source + 1,
                label: if source % 2 == 0 { "next" } else { "related" }.into(),
            });
        }
        database
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefScalar {
    Null,
    Bool(bool),
    Integer(i64),
    String(String),
}

impl RefScalar {
    pub fn from_json(value: Option<&Value>) -> Self {
        match value {
            Some(Value::Bool(value)) => Self::Bool(*value),
            Some(Value::Number(value)) => value.as_i64().map_or(Self::Null, Self::Integer),
            Some(Value::String(value)) => Self::String(value.clone()),
            _ => Self::Null,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    NotEq,
    Greater,
    GreaterEq,
    Less,
    LessEq,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    Compare {
        field: String,
        operation: CompareOp,
        value: RefScalar,
    },
    And(Box<Predicate>, Box<Predicate>),
    Or(Box<Predicate>, Box<Predicate>),
    Not(Box<Predicate>),
    True,
    False,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Order {
    pub field: String,
    pub direction: Direction,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum Query {
    Find {
        predicate: Predicate,
        order: Vec<Order>,
        offset: usize,
        limit: Option<usize>,
    },
    Match {
        source: Option<Predicate>,
        target: Option<Predicate>,
        label: Option<String>,
        offset: usize,
        limit: Option<usize>,
    },
    CountBy {
        predicate: Predicate,
        field: String,
    },
}

pub type CanonicalRow = BTreeMap<String, RefScalar>;
