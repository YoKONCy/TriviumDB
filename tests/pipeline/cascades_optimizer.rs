//! Cascades Memo、成本选择、规则应用与优化预算测试。
//! 计划只按稳定统计变化，且候选剪枝不能改变最终查询语义。

use serde_json::json;
use triviumdb::query::cascades::{
    OptimizationStatus, OptimizerBudget, PhysicalOperator, optimize_pipeline,
};
use triviumdb::query::pipeline::{
    ExactRerank, NodeIdsSource, PayloadFilter, PipelineBudget, PipelineContext, PipelineOperator,
    execute_pipeline,
};
use triviumdb::query::tql_executor::{execute_tql, execute_tql_values};
use triviumdb::query::tql_parser::parse_tql;
use triviumdb::storage::memtable::MemTable;

fn graph() -> MemTable<f32> {
    let mut mt = MemTable::new(2);
    for id in 1..=20 {
        mt.insert_with_id(
            id,
            &[id as f32, 1.0],
            json!({"active": id % 4 == 0, "group": id % 3}),
        )
        .unwrap();
    }
    mt.register_property_index("active");
    for id in 1..20 {
        mt.link(id, id + 1, "next".into(), 1.0).unwrap();
    }
    mt
}

#[test]
fn memo_计划确定且每组选择最低成本实现() {
    let mt = graph();
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 4 AS seed WITH seed EXPAND seed [:next*1..2] AS related WITH related WHERE related.active == true RETURN related",
    )
    .unwrap();
    let first = optimize_pipeline(&query, &mt, OptimizerBudget::default());
    let second = optimize_pipeline(&query, &mt, OptimizerBudget::default());
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&second).unwrap()
    );
    assert!(first.groups.iter().all(|group| {
        let selected = group.alternatives[group.best_alternative].estimated_cost;
        group
            .alternatives
            .iter()
            .all(|alternative| selected <= alternative.estimated_cost)
    }));
}

#[test]
fn similarity_仅在下游引用时插入精确重排() {
    let mt = graph();
    let plain = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 4 AS seed WITH seed EXPAND seed [:next*1..2] AS related WITH related RETURN related",
    )
    .unwrap();
    let scored = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 4 AS seed WITH seed EXPAND seed [:next*1..2] AS related WITH related WHERE similarity(related) > 0.1 RETURN related",
    )
    .unwrap();
    let plain_plan = optimize_pipeline(&plain, &mt, OptimizerBudget::default());
    let scored_plan = optimize_pipeline(&scored, &mt, OptimizerBudget::default());
    assert!(plain_plan.exact_rerank_after.is_empty());
    assert_eq!(scored_plan.exact_rerank_after.len(), 1);
    assert!(scored_plan.stages.iter().any(|stage| {
        stage.operator == PhysicalOperator::ExactRerankHeap && stage.vector_page_reads > 0
    }));
    assert!(
        !plain_plan
            .stages
            .iter()
            .skip(1)
            .any(|stage| stage.operator == PhysicalOperator::ExactRerankHeap)
    );
}

#[test]
fn graphstats_标签扇出影响_expand_基数估算() {
    let mt = graph();
    let labeled = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 2 AS seed WITH seed EXPAND seed [:next*1..1] AS related WITH related RETURN related",
    )
    .unwrap();
    let unrestricted = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 2 AS seed WITH seed EXPAND seed [*1..2] AS related WITH related RETURN related",
    )
    .unwrap();
    let labeled = optimize_pipeline(&labeled, &mt, OptimizerBudget::default());
    let unrestricted = optimize_pipeline(&unrestricted, &mt, OptimizerBudget::default());
    let labeled_rows = labeled
        .stages
        .iter()
        .find(|stage| {
            matches!(
                stage.operator,
                PhysicalOperator::GraphExpandSerial
                    | PhysicalOperator::GraphExpandParallel
                    | PhysicalOperator::GraphExpandIncoming
                    | PhysicalOperator::GraphExpandLabelDirectory
                    | PhysicalOperator::ExpandExactRerank
            )
        })
        .unwrap()
        .estimated_rows;
    let unrestricted_rows = unrestricted
        .stages
        .iter()
        .find(|stage| {
            matches!(
                stage.operator,
                PhysicalOperator::GraphExpandSerial
                    | PhysicalOperator::GraphExpandParallel
                    | PhysicalOperator::GraphExpandIncoming
                    | PhysicalOperator::GraphExpandLabelDirectory
                    | PhysicalOperator::ExpandExactRerank
            )
        })
        .unwrap()
        .estimated_rows;
    assert!(labeled_rows <= unrestricted_rows);
}

#[test]
fn 直方图二阶矩避免幂律_hub_扇出被平均值低估() {
    let mut uniform = MemTable::new(2);
    let mut power_law = MemTable::new(2);
    for id in 1..=100 {
        uniform
            .insert_with_id(id, &[id as f32, 1.0], json!({}))
            .unwrap();
        power_law
            .insert_with_id(id, &[id as f32, 1.0], json!({}))
            .unwrap();
    }
    for id in 1..=50 {
        uniform.link(id, id + 1, "edge".into(), 1.0).unwrap();
        let target = id % 99 + 2;
        power_law.link(1, target, "edge".into(), 1.0).unwrap();
    }
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed EXPAND seed [*1..1] AS related WITH related RETURN related",
    )
    .unwrap();
    let uniform_plan = optimize_pipeline(&query, &uniform, OptimizerBudget::default());
    let power_law_plan = optimize_pipeline(&query, &power_law, OptimizerBudget::default());
    let expand_rows = |plan: &triviumdb::query::cascades::CascadesPlan| {
        plan.stages
            .iter()
            .find(|stage| {
                matches!(
                    stage.operator,
                    PhysicalOperator::GraphExpandSerial
                        | PhysicalOperator::GraphExpandParallel
                        | PhysicalOperator::GraphExpandIncoming
                        | PhysicalOperator::GraphExpandLabelDirectory
                        | PhysicalOperator::ExpandExactRerank
                )
            })
            .unwrap()
            .estimated_rows
    };
    assert!(power_law.graph_stats().avg_out_degree <= uniform.graph_stats().avg_out_degree);
    assert!(expand_rows(&power_law_plan) > expand_rows(&uniform_plan));
}

#[test]
fn 相关字段对修正独立性假设导致的基数低估() {
    let mut mt = MemTable::new(2);
    for id in 1..=100 {
        let city = id % 10;
        mt.insert_with_id(
            id,
            &[id as f32, 1.0],
            json!({"city": city, "province": city}),
        )
        .unwrap();
    }
    mt.register_property_index("city");
    mt.register_property_index("province");
    mt.register_composite_property_index(&["city".into(), "province".into()]);
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 100 AS seed WITH seed WHERE seed.city == 1 AND seed.province == 1 RETURN seed",
    )
    .unwrap();
    let plan = optimize_pipeline(&query, &mt, OptimizerBudget::default());
    let filter = plan
        .stages
        .iter()
        .find(|stage| {
            matches!(
                stage.operator,
                PhysicalOperator::PayloadFilterScan
                    | PhysicalOperator::PropertyHashLookup
                    | PhysicalOperator::PropertyOrderedLookup
                    | PhysicalOperator::PropertyCompositeLookup
                    | PhysicalOperator::PropertyBitmapLookup
                    | PhysicalOperator::PropertyIndexIntersection
            )
        })
        .unwrap();
    assert_eq!(filter.estimated_rows, 10);
}

#[test]
fn 属性图偏斜让高扇出与低扇出集合得到不同_expand_估算() {
    let mut mt = MemTable::new(2);
    for id in 1..=100 {
        mt.insert_with_id(id, &[id as f32, 1.0], json!({"active": id <= 10}))
            .unwrap();
    }
    for source in 1..=10 {
        for offset in 1..=10 {
            mt.link(source, 10 + (source + offset) % 90, "edge".into(), 1.0)
                .unwrap();
        }
    }
    for source in 11..=100 {
        mt.link(source, source % 100 + 1, "edge".into(), 1.0)
            .unwrap();
    }
    mt.register_property_index("active");
    let query = |value| {
        parse_tql(&format!(
            "SEARCH VECTOR [1, 0] TOP 10 AS seed WITH seed WHERE seed.active == {value} WITH seed EXPAND seed [*1..1] AS related WITH related RETURN related"
        ))
        .unwrap()
    };
    let high = optimize_pipeline(&query("true"), &mt, OptimizerBudget::default());
    let low = optimize_pipeline(&query("false"), &mt, OptimizerBudget::default());
    let expand_rows = |plan: &triviumdb::query::cascades::CascadesPlan| {
        plan.stages
            .iter()
            .find(|stage| {
                matches!(
                    stage.operator,
                    PhysicalOperator::GraphExpandSerial
                        | PhysicalOperator::GraphExpandParallel
                        | PhysicalOperator::GraphExpandIncoming
                        | PhysicalOperator::GraphExpandLabelDirectory
                        | PhysicalOperator::ExpandExactRerank
                )
            })
            .unwrap()
            .estimated_rows
    };
    assert!(expand_rows(&high) > expand_rows(&low));
    let stats = mt.cross_modal_stats("active", &json!(true)).unwrap();
    assert!(stats.degree_skew > 1.0);
    assert_eq!(stats.generation, mt.generation());
}

#[test]
fn 跨模统计在写入后失效并按新_generation_重算() {
    let mut mt = graph();
    let first = mt.cross_modal_stats("active", &json!(true)).unwrap();
    mt.link(4, 1, "extra".into(), 1.0).unwrap();
    let second = mt.cross_modal_stats("active", &json!(true)).unwrap();
    assert!(second.generation > first.generation);
    assert_ne!(second.degree_skew, first.degree_skew);
}

#[test]
fn 优化器搜索预算有界且预算切片总量不超限() {
    let mt = graph();
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 4 AS seed WITH seed EXPAND seed [:next*1..2] AS a WITH a EXPAND a [:next*1..2] AS b WITH b WHERE similarity(b) > 0.1 RETURN b",
    )
    .unwrap();
    let memory = 64 * 1024;
    let plan = optimize_pipeline(
        &query,
        &mt,
        OptimizerBudget {
            max_groups: 3,
            max_expressions: 3,
            query_memory_bytes: memory,
        },
    );
    assert_eq!(plan.status, OptimizationStatus::Fallback);
    assert_eq!(plan.stages.len(), query.pipeline.len() + 1);
    assert!(plan.groups.len() <= 3);
    assert!(plan.explored_expressions <= 3);
    assert!(plan.pruned_expressions > 0);
    assert!(
        plan.stages
            .iter()
            .map(|stage| stage.budget_bytes)
            .sum::<usize>()
            <= memory
    );
}

#[test]
fn 变换规则拒绝跨越_expand_并记录语义原因() {
    let mt = graph();
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 4 AS seed WITH seed EXPAND seed [:next*1..2] AS related WITH related WHERE related.active == true RETURN related",
    )
    .unwrap();
    let plan = optimize_pipeline(&query, &mt, OptimizerBudget::default());
    let rule = plan
        .rules
        .iter()
        .find(|rule| rule.name == "push_filter_below_expand")
        .unwrap();
    assert!(!rule.applied);
    assert!(rule.reason.contains("改变语义"));
}

#[test]
fn 两个合法物理计划保持完全相同结果() {
    let mt = graph();
    let ids = mt.all_node_ids();
    let query_vector = vec![1.0, 0.0];
    let filter = triviumdb::Filter::eq("active", json!(true));
    let plans: [Vec<Box<dyn PipelineOperator<f32>>>; 2] = [
        vec![
            Box::new(NodeIdsSource { ids: ids.clone() }),
            Box::new(PayloadFilter {
                filter: filter.clone(),
            }),
            Box::new(ExactRerank {
                query: query_vector.clone(),
                top_k: Some(4),
            }),
        ],
        vec![
            Box::new(NodeIdsSource { ids }),
            Box::new(ExactRerank {
                query: query_vector,
                top_k: None,
            }),
            Box::new(PayloadFilter { filter }),
            Box::new(ExactRerank {
                query: vec![1.0, 0.0],
                top_k: Some(4),
            }),
        ],
    ];
    let mut outputs = Vec::new();
    for plan in plans {
        let mut context = PipelineContext::new(&mt, PipelineBudget::default());
        outputs.push(execute_pipeline(&mut context, &plan).unwrap());
    }
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0].len(), 4);
}

#[test]
fn 零优化预算显式返回_budget_exceeded且仍给出完整fallback计划() {
    let mt = graph();
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 4 AS seed WITH seed EXPAND seed [:next*1..2] AS related WITH related RETURN related",
    )
    .unwrap();
    let plan = optimize_pipeline(
        &query,
        &mt,
        OptimizerBudget {
            max_groups: 0,
            max_expressions: 0,
            query_memory_bytes: 4096,
        },
    );
    assert_eq!(plan.status, OptimizationStatus::BudgetExceeded);
    assert_eq!(plan.stages.len(), query.pipeline.len() + 1);
}

#[test]
fn 物理候选暴露索引类型_expand方向_融合和完整属性() {
    let mut mt = graph();
    mt.register_property_index("active");
    mt.register_ordered_property_index("active");
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 4 AS seed WITH seed WHERE seed.active == true WITH seed EXPAND seed INCOMING [:next*1..1] AS related WITH related RANK related BY VECTOR [1, 0] TOP 2 RETURN related",
    )
    .unwrap();
    let plan = optimize_pipeline(&query, &mt, OptimizerBudget::default());
    assert_eq!(plan.status, OptimizationStatus::Complete);
    assert!(
        plan.groups
            .iter()
            .flat_map(|group| &group.alternatives)
            .any(|alternative| {
                matches!(
                    alternative.operator,
                    PhysicalOperator::PropertyHashLookup | PhysicalOperator::PropertyOrderedLookup
                )
            })
    );
    assert!(plan.stages.iter().any(|stage| {
        matches!(
            stage.operator,
            PhysicalOperator::GraphExpandIncoming | PhysicalOperator::ExpandExactRerank
        )
    }));
    assert!(plan.stages.iter().all(|stage| {
        stage.properties.available_columns.contains("node_id")
            && stage.properties.available_columns.contains("payload")
    }));
}

#[test]
fn 索引候选lowering与扫描reference逐行一致() {
    let mut indexed = graph();
    indexed.register_property_index("active");
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 4 AS seed WITH seed WHERE seed.active == true RETURN seed",
    )
    .unwrap();
    let indexed_rows = execute_tql_values(&query, &indexed).unwrap();
    let scanned_rows = execute_tql_values(&query, &graph()).unwrap();
    let ids = |rows: Vec<
        std::collections::HashMap<String, triviumdb::query::tql_executor::TqlValue<f32>>,
    >| {
        rows.into_iter()
            .flat_map(|row| row.into_values())
            .filter_map(|value| match value {
                triviumdb::query::tql_executor::TqlValue::Node(node) => Some(node.id),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(indexed_rows), ids(scanned_rows));
}

#[test]
fn 物理变换合并相邻filter且结果与单个and谓词一致() {
    let mt = graph();
    let mut merged = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 4 AS seed WITH seed WHERE seed.active == true RETURN seed",
    )
    .unwrap();
    let filter = merged
        .pipeline
        .iter()
        .find_map(|stage| match stage {
            triviumdb::query::tql_ast::PipelineStage::Filter(predicate) => Some(predicate.clone()),
            _ => None,
        })
        .unwrap();
    let filter_index = merged
        .pipeline
        .iter()
        .position(|stage| matches!(stage, triviumdb::query::tql_ast::PipelineStage::Filter(_)))
        .unwrap();
    merged.pipeline.insert(
        filter_index + 1,
        triviumdb::query::tql_ast::PipelineStage::Filter(filter),
    );
    let reference = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 4 AS seed WITH seed WHERE seed.active == true AND seed.active == true RETURN seed",
    )
    .unwrap();
    let plan = optimize_pipeline(&merged, &mt, OptimizerBudget::default());
    assert!(!plan.merged_filter_pairs.is_empty());
    assert_eq!(
        serde_json::to_value(execute_tql_values(&merged, &mt).unwrap().len()).unwrap(),
        serde_json::to_value(execute_tql_values(&reference, &mt).unwrap().len()).unwrap()
    );
}

#[test]
fn pipeline_explain_暴露_cascades_阶段成本和预算() {
    let mt = graph();
    let query = parse_tql(
        "EXPLAIN SEARCH VECTOR [1, 0] TOP 4 AS seed WITH seed EXPAND seed [:next*1..2] AS related WITH related WHERE similarity(related) > 0.1 RETURN related",
    )
    .unwrap();
    let rows = execute_tql(&query, &mt).unwrap();
    assert_eq!(rows.len(), 1);
    let payload = &rows[0].values().next().unwrap().payload;
    assert_eq!(payload["optimizer"], "cascades_memo");
    assert_eq!(payload["optimizer_status"], "complete");
    assert!(payload["memo_groups"].as_u64().unwrap() > 0);
    let stages = payload["pipeline_stages"].as_array().unwrap();
    assert!(stages.iter().all(|stage| {
        stage.get("estimated_rows").is_some()
            && stage.get("budget_bytes").is_some()
            && stage.get("exact").is_some()
            && stage.get("materialized").is_some()
            && stage.get("properties").is_some()
    }));
    assert!(payload["cross_modal_stats"].is_array());
}
