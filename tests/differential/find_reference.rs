use super::canonical::{CanonicalValue, canonicalize_rows};
use super::model::RefDatabase;
use std::collections::BTreeMap;
use triviumdb::database::Database;

fn expected_groups(reference: &RefDatabase) -> Vec<BTreeMap<String, CanonicalValue>> {
    let mut groups = BTreeMap::<String, Vec<i64>>::new();
    for node in reference.nodes.values() {
        let group = node.payload["group"].as_str().unwrap().to_owned();
        let rank = node.payload["rank"].as_i64().unwrap();
        groups.entry(group).or_default().push(rank);
    }
    groups
        .into_iter()
        .map(|(group, ranks)| {
            let sum = ranks.iter().sum::<i64>();
            let count = ranks.len() as i64;
            BTreeMap::from([
                (
                    "avg_rank".into(),
                    CanonicalValue::Float(((sum as f64) / count as f64).to_bits()),
                ),
                ("bucket".into(), CanonicalValue::String(group)),
                (
                    "max_rank".into(),
                    CanonicalValue::Float((*ranks.iter().max().unwrap() as f64).to_bits()),
                ),
                (
                    "min_rank".into(),
                    CanonicalValue::Float((*ranks.iter().min().unwrap() as f64).to_bits()),
                ),
                (
                    "sum_rank".into(),
                    CanonicalValue::Float((sum as f64).to_bits()),
                ),
                ("total".into(), CanonicalValue::Integer(count)),
            ])
        })
        .collect()
}

pub fn assert_aggregate_differential(database: &Database<f32>, reference: &RefDatabase) {
    let query = "FIND {rank: {$gte: 0}} RETURN _.group AS bucket, count(_) AS total, sum(_.rank) AS sum_rank, avg(_.rank) AS avg_rank, min(_.rank) AS min_rank, max(_.rank) AS max_rank ORDER BY _.group ASC";
    let actual = canonicalize_rows(database.tql_values(query).unwrap(), true)
        .into_iter()
        .map(|mut row| {
            row.remove("collect_rank");
            row
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, expected_groups(reference), "聚合差分: {query}");

    let empty = canonicalize_rows(
        database
            .tql_values("FIND {rank: {$gte: 0}} WHERE _.rank < 0 RETURN count(_) AS total, avg(_.rank) AS avg_rank")
            .unwrap(),
        true,
    );
    assert_eq!(
        empty,
        vec![BTreeMap::from([
            ("avg_rank".into(), CanonicalValue::Null),
            ("total".into(), CanonicalValue::Integer(0)),
        ])]
    );
}

pub fn assert_projection_distinct_differential(database: &Database<f32>, reference: &RefDatabase) {
    let rows = canonicalize_rows(
        database
            .tql_values("FIND {rank: {$gte: 0}} RETURN DISTINCT _.group AS bucket")
            .unwrap(),
        false,
    );
    let expected = reference
        .nodes
        .values()
        .map(|node| node.payload["group"].as_str().unwrap().to_owned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|group| BTreeMap::from([("bucket".into(), CanonicalValue::String(group))]))
        .collect::<Vec<_>>();
    assert_eq!(rows, expected);

    let query = "FIND {rank: {$gte: 0}} WHERE _.active == true RETURN _, _.rank AS rank ORDER BY _.group ASC, _.rank DESC LIMIT 7 OFFSET 2";
    let actual = canonicalize_rows(database.tql_values(query).unwrap(), true);
    let mut nodes = reference
        .nodes
        .values()
        .filter(|node| node.payload["active"].as_bool() == Some(true))
        .collect::<Vec<_>>();
    nodes.sort_by(|left, right| {
        left.payload["group"]
            .as_str()
            .cmp(&right.payload["group"].as_str())
            .then_with(|| {
                right.payload["rank"]
                    .as_i64()
                    .cmp(&left.payload["rank"].as_i64())
            })
    });
    let expected = nodes
        .into_iter()
        .skip(2)
        .take(7)
        .map(|node| {
            BTreeMap::from([
                ("_".into(), CanonicalValue::Node(node.id)),
                (
                    "rank".into(),
                    CanonicalValue::Integer(node.payload["rank"].as_i64().unwrap()),
                ),
            ])
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, expected, "投影/多列排序/分页差分: {query}");
}
