use super::expression::RefValue;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use triviumdb::query::tql_executor::TqlValue;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalValue {
    Null,
    Bool(bool),
    Integer(i64),
    Float(u64),
    String(String),
    Node(u64),
    Path(Vec<u64>),
    List(Vec<CanonicalValue>),
}

impl Ord for CanonicalValue {
    fn cmp(&self, other: &Self) -> Ordering {
        format!("{self:?}").cmp(&format!("{other:?}"))
    }
}

impl PartialOrd for CanonicalValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub type CanonicalValueRow = BTreeMap<String, CanonicalValue>;

fn json_value(value: &serde_json::Value) -> CanonicalValue {
    match value {
        serde_json::Value::Null => CanonicalValue::Null,
        serde_json::Value::Bool(value) => CanonicalValue::Bool(*value),
        serde_json::Value::Number(value) => value.as_i64().map_or_else(
            || CanonicalValue::Float(value.as_f64().unwrap_or(f64::NAN).to_bits()),
            CanonicalValue::Integer,
        ),
        serde_json::Value::String(value) => CanonicalValue::String(value.clone()),
        serde_json::Value::Array(values) => {
            CanonicalValue::List(values.iter().map(json_value).collect())
        }
        serde_json::Value::Object(value) => {
            CanonicalValue::String(serde_json::to_string(value).expect("JSON object 必须可序列化"))
        }
    }
}

pub fn from_tql_value(value: &TqlValue<f32>) -> CanonicalValue {
    match value {
        TqlValue::Node(node) => CanonicalValue::Node(node.id),
        TqlValue::Int(value) => CanonicalValue::Integer(*value),
        TqlValue::Float(value) => CanonicalValue::Float(value.to_bits()),
        TqlValue::String(value) => CanonicalValue::String(value.clone()),
        TqlValue::Bool(value) => CanonicalValue::Bool(*value),
        TqlValue::Path(value) => CanonicalValue::Path(value.clone()),
        TqlValue::List(value) => CanonicalValue::List(value.iter().map(json_value).collect()),
        TqlValue::Null => CanonicalValue::Null,
    }
}

pub fn from_reference(value: &RefValue) -> CanonicalValue {
    match value {
        RefValue::Missing | RefValue::Null => CanonicalValue::Null,
        RefValue::Bool(value) => CanonicalValue::Bool(*value),
        RefValue::Integer(value) => CanonicalValue::Integer(*value),
        RefValue::Float(value) => CanonicalValue::Float(value.to_bits()),
        RefValue::String(value) => CanonicalValue::String(value.clone()),
        RefValue::List(value) => CanonicalValue::List(value.iter().map(from_reference).collect()),
        RefValue::Path(value) => CanonicalValue::Path(value.clone()),
    }
}

pub fn canonicalize_rows(
    rows: Vec<std::collections::HashMap<String, TqlValue<f32>>>,
    ordered: bool,
) -> Vec<CanonicalValueRow> {
    let mut rows = rows
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|(name, value)| (name, from_tql_value(&value)))
                .collect()
        })
        .collect::<Vec<_>>();
    if !ordered {
        rows.sort();
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical覆盖全部一等TQL值且无序行按多重集合比较() {
        let values = [
            TqlValue::Int(7),
            TqlValue::Float(1.5),
            TqlValue::String("x".into()),
            TqlValue::Bool(true),
            TqlValue::Path(vec![1, 2]),
            TqlValue::List(vec![serde_json::json!(1), serde_json::json!(null)]),
            TqlValue::Null,
        ];
        let converted = values.iter().map(from_tql_value).collect::<Vec<_>>();
        assert_eq!(converted.len(), 7);
        assert_eq!(from_reference(&RefValue::Missing), CanonicalValue::Null);
    }
}
