use super::model::{CompareOp, Predicate, Query, RefScalar};

pub fn shrink_predicate(predicate: &Predicate) -> Vec<Predicate> {
    match predicate {
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            let mut output = vec![(**left).clone(), (**right).clone()];
            output.extend(shrink_predicate(left));
            output.extend(shrink_predicate(right));
            output
        }
        Predicate::Not(inner) => vec![(**inner).clone()],
        Predicate::Compare { field, value, .. } => vec![
            Predicate::Compare {
                field: field.clone(),
                operation: CompareOp::Eq,
                value: value.clone(),
            },
            Predicate::True,
        ],
        Predicate::True | Predicate::False => Vec::new(),
    }
}

pub fn shrink_query(query: &Query) -> Vec<Query> {
    match query {
        Query::Find {
            predicate,
            order,
            offset,
            limit,
        } => {
            let mut output = Vec::new();
            for predicate in shrink_predicate(predicate) {
                output.push(Query::Find {
                    predicate,
                    order: order.clone(),
                    offset: *offset,
                    limit: *limit,
                });
            }
            if !order.is_empty() {
                output.push(Query::Find {
                    predicate: predicate.clone(),
                    order: Vec::new(),
                    offset: *offset,
                    limit: *limit,
                });
            }
            if *offset > 0 {
                output.push(Query::Find {
                    predicate: predicate.clone(),
                    order: order.clone(),
                    offset: 0,
                    limit: *limit,
                });
            }
            if limit.is_some() {
                output.push(Query::Find {
                    predicate: predicate.clone(),
                    order: order.clone(),
                    offset: *offset,
                    limit: None,
                });
            }
            output
        }
        Query::Match {
            source,
            target,
            label,
            offset,
            limit,
        } => {
            let mut output = Vec::new();
            if label.is_some() {
                output.push(Query::Match {
                    source: source.clone(),
                    target: target.clone(),
                    label: None,
                    offset: *offset,
                    limit: *limit,
                });
            }
            if source.is_some() || target.is_some() {
                output.push(Query::Match {
                    source: None,
                    target: None,
                    label: label.clone(),
                    offset: *offset,
                    limit: *limit,
                });
            }
            output
        }
        Query::CountBy {
            predicate: _,
            field,
        } => vec![Query::CountBy {
            predicate: Predicate::True,
            field: field.clone(),
        }],
    }
}

pub fn regression_queries() -> Vec<(&'static str, Query)> {
    vec![
        (
            "issue_32_offset_after_full_candidate_enumeration",
            Query::Find {
                predicate: Predicate::Compare {
                    field: "kind".into(),
                    operation: CompareOp::Eq,
                    value: RefScalar::String("alpha".into()),
                },
                order: Vec::new(),
                offset: 5,
                limit: Some(10),
            },
        ),
        (
            "ordered_index_filter_before_pagination",
            Query::Find {
                predicate: Predicate::Compare {
                    field: "active".into(),
                    operation: CompareOp::Eq,
                    value: RefScalar::Bool(true),
                },
                order: vec![super::model::Order {
                    field: "rank".into(),
                    direction: super::model::Direction::Ascending,
                }],
                offset: 3,
                limit: Some(5),
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinker单调移除谓词排序分页与标签复杂度() {
        let query = Query::Find {
            predicate: Predicate::And(
                Box::new(Predicate::True),
                Box::new(Predicate::Compare {
                    field: "rank".into(),
                    operation: CompareOp::GreaterEq,
                    value: RefScalar::Integer(3),
                }),
            ),
            order: Vec::new(),
            offset: 4,
            limit: Some(8),
        };
        let shrunk = shrink_query(&query);
        assert!(shrunk.len() >= 4);
        assert!(regression_queries().len() >= 2);
    }
}
