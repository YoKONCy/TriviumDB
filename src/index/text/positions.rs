//! 文档内存语料的位置posting与有界有序匹配。
use super::analyzer::AnalyzedToken;
use crate::document::{DocumentMatchMode, DocumentQueryLimits, budget};
use crate::{NodeId, Result, TriviumError};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

#[derive(Debug, Clone, Copy)]
pub(crate) struct TokenPosition {
    pub position: usize,
    pub start_byte: usize,
    pub end_byte: usize,
}

pub(crate) struct PositionQuery<'a> {
    pub tokens: &'a [AnalyzedToken],
    pub mode: DocumentMatchMode,
    pub max_gap: usize,
    pub limits: &'a DocumentQueryLimits,
}

pub(crate) struct TextMatch<'a> {
    pub start_byte: usize,
    pub end_byte: usize,
    pub terms: Vec<(&'a str, TokenPosition)>,
}

#[derive(Default)]
pub(crate) struct PositionIndex {
    postings: BTreeMap<String, BTreeMap<NodeId, Vec<TokenPosition>>>,
    bytes: usize,
}

#[derive(Clone, Copy)]
struct Completion {
    end_position: usize,
    next: usize,
}

fn allocation(error: std::collections::TryReserveError) -> TriviumError {
    TriviumError::CapacityAllocationFailed {
        reason: format!(
            "位置查询工作区分配失败 (Position query workspace allocation failed): {error}"
        ),
    }
}

fn charge(visits: &mut usize, limits: &DocumentQueryLimits) -> Result<()> {
    *visits = visits.saturating_add(1);
    budget(
        "document_position_visits",
        *visits,
        limits.max_position_visits,
    )?;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::DocumentPositionStep);
    crate::request::check_current()
}

impl PositionIndex {
    pub fn add(&mut self, id: NodeId, token: AnalyzedToken) {
        if !self.postings.contains_key(&token.term) {
            self.bytes = self.bytes.saturating_add(
                std::mem::size_of::<(String, BTreeMap<NodeId, Vec<TokenPosition>>)>()
                    + 64
                    + token.term.capacity(),
            );
        }
        let documents = self.postings.entry(token.term).or_default();
        if !documents.contains_key(&id) {
            self.bytes = self
                .bytes
                .saturating_add(std::mem::size_of::<(NodeId, Vec<TokenPosition>)>() + 64);
        }
        let positions = documents.entry(id).or_default();
        let previous = positions.capacity();
        positions.push(TokenPosition {
            position: token.position,
            start_byte: token.start_byte,
            end_byte: token.end_byte,
        });
        self.bytes = self.bytes.saturating_add(
            positions
                .capacity()
                .saturating_sub(previous)
                .saturating_mul(std::mem::size_of::<TokenPosition>()),
        );
    }

    pub fn heap_bytes(&self) -> usize {
        self.bytes
    }

    fn occurrences(&self, id: NodeId, term: &str) -> &[TokenPosition] {
        self.postings
            .get(term)
            .and_then(|documents| documents.get(&id))
            .map_or(&[], Vec::as_slice)
    }

    /// DP只为实际posting分配，重复查询词的工作量也逐次计入预算。
    pub fn workspace_bytes(
        &self,
        id: NodeId,
        query: &PositionQuery<'_>,
        visits: usize,
    ) -> Result<usize> {
        let query_bytes = query.tokens.len().saturating_mul(256);
        if query.mode == DocumentMatchMode::Any {
            return Ok(query_bytes);
        }
        let mut count = 0usize;
        for token in query.tokens {
            let occurrences = self.occurrences(id, &token.term);
            if occurrences.is_empty() {
                return Ok(query_bytes);
            }
            count = count.saturating_add(occurrences.len());
        }
        budget(
            "document_position_visits",
            visits.saturating_add(count),
            query.limits.max_position_visits,
        )?;
        Ok(query_bytes.saturating_add(count.saturating_mul(80)))
    }

    /// 返回每个起始词项的最短完整窗口，同终点时选词序最靠前的链。
    pub fn visit_matches<'a>(
        &self,
        id: NodeId,
        query: &'a PositionQuery<'a>,
        visits: &mut usize,
        mut emit: impl FnMut(TextMatch<'a>) -> Result<bool>,
    ) -> Result<bool> {
        if query.tokens.is_empty() {
            return Ok(false);
        }
        if query.mode == DocumentMatchMode::Any {
            return self.visit_any(id, query, visits, emit);
        }
        let lists = query
            .tokens
            .iter()
            .map(|token| self.occurrences(id, &token.term))
            .collect::<Vec<_>>();
        if lists.iter().any(|list| list.is_empty()) {
            return Ok(false);
        }
        let layers = completion_layers(&lists, query, visits)?;
        let mut found = false;
        for (index, first) in lists[0].iter().enumerate() {
            charge(visits, query.limits)?;
            let Some(completion) = layers[0][index] else {
                continue;
            };
            let gap = completion
                .end_position
                .saturating_sub(first.position)
                .saturating_sub(query.tokens.len().saturating_sub(1));
            if gap > query.max_gap {
                continue;
            }
            let mut terms = Vec::new();
            terms
                .try_reserve_exact(query.tokens.len())
                .map_err(allocation)?;
            let mut next = index;
            let mut end = first.end_byte;
            for (layer, token) in query.tokens.iter().enumerate() {
                charge(visits, query.limits)?;
                let occurrence = *lists[layer].get(next).ok_or_else(|| {
                    TriviumError::CorruptedFile(
                        "位置链索引无效 (Invalid position chain index)".into(),
                    )
                })?;
                let cell = layers[layer][next].ok_or_else(|| {
                    TriviumError::CorruptedFile(
                        "位置链缺少后继 (Position chain has no successor)".into(),
                    )
                })?;
                terms.push((token.term.as_str(), occurrence));
                next = cell.next;
                end = occurrence.end_byte;
            }
            found = true;
            if !emit(TextMatch {
                start_byte: first.start_byte,
                end_byte: end,
                terms,
            })? {
                return Ok(true);
            }
        }
        Ok(found)
    }

    fn visit_any<'a>(
        &self,
        id: NodeId,
        query: &'a PositionQuery<'a>,
        visits: &mut usize,
        mut emit: impl FnMut(TextMatch<'a>) -> Result<bool>,
    ) -> Result<bool> {
        let unique = query
            .tokens
            .iter()
            .map(|token| (token.term.as_str(), self.occurrences(id, &token.term)))
            .collect::<BTreeMap<_, _>>();
        let lists = unique.into_iter().collect::<Vec<_>>();
        let mut heap = BinaryHeap::new();
        for (index, (_, list)) in lists.iter().enumerate() {
            if let Some(first) = list.first() {
                heap.push(Reverse((first.position, index, 0usize)));
            }
        }
        let mut found = false;
        while let Some(Reverse((_, index, offset))) = heap.pop() {
            charge(visits, query.limits)?;
            // 堆元素由下面的长度检查生成，index与offset始终指向对应posting。
            let (term, list) = lists[index];
            let occurrence = list[offset];
            found = true;
            if !emit(TextMatch {
                start_byte: occurrence.start_byte,
                end_byte: occurrence.end_byte,
                terms: vec![(term, occurrence)],
            })? {
                return Ok(true);
            }
            let next = offset.saturating_add(1);
            if let Some(occurrence) = list.get(next) {
                heap.push(Reverse((occurrence.position, index, next)));
            }
        }
        Ok(found)
    }
}

fn completion_layers(
    lists: &[&[TokenPosition]],
    query: &PositionQuery<'_>,
    visits: &mut usize,
) -> Result<Vec<Vec<Option<Completion>>>> {
    let mut layers: Vec<Vec<Option<Completion>>> = Vec::new();
    for i in (0..lists.len()).rev() {
        let list = lists[i];
        let mut cells = Vec::new();
        cells.try_reserve_exact(list.len()).map_err(allocation)?;
        let mut suffix = Vec::new();
        if let Some(next) = layers.last() {
            suffix
                .try_reserve_exact(next.len().saturating_add(1))
                .map_err(allocation)?;
            suffix.resize(next.len().saturating_add(1), None);
            let mut best: Option<(usize, usize)> = None;
            for j in (0..next.len()).rev() {
                charge(visits, query.limits)?;
                if let Some(cell) = next[j] {
                    let candidate = (cell.end_position, j);
                    if best.is_none_or(|best| candidate <= best) {
                        best = Some(candidate);
                    }
                }
                suffix[j] = best.map(|(_, index)| index);
            }
        }
        for occurrence in list {
            charge(visits, query.limits)?;
            let Some(next) = layers.last() else {
                cells.push(Some(Completion {
                    end_position: occurrence.position,
                    next: 0,
                }));
                continue;
            };
            let following = lists[i + 1];
            let overlapping = query.tokens[i + 1].start_byte < query.tokens[i].end_byte;
            let choice = if overlapping {
                following
                    .binary_search_by_key(&occurrence.position.saturating_add(1), |item| {
                        item.position
                    })
                    .ok()
                    .filter(|&index| following[index].start_byte < occurrence.end_byte)
            } else {
                let lower = following.partition_point(|item| {
                    item.position <= occurrence.position || item.start_byte < occurrence.end_byte
                });
                suffix.get(lower).copied().flatten()
            };
            let completion = choice.and_then(|index| {
                next[index].map(|cell| Completion {
                    end_position: cell.end_position,
                    next: index,
                })
            });
            cells.push(completion);
        }
        layers.push(cells);
    }
    layers.reverse();
    Ok(layers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 位置堆内存账本覆盖词项节点及出现数组扩容() {
        let mut index = PositionIndex::default();
        for position in 0..130 {
            index.add(
                1,
                AnalyzedToken {
                    term: "alpha".into(),
                    position,
                    start_byte: position * 2,
                    end_byte: position * 2 + 1,
                },
            );
            index.add(
                2,
                AnalyzedToken {
                    term: format!("term-{position}"),
                    position,
                    start_byte: position * 2,
                    end_byte: position * 2 + 1,
                },
            );
            let actual = index
                .postings
                .iter()
                .map(|(term, documents)| {
                    std::mem::size_of::<(String, BTreeMap<NodeId, Vec<TokenPosition>>)>()
                        + 64
                        + term.capacity()
                        + documents
                            .values()
                            .map(|positions| {
                                std::mem::size_of::<(NodeId, Vec<TokenPosition>)>()
                                    + 64
                                    + positions.capacity() * std::mem::size_of::<TokenPosition>()
                            })
                            .sum::<usize>()
                })
                .sum::<usize>();
            assert_eq!(index.heap_bytes(), actual);
        }
        assert_eq!(index.occurrences(1, "alpha").len(), 130);
    }
}
