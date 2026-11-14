//! 正文、关键词及文本索引维护入口。

use super::arguments::{write_id, write_sequence, write_string};
use super::database::{PyTriviumDB, database_write};
use super::errors::{to_py_error, write_admission_error, write_input_error};
use super::json::mapping_dictionary;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

#[pymethods]
impl PyTriviumDB {
    fn index_text(
        slf: &Bound<'_, Self>,
        id: Bound<'_, PyAny>,
        text: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        let text = write_string(&text, "text")?;
        dispatch!(this, mut db => db.index_text(id, &text)).map_err(to_py_error)
    }

    fn index_keyword(
        slf: &Bound<'_, Self>,
        id: Bound<'_, PyAny>,
        keyword: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        let keyword = write_string(&keyword, "keyword")?;
        dispatch!(this, mut db => db.index_keyword(id, &keyword)).map_err(to_py_error)
    }

    fn build_text_index(slf: &Bound<'_, Self>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        dispatch!(this, mut db => db.build_text_index()).map_err(to_py_error)
    }

    /// 从完整正文与关键词集合重建文本索引，通过 flush 或 close 持久化。
    fn rebuild_text_index(slf: &Bound<'_, Self>, documents: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        // 先固定本批记录对象，字段转换回调对外层列表的修改留待下次调用处理。
        let records = write_sequence(&documents, "documents", |value, _| {
            Ok(value.clone().unbind())
        })?;
        let documents = records.iter().enumerate().map(|(index, value)| {
                let path = crate::sdk::field_member("documents", &index.to_string());
                let document = mapping_dictionary(value.bind(slf.py()), &path, Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION))?;
                for key in document.keys() {
                    let key = write_string(&key, &path)?;
                    if !matches!(key.as_str(), "id" | "text" | "keywords") {
                        return Err(write_input_error(PyValueError::new_err(
                            "文本重建记录包含未知字段 (Text rebuild record contains an unknown field)"),
                            &crate::sdk::field_member(&path, &key)));
                    }
                }
                let required = |name: &str| document.get_item(name)?.ok_or_else(|| write_input_error(
                    PyValueError::new_err("文本重建记录缺少字段 (Text rebuild record is missing a field)"),
                    &crate::sdk::field_member(&path, name)));
                Ok(crate::index::text::TextIndexDocument {
                    id: write_id(&required("id")?, &crate::sdk::field_member(&path, "id"))?,
                    text: write_string(&required("text")?, &crate::sdk::field_member(&path, "text"))?,
                    keywords: document.get_item("keywords")?.map(|value| write_sequence(&value, &crate::sdk::field_member(&path, "keywords"), write_string)).transpose()?.unwrap_or_default(),
                })
            }).collect::<PyResult<Vec<_>>>()?;
        dispatch!(this, mut db => db.rebuild_text_index(&documents)).map_err(to_py_error)
    }
}
