//! 类 MongoDB JSON Payload 过滤表达式及安全求值器。
//!
//! Parser 只接受白名单操作符并验证参数类型，求值时按显式 JSON 类型比较，不进行会
//! 改变语义的隐式字符串/数值转换。该过滤器既可直接扫描，也可由属性索引生成候选，
//! 但最终结果必须通过同一 `matches` 路径精确复核。

use serde_json::Value;

#[derive(Debug, Clone)]
pub enum ComparableValue {
    Number(serde_json::Number),
    String(String),
}

#[derive(Debug, Clone, Copy)]
pub enum RangeOp {
    Gt,
    Gte,
    Lt,
    Lte,
}

/// 过滤条件表达式
/// 支持: $eq, $ne, $gt, $gte, $lt, $lte, $in, $nin, $startsWith, $contains,
///       $exists, $size, $all, $type, $and, $or
#[derive(Debug, Clone)]
pub enum Filter {
    /// 精确匹配: {"field": {"$eq": value}}
    Eq(String, Value),
    /// 不等于
    Ne(String, Value),
    /// 兼容公开 Rust API 的数字范围比较。
    Gt(String, f64),
    Gte(String, f64),
    Lt(String, f64),
    Lte(String, f64),
    /// 数字或字符串的确定性范围比较。
    Range(String, RangeOp, ComparableValue),
    /// 严格 RFC3339 时间比较；阈值保存为 UTC 纳秒时间戳。
    TimeRange(String, RangeOp, i64),
    /// 值在集合中: {"field": {"$in": [v1, v2]}}
    In(String, Vec<Value>),
    /// 逻辑与
    And(Vec<Filter>),
    /// 逻辑或
    Or(Vec<Filter>),
    /// 字段是否存在
    Exists(String, bool),
    /// 值不在集合中
    Nin(String, Vec<Value>),
    /// 数组长度匹配
    Size(String, usize),
    /// 数组包含所有指定元素
    All(String, Vec<Value>),
    /// 字段类型匹配
    TypeMatch(String, String),
    /// 前缀匹配: {"field": {"$startsWith": "/地理"}}
    StartsWith(String, String),
    /// 包含子串: {"field": {"$contains": "关键词"}}
    Contains(String, String),
}

impl Filter {
    /// 逻辑组合返回空，其余条件返回所引用的顶层字段。
    pub(crate) fn field_name(&self) -> Option<&str> {
        match self {
            Self::And(_) | Self::Or(_) => None,
            Self::Eq(k, _)
            | Self::Ne(k, _)
            | Self::Gt(k, _)
            | Self::Gte(k, _)
            | Self::Lt(k, _)
            | Self::Lte(k, _)
            | Self::Range(k, _, _)
            | Self::TimeRange(k, _, _)
            | Self::In(k, _)
            | Self::Nin(k, _)
            | Self::Exists(k, _)
            | Self::Size(k, _)
            | Self::All(k, _)
            | Self::TypeMatch(k, _)
            | Self::StartsWith(k, _)
            | Self::Contains(k, _) => Some(k),
        }
    }
    /// 检查一个 JSON payload 是否满足该过滤条件
    pub fn matches(&self, payload: &Value) -> bool {
        match self {
            Filter::Eq(key, val) => payload.get(key) == Some(val),

            Filter::Ne(key, val) => payload.get(key) != Some(val),

            Filter::Gt(key, threshold) => {
                legacy_numeric_range(payload.get(key), *threshold, RangeOp::Gt)
            }
            Filter::Gte(key, threshold) => {
                legacy_numeric_range(payload.get(key), *threshold, RangeOp::Gte)
            }
            Filter::Lt(key, threshold) => {
                legacy_numeric_range(payload.get(key), *threshold, RangeOp::Lt)
            }
            Filter::Lte(key, threshold) => {
                legacy_numeric_range(payload.get(key), *threshold, RangeOp::Lte)
            }
            Filter::Range(key, op, threshold) => payload
                .get(key)
                .is_some_and(|value| compare_value(value, threshold, *op)),
            Filter::TimeRange(key, op, threshold) => payload
                .get(key)
                .and_then(Value::as_str)
                .and_then(parse_rfc3339_nanos)
                .is_some_and(|value| compare_ordered(value, *threshold, *op)),

            Filter::In(key, values) => {
                if let Some(field_val) = payload.get(key) {
                    values.contains(field_val)
                } else {
                    false
                }
            }
            Filter::Exists(key, exists) => payload.get(key).is_some() == *exists,
            Filter::Nin(key, values) => {
                if let Some(field_val) = payload.get(key) {
                    !values.contains(field_val)
                } else {
                    true
                }
            }
            Filter::Size(key, size) => payload
                .get(key)
                .and_then(|v| v.as_array())
                .is_some_and(|arr| arr.len() == *size),
            Filter::All(key, values) => payload
                .get(key)
                .and_then(|v| v.as_array())
                .is_some_and(|arr| values.iter().all(|val| arr.contains(val))),
            Filter::TypeMatch(key, type_str) => {
                if let Some(v) = payload.get(key) {
                    let actual_type = match v {
                        Value::Null => "null",
                        Value::Bool(_) => "boolean",
                        Value::Number(_) => "number",
                        Value::String(_) => "string",
                        Value::Array(_) => "array",
                        Value::Object(_) => "object",
                    };
                    actual_type == type_str.as_str()
                } else {
                    false
                }
            }

            Filter::And(filters) => filters.iter().all(|f| f.matches(payload)),
            Filter::Or(filters) => filters.iter().any(|f| f.matches(payload)),

            Filter::StartsWith(key, prefix) => payload
                .get(key)
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.starts_with(prefix.as_str())),

            Filter::Contains(key, substr) => payload
                .get(key)
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.contains(substr.as_str())),
        }
    }

    /// 提取出本查询必然要求的特征哈希位掩码（布隆过滤掩码）
    /// 用于在查询图谱全量数组时，实现超音速 O(N) 一级降维打击
    pub fn extract_must_have_mask(&self) -> u64 {
        match self {
            Filter::Eq(key, val) => {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                use std::hash::{Hash, Hasher};
                // Consistent with how fast_tags hashes values
                let val_str = match val {
                    Value::String(s) => s.clone(),
                    v => v.to_string(),
                };
                format!("{}:{}", key, val_str).hash(&mut hasher);
                1u64 << (hasher.finish() % 64)
            }
            Filter::And(filters) => {
                let mut mask = 0u64;
                for f in filters {
                    mask |= f.extract_must_have_mask();
                }
                mask
            }
            // 对于 Or, In, Gt 等操作，我们无法提取单根必达掩码，安全退化为0（即退化到原版全扫描）
            _ => 0,
        }
    }

    // ════════ Builder 便捷方法 ════════

    pub fn eq(key: impl Into<String>, val: Value) -> Self {
        Filter::Eq(key.into(), val)
    }
    pub fn ne(key: impl Into<String>, val: Value) -> Self {
        Filter::Ne(key.into(), val)
    }
    pub fn gt(key: impl Into<String>, val: f64) -> Self {
        Filter::Gt(key.into(), val)
    }
    pub fn gte(key: impl Into<String>, val: f64) -> Self {
        Filter::Gte(key.into(), val)
    }
    pub fn lt(key: impl Into<String>, val: f64) -> Self {
        Filter::Lt(key.into(), val)
    }
    pub fn lte(key: impl Into<String>, val: f64) -> Self {
        Filter::Lte(key.into(), val)
    }
    pub fn is_in(key: impl Into<String>, vals: Vec<Value>) -> Self {
        Filter::In(key.into(), vals)
    }
    pub fn and(filters: Vec<Filter>) -> Self {
        Filter::And(filters)
    }
    pub fn or(filters: Vec<Filter>) -> Self {
        Filter::Or(filters)
    }
    pub fn exists(key: impl Into<String>, e: bool) -> Self {
        Filter::Exists(key.into(), e)
    }
    pub fn nin(key: impl Into<String>, vals: Vec<Value>) -> Self {
        Filter::Nin(key.into(), vals)
    }
    pub fn size(key: impl Into<String>, s: usize) -> Self {
        Filter::Size(key.into(), s)
    }
    pub fn all(key: impl Into<String>, vals: Vec<Value>) -> Self {
        Filter::All(key.into(), vals)
    }
    pub fn type_match(key: impl Into<String>, t: impl Into<String>) -> Self {
        Filter::TypeMatch(key.into(), t.into())
    }
    pub fn starts_with(key: impl Into<String>, prefix: impl Into<String>) -> Self {
        Filter::StartsWith(key.into(), prefix.into())
    }
    pub fn contains(key: impl Into<String>, substr: impl Into<String>) -> Self {
        Filter::Contains(key.into(), substr.into())
    }

    /// 从 JSON Value 解析为 Filter（类 MongoDB 语法）
    ///
    /// 支持的语法示例：
    /// - `{"age": {"$gt": 18}}` → `Filter::Gt("age", 18.0)`
    /// - `{"$and": [{...}, {...}]}` → `Filter::And([...])`
    /// - `{"name": "Alice"}` → `Filter::Eq("name", "Alice")`（隐式 $eq）
    pub fn from_json(val: &Value) -> Result<Self, String> {
        let obj = val
            .as_object()
            .ok_or_else(|| "过滤条件必须是 JSON 对象".to_string())?;

        let mut filters = Vec::new();

        for (key, v) in obj {
            match key.as_str() {
                "$and" => {
                    let arr = v.as_array().ok_or_else(|| "$and 必须是数组".to_string())?;
                    let sub: Result<Vec<Filter>, String> =
                        arr.iter().map(Filter::from_json).collect();
                    filters.push(Filter::And(sub?));
                }
                "$or" => {
                    let arr = v.as_array().ok_or_else(|| "$or 必须是数组".to_string())?;
                    let sub: Result<Vec<Filter>, String> =
                        arr.iter().map(Filter::from_json).collect();
                    filters.push(Filter::Or(sub?));
                }
                field => {
                    if let Some(op_obj) = v.as_object() {
                        // 运算符语法: {"field": {"$gt": 18}}
                        for (op, op_val) in op_obj {
                            let f = match op.as_str() {
                                "$eq" => Filter::Eq(field.to_string(), op_val.clone()),
                                "$ne" => Filter::Ne(field.to_string(), op_val.clone()),
                                "$gt" | "$gte" | "$lt" | "$lte" => {
                                    let operator = op.as_str();
                                    let range_op = match operator {
                                        "$gt" => RangeOp::Gt,
                                        "$gte" => RangeOp::Gte,
                                        "$lt" => RangeOp::Lt,
                                        _ => RangeOp::Lte,
                                    };
                                    Filter::Range(
                                        field.to_string(),
                                        range_op,
                                        parse_comparable(op_val, operator)?,
                                    )
                                }
                                "$before" | "$beforeEq" | "$after" | "$afterEq" => {
                                    let operator = op.as_str();
                                    let range_op = match operator {
                                        "$before" => RangeOp::Lt,
                                        "$beforeEq" => RangeOp::Lte,
                                        "$after" => RangeOp::Gt,
                                        _ => RangeOp::Gte,
                                    };
                                    let input = op_val
                                        .as_str()
                                        .ok_or_else(|| format!("{operator} 需要 RFC3339 字符串"))?;
                                    let timestamp =
                                        parse_rfc3339_nanos(input).ok_or_else(|| {
                                            format!("{operator} 包含无效 RFC3339 时间: {input}")
                                        })?;
                                    Filter::TimeRange(field.to_string(), range_op, timestamp)
                                }
                                "$in" => {
                                    let arr = op_val
                                        .as_array()
                                        .ok_or_else(|| "$in 需要数组".to_string())?;
                                    Filter::In(field.to_string(), arr.clone())
                                }
                                "$nin" => {
                                    let arr = op_val
                                        .as_array()
                                        .ok_or_else(|| "$nin 需要数组".to_string())?;
                                    Filter::Nin(field.to_string(), arr.clone())
                                }
                                "$exists" => {
                                    let b = op_val
                                        .as_bool()
                                        .ok_or_else(|| "$exists 需要布尔值".to_string())?;
                                    Filter::Exists(field.to_string(), b)
                                }
                                "$size" => {
                                    let n = op_val
                                        .as_u64()
                                        .ok_or_else(|| "$size 需要正整数".to_string())?
                                        as usize;
                                    Filter::Size(field.to_string(), n)
                                }
                                "$all" => {
                                    let arr = op_val
                                        .as_array()
                                        .ok_or_else(|| "$all 需要数组".to_string())?;
                                    Filter::All(field.to_string(), arr.clone())
                                }
                                "$type" => {
                                    let t = op_val
                                        .as_str()
                                        .ok_or_else(|| "$type 需要字符串".to_string())?;
                                    Filter::TypeMatch(field.to_string(), t.to_string())
                                }
                                "$startsWith" => {
                                    let prefix = op_val
                                        .as_str()
                                        .ok_or_else(|| "$startsWith 需要字符串".to_string())?;
                                    Filter::StartsWith(field.to_string(), prefix.to_string())
                                }
                                "$contains" => {
                                    let substr = op_val
                                        .as_str()
                                        .ok_or_else(|| "$contains 需要字符串".to_string())?;
                                    Filter::Contains(field.to_string(), substr.to_string())
                                }
                                unknown => return Err(format!("未知操作符: {}", unknown)),
                            };
                            filters.push(f);
                        }
                    } else {
                        // 隐式 $eq 语法: {"name": "Alice"}
                        filters.push(Filter::Eq(field.to_string(), v.clone()));
                    }
                }
            }
        }

        match filters.len() {
            0 => Err("过滤条件不能为空".to_string()),
            1 => Ok(filters
                .into_iter()
                .next()
                .expect("BUG: len==1 but next() returned None")),
            _ => Ok(Filter::And(filters)),
        }
    }
}

fn parse_comparable(value: &Value, operator: &str) -> Result<ComparableValue, String> {
    if let Some(number) = value.as_number() {
        return Ok(ComparableValue::Number(number.clone()));
    }
    if let Some(string) = value.as_str() {
        return Ok(ComparableValue::String(string.to_string()));
    }
    Err(format!("{operator} 需要数字或字符串"))
}

fn compare_ordered<T: PartialOrd>(value: T, threshold: T, op: RangeOp) -> bool {
    match op {
        RangeOp::Gt => value > threshold,
        RangeOp::Gte => value >= threshold,
        RangeOp::Lt => value < threshold,
        RangeOp::Lte => value <= threshold,
    }
}

fn legacy_numeric_range(value: Option<&Value>, threshold: f64, op: RangeOp) -> bool {
    let Some(Value::Number(actual)) = value else {
        return false;
    };
    if let Some(expected) = serde_json::Number::from_f64(threshold) {
        return compare_numbers(actual, &expected, op);
    }
    // 保留旧 API 的无穷边界与 NaN 语义；有限整数只在精确数值域内比较。
    actual
        .as_f64()
        .is_some_and(|value| compare_ordered(value, threshold, op))
}

/// 整数保持 i128 比较域；与浮点比较时先分离整数部分和小数部分。
pub(crate) fn numeric_ordering(
    actual: &serde_json::Number,
    expected: &serde_json::Number,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let integer = |n: &serde_json::Number| {
        n.as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from))
    };
    let int_float = |i: i128, f: f64| {
        if f >= 18_446_744_073_709_551_616. {
            return Ordering::Less;
        }
        if f < i64::MIN as f64 {
            return Ordering::Greater;
        }
        i.cmp(&(f as i128)).then_with(|| {
            if f.fract() > 0. {
                Ordering::Less
            } else if f.fract() < 0. {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
    };
    match (integer(actual), integer(expected)) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(a), None) => int_float(a, expected.as_f64().unwrap_or_default()),
        (None, Some(b)) => int_float(b, actual.as_f64().unwrap_or_default()).reverse(),
        (None, None) => {
            let a = actual.as_f64().unwrap_or_default();
            let b = expected.as_f64().unwrap_or_default();
            if a == b {
                Ordering::Equal
            } else {
                a.total_cmp(&b)
            }
        }
    }
}
fn compare_numbers(
    actual: &serde_json::Number,
    expected: &serde_json::Number,
    op: RangeOp,
) -> bool {
    let ordering = numeric_ordering(actual, expected);
    match op {
        RangeOp::Gt => ordering.is_gt(),
        RangeOp::Gte => !ordering.is_lt(),
        RangeOp::Lt => ordering.is_lt(),
        RangeOp::Lte => !ordering.is_gt(),
    }
}

fn compare_value(value: &Value, threshold: &ComparableValue, op: RangeOp) -> bool {
    match (value, threshold) {
        (Value::Number(number), ComparableValue::Number(expected)) => {
            compare_numbers(number, expected, op)
        }
        (Value::String(actual), ComparableValue::String(expected)) => {
            compare_ordered(actual.as_str(), expected.as_str(), op)
        }
        _ => false,
    }
}

fn parse_rfc3339_nanos(value: &str) -> Option<i64> {
    let parsed = chrono::DateTime::parse_from_rfc3339(value).ok()?;
    parsed.timestamp_nanos_opt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 混合数值比较覆盖完整整数域和浮点相邻边界() {
        use std::cmp::Ordering::{Equal, Less};
        for (left, right, expected) in [
            (json!(u64::MAX - 1), json!(u64::MAX), Less),
            (json!(i64::MAX), json!(i64::MAX as u64 + 1), Less),
            (
                json!(9_007_199_254_740_992u64),
                json!(9_007_199_254_740_993u64),
                Less,
            ),
            (json!(u64::MAX), json!(18_446_744_073_709_551_616f64), Less),
            (json!(i64::MIN), json!(-9_223_372_036_854_775_808f64), Equal),
            (
                json!(i64::MIN + 1),
                json!(-9_223_372_036_854_775_808f64),
                std::cmp::Ordering::Greater,
            ),
            (json!(-1), json!(-0.5), Less),
            (json!(-0.5), json!(0), Less),
            (json!(0), json!(-0.), Equal),
            (json!(0), json!(0.5), Less),
        ] {
            assert_eq!(
                numeric_ordering(left.as_number().unwrap(), right.as_number().unwrap()),
                expected
            );
            assert_eq!(
                numeric_ordering(right.as_number().unwrap(), left.as_number().unwrap()),
                expected.reverse()
            );
        }
    }

    // ═══════ $startsWith 测试 ═══════

    #[test]
    fn starts_with_basic_match() {
        let filter = Filter::starts_with("folder", "/地理");
        let payload = json!({"folder": "/地理/亚洲/中国"});
        assert!(filter.matches(&payload));
    }

    #[test]
    fn starts_with_exact_match() {
        let filter = Filter::starts_with("folder", "/地理");
        let payload = json!({"folder": "/地理"});
        assert!(filter.matches(&payload), "精确等于前缀时也应匹配");
    }

    #[test]
    fn starts_with_no_match() {
        let filter = Filter::starts_with("folder", "/地理");
        let payload = json!({"folder": "/天文/恒星"});
        assert!(!filter.matches(&payload));
    }

    #[test]
    fn starts_with_missing_field() {
        let filter = Filter::starts_with("folder", "/地理");
        let payload = json!({"name": "test"});
        assert!(!filter.matches(&payload), "字段不存在应返回 false");
    }

    #[test]
    fn starts_with_non_string_field() {
        let filter = Filter::starts_with("count", "12");
        let payload = json!({"count": 123});
        assert!(!filter.matches(&payload), "非字符串字段应返回 false");
    }

    // ═══════ $contains 测试 ═══════

    #[test]
    fn contains_basic_match() {
        let filter = Filter::contains("text", "关键词");
        let payload = json!({"text": "这是一个包含关键词的文本"});
        assert!(filter.matches(&payload));
    }

    #[test]
    fn contains_no_match() {
        let filter = Filter::contains("text", "不存在");
        let payload = json!({"text": "正常文本"});
        assert!(!filter.matches(&payload));
    }

    // ═══════ from_json 解析测试 ═══════

    #[test]
    fn from_json_starts_with() {
        let json_filter = json!({"folder": {"$startsWith": "/地理"}});
        let filter = Filter::from_json(&json_filter).unwrap();
        let payload = json!({"folder": "/地理/亚洲"});
        assert!(filter.matches(&payload));

        let payload2 = json!({"folder": "/天文"});
        assert!(!filter.matches(&payload2));
    }

    #[test]
    fn from_json_contains() {
        let json_filter = json!({"tag": {"$contains": "重要"}});
        let filter = Filter::from_json(&json_filter).unwrap();
        assert!(filter.matches(&json!({"tag": "非常重要的文档"})));
        assert!(!filter.matches(&json!({"tag": "普通文档"})));
    }

    #[test]
    fn from_json_starts_with_invalid_type() {
        let json_filter = json!({"folder": {"$startsWith": 123}});
        assert!(
            Filter::from_json(&json_filter).is_err(),
            "$startsWith 传入数字应报错"
        );
    }

    // ═══════ 组合使用测试 ═══════

    #[test]
    fn or_with_starts_with() {
        // 模拟: folder 以 /地理 或 /天文 开头
        let filter = Filter::or(vec![
            Filter::starts_with("folder", "/地理"),
            Filter::starts_with("folder", "/天文"),
        ]);
        assert!(filter.matches(&json!({"folder": "/地理/亚洲"})));
        assert!(filter.matches(&json!({"folder": "/天文/恒星"})));
        assert!(!filter.matches(&json!({"folder": "/历史/近代"})));
    }
}
