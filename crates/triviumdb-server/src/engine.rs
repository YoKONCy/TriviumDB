//! TriviumDB Server 的并发执行内核。
//!
//! 读请求由受限 blocking worker 并发执行；Writer Actor 在提交前先声明写等待，再获取
//! 全部读许可，阻止新读插队并等待已有读退出。OCC 和幂等状态只在成功提交后发布。

use crate::protocol::{ApiError, MutationResponse, TransactionOperation, TransactionRequest};
use axum::http::StatusCode;
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, mpsc, oneshot};
use triviumdb::{
    Database,
    database::Config,
    node::{NodeView, SearchHit},
    query::{
        tql_executor::QueryControl,
        tql_prepared::{PreparedTql, TqlParamValue},
    },
};
use uuid::Uuid;

pub type QueryRows = triviumdb::query::tql_executor::TqlValueResult<f32>;

#[derive(Debug)]
pub struct EngineConfig {
    pub database_path: PathBuf,
    pub database: Config,
    pub write_queue_capacity: usize,
    pub max_concurrent_reads: usize,
    pub idempotency_capacity: usize,
    pub max_write_batch_size: usize,
    pub max_write_batch_delay: Duration,
    pub prepared_cache_capacity: usize,
}

tokio::task_local! {
    pub static REQUEST_TELEMETRY: Arc<RequestTelemetry>;
}

fn current_telemetry() -> Option<Arc<RequestTelemetry>> {
    REQUEST_TELEMETRY.try_with(Arc::clone).ok()
}

const UNRECORDED_NANOS: u64 = u64::MAX;

#[derive(Debug)]
pub struct RequestTelemetry {
    queue_wait_nanos: AtomicU64,
    execution_nanos: AtomicU64,
}

impl Default for RequestTelemetry {
    fn default() -> Self {
        Self {
            queue_wait_nanos: AtomicU64::new(UNRECORDED_NANOS),
            execution_nanos: AtomicU64::new(UNRECORDED_NANOS),
        }
    }
}

impl RequestTelemetry {
    fn record_queue_wait(&self, duration: Duration) {
        self.queue_wait_nanos.store(
            u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX - 1),
            Ordering::Release,
        );
    }

    fn record_execution(&self, duration: Duration) {
        self.execution_nanos.store(
            u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX - 1),
            Ordering::Release,
        );
    }

    pub fn queue_wait(&self) -> Option<Duration> {
        duration_metric(self.queue_wait_nanos.load(Ordering::Acquire))
    }

    pub fn execution(&self) -> Option<Duration> {
        duration_metric(self.execution_nanos.load(Ordering::Acquire))
    }
}

fn duration_metric(nanos: u64) -> Option<Duration> {
    (nanos != UNRECORDED_NANOS).then(|| Duration::from_nanos(nanos))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EngineMetrics {
    pub queue_depth: usize,
    pub queue_capacity: usize,
    pub queued_total: u64,
    pub rejected_total: u64,
    pub batches_total: u64,
    pub batched_writes_total: u64,
    pub max_observed_batch_size: usize,
    pub wal_sync_total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionSnapshot {
    pub epoch: String,
    pub generation: u64,
    fine_grained_generation: u64,
}

impl VersionSnapshot {
    pub fn global_etag(&self) -> String {
        format!("\"{}-g{}\"", self.epoch, self.generation)
    }

    pub fn node_etag(&self, id: u64, version: u64) -> String {
        format!(
            "\"{}-f{}-n{}-v{}\"",
            self.epoch, self.fine_grained_generation, id, version
        )
    }
}

#[derive(Debug, Clone)]
pub struct VersionedNode {
    pub node: NodeView<f32>,
    pub version: VersionSnapshot,
    pub node_version: u64,
    pub edge_versions: Vec<(u64, String, String)>,
}

#[derive(Debug, Clone)]
pub struct WriteResult {
    pub mutation: MutationResponse,
    pub version: VersionSnapshot,
    pub replayed: bool,
}

#[derive(Debug, Clone)]
pub struct TransactionResult {
    pub created_ids: Vec<String>,
    pub version: VersionSnapshot,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct EngineHandle {
    inner: Arc<EngineInner>,
    writes: mpsc::Sender<WriteCommand>,
}

struct EngineInner {
    database: Arc<RwLock<Database<f32>>>,
    read_slots: Arc<Semaphore>,
    max_concurrent_reads: u32,
    waiting_writers: AtomicUsize,
    versions: Arc<Mutex<VersionState>>,
    prepared: Mutex<PreparedCache>,
    metrics: EngineMetricState,
    max_write_batch_size: usize,
    max_write_batch_delay: Duration,
}

struct EngineMetricState {
    queue_depth: AtomicUsize,
    queue_capacity: usize,
    queued_total: AtomicU64,
    rejected_total: AtomicU64,
    batches_total: AtomicU64,
    batched_writes_total: AtomicU64,
    max_observed_batch_size: AtomicUsize,
    wal_sync_total: AtomicU64,
    writer_failed: AtomicBool,
}

struct VersionState {
    epoch: String,
    generation: u64,
    fine_grained_generation: u64,
    node_versions: HashMap<u64, u64>,
    edge_versions: HashMap<EdgeKey, u64>,
    idempotency: IdempotencyCache,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct EdgeKey {
    source: u64,
    target: u64,
    label: String,
}

struct QueryCancelGuard {
    control: QueryControl,
    armed: bool,
}

impl QueryCancelGuard {
    fn new(control: QueryControl) -> Self {
        Self {
            control,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for QueryCancelGuard {
    fn drop(&mut self) {
        if self.armed {
            self.control.cancel();
        }
    }
}

struct PreparedCache {
    capacity: usize,
    order: VecDeque<String>,
    entries: HashMap<String, PreparedTql>,
}

struct IdempotencyCache {
    capacity: usize,
    order: VecDeque<String>,
    entries: HashMap<String, IdempotencyEntry>,
}

#[derive(Clone)]
struct IdempotencyEntry {
    fingerprint: Vec<u8>,
    result: StoredWriteResult,
}

#[derive(Clone)]
enum StoredWriteResult {
    Mutation(WriteResult),
    Transaction(TransactionResult),
}

struct WriteCommand {
    deadline: Instant,
    enqueued_at: Instant,
    telemetry: Option<Arc<RequestTelemetry>>,
    idempotency_key: Option<String>,
    fingerprint: Vec<u8>,
    operation: WriteOperation,
    response: oneshot::Sender<Result<StoredWriteResult, ApiError>>,
}

enum WriteOperation {
    Tql {
        query: String,
        expected_generation: Option<String>,
    },
    Transaction(TransactionRequest),
}

impl EngineHandle {
    pub async fn start(config: EngineConfig) -> Result<Self, ApiError> {
        if config.write_queue_capacity == 0
            || config.max_concurrent_reads == 0
            || config.max_write_batch_size == 0
        {
            return Err(ApiError::invalid_request(
                "读并发数、写队列容量和批量上限必须大于 0 (Read concurrency, write queue capacity, and batch size must be greater than zero)",
            ));
        }
        let path = config.database_path.to_string_lossy().into_owned();
        let database = tokio::task::spawn_blocking(move || {
            Database::<f32>::open_with_config(&path, config.database).map_err(ApiError::from)
        })
        .await
        .map_err(|error| {
            ApiError::unavailable(format!(
                "数据库启动任务失败 (Database startup task failed): {error}"
            ))
        })??;
        let database = Arc::new(RwLock::new(database));
        let read_slots = Arc::new(Semaphore::new(config.max_concurrent_reads));
        let versions = Arc::new(Mutex::new(VersionState {
            epoch: Uuid::new_v4().simple().to_string(),
            generation: 0,
            fine_grained_generation: 0,
            node_versions: HashMap::new(),
            edge_versions: HashMap::new(),
            idempotency: IdempotencyCache::new(config.idempotency_capacity),
        }));
        let (writes, receiver) = mpsc::channel(config.write_queue_capacity);
        let inner = Arc::new(EngineInner {
            database,
            read_slots,
            max_concurrent_reads: u32::try_from(config.max_concurrent_reads).map_err(|_| {
                ApiError::invalid_request("读并发数过大 (Read concurrency is too large)")
            })?,
            waiting_writers: AtomicUsize::new(0),
            versions,
            prepared: Mutex::new(PreparedCache::new(config.prepared_cache_capacity)),
            metrics: EngineMetricState {
                queue_depth: AtomicUsize::new(0),
                queue_capacity: config.write_queue_capacity,
                queued_total: AtomicU64::new(0),
                rejected_total: AtomicU64::new(0),
                batches_total: AtomicU64::new(0),
                batched_writes_total: AtomicU64::new(0),
                max_observed_batch_size: AtomicUsize::new(0),
                wal_sync_total: AtomicU64::new(0),
                writer_failed: AtomicBool::new(false),
            },
            max_write_batch_size: config.max_write_batch_size,
            max_write_batch_delay: config.max_write_batch_delay,
        });
        tokio::spawn(run_writer(inner.clone(), receiver));
        Ok(Self { inner, writes })
    }

    pub async fn query(
        &self,
        query: String,
        deadline: Instant,
    ) -> Result<(QueryRows, VersionSnapshot), ApiError> {
        let versions = self.inner.versions.clone();
        let control = QueryControl::with_deadline(deadline);
        self.run_read(deadline, control.clone(), move |database| {
            let rows = database
                .tql_with_control(&query, control)
                .map_err(ApiError::from)?;
            let version = lock_or_recover(&versions).snapshot();
            Ok((rows, version))
        })
        .await
    }

    pub fn prepare(&self, query: &str) -> Result<(String, Vec<String>), ApiError> {
        let prepared = {
            let database = read_or_recover(&self.inner.database);
            database.prepare_tql(query).map_err(ApiError::from)?
        };
        let parameters = prepared
            .parameter_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let id = Uuid::new_v4().simple().to_string();
        lock_or_recover(&self.inner.prepared).insert(id.clone(), prepared);
        Ok((id, parameters))
    }

    pub async fn execute_prepared(
        &self,
        id: &str,
        parameters: serde_json::Map<String, serde_json::Value>,
        deadline: Instant,
    ) -> Result<(QueryRows, VersionSnapshot), ApiError> {
        let prepared = lock_or_recover(&self.inner.prepared)
            .get(id)
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::NOT_FOUND,
                    "PREPARED_NOT_FOUND",
                    "Prepared 查询不存在 (Prepared query not found)",
                    "Prepared 查询不存在或已被缓存淘汰 (Prepared query does not exist or was evicted)",
                    false,
                )
            })?;
        let parameters = parameters
            .into_iter()
            .map(|(name, value)| {
                Ok((
                    name,
                    TqlParamValue::from_json(&value).map_err(ApiError::from)?,
                ))
            })
            .collect::<Result<HashMap<_, _>, ApiError>>()?;
        let versions = self.inner.versions.clone();
        let control = QueryControl::with_deadline(deadline);
        self.run_read(deadline, control.clone(), move |database| {
            let rows = database
                .execute_prepared_tql_with_control(&prepared, &parameters, control)
                .map_err(ApiError::from)?;
            Ok((rows, lock_or_recover(&versions).snapshot()))
        })
        .await
    }

    pub async fn search_vector(
        &self,
        vector: Vec<f32>,
        top_k: usize,
        deadline: Instant,
    ) -> Result<(Vec<SearchHit>, VersionSnapshot), ApiError> {
        let versions = self.inner.versions.clone();
        let control = QueryControl::with_deadline(deadline);
        self.run_read(deadline, control.clone(), move |database| {
            control.check().map_err(ApiError::from)?;
            let hits = database
                .search(&vector, top_k, 0, -1.0)
                .map_err(ApiError::from)?;
            control.check().map_err(ApiError::from)?;
            Ok((hits, lock_or_recover(&versions).snapshot()))
        })
        .await
    }

    pub fn index_fields(&self) -> Vec<String> {
        let database = read_or_recover(&self.inner.database);
        database
            .index_info()
            .into_iter()
            .flat_map(|index| index.fields)
            .collect()
    }

    pub async fn get_node(&self, id: u64, deadline: Instant) -> Result<VersionedNode, ApiError> {
        let versions = self.inner.versions.clone();
        let control = QueryControl::with_deadline(deadline);
        self.run_read(deadline, control.clone(), move |database| {
            let node = database.get(id).ok_or_else(|| {
                ApiError::new(
                    StatusCode::NOT_FOUND,
                    "NODE_NOT_FOUND",
                    "节点不存在 (Node not found)",
                    format!("节点 {id} 不存在 (Node {id} does not exist)"),
                    false,
                )
            })?;
            let versions = lock_or_recover(&versions);
            let node_version = versions.node_versions.get(&id).copied().unwrap_or(0);
            let edge_versions = node
                .edges
                .iter()
                .map(|edge| {
                    let key = EdgeKey::new(id, edge.target_id, &edge.label);
                    let version = versions.edge_versions.get(&key).copied().unwrap_or(0);
                    (
                        edge.target_id,
                        edge.label.clone(),
                        edge_etag(
                            &versions.epoch,
                            versions.fine_grained_generation,
                            &key,
                            version,
                        ),
                    )
                })
                .collect();
            Ok(VersionedNode {
                node,
                version: versions.snapshot(),
                node_version,
                edge_versions,
            })
        })
        .await
    }

    pub async fn mutate(
        &self,
        query: String,
        expected_generation: Option<String>,
        idempotency_key: Option<String>,
        deadline: Instant,
    ) -> Result<WriteResult, ApiError> {
        let fingerprint = serde_json::to_vec(&(query.as_str(), expected_generation.as_deref()))
            .map_err(|error| ApiError::internal(error.to_string()))?;
        match self
            .send_write(
                WriteOperation::Tql {
                    query,
                    expected_generation,
                },
                idempotency_key,
                fingerprint,
                deadline,
            )
            .await?
        {
            StoredWriteResult::Mutation(result) => Ok(result),
            StoredWriteResult::Transaction(_) => Err(ApiError::internal(
                "写入结果类型不匹配 (Write result type mismatch)",
            )),
        }
    }

    pub async fn transaction(
        &self,
        request: TransactionRequest,
        idempotency_key: Option<String>,
        deadline: Instant,
    ) -> Result<TransactionResult, ApiError> {
        let fingerprint =
            serde_json::to_vec(&request).map_err(|error| ApiError::internal(error.to_string()))?;
        match self
            .send_write(
                WriteOperation::Transaction(request),
                idempotency_key,
                fingerprint,
                deadline,
            )
            .await?
        {
            StoredWriteResult::Transaction(result) => Ok(result),
            StoredWriteResult::Mutation(_) => Err(ApiError::internal(
                "事务结果类型不匹配 (Transaction result type mismatch)",
            )),
        }
    }

    pub async fn ready(&self) -> bool {
        !self.writes.is_closed() && !self.inner.metrics.writer_failed.load(Ordering::Acquire)
    }

    pub fn metrics(&self) -> EngineMetrics {
        self.inner.metrics.snapshot()
    }

    async fn run_read<R, F>(
        &self,
        deadline: Instant,
        control: QueryControl,
        operation: F,
    ) -> Result<R, ApiError>
    where
        R: Send + 'static,
        F: FnOnce(&Database<f32>) -> Result<R, ApiError> + Send + 'static,
    {
        if self.inner.metrics.writer_failed.load(Ordering::Acquire) {
            return Err(engine_stopped());
        }
        let mut operation = Some(operation);
        let telemetry = current_telemetry();
        let wait_started = Instant::now();
        let permit = loop {
            if Instant::now() >= deadline {
                return Err(ApiError::timeout());
            }
            while self.inner.waiting_writers.load(Ordering::Acquire) != 0 {
                if Instant::now() >= deadline {
                    return Err(ApiError::timeout());
                }
                tokio::task::yield_now().await;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let permit =
                tokio::time::timeout(remaining, self.inner.read_slots.clone().acquire_owned())
                    .await
                    .map_err(|_| ApiError::timeout())?
                    .map_err(|_| engine_stopped())?;
            if self.inner.waiting_writers.load(Ordering::Acquire) == 0 {
                break permit;
            }
            drop(permit);
        };
        if let Some(telemetry) = &telemetry {
            telemetry.record_queue_wait(wait_started.elapsed());
        }
        let database = self.inner.database.clone();
        let mut cancel_guard = QueryCancelGuard::new(control.clone());
        let task_telemetry = telemetry;
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let guard = read_or_recover(&database);
            let execution_started = Instant::now();
            let result = match operation.take() {
                Some(operation) => operation(&guard),
                None => Err(ApiError::internal(
                    "读取操作状态无效 (Read operation state is invalid)",
                )),
            };
            if let Some(telemetry) = task_telemetry {
                telemetry.record_execution(execution_started.elapsed());
            }
            result
        });
        match tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), task).await {
            Ok(result) => {
                let result = result.map_err(|error| {
                    ApiError::internal(format!("读取任务失败 (Read task failed): {error}"))
                })?;
                cancel_guard.disarm();
                result
            }
            Err(_) => {
                control.cancel();
                Err(ApiError::timeout())
            }
        }
    }

    async fn send_write(
        &self,
        operation: WriteOperation,
        idempotency_key: Option<String>,
        fingerprint: Vec<u8>,
        deadline: Instant,
    ) -> Result<StoredWriteResult, ApiError> {
        validate_idempotency_key(idempotency_key.as_deref())?;
        let (response, receiver) = oneshot::channel();
        self.inner
            .metrics
            .queue_depth
            .fetch_add(1, Ordering::AcqRel);
        let telemetry = current_telemetry();
        let enqueued_at = Instant::now();
        match self.writes.try_send(WriteCommand {
            deadline,
            enqueued_at,
            telemetry,
            idempotency_key,
            fingerprint,
            operation,
            response,
        }) {
            Ok(()) => {
                self.inner
                    .metrics
                    .queued_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.inner
                    .metrics
                    .queue_depth
                    .fetch_sub(1, Ordering::AcqRel);
                self.inner
                    .metrics
                    .rejected_total
                    .fetch_add(1, Ordering::Relaxed);
                return Err(ApiError::queue_full());
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.inner
                    .metrics
                    .queue_depth
                    .fetch_sub(1, Ordering::AcqRel);
                return Err(engine_stopped());
            }
        }
        tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), receiver)
            .await
            .map_err(|_| ApiError::timeout())?
            .map_err(|_| engine_stopped())?
    }
}

async fn run_writer(inner: Arc<EngineInner>, mut receiver: mpsc::Receiver<WriteCommand>) {
    while let Some(command) = receiver.recv().await {
        inner.metrics.queue_depth.fetch_sub(1, Ordering::AcqRel);
        let mut batch = vec![command];
        collect_write_batch(&inner, &mut receiver, &mut batch).await;
        process_write_batch(&inner, batch).await;
        if inner.metrics.writer_failed.load(Ordering::Acquire) {
            receiver.close();
            while let Some(command) = receiver.recv().await {
                inner.metrics.queue_depth.fetch_sub(1, Ordering::AcqRel);
                let _ = command.response.send(Err(engine_stopped()));
            }
            break;
        }
    }
}

async fn collect_write_batch(
    inner: &EngineInner,
    receiver: &mut mpsc::Receiver<WriteCommand>,
    batch: &mut Vec<WriteCommand>,
) {
    if batch.len() >= inner.max_write_batch_size {
        return;
    }
    if receiver.is_empty() && !inner.max_write_batch_delay.is_zero() {
        let _ = tokio::time::timeout(inner.max_write_batch_delay, receiver.recv())
            .await
            .ok()
            .flatten()
            .map(|command| {
                inner.metrics.queue_depth.fetch_sub(1, Ordering::AcqRel);
                batch.push(command);
            });
    }
    while batch.len() < inner.max_write_batch_size {
        match receiver.try_recv() {
            Ok(command) => {
                inner.metrics.queue_depth.fetch_sub(1, Ordering::AcqRel);
                batch.push(command);
            }
            Err(_) => break,
        }
    }
}

async fn process_write_batch(inner: &Arc<EngineInner>, mut batch: Vec<WriteCommand>) {
    batch.retain_mut(|command| {
        if Instant::now() >= command.deadline {
            let response = std::mem::replace(&mut command.response, oneshot::channel().0);
            let _ = response.send(Err(ApiError::timeout()));
            false
        } else {
            !command.response.is_closed()
        }
    });
    if batch.is_empty() {
        return;
    }

    inner.waiting_writers.fetch_add(1, Ordering::AcqRel);
    let permits = loop {
        let earliest_deadline = batch
            .iter()
            .map(|command| command.deadline)
            .min()
            .unwrap_or_else(Instant::now);
        match tokio::time::timeout_at(
            earliest_deadline.into(),
            inner
                .read_slots
                .clone()
                .acquire_many_owned(inner.max_concurrent_reads),
        )
        .await
        {
            Ok(Ok(permits)) => break Some(permits),
            Ok(Err(_)) => break None,
            Err(_) => {
                let now = Instant::now();
                let mut remaining = Vec::with_capacity(batch.len());
                for command in batch {
                    if command.deadline <= now {
                        let _ = command.response.send(Err(ApiError::timeout()));
                    } else if !command.response.is_closed() {
                        remaining.push(command);
                    }
                }
                batch = remaining;
                if batch.is_empty() {
                    break None;
                }
            }
        }
    };
    inner.waiting_writers.fetch_sub(1, Ordering::AcqRel);
    let Some(permits) = permits else {
        if !batch.is_empty() {
            reject_batch(batch, engine_stopped());
        }
        return;
    };

    batch.retain(|command| !command.response.is_closed() && Instant::now() < command.deadline);
    if batch.is_empty() {
        return;
    }
    let batch_size = batch.len();
    inner.metrics.batches_total.fetch_add(1, Ordering::Relaxed);
    inner
        .metrics
        .batched_writes_total
        .fetch_add(batch_size as u64, Ordering::Relaxed);
    inner
        .metrics
        .max_observed_batch_size
        .fetch_max(batch_size, Ordering::Relaxed);

    let database = inner.database.clone();
    let versions = inner.versions.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _permits = permits;
        let mut database = write_or_recover(&database);
        let mut versions = lock_or_recover(&versions);
        let (outcomes, synced) = database
            .group_commit(|database| {
                let mut outcomes = Vec::with_capacity(batch.len());
                for command in batch {
                    let execution_started = Instant::now();
                    if let Some(telemetry) = &command.telemetry {
                        telemetry.record_queue_wait(
                            execution_started.duration_since(command.enqueued_at),
                        );
                    }
                    let outcome = if let Some(result) =
                        lookup_idempotent_locked(&versions, &command)
                    {
                        result
                    } else {
                        let result = execute_write(database, &mut versions, command.operation);
                        if let (Ok(result), Some(key)) = (&result, command.idempotency_key) {
                            versions
                                .idempotency
                                .insert(key, command.fingerprint, result.clone());
                        }
                        result
                    };
                    outcomes.push((
                        command.response,
                        command.telemetry,
                        execution_started,
                        outcome,
                    ));
                }
                outcomes
            })
            .map_err(ApiError::from)?;
        Ok::<_, ApiError>((outcomes, synced))
    })
    .await;

    match result {
        Ok(Ok((outcomes, synced))) => {
            if synced {
                inner.metrics.wal_sync_total.fetch_add(1, Ordering::Relaxed);
            }
            for (response, telemetry, execution_started, outcome) in outcomes {
                if let Some(telemetry) = telemetry {
                    telemetry.record_execution(execution_started.elapsed());
                }
                let _ = response.send(outcome);
            }
        }
        Ok(Err(error)) => {
            inner.metrics.writer_failed.store(true, Ordering::Release);
            tracing::error!(%error, "Group Commit 同步失败，Writer Actor 已停止 (Group commit sync failed; writer actor stopped)");
        }
        Err(error) => {
            inner.metrics.writer_failed.store(true, Ordering::Release);
            tracing::error!(%error, "Group Commit 任务失败，Writer Actor 已停止 (Group commit task failed; writer actor stopped)");
        }
    }
}

fn reject_batch(batch: Vec<WriteCommand>, error: ApiError) {
    for command in batch {
        let _ = command.response.send(Err(error.clone()));
    }
}

fn execute_write(
    database: &mut Database<f32>,
    versions: &mut VersionState,
    operation: WriteOperation,
) -> Result<StoredWriteResult, ApiError> {
    match operation {
        WriteOperation::Tql {
            query,
            expected_generation,
        } => {
            validate_global_precondition(versions, expected_generation.as_deref())?;
            let mutation = database.tql_mut(&query).map_err(ApiError::from)?;
            versions.generation = versions.generation.saturating_add(1);
            // TQL 暂不返回完整 write-set，因此保守失效此前签发的全部细粒度 ETag。
            versions.fine_grained_generation = versions.generation;
            for id in &mutation.created_ids {
                versions.node_versions.insert(*id, versions.generation);
            }
            Ok(StoredWriteResult::Mutation(WriteResult {
                mutation: mutation.into(),
                version: versions.snapshot(),
                replayed: false,
            }))
        }
        WriteOperation::Transaction(request) => {
            validate_transaction_preconditions(versions, &request)?;
            let mut transaction = database.begin_tx();
            let mut touched_nodes = Vec::new();
            let mut touched_edges = Vec::new();
            for operation in &request.operations {
                match operation {
                    TransactionOperation::Insert {
                        id,
                        vector,
                        payload,
                    } => {
                        if let Some(id) = id {
                            transaction.insert_with_id(*id, vector, payload.clone());
                            touched_nodes.push(*id);
                        } else {
                            transaction.insert(vector, payload.clone());
                        }
                    }
                    TransactionOperation::UpdatePayload { id, payload } => {
                        transaction.update_payload(*id, payload.clone());
                        touched_nodes.push(*id);
                    }
                    TransactionOperation::Delete { id } => {
                        transaction.delete(*id);
                        touched_nodes.push(*id);
                    }
                    TransactionOperation::Link {
                        source,
                        target,
                        label,
                        weight,
                    } => {
                        transaction.link(*source, *target, label, *weight);
                        touched_nodes.extend([*source, *target]);
                        touched_edges.push(EdgeKey::new(*source, *target, label));
                    }
                    TransactionOperation::Unlink {
                        source,
                        target,
                        label,
                    } => {
                        if let Some(label) = label {
                            transaction.unlink_label(*source, *target, label);
                            touched_edges.push(EdgeKey::new(*source, *target, label));
                        } else {
                            transaction.unlink(*source, *target);
                            touched_edges.extend(
                                versions
                                    .edge_versions
                                    .keys()
                                    .filter(|key| key.source == *source && key.target == *target)
                                    .cloned(),
                            );
                        }
                        touched_nodes.extend([*source, *target]);
                    }
                }
            }
            let created_ids = transaction.commit().map_err(ApiError::from)?;
            versions.generation = versions.generation.saturating_add(1);
            let generation = versions.generation;
            touched_nodes.extend(created_ids.iter().copied());
            for id in touched_nodes {
                versions.node_versions.insert(id, generation);
            }
            for edge in touched_edges {
                versions.edge_versions.insert(edge, generation);
            }
            Ok(StoredWriteResult::Transaction(TransactionResult {
                created_ids: created_ids.into_iter().map(|id| id.to_string()).collect(),
                version: versions.snapshot(),
                replayed: false,
            }))
        }
    }
}

fn validate_global_precondition(
    versions: &VersionState,
    expected: Option<&str>,
) -> Result<(), ApiError> {
    if let Some(expected) = expected
        && expected != versions.snapshot().global_etag()
    {
        return Err(ApiError::write_conflict(format!(
            "全局版本不匹配：期望 {expected}，当前 {} (Global version mismatch: expected {expected}, current {})",
            versions.snapshot().global_etag(),
            versions.snapshot().global_etag()
        )));
    }
    Ok(())
}

fn validate_transaction_preconditions(
    versions: &VersionState,
    request: &TransactionRequest,
) -> Result<(), ApiError> {
    validate_global_precondition(versions, request.expected_generation.as_deref())?;
    for (id, expected) in &request.expected_nodes {
        let current = versions.node_versions.get(id).copied().unwrap_or(0);
        let current_etag = versions.snapshot().node_etag(*id, current);
        if expected != &current_etag {
            return Err(ApiError::write_conflict(format!(
                "节点 {id} 版本不匹配：期望 {expected}，当前 {current_etag} (Node {id} version mismatch: expected {expected}, current {current_etag})"
            )));
        }
    }
    for expected in &request.expected_edges {
        let key = EdgeKey::new(expected.source, expected.target, &expected.label);
        let current = versions.edge_versions.get(&key).copied().unwrap_or(0);
        let current_etag = edge_etag(
            &versions.epoch,
            versions.fine_grained_generation,
            &key,
            current,
        );
        if expected.etag != current_etag {
            return Err(ApiError::write_conflict(format!(
                "边版本不匹配：期望 {}，当前 {} (Edge version mismatch: expected {}, current {})",
                expected.etag, current_etag, expected.etag, current_etag
            )));
        }
    }
    Ok(())
}

fn lookup_idempotent_locked(
    versions: &VersionState,
    command: &WriteCommand,
) -> Option<Result<StoredWriteResult, ApiError>> {
    let key = command.idempotency_key.as_ref()?;
    versions
        .idempotency
        .get(key, &command.fingerprint)
        .map(|result| {
            result.map(|mut result| {
                match &mut result {
                    StoredWriteResult::Mutation(result) => result.replayed = true,
                    StoredWriteResult::Transaction(result) => result.replayed = true,
                }
                result
            })
        })
}

impl EngineMetricState {
    fn snapshot(&self) -> EngineMetrics {
        EngineMetrics {
            queue_depth: self.queue_depth.load(Ordering::Acquire),
            queue_capacity: self.queue_capacity,
            queued_total: self.queued_total.load(Ordering::Relaxed),
            rejected_total: self.rejected_total.load(Ordering::Relaxed),
            batches_total: self.batches_total.load(Ordering::Relaxed),
            batched_writes_total: self.batched_writes_total.load(Ordering::Relaxed),
            max_observed_batch_size: self.max_observed_batch_size.load(Ordering::Relaxed),
            wal_sync_total: self.wal_sync_total.load(Ordering::Relaxed),
        }
    }
}

impl VersionState {
    fn snapshot(&self) -> VersionSnapshot {
        VersionSnapshot {
            epoch: self.epoch.clone(),
            generation: self.generation,
            fine_grained_generation: self.fine_grained_generation,
        }
    }
}

impl EdgeKey {
    fn new(source: u64, target: u64, label: &str) -> Self {
        Self {
            source,
            target,
            label: label.to_owned(),
        }
    }
}

fn edge_etag(epoch: &str, fine_grained_generation: u64, key: &EdgeKey, version: u64) -> String {
    let label_hash = key.label.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!(
        "\"{}-f{}-e{}-{:016x}-{}-v{}\"",
        epoch, fine_grained_generation, key.source, label_hash, key.target, version
    )
}

impl PreparedCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::new(),
            entries: HashMap::new(),
        }
    }

    fn insert(&mut self, id: String, prepared: PreparedTql) {
        if self.capacity == 0 {
            return;
        }
        while self.entries.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        self.order.push_back(id.clone());
        self.entries.insert(id, prepared);
    }

    fn get(&mut self, id: &str) -> Option<PreparedTql> {
        let prepared = self.entries.get(id)?.clone();
        if let Some(position) = self.order.iter().position(|entry| entry == id) {
            self.order.remove(position);
        }
        self.order.push_back(id.to_owned());
        Some(prepared)
    }
}

impl IdempotencyCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::new(),
            entries: HashMap::new(),
        }
    }

    fn get(&self, key: &str, fingerprint: &[u8]) -> Option<Result<StoredWriteResult, ApiError>> {
        let entry = self.entries.get(key)?;
        Some(if entry.fingerprint == fingerprint {
            Ok(entry.result.clone())
        } else {
            Err(ApiError::new(
                StatusCode::CONFLICT,
                "IDEMPOTENCY_KEY_REUSED",
                "幂等键已用于其他请求 (Idempotency key reused)",
                "同一幂等键不能用于不同请求体 (The same idempotency key cannot be used with a different request body)",
                false,
            ))
        })
    }

    fn insert(&mut self, key: String, fingerprint: Vec<u8>, result: StoredWriteResult) {
        if self.capacity == 0 {
            return;
        }
        if !self.entries.contains_key(&key) {
            while self.entries.len() >= self.capacity {
                if let Some(oldest) = self.order.pop_front() {
                    self.entries.remove(&oldest);
                }
            }
            self.order.push_back(key.clone());
        }
        self.entries.insert(
            key,
            IdempotencyEntry {
                fingerprint,
                result,
            },
        );
    }
}

fn validate_idempotency_key(key: Option<&str>) -> Result<(), ApiError> {
    if let Some(key) = key
        && (key.is_empty() || key.len() > 128 || !key.is_ascii())
    {
        return Err(ApiError::invalid_request(
            "Idempotency-Key 必须是 1..=128 字节 ASCII (Idempotency-Key must be 1..=128 ASCII bytes)",
        ));
    }
    Ok(())
}

fn read_or_recover<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_or_recover<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_or_recover<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn engine_stopped() -> ApiError {
    ApiError::unavailable("数据库执行线程不可用 (Database execution thread is unavailable)")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_engine(
        name: &str,
        queue: usize,
        reads: usize,
    ) -> (EngineHandle, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let engine = EngineHandle::start(EngineConfig {
            database_path: directory.path().join(format!("{name}.tdb")),
            database: Config {
                dim: 2,
                storage_mode: triviumdb::database::StorageMode::Rom,
                ..Config::default()
            },
            write_queue_capacity: queue,
            max_concurrent_reads: reads,
            idempotency_capacity: 8,
            max_write_batch_size: 8,
            max_write_batch_delay: Duration::from_millis(1),
            prepared_cache_capacity: 8,
        })
        .await
        .unwrap();
        (engine, directory)
    }

    #[test]
    fn prepared缓存按lru有界淘汰() {
        let first = PreparedTql::from_query(
            triviumdb::query::tql_parser::parse_tql("MATCH (n) RETURN n").unwrap(),
        );
        let second = PreparedTql::from_query(
            triviumdb::query::tql_parser::parse_tql("MATCH (n) RETURN n LIMIT 1").unwrap(),
        );
        let mut cache = PreparedCache::new(1);
        cache.insert("first".into(), first);
        cache.insert("second".into(), second);
        assert!(cache.get("first").is_none());
        assert!(cache.get("second").is_some());
    }

    #[test]
    fn 幂等缓存拒绝同键不同请求并有界淘汰() {
        let mut cache = IdempotencyCache::new(1);
        let result = StoredWriteResult::Transaction(TransactionResult {
            created_ids: vec![],
            version: VersionSnapshot {
                epoch: "epoch".into(),
                generation: 1,
                fine_grained_generation: 0,
            },
            replayed: false,
        });
        cache.insert("a".into(), vec![1], result.clone());
        assert!(cache.get("a", &[1]).unwrap().is_ok());
        assert!(cache.get("a", &[2]).unwrap().is_err());
        cache.insert("b".into(), vec![2], result);
        assert!(cache.get("a", &[1]).is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn 读semaphore限制并发且写等待时阻止新读插队() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (engine, _directory) = test_engine("fairness", 4, 2).await;
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut reads = Vec::new();
        for _ in 0..8 {
            let engine = engine.clone();
            let active = active.clone();
            let peak = peak.clone();
            reads.push(tokio::spawn(async move {
                engine
                    .run_read(
                        Instant::now() + std::time::Duration::from_secs(2),
                        QueryControl::default(),
                        move |_| {
                            let current = active.fetch_add(1, Ordering::AcqRel) + 1;
                            peak.fetch_max(current, Ordering::AcqRel);
                            std::thread::sleep(std::time::Duration::from_millis(10));
                            active.fetch_sub(1, Ordering::AcqRel);
                            Ok(())
                        },
                    )
                    .await
            }));
        }
        for read in reads {
            read.await.unwrap().unwrap();
        }
        assert!(peak.load(Ordering::Acquire) <= 2);

        let held = engine
            .inner
            .read_slots
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        let writer = tokio::spawn({
            let engine = engine.clone();
            async move {
                engine
                    .mutate(
                        "CREATE ({name: \"writer\"})".into(),
                        None,
                        None,
                        Instant::now() + std::time::Duration::from_secs(2),
                    )
                    .await
            }
        });
        while engine.inner.waiting_writers.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        let late_read = tokio::spawn({
            let engine = engine.clone();
            async move {
                engine
                    .query(
                        "MATCH (n) RETURN count(*) AS total".into(),
                        Instant::now() + std::time::Duration::from_secs(2),
                    )
                    .await
            }
        });
        drop(held);
        writer.await.unwrap().unwrap();
        let (rows, _) = late_read.await.unwrap().unwrap();
        assert!(matches!(
            rows[0].get("total"),
            Some(triviumdb::query::tql_executor::TqlValue::Int(1))
        ));
    }

    #[tokio::test]
    async fn 有界写队列在饱和时明确背压() {
        let (engine, _directory) = test_engine("backpressure", 1, 1).await;
        let held = engine
            .inner
            .read_slots
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        let first = tokio::spawn({
            let engine = engine.clone();
            async move {
                engine
                    .mutate(
                        "CREATE ({n: 1})".into(),
                        None,
                        None,
                        Instant::now() + std::time::Duration::from_secs(2),
                    )
                    .await
            }
        });
        while engine.inner.waiting_writers.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        let second = tokio::spawn({
            let engine = engine.clone();
            async move {
                engine
                    .mutate(
                        "CREATE ({n: 2})".into(),
                        None,
                        None,
                        Instant::now() + std::time::Duration::from_secs(2),
                    )
                    .await
            }
        });
        tokio::task::yield_now().await;
        let error = engine
            .mutate(
                "CREATE ({n: 3})".into(),
                None,
                None,
                Instant::now() + std::time::Duration::from_secs(2),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("写队列已满"));
        drop(held);
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn 过期deadline在执行前拒绝且不写入() {
        let (engine, _directory) = test_engine("deadline", 4, 1).await;
        let error = engine
            .mutate(
                "CREATE ({name: \"late\"})".into(),
                None,
                None,
                Instant::now(),
            )
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("超时") || message.contains("不可用"),
            "实际错误: {message}"
        );
        let (rows, _) = engine
            .query(
                "MATCH (n) RETURN count(*) AS total".into(),
                Instant::now() + std::time::Duration::from_secs(2),
            )
            .await
            .unwrap();
        assert!(matches!(
            rows[0].get("total"),
            Some(triviumdb::query::tql_executor::TqlValue::Int(0))
        ));
    }

    #[tokio::test]
    async fn 取消写请求在出队前不产生副作用() {
        let (engine, _directory) = test_engine("cancel", 4, 1).await;
        let permit = engine
            .inner
            .read_slots
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        let task = tokio::spawn({
            let engine = engine.clone();
            async move {
                engine
                    .mutate(
                        "CREATE ({name: \"cancelled\"})".into(),
                        None,
                        None,
                        deadline,
                    )
                    .await
            }
        });
        tokio::task::yield_now().await;
        task.abort();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while engine.inner.waiting_writers.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(permit);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let (rows, _) = engine
            .query(
                "MATCH (n) RETURN count(*) AS total".into(),
                Instant::now() + std::time::Duration::from_secs(2),
            )
            .await
            .unwrap();
        assert!(matches!(
            rows[0].get("total"),
            Some(triviumdb::query::tql_executor::TqlValue::Int(0))
        ));
    }
}
