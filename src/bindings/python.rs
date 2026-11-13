//! Python PyO3 公共绑定。
//!
//! 负责 Python 值转换、GIL 释放、上下文管理器和异常映射，不复制核心查询逻辑。批量
//! 计算在进入 Rust 后释放 GIL；`.pyi` 由已编译模块生成并通过 stubtest 防止签名漂移。

#[cfg(feature = "python")]
#[allow(clippy::module_inception)]
pub mod python {
    use crate::database::Database as GenericDatabase;
    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;
    use pyo3::types::{PyDict, PyList, PyString, PyTuple};

    pyo3::create_exception!(triviumdb, TriviumDBError, pyo3::exceptions::PyRuntimeError);
    pyo3::create_exception!(triviumdb, InvalidInputError, pyo3::exceptions::PyValueError);
    pyo3::create_exception!(triviumdb, CapacityError, pyo3::exceptions::PyMemoryError);
    pyo3::create_exception!(
        triviumdb,
        SnapshotDestinationError,
        pyo3::exceptions::PyFileExistsError
    );
    pyo3::create_exception!(triviumdb, ReadOnlyError, TriviumDBError);
    pyo3::create_exception!(triviumdb, RecoveryRequiredError, TriviumDBError);
    pyo3::create_exception!(triviumdb, ImmutableArtifactError, TriviumDBError);
    pyo3::create_exception!(triviumdb, GenerationBusyError, TriviumDBError);
    pyo3::create_exception!(triviumdb, UnsupportedDatabaseVersionError, TriviumDBError);
    pyo3::create_exception!(triviumdb, ResourceBudgetError, TriviumDBError);
    pyo3::create_exception!(triviumdb, SnapshotPublicationError, TriviumDBError);

    #[pyclass(name = "ResourceRuntime", frozen)]
    pub struct PyResourceRuntime {
        inner: crate::resources::ResourceRuntime,
    }

    #[pyclass(name = "RequestContext", frozen)]
    pub struct PyRequestContext {
        inner: crate::sdk::RequestContext,
    }

    #[pymethods]
    impl PyRequestContext {
        #[new]
        #[pyo3(signature = (*args, **kwargs), text_signature="(request_id, timeout_ms=0)")]
        fn new(
            py: Python<'_>,
            args: &Bound<'_, PyTuple>,
            kwargs: Option<&Bound<'_, PyDict>>,
        ) -> PyResult<Self> {
            let values = constructor_arguments(py, "RequestContext", args, kwargs)?;
            let request_id: String = extract_argument(
                py,
                &constructor_argument(&values, "request_id")?,
                "request_id",
                None,
            )?;
            let timeout_ms: u64 = extract_argument(
                py,
                &constructor_argument(&values, "timeout_ms")?,
                "timeout_ms",
                None,
            )?;
            crate::sdk::RequestContext::with_timeout(
                &request_id,
                std::time::Duration::from_millis(timeout_ms),
                false,
            )
            .map(|inner| Self { inner })
            .map_err(to_py_error)
        }

        #[getter]
        fn request_id(&self) -> &str {
            self.inner.request_id()
        }
        fn cancel(&self) {
            self.inner.cancel();
        }
        fn check(&self) -> PyResult<()> {
            self.inner.control().check().map_err(to_py_error)
        }

        fn __enter__(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<Py<Self>> {
            py.import("triviumdb._requests")?
                .getattr("enter")?
                .call1((slf,))
                .map_err(|error| request_scope_error(py, error))?;
            Ok(slf.clone().unbind())
        }

        fn __exit__(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            _exc_type: &Bound<'_, PyAny>,
            _exc_val: &Bound<'_, PyAny>,
            _exc_tb: &Bound<'_, PyAny>,
        ) -> PyResult<bool> {
            py.import("triviumdb._requests")?
                .getattr("exit")?
                .call1((slf,))
                .map_err(|error| request_scope_error(py, error))?;
            Ok(false)
        }
    }

    fn request_scope_error(py: Python<'_>, error: PyErr) -> PyErr {
        if error.is_instance_of::<PyValueError>(py) {
            attach_error_info(
                error,
                crate::TriviumError::InvalidInput(
                    "请求上下文进入或退出无效 (Invalid request context entry or exit)".into(),
                )
                .at_field("request_context"),
            )
        } else {
            error
        }
    }

    #[pyfunction]
    fn _invoke_request(
        context: &PyRequestContext,
        owner: &str,
        operation: &str,
        callback: &Bound<'_, PyAny>,
        args: &Bound<'_, PyTuple>,
        kwargs: &Bound<'_, PyDict>,
    ) -> PyResult<PyObject> {
        let admission = |error| to_py_error(crate::sdk::request_admission(error, owner, operation));
        let _scope = context.inner.enter().map_err(admission)?;
        if operation != "console_request" {
            crate::request::check_current().map_err(admission)?;
        }
        Ok(callback.call(args, Some(kwargs))?.into_any().unbind())
    }

    /// 控制台策略与取消状态不持有数据库或文件锁。
    #[pyclass(name = "ConsoleSession", frozen)]
    pub struct PyConsoleSession {
        inner: crate::ConsoleSession,
    }

    #[pymethods]
    impl PyConsoleSession {
        #[new]
        #[pyo3(signature = (*args, **kwargs), text_signature="(options_json='{}')")]
        fn new(
            py: Python<'_>,
            args: &Bound<'_, PyTuple>,
            kwargs: Option<&Bound<'_, PyDict>>,
        ) -> PyResult<Self> {
            let values = constructor_arguments(py, "ConsoleSession", args, kwargs)?;
            let options_json: String = extract_argument(
                py,
                &constructor_argument(&values, "options_json")?,
                "options_json",
                None,
            )?;
            crate::ConsoleSession::from_json(&options_json)
                .map(|inner| Self { inner })
                .map_err(|error| to_py_error(error.at_field("options_json")))
        }

        fn detach(&self) {
            self.inner.detach();
        }
        fn is_detached(&self) -> bool {
            self.inner.is_detached()
        }
        fn is_drained(&self) -> bool {
            self.inner.is_drained()
        }
        fn cancel(&self, request_id: Bound<'_, PyAny>) -> PyResult<bool> {
            let request_id = read_string(&request_id, "request_id")?;
            Ok(self.inner.cancel(&request_id))
        }
    }

    #[pymethods]
    impl PyResourceRuntime {
        #[new]
        #[pyo3(signature = (*args, **kwargs), text_signature="(memory_limit_mb=0, max_threads=0, max_active_tasks=0, maintenance_memory_limit_mb=0)")]
        fn new(
            py: Python<'_>,
            args: &Bound<'_, PyTuple>,
            kwargs: Option<&Bound<'_, PyDict>>,
        ) -> PyResult<Self> {
            let values = constructor_arguments(py, "ResourceRuntime", args, kwargs)?;
            let memory_limit_mb = constructor_argument(&values, "memory_limit_mb")?;
            let max_threads = constructor_argument(&values, "max_threads")?;
            let max_active_tasks = constructor_argument(&values, "max_active_tasks")?;
            let maintenance_memory_limit_mb =
                constructor_argument(&values, "maintenance_memory_limit_mb")?;
            let memory_limit_mb = extract_argument(py, &memory_limit_mb, "memory_limit_mb", None)?;
            let max_threads = extract_argument(py, &max_threads, "max_threads", None)?;
            let max_active_tasks =
                extract_argument(py, &max_active_tasks, "max_active_tasks", None)?;
            let maintenance_memory_limit_mb = extract_argument(
                py,
                &maintenance_memory_limit_mb,
                "maintenance_memory_limit_mb",
                None,
            )?;
            let memory_limit = constructor_mib(memory_limit_mb, "memory_limit_mb")?;
            crate::resources::ResourceRuntime::new(crate::resources::ResourceLimits {
                maintenance_memory_limit: constructor_mib(
                    maintenance_memory_limit_mb,
                    "maintenance_memory_limit_mb",
                )?,
                memory_limit,
                max_threads,
                max_active_tasks,
            })
            .map(|inner| Self { inner })
            .map_err(|error| {
                let error = match error.info().target {
                    Some(crate::error::ErrorTarget::Field { field })
                        if field == "maintenance_memory_limit" =>
                    {
                        error.at_field("maintenance_memory_limit_mb")
                    }
                    _ => error,
                };
                to_py_error(error)
            })
        }

        fn stats(&self, py: Python<'_>) -> PyResult<PyObject> {
            let value = serde_json::to_value(self.inner.stats())
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
        }
    }

    fn to_py_error(error: crate::error::TriviumError) -> PyErr {
        use crate::error::TriviumError;
        let exception = match error.underlying() {
            TriviumError::SnapshotPublicationUncertain { .. } => {
                SnapshotPublicationError::new_err(error.to_string())
            }
            TriviumError::SnapshotDestinationExists { .. } => {
                SnapshotDestinationError::new_err(error.to_string())
            }
            TriviumError::ResourceBudgetExceeded { .. } => {
                ResourceBudgetError::new_err(error.to_string())
            }
            TriviumError::ReadOnlyViolation { .. } => ReadOnlyError::new_err(error.to_string()),
            TriviumError::RecoveryRequired { .. } => {
                RecoveryRequiredError::new_err(error.to_string())
            }
            TriviumError::ImmutableArtifactInvalid { .. } => {
                ImmutableArtifactError::new_err(error.to_string())
            }
            TriviumError::GenerationBusy { .. } => GenerationBusyError::new_err(error.to_string()),
            TriviumError::UnsupportedDatabaseVersion { .. } => {
                UnsupportedDatabaseVersionError::new_err(error.to_string())
            }
            TriviumError::CapacityReservationRejected { .. }
            | TriviumError::CapacityAllocationFailed { .. } => {
                CapacityError::new_err(error.to_string())
            }
            TriviumError::InvalidInput(_) => InvalidInputError::new_err(error.to_string()),
            _ => TriviumDBError::new_err(error.to_string()),
        };
        attach_error_info(exception, error)
    }

    fn attach_error_info(exception: PyErr, error: crate::TriviumError) -> PyErr {
        attach_error_descriptor(exception, error.info())
    }

    fn attach_error_descriptor(exception: PyErr, info: crate::error::ErrorInfo) -> PyErr {
        Python::with_gil(|py| -> PyResult<()> {
            let value = exception.value(py);
            value.setattr("schema_version", info.schema_version)?;
            value.setattr("code", info.code)?;
            value.setattr("message", info.message)?;
            value.setattr(
                "stage",
                info.stage.map(crate::error::QueryErrorStage::as_str),
            )?;
            let target = serde_json::to_value(info.target)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            value.setattr("target", json_to_pyobject(py, &target)?)?;
            let write = serde_json::to_value(info.write)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            value.setattr("write", json_to_pyobject(py, &write)?)?;
            let details = info.details;
            value.setattr("details", json_to_pyobject(py, &details)?)?;
            // 保留资源预算等已有的直接属性，字段的权威值来自内核。
            if let Some(fields) = details.as_object() {
                for (name, field) in fields {
                    if matches!(
                        name.as_str(),
                        "schema_version"
                            | "code"
                            | "message"
                            | "details"
                            | "stage"
                            | "target"
                            | "write"
                    ) {
                        continue;
                    }
                    value.setattr(name.as_str(), json_to_pyobject(py, field)?)?;
                }
            }
            Ok(())
        })
        .map_or_else(|conversion| conversion, |()| exception)
    }

    enum DbBackend {
        F32(GenericDatabase<f32>),
        F16(GenericDatabase<half::f16>),
        U64(GenericDatabase<u64>),
    }

    const TRANSACTION_VALIDATION: crate::error::WriteErrorInfo = crate::error::WriteErrorInfo {
        transaction_consumed: Some(false),
        ..crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION
    };

    fn check_transaction_control() -> PyResult<()> {
        crate::request::check_current()
            .map_err(|error| to_py_error(error.with_write_info(TRANSACTION_VALIDATION)))
    }

    fn transaction_admission_error(error: PyErr) -> PyErr {
        python_write_error(error, crate::error::WriteErrorInfo::TRANSACTION_ADMISSION)
    }

    fn write_admission_error(error: PyErr) -> PyErr {
        python_write_error(error, crate::error::WriteErrorInfo::MAINTENANCE_ADMISSION)
    }

    fn python_write_error(error: PyErr, write: crate::error::WriteErrorInfo) -> PyErr {
        Python::with_gil(|py| -> PyResult<()> {
            let write = serde_json::to_value(write)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            error
                .value(py)
                .setattr("write", json_to_pyobject(py, &write)?)
        })
        .map_or_else(|conversion| conversion, |()| error)
    }

    /// 默认值构造为真实 Python 整数，使显式提取保留原有数值默认值。
    fn integer_argument(value: u64) -> Py<PyAny> {
        Python::with_gil(|py| match value.into_pyobject(py) {
            Ok(value) => value.into_any().unbind(),
            Err(never) => match never {},
        })
    }

    fn string_argument(value: &str) -> Py<PyAny> {
        Python::with_gil(|py| pyo3::types::PyString::new(py, value).into_any().unbind())
    }

    fn float_argument(value: f64) -> Py<PyAny> {
        Python::with_gil(|py| pyo3::types::PyFloat::new(py, value).into_any().unbind())
    }

    fn boolean_argument(value: bool) -> Py<PyAny> {
        Python::with_gil(|py| {
            value
                .into_pyobject(py)
                .unwrap()
                .to_owned()
                .into_any()
                .unbind()
        })
    }

    fn constructor_arguments<'py>(
        py: Python<'py>,
        owner: &str,
        args: &Bound<'py, PyTuple>,
        kwargs: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<Bound<'py, PyDict>> {
        py.import("triviumdb.triviumdb")?
            .getattr("_bind_constructor")?
            .call1((owner, args, kwargs))?
            .downcast_into::<PyDict>()
            .map_err(Into::into)
    }

    fn constructor_argument<'py>(
        values: &Bound<'py, PyDict>,
        field: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        values.get_item(field)?.ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "构造签名绑定缺少字段 (Constructor signature binding omitted a field)",
            )
        })
    }

    fn parse_python_runtime(
        runtime: Option<Bound<'_, PyAny>>,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> PyResult<crate::resources::ResourceRuntime> {
        let Some(value) = runtime else {
            return Ok(crate::resources::ResourceRuntime::shared_default());
        };
        // PyRef 按原生类型提取，错误对象的宿主属性和转换协议均不参与句柄校验。
        let runtime = value
            .extract::<PyRef<'_, PyResourceRuntime>>()
            .map_err(|error| {
                let input = crate::TriviumError::InvalidInput(
                    "runtime 必须是 ResourceRuntime (runtime must be a ResourceRuntime)".into(),
                )
                .at_field("runtime");
                let input = match write {
                    Some(write) => input.with_write_info(write),
                    None => input,
                };
                attach_error_info(error, input)
            })?;
        Ok(runtime.inner.clone())
    }

    fn constructor_input_error(error: PyErr, field: &str) -> PyErr {
        attach_error_info(
            error,
            crate::TriviumError::InvalidInput(
                "构造参数值无效 (Invalid constructor argument value)".into(),
            )
            .at_field(field),
        )
    }

    fn constructor_mib(value: usize, field: &str) -> PyResult<usize> {
        value.checked_mul(1024 * 1024).ok_or_else(|| {
            constructor_input_error(
                pyo3::exceptions::PyOverflowError::new_err(
                    "MiB 换算字节时溢出 (MiB conversion to bytes overflowed)",
                ),
                field,
            )
        })
    }

    fn extract_management_argument<'py, T: FromPyObject<'py>>(
        py: Python<'py>,
        value: &Bound<'py, PyAny>,
        field: &str,
    ) -> PyResult<T> {
        extract_argument(
            py,
            value,
            field,
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        )
    }

    fn extract_argument<'py, T: FromPyObject<'py>>(
        py: Python<'py>,
        value: &Bound<'py, PyAny>,
        field: &str,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> PyResult<T> {
        value.extract().map_err(|error| {
            use pyo3::types::{PyBool, PyBytes, PyComplex, PyFloat, PyInt, PyString, PyTuple};
            let value_type = value.get_type();
            let builtin = value.is_none()
                || [
                    py.get_type::<PyBool>(),
                    py.get_type::<PyInt>(),
                    py.get_type::<PyFloat>(),
                    py.get_type::<PyComplex>(),
                    py.get_type::<PyString>(),
                    py.get_type::<PyBytes>(),
                    py.get_type::<PyList>(),
                    py.get_type::<PyTuple>(),
                    py.get_type::<PyDict>(),
                ]
                .iter()
                .any(|expected| value_type.is(expected));
            if builtin {
                let input = crate::TriviumError::InvalidInput(format!(
                    "参数类型或范围无效 (Invalid argument type or range): {field}"
                ))
                .at_field(field);
                let input = match write {
                    Some(write) => input.with_write_info(write),
                    None => input,
                };
                attach_error_info(error, input)
            } else {
                // 自定义转换协议中的宿主异常保留原对象、cause 与已有诊断。
                error
            }
        })
    }

    fn numeric_protocol(value: &Bound<'_, PyAny>, slot: i32) -> bool {
        // SAFETY: Bound 与 GIL 保证对象和类型有效；只检测协议，不调用槽位指针。
        // Python 3.9 的 PyType_GetSlot 只支持堆类型，静态类型交给既有数值协议提取。
        unsafe {
            let value_type = value.get_type();
            if pyo3::ffi::PyType_GetFlags(value_type.as_type_ptr()) & pyo3::ffi::Py_TPFLAGS_HEAPTYPE
                == 0
            {
                pyo3::ffi::PyNumber_Check(value.as_ptr()) != 0
            } else {
                !pyo3::ffi::PyType_GetSlot(value_type.as_type_ptr(), slot).is_null()
            }
        }
    }

    fn integer_value<'py, T: FromPyObject<'py>>(
        value: &Bound<'py, PyAny>,
        field: &str,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> PyResult<T> {
        if !numeric_protocol(value, pyo3::ffi::Py_nb_index) {
            return Err(argument_input_error(
                pyo3::exceptions::PyTypeError::new_err(
                    "参数必须支持整数协议 (Argument must support the integer protocol)",
                ),
                field,
                write,
            ));
        }
        extract_argument(value.py(), value, field, write)
    }

    fn float_value(
        value: &Bound<'_, PyAny>,
        field: &str,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> PyResult<f32> {
        let numeric = numeric_protocol(value, pyo3::ffi::Py_nb_index)
            || numeric_protocol(value, pyo3::ffi::Py_nb_float);
        if !numeric {
            return Err(argument_input_error(
                pyo3::exceptions::PyTypeError::new_err(
                    "参数必须支持数值协议 (Argument must support a numeric protocol)",
                ),
                field,
                write,
            ));
        }
        extract_argument(value.py(), value, field, write)
    }

    fn string_value(
        value: &Bound<'_, PyAny>,
        field: &str,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> PyResult<String> {
        if !value.is_instance_of::<PyString>() {
            return Err(argument_input_error(
                pyo3::exceptions::PyTypeError::new_err(
                    "参数必须是字符串 (Argument must be a string)",
                ),
                field,
                write,
            ));
        }
        value
            .extract()
            .map_err(|error| argument_input_error(error, field, write))
    }

    fn write_input_error(error: PyErr, field: &str) -> PyErr {
        argument_input_error(
            error,
            field,
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        )
    }

    fn argument_input_error(
        error: PyErr,
        field: &str,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> PyErr {
        let input =
            crate::TriviumError::InvalidInput("参数无法转换 (Argument conversion failed)".into())
                .at_field(field);
        attach_error_info(
            error,
            match write {
                Some(write) => input.with_write_info(write),
                None => input,
            },
        )
    }

    fn read_id(value: &Bound<'_, PyAny>, field: &str) -> PyResult<u64> {
        integer_value(value, field, None)
    }

    fn write_id(value: &Bound<'_, PyAny>, field: &str) -> PyResult<u64> {
        integer_value(
            value,
            field,
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        )
    }

    fn read_float(value: &Bound<'_, PyAny>, field: &str) -> PyResult<f32> {
        float_value(value, field, None)
    }

    fn write_float(value: &Bound<'_, PyAny>, field: &str) -> PyResult<f32> {
        float_value(
            value,
            field,
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        )
    }

    fn read_string(value: &Bound<'_, PyAny>, field: &str) -> PyResult<String> {
        string_value(value, field, None)
    }

    fn write_string(value: &Bound<'_, PyAny>, field: &str) -> PyResult<String> {
        string_value(
            value,
            field,
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        )
    }

    fn write_list<'py>(value: &Bound<'py, PyAny>, field: &str) -> PyResult<Bound<'py, PyList>> {
        value
            .clone()
            .downcast_into::<PyList>()
            .map_err(|error| write_input_error(error.into(), field))
    }

    fn write_sequence<T>(
        value: &Bound<'_, PyAny>,
        field: &str,
        parse: impl Fn(&Bound<'_, PyAny>, &str) -> PyResult<T>,
    ) -> PyResult<Vec<T>> {
        sequence_value(
            value,
            field,
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
            parse,
        )
    }

    fn read_sequence<T>(
        value: &Bound<'_, PyAny>,
        field: &str,
        parse: impl Fn(&Bound<'_, PyAny>, &str) -> PyResult<T>,
    ) -> PyResult<Vec<T>> {
        sequence_value(value, field, None, parse)
    }

    fn sequence_value<T>(
        value: &Bound<'_, PyAny>,
        field: &str,
        write: Option<crate::error::WriteErrorInfo>,
        parse: impl Fn(&Bound<'_, PyAny>, &str) -> PyResult<T>,
    ) -> PyResult<Vec<T>> {
        // SAFETY: Bound 保持对象存活且 GIL 覆盖检测；只调用 CPython 协议检测，不解引用裸内存。
        let sequence = unsafe { pyo3::ffi::PySequence_Check(value.as_ptr()) != 0 };
        if !sequence || value.is_instance_of::<PyString>() {
            return Err(argument_input_error(
                pyo3::exceptions::PyTypeError::new_err(
                    "参数必须是非字符串序列 (Argument must be a non-string sequence)",
                ),
                field,
                write,
            ));
        }
        // 复用 PyO3 的长度提示容错规则；按真实元素增长，避免相信宿主提供的巨大提示。
        let _ = value.len();
        let mut result = Vec::new();
        for (index, item) in value.try_iter()?.enumerate() {
            let item = item?;
            let path = crate::sdk::field_member(field, &index.to_string());
            let parsed = parse(&item, &path)?;
            result.try_reserve(1).map_err(|error| {
                let error = crate::TriviumError::CapacityAllocationFailed {
                    reason: format!(
                        "参数缓冲分配失败 (Argument buffer allocation failed): {error}"
                    ),
                }
                .at_field(&path);
                to_py_error(match write {
                    Some(write) => error.with_write_info(write),
                    None => error,
                })
            })?;
            result.push(parsed);
        }
        Ok(result)
    }

    fn collect_prepared_parameters(
        parameters: &Bound<'_, PyAny>,
        prepared: &crate::query::tql_prepared::PreparedTql,
    ) -> PyResult<std::collections::HashMap<String, crate::query::tql_prepared::TqlParamValue>>
    {
        let py = parameters.py();
        let mut values = std::collections::HashMap::new();
        let mut insert = |key: Bound<'_, PyAny>, value: Bound<'_, PyAny>| -> PyResult<()> {
            let name = key.extract::<String>().map_err(|error| {
                attach_error_info(
                    error,
                    crate::TriviumError::InvalidInput(
                        "参数映射键必须是字符串 (Parameter mapping keys must be strings)".into(),
                    )
                    .at_field("parameters"),
                )
            })?;
            let json =
                pyobject_to_json(&value).map_err(|error| {
                    attach_error_info(
                    error,
                    prepared.locate_parameter_error(crate::TriviumError::InvalidInput(
                        "参数值无法转换为 JSON (Parameter value cannot be converted to JSON)"
                            .into(),
                    )
                    .at_query_parameter(&name)),
                )
                })?;
            let value = prepared
                .parameter_from_json(&name, &json)
                .map_err(to_py_error)?;
            values.insert(name, value);
            Ok(())
        };
        if let Ok(dictionary) = parameters.downcast::<PyDict>() {
            for (key, value) in dictionary.iter() {
                insert(key, value)?;
            }
        } else {
            let mapping = py.import("collections.abc")?.getattr("Mapping")?;
            if !parameters.is_instance(&mapping)? {
                let reason =
                    "Prepared TQL 参数必须是映射 (Prepared TQL parameters must be a mapping)";
                return Err(attach_error_info(
                    pyo3::exceptions::PyTypeError::new_err(reason),
                    crate::TriviumError::InvalidInput(reason.into()).at_field("parameters"),
                ));
            }
            // Mapping 的 items、迭代与取值可执行宿主代码，异常沿原路径传播。
            for pair in parameters.call_method0("items")?.try_iter()? {
                let (key, value) = pair?.extract::<(Bound<'_, PyAny>, Bound<'_, PyAny>)>()?;
                insert(key, value)?;
            }
        }
        Ok(values)
    }

    fn tql_row_to_json<T: crate::VectorType>(
        row: std::collections::HashMap<String, crate::query::tql_executor::TqlValue<T>>,
    ) -> serde_json::Value {
        use crate::query::tql_executor::TqlValue;
        let mut output = serde_json::Map::new();
        for (name, value) in row {
            let value = match value {
                TqlValue::Node(node) => serde_json::json!({
                    "id": node.id,
                    "payload": node.payload,
                    "num_edges": node.edges.len(),
                }),
                TqlValue::Edge(edge) => serde_json::json!({
                    "type": "edge",
                    "source": edge.source_id,
                    "target": edge.target_id,
                    "label": edge.label,
                    "weight": edge.weight,
                    "metadata": edge.metadata,
                }),
                TqlValue::Int(value) => serde_json::json!(value),
                TqlValue::UInt(value) => serde_json::json!(value),
                TqlValue::Float(value) => serde_json::json!(value),
                TqlValue::String(value) => serde_json::json!(value),
                TqlValue::Bool(value) => serde_json::json!(value),
                TqlValue::Path(value) => serde_json::json!(value),
                TqlValue::List(value) => serde_json::Value::Array(value),
                TqlValue::Object(value) => serde_json::Value::Object(value),
                TqlValue::Null => serde_json::Value::Null,
            };
            output.insert(name, value);
        }
        serde_json::Value::Object(output)
    }

    #[pyclass(name = "PreparedTql")]
    pub struct PyPreparedTql {
        inner: crate::query::tql_prepared::PreparedTql,
    }

    fn parse_row_overflow(value: &str) -> PyResult<crate::database::RowOverflowPolicy> {
        match value {
            "throw" => Ok(crate::database::RowOverflowPolicy::Throw),
            "break" => Ok(crate::database::RowOverflowPolicy::Break),
            _ => Err(pyo3::exceptions::PyValueError::new_err(
                "row_overflow 必须是 throw / break",
            )),
        }
    }

    #[pymethods]
    impl PyPreparedTql {
        fn parameter_names(&self) -> Vec<String> {
            self.inner
                .parameter_names()
                .into_iter()
                .map(str::to_owned)
                .collect()
        }
    }

    /// Python 侧的 TriviumDB 包装器
    #[pyclass(name = "TriviumDB")]
    pub struct PyTriviumDB {
        inner: DbBackend,
        dtype: String,
    }

    // 在 PyO3 借用前登记请求：同线程重入与其他线程的借用争用分别返回稳定类别。
    // 请求作用域只保留对象身份；Bound 和 PyRef 保证对象在作用域内存活。
    fn database_read<'py>(
        database: &Bound<'py, PyTriviumDB>,
    ) -> PyResult<(crate::sdk::RequestScope, PyRef<'py, PyTriviumDB>)> {
        let request =
            crate::sdk::RequestScope::enter(database.as_ptr() as usize, "python_callback")
                .map_err(to_py_error)?;
        let borrowed = database.try_borrow().map_err(|_| {
            to_py_error(crate::TriviumError::DatabaseBusy {
                resource: "python_database",
            })
        })?;
        Ok((request, borrowed))
    }

    fn database_write<'py>(
        database: &Bound<'py, PyTriviumDB>,
    ) -> PyResult<(crate::sdk::RequestScope, PyRefMut<'py, PyTriviumDB>)> {
        let request =
            crate::sdk::RequestScope::enter(database.as_ptr() as usize, "python_callback")
                .map_err(to_py_error)?;
        let borrowed = database.try_borrow_mut().map_err(|_| {
            to_py_error(crate::TriviumError::DatabaseBusy {
                resource: "python_database",
            })
        })?;
        Ok((request, borrowed))
    }

    macro_rules! dispatch {
        ($self:expr, $db:ident => $expr:expr) => {
            match &$self.inner {
                DbBackend::F32($db) => $expr,
                DbBackend::F16($db) => $expr,
                DbBackend::U64($db) => $expr,
            }
        };
        ($self:expr, mut $db:ident => $expr:expr) => {
            match &mut $self.inner {
                DbBackend::F32($db) => $expr,
                DbBackend::F16($db) => $expr,
                DbBackend::U64($db) => $expr,
            }
        };
    }

    fn to_py_reachability(
        py: Python<'_>,
        result: crate::graph::reachability::ReachabilityResult,
    ) -> PyResult<PyReachabilityResult> {
        Ok(PyReachabilityResult {
            source_id: result.source_id,
            target_id: result.target_id,
            depth: result.depth,
            path: result.path,
            steps: result
                .steps
                .into_iter()
                .map(|step| {
                    Ok(PyReachabilityStep {
                        from_id: step.from,
                        to_id: step.to,
                        label: step.label,
                        weight: round_api_f32(step.weight),
                        metadata: json_to_pyobject(py, &step.metadata)?,
                    })
                })
                .collect::<PyResult<Vec<_>>>()?,
        })
    }

    /// Python 侧的查询命中结果
    #[pyclass(name = "SearchHit")]
    pub struct PySearchHit {
        #[pyo3(get)]
        pub id: u64,
        #[pyo3(get)]
        pub score: f32,
        #[pyo3(get)]
        pub payload: PyObject,
    }

    #[pyclass(name = "GroupedSearchResult")]
    pub struct PyGroupedSearchResult {
        #[pyo3(get)]
        pub semantic_hits: PyObject,
        #[pyo3(get)]
        pub graph_hits: PyObject,
    }

    #[pyclass(name = "ReachabilityStep")]
    pub struct PyReachabilityStep {
        #[pyo3(get)]
        pub from_id: u64,
        #[pyo3(get)]
        pub to_id: u64,
        #[pyo3(get)]
        pub label: String,
        #[pyo3(get)]
        pub weight: f64,
        #[pyo3(get)]
        pub metadata: PyObject,
    }

    impl Clone for PyReachabilityStep {
        fn clone(&self) -> Self {
            Python::with_gil(|py| Self {
                from_id: self.from_id,
                to_id: self.to_id,
                label: self.label.clone(),
                weight: self.weight,
                metadata: self.metadata.clone_ref(py),
            })
        }
    }

    #[pyclass(name = "ReachabilityResult")]
    pub struct PyReachabilityResult {
        #[pyo3(get)]
        pub source_id: u64,
        #[pyo3(get)]
        pub target_id: u64,
        #[pyo3(get)]
        pub depth: usize,
        #[pyo3(get)]
        pub path: Vec<u64>,
        #[pyo3(get)]
        pub steps: Vec<PyReachabilityStep>,
    }

    #[pyclass(name = "Edge")]
    pub struct PyEdge {
        #[pyo3(get)]
        pub target_id: u64,
        #[pyo3(get)]
        pub label: String,
        #[pyo3(get)]
        pub weight: f64,
        #[pyo3(get)]
        pub metadata: PyObject,
    }

    impl Clone for PyEdge {
        fn clone(&self) -> Self {
            Python::with_gil(|py| Self {
                target_id: self.target_id,
                label: self.label.clone(),
                weight: self.weight,
                metadata: self.metadata.clone_ref(py),
            })
        }
    }

    #[pyclass(name = "IncomingEdge")]
    pub struct PyIncomingEdge {
        #[pyo3(get)]
        pub source_id: u64,
        #[pyo3(get)]
        pub target_id: u64,
        #[pyo3(get)]
        pub label: String,
        #[pyo3(get)]
        pub weight: f64,
        #[pyo3(get)]
        pub metadata: PyObject,
    }

    fn round_api_f32(value: f32) -> f64 {
        ((value as f64) * 1_000_000.0).round() / 1_000_000.0
    }

    fn search_hit_to_python(py: Python<'_>, hit: crate::node::SearchHit) -> PyResult<PySearchHit> {
        Ok(PySearchHit {
            id: hit.id,
            score: hit.score,
            payload: json_to_pyobject(py, &hit.payload)?,
        })
    }

    /// Python 侧的节点完整视图
    #[pyclass(name = "NodeView")]
    pub struct PyNodeView {
        #[pyo3(get)]
        pub id: u64,
        #[pyo3(get)]
        pub vector: PyObject, // 可能是 f32/f16(透传给py仍是float)/u64
        #[pyo3(get)]
        pub payload: PyObject,
        #[pyo3(get)]
        pub edges: Vec<PyEdge>,
        #[pyo3(get)]
        pub num_edges: usize,
    }

    /// Python 侧的 Cypher 查询单行结果
    /// 每一行是一个变量名 -> 节点视图的映射
    /// 例如: MATCH (a)-[:knows]->(b) RETURN a, b
    /// 则 row.get("a") 和 row.get("b") 各返回对应的节点
    #[pyclass(name = "QueryRow")]
    pub struct PyQueryRow {
        /// 变量名 -> (id, payload_dict)
        #[pyo3(get)]
        pub row: PyObject,
    }

    /// Hook 管线执行上下文（包含各阶段计时统计和自定义数据）
    #[pyclass(name = "HookContext")]
    pub struct PyHookContext {
        /// 各管线阶段的耗时统计（阶段名 → 耗时微秒数）
        #[pyo3(get)]
        pub timings: PyObject,
        /// 每阶段候选数量
        #[pyo3(get)]
        pub counts: PyObject,
        /// Hook 注入的自定义数据
        #[pyo3(get)]
        pub custom_data: PyObject,
        #[pyo3(get)]
        pub observations: PyObject,
        /// 管线是否被 Hook 提前终止
        #[pyo3(get)]
        pub aborted: bool,
    }

    #[pymethods]
    impl PyHookContext {
        fn __repr__(&self, py: Python<'_>) -> String {
            format!(
                "HookContext(aborted={}, timings={:?})",
                self.aborted,
                self.timings
                    .bind(py)
                    .repr()
                    .map(|r| r.to_string())
                    .unwrap_or_default()
            )
        }
    }

    #[pymethods]
    impl PyQueryRow {
        fn __repr__(&self, py: Python<'_>) -> String {
            format!(
                "QueryRow({:?})",
                self.row
                    .bind(py)
                    .repr()
                    .map(|r| r.to_string())
                    .unwrap_or_default()
            )
        }
    }

    // ════════ 辅助转换 ════════

    fn json_to_pyobject(py: Python<'_>, val: &serde_json::Value) -> PyResult<PyObject> {
        use pyo3::IntoPyObjectExt;
        match val {
            serde_json::Value::Null => Ok(py.None()),
            serde_json::Value::Bool(b) => b.into_py_any(py),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    i.into_py_any(py)
                } else if let Some(u) = n.as_u64() {
                    u.into_py_any(py)
                } else {
                    n.as_f64()
                        .ok_or_else(|| {
                            PyValueError::new_err("JSON 数值无效 (Invalid JSON number)")
                        })?
                        .into_py_any(py)
                }
            }
            serde_json::Value::String(s) => s.into_py_any(py),
            serde_json::Value::Array(arr) => {
                let values = arr
                    .iter()
                    .map(|v| json_to_pyobject(py, v))
                    .collect::<PyResult<Vec<_>>>()?;
                Ok(PyList::new(py, values)?.into_any().unbind())
            }
            serde_json::Value::Object(map) => {
                let dict = PyDict::new(py);
                for (k, v) in map {
                    dict.set_item(k, json_to_pyobject(py, v)?)?;
                }
                Ok(dict.into_any().unbind())
            }
        }
    }

    fn pyobject_to_json(obj: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
        pyobject_to_json_at(obj, None, None)
    }

    fn write_json(obj: &Bound<'_, PyAny>, field: &str) -> PyResult<serde_json::Value> {
        pyobject_to_json_at(
            obj,
            Some(field),
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        )
    }

    fn read_json(obj: &Bound<'_, PyAny>, field: &str) -> PyResult<serde_json::Value> {
        pyobject_to_json_at(obj, Some(field), None)
    }

    fn pyobject_to_json_at(
        obj: &Bound<'_, PyAny>,
        field: Option<&str>,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> PyResult<serde_json::Value> {
        fn convert(
            obj: &Bound<'_, PyAny>,
            depth: usize,
            field: Option<&str>,
            write: Option<crate::error::WriteErrorInfo>,
        ) -> PyResult<serde_json::Value> {
            let result = (|| {
                use pyo3::exceptions::{PyOverflowError, PyTypeError};
                use pyo3::types::{PyBool, PyFloat, PyInt, PyString, PyTuple};
                if obj.is_none() {
                    return Ok(serde_json::Value::Null);
                }
                if obj.is_instance_of::<PyBool>() {
                    return Ok(obj.extract::<bool>()?.into());
                }
                if obj.is_instance_of::<PyInt>() {
                    if let Ok(value) = obj.extract::<i64>() {
                        return Ok(value.into());
                    }
                    return obj
                        .extract::<u64>()
                        .map(serde_json::Value::from)
                        .map_err(|_| {
                            PyOverflowError::new_err(
                                "JSON 整数超出 i64/u64 范围 (JSON integer exceeds i64/u64 range)",
                            )
                        });
                }
                if obj.is_instance_of::<PyFloat>() {
                    return serde_json::Number::from_f64(obj.extract()?)
                        .map(serde_json::Value::Number)
                        .ok_or_else(|| {
                            PyValueError::new_err(
                                "JSON 浮点数必须有限 (JSON floats must be finite)",
                            )
                        });
                }
                if obj.is_instance_of::<PyString>() {
                    return Ok(obj.extract::<String>()?.into());
                }
                if depth >= 127 {
                    return Err(PyValueError::new_err(
                        "JSON 容器嵌套超过 127 层 (JSON container nesting exceeds 127 levels)",
                    ));
                }
                if let Ok(dict) = obj.downcast::<PyDict>() {
                    let mut map = serde_json::Map::new();
                    for (k, v) in dict.iter() {
                        if !k.is_instance_of::<PyString>() {
                            return Err(PyTypeError::new_err(
                                "JSON 对象键必须是字符串 (JSON object keys must be strings)",
                            ));
                        }
                        let name = k.extract::<String>()?;
                        let path = field.map(|field| crate::sdk::field_member(field, &name));
                        map.insert(name, convert(&v, depth + 1, path.as_deref(), write)?);
                    }
                    return Ok(serde_json::Value::Object(map));
                }
                let values = if let Ok(list) = obj.downcast::<PyList>() {
                    list.iter()
                        .enumerate()
                        .map(|(index, v)| {
                            let path = field
                                .map(|field| crate::sdk::field_member(field, &index.to_string()));
                            convert(&v, depth + 1, path.as_deref(), write)
                        })
                        .collect::<PyResult<Vec<_>>>()?
                } else if let Ok(tuple) = obj.downcast::<PyTuple>() {
                    tuple
                        .iter()
                        .enumerate()
                        .map(|(index, v)| {
                            let path = field
                                .map(|field| crate::sdk::field_member(field, &index.to_string()));
                            convert(&v, depth + 1, path.as_deref(), write)
                        })
                        .collect::<PyResult<Vec<_>>>()?
                } else {
                    return Err(PyTypeError::new_err(
                        "Payload 包含不支持的类型 (Payload contains an unsupported type)",
                    ));
                };
                Ok(serde_json::Value::Array(values))
            })();
            result.map_err(|error: PyErr| match field {
                Some(field) if !error.value(obj.py()).hasattr("target").unwrap_or(false) => {
                    argument_input_error(error, field, write)
                }
                _ => error,
            })
        }
        convert(obj, 0, field, write)
    }

    use crate::filter::Filter;

    fn mapping_dictionary<'py>(
        value: &Bound<'py, PyAny>,
        field: &str,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let py = value.py();
        if let Ok(dictionary) = value.downcast::<PyDict>() {
            Ok(dictionary.clone())
        } else {
            let mapping = py.import("collections.abc")?.getattr("Mapping")?;
            if !value.is_instance(&mapping)? {
                let reason = "参数必须是映射 (Argument must be a mapping)";
                return Err(argument_input_error(
                    pyo3::exceptions::PyTypeError::new_err(reason),
                    field,
                    write,
                ));
            }
            // 映射协议在已有数据库请求借用内执行，宿主异常保留原对象。
            Ok(py
                .get_type::<PyDict>()
                .call1((value,))?
                .downcast_into::<PyDict>()?)
        }
    }

    fn mapping_to_filter(_py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Filter> {
        let dictionary = mapping_dictionary(value, "payload_filter", None)?;
        let json = read_json(dictionary.as_any(), "payload_filter")?;
        Filter::from_json(&json).map_err(|reason| {
            to_py_error(crate::TriviumError::InvalidInput(reason).at_field("payload_filter"))
        })
    }

    fn parse_sync_mode(s: &str) -> PyResult<crate::storage::wal::SyncMode> {
        crate::storage::wal::SyncMode::parse(s).map_err(pyo3::exceptions::PyValueError::new_err)
    }

    fn parse_access_mode(s: &str) -> PyResult<crate::database::AccessMode> {
        match s {
            "read_write" => Ok(crate::database::AccessMode::ReadWrite),
            "read_only" => Ok(crate::database::AccessMode::ReadOnly),
            "immutable" => Ok(crate::database::AccessMode::Immutable),
            _ => Err(pyo3::exceptions::PyValueError::new_err(
                "access_mode 必须是 read_write / read_only / immutable",
            )),
        }
    }

    fn parse_missing_index_policy(value: &str) -> PyResult<crate::database::MissingIndexPolicy> {
        match value {
            "fallback" => Ok(crate::database::MissingIndexPolicy::Fallback),
            "build_in_memory" => Ok(crate::database::MissingIndexPolicy::BuildInMemory),
            "error" => Ok(crate::database::MissingIndexPolicy::Error),
            _ => Err(pyo3::exceptions::PyValueError::new_err(
                "missing_index_policy 必须是 fallback / build_in_memory / error",
            )),
        }
    }

    fn parse_edge_direction(value: &str) -> PyResult<crate::database::EdgeDirection> {
        match value {
            "out" | "outgoing" => Ok(crate::database::EdgeDirection::Outgoing),
            "in" | "incoming" => Ok(crate::database::EdgeDirection::Incoming),
            "both" => Ok(crate::database::EdgeDirection::Both),
            _ => Err(argument_input_error(
                pyo3::exceptions::PyValueError::new_err(
                    "edge_direction 必须是 out / in / both (edge_direction must be out / in / both)",
                ),
                "edge_direction",
                None,
            )),
        }
    }

    #[pymethods]
    impl PyTriviumDB {
        #[getter]
        fn dtype(slf: &Bound<'_, Self>) -> PyResult<String> {
            let (_request, borrowed) = database_read(slf)?;
            Ok(borrowed.dtype.clone())
        }

        #[new]
        #[pyo3(signature = (*args, **kwargs), text_signature="(path, dim=1536, dtype='f32', sync_mode='normal', load_text_index=False, auto_build_quiver=True, expected_nodes=None, memory_limit_mb=0, payload_cache_mb=64, payload_cache_entry_mb=8, access_mode='read_write', missing_index_policy='fallback', max_query_rows=None, row_overflow='throw', runtime=None)")]
        fn new(
            py: Python<'_>,
            args: &Bound<'_, PyTuple>,
            kwargs: Option<&Bound<'_, PyDict>>,
        ) -> PyResult<Self> {
            let values = constructor_arguments(py, "TriviumDB", args, kwargs)?;
            let path = constructor_argument(&values, "path")?;
            let dim = constructor_argument(&values, "dim")?.unbind();
            let dtype = constructor_argument(&values, "dtype")?.unbind();
            let sync_mode = constructor_argument(&values, "sync_mode")?.unbind();
            let load_text_index = constructor_argument(&values, "load_text_index")?.unbind();
            let auto_build_quiver = constructor_argument(&values, "auto_build_quiver")?.unbind();
            let expected_nodes: Option<Bound<'_, PyAny>> =
                constructor_argument(&values, "expected_nodes")?.extract()?;
            let memory_limit_mb = constructor_argument(&values, "memory_limit_mb")?.unbind();
            let payload_cache_mb = constructor_argument(&values, "payload_cache_mb")?.unbind();
            let payload_cache_entry_mb =
                constructor_argument(&values, "payload_cache_entry_mb")?.unbind();
            let access_mode = constructor_argument(&values, "access_mode")?.unbind();
            let missing_index_policy =
                constructor_argument(&values, "missing_index_policy")?.unbind();
            let max_query_rows: Option<Bound<'_, PyAny>> =
                constructor_argument(&values, "max_query_rows")?.extract()?;
            let row_overflow = constructor_argument(&values, "row_overflow")?.unbind();
            let runtime = constructor_argument(&values, "runtime")?.extract()?;
            let path: String = extract_argument(py, &path, "path", None)?;
            let dim = extract_argument(py, dim.bind(py), "dim", None)?;
            let dtype: String = extract_argument(py, dtype.bind(py), "dtype", None)?;
            let sync_mode: String = extract_argument(py, sync_mode.bind(py), "sync_mode", None)?;
            let load_text_index =
                extract_argument(py, load_text_index.bind(py), "load_text_index", None)?;
            let auto_build_quiver =
                extract_argument(py, auto_build_quiver.bind(py), "auto_build_quiver", None)?;
            let expected_nodes = expected_nodes
                .map(|value| extract_argument(py, &value, "expected_nodes", None))
                .transpose()?;
            let memory_limit_mb =
                extract_argument(py, memory_limit_mb.bind(py), "memory_limit_mb", None)?;
            let payload_cache_mb =
                extract_argument(py, payload_cache_mb.bind(py), "payload_cache_mb", None)?;
            let payload_cache_entry_mb = extract_argument(
                py,
                payload_cache_entry_mb.bind(py),
                "payload_cache_entry_mb",
                None,
            )?;
            let access_mode: String =
                extract_argument(py, access_mode.bind(py), "access_mode", None)?;
            let missing_index_policy: String = extract_argument(
                py,
                missing_index_policy.bind(py),
                "missing_index_policy",
                None,
            )?;
            let max_query_rows = max_query_rows
                .map(|value| extract_argument(py, &value, "max_query_rows", None))
                .transpose()?;
            let row_overflow: String =
                extract_argument(py, row_overflow.bind(py), "row_overflow", None)?;
            let sm = parse_sync_mode(&sync_mode)
                .map_err(|error| constructor_input_error(error, "sync_mode"))?;
            let memory_limit = constructor_mib(memory_limit_mb, "memory_limit_mb")?;
            let payload_cache_bytes = constructor_mib(payload_cache_mb, "payload_cache_mb")?;
            let payload_cache_entry_bytes =
                constructor_mib(payload_cache_entry_mb, "payload_cache_entry_mb")?;
            let config = crate::database::Config {
                dim,
                sync_mode: sm,
                load_text_index,
                auto_build_quiver,
                expected_nodes,
                memory_limit,
                payload_cache_bytes,
                payload_cache_entry_bytes,
                access_mode: parse_access_mode(&access_mode)
                    .map_err(|error| constructor_input_error(error, "access_mode"))?,
                missing_index_policy: parse_missing_index_policy(&missing_index_policy)
                    .map_err(|error| constructor_input_error(error, "missing_index_policy"))?,
                max_query_rows,
                row_overflow: parse_row_overflow(&row_overflow)
                    .map_err(|error| constructor_input_error(error, "row_overflow"))?,
                ..Default::default()
            };
            let runtime = parse_python_runtime(runtime, None)?;
            let inner = match dtype.as_str() {
                "f32" => DbBackend::F32(
                    GenericDatabase::<f32>::open_with_runtime(&path, config, runtime.clone())
                        .map_err(to_py_error)?,
                ),
                "f16" => DbBackend::F16(
                    GenericDatabase::<half::f16>::open_with_runtime(&path, config, runtime.clone())
                        .map_err(to_py_error)?,
                ),
                "u64" => DbBackend::U64(
                    GenericDatabase::<u64>::open_with_runtime(&path, config, runtime.clone())
                        .map_err(to_py_error)?,
                ),
                _ => {
                    return Err(constructor_input_error(
                        pyo3::exceptions::PyValueError::new_err(
                            "dtype 必须是 f32 / f16 / u64 (dtype must be f32 / f16 / u64)",
                        ),
                        "dtype",
                    ));
                }
            };
            Ok(Self {
                inner,
                dtype: dtype.to_string(),
            })
        }

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

        // ════════ Hook 管理 ════════

        /// 加载 C/C++ 动态库作为检索管线 Hook
        ///
        /// 动态库需要导出以下 C ABI 符号（均为可选）：
        /// - `trivium_recall`: 自定义召回
        /// - `trivium_rerank`: 自定义重排序
        ///
        /// 示例：
        /// ```python
        /// db.load_ffi_hook("./libmy_plugin.so")
        /// results = db.search(query_vec)  # 自动经过 C++ Hook
        /// ```
        fn load_ffi_hook(slf: &Bound<'_, Self>, lib_path: Bound<'_, PyAny>) -> PyResult<()> {
            let (_request, mut borrowed) = database_write(slf)?;
            let this = &mut *borrowed;
            let lib_path = read_string(&lib_path, "lib_path")?;
            let ffi_hook = crate::hook::FfiHook::load(&lib_path).map_err(to_py_error)?;
            dispatch!(this, mut db => db.set_hook(ffi_hook)).map_err(to_py_error)
        }

        /// 清除当前已注册的 Hook，恢复为默认的零开销 NoopHook
        fn clear_hook(slf: &Bound<'_, Self>) -> PyResult<()> {
            let (_request, mut borrowed) = database_write(slf)?;
            let this = &mut *borrowed;
            dispatch!(this, mut db => db.clear_hook());
            Ok(())
        }

        /// 注册一个 Python 原生 Hook 对象
        ///
        /// Python 类只需实现感兴趣的方法（鸭子类型，全部可选）：
        /// - on_pre_search(self, query_vector, ctx) -> Optional[list[float]]
        /// - on_post_recall(self, hits, ctx) -> Optional[list[dict]]
        /// - on_rerank(self, hits, ctx) -> Optional[list[dict]]
        /// - on_post_search(self, hits, ctx) -> Optional[list[dict]]
        ///
        /// 示例：
        /// ```python
        /// class MyHook:
        ///     def on_post_recall(self, hits, ctx):
        ///         return [h for h in hits if h["score"] > 0.5]
        ///
        /// db.set_hook(MyHook())
        /// ```
        fn set_hook(slf: &Bound<'_, Self>, hook: PyObject) -> PyResult<()> {
            let (_request, mut borrowed) = database_write(slf)?;
            let this = &mut *borrowed;
            let wrapper = PySearchHookWrapper {
                py_hook: hook,
                database_identity: slf.as_ptr() as usize,
            };
            dispatch!(this, mut db => db.set_hook(wrapper)).map_err(to_py_error)
        }

        fn __traverse__(
            &self,
            visit: pyo3::PyVisit<'_>,
        ) -> std::result::Result<(), pyo3::PyTraverseError> {
            // GC 遍历不获取 GIL，也不等待正在关闭的 Hook 槽。
            dispatch!(self, db => db.try_with_hook(|hook| {
                // 原生查询仍持有 Hook 时，该引用属于 GC 图之外的活动根。
                if std::sync::Arc::strong_count(hook) == 1
                    && let Some(wrapper) = (hook.as_ref() as &dyn std::any::Any).downcast_ref::<PySearchHookWrapper>()
                {
                    visit.call(&wrapper.py_hook)?;
                }
                Ok(())
            })).unwrap_or(Ok(()))
        }

        fn __clear__(&mut self) {
            dispatch!(self, mut db => db.clear_hook());
        }

        /// 带 Hook 上下文的检索：返回 (hits, context)
        ///
        /// 除了返回检索结果外，同时返回 HookContext 对象，
        /// 其中包含管线各阶段的计时统计和 Hook 注入的自定义数据。
        ///
        /// 示例：
        /// ```python
        /// hits, ctx = db.search_with_context(query_vec, top_k=10)
        /// print(ctx.timings)   # {'hook_pre_search': 0.1, 'graph_expand': 2.3, ...}
        /// print(ctx.custom_data)  # Hook 注入的自定义数据
        /// ```
        #[pyo3(signature = (query_vector, top_k=integer_argument(5), recall_k=integer_argument(0), rerank_k=integer_argument(0), expand_depth=integer_argument(2), min_score=float_argument(0.1), payload_filter=None), text_signature="($self, query_vector, top_k=5, recall_k=0, rerank_k=0, expand_depth=2, min_score=0.1, payload_filter=None)")]
        fn search_with_context(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            query_vector: Bound<'_, PyAny>,
            top_k: Py<PyAny>,
            recall_k: Py<PyAny>,
            rerank_k: Py<PyAny>,
            expand_depth: Py<PyAny>,
            min_score: Py<PyAny>,
            payload_filter: Option<&Bound<'_, PyAny>>,
        ) -> PyResult<(Vec<PySearchHit>, PyHookContext)> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let min_score = read_float(min_score.bind(py), "min_score")?;
            let top_k = extract_argument(py, top_k.bind(py), "top_k", None)?;
            let recall_k = extract_argument(py, recall_k.bind(py), "recall_k", None)?;
            let rerank_k = extract_argument(py, rerank_k.bind(py), "rerank_k", None)?;
            let expand_depth = extract_argument(py, expand_depth.bind(py), "expand_depth", None)?;
            let rust_filter = match payload_filter {
                Some(dict) => Some(mapping_to_filter(py, dict)?),
                None => None,
            };
            let config = crate::database::SearchConfig {
                top_k,
                recall_k,
                rerank_k,
                expand_depth,
                min_score,
                payload_filter: rust_filter,
                ..Default::default()
            };

            let (results, hook_ctx) = match &this.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                    py.allow_threads(|| db.search_hybrid_with_context(None, Some(&vec), &config))
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    py.allow_threads(|| db.search_hybrid_with_context(None, Some(&vec16), &config))
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = read_sequence(&query_vector, "query_vector", read_id)?;
                    py.allow_threads(|| db.search_hybrid_with_context(None, Some(&vec), &config))
                }
            }
            .map_err(to_py_error)?;

            // 转换搜索结果
            let hits: Vec<PySearchHit> = results
                .into_iter()
                .map(|h| {
                    Ok(PySearchHit {
                        id: h.id,
                        score: h.score,
                        payload: json_to_pyobject(py, &h.payload)?,
                    })
                })
                .collect::<PyResult<Vec<_>>>()?;

            // 转换 HookContext → PyHookContext
            let timings_dict = PyDict::new(py);
            for (stage, dur) in &hook_ctx.stage_timings {
                let _ = timings_dict.set_item(stage, dur.as_secs_f64() * 1000.0); // 转为毫秒
            }
            let counts_dict = PyDict::new(py);
            for (stage, count) in &hook_ctx.stage_counts {
                let _ = counts_dict.set_item(stage, count);
            }
            let ctx = PyHookContext {
                timings: timings_dict.into_any().unbind(),
                counts: counts_dict.into_any().unbind(),
                custom_data: json_to_pyobject(py, &hook_ctx.custom_data)?,
                observations: hook_ctx
                    .observations
                    .iter()
                    .map(|(name, value)| (name.clone(), value))
                    .collect::<std::collections::HashMap<_, _>>()
                    .into_pyobject(py)
                    .unwrap()
                    .into_any()
                    .unbind(),
                aborted: hook_ctx.abort,
            };

            Ok((hits, ctx))
        }

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

        #[pyo3(signature = (src, dst, label=string_argument("related"), weight=float_argument(1.0)), text_signature="($self, src, dst, label='related', weight=1.0)")]
        fn link(
            slf: &Bound<'_, Self>,
            src: Bound<'_, PyAny>,
            dst: Bound<'_, PyAny>,
            label: Py<PyAny>,
            weight: Py<PyAny>,
        ) -> PyResult<()> {
            let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
            let this = &mut *borrowed;
            let src = write_id(&src, "src")?;
            let dst = write_id(&dst, "dst")?;
            let label: String = write_string(label.bind(slf.py()), "label")?;
            let weight = write_float(weight.bind(slf.py()), "weight")?;
            dispatch!(this, mut db => db.link(src, dst, &label, weight)).map_err(to_py_error)
        }

        fn get_edge(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            src: Bound<'_, PyAny>,
            dst: Bound<'_, PyAny>,
            label: Bound<'_, PyAny>,
        ) -> PyResult<Option<PyEdge>> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let src = extract_argument(py, &src, "src", None)?;
            let dst = extract_argument(py, &dst, "dst", None)?;
            let label: String = extract_argument(py, &label, "label", None)?;
            Ok({
                dispatch!(this, db => db.get_edge(src, dst, &label).map_err(to_py_error)?)
                    .map(|edge| -> PyResult<PyEdge> {
                        Ok(PyEdge {
                            target_id: edge.target_id,
                            label: edge.label,
                            weight: round_api_f32(edge.weight),
                            metadata: json_to_pyobject(py, &edge.metadata)?,
                        })
                    })
                    .transpose()?
            })
        }

        #[pyo3(signature = (src, dst, label, weight=float_argument(1.0), metadata=None), text_signature="($self, src, dst, label, weight=1.0, metadata=None)")]
        fn upsert_edge(
            slf: &Bound<'_, Self>,
            src: Bound<'_, PyAny>,
            dst: Bound<'_, PyAny>,
            label: Bound<'_, PyAny>,
            weight: Py<PyAny>,
            metadata: Option<&Bound<'_, PyAny>>,
        ) -> PyResult<()> {
            let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
            let this = &mut *borrowed;
            let src = write_id(&src, "src")?;
            let dst = write_id(&dst, "dst")?;
            let label: String = write_string(&label, "label")?;
            let weight = write_float(weight.bind(slf.py()), "weight")?;
            let metadata = metadata
                .map(|value| write_json(value, "metadata"))
                .transpose()?
                .unwrap_or(serde_json::Value::Null);
            dispatch!(this, mut db => db.upsert_edge(src, dst, &label, weight, metadata))
                .map_err(to_py_error)
        }

        #[pyo3(signature = (src, dst, label, weight=None, metadata=None))]
        fn update_edge(
            slf: &Bound<'_, Self>,
            src: Bound<'_, PyAny>,
            dst: Bound<'_, PyAny>,
            label: Bound<'_, PyAny>,
            weight: Option<Bound<'_, PyAny>>,
            metadata: Option<&Bound<'_, PyAny>>,
        ) -> PyResult<()> {
            let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
            let this = &mut *borrowed;
            let src = write_id(&src, "src")?;
            let dst = write_id(&dst, "dst")?;
            let label: String = write_string(&label, "label")?;
            let weight = weight
                .map(|value| write_float(&value, "weight"))
                .transpose()?;
            dispatch!(this, mut db => db.update_edge(
                src,
                dst,
                &label,
                weight,
                metadata.map(|value| write_json(value, "metadata")).transpose()?,
            ))
            .map_err(to_py_error)
        }

        #[pyo3(signature = (query_vector, top_k=integer_argument(5), recall_k=integer_argument(0), rerank_k=integer_argument(0), expand_depth=integer_argument(0), min_score=float_argument(0.5), payload_filter=None, max_edges_per_node=integer_argument(0), min_edge_weight=float_argument(0.0), edge_direction=string_argument("out")), text_signature="($self, query_vector, top_k=5, recall_k=0, rerank_k=0, expand_depth=0, min_score=0.5, payload_filter=None, max_edges_per_node=0, min_edge_weight=0.0, edge_direction='out')")]
        fn search(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            query_vector: Bound<'_, PyAny>,
            top_k: Py<PyAny>,
            recall_k: Py<PyAny>,
            rerank_k: Py<PyAny>,
            expand_depth: Py<PyAny>,
            min_score: Py<PyAny>,
            payload_filter: Option<&Bound<'_, PyAny>>,
            max_edges_per_node: Py<PyAny>,
            min_edge_weight: Py<PyAny>,
            edge_direction: Py<PyAny>,
        ) -> PyResult<Vec<PySearchHit>> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let min_score = read_float(min_score.bind(py), "min_score")?;
            let min_edge_weight = read_float(min_edge_weight.bind(py), "min_edge_weight")?;
            let edge_direction = read_string(edge_direction.bind(py), "edge_direction")?;
            let top_k = extract_argument(py, top_k.bind(py), "top_k", None)?;
            let recall_k = extract_argument(py, recall_k.bind(py), "recall_k", None)?;
            let rerank_k = extract_argument(py, rerank_k.bind(py), "rerank_k", None)?;
            let expand_depth = extract_argument(py, expand_depth.bind(py), "expand_depth", None)?;
            let max_edges_per_node =
                extract_argument(py, max_edges_per_node.bind(py), "max_edges_per_node", None)?;
            let rust_filter = match payload_filter {
                Some(dict) => Some(mapping_to_filter(py, dict)?),
                None => None,
            };

            let config = crate::database::SearchConfig {
                top_k,
                recall_k,
                rerank_k,
                expand_depth,
                min_score,
                enable_advanced_pipeline: false,
                payload_filter: rust_filter,
                max_edges_per_node,
                min_edge_weight,
                edge_direction: parse_edge_direction(&edge_direction)?,
                ..Default::default()
            };

            let results = match &this.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                    py.allow_threads(|| db.search_hybrid(None, Some(&vec), &config))
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    py.allow_threads(|| db.search_hybrid(None, Some(&vec16), &config))
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = read_sequence(&query_vector, "query_vector", read_id)?;
                    py.allow_threads(|| db.search_hybrid(None, Some(&vec), &config))
                }
            }
            .map_err(to_py_error)?;

            results
                .into_iter()
                .map(|h| {
                    Ok(PySearchHit {
                        id: h.id,
                        score: h.score,
                        payload: json_to_pyobject(py, &h.payload)?,
                    })
                })
                .collect::<PyResult<Vec<_>>>()
        }

        #[pyo3(signature = (query_vector, top_k=integer_argument(5), recall_k=integer_argument(0), rerank_k=integer_argument(0), expand_depth=integer_argument(2), min_score=float_argument(0.1), payload_filter=None, max_edges_per_node=integer_argument(0), min_edge_weight=float_argument(0.0), edge_direction=string_argument("out")), text_signature="($self, query_vector, top_k=5, recall_k=0, rerank_k=0, expand_depth=2, min_score=0.1, payload_filter=None, max_edges_per_node=0, min_edge_weight=0.0, edge_direction='out')")]
        fn search_grouped(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            query_vector: Bound<'_, PyAny>,
            top_k: Py<PyAny>,
            recall_k: Py<PyAny>,
            rerank_k: Py<PyAny>,
            expand_depth: Py<PyAny>,
            min_score: Py<PyAny>,
            payload_filter: Option<&Bound<'_, PyAny>>,
            max_edges_per_node: Py<PyAny>,
            min_edge_weight: Py<PyAny>,
            edge_direction: Py<PyAny>,
        ) -> PyResult<PyGroupedSearchResult> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let min_score = read_float(min_score.bind(py), "min_score")?;
            let min_edge_weight = read_float(min_edge_weight.bind(py), "min_edge_weight")?;
            let edge_direction = read_string(edge_direction.bind(py), "edge_direction")?;
            let top_k = extract_argument(py, top_k.bind(py), "top_k", None)?;
            let recall_k = extract_argument(py, recall_k.bind(py), "recall_k", None)?;
            let rerank_k = extract_argument(py, rerank_k.bind(py), "rerank_k", None)?;
            let expand_depth = extract_argument(py, expand_depth.bind(py), "expand_depth", None)?;
            let max_edges_per_node =
                extract_argument(py, max_edges_per_node.bind(py), "max_edges_per_node", None)?;
            let rust_filter = payload_filter
                .map(|dict| mapping_to_filter(py, dict))
                .transpose()?;
            let config = crate::database::SearchConfig {
                top_k,
                recall_k,
                rerank_k,
                expand_depth,
                min_score,
                payload_filter: rust_filter,
                max_edges_per_node,
                min_edge_weight,
                edge_direction: parse_edge_direction(&edge_direction)?,
                ..Default::default()
            };
            let result = match &this.inner {
                DbBackend::F32(db) => {
                    let vector: Vec<f32> =
                        read_sequence(&query_vector, "query_vector", read_float)?;
                    py.allow_threads(|| db.search_hybrid_grouped(None, Some(&vector), &config))
                }
                DbBackend::F16(db) => {
                    let vector: Vec<f32> =
                        read_sequence(&query_vector, "query_vector", read_float)?;
                    let vector: Vec<half::f16> =
                        vector.into_iter().map(half::f16::from_f32).collect();
                    py.allow_threads(|| db.search_hybrid_grouped(None, Some(&vector), &config))
                }
                DbBackend::U64(db) => {
                    let vector: Vec<u64> = read_sequence(&query_vector, "query_vector", read_id)?;
                    py.allow_threads(|| db.search_hybrid_grouped(None, Some(&vector), &config))
                }
            }
            .map_err(to_py_error)?;
            Ok(PyGroupedSearchResult {
                semantic_hits: result
                    .semantic_hits
                    .into_iter()
                    .map(|hit| search_hit_to_python(py, hit))
                    .collect::<PyResult<Vec<_>>>()?
                    .into_pyobject(py)
                    .unwrap()
                    .into_any()
                    .unbind(),
                graph_hits: result
                    .graph_hits
                    .into_iter()
                    .map(|hit| search_hit_to_python(py, hit))
                    .collect::<PyResult<Vec<_>>>()?
                    .into_pyobject(py)
                    .unwrap()
                    .into_any()
                    .unbind(),
            })
        }

        #[pyo3(signature = (query_vectors, top_k=integer_argument(5), recall_k=integer_argument(0), rerank_k=integer_argument(0), expand_depth=integer_argument(0), min_score=float_argument(0.5), parallelism=integer_argument(0)), text_signature="($self, query_vectors, top_k=5, recall_k=0, rerank_k=0, expand_depth=0, min_score=0.5, parallelism=0)")]
        fn search_batch(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            query_vectors: Bound<'_, PyAny>,
            top_k: Py<PyAny>,
            recall_k: Py<PyAny>,
            rerank_k: Py<PyAny>,
            expand_depth: Py<PyAny>,
            min_score: Py<PyAny>,
            parallelism: Py<PyAny>,
        ) -> PyResult<Vec<Vec<PySearchHit>>> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let min_score = read_float(min_score.bind(py), "min_score")?;
            let top_k = extract_argument(py, top_k.bind(py), "top_k", None)?;
            let recall_k = extract_argument(py, recall_k.bind(py), "recall_k", None)?;
            let rerank_k = extract_argument(py, rerank_k.bind(py), "rerank_k", None)?;
            let expand_depth = extract_argument(py, expand_depth.bind(py), "expand_depth", None)?;
            let parallelism = extract_argument(py, parallelism.bind(py), "parallelism", None)?;
            let config = crate::database::SearchConfig {
                top_k,
                recall_k,
                rerank_k,
                expand_depth,
                min_score,
                enable_advanced_pipeline: false,
                ..Default::default()
            };
            let batch_config = crate::database::BatchSearchConfig { parallelism };
            let results = match &this.inner {
                DbBackend::F32(db) => {
                    let queries: Vec<Vec<f32>> =
                        read_sequence(&query_vectors, "query_vectors", |value, field| {
                            read_sequence(value, field, read_float)
                        })?;
                    py.allow_threads(|| db.search_batch(&queries, &config, &batch_config))
                }
                DbBackend::F16(db) => {
                    let queries: Vec<Vec<f32>> =
                        read_sequence(&query_vectors, "query_vectors", |value, field| {
                            read_sequence(value, field, read_float)
                        })?;
                    let queries: Vec<Vec<half::f16>> = queries
                        .into_iter()
                        .map(|query| query.into_iter().map(half::f16::from_f32).collect())
                        .collect();
                    py.allow_threads(|| db.search_batch(&queries, &config, &batch_config))
                }
                DbBackend::U64(db) => {
                    let queries: Vec<Vec<u64>> =
                        read_sequence(&query_vectors, "query_vectors", |value, field| {
                            read_sequence(value, field, read_id)
                        })?;
                    py.allow_threads(|| db.search_batch(&queries, &config, &batch_config))
                }
            }
            .map_err(to_py_error)?;

            results
                .into_iter()
                .map(|hits| {
                    hits.into_iter()
                        .map(|hit| {
                            Ok(PySearchHit {
                                id: hit.id,
                                score: hit.score,
                                payload: json_to_pyobject(py, &hit.payload)?,
                            })
                        })
                        .collect::<PyResult<Vec<_>>>()
                })
                .collect::<PyResult<Vec<_>>>()
        }

        #[pyo3(signature = (
            query_vector,
            top_k=integer_argument(5),
            recall_k=integer_argument(0),
            rerank_k=integer_argument(0),
            expand_depth=integer_argument(2),
            min_score=float_argument(0.1),
            teleport_alpha=float_argument(0.0),
            enable_advanced_pipeline=boolean_argument(true),
            enable_sparse_residual=boolean_argument(false),
            fista_lambda=float_argument(0.1),
            fista_threshold=float_argument(0.3),
            enable_dpp=boolean_argument(false),
            dpp_quality_weight=float_argument(1.0),
            enable_refractory_fatigue=boolean_argument(false),
            enable_text_hybrid_search=boolean_argument(false),
            text_boost=float_argument(1.5),
            custom_query_text=None,
            payload_filter=None,
            force_brute_force=boolean_argument(false),
            expand_labels=None,
            max_edges_per_node=integer_argument(0),
            min_edge_weight=float_argument(0.0),
            edge_direction=string_argument("out")
        ), text_signature="($self, query_vector, top_k=5, recall_k=0, rerank_k=0, expand_depth=2, min_score=0.1, teleport_alpha=0.0, enable_advanced_pipeline=True, enable_sparse_residual=False, fista_lambda=0.1, fista_threshold=0.3, enable_dpp=False, dpp_quality_weight=1.0, enable_refractory_fatigue=False, enable_text_hybrid_search=False, text_boost=1.5, custom_query_text=None, payload_filter=None, force_brute_force=False, expand_labels=None, max_edges_per_node=0, min_edge_weight=0.0, edge_direction='out')")]
        fn search_advanced(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            query_vector: Bound<'_, PyAny>,
            top_k: Py<PyAny>,
            recall_k: Py<PyAny>,
            rerank_k: Py<PyAny>,
            expand_depth: Py<PyAny>,
            min_score: Py<PyAny>,
            teleport_alpha: Py<PyAny>,
            enable_advanced_pipeline: Py<PyAny>,
            enable_sparse_residual: Py<PyAny>,
            fista_lambda: Py<PyAny>,
            fista_threshold: Py<PyAny>,
            enable_dpp: Py<PyAny>,
            dpp_quality_weight: Py<PyAny>,
            enable_refractory_fatigue: Py<PyAny>,
            enable_text_hybrid_search: Py<PyAny>,
            text_boost: Py<PyAny>,
            custom_query_text: Option<Bound<'_, PyAny>>,
            payload_filter: Option<&Bound<'_, PyAny>>,
            force_brute_force: Py<PyAny>,
            expand_labels: Option<Bound<'_, PyAny>>,
            max_edges_per_node: Py<PyAny>,
            min_edge_weight: Py<PyAny>,
            edge_direction: Py<PyAny>,
        ) -> PyResult<Vec<PySearchHit>> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let min_score = read_float(min_score.bind(py), "min_score")?;
            let teleport_alpha = read_float(teleport_alpha.bind(py), "teleport_alpha")?;
            let fista_lambda = read_float(fista_lambda.bind(py), "fista_lambda")?;
            let fista_threshold = read_float(fista_threshold.bind(py), "fista_threshold")?;
            let dpp_quality_weight = read_float(dpp_quality_weight.bind(py), "dpp_quality_weight")?;
            let text_boost = read_float(text_boost.bind(py), "text_boost")?;
            let min_edge_weight = read_float(min_edge_weight.bind(py), "min_edge_weight")?;
            let enable_advanced_pipeline = extract_argument(
                py,
                enable_advanced_pipeline.bind(py),
                "enable_advanced_pipeline",
                None,
            )?;
            let enable_sparse_residual = extract_argument(
                py,
                enable_sparse_residual.bind(py),
                "enable_sparse_residual",
                None,
            )?;
            let enable_dpp = extract_argument(py, enable_dpp.bind(py), "enable_dpp", None)?;
            let enable_refractory_fatigue = extract_argument(
                py,
                enable_refractory_fatigue.bind(py),
                "enable_refractory_fatigue",
                None,
            )?;
            let enable_text_hybrid_search = extract_argument(
                py,
                enable_text_hybrid_search.bind(py),
                "enable_text_hybrid_search",
                None,
            )?;
            let force_brute_force =
                extract_argument(py, force_brute_force.bind(py), "force_brute_force", None)?;
            let edge_direction = read_string(edge_direction.bind(py), "edge_direction")?;
            let custom_query_text = custom_query_text
                .map(|value| read_string(&value, "custom_query_text"))
                .transpose()?;
            let expand_labels = expand_labels
                .map(|value| read_sequence(&value, "expand_labels", read_string))
                .transpose()?;
            let top_k = extract_argument(py, top_k.bind(py), "top_k", None)?;
            let recall_k = extract_argument(py, recall_k.bind(py), "recall_k", None)?;
            let rerank_k = extract_argument(py, rerank_k.bind(py), "rerank_k", None)?;
            let expand_depth = extract_argument(py, expand_depth.bind(py), "expand_depth", None)?;
            let max_edges_per_node =
                extract_argument(py, max_edges_per_node.bind(py), "max_edges_per_node", None)?;
            // 解析 payload_filter（类 MongoDB 语法的 dict -> Rust Filter）
            let rust_filter = match payload_filter {
                Some(dict) => Some(mapping_to_filter(py, dict)?),
                None => None,
            };

            let config = crate::database::SearchConfig {
                top_k,
                recall_k,
                rerank_k,
                expand_depth,
                min_score,
                teleport_alpha,
                enable_advanced_pipeline,
                enable_sparse_residual,
                fista_lambda,
                fista_threshold,
                enable_dpp,
                dpp_quality_weight,
                enable_refractory_fatigue,
                enable_text_hybrid_search,
                text_boost,
                force_brute_force,
                expand_labels,
                max_edges_per_node,
                min_edge_weight,
                edge_direction: parse_edge_direction(&edge_direction)?,
                payload_filter: rust_filter,
                ..Default::default()
            };

            let q_text = custom_query_text.as_deref();

            let results = match &this.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                    py.allow_threads(|| db.search_hybrid(q_text, Some(&vec), &config))
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    py.allow_threads(|| db.search_hybrid(q_text, Some(&vec16), &config))
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = read_sequence(&query_vector, "query_vector", read_id)?;
                    py.allow_threads(|| db.search_hybrid(q_text, Some(&vec), &config))
                }
            }
            .map_err(to_py_error)?;

            results
                .into_iter()
                .map(|h| {
                    Ok(PySearchHit {
                        id: h.id,
                        score: h.score,
                        payload: json_to_pyobject(py, &h.payload)?,
                    })
                })
                .collect::<PyResult<Vec<_>>>()
        }

        #[pyo3(signature = (query_vector, query_text, top_k=integer_argument(5), expand_depth=integer_argument(2), min_score=float_argument(0.1), hybrid_alpha=float_argument(0.7), payload_filter=None), text_signature="($self, query_vector, query_text, top_k=5, expand_depth=2, min_score=0.1, hybrid_alpha=0.7, payload_filter=None)")]
        fn search_hybrid(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            query_vector: Bound<'_, PyAny>,
            query_text: Bound<'_, PyAny>,
            top_k: Py<PyAny>,
            expand_depth: Py<PyAny>,
            min_score: Py<PyAny>,
            hybrid_alpha: Py<PyAny>,
            payload_filter: Option<&Bound<'_, PyAny>>,
        ) -> PyResult<Vec<PySearchHit>> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let min_score = read_float(min_score.bind(py), "min_score")?;
            let hybrid_alpha = read_float(hybrid_alpha.bind(py), "hybrid_alpha")?;
            let query_text: String = extract_argument(py, &query_text, "query_text", None)?;
            let top_k = extract_argument(py, top_k.bind(py), "top_k", None)?;
            let expand_depth = extract_argument(py, expand_depth.bind(py), "expand_depth", None)?;
            let rust_filter = match payload_filter {
                Some(dict) => Some(mapping_to_filter(py, dict)?),
                None => None,
            };

            // hybrid_alpha 越大，向量分数占比越高。
            // TriviumDB 底层使用 text_boost = (1.0 - alpha) * 2.5 作为启发式倍率
            let boost = crate::sdk::hybrid_text_boost(hybrid_alpha).map_err(to_py_error)?;
            let config = crate::database::SearchConfig {
                top_k,
                expand_depth,
                min_score,
                enable_text_hybrid_search: true,
                text_boost: boost,
                payload_filter: rust_filter,
                ..Default::default()
            };
            let query_text = query_text.to_owned();
            let results = match &this.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                    py.allow_threads(|| db.search_hybrid(Some(&query_text), Some(&vec), &config))
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    py.allow_threads(|| db.search_hybrid(Some(&query_text), Some(&vec16), &config))
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = read_sequence(&query_vector, "query_vector", read_id)?;
                    py.allow_threads(|| db.search_hybrid(Some(&query_text), Some(&vec), &config))
                }
            }
            .map_err(to_py_error)?;
            results
                .into_iter()
                .map(|h| {
                    Ok(PySearchHit {
                        id: h.id,
                        score: h.score,
                        payload: json_to_pyobject(py, &h.payload)?,
                    })
                })
                .collect::<PyResult<Vec<_>>>()
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

        #[pyo3(signature = (src, dst, label=None))]
        fn unlink(
            slf: &Bound<'_, Self>,
            src: Bound<'_, PyAny>,
            dst: Bound<'_, PyAny>,
            label: Option<Bound<'_, PyAny>>,
        ) -> PyResult<()> {
            let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
            let this = &mut *borrowed;
            let src = write_id(&src, "src")?;
            let dst = write_id(&dst, "dst")?;
            let label: Option<String> = label
                .map(|value| write_string(&value, "label"))
                .transpose()?;
            match label.as_deref() {
                Some(label) => dispatch!(this, mut db => db.unlink_label(src, dst, label)),
                None => dispatch!(this, mut db => db.unlink(src, dst)),
            }
            .map_err(to_py_error)
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
            dispatch!(this, mut db => db.create_unique_composite_index(&fields))
                .map_err(to_py_error)
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
                    .map_err(|error| {
                        pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
                    })?;
            json_to_pyobject(py, &value)
        }

        fn storage_info(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let value = dispatch!(this, db => serde_json::to_value(db.storage_info()))
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
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

        /// 获取节点的出边列表
        fn get_edges(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            id: Bound<'_, PyAny>,
        ) -> PyResult<Vec<PyEdge>> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let id = extract_argument(py, &id, "id", None)?;
            Ok({
                dispatch!(this, db => db.get_edges(id).map_err(to_py_error)?)
                    .into_iter()
                    .map(|e| {
                        Ok(PyEdge {
                            target_id: e.target_id,
                            label: e.label,
                            weight: round_api_f32(e.weight),
                            metadata: json_to_pyobject(py, &e.metadata)?,
                        })
                    })
                    .collect::<PyResult<Vec<_>>>()?
            })
        }

        #[pyo3(signature = (id, label=None))]
        fn get_incoming_edges(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            id: Bound<'_, PyAny>,
            label: Option<&Bound<'_, PyAny>>,
        ) -> PyResult<Vec<PyIncomingEdge>> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let id = extract_argument(py, &id, "id", None)?;
            let label: Option<String> = label
                .map(|value| extract_argument(py, value, "label", None))
                .transpose()?;
            Ok({
                dispatch!(this, db => db.get_incoming_edges(id, label.as_deref()).map_err(to_py_error)?)
                    .into_iter()
                    .map(|edge| {
                        Ok(PyIncomingEdge {
                            source_id: edge.source_id,
                            target_id: edge.target_id,
                            label: edge.label,
                            weight: round_api_f32(edge.weight),
                            metadata: json_to_pyobject(py, &edge.metadata)?,
                        })
                    })
                    .collect::<PyResult<Vec<_>>>()?
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

        #[pyo3(signature = (id, depth=integer_argument(1), labels=None), text_signature="($self, id, depth=1, labels=None)")]
        fn neighbors(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            id: Bound<'_, PyAny>,
            depth: Py<PyAny>,
            labels: Option<&Bound<'_, PyAny>>,
        ) -> PyResult<Vec<u64>> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let id = extract_argument(py, &id, "id", None)?;
            let depth = extract_argument(py, depth.bind(py), "depth", None)?;
            let labels: Option<Vec<String>> = labels
                .map(|value| read_sequence(value, "labels", read_string))
                .transpose()?;
            Ok({
                dispatch!(this, db => db.neighbors_with_labels(id, depth, labels.as_deref()).map_err(to_py_error)?)
            })
        }

        #[pyo3(signature = (id, min_depth=integer_argument(1), max_depth=integer_argument(1), labels=None, direction=string_argument("outgoing"), max_visited_nodes=integer_argument(10_000), max_results=integer_argument(10_000), max_edges=integer_argument(50_000)), text_signature="($self, id, min_depth=1, max_depth=1, labels=None, direction='outgoing', max_visited_nodes=10000, max_results=10000, max_edges=50000)")]
        fn reachable(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            id: Bound<'_, PyAny>,
            min_depth: Py<PyAny>,
            max_depth: Py<PyAny>,
            labels: Option<&Bound<'_, PyAny>>,
            direction: Py<PyAny>,
            max_visited_nodes: Py<PyAny>,
            max_results: Py<PyAny>,
            max_edges: Py<PyAny>,
        ) -> PyResult<Vec<PyReachabilityResult>> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let id = extract_argument(py, &id, "id", None)?;
            let min_depth = extract_argument(py, min_depth.bind(py), "min_depth", None)?;
            let max_depth = extract_argument(py, max_depth.bind(py), "max_depth", None)?;
            let labels = labels
                .map(|value| read_sequence(value, "labels", read_string))
                .transpose()?;
            let direction: String = extract_argument(py, direction.bind(py), "direction", None)?;
            let max_visited_nodes =
                extract_argument(py, max_visited_nodes.bind(py), "max_visited_nodes", None)?;
            let max_results = extract_argument(py, max_results.bind(py), "max_results", None)?;
            let max_edges = extract_argument(py, max_edges.bind(py), "max_edges", None)?;
            let direction = match direction.as_str() {
                "outgoing" => crate::graph::reachability::ReachabilityDirection::Outgoing,
                "incoming" => crate::graph::reachability::ReachabilityDirection::Incoming,
                "both" => crate::graph::reachability::ReachabilityDirection::Both,
                _ => {
                    let reason = "direction 必须是 outgoing / incoming / both (direction must be outgoing / incoming / both)";
                    return Err(attach_error_info(
                        pyo3::exceptions::PyValueError::new_err(reason),
                        crate::TriviumError::InvalidInput(reason.into()).at_field("direction"),
                    ));
                }
            };
            let config = crate::graph::reachability::ReachabilityConfig {
                min_depth,
                max_depth,
                labels,
                direction,
                max_visited_nodes,
                max_results,
                max_edges,
                max_frontier_size: 10_000,
                exhaustion_policy: crate::graph::budget::BudgetExhaustionPolicy::Partial,
            };
            dispatch!(this, db => db.reachable(id, &config))
                .map(|results| {
                    results
                        .into_iter()
                        .map(|result| to_py_reachability(py, result))
                        .collect::<PyResult<Vec<_>>>()
                })
                .map_err(to_py_error)?
        }

        #[pyo3(signature = (id, min_depth=integer_argument(1), max_depth=integer_argument(1), labels=None, direction=string_argument("outgoing"), max_visited_nodes=integer_argument(10_000), max_results=integer_argument(10_000), max_edges=integer_argument(50_000)), text_signature="($self, id, min_depth=1, max_depth=1, labels=None, direction='outgoing', max_visited_nodes=10000, max_results=10000, max_edges=50000)")]
        fn reachable_detailed(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            id: Bound<'_, PyAny>,
            min_depth: Py<PyAny>,
            max_depth: Py<PyAny>,
            labels: Option<&Bound<'_, PyAny>>,
            direction: Py<PyAny>,
            max_visited_nodes: Py<PyAny>,
            max_results: Py<PyAny>,
            max_edges: Py<PyAny>,
        ) -> PyResult<PyObject> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let id = extract_argument(py, &id, "id", None)?;
            let min_depth = extract_argument(py, min_depth.bind(py), "min_depth", None)?;
            let max_depth = extract_argument(py, max_depth.bind(py), "max_depth", None)?;
            let labels = labels
                .map(|value| read_sequence(value, "labels", read_string))
                .transpose()?;
            let direction: String = extract_argument(py, direction.bind(py), "direction", None)?;
            let max_visited_nodes =
                extract_argument(py, max_visited_nodes.bind(py), "max_visited_nodes", None)?;
            let max_results = extract_argument(py, max_results.bind(py), "max_results", None)?;
            let max_edges = extract_argument(py, max_edges.bind(py), "max_edges", None)?;
            let direction = match direction.as_str() {
                "outgoing" => crate::graph::reachability::ReachabilityDirection::Outgoing,
                "incoming" => crate::graph::reachability::ReachabilityDirection::Incoming,
                "both" => crate::graph::reachability::ReachabilityDirection::Both,
                _ => {
                    let reason = "direction 必须是 outgoing / incoming / both (direction must be outgoing / incoming / both)";
                    return Err(attach_error_info(
                        pyo3::exceptions::PyValueError::new_err(reason),
                        crate::TriviumError::InvalidInput(reason.into()).at_field("direction"),
                    ));
                }
            };
            let config = crate::graph::reachability::ReachabilityConfig {
                min_depth,
                max_depth,
                labels,
                direction,
                max_visited_nodes,
                max_results,
                max_edges,
                max_frontier_size: 10_000,
                exhaustion_policy: crate::graph::budget::BudgetExhaustionPolicy::Partial,
            };
            let output =
                dispatch!(this, db => db.reachable_detailed(id, &config)).map_err(to_py_error)?;
            let value = serde_json::to_value(output)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
        }

        #[pyo3(signature = (id, min_depth=integer_argument(1), max_depth=integer_argument(1), labels=None, direction=string_argument("outgoing"), max_visited_nodes=integer_argument(10_000), max_results=integer_argument(10_000), max_edges=integer_argument(50_000)), text_signature="($self, id, min_depth=1, max_depth=1, labels=None, direction='outgoing', max_visited_nodes=10000, max_results=10000, max_edges=50000)")]
        fn query_subgraph(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            id: Bound<'_, PyAny>,
            min_depth: Py<PyAny>,
            max_depth: Py<PyAny>,
            labels: Option<&Bound<'_, PyAny>>,
            direction: Py<PyAny>,
            max_visited_nodes: Py<PyAny>,
            max_results: Py<PyAny>,
            max_edges: Py<PyAny>,
        ) -> PyResult<PyObject> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let id = extract_argument(py, &id, "id", None)?;
            let min_depth = extract_argument(py, min_depth.bind(py), "min_depth", None)?;
            let max_depth = extract_argument(py, max_depth.bind(py), "max_depth", None)?;
            let labels = labels
                .map(|value| read_sequence(value, "labels", read_string))
                .transpose()?;
            let direction: String = extract_argument(py, direction.bind(py), "direction", None)?;
            let max_visited_nodes =
                extract_argument(py, max_visited_nodes.bind(py), "max_visited_nodes", None)?;
            let max_results = extract_argument(py, max_results.bind(py), "max_results", None)?;
            let max_edges = extract_argument(py, max_edges.bind(py), "max_edges", None)?;
            let direction = match direction.as_str() {
                "outgoing" => crate::graph::reachability::ReachabilityDirection::Outgoing,
                "incoming" => crate::graph::reachability::ReachabilityDirection::Incoming,
                "both" => crate::graph::reachability::ReachabilityDirection::Both,
                _ => {
                    let reason = "direction 必须是 outgoing / incoming / both (direction must be outgoing / incoming / both)";
                    return Err(attach_error_info(
                        pyo3::exceptions::PyValueError::new_err(reason),
                        crate::TriviumError::InvalidInput(reason.into()).at_field("direction"),
                    ));
                }
            };
            let config = crate::graph::reachability::ReachabilityConfig {
                min_depth,
                max_depth,
                labels,
                direction,
                max_visited_nodes,
                max_results,
                max_edges,
                max_frontier_size: 10_000,
                exhaustion_policy: crate::graph::budget::BudgetExhaustionPolicy::Partial,
            };
            let output =
                dispatch!(this, db => db.query_subgraph(id, &config)).map_err(to_py_error)?;
            let value = serde_json::to_value(output)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
        }

        #[pyo3(signature = (query_vector, anchor_ids, top_k, max_anchor_nodes=integer_argument(100_000)), text_signature="($self, query_vector, anchor_ids, top_k, max_anchor_nodes=100000)")]
        fn search_graph_first(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            query_vector: Bound<'_, PyAny>,
            anchor_ids: Bound<'_, PyAny>,
            top_k: Bound<'_, PyAny>,
            max_anchor_nodes: Py<PyAny>,
        ) -> PyResult<Vec<PySearchHit>> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;

            let anchor_ids = read_sequence(&anchor_ids, "anchor_ids", read_id)?;
            let top_k = extract_argument(py, &top_k, "top_k", None)?;
            let max_anchor_nodes =
                extract_argument(py, max_anchor_nodes.bind(py), "max_anchor_nodes", None)?;
            let hits = match &this.inner {
                DbBackend::F32(db) => {
                    let query: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                    db.search_graph_first(&query, &anchor_ids, top_k, max_anchor_nodes)
                }
                DbBackend::F16(db) => {
                    let query: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                    let query: Vec<half::f16> =
                        query.into_iter().map(half::f16::from_f32).collect();
                    db.search_graph_first(&query, &anchor_ids, top_k, max_anchor_nodes)
                }
                DbBackend::U64(db) => {
                    let query: Vec<u64> = read_sequence(&query_vector, "query_vector", read_id)?;
                    db.search_graph_first(&query, &anchor_ids, top_k, max_anchor_nodes)
                }
            }
            .map_err(to_py_error)?;
            hits.into_iter()
                .map(|hit| {
                    Ok(PySearchHit {
                        id: hit.id,
                        score: hit.score,
                        payload: json_to_pyobject(py, &hit.payload)?,
                    })
                })
                .collect::<PyResult<Vec<_>>>()
        }

        #[pyo3(signature = (query_vector, top_k))]
        fn search_exact(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            query_vector: Bound<'_, PyAny>,
            top_k: Bound<'_, PyAny>,
        ) -> PyResult<Vec<PySearchHit>> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;

            let top_k = extract_argument(py, &top_k, "top_k", None)?;
            let hits = match &this.inner {
                DbBackend::F32(db) => {
                    let query: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                    db.search_exact(&query, top_k)
                }
                DbBackend::F16(db) => {
                    let query: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                    let query: Vec<half::f16> =
                        query.into_iter().map(half::f16::from_f32).collect();
                    db.search_exact(&query, top_k)
                }
                DbBackend::U64(db) => {
                    let query: Vec<u64> = read_sequence(&query_vector, "query_vector", read_id)?;
                    db.search_exact(&query, top_k)
                }
            }
            .map_err(to_py_error)?;
            hits.into_iter()
                .map(|hit| {
                    Ok(PySearchHit {
                        id: hit.id,
                        score: hit.score,
                        payload: json_to_pyobject(py, &hit.payload)?,
                    })
                })
                .collect::<PyResult<Vec<_>>>()
        }

        fn graph_stats(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let value =
                serde_json::to_value(dispatch!(this, db => db.graph_stats().map_err(to_py_error)?))
                    .map_err(|error| {
                        pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
                    })?;
            json_to_pyobject(py, &value)
        }

        fn validate_graph(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let value = serde_json::to_value(
                dispatch!(this, db => db.validate_graph().map_err(to_py_error)?),
            )
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
        }

        fn repair_graph_indexes(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
            let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
            let this = &mut *borrowed;
            let report =
                dispatch!(this, mut db => db.repair_graph_indexes()).map_err(to_py_error)?;
            let value = serde_json::to_value(report)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
        }

        fn node_count(slf: &Bound<'_, Self>) -> PyResult<usize> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            Ok(dispatch!(this, db => db.node_count().map_err(to_py_error)?))
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
            let manifest =
                dispatch!(this, mut db => db.publish_generation_manifest(&generation_id))
                    .map_err(to_py_error)?;
            let value = serde_json::to_value(manifest)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
        }

        fn dim(slf: &Bound<'_, Self>) -> PyResult<usize> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            Ok(dispatch!(this, db => db.dim()))
        }

        /// 获取所有活跃节点的 ID 列表
        fn all_node_ids(slf: &Bound<'_, Self>) -> PyResult<Vec<u64>> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            Ok(dispatch!(this, db => db.all_node_ids().map_err(to_py_error)?))
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
                let reason =
                    "内存上限换算字节时溢出 (Memory limit overflows when converted to bytes)";
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

        fn __len__(slf: &Bound<'_, Self>) -> PyResult<usize> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            dispatch!(this, db => db.node_count()).map_err(to_py_error)
        }

        fn __contains__(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            id: Bound<'_, PyAny>,
        ) -> PyResult<bool> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let id = extract_argument(py, &id, "id", None)?;
            Ok(dispatch!(this, db => db.contains(id).map_err(to_py_error)?))
        }

        fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            Ok(format!(
                "TriviumDB(dtype={}, nodes={}, dim={})",
                this.dtype,
                dispatch!(this, db => db.node_count())
                    .map(|count| count.to_string())
                    .unwrap_or_else(|_| "已关闭".into()),
                dispatch!(this, db => db.dim())
            ))
        }

        fn __enter__(slf: Py<Self>) -> Py<Self> {
            slf
        }

        #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
        fn __exit__(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            _exc_type: Option<&Bound<'_, PyAny>>,
            _exc_val: Option<&Bound<'_, PyAny>>,
            _exc_tb: Option<&Bound<'_, PyAny>>,
        ) -> PyResult<bool> {
            Self::close(slf, py)?;
            Ok(false)
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
                        let vec16: Vec<half::f16> =
                            vec.into_iter().map(half::f16::from_f32).collect();
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
                        let vec16: Vec<half::f16> =
                            vec.into_iter().map(half::f16::from_f32).collect();
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

        /// 执行 TQL (Trivium Query Language) 统一查询
        ///
        /// 支持三种入口：MATCH (图遍历) / FIND (文档过滤) / SEARCH (向量检索)
        ///
        /// 示例：
        /// ```python
        /// # 图遍历
        /// rows = db.tql('MATCH (a)-[:knows]->(b) WHERE b.age > 18 RETURN b')
        /// for row in rows:
        ///     node = row.row["b"]   # {"id": ..., "payload": {...}}
        ///
        /// # 文档过滤
        /// rows = db.tql('FIND {type: "event", heat: {$gte: 0.7}} RETURN *')
        /// ```
        fn tql(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            query: Bound<'_, PyAny>,
        ) -> PyResult<Vec<PyQueryRow>> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let query: String = extract_argument(py, &query, "query", None)?;
            fn convert_rows<T: crate::VectorType>(
                py: Python<'_>,
                rows: crate::query::tql_executor::TqlValueResult<T>,
            ) -> PyResult<Vec<PyQueryRow>> {
                use crate::query::tql_executor::TqlValue;
                let mut out = Vec::with_capacity(rows.len());
                for row in rows {
                    let py_row = PyDict::new(py);
                    for (name, value) in row {
                        match value {
                            TqlValue::Node(node) => {
                                let node_dict = PyDict::new(py);
                                let _ = node_dict.set_item("id", node.id);
                                let _ = node_dict
                                    .set_item("payload", json_to_pyobject(py, &node.payload)?);
                                let _ = node_dict.set_item("num_edges", node.edges.len());
                                let _ = py_row.set_item(name, node_dict);
                            }
                            TqlValue::Edge(edge) => {
                                let edge_dict = PyDict::new(py);
                                let _ = edge_dict.set_item("type", "edge");
                                let _ = edge_dict.set_item("source", edge.source_id);
                                let _ = edge_dict.set_item("target", edge.target_id);
                                let _ = edge_dict.set_item("label", edge.label);
                                let _ = edge_dict.set_item("weight", edge.weight);
                                let _ = edge_dict
                                    .set_item("metadata", json_to_pyobject(py, &edge.metadata)?);
                                let _ = py_row.set_item(name, edge_dict);
                            }
                            TqlValue::Int(value) => {
                                let _ = py_row.set_item(name, value);
                            }
                            TqlValue::UInt(value) => {
                                py_row.set_item(name, value)?;
                            }
                            TqlValue::Float(value) => {
                                let _ = py_row.set_item(name, value);
                            }
                            TqlValue::String(value) => {
                                let _ = py_row.set_item(name, value);
                            }
                            TqlValue::Bool(value) => {
                                let _ = py_row.set_item(name, value);
                            }
                            TqlValue::Path(value) => {
                                let _ = py_row.set_item(name, value);
                            }
                            TqlValue::List(value) => {
                                let _ = py_row.set_item(
                                    name,
                                    json_to_pyobject(py, &serde_json::Value::Array(value))?,
                                );
                            }
                            TqlValue::Object(value) => {
                                py_row.set_item(
                                    name,
                                    json_to_pyobject(py, &serde_json::Value::Object(value))?,
                                )?;
                            }
                            TqlValue::Null => {
                                let _ = py_row.set_item(name, py.None());
                            }
                        }
                    }
                    out.push(PyQueryRow {
                        row: py_row.into_any().unbind(),
                    });
                }
                Ok(out)
            }

            match &this.inner {
                DbBackend::F32(db) => {
                    let rows = db.tql_values(&query).map_err(to_py_error)?;
                    convert_rows(py, rows)
                }
                DbBackend::F16(db) => {
                    let rows = db.tql_values(&query).map_err(to_py_error)?;
                    convert_rows(py, rows)
                }
                DbBackend::U64(db) => {
                    let rows = db.tql_values(&query).map_err(to_py_error)?;
                    convert_rows(py, rows)
                }
            }
        }

        fn prepare_tql(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            query: Bound<'_, PyAny>,
        ) -> PyResult<PyPreparedTql> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let query: String = extract_argument(py, &query, "query", None)?;
            dispatch!(this, db => db.prepare_tql(&query))
                .map(|inner| PyPreparedTql { inner })
                .map_err(to_py_error)
        }

        fn execute_prepared_tql(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            prepared: Bound<'_, PyAny>,
            parameters: &Bound<'_, PyAny>,
        ) -> PyResult<Vec<PyQueryRow>> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let prepared = prepared
                .extract::<PyRef<'_, PyPreparedTql>>()
                .map_err(|error| argument_input_error(error, "prepared", None))?;
            let values = collect_prepared_parameters(parameters, &prepared.inner)?;
            let rows = match &this.inner {
                DbBackend::F32(db) => db
                    .execute_prepared_tql(&prepared.inner, &values)
                    .map(|rows| rows.into_iter().map(tql_row_to_json).collect::<Vec<_>>()),
                DbBackend::F16(db) => db
                    .execute_prepared_tql(&prepared.inner, &values)
                    .map(|rows| rows.into_iter().map(tql_row_to_json).collect::<Vec<_>>()),
                DbBackend::U64(db) => db
                    .execute_prepared_tql(&prepared.inner, &values)
                    .map(|rows| rows.into_iter().map(tql_row_to_json).collect::<Vec<_>>()),
            }
            .map_err(to_py_error)?;
            rows.into_iter()
                .map(|row| {
                    let value = json_to_pyobject(py, &row)?;
                    Ok(PyQueryRow { row: value })
                })
                .collect()
        }

        /// 文档JSON适配复用Core的解析、来源映射、分批发布、修订、检索与证据上下文。
        fn document_request(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            request_json: Bound<'_, PyAny>,
        ) -> PyResult<String> {
            let (_request, mut borrowed) = database_write(slf)?;
            let this = &mut *borrowed;
            let request_json = read_string(&request_json, "request_json")?;
            py.allow_threads(
                || dispatch!(this, mut db => crate::sdk::documents::request(db, &request_json)),
            )
            .map_err(to_py_error)
        }

        /// SSD运行时JSON适配复用Core的事实绑定、驻留、缓存与I/O生命周期。
        fn quiver_request(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            request_json: Bound<'_, PyAny>,
        ) -> PyResult<String> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let request_json = read_string(&request_json, "request_json")?;
            py.allow_threads(
                || dispatch!(this, db => crate::sdk::quiver::request(db, &request_json)),
            )
            .map_err(to_py_error)
        }

        /// 命名图JSON适配复用Core的成员、独立边、修订及作用域契约。
        fn graph_request(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            request_json: Bound<'_, PyAny>,
        ) -> PyResult<String> {
            let (_request, mut borrowed) = database_write(slf)?;
            let this = &mut *borrowed;
            let request_json = read_string(&request_json, "request_json")?;
            py.allow_threads(
                || dispatch!(this, mut db => crate::sdk::named_graphs::request(db, &request_json)),
            )
            .map_err(to_py_error)
        }

        /// 原样传输 JSON 字符串，避免宿主 JavaScript 对 u64 的中间降精度。
        fn console_request(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            session: Bound<'_, PyAny>,
            request_json: Bound<'_, PyAny>,
        ) -> PyResult<String> {
            let (_request, mut borrowed) = database_write(slf)?;
            let this = &mut *borrowed;
            let session = session
                .extract::<PyRef<'_, PyConsoleSession>>()
                .map_err(|error| argument_input_error(error, "session", None))?;
            let session = session.inner.clone();
            let request_json = read_string(&request_json, "request_json")?;
            py.allow_threads(
                || dispatch!(this, mut db => db.console_request(&session, &request_json)),
            )
            .map_err(to_py_error)
        }

        /// 执行 TQL 写操作（CREATE / SET / DELETE / DETACH DELETE）
        ///
        /// 返回 dict: {"affected": int, "created_ids": list[int]}
        ///
        /// 示例：
        /// ```python
        /// result = db.tql_mut('CREATE (a {name: "Alice", age: 30})')
        /// print(result["affected"])      # 1
        /// print(result["created_ids"])   # [1]
        ///
        /// db.tql_mut('MATCH (a {name: "Alice"}) SET a.age == 31')
        /// db.tql_mut('MATCH (a {name: "Alice"}) DELETE a')
        /// ```
        fn tql_mut(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            query: Bound<'_, PyAny>,
        ) -> PyResult<PyObject> {
            let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
            let this = &mut *borrowed;
            let query: String = extract_management_argument(py, &query, "query")?;
            let result = dispatch!(this, mut db => db.tql_mut(&query)).map_err(to_py_error)?;
            let dict = PyDict::new(py);
            let _ = dict.set_item("affected", result.affected);
            let created: Vec<u64> = result.created_ids;
            let _ = dict.set_item("created_ids", created);
            Ok(dict.into_any().unbind())
        }

        // ════════ Leiden 社区检测 ════════

        /// Leiden 社区聚类
        ///
        /// 基于图谱边结构进行 Leiden/Louvain 近似社区发现。
        /// 返回一个字典，包含:
        /// - communities: list[list[int]] — 每个社区的节点 ID 列表
        /// - centroids: dict[int, list[float]] — 社区质心向量（可选）
        /// - num_clusters: int — 发现的社区总数
        ///
        /// 示例：
        /// ```python
        /// result = db.leiden_cluster(min_community_size=3, max_iterations=15)
        /// for community in result["communities"]:
        ///     print(f"社区: {community}")
        /// ```
        #[pyo3(signature = (min_community_size=integer_argument(3), max_iterations=integer_argument(15), compute_centroids=boolean_argument(true)), text_signature="($self, min_community_size=3, max_iterations=15, compute_centroids=True)")]
        fn leiden_cluster(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            min_community_size: Py<PyAny>,
            max_iterations: Py<PyAny>,
            compute_centroids: Py<PyAny>,
        ) -> PyResult<PyObject> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let min_community_size =
                integer_value(min_community_size.bind(py), "min_community_size", None)?;
            let max_iterations = integer_value(max_iterations.bind(py), "max_iterations", None)?;
            let compute_centroids: bool =
                extract_argument(py, compute_centroids.bind(py), "compute_centroids", None)?;
            let result = dispatch!(this, db => db.leiden_cluster(
                min_community_size,
                Some(max_iterations),
                Some(compute_centroids),
            ))
            .map_err(to_py_error)?;

            // 按社区分组: cluster_id -> [node_ids]
            let mut clusters: std::collections::HashMap<u32, Vec<u64>> =
                std::collections::HashMap::new();
            for (&node_id, &cluster_id) in &result.node_to_cluster {
                clusters.entry(cluster_id).or_default().push(node_id);
            }

            // 排序确保确定性输出
            let mut sorted_keys: Vec<u32> = clusters.keys().copied().collect();
            sorted_keys.sort_unstable();

            let communities = PyList::new(
                py,
                sorted_keys.iter().map(|k| {
                    let mut ids = clusters.get(k).cloned().unwrap_or_default();
                    ids.sort_unstable();
                    ids
                }),
            )?;

            let centroids_dict = PyDict::new(py);
            if compute_centroids {
                for &k in &sorted_keys {
                    if let Some(centroid) = result.centroids.get(&k) {
                        let _ = centroids_dict.set_item(k, centroid.clone());
                    }
                }
            }

            let out = PyDict::new(py);
            let _ = out.set_item("communities", communities);
            let _ = out.set_item("centroids", centroids_dict);
            let _ = out.set_item("num_clusters", result.num_clusters);
            Ok(out.into_any().unbind())
        }

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

        fn request_shutdown(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
            let (_request, mut borrowed) = database_write(slf)?;
            let this = &mut *borrowed;
            let status = dispatch!(this, mut db => db.request_shutdown()).map_err(to_py_error)?;
            let value = serde_json::to_value(status)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
            json_to_pyobject(py, &value)
        }

        fn shutdown_status(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
            let (_request, borrowed) = database_read(slf)?;
            let this = &*borrowed;
            let value = serde_json::to_value(dispatch!(this, db => db.shutdown_status()))
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
            json_to_pyobject(py, &value)
        }

        fn close_timeout(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            timeout_ms: Bound<'_, PyAny>,
        ) -> PyResult<bool> {
            let (_request, mut borrowed) = database_write(slf)?;
            let this = &mut *borrowed;
            let timeout_ms: u64 = integer_value(&timeout_ms, "timeout_ms", None)?;
            py.allow_threads(|| dispatch!(this, mut db => db.close_timeout(std::time::Duration::from_millis(timeout_ms))))
                .map_err(to_py_error)
        }

        fn create_snapshot(
            slf: &Bound<'_, Self>,
            py: Python<'_>,
            destination: Bound<'_, PyAny>,
            generation_id: Bound<'_, PyAny>,
        ) -> PyResult<PyObject> {
            let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
            let this = &mut *borrowed;
            let destination = write_string(&destination, "destination")?;
            let generation_id = write_string(&generation_id, "generation_id")?;
            let manifest = py
                .allow_threads(
                    || dispatch!(this, mut db => db.create_snapshot(&destination, &generation_id)),
                )
                .map_err(to_py_error)?;
            let value = serde_json::to_value(manifest)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
            json_to_pyobject(py, &value)
        }

        /// 显式关闭数据库（落盘后释放资源）
        fn close(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<()> {
            let (_request, mut borrowed) = database_write(slf)?;
            let this = &mut *borrowed;
            match py.allow_threads(|| dispatch!(this, mut db => db.close())) {
                Ok(()) | Err(crate::error::TriviumError::DatabaseClosed) => Ok(()),
                Err(error) => Err(to_py_error(error)),
            }
        }
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
    struct PyTransaction {
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
                to_py_error(
                    error.with_write_info(crate::error::WriteErrorInfo::TRANSACTION_ADMISSION),
                )
            })?;
            let builder = self
                .builder
                .take()
                .ok_or_else(|| to_py_error(crate::TriviumError::TransactionFinished))?;
            match (&mut db_ref.inner, builder) {
                (DbBackend::F32(db), TxBuilderBackend::F32(b)) => {
                    db.commit_tx(b).map_err(to_py_error)
                }
                (DbBackend::F16(db), TxBuilderBackend::F16(b)) => {
                    db.commit_tx(b).map_err(to_py_error)
                }
                (DbBackend::U64(db), TxBuilderBackend::U64(b)) => {
                    db.commit_tx(b).map_err(to_py_error)
                }
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
            let (_request, mut this) =
                transaction_write(slf).map_err(transaction_admission_error)?;
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
            let (_request, mut this) =
                transaction_write(slf).map_err(transaction_admission_error)?;
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

    // ════════════════════════════════════════════════════════
    //  PySearchHookWrapper — Python 原生 Hook 支持
    // ════════════════════════════════════════════════════════

    /// 将 Python 对象包装为 Rust SearchHook trait 实现
    ///
    /// Python 类只需实现感兴趣的方法（鸭子类型）：
    /// - on_pre_search(self, query_vector, config, ctx) -> None
    /// - on_post_recall(self, hits, ctx) -> None
    /// - on_rerank(self, hits, ctx) -> Optional[list]
    /// - on_post_search(self, hits, ctx) -> None
    struct PySearchHookWrapper {
        py_hook: PyObject,
        // 身份只用于准入匹配；正在调用的方法持有 PyRef，保证原实例存活。
        database_identity: usize,
    }

    // SAFETY: PyObject 本身是 Send+Sync（它只是一个引用计数指针），
    // 实际的 Python 调用在 with_gil 中执行，由 GIL 保证线程安全。
    unsafe impl Send for PySearchHookWrapper {}
    unsafe impl Sync for PySearchHookWrapper {}

    impl PySearchHookWrapper {
        fn with_callback<R: Default>(
            &self,
            ctx: &mut crate::hook::HookContext,
            call: impl FnOnce(Python<'_>, &mut crate::hook::HookContext) -> R,
        ) -> R {
            let _request =
                match crate::sdk::RequestScope::callback(self.database_identity, "python_callback")
                {
                    Ok(request) => request,
                    Err(error) => {
                        ctx.error = Some(error.to_string());
                        return R::default();
                    }
                };
            Python::with_gil(|py| call(py, ctx))
        }

        fn record_python_error(ctx: &mut crate::hook::HookContext, stage: &str, error: PyErr) {
            ctx.error = Some(format!("Python Hook {stage} 执行失败: {error}"));
        }

        /// 将 Rust Vec<SearchHit> 转换为 Python list[dict]
        fn hits_to_py(py: Python<'_>, hits: &[crate::node::SearchHit]) -> PyResult<PyObject> {
            let values = hits
                .iter()
                .map(|h| -> PyResult<_> {
                    let d = PyDict::new(py);
                    d.set_item("id", h.id)?;
                    d.set_item("score", h.score)?;
                    d.set_item("payload", json_to_pyobject(py, &h.payload)?)?;
                    Ok(d)
                })
                .collect::<PyResult<Vec<_>>>()?;
            Ok(PyList::new(py, values)?.into_any().unbind())
        }

        fn py_to_hits(py: Python<'_>, obj: &PyObject) -> PyResult<Vec<crate::node::SearchHit>> {
            let list = obj.bind(py).downcast::<PyList>()?;
            list.iter()
                .map(|item| {
                    let dict = item.downcast::<PyDict>()?;
                    let required = |name: &str| {
                        dict.get_item(name)?.ok_or_else(|| {
                            PyValueError::new_err(format!(
                                "Hook 命中缺少字段 (Hook hit field missing): {name}"
                            ))
                        })
                    };
                    let id = required("id")?.extract::<u64>()?;
                    let score = required("score")?.extract::<f32>()?;
                    if id == 0 || !score.is_finite() {
                        return Err(PyValueError::new_err(
                            "Hook 命中 ID 或分数无效 (Invalid hook hit ID or score)",
                        ));
                    }
                    let payload = dict
                        .get_item("payload")?
                        .map(|v| pyobject_to_json(&v))
                        .transpose()?
                        .unwrap_or_default();
                    Ok(crate::node::SearchHit { id, score, payload })
                })
                .collect()
        }

        fn set_context_data(
            py: Python<'_>,
            dict: &Bound<'_, PyDict>,
            ctx: &mut crate::hook::HookContext,
        ) -> bool {
            match json_to_pyobject(py, &ctx.custom_data)
                .and_then(|v| dict.set_item("custom_data", v))
            {
                Ok(()) => true,
                Err(error) => {
                    Self::record_python_error(ctx, "context", error);
                    false
                }
            }
        }
    }

    impl crate::hook::SearchHook for PySearchHookWrapper {
        fn on_pre_search(
            &self,
            query_vector: &mut Vec<f32>,
            _config: &mut crate::database::SearchConfig,
            ctx: &mut crate::hook::HookContext,
        ) {
            self.with_callback(ctx, |py, ctx| {
                let hook = self.py_hook.bind(py);
                if let Ok(method) = hook.getattr("on_pre_search")
                    && let Ok(py_vec) = pyo3::types::PyList::new(py, query_vector.iter())
                {
                    let py_ctx = PyDict::new(py);
                    if !Self::set_context_data(py, &py_ctx, ctx) {
                        return;
                    }
                    let _ = py_ctx.set_item("abort", ctx.abort);

                    match method.call1((&py_vec, &py_ctx)) {
                        Ok(result) => {
                            if let Ok(new_vec) = result.extract::<Vec<f32>>() {
                                *query_vector = new_vec;
                            }
                            if let Ok(Some(abort_val)) = py_ctx.get_item("abort")
                                && let Ok(ab) = abort_val.extract::<bool>()
                            {
                                ctx.abort = ab;
                            }
                        }
                        Err(error) => Self::record_python_error(ctx, "on_pre_search", error),
                    }
                }
            });
        }

        fn on_custom_recall(
            &self,
            query_vector: &[f32],
            _config: &crate::database::SearchConfig,
            ctx: &mut crate::hook::HookContext,
        ) -> Option<Vec<crate::node::SearchHit>> {
            self.with_callback(ctx, |py, ctx| {
                let hook = self.py_hook.bind(py);
                let Ok(method) = hook.getattr("on_custom_recall") else {
                    return None;
                };
                let py_ctx = PyDict::new(py);
                if !Self::set_context_data(py, &py_ctx, ctx) {
                    return None;
                }
                match method.call1((query_vector.to_vec(), py_ctx)) {
                    Ok(result) if !result.is_none() => match Self::py_to_hits(py, &result.unbind())
                    {
                        Ok(hits) => Some(hits),
                        Err(error) => {
                            Self::record_python_error(ctx, "on_custom_recall", error);
                            None
                        }
                    },
                    Ok(_) => None,
                    Err(error) => {
                        Self::record_python_error(ctx, "on_custom_recall", error);
                        None
                    }
                }
            })
        }

        fn on_pre_graph_expand(
            &self,
            seeds: &mut Vec<crate::node::SearchHit>,
            ctx: &mut crate::hook::HookContext,
        ) {
            self.with_callback(ctx, |py, ctx| {
                let hook = self.py_hook.bind(py);
                let Ok(method) = hook.getattr("on_pre_graph_expand") else {
                    return;
                };
                let py_hits = match Self::hits_to_py(py, seeds) {
                    Ok(value) => value,
                    Err(error) => {
                        Self::record_python_error(ctx, "on_pre_graph_expand", error);
                        return;
                    }
                };
                let py_ctx = PyDict::new(py);
                if !Self::set_context_data(py, &py_ctx, ctx) {
                    return;
                }
                match method.call1((py_hits, py_ctx)) {
                    Ok(result) if !result.is_none() => {
                        match Self::py_to_hits(py, &result.unbind()) {
                            Ok(value) => *seeds = value,
                            Err(error) => {
                                Self::record_python_error(ctx, "on_pre_graph_expand", error)
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(error) => Self::record_python_error(ctx, "on_pre_graph_expand", error),
                }
            });
        }

        fn on_post_recall(
            &self,
            hits: &mut Vec<crate::node::SearchHit>,
            ctx: &mut crate::hook::HookContext,
        ) {
            self.with_callback(ctx, |py, ctx| {
                let hook = self.py_hook.bind(py);
                if let Ok(method) = hook.getattr("on_post_recall") {
                    let py_hits = match Self::hits_to_py(py, hits) {
                        Ok(value) => value,
                        Err(error) => {
                            Self::record_python_error(ctx, "on_post_recall", error);
                            return;
                        }
                    };
                    let py_ctx = PyDict::new(py);
                    if !Self::set_context_data(py, &py_ctx, ctx) {
                        return;
                    }

                    match method.call1((&py_hits, &py_ctx)) {
                        Ok(result) if !result.is_none() => {
                            let obj = result.unbind();
                            match Self::py_to_hits(py, &obj) {
                                Ok(value) => *hits = value,
                                Err(error) => {
                                    Self::record_python_error(ctx, "on_post_recall", error)
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(error) => Self::record_python_error(ctx, "on_post_recall", error),
                    }
                }
            });
        }

        fn on_rerank(
            &self,
            hits: &mut Vec<crate::node::SearchHit>,
            ctx: &mut crate::hook::HookContext,
        ) -> Option<Vec<crate::node::SearchHit>> {
            self.with_callback(ctx, |py, ctx| {
                let hook = self.py_hook.bind(py);
                if let Ok(method) = hook.getattr("on_rerank") {
                    let py_hits = match Self::hits_to_py(py, hits) {
                        Ok(value) => value,
                        Err(error) => {
                            Self::record_python_error(ctx, "on_rerank", error);
                            return None;
                        }
                    };
                    let py_ctx = PyDict::new(py);
                    if !Self::set_context_data(py, &py_ctx, ctx) {
                        return None;
                    }
                    match method.call1((&py_hits, &py_ctx)) {
                        Ok(result) if !result.is_none() => {
                            return match Self::py_to_hits(py, &result.unbind()) {
                                Ok(hits) => Some(hits),
                                Err(error) => {
                                    Self::record_python_error(ctx, "on_rerank", error);
                                    None
                                }
                            };
                        }
                        Ok(_) => {}
                        Err(error) => Self::record_python_error(ctx, "on_rerank", error),
                    }
                }
                None
            })
        }

        fn on_post_search(
            &self,
            hits: &mut Vec<crate::node::SearchHit>,
            ctx: &mut crate::hook::HookContext,
        ) {
            self.with_callback(ctx, |py, ctx| {
                let hook = self.py_hook.bind(py);
                if let Ok(method) = hook.getattr("on_post_search") {
                    let py_hits = match Self::hits_to_py(py, hits) {
                        Ok(value) => value,
                        Err(error) => {
                            Self::record_python_error(ctx, "on_post_search", error);
                            return;
                        }
                    };
                    let py_ctx = PyDict::new(py);
                    if !Self::set_context_data(py, &py_ctx, ctx) {
                        return;
                    }
                    match method.call1((&py_hits, &py_ctx)) {
                        Ok(result) if !result.is_none() => {
                            match Self::py_to_hits(py, &result.unbind()) {
                                Ok(value) => *hits = value,
                                Err(error) => {
                                    Self::record_python_error(ctx, "on_post_search", error)
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(error) => Self::record_python_error(ctx, "on_post_search", error),
                    }
                }
            });
        }
    }

    #[pyfunction]
    pub fn init_logger() {
        use tracing_subscriber::{EnvFilter, fmt};
        let _ = fmt()
            .with_env_filter(
                EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()),
            )
            .try_init();
    }

    #[pyfunction]
    #[pyo3(signature = (directory, runtime=None))]
    fn verify_snapshot(
        py: Python<'_>,
        directory: Bound<'_, PyAny>,
        runtime: Option<Bound<'_, PyAny>>,
    ) -> PyResult<PyObject> {
        let directory = read_string(&directory, "directory")?;
        let runtime = parse_python_runtime(runtime, None)?;
        let manifest = py
            .allow_threads(|| {
                crate::storage::snapshot::verify_snapshot_with_runtime(&directory, &runtime)
            })
            .map_err(to_py_error)?;
        let value = serde_json::to_value(manifest)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        json_to_pyobject(py, &value)
    }

    #[pyfunction]
    #[pyo3(signature = (source, destination, runtime=None))]
    fn restore_snapshot(
        py: Python<'_>,
        source: Bound<'_, PyAny>,
        destination: Bound<'_, PyAny>,
        runtime: Option<Bound<'_, PyAny>>,
    ) -> PyResult<PyObject> {
        let source = write_string(&source, "source")?;
        let destination = write_string(&destination, "destination")?;
        let runtime = parse_python_runtime(
            runtime,
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        )?;
        let manifest = py
            .allow_threads(|| {
                crate::storage::snapshot::restore_snapshot_with_runtime(
                    &source,
                    &destination,
                    &runtime,
                )
            })
            .map_err(to_py_error)?;
        let value = serde_json::to_value(manifest)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        json_to_pyobject(py, &value)
    }

    #[pymodule]
    pub fn triviumdb(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add("TriviumDBError", m.py().get_type::<TriviumDBError>())?;
        m.add("InvalidInputError", m.py().get_type::<InvalidInputError>())?;
        m.add("CapacityError", m.py().get_type::<CapacityError>())?;
        m.add(
            "SnapshotDestinationError",
            m.py().get_type::<SnapshotDestinationError>(),
        )?;
        m.add("ReadOnlyError", m.py().get_type::<ReadOnlyError>())?;
        m.add(
            "RecoveryRequiredError",
            m.py().get_type::<RecoveryRequiredError>(),
        )?;
        m.add(
            "ImmutableArtifactError",
            m.py().get_type::<ImmutableArtifactError>(),
        )?;
        m.add(
            "GenerationBusyError",
            m.py().get_type::<GenerationBusyError>(),
        )?;
        m.add(
            "UnsupportedDatabaseVersionError",
            m.py().get_type::<UnsupportedDatabaseVersionError>(),
        )?;
        m.add(
            "ResourceBudgetError",
            m.py().get_type::<ResourceBudgetError>(),
        )?;
        m.add(
            "SnapshotPublicationError",
            m.py().get_type::<SnapshotPublicationError>(),
        )?;
        m.add_class::<PyResourceRuntime>()?;
        m.add_class::<PyRequestContext>()?;
        m.add_class::<PyConsoleSession>()?;
        m.add_class::<PyPreparedTql>()?;
        m.add_class::<PyTriviumDB>()?;
        m.add_class::<PySearchHit>()?;
        m.add_class::<PyGroupedSearchResult>()?;
        m.add_class::<PyReachabilityStep>()?;
        m.add_class::<PyReachabilityResult>()?;
        m.add_class::<PyEdge>()?;
        m.add_class::<PyIncomingEdge>()?;
        m.add_class::<PyNodeView>()?;
        m.add_class::<PyQueryRow>()?;
        m.add_class::<PyHookContext>()?;
        m.add_class::<PyTransaction>()?;
        m.add_function(wrap_pyfunction!(init_logger, m)?)?;
        m.add_function(wrap_pyfunction!(verify_snapshot, m)?)?;
        m.add_function(wrap_pyfunction!(restore_snapshot, m)?)?;
        // 私有诊断桥接不加入公开导出目录。
        m.setattr(
            "_call_argument_error",
            wrap_pyfunction!(_call_argument_error, m)?,
        )?;
        m.setattr("_invoke_request", wrap_pyfunction!(_invoke_request, m)?)?;
        m.setattr(
            "_request_operation",
            wrap_pyfunction!(_request_operation, m)?,
        )?;
        Ok(())
    }

    #[pyfunction]
    fn _request_operation(owner: &str, operation: &str) -> bool {
        crate::sdk::request_operation(owner, operation)
    }

    #[pyfunction]
    fn _call_argument_error(
        py: Python<'_>,
        exception: Bound<'_, PyAny>,
        owner: &str,
        operation: &str,
        field: &Bound<'_, PyString>,
        kind: &str,
    ) -> PyResult<Py<pyo3::exceptions::PyBaseException>> {
        let info = crate::sdk::call_argument_info(owner, operation, "arguments", kind)
            .map_err(to_py_error)?;
        let exception = attach_error_descriptor(PyErr::from_value(exception), info);
        // Python 关键字允许孤立 surrogate，目标字段直接保留原始 PyString 内容。
        let target = PyDict::new(py);
        target.set_item("field", field)?;
        exception.value(py).setattr("target", target)?;
        Ok(exception.into_value(py))
    }

    #[cfg(test)]
    mod tests {
        use super::round_api_f32;

        #[test]
        fn 核心错误完整映射到_python异常() {
            pyo3::prepare_freethreaded_python();
            pyo3::Python::with_gil(|py| {
                use pyo3::prelude::*;
                let json = py.import("json").unwrap();
                for (error, code, expected) in crate::error::tests::cases() {
                    let message = error.to_string();
                    let exception = super::to_py_error(error);
                    let value = exception.value(py);
                    assert_eq!(
                        value
                            .getattr("schema_version")
                            .unwrap()
                            .extract::<u32>()
                            .unwrap(),
                        1
                    );
                    assert_eq!(
                        value
                            .getattr("message")
                            .unwrap()
                            .extract::<String>()
                            .unwrap(),
                        message
                    );
                    assert_eq!(
                        value.getattr("code").unwrap().extract::<String>().unwrap(),
                        code
                    );
                    let expected = json.call_method1("loads", (expected.to_string(),)).unwrap();
                    assert!(
                        value.getattr("details").unwrap().eq(expected).unwrap(),
                        "{code}"
                    );
                }
            });
        }

        #[test]
        fn 边权重_api舍入到六位小数() {
            assert_eq!(round_api_f32(0.9f32), 0.9);
            assert_eq!(round_api_f32(0.12345678f32), 0.123457);
        }
    }
}
