//! 持久化、后台维护、容量与资源管理入口。

use super::arguments::{extract_management_argument, integer_argument, write_string};
use super::database::{DbBackend, PyTriviumDB, database_read, database_write};
use super::errors::{attach_error_info, to_py_error, write_admission_error};
use super::json::json_to_pyobject;
use pyo3::prelude::*;

#[pymethods]
impl PyTriviumDB {
    /// 运行时切换 WAL 同步模式: "full" / "normal" / "off"
    fn set_sync_mode(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        mode: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let mode: String = extract_management_argument(py, &mode, "mode")?;
        let sm = crate::storage::wal::SyncMode::parse(&mode).map_err(|reason| {
            to_py_error(
                crate::TriviumError::InvalidInput(reason)
                    .at_field("mode")
                    .with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
            )
        })?;
        dispatch!(this, mut db => db.set_sync_mode(sm)).map_err(to_py_error)
    }

    fn storage_info(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let value = dispatch!(this, db => serde_json::to_value(db.storage_info()))
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        json_to_pyobject(py, &value)
    }

    fn flush(slf: &Bound<'_, Self>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf)?;
        let this = &mut *borrowed;
        dispatch!(this, mut db => db.flush()).map_err(to_py_error)
    }

    fn publish_generation_manifest(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        generation_id: Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let generation_id: String =
            extract_management_argument(py, &generation_id, "generation_id")?;
        let manifest = dispatch!(this, mut db => db.publish_generation_manifest(&generation_id))
            .map_err(to_py_error)?;
        let value = serde_json::to_value(manifest)
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        json_to_pyobject(py, &value)
    }

    /// 维度迁移：将当前数据库的所有节点和边迁移到一个新维度的数据库。
    ///
    /// 向量会被置零（因为维度变了），需要后续调用 update_vector 按节点 ID 逐个更新。
    ///
    /// 返回需要更新向量的节点 ID 列表。
    ///
    /// 示例：
    /// ```python
    /// ids = old_db.migrate("new.tdb", new_dim=1536)
    /// new_db = triviumdb.TriviumDB("new.tdb", dim=1536)
    /// for nid in ids:
    ///     new_vec = new_model.encode(payloads[nid]["text"]).tolist()
    ///     new_db.update_vector(new_vec, nid)
    /// ```
    fn migrate(
        slf: &Bound<'_, Self>,
        new_path: Bound<'_, PyAny>,
        new_dim: Bound<'_, PyAny>,
    ) -> PyResult<Vec<u64>> {
        let (_request, borrowed) = database_read(slf).map_err(write_admission_error)?;
        let this = &*borrowed;
        let new_path = write_string(&new_path, "new_path")?;
        let new_dim: usize = extract_management_argument(slf.py(), &new_dim, "new_dim")?;
        match &this.inner {
            DbBackend::F32(db) => {
                let (_new_db, ids) = db.migrate_to(&new_path, new_dim).map_err(to_py_error)?;
                Ok(ids)
            }
            DbBackend::F16(db) => {
                let (_new_db, ids) = db.migrate_to(&new_path, new_dim).map_err(to_py_error)?;
                Ok(ids)
            }
            DbBackend::U64(db) => {
                let (_new_db, ids) = db.migrate_to(&new_path, new_dim).map_err(to_py_error)?;
                Ok(ids)
            }
        }
    }

    #[pyo3(signature = (interval_secs=integer_argument(7200)), text_signature = "($self, interval_secs=7200)")]
    fn enable_auto_compaction(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        interval_secs: Py<PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let interval_secs: u64 =
            extract_management_argument(py, interval_secs.bind(py), "interval_secs")?;
        dispatch!(this, mut db => db.enable_auto_compaction(std::time::Duration::from_secs(interval_secs)))
                .map_err(to_py_error)
    }

    fn set_auto_build_quiver(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        enabled: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let enabled: bool = extract_management_argument(py, &enabled, "enabled")?;
        dispatch!(this, mut db => db.set_auto_build_quiver(enabled)).map_err(to_py_error)
    }

    fn clear_search_state(slf: &Bound<'_, Self>) -> PyResult<()> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        dispatch!(this, db => db.clear_search_state()).map_err(to_py_error)
    }

    fn disable_auto_compaction(slf: &Bound<'_, Self>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf)?;
        let this = &mut *borrowed;
        dispatch!(this, mut db => db.disable_auto_compaction());
        Ok(())
    }

    fn compact(slf: &Bound<'_, Self>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf)?;
        let this = &mut *borrowed;
        dispatch!(this, mut db => db.compact()).map_err(to_py_error)
    }

    /// 为后续插入主动预留额外节点容量。
    fn reserve_nodes(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        additional: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let additional: usize = extract_management_argument(py, &additional, "additional")?;
        dispatch!(this, db => db.reserve_nodes(additional)).map_err(to_py_error)
    }

    /// 设置内核内存预算（MiB），0 表示不限制。
    #[pyo3(signature = (mb=integer_argument(0)), text_signature = "($self, mb=0)")]
    fn set_memory_limit(slf: &Bound<'_, Self>, py: Python<'_>, mb: Py<PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let mb: usize = extract_management_argument(py, mb.bind(py), "mb")?;
        let bytes = mb.checked_mul(1024 * 1024).ok_or_else(|| {
            let reason = "内存上限换算字节时溢出 (Memory limit overflows when converted to bytes)";
            attach_error_info(
                pyo3::exceptions::PyOverflowError::new_err(reason),
                crate::TriviumError::InvalidInput(reason.into())
                    .at_field("mb")
                    .with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
            )
        })?;
        dispatch!(this, mut db => db.set_memory_limit(bytes)).map_err(|error| {
            let error = if error.info().target.is_some() {
                error.at_field("mb")
            } else {
                error
            };
            to_py_error(error)
        })
    }

    /// 读取多个数据库共享的配额、工作线程和任务统计。
    fn resource_stats(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let stats = dispatch!(this, db => db.resource_stats());
        let value = serde_json::to_value(stats)
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        json_to_pyobject(py, &value)
    }

    fn instance_stats(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let stats = dispatch!(this, db => db.instance_stats());
        let value = serde_json::to_value(stats)
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        json_to_pyobject(py, &value)
    }

    fn estimated_memory(slf: &Bound<'_, Self>) -> PyResult<usize> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        Ok(dispatch!(this, db => db.estimated_memory()))
    }
}
