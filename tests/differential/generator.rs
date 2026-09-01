use super::model::*;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

fn scalar_literal(value: &RefScalar) -> String {
    match value {
        RefScalar::Null => "null".into(),
        RefScalar::Bool(value) => value.to_string(),
        RefScalar::Integer(value) => value.to_string(),
        RefScalar::String(value) => format!("\"{}\"", value.replace('"', "\\\"")),
    }
}

pub fn predicate_tql(predicate: &Predicate, variable: &str) -> String {
    match predicate {
        Predicate::Compare {
            field,
            operation,
            value,
        } => {
            let operation = match operation {
                CompareOp::Eq => "==",
                CompareOp::NotEq => "!=",
                CompareOp::Greater => ">",
                CompareOp::GreaterEq => ">=",
                CompareOp::Less => "<",
                CompareOp::LessEq => "<=",
            };
            format!("{variable}.{field} {operation} {}", scalar_literal(value))
        }
        Predicate::And(left, right) => format!(
            "({}) AND ({})",
            predicate_tql(left, variable),
            predicate_tql(right, variable)
        ),
        Predicate::Or(left, right) => format!(
            "({}) OR ({})",
            predicate_tql(left, variable),
            predicate_tql(right, variable)
        ),
        Predicate::Not(inner) => format!("NOT ({})", predicate_tql(inner, variable)),
        Predicate::True => format!("{variable}.rank >= 0"),
        Predicate::False => format!("{variable}.rank < 0"),
    }
}

pub fn query_tql(query: &Query) -> String {
    match query {
        Query::Find {
            predicate,
            order,
            offset,
            limit,
        } => {
            let mut query = format!(
                "FIND {{rank: {{$gte: 0}}}} WHERE {} RETURN _",
                predicate_tql(predicate, "_")
            );
            if !order.is_empty() {
                query.push_str(" ORDER BY ");
                query.push_str(
                    &order
                        .iter()
                        .map(|item| {
                            format!(
                                "_.{} {}",
                                item.field,
                                if item.direction == Direction::Descending {
                                    "DESC"
                                } else {
                                    "ASC"
                                }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            if let Some(limit) = limit {
                query.push_str(&format!(" LIMIT {limit}"));
            }
            if *offset > 0 {
                query.push_str(&format!(" OFFSET {offset}"));
            }
            query
        }
        Query::Match {
            source,
            target,
            label,
            offset,
            limit,
        } => {
            let relationship = label
                .as_ref()
                .map_or_else(|| "[]".into(), |label| format!("[:{label}]"));
            let mut query = format!("MATCH (a)-{relationship}->(b)");
            let mut predicates = Vec::new();
            if let Some(source) = source {
                predicates.push(predicate_tql(source, "a"));
            }
            if let Some(target) = target {
                predicates.push(predicate_tql(target, "b"));
            }
            if !predicates.is_empty() {
                query.push_str(" WHERE ");
                query.push_str(&predicates.join(" AND "));
            }
            query.push_str(" RETURN a, b");
            if let Some(limit) = limit {
                query.push_str(&format!(" LIMIT {limit}"));
            }
            if *offset > 0 {
                query.push_str(&format!(" OFFSET {offset}"));
            }
            query
        }
        Query::CountBy { predicate, field } => format!(
            "FIND {{rank: {{$gte: 0}}}} WHERE {} RETURN _.{field} AS bucket, count(_) AS total ORDER BY _.{field} ASC",
            predicate_tql(predicate, "_")
        ),
    }
}

pub fn queries(seed: u64, count: usize) -> Vec<Query> {
    let mut rng = StdRng::seed_from_u64(seed);
    let fields = ["kind", "rank", "active", "group"];
    let mut output = Vec::new();
    for _ in 0..count {
        let field = fields[rng.gen_range(0..fields.len())];
        let value = match field {
            "kind" => RefScalar::String(["alpha", "beta", "gamma"][rng.gen_range(0..3)].into()),
            "group" => RefScalar::String(["north", "south"][rng.gen_range(0..2)].into()),
            "active" => RefScalar::Bool(rng.gen_bool(0.5)),
            _ => RefScalar::Integer(rng.gen_range(0..31)),
        };
        let operation = if field == "rank" {
            [
                CompareOp::Eq,
                CompareOp::NotEq,
                CompareOp::Greater,
                CompareOp::GreaterEq,
                CompareOp::Less,
                CompareOp::LessEq,
            ][rng.gen_range(0..6)]
        } else {
            CompareOp::Eq
        };
        let predicate = Predicate::Compare {
            field: field.into(),
            operation,
            value,
        };
        output.push(Query::Find {
            predicate: predicate.clone(),
            order: Vec::new(),
            offset: 0,
            limit: None,
        });
        output.push(Query::Match {
            source: Some(predicate),
            target: None,
            label: rng
                .gen_bool(0.7)
                .then(|| ["next", "related"][rng.gen_range(0..2)].into()),
            offset: 0,
            limit: None,
        });
    }
    output
}
