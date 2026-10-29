//! 验证入口的有向可达性，并在维护时保护覆盖全图的导航树。

use super::*;

impl QuIVer {
    /// 同码近邻占满名额时，利用原有余量补充未被表示的签名，防止剪掉全部跨簇出口。
    /// 没有签名碰撞的选择结果逐边不变，容量仍受既有邻接表限制。
    pub(super) fn select_diverse_layer0(
        sigs: &Bq2Store,
        target: u32,
        candidates: &[(u32, u32)],
        max_k: usize,
        dim: usize,
        alpha: f32,
    ) -> Vec<u32> {
        let mut selected = Self::vamana_select(sigs, target, candidates, max_k, dim, alpha);
        let collision = selected.iter().enumerate().any(|(i, &node)| {
            selected[..i]
                .iter()
                .any(|&other| sigs.same_signature(node as usize, other as usize))
        });
        if !collision {
            return selected;
        }
        for &(_, candidate) in candidates {
            if selected.len() >= max_k * 2 {
                break;
            }
            if candidate != target
                && selected
                    .iter()
                    .all(|&node| !sigs.same_signature(candidate as usize, node as usize))
            {
                selected.push(candidate);
            }
        }
        selected
    }

    /// 构图收尾时只连接不可达区域；已经全可达的图保持逐边不变。
    pub(super) fn repair_layer0_connectivity(&mut self) -> usize {
        self.connectivity_parents.clear();
        self.connectivity_parents.resize(self.n, EMPTY_NB);
        if self.n == 0 {
            return 0;
        }

        let mut seen = Bitset::new(self.n);
        // 一次预留明确上界，避免 Vec 倍增使实际遍历峰值超过构图预估。
        let mut stack = Vec::with_capacity(self.n);
        stack.push(self.entry_point);
        seen.set(self.entry_point as usize);
        let mut cursor = 0;
        let mut bridges = 0;
        loop {
            let mut leaf = self.entry_point;
            while let Some(node) = stack.pop() {
                leaf = node;
                for &neighbor in self.layer0.neighbors(node) {
                    if !seen.test(neighbor as usize) {
                        seen.set(neighbor as usize);
                        self.connectivity_parents[neighbor as usize] = node;
                        stack.push(neighbor);
                    }
                }
            }
            while cursor < self.n && seen.test(cursor) {
                cursor += 1;
            }
            if cursor == self.n {
                break;
            }

            // 最后处理的叶节点没有导航树子边；满容量时可安全腾出一条非树边。
            // 默认构图剪枝后只有 m0 条边，已有的双倍容量通常无需删除任何近邻。
            let mut neighbors = self.layer0.neighbors(leaf).to_vec();
            if neighbors.len() == self.layer0.stride - 1 {
                neighbors.pop();
            }
            neighbors.push(cursor as u32);
            self.layer0.set_neighbors(leaf, &neighbors);
            self.connectivity_parents[cursor] = leaf;
            seen.set(cursor);
            stack.push(cursor as u32);
            bridges += 1;
        }
        bridges
    }

    /// 加载只恢复运行期父节点缓存，不修改邻接表；断连索引交由现有流程重建。
    pub(super) fn restore_navigation_tree(&mut self) -> std::io::Result<()> {
        self.connectivity_parents.clear();
        self.connectivity_parents.resize(self.n, EMPTY_NB);
        if self.n == 0 {
            return Ok(());
        }
        let mut seen = Bitset::new(self.n);
        let mut stack = Vec::with_capacity(self.n);
        stack.push(self.entry_point);
        seen.set(self.entry_point as usize);
        let mut reached = 0;
        while let Some(node) = stack.pop() {
            reached += 1;
            for &neighbor in self.layer0.neighbors(node) {
                if !seen.test(neighbor as usize) {
                    seen.set(neighbor as usize);
                    self.connectivity_parents[neighbor as usize] = node;
                    stack.push(neighbor);
                }
            }
        }
        if reached != self.n {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "QuIVer 索引存在入口不可达节点，需要重建索引",
            ));
        }
        Ok(())
    }

    /// 常规近邻选择保持原规则，只补回将被剪掉的导航树子边。
    pub(super) fn select_layer0_neighbors(
        &self,
        target: u32,
        candidates: &[(u32, u32)],
    ) -> Vec<u32> {
        let mut selected = Self::select_diverse_layer0(
            &self.bq_store,
            target,
            candidates,
            self.m0,
            self.dim,
            self.alpha,
        );
        for &(_, candidate) in candidates {
            if self.connectivity_parents[candidate as usize] == target
                && !selected.contains(&candidate)
            {
                selected.push(candidate);
            }
        }
        // 原有邻接表容量不变；树边优先，超出容量时只移除普通近邻。
        let mut excess = selected.len().saturating_sub(self.layer0.stride - 1);
        if excess > 0 {
            selected.reverse();
            selected.retain(|&node| {
                if excess > 0 && self.connectivity_parents[node as usize] != target {
                    excess -= 1;
                    false
                } else {
                    true
                }
            });
            selected.reverse();
        }
        selected
    }

    /// 增量节点接入导航树；入口升层时把旧树挂到新入口，避免旧树失去访问路径。
    pub(super) fn finish_layer0_insertion(&mut self, node: u32, previous_entry: u32) {
        let (parent, child) = if self.entry_point != previous_entry {
            (node, previous_entry)
        } else {
            let available = |&candidate: &u32| {
                self.layer0
                    .neighbors(candidate)
                    .iter()
                    .filter(|&&neighbor| self.connectivity_parents[neighbor as usize] == candidate)
                    .count()
                    < self.m0
            };
            let parent = self
                .layer0
                .neighbors(node)
                .iter()
                .copied()
                .find(|candidate| available(candidate) && self.layer0.contains(*candidate, node))
                .or_else(|| self.layer0.neighbors(node).iter().copied().find(available))
                .or_else(|| (0..node).find(available));
            let Some(parent) = parent else {
                return;
            };
            (parent, node)
        };
        self.connectivity_parents[child as usize] = parent;
        if self.layer0.contains(parent, child) {
            return;
        }
        let mut candidates = self
            .layer0
            .neighbors(parent)
            .iter()
            .map(|&neighbor| {
                (
                    self.bq_store
                        .distance_in_bounds(parent as usize, neighbor as usize, self.dim),
                    neighbor,
                )
            })
            .collect::<Vec<_>>();
        if !self.layer0.contains(parent, child) {
            candidates.push((
                self.bq_store
                    .distance_in_bounds(parent as usize, child as usize, self.dim),
                child,
            ));
        }
        candidates.sort_unstable_by_key(|&(distance, _)| distance);
        let selected = self.select_layer0_neighbors(parent, &candidates);
        self.layer0.set_neighbors(parent, &selected);
    }
}
