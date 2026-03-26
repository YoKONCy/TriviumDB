use serde_json::Value;

/// 过滤条件表达式
/// 支持: $eq, $ne, $gt, $gte, $lt, $lte, $in, $and, $or
#[derive(Debug, Clone)]
pub enum Filter {
    /// 精确匹配: {"field": {"$eq": value}}
    Eq(String, Value),
    /// 不等于
    Ne(String, Value),
    /// 大于 (仅数字)
    Gt(String, f64),
    /// 大于等于
    Gte(String, f64),
    /// 小于
    Lt(String, f64),
    /// 小于等于
    Lte(String, f64),
    /// 值在集合中: {"field": {"$in": [v1, v2]}}
    In(String, Vec<Value>),
    /// 逻辑与
    And(Vec<Filter>),
    /// 逻辑或
    Or(Vec<Filter>),
}

impl Filter {
    /// 检查一个 JSON payload 是否满足该过滤条件
    pub fn matches(&self, payload: &Value) -> bool {
        match self {
            Filter::Eq(key, val) => payload.get(key) == Some(val),

            Filter::Ne(key, val) => payload.get(key) != Some(val),

            Filter::Gt(key, threshold) => {
                extract_number(payload, key).map_or(false, |v| v > *threshold)
            }
            Filter::Gte(key, threshold) => {
                extract_number(payload, key).map_or(false, |v| v >= *threshold)
            }
            Filter::Lt(key, threshold) => {
                extract_number(payload, key).map_or(false, |v| v < *threshold)
            }
            Filter::Lte(key, threshold) => {
                extract_number(payload, key).map_or(false, |v| v <= *threshold)
            }

            Filter::In(key, values) => {
                if let Some(field_val) = payload.get(key) {
                    values.contains(field_val)
                } else {
                    false
                }
            }

            Filter::And(filters) => filters.iter().all(|f| f.matches(payload)),
            Filter::Or(filters) => filters.iter().any(|f| f.matches(payload)),
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
}

fn extract_number(payload: &Value, key: &str) -> Option<f64> {
    payload.get(key)?.as_f64()
}
