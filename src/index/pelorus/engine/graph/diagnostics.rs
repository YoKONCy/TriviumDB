//! 构图完成后的只读结构诊断，不参与寻路、候选选择或剪枝。

use super::QuIVer;

#[derive(Debug)]
pub struct NodeDiagnostics {
    pub id: u64,
    pub out_degree: usize,
    pub in_degree: usize,
    pub reachable: bool,
    pub active: bool,
}

#[derive(Debug)]
pub struct GraphDiagnostics {
    pub nodes: Vec<NodeDiagnostics>,
    pub entry_id: Option<u64>,
    pub edges: usize,
    pub reachable_nodes: usize,
    pub weak_components: usize,
    pub largest_weak_component: usize,
    pub zero_out_degree: usize,
    pub zero_in_degree: usize,
    pub self_loops: usize,
    pub duplicate_edges: usize,
    pub invalid_edges: usize,
    pub max_level: usize,
    pub topology_crc32: u32,
}

fn root(parents: &mut [usize], mut node: usize) -> usize {
    while parents[node] != node {
        parents[node] = parents[parents[node]];
        node = parents[node];
    }
    node
}

impl QuIVer {
    pub(crate) fn graph_diagnostics(&self) -> GraphDiagnostics {
        let mut incoming = vec![0; self.n];
        let mut parents = (0..self.n).collect::<Vec<_>>();
        let mut sizes = vec![1; self.n];
        let mut components = self.n;
        let mut edges = 0;
        let mut invalid_edges = 0;
        let mut self_loops = 0;
        let mut duplicate_edges = 0;
        let mut topology = crc32fast::Hasher::new();
        topology.update(&(self.n as u64).to_le_bytes());
        topology.update(&self.entry_point.to_le_bytes());
        for node in 0..self.n {
            let neighbors = self.layer0.neighbors(node as u32);
            topology.update(&self.ids[node].to_le_bytes());
            topology.update(&(neighbors.len() as u64).to_le_bytes());
            for neighbor in neighbors {
                topology.update(&neighbor.to_le_bytes());
            }
            let mut sorted = neighbors.to_vec();
            sorted.sort_unstable();
            duplicate_edges += sorted.windows(2).filter(|pair| pair[0] == pair[1]).count();
            edges += neighbors.len();
            for &neighbor in neighbors {
                let neighbor = neighbor as usize;
                if neighbor >= self.n {
                    invalid_edges += 1;
                    continue;
                }
                self_loops += usize::from(neighbor == node);
                incoming[neighbor] += 1;
                let mut a = root(&mut parents, node);
                let mut b = root(&mut parents, neighbor);
                if a != b {
                    if sizes[a] < sizes[b] {
                        std::mem::swap(&mut a, &mut b);
                    }
                    parents[b] = a;
                    sizes[a] += sizes[b];
                    components -= 1;
                }
            }
        }
        let mut reachable = vec![false; self.n];
        let mut queue = Vec::new();
        if self.n > 0 {
            reachable[self.entry_point as usize] = true;
            queue.push(self.entry_point);
        }
        let mut cursor = 0;
        while cursor < queue.len() {
            for &neighbor in self.layer0.neighbors(queue[cursor]) {
                if (neighbor as usize) < self.n && !reachable[neighbor as usize] {
                    reachable[neighbor as usize] = true;
                    queue.push(neighbor);
                }
            }
            cursor += 1;
        }
        let nodes = (0..self.n)
            .map(|i| NodeDiagnostics {
                id: self.ids[i],
                out_degree: self.layer0.neighbors(i as u32).len(),
                in_degree: incoming[i],
                reachable: reachable[i],
                active: !self.tombstones[i],
            })
            .collect::<Vec<_>>();
        GraphDiagnostics {
            entry_id: (self.n > 0).then(|| self.ids[self.entry_point as usize]),
            edges,
            reachable_nodes: queue.len(),
            weak_components: components,
            largest_weak_component: sizes.into_iter().max().unwrap_or(0),
            zero_out_degree: nodes.iter().filter(|node| node.out_degree == 0).count(),
            zero_in_degree: nodes.iter().filter(|node| node.in_degree == 0).count(),
            nodes,
            self_loops,
            duplicate_edges,
            invalid_edges,
            max_level: self.max_level,
            topology_crc32: topology.finalize(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::pelorus::engine::BuildConfig;

    #[test]
    fn 区分有向可达性与弱连通且墓碑仍可中转() {
        let mut graph = QuIVer::new(2, &BuildConfig::default());
        graph.n = 5;
        graph.ids = vec![10, 20, 30, 40, 50];
        graph.tombstones = vec![false, true, false, false, false];
        graph.layer0.reset_full(5);
        graph.layer0.set_neighbors(0, &[1]);
        graph.layer0.set_neighbors(1, &[2]);
        graph.layer0.set_neighbors(3, &[2]);
        let before = graph.layer0.data.clone();
        let result = graph.graph_diagnostics();
        assert_eq!(result.entry_id, Some(10));
        assert_eq!(result.reachable_nodes, 3);
        assert_eq!(result.weak_components, 2);
        assert_eq!(result.largest_weak_component, 4);
        assert_eq!(result.edges, 3);
        assert_eq!(result.zero_out_degree, 2);
        assert_eq!(result.zero_in_degree, 3);
        assert_eq!(result.nodes[2].in_degree, 2);
        assert!(!result.nodes[1].active && result.nodes[2].reachable);
        assert_eq!(graph.layer0.data, before);
    }

    #[test]
    fn 空图及畸形边诊断不改变原始数据() {
        let mut graph = QuIVer::new(2, &BuildConfig::default());
        assert_eq!(graph.graph_diagnostics().entry_id, None);
        graph.n = 2;
        graph.ids = vec![1, 2];
        graph.tombstones = vec![false; 2];
        graph.layer0.reset_full(2);
        graph.layer0.set_neighbors(0, &[0, 1, 1, 99]);
        let result = graph.graph_diagnostics();
        assert_eq!(result.self_loops, 1);
        assert_eq!(result.duplicate_edges, 1);
        assert_eq!(result.invalid_edges, 1);
        assert_eq!(result.reachable_nodes, 2);
    }
}
