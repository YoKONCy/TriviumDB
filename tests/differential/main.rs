#![allow(non_snake_case)]
//! 独立查询语义差分测试入口。
//!
//! Reference evaluator 只读取测试数据模型，不调用生产 Filter、Parser、Planner、索引或 Pipeline。

mod canonical;
mod cognitive_pipeline_metamorphic;
mod evaluator;
mod expression;
mod extended_pipeline_generator;
mod find_reference;
mod generator;
mod graph_algorithms_reference;
mod graph_reference;
mod matrix;
mod model;
mod physical_strategy;
mod pipeline_generator;
mod pipeline_reference;
mod prepared;
mod replay;
mod shrinker;
