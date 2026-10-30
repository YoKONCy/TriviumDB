//! 现有嵌入式实例的 HTTP 适配，不创建数据库，不拥有监听端口或关闭宿主。

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use std::{
    error::Error as _,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use triviumdb::query::tql_executor::QueryControl;
use triviumdb::{ConsoleHost, TriviumError, VectorType};

#[derive(Debug, Clone)]
pub struct ConsoleHttpOptions {
    pub max_in_flight: usize,
}
impl Default for ConsoleHttpOptions {
    fn default() -> Self {
        Self { max_in_flight: 8 }
    }
}

struct HostState<T: VectorType> {
    host: ConsoleHost<T>,
    slots: Arc<Semaphore>,
}

struct CancelOnDrop(QueryControl);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// 可用 Router::nest 配置路径前缀；宿主在外层配置认证与跨域策略。
pub fn build_console_app<T: VectorType + Serialize + DeserializeOwned>(
    host: ConsoleHost<T>,
    options: ConsoleHttpOptions,
) -> triviumdb::Result<Router> {
    if options.max_in_flight == 0 || options.max_in_flight > 1024 {
        return Err(TriviumError::InvalidInput(
            "HTTP 并发上限必须为 1..=1024 (HTTP concurrency must be 1..=1024)".into(),
        ));
    }
    let state = Arc::new(HostState {
        host,
        slots: Arc::new(Semaphore::new(options.max_in_flight)),
    });
    Ok(Router::new()
        .route("/v1/console", post(request::<T>))
        .route("/v1/console/cancel/{id}", post(cancel::<T>))
        .route("/v1/console/contract", get(contract))
        .with_state(state))
}

async fn contract() -> impl IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        triviumdb::database::console::CONSOLE_CONTRACT,
    )
}

async fn cancel<T: VectorType + Serialize + DeserializeOwned>(
    State(state): State<Arc<HostState<T>>>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    Json(json!({"cancelled":state.host.session().cancel(&id)}))
}

async fn request<T: VectorType + Serialize + DeserializeOwned>(
    State(state): State<Arc<HostState<T>>>,
    request: Request,
) -> Response {
    // 先准入再读取请求体，未获许可的连接不能提前物化整个 JSON。
    let permit = match state.slots.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "TDB_IO",
                "控制台请求繁忙 (Console request capacity is busy)",
                json!({"io_kind":"WouldBlock","os_code":null}),
            );
        }
    };
    let session = state.host.session();
    if session.is_detached() {
        let error = TriviumError::ConsoleSessionClosed;
        return failure(
            StatusCode::GONE,
            error.code(),
            &error.to_string(),
            error.details(),
        );
    }
    let Some(deadline) =
        Instant::now().checked_add(Duration::from_millis(session.options().query_timeout_ms))
    else {
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "TDB_INVALID_INPUT",
            "请求 deadline 溢出 (Request deadline overflow)",
            json!({}),
        );
    };
    let control = QueryControl::with_deadline(deadline);
    let _cancel = CancelOnDrop(control.clone());
    let body = match tokio::time::timeout_at(
        deadline.into(),
        to_bytes(request.into_body(), session.options().max_request_bytes),
    )
    .await
    {
        Ok(Ok(body)) => body,
        Ok(Err(error)) => {
            let too_large = error
                .source()
                .is_some_and(|source| source.is::<http_body_util::LengthLimitError>());
            return failure(
                if too_large {
                    StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    StatusCode::BAD_REQUEST
                },
                if too_large {
                    "TDB_CONSOLE_REQUEST_TOO_LARGE"
                } else {
                    "TDB_CONSOLE_REQUEST_BODY"
                },
                &format!("读取控制台请求体失败 (Failed to read console request body): {error}"),
                json!({}),
            );
        }
        Err(_) => {
            return failure(
                StatusCode::REQUEST_TIMEOUT,
                "TDB_QUERY_CANCELLED",
                "读取请求体超时 (Request body read timed out)",
                json!({}),
            );
        }
    };
    let task = tokio::task::spawn_blocking(move || {
        // 许可随真实 blocking 工作释放，客户端断开不会提前归还并发额度。
        let _permit = permit;
        let raw = std::str::from_utf8(&body).map_err(|_| {
            TriviumError::InvalidInput("请求必须为 UTF-8 (Request must be UTF-8)".into())
        })?;
        state.host.request_with_control(raw, control)
    })
    .await;
    match task {
        Ok(Ok(body)) => {
            ([("content-type", "application/json; charset=utf-8")], body).into_response()
        }
        Ok(Err(error)) => {
            let status = match &error {
                TriviumError::Io(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    StatusCode::SERVICE_UNAVAILABLE
                }
                TriviumError::ReadOnlyViolation { .. } => StatusCode::FORBIDDEN,
                TriviumError::DatabaseClosed | TriviumError::ConsoleSessionClosed => {
                    StatusCode::GONE
                }
                TriviumError::ConsoleResponseTooLarge { .. }
                | TriviumError::ResourceBudgetExceeded {
                    resource: "console_request_bytes" | "console_operations",
                    ..
                } => StatusCode::PAYLOAD_TOO_LARGE,
                TriviumError::ResourceBudgetExceeded { .. } => StatusCode::SERVICE_UNAVAILABLE,
                TriviumError::QueryCancelled => StatusCode::REQUEST_TIMEOUT,
                TriviumError::Io(_) | TriviumError::WalClosed => StatusCode::INTERNAL_SERVER_ERROR,
                _ => StatusCode::BAD_REQUEST,
            };
            failure(status, error.code(), &error.to_string(), error.details())
        }
        Err(error) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "TDB_CONSOLE_TASK_FAILED",
            &format!("控制台执行任务失败 (Console execution task failed): {error}"),
            json!({}),
        ),
    }
}

fn failure(status: StatusCode, code: &str, message: &str, details: serde_json::Value) -> Response {
    (
        status,
        Json(json!({"version":1,"error":{"code":code,"message":message,"details":lossless_details(details)}})),
    )
        .into_response()
}

fn lossless_details(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Number(number)
            if number
                .as_u64()
                .is_some_and(|value| value > 9_007_199_254_740_991)
                || number
                    .as_i64()
                    .is_some_and(|value| value.unsigned_abs() > 9_007_199_254_740_991) =>
        {
            Value::String(number.to_string())
        }
        Value::Array(values) => Value::Array(values.into_iter().map(lossless_details).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, lossless_details(value)))
                .collect(),
        ),
        other => other,
    }
}
