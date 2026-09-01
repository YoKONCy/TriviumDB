use super::evaluator::evaluate;
use super::generator::query_tql;
use super::model::{Query, RefDatabase};
use super::shrinker::shrink_query;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ReplayRecord {
    pub seed: u64,
    pub configuration: String,
    pub tql: String,
    pub query_debug: String,
    pub node_count: usize,
    pub edge_count: usize,
}

pub fn replay_record(
    seed: u64,
    configuration: impl Into<String>,
    database: &RefDatabase,
    query: &Query,
) -> ReplayRecord {
    ReplayRecord {
        seed,
        configuration: configuration.into(),
        tql: query_tql(query),
        query_debug: format!("{query:?}"),
        node_count: database.nodes.len(),
        edge_count: database.edges.len(),
    }
}

pub fn greedily_shrink_query(
    database: &RefDatabase,
    initial: Query,
    still_fails: impl Fn(&RefDatabase, &Query) -> bool,
) -> Query {
    let mut current = initial;
    loop {
        let Some(next) = shrink_query(&current)
            .into_iter()
            .filter(|candidate| candidate != &current)
            .find(|candidate| still_fails(database, candidate))
        else {
            return current;
        };
        current = next;
    }
}

pub fn shrink_database(
    initial: &RefDatabase,
    query: &Query,
    still_fails: impl Fn(&RefDatabase, &Query) -> bool,
) -> RefDatabase {
    let mut current = initial.clone();
    let ids = current.nodes.keys().copied().collect::<Vec<_>>();
    for id in ids {
        let mut candidate = current.clone();
        candidate.nodes.remove(&id);
        candidate
            .edges
            .retain(|edge| edge.source != id && edge.target != id);
        if still_fails(&candidate, query) {
            current = candidate;
        }
    }
    current
}

pub fn reference_signature(database: &RefDatabase, query: &Query) -> String {
    format!("{:?}", evaluate(database, query))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CompareOp, Direction, Order, Predicate, RefScalar};

    #[test]
    fn 贪心shrinker与数据库缩减均由失败判定器驱动() {
        let database = RefDatabase::fixture(12);
        let query = Query::Find {
            predicate: Predicate::Compare {
                field: "rank".into(),
                operation: CompareOp::GreaterEq,
                value: RefScalar::Integer(3),
            },
            order: vec![Order {
                field: "rank".into(),
                direction: Direction::Ascending,
            }],
            offset: 2,
            limit: Some(5),
        };
        let shrunk = greedily_shrink_query(&database, query.clone(), |_, candidate| {
            matches!(candidate, Query::Find { offset: 0, .. })
        });
        assert!(matches!(shrunk, Query::Find { offset: 0, .. }));
        let smaller = shrink_database(&database, &query, |candidate, _| candidate.nodes.len() >= 3);
        assert_eq!(smaller.nodes.len(), 3);
        assert!(
            smaller
                .edges
                .iter()
                .all(|edge| smaller.nodes.contains_key(&edge.source)
                    && smaller.nodes.contains_key(&edge.target))
        );
    }

    #[test]
    fn replay记录包含固定seed配置TQL和fixture规模() {
        let database = RefDatabase::fixture(4);
        let query = Query::Find {
            predicate: Predicate::True,
            order: Vec::new(),
            offset: 0,
            limit: None,
        };
        let record = replay_record(7, "Mmap/no-index", &database, &query);
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("Mmap/no-index"));
        assert!(json.contains("\"seed\":7"));
        assert!(!reference_signature(&database, &query).is_empty());
    }
}
