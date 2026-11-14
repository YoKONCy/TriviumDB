//! TQL 源码树与请求级错误来源，独立于公开 AST 和数据库资源。

use crate::error::{ErrorTarget, QueryErrorStage, QuerySpan, TriviumError};
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

/// 解析构造的节点及其原始 UTF-8 范围；合成节点使用空范围。
#[derive(Debug, Clone)]
pub struct TqlSourceNode {
    pub kind: &'static str,
    pub range: Option<Range<usize>>,
    pub children: Vec<TqlSourceNode>,
}

impl TqlSourceNode {
    pub fn child(&self, kind: &str, index: usize) -> Option<&Self> {
        self.children
            .iter()
            .filter(|node| node.kind == kind)
            .nth(index)
    }
}

/// 同一 AST 的源码与来源树，Prepared 克隆及执行请求共享所有权。
#[derive(Debug)]
pub struct TqlSource {
    input: String,
    pub root: TqlSourceNode,
    pub(crate) parameters: BTreeMap<String, Range<usize>>,
}

impl TqlSource {
    pub(crate) fn new(
        input: &str,
        root: TqlSourceNode,
        parameters: BTreeMap<String, Range<usize>>,
    ) -> Self {
        Self {
            input: input.into(),
            root,
            parameters,
        }
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn span(&self, node: &TqlSourceNode) -> Option<QuerySpan> {
        let range = node.range.as_ref()?;
        QuerySpan::range(&self.input, range.start, range.end)
    }

    pub(crate) fn locate_parameter_error(&self, error: TriviumError) -> TriviumError {
        let Some(ErrorTarget::Parameter { parameter }) = error.info().target else {
            return error;
        };
        let Some(range) = self.parameters.get(&parameter) else {
            return error;
        };
        let Some(query_span) = QuerySpan::range(self.input(), range.start, range.end) else {
            return error;
        };
        TriviumError::QueryParameterSource {
            parameter,
            query_span,
            source: Box::new(error),
        }
    }

    pub(crate) fn query(&self) -> &TqlSourceNode {
        if self.root.kind == "statement" {
            self.root.child("query", 0).unwrap_or(&self.root)
        } else {
            &self.root
        }
    }

    pub(crate) fn node(&self, selector: SourceSelector) -> Option<&TqlSourceNode> {
        let query = self.query();
        match selector {
            SourceSelector::Root => Some(&self.root),
            SourceSelector::GraphScope => query.child("graph_scope", 0),
            SourceSelector::Entry => query.child("entry", 0).or_else(|| {
                self.node(SourceSelector::Mutation)?
                    .child("mutation_source", 0)
            }),
            SourceSelector::Stage(index) => query.child("pipeline", 0)?.child("stage", index),
            SourceSelector::WithItem(stage, item) => self
                .node(SourceSelector::Stage(stage))?
                .child("with_item", item),
            SourceSelector::Predicate => query.child("predicate", 0).or_else(|| {
                self.node(SourceSelector::Mutation)?
                    .child("mutation_source", 0)?
                    .child("predicate", 0)
            }),
            SourceSelector::Return => query.child("return", 0),
            SourceSelector::ReturnItem(index) => self
                .node(SourceSelector::Return)?
                .child("return_item", index),
            SourceSelector::Order(index) => query.child("order", index),
            SourceSelector::Rank => query.child("rank", 0),
            SourceSelector::Mutation => self
                .root
                .child("mutation", 0)
                .or_else(|| (self.root.kind == "mutation").then_some(&self.root)),
            SourceSelector::Action => self.node(SourceSelector::Mutation)?.child("action", 0),
            SourceSelector::Assignment(index) => self
                .node(SourceSelector::Action)?
                .child("assignments", 0)?
                .child("assignment", index),
            SourceSelector::Path(path) => path
                .into_iter()
                .try_fold(query, |node, (kind, index)| node.child(kind, index)),
        }
    }

    pub(crate) fn origin(self: &Arc<Self>, selector: SourceSelector) -> QueryOrigin {
        QueryOrigin {
            source: self.clone(),
            range: self.node(selector).and_then(|node| node.range.clone()),
        }
    }
}

/// 保持 AST 与来源配对的解析结果，手工编辑时显式取出无来源的 AST。
#[derive(Debug, Clone)]
pub struct LocatedTql<T> {
    pub(crate) ast: T,
    pub(crate) source: Arc<TqlSource>,
}

impl<T> LocatedTql<T> {
    pub fn ast(&self) -> &T {
        &self.ast
    }
    pub fn source(&self) -> &TqlSource {
        &self.source
    }
    pub fn into_ast(self) -> T {
        self.ast
    }
}

#[derive(Debug, Clone)]
pub(crate) enum SourceSelector {
    Root,
    GraphScope,
    Entry,
    Stage(usize),
    WithItem(usize, usize),
    Predicate,
    Return,
    ReturnItem(usize),
    Order(usize),
    Rank,
    Mutation,
    Action,
    Assignment(usize),
    Path(Vec<(&'static str, usize)>),
}

#[derive(Debug, Clone)]
pub(crate) struct QueryOrigin {
    source: Arc<TqlSource>,
    range: Option<Range<usize>>,
}

impl QueryOrigin {
    pub(crate) fn error(&self, error: TriviumError, stage: QueryErrorStage) -> TriviumError {
        let span = self
            .range
            .as_ref()
            .and_then(|range| QuerySpan::range(self.source.input(), range.start, range.end));
        error.at_query_source(stage, span)
    }

    pub(crate) fn include(mut self, other: &Self) -> Self {
        if let (Some(range), Some(other)) = (&mut self.range, &other.range) {
            range.start = range.start.min(other.start);
            range.end = range.end.max(other.end);
        }
        self
    }
}
