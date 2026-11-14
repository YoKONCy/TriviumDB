//! 属性索引、唯一约束与索引查找入口。

use super::arguments::{
    integer_argument, integer_value, read_string, write_sequence, write_string,
};
use super::database::{PyTriviumDB, database_read, database_write};
use super::errors::{argument_input_error, to_py_error, write_admission_error};
use super::json::{json_to_pyobject, read_json};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

#[pymethods]
impl PyTriviumDB {
    // ════════ 属性二级索引 ════════

    /// 创建属性索引：对指定 payload 字段建立倒排索引，加速 MATCH/FIND 查询
    ///
    /// 示例：
    /// ```python
    /// db.create_index("name")    # 之后 tql('FIND {name: "Alice"} RETURN *') 使用 O(1) 索引
    /// db.create_index("type")
    /// ```
    #[pyo3(signature = (equalities, max_results=integer_argument(10000)), text_signature="($self, equalities, max_results=10000)")]
    fn indexed_lookup(
        slf: &Bound<'_, Self>,
        equalities: &Bound<'_, PyAny>,
        max_results: Py<PyAny>,
    ) -> PyResult<Vec<u64>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let max_results = integer_value(max_results.bind(slf.py()), "max_results", None)?;
        let value = read_json(equalities, "equalities")?;
        let object = value.as_object().ok_or_else(|| {
            argument_input_error(
                PyValueError::new_err("equalities 必须是对象 (equalities must be an object)"),
                "equalities",
                None,
            )
        })?;
        let equalities = object
            .iter()
            .map(|(field, value)| (field.clone(), value.clone()))
            .collect::<Vec<_>>();
        dispatch!(this, db => db.indexed_lookup(&equalities, max_results)).map_err(to_py_error)
    }

    fn create_ngram_index(slf: &Bound<'_, Self>, field: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let field = write_string(&field, "field")?;
        dispatch!(this, mut db => db.create_ngram_index(&field)).map_err(to_py_error)
    }

    fn drop_ngram_index(slf: &Bound<'_, Self>, field: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let field = write_string(&field, "field")?;
        dispatch!(this, mut db => db.drop_ngram_index(&field)).map_err(to_py_error)
    }

    #[pyo3(signature = (field, needle, max_results=integer_argument(10000)), text_signature="($self, field, needle, max_results=10000)")]
    fn substring_lookup(
        slf: &Bound<'_, Self>,
        field: Bound<'_, PyAny>,
        needle: Bound<'_, PyAny>,
        max_results: Py<PyAny>,
    ) -> PyResult<Vec<u64>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let field = read_string(&field, "field")?;
        let needle = read_string(&needle, "needle")?;
        let max_results = integer_value(max_results.bind(slf.py()), "max_results", None)?;
        dispatch!(this, db => db.substring_lookup(&field, &needle, max_results))
            .map_err(to_py_error)
    }

    fn create_unique_index(slf: &Bound<'_, Self>, field: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let field = write_string(&field, "field")?;
        dispatch!(this, mut db => db.create_unique_index(&field)).map_err(to_py_error)
    }

    fn create_unique_composite_index(
        slf: &Bound<'_, Self>,
        fields: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let fields = write_sequence(&fields, "fields", write_string)?;
        dispatch!(this, mut db => db.create_unique_composite_index(&fields)).map_err(to_py_error)
    }

    fn list_unique_indexes(slf: &Bound<'_, Self>) -> PyResult<Vec<Vec<String>>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        Ok(dispatch!(this, db => db.list_unique_indexes().map_err(to_py_error)?))
    }

    fn create_index(slf: &Bound<'_, Self>, field: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let field = write_string(&field, "field")?;
        dispatch!(this, mut db => db.create_index(&field)).map_err(to_py_error)
    }

    fn create_ordered_index(slf: &Bound<'_, Self>, field: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let field = write_string(&field, "field")?;
        dispatch!(this, mut db => db.create_ordered_index(&field)).map_err(to_py_error)
    }

    fn create_composite_index(slf: &Bound<'_, Self>, fields: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let fields = write_sequence(&fields, "fields", write_string)?;
        dispatch!(this, mut db => db.create_composite_index(&fields)).map_err(to_py_error)
    }

    fn create_bitmap_index(slf: &Bound<'_, Self>, field: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let field = write_string(&field, "field")?;
        dispatch!(this, mut db => db.create_bitmap_index(&field)).map_err(to_py_error)
    }

    /// 删除属性索引（查询仍可用，退化为全扫描）
    fn drop_index(slf: &Bound<'_, Self>, field: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let field = write_string(&field, "field")?;
        dispatch!(this, mut db => db.drop_index(&field)).map_err(to_py_error)
    }

    fn drop_ordered_index(slf: &Bound<'_, Self>, field: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let field = write_string(&field, "field")?;
        dispatch!(this, mut db => db.drop_ordered_index(&field)).map_err(to_py_error)
    }

    fn drop_composite_index(slf: &Bound<'_, Self>, fields: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let fields = write_sequence(&fields, "fields", write_string)?;
        dispatch!(this, mut db => db.drop_composite_index(&fields)).map_err(to_py_error)
    }

    fn drop_bitmap_index(slf: &Bound<'_, Self>, field: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let field = write_string(&field, "field")?;
        dispatch!(this, mut db => db.drop_bitmap_index(&field)).map_err(to_py_error)
    }

    fn index_info(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let value =
            dispatch!(this, db => serde_json::to_value(db.index_info().map_err(to_py_error)?))
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        json_to_pyobject(py, &value)
    }
}
