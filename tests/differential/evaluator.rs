use super::model::*;
use std::cmp::Ordering;
use std::collections::BTreeMap;

fn compare(left: &RefScalar, right: &RefScalar) -> Option<Ordering> {
    match (left, right) {
        (RefScalar::Bool(left), RefScalar::Bool(right)) => Some(left.cmp(right)),
        (RefScalar::Integer(left), RefScalar::Integer(right)) => Some(left.cmp(right)),
        (RefScalar::String(left), RefScalar::String(right)) => Some(left.cmp(right)),
        (RefScalar::Null, RefScalar::Null) => Some(Ordering::Equal),
        _ => None,
    }
}

pub fn matches(node: &RefNode, predicate: &Predicate) -> bool {
    match predicate {
        Predicate::Compare {
            field,
            operation,
            value,
        } => {
            let actual = RefScalar::from_json(node.payload.get(field));
            let ordering = compare(&actual, value);
            match operation {
                CompareOp::Eq => ordering == Some(Ordering::Equal),
                CompareOp::NotEq => ordering.is_some_and(|value| value != Ordering::Equal),
                CompareOp::Greater => ordering == Some(Ordering::Greater),
                CompareOp::GreaterEq => ordering.is_some_and(|value| value != Ordering::Less),
                CompareOp::Less => ordering == Some(Ordering::Less),
                CompareOp::LessEq => ordering.is_some_and(|value| value != Ordering::Greater),
            }
        }
        Predicate::And(left, right) => matches(node, left) && matches(node, right),
        Predicate::Or(left, right) => matches(node, left) || matches(node, right),
        Predicate::Not(inner) => !matches(node, inner),
        Predicate::True => true,
        Predicate::False => false,
    }
}

fn apply_page<T>(values: &mut Vec<T>, offset: usize, limit: Option<usize>) {
    if offset >= values.len() {
        values.clear();
    } else if offset > 0 {
        values.drain(..offset);
    }
    if let Some(limit) = limit {
        values.truncate(limit);
    }
}

pub fn evaluate(database: &RefDatabase, query: &Query) -> Vec<CanonicalRow> {
    match query {
        Query::Find {
            predicate,
            order,
            offset,
            limit,
        } => {
            let mut nodes = database
                .nodes
                .values()
                .filter(|node| matches(node, predicate))
                .collect::<Vec<_>>();
            if order.is_empty() {
                nodes.sort_by_key(|node| node.id);
            } else {
                nodes.sort_by(|left, right| {
                    for item in order {
                        let ordering = compare(
                            &RefScalar::from_json(left.payload.get(&item.field)),
                            &RefScalar::from_json(right.payload.get(&item.field)),
                        )
                        .unwrap_or(Ordering::Equal);
                        let ordering = if item.direction == Direction::Descending {
                            ordering.reverse()
                        } else {
                            ordering
                        };
                        if ordering != Ordering::Equal {
                            return ordering;
                        }
                    }
                    left.id.cmp(&right.id)
                });
            }
            apply_page(&mut nodes, *offset, *limit);
            nodes
                .into_iter()
                .map(|node| BTreeMap::from([("id".into(), RefScalar::Integer(node.id as i64))]))
                .collect()
        }
        Query::Match {
            source,
            target,
            label,
            offset,
            limit,
        } => {
            let mut pairs = database
                .edges
                .iter()
                .filter(|edge| label.as_ref().is_none_or(|value| value == &edge.label))
                .filter_map(|edge| {
                    let source_node = database.nodes.get(&edge.source)?;
                    let target_node = database.nodes.get(&edge.target)?;
                    if source
                        .as_ref()
                        .is_none_or(|value| matches(source_node, value))
                        && target
                            .as_ref()
                            .is_none_or(|value| matches(target_node, value))
                    {
                        Some((edge.source, edge.target))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            pairs.sort_unstable();
            apply_page(&mut pairs, *offset, *limit);
            pairs
                .into_iter()
                .map(|(source, target)| {
                    BTreeMap::from([
                        ("source".into(), RefScalar::Integer(source as i64)),
                        ("target".into(), RefScalar::Integer(target as i64)),
                    ])
                })
                .collect()
        }
        Query::CountBy { predicate, field } => {
            let mut groups = BTreeMap::<String, i64>::new();
            for node in database
                .nodes
                .values()
                .filter(|node| matches(node, predicate))
            {
                if let Some(value) = node.payload.get(field).and_then(|value| value.as_str()) {
                    *groups.entry(value.to_owned()).or_default() += 1;
                }
            }
            groups
                .into_iter()
                .map(|(group, count)| {
                    BTreeMap::from([
                        ("group".into(), RefScalar::String(group)),
                        ("count".into(), RefScalar::Integer(count)),
                    ])
                })
                .collect()
        }
    }
}
