use super::model::{RefDatabase, RefEdge};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraverseDirection {
    Forward,
    Backward,
    Both,
}

fn adjacent(
    database: &RefDatabase,
    node: u64,
    label: Option<&str>,
    direction: TraverseDirection,
) -> Vec<u64> {
    let mut output = BTreeSet::new();
    for edge in &database.edges {
        if label.is_some_and(|value| value != edge.label) {
            continue;
        }
        if matches!(
            direction,
            TraverseDirection::Forward | TraverseDirection::Both
        ) && edge.source == node
        {
            output.insert(edge.target);
        }
        if matches!(
            direction,
            TraverseDirection::Backward | TraverseDirection::Both
        ) && edge.target == node
        {
            output.insert(edge.source);
        }
    }
    output.into_iter().collect()
}

pub fn reachable(
    database: &RefDatabase,
    source: u64,
    min_depth: usize,
    max_depth: usize,
    label: Option<&str>,
    direction: TraverseDirection,
) -> BTreeMap<u64, usize> {
    if !database.nodes.contains_key(&source) || min_depth > max_depth {
        return BTreeMap::new();
    }
    let mut depths = BTreeMap::new();
    let mut queue = VecDeque::from([(source, 0usize)]);
    let mut visited = BTreeSet::from([source]);
    while let Some((node, depth)) = queue.pop_front() {
        if depth >= min_depth {
            depths.entry(node).or_insert(depth);
        }
        if depth == max_depth {
            continue;
        }
        for target in adjacent(database, node, label, direction) {
            if visited.insert(target) {
                queue.push_back((target, depth + 1));
            }
        }
    }
    depths
}

pub fn shortest_path(
    database: &RefDatabase,
    source: u64,
    target: u64,
    max_depth: usize,
    label: Option<&str>,
    direction: TraverseDirection,
) -> Option<Vec<u64>> {
    if source == target && database.nodes.contains_key(&source) {
        return Some(vec![source]);
    }
    let mut queue = VecDeque::from([source]);
    let mut predecessor = BTreeMap::<u64, u64>::new();
    let mut depth = BTreeMap::from([(source, 0usize)]);
    while let Some(node) = queue.pop_front() {
        let current_depth = depth[&node];
        if current_depth >= max_depth {
            continue;
        }
        for next in adjacent(database, node, label, direction) {
            if depth.contains_key(&next) {
                continue;
            }
            predecessor.insert(next, node);
            depth.insert(next, current_depth + 1);
            if next == target {
                let mut path = vec![target];
                let mut cursor = target;
                while let Some(previous) = predecessor.get(&cursor).copied() {
                    path.push(previous);
                    if previous == source {
                        break;
                    }
                    cursor = previous;
                }
                path.reverse();
                return Some(path);
            }
            queue.push_back(next);
        }
    }
    None
}

pub fn union(left: &BTreeSet<u64>, right: &BTreeSet<u64>) -> BTreeSet<u64> {
    left.union(right).copied().collect()
}

pub fn intersection(left: &BTreeSet<u64>, right: &BTreeSet<u64>) -> BTreeSet<u64> {
    left.intersection(right).copied().collect()
}

pub fn difference(left: &BTreeSet<u64>, right: &BTreeSet<u64>) -> BTreeSet<u64> {
    left.difference(right).copied().collect()
}

pub fn remap(database: &RefDatabase, mapping: &BTreeMap<u64, u64>) -> RefDatabase {
    let mut output = RefDatabase::default();
    for (old, node) in &database.nodes {
        let id = mapping[old];
        let mut node = node.clone();
        node.id = id;
        output.nodes.insert(id, node);
    }
    output.edges = database
        .edges
        .iter()
        .map(|edge| RefEdge {
            source: mapping[&edge.source],
            target: mapping[&edge.target],
            label: edge.label.clone(),
        })
        .collect();
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 多方向可达性与最短路径满足深度单调性() {
        let database = RefDatabase::fixture(12);
        let shallow = reachable(&database, 1, 1, 2, None, TraverseDirection::Forward);
        let deep = reachable(&database, 1, 1, 5, None, TraverseDirection::Forward);
        assert!(shallow.keys().all(|id| deep.contains_key(id)));
        assert_eq!(
            shortest_path(&database, 1, 5, 8, None, TraverseDirection::Forward),
            Some(vec![1, 2, 3, 4, 5])
        );
        assert_eq!(
            shortest_path(&database, 5, 1, 8, None, TraverseDirection::Backward),
            Some(vec![5, 4, 3, 2, 1])
        );
    }

    #[test]
    fn 集合代数满足幂等交换和差集性质() {
        let a = BTreeSet::from([1, 2, 3]);
        let b = BTreeSet::from([3, 4]);
        assert_eq!(union(&a, &a), a);
        assert_eq!(intersection(&a, &b), intersection(&b, &a));
        assert!(difference(&a, &a).is_empty());
    }

    #[test]
    fn 节点双射重映射保持图可达结构同构() {
        let database = RefDatabase::fixture(8);
        let mapping = database
            .nodes
            .keys()
            .map(|id| (*id, id + 100))
            .collect::<BTreeMap<_, _>>();
        let mapped = remap(&database, &mapping);
        let original = reachable(&database, 1, 0, 4, None, TraverseDirection::Both);
        let actual = reachable(&mapped, 101, 0, 4, None, TraverseDirection::Both);
        let expected = original
            .into_iter()
            .map(|(id, depth)| (mapping[&id], depth))
            .collect();
        assert_eq!(actual, expected);
    }
}
