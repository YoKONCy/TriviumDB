#![allow(non_snake_case)]
//! 独立查询语义差分测试入口。
//!
//! Reference evaluator 只读取测试数据模型，不调用生产 Filter、Parser、Planner、索引或 Pipeline。

mod canonical;
mod evaluator;
mod expression;
mod find_reference;
mod generator;
mod graph_reference;
mod matrix;
mod model;
mod physical_strategy;
mod pipeline_reference;
mod prepared;
mod replay;
mod shrinker;
