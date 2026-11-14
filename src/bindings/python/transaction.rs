//! 事务构建、提交及事务对象的语言生命周期。

use super::arguments::{
    float_argument, float_value, integer_value, sequence_value, string_argument, string_value,
};
use super::database::{DbBackend, PyTriviumDB, database_read, database_write};
use super::errors::{TRANSACTION_VALIDATION, to_py_error, transaction_admission_error};
use super::json::pyobject_to_json_at;
use pyo3::prelude::*;

fn check_transaction_control() -> PyResult<()> {
    crate::request::check_current()
        .map_err(|error| to_py_error(error.with_write_info(TRANSACTION_VALIDATION)))
}

// ════════════════════════════════════════════════════════
//  PyTransaction — 基于 Rust TxBuilder 的事务绑定
// ════════════════════════════════════════════════════════

/// 按 dtype 分发的 TxBuilder 后端
enum TxBuilderBackend {
    F32(crate::database::TxBuilder<f32>),
    F16(crate::database::TxBuilder<half::f16>),
    U64(crate::database::TxBuilder<u64>),
}

/// Python 侧的轻量级事务对象
///
/// 底层直接使用 Rust TxBuilder 收集操作，commit 时调用 Database::commit_tx。
/// 支持上下文管理器 (with 语句)。
#[pyclass(name = "Transaction")]
pub(super) struct PyTransaction {
    db: Py<PyTriviumDB>,
    builder: Option<TxBuilderBackend>,
}

// 参数转换可执行 Python 回调；先登记事务身份，再取得可失败的借用。
fn transaction_read<'py>(
    transaction: &Bound<'py, PyTransaction>,
) -> PyResult<(crate::sdk::RequestScope, PyRef<'py, PyTransaction>)> {
    let request = crate::sdk::RequestScope::enter(
        transaction.as_ptr() as usize,
        "python_transaction_callback",
    )
    .map_err(to_py_error)?;
    let borrowed = transaction.try_borrow().map_err(|_| {
        to_py_error(crate::TriviumError::DatabaseBusy {
            resource: "python_transaction",
        })
    })?;
    Ok((request, borrowed))
}

fn transaction_write<'py>(
    transaction: &Bound<'py, PyTransaction>,
) -> PyResult<(crate::sdk::RequestScope, PyRefMut<'py, PyTransaction>)> {
    let request = crate::sdk::RequestScope::enter(
        transaction.as_ptr() as usize,
        "python_transaction_callback",
    )
    .map_err(|error| {
        to_py_error(error.with_write_info(crate::error::WriteErrorInfo::TRANSACTION_ADMISSION))
    })?;
    let borrowed = transaction.try_borrow_mut().map_err(|_| {
        to_py_error(
            crate::TriviumError::DatabaseBusy {
                resource: "python_transaction",
            }
            .with_write_info(crate::error::WriteErrorInfo::TRANSACTION_ADMISSION),
        )
    })?;
    Ok((request, borrowed))
}

impl PyTransaction {
    fn active_builder(&mut self) -> PyResult<&mut TxBuilderBackend> {
        self.builder.as_mut().ok_or_else(|| {
            to_py_error(
                crate::TriviumError::TransactionFinished
                    .with_write_info(crate::error::WriteErrorInfo::TRANSACTION_ADMISSION),
            )
        })
    }

    fn pending_count_inner(&self) -> usize {
        match self.builder.as_ref() {
            Some(TxBuilderBackend::F32(b)) => b.pending_count(),
            Some(TxBuilderBackend::F16(b)) => b.pending_count(),
            Some(TxBuilderBackend::U64(b)) => b.pending_count(),
            None => 0,
        }
    }

    fn commit_inner(&mut self, py: Python<'_>) -> PyResult<Vec<u64>> {
        self.active_builder().map_err(transaction_admission_error)?;
        // 先取得借用并完成内核准入，拒绝时保留整笔待提交操作。
        let (_request, mut db_ref) =
            database_write(self.db.bind(py)).map_err(transaction_admission_error)?;
        dispatch!(db_ref, db => db.check_transaction_admission()).map_err(|error| {
            to_py_error(error.with_write_info(crate::error::WriteErrorInfo::TRANSACTION_ADMISSION))
        })?;
        let builder = self
            .builder
            .take()
            .ok_or_else(|| to_py_error(crate::TriviumError::TransactionFinished))?;
        match (&mut db_ref.inner, builder) {
            (DbBackend::F32(db), TxBuilderBackend::F32(b)) => db.commit_tx(b).map_err(to_py_error),
            (DbBackend::F16(db), TxBuilderBackend::F16(b)) => db.commit_tx(b).map_err(to_py_error),
            (DbBackend::U64(db), TxBuilderBackend::U64(b)) => db.commit_tx(b).map_err(to_py_error),
            _ => Err(to_py_error(crate::TriviumError::InvalidInput(
                "事务与数据库类型不匹配 (Transaction and database dtypes do not match)".into(),
            ))),
        }
    }
}

#[pymethods]
impl PyTransaction {
    /// 缓冲一个插入操作
    fn insert(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        vector: Bound<'_, PyAny>,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut this) = transaction_write(slf)?;
        let builder = this.active_builder()?;
        let json = pyobject_to_json_at(payload, Some("payload"), Some(TRANSACTION_VALIDATION))?;
        match builder {
            TxBuilderBackend::F32(b) => {
                let v: Vec<f32> = sequence_value(
                    &vector,
                    "vector",
                    Some(TRANSACTION_VALIDATION),
                    |value, field| float_value(value, field, Some(TRANSACTION_VALIDATION)),
                )?;
                check_transaction_control()?;
                b.insert(&v, json);
            }
            TxBuilderBackend::F16(b) => {
                let values: Vec<f32> = sequence_value(
                    &vector,
                    "vector",
                    Some(TRANSACTION_VALIDATION),
                    |value, field| float_value(value, field, Some(TRANSACTION_VALIDATION)),
                )?;
                let v: Vec<half::f16> = values.into_iter().map(half::f16::from_f32).collect();
                check_transaction_control()?;
                b.insert(&v, json);
            }
            TxBuilderBackend::U64(b) => {
                let v: Vec<u64> = sequence_value(
                    &vector,
                    "vector",
                    Some(TRANSACTION_VALIDATION),
                    |value, field| integer_value(value, field, Some(TRANSACTION_VALIDATION)),
                )?;
                check_transaction_control()?;
                b.insert(&v, json);
            }
        }
        Ok(())
    }

    /// 缓冲一个带自定义 ID 的插入操作
    fn insert_with_id(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        id: Bound<'_, PyAny>,
        vector: Bound<'_, PyAny>,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut this) = transaction_write(slf)?;
        let id = integer_value(&id, "id", Some(TRANSACTION_VALIDATION))?;
        let builder = this.active_builder()?;
        let json = pyobject_to_json_at(payload, Some("payload"), Some(TRANSACTION_VALIDATION))?;
        match builder {
            TxBuilderBackend::F32(b) => {
                let v: Vec<f32> = sequence_value(
                    &vector,
                    "vector",
                    Some(TRANSACTION_VALIDATION),
                    |value, field| float_value(value, field, Some(TRANSACTION_VALIDATION)),
                )?;
                check_transaction_control()?;
                b.insert_with_id(id, &v, json);
            }
            TxBuilderBackend::F16(b) => {
                let values: Vec<f32> = sequence_value(
                    &vector,
                    "vector",
                    Some(TRANSACTION_VALIDATION),
                    |value, field| float_value(value, field, Some(TRANSACTION_VALIDATION)),
                )?;
                let v: Vec<half::f16> = values.into_iter().map(half::f16::from_f32).collect();
                check_transaction_control()?;
                b.insert_with_id(id, &v, json);
            }
            TxBuilderBackend::U64(b) => {
                let v: Vec<u64> = sequence_value(
                    &vector,
                    "vector",
                    Some(TRANSACTION_VALIDATION),
                    |value, field| integer_value(value, field, Some(TRANSACTION_VALIDATION)),
                )?;
                check_transaction_control()?;
                b.insert_with_id(id, &v, json);
            }
        }
        Ok(())
    }

    /// 缓冲一个连边操作
    #[pyo3(signature = (src, dst, label=string_argument("related"), weight=float_argument(1.0)), text_signature="($self, src, dst, label='related', weight=1.0)")]
    fn link(
        slf: &Bound<'_, Self>,
        src: Bound<'_, PyAny>,
        dst: Bound<'_, PyAny>,
        label: Py<PyAny>,
        weight: Py<PyAny>,
    ) -> PyResult<()> {
        let (_request, mut this) = transaction_write(slf)?;
        let src = integer_value(&src, "src", Some(TRANSACTION_VALIDATION))?;
        let dst = integer_value(&dst, "dst", Some(TRANSACTION_VALIDATION))?;
        let label = string_value(label.bind(slf.py()), "label", Some(TRANSACTION_VALIDATION))?;
        let weight = float_value(
            weight.bind(slf.py()),
            "weight",
            Some(TRANSACTION_VALIDATION),
        )?;
        check_transaction_control()?;
        match this.active_builder()? {
            TxBuilderBackend::F32(b) => b.link(src, dst, &label, weight),
            TxBuilderBackend::F16(b) => b.link(src, dst, &label, weight),
            TxBuilderBackend::U64(b) => b.link(src, dst, &label, weight),
        }
        Ok(())
    }

    /// 缓冲一个删除操作
    fn delete(slf: &Bound<'_, Self>, id: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut this) = transaction_write(slf)?;
        let id = integer_value(&id, "id", Some(TRANSACTION_VALIDATION))?;
        check_transaction_control()?;
        match this.active_builder()? {
            TxBuilderBackend::F32(b) => b.delete(id),
            TxBuilderBackend::F16(b) => b.delete(id),
            TxBuilderBackend::U64(b) => b.delete(id),
        }
        Ok(())
    }

    /// 缓冲一个断边操作
    #[pyo3(signature = (src, dst, label=None))]
    fn unlink(
        slf: &Bound<'_, Self>,
        src: Bound<'_, PyAny>,
        dst: Bound<'_, PyAny>,
        label: Option<Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let (_request, mut this) = transaction_write(slf)?;
        let src = integer_value(&src, "src", Some(TRANSACTION_VALIDATION))?;
        let dst = integer_value(&dst, "dst", Some(TRANSACTION_VALIDATION))?;
        let label = label
            .map(|value| string_value(&value, "label", Some(TRANSACTION_VALIDATION)))
            .transpose()?;
        check_transaction_control()?;
        match (this.active_builder()?, label.as_deref()) {
            (TxBuilderBackend::F32(b), Some(label)) => b.unlink_label(src, dst, label),
            (TxBuilderBackend::F16(b), Some(label)) => b.unlink_label(src, dst, label),
            (TxBuilderBackend::U64(b), Some(label)) => b.unlink_label(src, dst, label),
            (TxBuilderBackend::F32(b), None) => b.unlink(src, dst),
            (TxBuilderBackend::F16(b), None) => b.unlink(src, dst),
            (TxBuilderBackend::U64(b), None) => b.unlink(src, dst),
        }
        Ok(())
    }

    /// 缓冲一个更新 payload 操作
    fn update_payload(
        slf: &Bound<'_, Self>,
        _py: Python<'_>,
        id: Bound<'_, PyAny>,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut this) = transaction_write(slf)?;
        let id = integer_value(&id, "id", Some(TRANSACTION_VALIDATION))?;
        let builder = this.active_builder()?;
        let json = pyobject_to_json_at(payload, Some("payload"), Some(TRANSACTION_VALIDATION))?;
        check_transaction_control()?;
        match builder {
            TxBuilderBackend::F32(b) => b.update_payload(id, json),
            TxBuilderBackend::F16(b) => b.update_payload(id, json),
            TxBuilderBackend::U64(b) => b.update_payload(id, json),
        }
        Ok(())
    }

    /// 缓冲一个更新向量操作
    fn update_vector(
        slf: &Bound<'_, Self>,
        id: Bound<'_, PyAny>,
        vector: Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let (_request, mut this) = transaction_write(slf)?;
        let id = integer_value(&id, "id", Some(TRANSACTION_VALIDATION))?;
        check_transaction_control()?;
        match this.active_builder()? {
            TxBuilderBackend::F32(b) => {
                let v: Vec<f32> = sequence_value(
                    &vector,
                    "vector",
                    Some(TRANSACTION_VALIDATION),
                    |value, field| float_value(value, field, Some(TRANSACTION_VALIDATION)),
                )?;
                check_transaction_control()?;
                b.update_vector(id, &v);
            }
            TxBuilderBackend::F16(b) => {
                let values: Vec<f32> = sequence_value(
                    &vector,
                    "vector",
                    Some(TRANSACTION_VALIDATION),
                    |value, field| float_value(value, field, Some(TRANSACTION_VALIDATION)),
                )?;
                let v: Vec<half::f16> = values.into_iter().map(half::f16::from_f32).collect();
                check_transaction_control()?;
                b.update_vector(id, &v);
            }
            TxBuilderBackend::U64(b) => {
                let v: Vec<u64> = sequence_value(
                    &vector,
                    "vector",
                    Some(TRANSACTION_VALIDATION),
                    |value, field| integer_value(value, field, Some(TRANSACTION_VALIDATION)),
                )?;
                check_transaction_control()?;
                b.update_vector(id, &v);
            }
        }
        Ok(())
    }

    /// 当前事务中缓冲的操作数
    fn pending_count(slf: &Bound<'_, Self>) -> PyResult<usize> {
        let (_request, this) = transaction_read(slf)?;
        Ok(this.pending_count_inner())
    }

    /// 原子提交事务：Dry-Run 预检 + WAL-first 写入
    fn commit(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<Vec<u64>> {
        let (_request, mut this) = transaction_write(slf).map_err(transaction_admission_error)?;
        this.commit_inner(py)
    }

    /// 回滚事务（丢弃所有缓冲操作）
    fn rollback(slf: &Bound<'_, Self>) -> PyResult<()> {
        let (_request, mut this) = transaction_write(slf)?;
        this.builder
            .take()
            .ok_or_else(|| to_py_error(crate::TriviumError::TransactionFinished))?;
        Ok(())
    }

    // 上下文管理器支持
    fn __enter__(slf: &Bound<'_, Self>) -> PyResult<Py<Self>> {
        let (_request, mut this) = transaction_write(slf)?;
        this.active_builder()?;
        Ok(slf.clone().unbind())
    }

    #[pyo3(signature = (exc_type=None, _exc_val=None, _exc_tb=None))]
    fn __exit__(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        exc_type: Option<&Bound<'_, PyAny>>,
        _exc_val: Option<&Bound<'_, PyAny>>,
        _exc_tb: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        let (_request, mut this) = transaction_write(slf).map_err(transaction_admission_error)?;
        if this.builder.is_none() {
            return Ok(false);
        }
        if exc_type.is_some() {
            this.builder.take();
        } else {
            this.commit_inner(py)?;
        }
        Ok(false)
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        let (_request, this) = transaction_read(slf)?;
        Ok(format!(
            "Transaction(pending={}, finished={})",
            this.pending_count_inner(),
            this.builder.is_none()
        ))
    }

    fn __traverse__(
        &self,
        visit: pyo3::PyVisit<'_>,
    ) -> std::result::Result<(), pyo3::PyTraverseError> {
        // 向 GC 暴露 Transaction → Database，数据库的 __clear__ 负责断开 Hook 环。
        visit.call(&self.db)
    }
}

#[pymethods]
impl PyTriviumDB {
    // ════════ 事务 ════════

    /// 开启一个轻量级事务，返回 PyTransaction 对象
    ///
    /// 支持上下文管理器风格：
    /// ```python
    /// with db.transaction() as tx:
    ///     tx.insert([1.0, 0.0], {"name": "Alice"})
    ///     tx.link(1, 2, label="knows")
    ///     # 正常退出 → 自动 commit
    ///     # 异常 → 自动 rollback
    /// ```
    fn transaction(slf: Py<Self>, py: Python<'_>) -> PyResult<PyTransaction> {
        let (_request, borrowed) = database_read(slf.bind(py))?;
        dispatch!(borrowed, db => db.check_request_admission()).map_err(to_py_error)?;
        let dtype = borrowed.dtype.clone();
        let builder = match dtype.as_str() {
            "f32" => TxBuilderBackend::F32(crate::database::TxBuilder::new()),
            "f16" => TxBuilderBackend::F16(crate::database::TxBuilder::new()),
            "u64" => TxBuilderBackend::U64(crate::database::TxBuilder::new()),
            _ => {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "不支持的 dtype: {}",
                    dtype
                )));
            }
        };
        drop(borrowed);
        Ok(PyTransaction {
            db: slf,
            builder: Some(builder),
        })
    }
}
