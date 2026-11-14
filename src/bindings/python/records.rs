//! 节点读取、批量写入与 CRUD 参数转换。

use super::arguments::{
    extract_argument, write_float, write_id, write_list, write_sequence, write_string,
};
use super::database::{DbBackend, PyTriviumDB, database_read, database_write};
use super::errors::{to_py_error, write_admission_error, write_input_error};
use super::json::{json_to_pyobject, write_json};
use super::types::{PyEdge, PyNodeView, round_api_f32};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

#[pymethods]
impl PyTriviumDB {
    fn insert(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        vector: Bound<'_, PyAny>,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<u64> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let json = write_json(payload, "payload")?;
        match &mut this.inner {
            DbBackend::F32(db) => {
                let vec: Vec<f32> = write_sequence(&vector, "vector", write_float)?;
                db.insert(&vec, json).map_err(to_py_error)
            }
            DbBackend::F16(db) => {
                let vec: Vec<f32> = write_sequence(&vector, "vector", write_float)?;
                let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                db.insert(&vec16, json).map_err(to_py_error)
            }
            DbBackend::U64(db) => {
                let vec: Vec<u64> = write_sequence(&vector, "vector", write_id)?;
                db.insert(&vec, json).map_err(to_py_error)
            }
        }
    }

    fn insert_with_id(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        id: Bound<'_, PyAny>,
        vector: Bound<'_, PyAny>,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        let json = write_json(payload, "payload")?;
        match &mut this.inner {
            DbBackend::F32(db) => {
                let vec: Vec<f32> = write_sequence(&vector, "vector", write_float)?;
                db.insert_with_id(id, &vec, json)
            }
            DbBackend::F16(db) => {
                let vec: Vec<f32> = write_sequence(&vector, "vector", write_float)?;
                let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                db.insert_with_id(id, &vec16, json)
            }
            DbBackend::U64(db) => {
                let vec: Vec<u64> = write_sequence(&vector, "vector", write_id)?;
                db.insert_with_id(id, &vec, json)
            }
        }
        .map_err(to_py_error)
    }

    fn upsert_with_id(
        slf: &Bound<'_, Self>,
        id: Bound<'_, PyAny>,
        vector: Bound<'_, PyAny>,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        let json = write_json(payload, "payload")?;
        match &mut this.inner {
            DbBackend::F32(db) => {
                db.upsert_with_id(id, &write_sequence(&vector, "vector", write_float)?, json)
            }
            DbBackend::F16(db) => {
                let vector = write_sequence(&vector, "vector", write_float)?
                    .into_iter()
                    .map(half::f16::from_f32)
                    .collect::<Vec<_>>();
                db.upsert_with_id(id, &vector, json)
            }
            DbBackend::U64(db) => {
                db.upsert_with_id(id, &write_sequence(&vector, "vector", write_id)?, json)
            }
        }
        .map_err(to_py_error)
    }

    fn delete_many_atomic(slf: &Bound<'_, Self>, ids: Bound<'_, PyAny>) -> PyResult<usize> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let ids = write_sequence(&ids, "ids", write_id)?;
        dispatch!(this, mut db => db.delete_many_atomic(&ids)).map_err(to_py_error)
    }

    fn compare_and_set_payload_field(
        slf: &Bound<'_, Self>,
        id: Bound<'_, PyAny>,
        field: Bound<'_, PyAny>,
        expected: &Bound<'_, PyAny>,
        replacement: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        let field: String = write_string(&field, "field")?;
        let expected = write_json(expected, "expected")?;
        let replacement = write_json(replacement, "replacement")?;
        dispatch!(this, mut db => db.compare_and_set_payload_field(id, &field, &expected, replacement))
                .map_err(to_py_error)
    }

    fn delete(slf: &Bound<'_, Self>, id: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        dispatch!(this, mut db => db.delete(id)).map_err(to_py_error)
    }

    fn update_payload(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        id: Bound<'_, PyAny>,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        let json = write_json(payload, "payload")?;
        dispatch!(this, mut db => db.update_payload(id, json)).map_err(to_py_error)
    }

    /// 部分更新节点 Payload（$set / $inc / $unset）
    ///
    /// 只修改指定字段，其他字段保持不变。
    ///
    /// 示例：
    /// ```python
    /// db.patch_payload(id, {"$set": {"name": "Alice"}})
    /// db.patch_payload(id, {"$inc": {"visits": 1}})
    /// db.patch_payload(id, {"$unset": {"old_field": True}})
    /// ```
    fn patch_payload(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        id: Bound<'_, PyAny>,
        patch: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        let json = write_json(patch, "patch")?;
        dispatch!(this, mut db => db.patch_payload(id, json)).map_err(to_py_error)
    }

    fn update_vector(
        slf: &Bound<'_, Self>,
        vector: Bound<'_, PyAny>,
        id: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let id = write_id(&id, "id")?;
        match &mut this.inner {
            DbBackend::F32(db) => {
                let vec: Vec<f32> = write_sequence(&vector, "vector", write_float)?;
                db.update_vector(id, &vec).map_err(to_py_error)
            }
            DbBackend::F16(db) => {
                let vec: Vec<f32> = write_sequence(&vector, "vector", write_float)?;
                let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                db.update_vector(id, &vec16).map_err(to_py_error)
            }
            DbBackend::U64(db) => {
                let vec: Vec<u64> = write_sequence(&vector, "vector", write_id)?;
                db.update_vector(id, &vec).map_err(to_py_error)
            }
        }
    }

    // ════════ 轻量级单字段查询 ════════

    /// 获取节点的 payload（不含向量，比 get() 更轻量）
    fn get_payload(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        id: Bound<'_, PyAny>,
    ) -> PyResult<Option<PyObject>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let id = extract_argument(py, &id, "id", None)?;
        Ok({
            dispatch!(this, db => db.get_payload(id).map_err(to_py_error)?)
                .map(|p| json_to_pyobject(py, &p))
                .transpose()?
        })
    }

    fn get(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        id: Bound<'_, PyAny>,
    ) -> PyResult<Option<PyNodeView>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let id = extract_argument(py, &id, "id", None)?;
        match &this.inner {
            DbBackend::F32(db) => {
                if let Some(n) = db.get(id).map_err(to_py_error)? {
                    return Ok(Some(PyNodeView {
                        id: n.id,
                        vector: n.vector.into_pyobject(py).unwrap().into_any().unbind(),
                        payload: json_to_pyobject(py, &n.payload)?,
                        edges: n
                            .edges
                            .iter()
                            .map(|e| {
                                Ok(PyEdge {
                                    target_id: e.target_id,
                                    label: e.label.clone(),
                                    weight: round_api_f32(e.weight),
                                    metadata: json_to_pyobject(py, &e.metadata)?,
                                })
                            })
                            .collect::<PyResult<Vec<_>>>()?,
                        num_edges: n.edges.len(),
                    }));
                }
            }
            DbBackend::F16(db) => {
                if let Some(n) = db.get(id).map_err(to_py_error)? {
                    let f32_vec: Vec<f32> = n.vector.into_iter().map(|f| f.to_f32()).collect();
                    return Ok(Some(PyNodeView {
                        id: n.id,
                        vector: f32_vec.into_pyobject(py).unwrap().into_any().unbind(),
                        payload: json_to_pyobject(py, &n.payload)?,
                        edges: n
                            .edges
                            .iter()
                            .map(|e| {
                                Ok(PyEdge {
                                    target_id: e.target_id,
                                    label: e.label.clone(),
                                    weight: round_api_f32(e.weight),
                                    metadata: json_to_pyobject(py, &e.metadata)?,
                                })
                            })
                            .collect::<PyResult<Vec<_>>>()?,
                        num_edges: n.edges.len(),
                    }));
                }
            }
            DbBackend::U64(db) => {
                if let Some(n) = db.get(id).map_err(to_py_error)? {
                    return Ok(Some(PyNodeView {
                        id: n.id,
                        vector: n.vector.into_pyobject(py).unwrap().into_any().unbind(),
                        payload: json_to_pyobject(py, &n.payload)?,
                        edges: n
                            .edges
                            .iter()
                            .map(|e| {
                                Ok(PyEdge {
                                    target_id: e.target_id,
                                    label: e.label.clone(),
                                    weight: round_api_f32(e.weight),
                                    metadata: json_to_pyobject(py, &e.metadata)?,
                                })
                            })
                            .collect::<PyResult<Vec<_>>>()?,
                        num_edges: n.edges.len(),
                    }));
                }
            }
        }
        Ok(None)
    }

    fn batch_insert(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        vectors: Bound<'_, PyAny>,
        payloads: &Bound<'_, PyAny>,
    ) -> PyResult<Vec<u64>> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let vectors = write_list(&vectors, "vectors")?;
        let payloads = write_list(payloads, "payloads")?;
        if vectors.len() != payloads.len() {
            return Err(write_input_error(
                pyo3::exceptions::PyValueError::new_err(
                    "vectors 与 payloads 长度必须相同 (vectors and payloads must have the same length)",
                ),
                "payloads",
            ));
        }
        match &mut this.inner {
            DbBackend::F32(db) => {
                let mut tx = crate::database::TxBuilder::new();
                for i in 0..payloads.len() {
                    let payload_obj = payloads.get_item(i).map_err(|error| {
                        write_input_error(
                            error,
                            &crate::sdk::field_member("payloads", &i.to_string()),
                        )
                    })?;
                    let path = crate::sdk::field_member("vectors", &i.to_string());
                    let vec_obj = vectors
                        .get_item(i)
                        .map_err(|error| write_input_error(error, &path))?;
                    let vec: Vec<f32> = write_sequence(&vec_obj, &path, write_float)?;
                    tx.insert(
                        &vec,
                        write_json(
                            &payload_obj,
                            &crate::sdk::field_member("payloads", &i.to_string()),
                        )?,
                    );
                }
                db.commit_tx(tx)
                    .map_err(crate::sdk::direct_write_error)
                    .map_err(to_py_error)
            }
            DbBackend::F16(db) => {
                let mut tx = crate::database::TxBuilder::new();
                for i in 0..payloads.len() {
                    let payload_obj = payloads.get_item(i).map_err(|error| {
                        write_input_error(
                            error,
                            &crate::sdk::field_member("payloads", &i.to_string()),
                        )
                    })?;
                    let path = crate::sdk::field_member("vectors", &i.to_string());
                    let vec_obj = vectors
                        .get_item(i)
                        .map_err(|error| write_input_error(error, &path))?;
                    let vec: Vec<f32> = write_sequence(&vec_obj, &path, write_float)?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    tx.insert(
                        &vec16,
                        write_json(
                            &payload_obj,
                            &crate::sdk::field_member("payloads", &i.to_string()),
                        )?,
                    );
                }
                db.commit_tx(tx)
                    .map_err(crate::sdk::direct_write_error)
                    .map_err(to_py_error)
            }
            DbBackend::U64(db) => {
                let mut tx = crate::database::TxBuilder::new();
                for i in 0..payloads.len() {
                    let payload_obj = payloads.get_item(i).map_err(|error| {
                        write_input_error(
                            error,
                            &crate::sdk::field_member("payloads", &i.to_string()),
                        )
                    })?;
                    let path = crate::sdk::field_member("vectors", &i.to_string());
                    let vec_obj = vectors
                        .get_item(i)
                        .map_err(|error| write_input_error(error, &path))?;
                    let vec: Vec<u64> = write_sequence(&vec_obj, &path, write_id)?;
                    tx.insert(
                        &vec,
                        write_json(
                            &payload_obj,
                            &crate::sdk::field_member("payloads", &i.to_string()),
                        )?,
                    );
                }
                db.commit_tx(tx)
                    .map_err(crate::sdk::direct_write_error)
                    .map_err(to_py_error)
            }
        }
    }

    fn batch_insert_with_ids(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        ids: Bound<'_, PyAny>,
        vectors: Bound<'_, PyAny>,
        payloads: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let ids = write_sequence(&ids, "ids", write_id)?;
        let vectors = write_list(&vectors, "vectors")?;
        let payloads = write_list(payloads, "payloads")?;
        if vectors.len() != payloads.len() {
            return Err(write_input_error(
                pyo3::exceptions::PyValueError::new_err(
                    "vectors 与 payloads 长度必须相同 (vectors and payloads must have the same length)",
                ),
                "payloads",
            ));
        }
        if ids.len() != vectors.len() {
            return Err(write_input_error(
                pyo3::exceptions::PyValueError::new_err(
                    "ids 与 vectors 长度必须相同 (ids and vectors must have the same length)",
                ),
                "ids",
            ));
        }

        match &mut this.inner {
            DbBackend::F32(db) => {
                let mut tx = crate::database::TxBuilder::new();
                for i in 0..payloads.len() {
                    let payload_obj = payloads.get_item(i).map_err(|error| {
                        write_input_error(
                            error,
                            &crate::sdk::field_member("payloads", &i.to_string()),
                        )
                    })?;
                    let path = crate::sdk::field_member("vectors", &i.to_string());
                    let value = vectors
                        .get_item(i)
                        .map_err(|error| write_input_error(error, &path))?;
                    let vec: Vec<f32> = write_sequence(&value, &path, write_float)?;
                    let id = ids.get(i).copied().ok_or_else(|| {
                        write_input_error(
                            PyValueError::new_err(
                                "转换期间批次长度改变 (Batch length changed during conversion)",
                            ),
                            "ids",
                        )
                    })?;
                    tx.insert_with_id(
                        id,
                        &vec,
                        write_json(
                            &payload_obj,
                            &crate::sdk::field_member("payloads", &i.to_string()),
                        )?,
                    );
                }
                db.commit_tx(tx)
                    .map_err(crate::sdk::direct_write_error)
                    .map(|_| ())
                    .map_err(to_py_error)
            }
            DbBackend::F16(db) => {
                let mut tx = crate::database::TxBuilder::new();
                for i in 0..payloads.len() {
                    let payload_obj = payloads.get_item(i).map_err(|error| {
                        write_input_error(
                            error,
                            &crate::sdk::field_member("payloads", &i.to_string()),
                        )
                    })?;
                    let path = crate::sdk::field_member("vectors", &i.to_string());
                    let value = vectors
                        .get_item(i)
                        .map_err(|error| write_input_error(error, &path))?;
                    let vec: Vec<f32> = write_sequence(&value, &path, write_float)?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    let id = ids.get(i).copied().ok_or_else(|| {
                        write_input_error(
                            PyValueError::new_err(
                                "转换期间批次长度改变 (Batch length changed during conversion)",
                            ),
                            "ids",
                        )
                    })?;
                    tx.insert_with_id(
                        id,
                        &vec16,
                        write_json(
                            &payload_obj,
                            &crate::sdk::field_member("payloads", &i.to_string()),
                        )?,
                    );
                }
                db.commit_tx(tx)
                    .map_err(crate::sdk::direct_write_error)
                    .map(|_| ())
                    .map_err(to_py_error)
            }
            DbBackend::U64(db) => {
                let mut tx = crate::database::TxBuilder::new();
                for i in 0..payloads.len() {
                    let payload_obj = payloads.get_item(i).map_err(|error| {
                        write_input_error(
                            error,
                            &crate::sdk::field_member("payloads", &i.to_string()),
                        )
                    })?;
                    let path = crate::sdk::field_member("vectors", &i.to_string());
                    let value = vectors
                        .get_item(i)
                        .map_err(|error| write_input_error(error, &path))?;
                    let vec: Vec<u64> = write_sequence(&value, &path, write_id)?;
                    let id = ids.get(i).copied().ok_or_else(|| {
                        write_input_error(
                            PyValueError::new_err(
                                "转换期间批次长度改变 (Batch length changed during conversion)",
                            ),
                            "ids",
                        )
                    })?;
                    tx.insert_with_id(
                        id,
                        &vec,
                        write_json(
                            &payload_obj,
                            &crate::sdk::field_member("payloads", &i.to_string()),
                        )?,
                    );
                }
                db.commit_tx(tx)
                    .map_err(crate::sdk::direct_write_error)
                    .map(|_| ())
                    .map_err(to_py_error)
            }
        }
    }
}
