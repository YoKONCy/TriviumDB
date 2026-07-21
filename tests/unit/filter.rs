//! 从 src/filter.rs 分离的单元测试 + 补齐所有未覆盖的 Filter 变体
//!
//! 覆盖范围: Filter 的 14 种变体 + from_json 解析 + extract_must_have_mask 布隆掩码

use serde_json::json;
use triviumdb::Filter;

// ════════════════════════════════════════════════════════════════
//  Filter::matches — 14 种变体点到点
// ════════════════════════════════════════════════════════════════

#[test]
fn filter_eq_精确匹配() {
    let f = Filter::eq("role", json!("admin"));
    assert!(f.matches(&json!({"role": "admin"})));
    assert!(!f.matches(&json!({"role": "user"})));
    assert!(!f.matches(&json!({"name": "admin"})));
    assert!(!f.matches(&json!({})));
}

#[test]
fn filter_ne_不等于() {
    let f = Filter::ne("role", json!("admin"));
    assert!(f.matches(&json!({"role": "user"})));
    assert!(!f.matches(&json!({"role": "admin"})));
    assert!(f.matches(&json!({})));
}

#[test]
fn filter_gt_大于() {
    let f = Filter::gt("age", 18.0);
    assert!(f.matches(&json!({"age": 25})));
    assert!(!f.matches(&json!({"age": 18})));
    assert!(!f.matches(&json!({"age": 10})));
    assert!(!f.matches(&json!({"age": "not_a_number"})));
    assert!(!f.matches(&json!({})));
}

#[test]
fn filter_gte_大于等于() {
    let f = Filter::gte("score", 0.5);
    assert!(f.matches(&json!({"score": 0.5})));
    assert!(f.matches(&json!({"score": 0.9})));
    assert!(!f.matches(&json!({"score": 0.4})));
}

#[test]
fn filter_lt_小于() {
    let f = Filter::lt("price", 100.0);
    assert!(f.matches(&json!({"price": 50})));
    assert!(!f.matches(&json!({"price": 100})));
    assert!(!f.matches(&json!({"price": 200})));
}

#[test]
fn filter_lte_小于等于() {
    let f = Filter::lte("count", 10.0);
    assert!(f.matches(&json!({"count": 10})));
    assert!(f.matches(&json!({"count": 5})));
    assert!(!f.matches(&json!({"count": 11})));
}

#[test]
fn filter_in_值在集合中() {
    let f = Filter::is_in("color", vec![json!("red"), json!("blue")]);
    assert!(f.matches(&json!({"color": "red"})));
    assert!(f.matches(&json!({"color": "blue"})));
    assert!(!f.matches(&json!({"color": "green"})));
    assert!(!f.matches(&json!({})));
}

#[test]
fn filter_nin_值不在集合中() {
    let f = Filter::nin("status", vec![json!("deleted"), json!("banned")]);
    assert!(f.matches(&json!({"status": "active"})));
    assert!(!f.matches(&json!({"status": "deleted"})));
    assert!(f.matches(&json!({})));
}

#[test]
fn filter_exists_字段存在性() {
    let f_exists = Filter::exists("email", true);
    assert!(f_exists.matches(&json!({"email": "a@b.com"})));
    assert!(!f_exists.matches(&json!({"name": "test"})));

    let f_not_exists = Filter::exists("email", false);
    assert!(!f_not_exists.matches(&json!({"email": "a@b.com"})));
    assert!(f_not_exists.matches(&json!({"name": "test"})));
}

#[test]
fn filter_size_数组长度() {
    let f = Filter::size("tags", 3);
    assert!(f.matches(&json!({"tags": [1, 2, 3]})));
    assert!(!f.matches(&json!({"tags": [1, 2]})));
    assert!(!f.matches(&json!({"tags": "not_array"})));
    assert!(!f.matches(&json!({})));
}

#[test]
fn filter_all_数组包含所有() {
    let f = Filter::all("skills", vec![json!("rust"), json!("python")]);
    assert!(f.matches(&json!({"skills": ["rust", "python", "go"]})));
    assert!(!f.matches(&json!({"skills": ["rust", "go"]})));
    assert!(!f.matches(&json!({"skills": "not_array"})));
}

#[test]
fn filter_type_match_类型匹配() {
    let f = Filter::type_match("value", "string");
    assert!(f.matches(&json!({"value": "hello"})));
    assert!(!f.matches(&json!({"value": 42})));

    let f_num = Filter::type_match("value", "number");
    assert!(f_num.matches(&json!({"value": 42})));
    assert!(!f_num.matches(&json!({"value": "42"})));

    let f_bool = Filter::type_match("value", "boolean");
    assert!(f_bool.matches(&json!({"value": true})));

    let f_null = Filter::type_match("value", "null");
    assert!(f_null.matches(&json!({"value": null})));

    let f_arr = Filter::type_match("value", "array");
    assert!(f_arr.matches(&json!({"value": [1, 2]})));

    let f_obj = Filter::type_match("value", "object");
    assert!(f_obj.matches(&json!({"value": {"a": 1}})));
}

#[test]
fn filter_and_逻辑与() {
    let f = Filter::and(vec![
        Filter::gt("age", 18.0),
        Filter::eq("role", json!("admin")),
    ]);
    assert!(f.matches(&json!({"age": 25, "role": "admin"})));
    assert!(!f.matches(&json!({"age": 25, "role": "user"})));
    assert!(!f.matches(&json!({"age": 15, "role": "admin"})));
}

#[test]
fn filter_or_逻辑或() {
    let f = Filter::or(vec![
        Filter::eq("status", json!("active")),
        Filter::eq("status", json!("pending")),
    ]);
    assert!(f.matches(&json!({"status": "active"})));
    assert!(f.matches(&json!({"status": "pending"})));
    assert!(!f.matches(&json!({"status": "deleted"})));
}

// ════════════════════════════════════════════════════════════════
//  Filter::from_json — MongoDB 语法解析
// ════════════════════════════════════════════════════════════════

#[test]
fn from_json_隐式eq() {
    let f = Filter::from_json(&json!({"name": "Alice"})).unwrap();
    assert!(f.matches(&json!({"name": "Alice"})));
    assert!(!f.matches(&json!({"name": "Bob"})));
}

#[test]
fn from_json_操作符语法() {
    let f = Filter::from_json(&json!({"age": {"$gt": 18}})).unwrap();
    assert!(f.matches(&json!({"age": 25})));
    assert!(!f.matches(&json!({"age": 15})));
}

#[test]
fn from_json_and组合() {
    let f = Filter::from_json(&json!({
        "$and": [
            {"age": {"$gte": 18}},
            {"role": "admin"}
        ]
    }))
    .unwrap();
    assert!(f.matches(&json!({"age": 20, "role": "admin"})));
    assert!(!f.matches(&json!({"age": 20, "role": "user"})));
}

#[test]
fn from_json_or组合() {
    let f = Filter::from_json(&json!({
        "$or": [
            {"status": "active"},
            {"status": "pending"}
        ]
    }))
    .unwrap();
    assert!(f.matches(&json!({"status": "active"})));
    assert!(!f.matches(&json!({"status": "deleted"})));
}

#[test]
fn from_json_所有操作符() {
    let f = Filter::from_json(&json!({"color": {"$in": ["red", "blue"]}})).unwrap();
    assert!(f.matches(&json!({"color": "red"})));

    let f = Filter::from_json(&json!({"color": {"$nin": ["red"]}})).unwrap();
    assert!(f.matches(&json!({"color": "blue"})));

    let f = Filter::from_json(&json!({"email": {"$exists": true}})).unwrap();
    assert!(f.matches(&json!({"email": "test@test.com"})));

    let f = Filter::from_json(&json!({"tags": {"$size": 2}})).unwrap();
    assert!(f.matches(&json!({"tags": [1, 2]})));

    let f = Filter::from_json(&json!({"tags": {"$all": [1, 2]}})).unwrap();
    assert!(f.matches(&json!({"tags": [1, 2, 3]})));

    let f = Filter::from_json(&json!({"value": {"$type": "string"}})).unwrap();
    assert!(f.matches(&json!({"value": "hello"})));

    let f = Filter::from_json(&json!({"x": {"$ne": 5}})).unwrap();
    assert!(f.matches(&json!({"x": 6})));
    let f = Filter::from_json(&json!({"x": {"$lt": 10}})).unwrap();
    assert!(f.matches(&json!({"x": 5})));
    let f = Filter::from_json(&json!({"x": {"$lte": 10}})).unwrap();
    assert!(f.matches(&json!({"x": 10})));
}

#[test]
fn from_json_未知操作符应报错() {
    let result = Filter::from_json(&json!({"x": {"$unknown": 1}}));
    assert!(result.is_err());
}

#[test]
fn from_json_空对象应报错() {
    let result = Filter::from_json(&json!({}));
    assert!(result.is_err());
}

#[test]
fn from_json_非对象应报错() {
    let result = Filter::from_json(&json!("not an object"));
    assert!(result.is_err());
}

// ════════════════════════════════════════════════════════════════
//  Filter::extract_must_have_mask — 布隆过滤掩码
// ════════════════════════════════════════════════════════════════

#[test]
fn bloom_mask_eq产生非零掩码() {
    let f = Filter::eq("role", json!("admin"));
    let mask = f.extract_must_have_mask();
    assert_ne!(mask, 0, "Eq 过滤应生成非零布隆掩码");
    assert_eq!(mask.count_ones(), 1);
}

#[test]
fn bloom_mask_eq仅为安全标量生成掩码() {
    assert_ne!(
        Filter::eq("enabled", json!(true)).extract_must_have_mask(),
        0
    );
    assert_ne!(Filter::eq("count", json!(3)).extract_must_have_mask(), 0);

    assert_eq!(
        Filter::eq("items", json!([1, 2])).extract_must_have_mask(),
        0
    );
    assert_eq!(
        Filter::eq("meta", json!({"kind": "x"})).extract_must_have_mask(),
        0
    );
    assert_eq!(
        Filter::eq("missing", json!(null)).extract_must_have_mask(),
        0
    );
}

#[test]
fn bloom_mask_浮点正负零使用相同规范表示() {
    let positive_zero = Filter::eq("zero", json!(0.0)).extract_must_have_mask();
    let negative_zero = Filter::eq("zero", json!(-0.0)).extract_must_have_mask();

    assert_ne!(positive_zero, 0);
    assert_eq!(positive_zero, negative_zero);
}

#[test]
fn bloom_mask_and合并多个掩码() {
    let f = Filter::and(vec![
        Filter::eq("role", json!("admin")),
        Filter::eq("dept", json!("eng")),
    ]);
    let mask = f.extract_must_have_mask();
    assert!(mask.count_ones() >= 1);
}

#[test]
fn bloom_mask_or返回零() {
    let f = Filter::or(vec![Filter::eq("a", json!(1)), Filter::eq("b", json!(2))]);
    let mask = f.extract_must_have_mask();
    assert_eq!(mask, 0, "Or 条件无法提取必达掩码");
}

#[test]
fn bloom_mask_gt等返回零() {
    assert_eq!(Filter::gt("x", 1.0).extract_must_have_mask(), 0);
    assert_eq!(Filter::lt("x", 1.0).extract_must_have_mask(), 0);
    assert_eq!(
        Filter::is_in("x", vec![json!(1)]).extract_must_have_mask(),
        0
    );
}

// ════════════════════════════════════════════════════════════════
//  from_json 错误路径分支覆盖
// ════════════════════════════════════════════════════════════════

#[test]
fn from_json_gt_非数字报错() {
    let r = Filter::from_json(&json!({"age": {"$gt": "not_a_number"}}));
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("$gt"));
}

#[test]
fn from_json_gte_非数字报错() {
    let r = Filter::from_json(&json!({"age": {"$gte": "x"}}));
    assert!(r.is_err());
}

#[test]
fn from_json_lt_非数字报错() {
    let r = Filter::from_json(&json!({"age": {"$lt": true}}));
    assert!(r.is_err());
}

#[test]
fn from_json_lte_非数字报错() {
    let r = Filter::from_json(&json!({"age": {"$lte": [1]}}));
    assert!(r.is_err());
}

#[test]
fn from_json_in_非数组报错() {
    let r = Filter::from_json(&json!({"x": {"$in": "not_array"}}));
    assert!(r.is_err());
}

#[test]
fn from_json_nin_非数组报错() {
    let r = Filter::from_json(&json!({"x": {"$nin": 42}}));
    assert!(r.is_err());
}

#[test]
fn from_json_exists_非布尔报错() {
    let r = Filter::from_json(&json!({"x": {"$exists": 1}}));
    assert!(r.is_err());
}

#[test]
fn from_json_size_非正整数报错() {
    let r = Filter::from_json(&json!({"x": {"$size": "big"}}));
    assert!(r.is_err());
}

#[test]
fn from_json_all_非数组报错() {
    let r = Filter::from_json(&json!({"x": {"$all": "nope"}}));
    assert!(r.is_err());
}

#[test]
fn from_json_type_非字符串报错() {
    let r = Filter::from_json(&json!({"x": {"$type": 42}}));
    assert!(r.is_err());
}

#[test]
fn from_json_and_非数组报错() {
    let r = Filter::from_json(&json!({"$and": "not_array"}));
    assert!(r.is_err());
}

#[test]
fn from_json_or_非数组报错() {
    let r = Filter::from_json(&json!({"$or": 123}));
    assert!(r.is_err());
}

#[test]
fn from_json_显式eq() {
    let f = Filter::from_json(&json!({"name": {"$eq": "Alice"}})).unwrap();
    assert!(f.matches(&json!({"name": "Alice"})));
    assert!(!f.matches(&json!({"name": "Bob"})));
}

#[test]
fn from_json_多字段隐式AND() {
    // 多个字段同时匹配应产生 And 组合
    let f = Filter::from_json(&json!({"name": "Alice", "age": {"$gt": 20}})).unwrap();
    assert!(f.matches(&json!({"name": "Alice", "age": 25})));
    assert!(!f.matches(&json!({"name": "Alice", "age": 15})));
    assert!(!f.matches(&json!({"name": "Bob", "age": 25})));
}

#[test]
fn from_json_嵌套and_or() {
    let f = Filter::from_json(&json!({
        "$and": [
            {"$or": [{"x": 1}, {"x": 2}]},
            {"y": {"$gt": 0}}
        ]
    }))
    .unwrap();
    assert!(f.matches(&json!({"x": 1, "y": 5})));
    assert!(f.matches(&json!({"x": 2, "y": 1})));
    assert!(!f.matches(&json!({"x": 3, "y": 5})));
    assert!(!f.matches(&json!({"x": 1, "y": -1})));
}

// ════════════════════════════════════════════════════════════════
//  matches 边界补充
// ════════════════════════════════════════════════════════════════

#[test]
fn filter_type_match_字段不存在() {
    let f = Filter::type_match("missing", "string");
    assert!(!f.matches(&json!({})));
}

#[test]
fn bloom_mask_其他变体返回零() {
    assert_eq!(Filter::exists("x", true).extract_must_have_mask(), 0);
    assert_eq!(Filter::nin("x", vec![json!(1)]).extract_must_have_mask(), 0);
    assert_eq!(Filter::size("x", 3).extract_must_have_mask(), 0);
    assert_eq!(Filter::all("x", vec![json!(1)]).extract_must_have_mask(), 0);
    assert_eq!(
        Filter::type_match("x", "string").extract_must_have_mask(),
        0
    );
    assert_eq!(Filter::ne("x", json!(1)).extract_must_have_mask(), 0);
    assert_eq!(Filter::gte("x", 1.0).extract_must_have_mask(), 0);
    assert_eq!(Filter::lte("x", 1.0).extract_must_have_mask(), 0);
}

#[test]
fn bloom_mask_eq_值为非字符串() {
    // extract_must_have_mask 中 val_str 分支: Value::String vs other
    let f1 = Filter::eq("x", json!("hello")); // String 分支
    let f2 = Filter::eq("x", json!(42)); // non-String 分支 (to_string)
    assert_ne!(f1.extract_must_have_mask(), 0);
    assert_ne!(f2.extract_must_have_mask(), 0);
}
