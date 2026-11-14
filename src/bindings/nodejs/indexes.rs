//! 属性索引、唯一约束与索引查找入口。

use super::arguments::{
    node_ids_to_js, parse_search_usize, read_json, read_string, write_array, write_string,
};
use super::database::TriviumDB;
use super::errors::{argument_input_error, to_napi_error};
use super::json::{NodeJson, node_json_value};
use napi::bindgen_prelude::Either;
use napi::{Env, JsUnknown};
use napi_derive::napi;

#[napi]
impl TriviumDB {
    // ── 属性二级索引 ──

    /// 创建属性索引：对指定 payload 字段建立倒排索引
    ///
    /// ```js
    /// db.createIndex('name')   // 之后 tql('FIND {name: "Alice"} RETURN *') 使用 O(1) 索引
    /// ```
    #[napi]
    pub fn indexed_lookup(
        &self,
        env: Env,
        equalities: JsUnknown,
        max_results: Option<Either<f64, JsUnknown>>,
    ) -> napi::Result<Vec<f64>> {
        let value = read_json(env, equalities, "equalities")?;
        let object = value.as_object().ok_or_else(|| {
            argument_input_error(
                env,
                "equalities",
                "equalities 必须是对象 (equalities must be an object)",
                None,
            )
        })?;
        let equalities = object
            .iter()
            .map(|(field, value)| (field.clone(), value.clone()))
            .collect::<Vec<_>>();
        let max_results = parse_search_usize(env, max_results, 10_000.0, "maxResults", false)?;
        dispatch!(self, db => db.indexed_lookup(&equalities, max_results))
            .map_err(|error| to_napi_error(env, error))
            .and_then(|value| node_ids_to_js(env, value))
    }

    #[napi]
    pub fn create_ngram_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
        let field = write_string(env, field, "field")?;
        dispatch!(self, mut db => db.create_ngram_index(&field))
            .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn drop_ngram_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
        let field = write_string(env, field, "field")?;
        dispatch!(self, mut db => db.drop_ngram_index(&field))
            .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn substring_lookup(
        &self,
        env: Env,
        field: JsUnknown,
        needle: JsUnknown,
        max_results: Option<Either<f64, JsUnknown>>,
    ) -> napi::Result<Vec<f64>> {
        let field = read_string(env, field, "field")?;
        let needle = read_string(env, needle, "needle")?;
        let max_results = parse_search_usize(env, max_results, 10_000.0, "maxResults", false)?;
        dispatch!(self, db => db.substring_lookup(&field, &needle, max_results))
            .map_err(|error| to_napi_error(env, error))
            .and_then(|value| node_ids_to_js(env, value))
    }

    #[napi]
    pub fn create_unique_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
        let field = write_string(env, field, "field")?;
        dispatch!(self, mut db => db.create_unique_index(&field))
            .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn create_unique_composite_index(
        &mut self,
        env: Env,
        fields: JsUnknown,
    ) -> napi::Result<()> {
        let fields = write_array(env, fields, "fields", write_string)?;
        dispatch!(self, mut db => db.create_unique_composite_index(&fields))
            .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn list_unique_indexes(&self, env: Env) -> napi::Result<Vec<Vec<String>>> {
        Ok(
            dispatch!(self, db => db.list_unique_indexes().map_err(|error| to_napi_error(env, error))?),
        )
    }

    #[napi]
    pub fn create_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
        let field = write_string(env, field, "field")?;
        dispatch!(self, mut db => db.create_index(&field))
            .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn create_ordered_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
        let field = write_string(env, field, "field")?;
        dispatch!(self, mut db => db.create_ordered_index(&field))
            .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn create_composite_index(&mut self, env: Env, fields: JsUnknown) -> napi::Result<()> {
        let fields = write_array(env, fields, "fields", write_string)?;
        dispatch!(self, mut db => db.create_composite_index(&fields))
            .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn create_bitmap_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
        let field = write_string(env, field, "field")?;
        dispatch!(self, mut db => db.create_bitmap_index(&field))
            .map_err(|error| to_napi_error(env, error))
    }

    /// 删除属性索引（查询仍可用，退化为全扫描）
    #[napi]
    pub fn drop_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
        let field = write_string(env, field, "field")?;
        dispatch!(self, mut db => db.drop_index(&field)).map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn drop_ordered_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
        let field = write_string(env, field, "field")?;
        dispatch!(self, mut db => db.drop_ordered_index(&field))
            .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn drop_composite_index(&mut self, env: Env, fields: JsUnknown) -> napi::Result<()> {
        let fields = write_array(env, fields, "fields", write_string)?;
        dispatch!(self, mut db => db.drop_composite_index(&fields))
            .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn drop_bitmap_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
        let field = write_string(env, field, "field")?;
        dispatch!(self, mut db => db.drop_bitmap_index(&field))
            .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn index_info(&self, env: Env) -> napi::Result<NodeJson> {
        dispatch!(self, db => node_json_value(db.index_info().map_err(|error| to_napi_error(env, error))?))
                .map_err(|error| napi::Error::from_reason(error.to_string()))
    }
}
