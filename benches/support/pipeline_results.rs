//! 管线基准逐项验证执行来源，并按原始顺序比较完整逻辑行。

use triviumdb::query::pipeline::{NodeSet, PropertyOrigin};

pub fn assert_matched_results(
    label: &str,
    baseline: &NodeSet,
    optimized: &NodeSet,
    expected_origins: [Option<PropertyOrigin>; 2],
) {
    let logical_rows = |set: &NodeSet, expected: &Option<PropertyOrigin>| {
        let mut rows = set.clone().into_rows();
        for row in &mut rows {
            assert_eq!(
                &row.provenance.property_origin, expected,
                "{label} 节点 {} 的属性执行来源不符 (Unexpected property execution origin)",
                row.id
            );
            row.provenance.property_origin = None;
        }
        rows
    };
    assert_eq!(
        logical_rows(baseline, &expected_origins[0]),
        logical_rows(optimized, &expected_origins[1]),
        "{label} 完整逻辑行或原始排序不一致 (Logical rows or original ordering differ)"
    );
}
