//! 正文、关键词及文本索引维护入口。

use super::arguments::{argument_object, write_array, write_id, write_string};
use super::database::TriviumDB;
use super::errors::{argument_input_error, to_napi_error};
use napi::{Env, JsUnknown};
use napi_derive::napi;

#[napi]
impl TriviumDB {
    // ── 文本索引 ──

    /// 对节点建立用于双路召回的长文本 BM25 索引
    #[napi]
    pub fn index_text(&mut self, env: Env, id: JsUnknown, text: JsUnknown) -> napi::Result<()> {
        let id = write_id(env, id, "id")?;
        let text = write_string(env, text, "text")?;
        dispatch!(self, mut db => db.index_text(id, &text))
            .map_err(|error| to_napi_error(env, error))
    }

    /// 对节点建立用于精确命中的 AC自动机 高级关键词索引
    #[napi]
    pub fn index_keyword(
        &mut self,
        env: Env,
        id: JsUnknown,
        keyword: JsUnknown,
    ) -> napi::Result<()> {
        let id = write_id(env, id, "id")?;
        let keyword = write_string(env, keyword, "keyword")?;
        dispatch!(self, mut db => db.index_keyword(id, &keyword))
            .map_err(|error| to_napi_error(env, error))
    }

    /// 预热关键词自动机；损坏或缺失的延期索引会返回错误。
    #[napi]
    pub fn build_text_index(&mut self, env: Env) -> napi::Result<()> {
        dispatch!(self, mut db => db.build_text_index()).map_err(|error| to_napi_error(env, error))
    }

    /// 从完整正文与关键词集合重建文本索引，通过 flush 或 close 持久化。
    #[napi]
    pub fn rebuild_text_index(&mut self, env: Env, documents: JsUnknown) -> napi::Result<()> {
        let documents = write_array(env, documents, "documents", |env, value, path| {
            let write = Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION);
            let document = argument_object(env, value, path, write)?;
            let keys = document.get_all_property_names(
                napi::KeyCollectionMode::OwnOnly,
                napi::KeyFilter::Enumerable,
                napi::KeyConversion::NumbersToStrings,
            )?;
            for index in 0..keys.get_array_length()? {
                let key = write_string(env, keys.get_element(index)?, path)?;
                if !matches!(key.as_str(), "id" | "text" | "keywords") {
                    return Err(argument_input_error(
                        env,
                        &crate::sdk::field_member(path, &key),
                        "文本重建记录包含未知字段 (Text rebuild record contains an unknown field)",
                        write,
                    ));
                }
            }
            let required = |name: &str| {
                document.get(name)?.ok_or_else(|| {
                    argument_input_error(
                        env,
                        &crate::sdk::field_member(path, name),
                        "文本重建记录缺少字段 (Text rebuild record is missing a field)",
                        write,
                    )
                })
            };
            Ok(crate::index::text::TextIndexDocument {
                id: write_id(env, required("id")?, &crate::sdk::field_member(path, "id"))?,
                text: write_string(
                    env,
                    required("text")?,
                    &crate::sdk::field_member(path, "text"),
                )?,
                keywords: document
                    .get("keywords")?
                    .map(|value| {
                        write_array(
                            env,
                            value,
                            &crate::sdk::field_member(path, "keywords"),
                            write_string,
                        )
                    })
                    .transpose()?
                    .unwrap_or_default(),
            })
        })?;
        dispatch!(self, mut db => db.rebuild_text_index(&documents))
            .map_err(|error| to_napi_error(env, error))
    }
}
