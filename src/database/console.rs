//! 嵌入式控制台宿主协议；只借用现有实例，不打开路径、不接管文件锁。

use super::{AccessMode, Database, LifecycleState, TxBuilder, lock_or_recover};
use crate::query::tql_executor::{QueryControl, TqlValue};
use crate::{Result, TriviumError, VectorType};
use serde::{Deserialize, Serialize, Serializer, ser::SerializeMap};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex, TryLockError, Weak};
use std::time::{Duration, Instant};

pub const CONSOLE_PROTOCOL_VERSION: u32 = 1;
pub const CONSOLE_CONTRACT: &str = include_str!("console_contract.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConsoleFeature {
    Query,
    Vector,
    Graph,
    Indexes,
    Transactions,
    Import,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ConsoleOptions {
    pub read_only: bool,
    pub features: Vec<ConsoleFeature>,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub max_operations: usize,
    pub query_timeout_ms: u64,
}

impl Default for ConsoleOptions {
    fn default() -> Self {
        Self {
            read_only: true,
            features: vec![
                ConsoleFeature::Query,
                ConsoleFeature::Vector,
                ConsoleFeature::Graph,
                ConsoleFeature::Indexes,
                ConsoleFeature::Transactions,
                ConsoleFeature::Import,
            ],
            max_request_bytes: 4 * 1024 * 1024,
            max_response_bytes: 16 * 1024 * 1024,
            max_operations: 1000,
            query_timeout_ms: 30_000,
        }
    }
}

type Instance = (Mutex<LifecycleState>, Condvar);

#[derive(Default)]
struct SessionState {
    detached: bool,
    instance: Option<Weak<Instance>>,
    requests: HashMap<String, QueryControl>,
}

/// 可被多个传输端共享的会话；只保存实例身份的弱引用和请求取消状态。
#[derive(Clone)]
pub struct ConsoleSession {
    options: Arc<ConsoleOptions>,
    state: Arc<Mutex<SessionState>>,
}

impl ConsoleSession {
    pub fn new(options: ConsoleOptions) -> Result<Self> {
        if options.max_request_bytes == 0
            || options.max_request_bytes > 64 * 1024 * 1024
            || options.max_response_bytes == 0
            || options.max_response_bytes > 64 * 1024 * 1024
            || options.max_operations == 0
            || options.max_operations > 1000
            || options.query_timeout_ms == 0
            || options.query_timeout_ms > 300_000
        {
            return Err(invalid(
                "控制台预算超出支持范围 (Console budgets are out of range)",
            ));
        }
        Ok(Self {
            options: Arc::new(options),
            state: Arc::new(Mutex::new(SessionState::default())),
        })
    }

    pub fn from_json(options_json: &str) -> Result<Self> {
        if options_json.len() > 8192 {
            return Err(invalid(
                "控制台配置过大 (Console configuration is too large)",
            ));
        }
        Self::new(parse(options_json)?)
    }

    /// 停止接收新请求并取消协作式查询；不获取数据库互斥锁，也不关闭宿主数据库。
    pub fn detach(&self) {
        let mut state = lock_or_recover(&self.state);
        state.detached = true;
        for control in state.requests.values() {
            control.cancel();
        }
    }

    pub fn is_detached(&self) -> bool {
        lock_or_recover(&self.state).detached
    }

    pub fn options(&self) -> &ConsoleOptions {
        &self.options
    }

    pub fn cancel(&self, request_id: &str) -> bool {
        let state = lock_or_recover(&self.state);
        if let Some(control) = state.requests.get(request_id) {
            control.cancel();
            true
        } else {
            false
        }
    }

    fn enter(
        &self,
        instance: &Arc<Instance>,
        request_id: &str,
        external: Option<QueryControl>,
    ) -> Result<RequestGuard> {
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(self.options.query_timeout_ms))
            .ok_or_else(|| invalid("查询 deadline 溢出 (Query deadline overflow)"))?;
        let mut state = lock_or_recover(&self.state);
        if state.detached {
            return Err(TriviumError::ConsoleSessionClosed);
        }
        if let Some(bound) = &state.instance {
            if !Weak::ptr_eq(bound, &Arc::downgrade(instance)) {
                return Err(invalid(
                    "控制台会话已经绑定其他实例 (Console session is bound to another instance)",
                ));
            }
        } else {
            state.instance = Some(Arc::downgrade(instance));
        }
        if state.requests.contains_key(request_id) {
            return Err(busy("请求 ID 正在执行 (Request ID is already active)"));
        }
        let control = external.map_or_else(
            || QueryControl::with_deadline(deadline),
            |control| control.bounded_by(deadline),
        );
        state
            .requests
            .insert(request_id.to_owned(), control.clone());
        Ok(RequestGuard {
            state: Arc::downgrade(&self.state),
            request_id: request_id.to_owned(),
            control,
        })
    }

    fn require(&self, feature: ConsoleFeature, write: bool) -> Result<()> {
        if !self.options.features.contains(&feature) {
            return Err(invalid(
                "宿主未开放此控制台能力 (Console capability is disabled by the host)",
            ));
        }
        if write && self.options.read_only {
            return Err(TriviumError::ReadOnlyViolation {
                operation: "console_request",
            });
        }
        Ok(())
    }
}

impl Drop for SessionState {
    fn drop(&mut self) {
        for control in self.requests.values() {
            control.cancel();
        }
    }
}

struct RequestGuard {
    state: Weak<Mutex<SessionState>>,
    request_id: String,
    control: QueryControl,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            lock_or_recover(&state).requests.remove(&self.request_id);
        }
    }
}

/// Rust 宿主适配器；所有宿主访问使用同一个 Mutex，组件只持有弱引用。
pub struct ConsoleHost<T: VectorType> {
    database: Weak<Mutex<Database<T>>>,
    session: ConsoleSession,
}

impl<T: VectorType> Clone for ConsoleHost<T> {
    fn clone(&self) -> Self {
        Self {
            database: self.database.clone(),
            session: self.session.clone(),
        }
    }
}

impl<T: VectorType + Serialize + serde::de::DeserializeOwned> ConsoleHost<T> {
    pub fn attach(database: &Arc<Mutex<Database<T>>>, options: ConsoleOptions) -> Result<Self> {
        let session = ConsoleSession::new(options)?;
        // 不等待宿主锁，避免 Hook 重入、双向 IPC 或同线程重复加锁造成死锁。
        let guard = database.try_lock().map_err(lock_error)?;
        let _operation = guard.enter_operation()?;
        lock_or_recover(&session.state).instance = Some(Arc::downgrade(&guard.lifecycle));
        Ok(Self {
            database: Arc::downgrade(database),
            session,
        })
    }

    pub fn session(&self) -> ConsoleSession {
        self.session.clone()
    }

    pub fn request(&self, request_json: &str) -> Result<String> {
        self.request_inner(request_json, None)
    }

    pub fn request_with_control(
        &self,
        request_json: &str,
        control: QueryControl,
    ) -> Result<String> {
        self.request_inner(request_json, Some(control))
    }

    fn request_inner(&self, request_json: &str, control: Option<QueryControl>) -> Result<String> {
        if self.session.is_detached() {
            return Err(TriviumError::ConsoleSessionClosed);
        }
        let database = self
            .database
            .upgrade()
            .ok_or(TriviumError::DatabaseClosed)?;
        let mut guard = database.try_lock().map_err(lock_error)?;
        guard.console_request_inner(&self.session, request_json, control)
    }
}

fn lock_error<T>(error: TryLockError<T>) -> TriviumError {
    match error {
        TryLockError::WouldBlock => busy("宿主数据库正在使用 (Host database is busy)"),
        TryLockError::Poisoned(_) => std::io::Error::other(
            "宿主互斥锁中毒，拒绝继续访问 (Host mutex is poisoned; access rejected)",
        )
        .into(),
    }
}

fn busy(message: &str) -> TriviumError {
    std::io::Error::new(std::io::ErrorKind::WouldBlock, message).into()
}
fn invalid(message: impl Into<String>) -> TriviumError {
    TriviumError::InvalidInput(message.into())
}
fn parse<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T> {
    serde_json::from_str(raw).map_err(|error| {
        invalid(format!(
            "控制台 JSON 无效 (Invalid console JSON): category={:?}, line={}, column={}",
            error.classify(),
            error.line(),
            error.column()
        ))
    })
}
fn id(raw: &str) -> Result<u64> {
    if raw.is_empty() || !raw.bytes().all(|c| c.is_ascii_digit()) {
        return Err(invalid(
            "节点 ID 必须是十进制字符串 (Node ID must be a decimal string)",
        ));
    }
    raw.parse::<u64>()
        .ok()
        .filter(|id| *id != 0)
        .ok_or_else(|| invalid("节点 ID 超出范围 (Node ID is out of range)"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    version: u32,
    request_id: String,
    command: Command,
}

#[derive(Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Command {
    Info,
    Query {
        query: String,
        #[serde(default)]
        parameters: BTreeMap<String, Value>,
    },
    Mutate {
        query: String,
    },
    GetNodes {
        ids: Vec<String>,
    },
    Search {
        vector: Vec<Value>,
        top_k: usize,
        #[serde(default)]
        expand_depth: usize,
        #[serde(default)]
        min_score: f32,
    },
    Subgraph {
        source: String,
        #[serde(default = "one")]
        depth: usize,
        #[serde(default)]
        labels: Option<Vec<String>>,
        #[serde(default = "thousand")]
        max_nodes: usize,
    },
    Edges {
        node: String,
        #[serde(default)]
        incoming: bool,
    },
    Path {
        source: String,
        target: String,
        max_depth: usize,
        #[serde(default)]
        label: Option<String>,
    },
    Indexes,
    ManageIndex {
        kind: String,
        fields: Vec<String>,
        create: bool,
    },
    BuildQuiver,
    Transaction {
        operations: Vec<Operation>,
    },
    ImportNodes {
        records: Vec<NodeRecord>,
    },
    ImportEdges {
        records: Vec<EdgeRecord>,
    },
}
fn one() -> usize {
    1
}
fn thousand() -> usize {
    1000
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NodeRecord {
    #[serde(default)]
    id: Option<String>,
    vector: Vec<Value>,
    payload: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EdgeRecord {
    source: String,
    target: String,
    label: String,
    weight: f32,
    #[serde(default)]
    metadata: Value,
}
#[derive(Deserialize)]
#[serde(
    tag = "op",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Operation {
    Insert {
        #[serde(flatten)]
        record: NodeRecord,
    },
    UpdatePayload {
        id: String,
        payload: Value,
    },
    UpdateVector {
        id: String,
        vector: Vec<Value>,
    },
    Delete {
        id: String,
    },
    Link {
        #[serde(flatten)]
        record: EdgeRecord,
    },
    Unlink {
        source: String,
        target: String,
        #[serde(default)]
        label: Option<String>,
    },
}

impl Command {
    fn permission(&self) -> (ConsoleFeature, bool) {
        use ConsoleFeature::*;
        match self {
            Self::Info | Self::Query { .. } | Self::GetNodes { .. } => (Query, false),
            Self::Mutate { .. } => (Query, true),
            Self::Search { .. } => (Vector, false),
            Self::Subgraph { .. } | Self::Edges { .. } | Self::Path { .. } => (Graph, false),
            Self::Indexes => (Indexes, false),
            Self::ManageIndex { .. } | Self::BuildQuiver => (Indexes, true),
            Self::Transaction { .. } => (Transactions, true),
            Self::ImportNodes { .. } | Self::ImportEdges { .. } => (Import, true),
        }
    }
}

impl<T: VectorType + Serialize + serde::de::DeserializeOwned> Database<T> {
    /// 宿主可信代码传入会话策略；不接受前端传入文件路径、权限或关闭命令。
    pub fn console_request(
        &mut self,
        session: &ConsoleSession,
        request_json: &str,
    ) -> Result<String> {
        self.console_request_inner(session, request_json, None)
    }

    fn console_request_inner(
        &mut self,
        session: &ConsoleSession,
        request_json: &str,
        external: Option<QueryControl>,
    ) -> Result<String> {
        if session.is_detached() {
            return Err(TriviumError::ConsoleSessionClosed);
        }
        let _operation = self.enter_operation()?;
        let options = &session.options;
        if request_json.len() > options.max_request_bytes {
            return Err(TriviumError::ResourceBudgetExceeded {
                resource: "console_request_bytes",
                requested: request_json.len(),
                used: 0,
                limit: options.max_request_bytes,
            });
        }
        let request: Request = parse(request_json)?;
        if request.version != CONSOLE_PROTOCOL_VERSION
            || request.request_id.is_empty()
            || request.request_id.len() > 128
        {
            return Err(invalid(
                "控制台协议版本或请求 ID 无效 (Invalid console protocol version or request ID)",
            ));
        }
        let active = session.enter(&self.lifecycle, &request.request_id, external)?;
        let (feature, write) = request.command.permission();
        if !matches!(request.command, Command::Info) {
            session.require(feature, write)?;
        }
        if write {
            self.require_write_access("console_request")?;
        }
        active.control.check()?;
        let limit = options.max_response_bytes;
        let runtime = self.resource_runtime.clone();
        let respond = |data: &Value, committed| {
            encode_response(&request.request_id, data, limit, committed, &runtime)
        };
        match request.command {
            Command::Info => {
                let writable = !options.read_only && self.access_mode == AccessMode::ReadWrite;
                let max_rows = self
                    .tql_limits
                    .max_query_rows
                    .filter(|limit| *limit > 0)
                    .map_or(options.max_operations, |limit| {
                        limit.min(options.max_operations)
                    });
                let dtype = if crate::vector::is_binary_vector::<T>() {
                    "u64"
                } else if std::any::TypeId::of::<T>() == std::any::TypeId::of::<f32>() {
                    "f32"
                } else {
                    "f16"
                };
                let features: Vec<_> = options
                    .features
                    .iter()
                    .copied()
                    .filter(|f| {
                        writable
                            || !matches!(f, ConsoleFeature::Transactions | ConsoleFeature::Import)
                    })
                    .collect();
                respond(
                    &json!({"protocolVersion": CONSOLE_PROTOCOL_VERSION, "packageVersion": env!("CARGO_PKG_VERSION"),
                    "readOnly": !writable, "features": features, "dtype":dtype,"storage": self.storage_info(),
                    "limits": {"maxRequestBytes":options.max_request_bytes,"maxResponseBytes":limit,
                        "maxOperations":options.max_operations,"maxQueryRows":max_rows,"queryTimeoutMs":options.query_timeout_ms},
                    "transport": {"streaming":false,"occ":false,"idEncoding":"decimal-string","cooperativeCancellation":["query"],"cooperativeWritePlanning":true}}),
                    false,
                )
            }
            Command::Query { query, parameters } => {
                let prepared = self.prepare_tql(&query)?;
                let parameters = parameters
                    .iter()
                    .map(|(key, value)| {
                        crate::query::tql_prepared::TqlParamValue::from_json(value)
                            .map(|value| (key.clone(), value))
                    })
                    .collect::<Result<HashMap<_, _>>>()?;
                let rows = self.execute_prepared_tql_controlled(
                    &prepared,
                    &parameters,
                    active.control.clone(),
                    Some(options.max_operations),
                )?;
                active.control.check()?;
                encode_response(
                    &request.request_id,
                    &QueryResult { rows: &rows },
                    limit,
                    false,
                    &runtime,
                )
            }
            Command::Mutate { query } => {
                let mut prepared = None;
                let result = self.tql_mut_admitted(
                    &query,
                    u64::MAX - 1,
                    Some(options.max_operations),
                    Some(active.control.clone()),
                    |affected, created| {
                        if affected > options.max_operations {
                            return Err(TriviumError::ResourceBudgetExceeded {
                                resource: "console_operations",
                                requested: affected,
                                used: 0,
                                limit: options.max_operations,
                            });
                        }
                        prepared = Some(preflight_ack(
                            &request.request_id,
                            Some(affected),
                            created,
                            limit,
                            &runtime,
                        )?);
                        active.control.check()
                    },
                )?;
                prepared.ok_or_else(||invalid("写入响应未准入 (Write response was not admitted)"))?.complete(&json!({"affected":result.affected,"createdIds":strings(result.created_ids)}))
            }
            Command::GetNodes { ids } => {
                check_count(ids.len(), options.max_operations)?;
                encode_nodes(self, &request.request_id, ids, &active.control, limit)
            }
            Command::Search {
                vector,
                top_k,
                expand_depth,
                min_score,
            } => {
                check_count(top_k, options.max_operations)?;
                let vector = decode_vector::<T>(vector)?;
                let hits = self.search(&vector, top_k, expand_depth, min_score)?;
                active.control.check()?;
                #[derive(Serialize)]
                struct Hit<'a> {
                    id: String,
                    score: f64,
                    payload: &'a Value,
                }
                #[derive(Serialize)]
                struct Hits<'a> {
                    hits: Vec<Hit<'a>>,
                }
                let output = Hits {
                    hits: hits
                        .iter()
                        .map(|hit| Hit {
                            id: hit.id.to_string(),
                            score: f64::from(hit.score),
                            payload: &hit.payload,
                        })
                        .collect(),
                };
                encode_response(&request.request_id, &output, limit, false, &runtime)
            }
            Command::Subgraph {
                source,
                depth,
                labels,
                max_nodes,
            } => {
                check_count(max_nodes, options.max_operations)?;
                let graph = self.query_subgraph(
                    id(&source)?,
                    &crate::ReachabilityConfig {
                        max_depth: depth,
                        labels,
                        max_results: max_nodes,
                        max_visited_nodes: max_nodes,
                        max_edges: options.max_operations,
                        ..Default::default()
                    },
                )?;
                #[derive(Serialize)]
                struct GraphNode<'a> {
                    id: String,
                    payload: &'a Value,
                }
                #[derive(Serialize)]
                struct Graph<'a> {
                    nodes: Vec<GraphNode<'a>>,
                    edges: Vec<EdgeOutput<'a>>,
                    truncated: bool,
                }
                let output = Graph {
                    nodes: graph
                        .nodes
                        .iter()
                        .map(|node| GraphNode {
                            id: node.id.to_string(),
                            payload: &node.payload,
                        })
                        .collect(),
                    edges: graph
                        .edges
                        .iter()
                        .map(|edge| EdgeOutput {
                            source: Some(edge.source_id),
                            target: edge.target_id,
                            label: &edge.label,
                            weight: edge.weight,
                            metadata: &edge.metadata,
                            typed: false,
                        })
                        .collect(),
                    truncated: graph.truncated,
                };
                encode_response(&request.request_id, &output, limit, false, &runtime)
            }
            Command::Edges { node, incoming } => {
                let node_id = id(&node)?;
                #[derive(Serialize)]
                struct Edges<'a> {
                    edges: Vec<EdgeOutput<'a>>,
                }
                if incoming {
                    let edges = self.get_incoming_edges(node_id, None)?;
                    if !edges.is_empty() {
                        check_count(edges.len(), options.max_operations)?;
                    }
                    let output = Edges {
                        edges: edges
                            .iter()
                            .map(|edge| EdgeOutput {
                                source: Some(edge.source_id),
                                target: edge.target_id,
                                label: &edge.label,
                                weight: edge.weight,
                                metadata: &edge.metadata,
                                typed: false,
                            })
                            .collect(),
                    };
                    encode_response(&request.request_id, &output, limit, false, &runtime)
                } else {
                    let edges = self.get_edges(node_id)?;
                    if !edges.is_empty() {
                        check_count(edges.len(), options.max_operations)?;
                    }
                    let output = Edges {
                        edges: edges
                            .iter()
                            .map(|edge| EdgeOutput {
                                source: Some(node_id),
                                target: edge.target_id,
                                label: &edge.label,
                                weight: edge.weight,
                                metadata: &edge.metadata,
                                typed: false,
                            })
                            .collect(),
                    };
                    encode_response(&request.request_id, &output, limit, false, &runtime)
                }
            }
            Command::Path {
                source,
                target,
                max_depth,
                label,
            } => {
                let path =
                    self.shortest_path(id(&source)?, id(&target)?, max_depth, label.as_deref())?;
                respond(&json!({"path":path.map(strings)}), false)
            }
            Command::Indexes => respond(&json!({"indexes":self.index_info()?}), false),
            Command::ManageIndex {
                kind,
                fields,
                create,
            } => {
                let prepared = prepare_response(
                    &request.request_id,
                    &json!({"changed":true}),
                    limit,
                    &runtime,
                )?;
                self.console_index(&kind, &fields, create)?;
                prepared.into_response()
            }
            Command::BuildQuiver => {
                let prepared =
                    prepare_response(&request.request_id, &json!({"built":true}), limit, &runtime)?;
                self.build_quiver_index(None)?;
                prepared.into_response()
            }
            Command::Transaction { operations } => {
                check_count(operations.len(), options.max_operations)?;
                let prepared = preflight_ack(
                    &request.request_id,
                    None,
                    operations
                        .iter()
                        .filter(|op| matches!(op, Operation::Insert { .. }))
                        .count(),
                    limit,
                    &runtime,
                )?;
                let mut builder = TxBuilder::new();
                for operation in operations {
                    append_operation(&mut builder, operation)?;
                }
                active.control.check()?;
                let ids = self.commit_tx(builder)?;
                prepared.complete(&json!({"createdIds":strings(ids)}))
            }
            Command::ImportNodes { records } => {
                check_count(records.len(), options.max_operations)?;
                let prepared =
                    preflight_ack(&request.request_id, None, records.len(), limit, &runtime)?;
                let mut builder = TxBuilder::new();
                for record in records {
                    append_operation(&mut builder, Operation::Insert { record })?;
                }
                active.control.check()?;
                let ids = self.commit_tx(builder)?;
                prepared.complete(&json!({"createdIds":strings(ids)}))
            }
            Command::ImportEdges { records } => {
                check_count(records.len(), options.max_operations)?;
                let count = records.len();
                let prepared = prepare_response(
                    &request.request_id,
                    &json!({"imported":count}),
                    limit,
                    &runtime,
                )?;
                let mut builder = TxBuilder::new();
                for record in records {
                    append_operation(&mut builder, Operation::Link { record })?;
                }
                active.control.check()?;
                self.commit_tx(builder)?;
                prepared.into_response()
            }
        }
    }

    fn console_index(&mut self, kind: &str, fields: &[String], create: bool) -> Result<()> {
        if fields.iter().any(|field| field.trim().is_empty()) {
            return Err(invalid("索引字段不得为空 (Index fields must not be empty)"));
        }
        match (kind, fields, create) {
            ("hash", [field], true) => self.create_index(field),
            ("hash" | "unique", [field], false) => self.drop_index(field),
            ("ordered", [field], true) => self.create_ordered_index(field),
            ("ordered", [field], false) => self.drop_ordered_index(field),
            ("bitmap", [field], true) => self.create_bitmap_index(field),
            ("bitmap", [field], false) => self.drop_bitmap_index(field),
            ("ngram", [field], true) => self.create_ngram_index(field),
            ("ngram", [field], false) => self.drop_ngram_index(field),
            ("unique", [field], true) => self.create_unique_index(field),
            ("composite", fields, true) if fields.len() >= 2 => self.create_composite_index(fields),
            ("uniqueComposite", fields, true) if fields.len() >= 2 => {
                self.create_unique_composite_index(fields)
            }
            ("composite" | "uniqueComposite", fields, false) if fields.len() >= 2 => {
                self.drop_composite_index(fields)
            }
            _ => Err(invalid(
                "索引类别或字段数量无效 (Invalid index kind or field count)",
            )),
        }
    }
}

fn check_count(count: usize, limit: usize) -> Result<()> {
    if count == 0 || count > limit {
        return Err(TriviumError::ResourceBudgetExceeded {
            resource: "console_operations",
            requested: count,
            used: 0,
            limit,
        });
    }
    Ok(())
}
fn strings(ids: Vec<u64>) -> Vec<String> {
    ids.into_iter().map(|id| id.to_string()).collect()
}

fn preflight_ack(
    request_id: &str,
    affected: Option<usize>,
    created: usize,
    limit: usize,
    runtime: &crate::ResourceRuntime,
) -> Result<PreparedResponse> {
    // 所有真实 u64 ID 的十进制编码至多二十字节；完整确认体必须在提交前可交付。
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Ack {
        created_ids: RepeatedIds,
        #[serde(skip_serializing_if = "Option::is_none")]
        affected: Option<usize>,
    }
    prepare_response(
        request_id,
        &Ack {
            created_ids: RepeatedIds(created),
            affected,
        },
        limit,
        runtime,
    )
}

struct RepeatedIds(usize);
impl Serialize for RepeatedIds {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.0))?;
        for _ in 0..self.0 {
            seq.serialize_element("18446744073709551615")?;
        }
        seq.end()
    }
}

fn decode_vector<T: VectorType + serde::de::DeserializeOwned>(
    values: Vec<Value>,
) -> Result<Vec<T>> {
    values.into_iter().map(|value| {
        if !crate::vector::is_binary_vector::<T>() {
            let number=value.as_f64().ok_or_else(||invalid("浮点向量分量必须是数字 (Floating vector components must be numbers)"))?;
            // 浮点入站复用内核 f32 通道；half 的 serde 位模式不属于公开数值协议。
            return Ok(T::from_f32(number as f32));
        }
        let number=match value {
            Value::String(raw) if !raw.is_empty() && raw.bytes().all(|byte|byte.is_ascii_digit()) => raw.parse::<u64>().map_err(|_| invalid("u64 向量分量无效 (Invalid u64 vector component)"))?,
            Value::Number(number) => number.as_u64().filter(|number|*number<=9_007_199_254_740_991)
                .ok_or_else(||invalid("大整数向量分量必须使用字符串 (Large integer vector components require strings)"))?,
            _=>return Err(invalid("u64 向量分量无效 (Invalid u64 vector component)")),
        };
        serde_json::from_value(json!(number)).map_err(|_|invalid("向量分量与 dtype 不匹配 (Vector component does not match dtype)"))
    }).collect()
}
fn append_operation<T: VectorType + serde::de::DeserializeOwned>(
    builder: &mut TxBuilder<T>,
    operation: Operation,
) -> Result<()> {
    match operation {
        Operation::Insert { record } => {
            let vector = decode_vector::<T>(record.vector)?;
            if let Some(raw) = record.id {
                builder.insert_with_id(id(&raw)?, &vector, record.payload);
            } else {
                builder.insert(&vector, record.payload);
            }
        }
        Operation::UpdatePayload { id: raw, payload } => builder.update_payload(id(&raw)?, payload),
        Operation::UpdateVector { id: raw, vector } => {
            builder.update_vector(id(&raw)?, &decode_vector::<T>(vector)?)
        }
        Operation::Delete { id: raw } => builder.delete(id(&raw)?),
        Operation::Link { record } => builder.upsert_edge(
            id(&record.source)?,
            id(&record.target)?,
            &record.label,
            record.weight,
            record.metadata,
        ),
        Operation::Unlink {
            source,
            target,
            label,
        } => {
            if let Some(label) = label {
                builder.unlink_label(id(&source)?, id(&target)?, &label);
            } else {
                builder.unlink(id(&source)?, id(&target)?);
            }
        }
    }
    Ok(())
}

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
    workspace: crate::resources::WorkspaceReservation,
    failure: Option<TriviumError>,
}
impl LimitedWriter {
    fn new(limit: usize, runtime: &crate::ResourceRuntime) -> Result<Self> {
        Ok(Self {
            bytes: Vec::new(),
            limit,
            exceeded: false,
            workspace: runtime.reserve_workspace(0)?,
            failure: None,
        })
    }
    fn json(&mut self, value: &impl Serialize) -> Result<()> {
        serde_json::to_writer(self, value).map_err(|error| {
            invalid(format!(
                "控制台响应编码失败 (Console response encoding failed): {error}"
            ))
        })
    }
    fn raw(&mut self, bytes: &[u8]) -> Result<()> {
        self.write_all(bytes).map_err(Into::into)
    }
    fn verify(&mut self, result: Result<()>, committed: bool) -> Result<()> {
        if self.exceeded {
            return Err(TriviumError::ConsoleResponseTooLarge {
                max_bytes: self.limit,
                committed,
            });
        }
        if let Some(failure) = self.failure.take() {
            return Err(failure);
        }
        result?;
        Ok(())
    }
    fn finish(mut self, result: Result<()>, committed: bool) -> Result<String> {
        self.verify(result, committed)?;
        String::from_utf8(self.bytes)
            .map_err(|_| invalid("控制台响应编码无效 (Invalid console response encoding)"))
    }
}
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(std::io::Error::other(
                "控制台响应大小超限 (Console response size exceeded)",
            ));
        }
        let required = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("响应长度溢出 (Response length overflow)"))?;
        if required > self.bytes.capacity() {
            let capacity = required
                .checked_next_power_of_two()
                .unwrap_or(self.limit)
                .min(self.limit);
            let peak = capacity.checked_add(self.bytes.capacity()).ok_or_else(|| {
                std::io::Error::other("响应分配长度溢出 (Response allocation overflow)")
            })?;
            // 扩容前为新旧缓冲并存的峰值准入；成功后只保留实际容量，失败由凭证析构归还。
            if let Err(error) = self.workspace.resize(peak) {
                self.failure = Some(error);
                return Err(std::io::Error::other(
                    "响应工作区准入失败 (Response workspace admission failed)",
                ));
            }
            if let Err(error) = self
                .bytes
                .try_reserve_exact(capacity.saturating_sub(self.bytes.len()))
            {
                self.failure = Some(TriviumError::CapacityAllocationFailed {
                    reason: format!(
                        "控制台响应缓冲分配失败 (Console response buffer allocation failed): {error}"
                    ),
                });
                return Err(std::io::Error::other(
                    "响应缓冲分配失败 (Response buffer allocation failed)",
                ));
            }
            if let Err(error) = self.workspace.resize(self.bytes.capacity()) {
                self.failure = Some(error);
                return Err(std::io::Error::other(
                    "响应工作区更新失败 (Response workspace update failed)",
                ));
            }
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encode_response(
    data_id: &str,
    data: &impl Serialize,
    limit: usize,
    committed: bool,
    runtime: &crate::ResourceRuntime,
) -> Result<String> {
    let mut writer = LimitedWriter::new(limit, runtime)?;
    let result = writer.json(&Response {
        version: CONSOLE_PROTOCOL_VERSION,
        request_id: data_id,
        data,
    });
    writer.finish(result, committed)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Response<'a, T> {
    version: u32,
    request_id: &'a str,
    data: &'a T,
}

struct PreparedResponse {
    writer: LimitedWriter,
    request_id: String,
}
fn prepare_response(
    request_id: &str,
    data: &impl Serialize,
    limit: usize,
    runtime: &crate::ResourceRuntime,
) -> Result<PreparedResponse> {
    let mut writer = LimitedWriter::new(limit, runtime)?;
    let result = writer.json(&Response {
        version: CONSOLE_PROTOCOL_VERSION,
        request_id,
        data,
    });
    writer.verify(result, false)?;
    Ok(PreparedResponse {
        writer,
        request_id: request_id.to_owned(),
    })
}
impl PreparedResponse {
    fn into_response(self) -> Result<String> {
        self.writer.finish(Ok(()), true)
    }
    fn complete(mut self, data: &impl Serialize) -> Result<String> {
        // 保留确认缓冲及工作区凭证跨越提交，其他实例不能抢走已准入的响应额度。
        self.writer.bytes.clear();
        let result = self.writer.json(&Response {
            version: CONSOLE_PROTOCOL_VERSION,
            request_id: &self.request_id,
            data,
        });
        self.writer.finish(result, true)
    }
}

fn encode_nodes<T: VectorType + Serialize + serde::de::DeserializeOwned>(
    database: &Database<T>,
    request_id: &str,
    ids: Vec<String>,
    control: &QueryControl,
    limit: usize,
) -> Result<String> {
    let mut writer = LimitedWriter::new(limit, &database.resource_runtime)?;
    let result = (|| {
        writer.raw(b"{\"version\":1,\"requestId\":")?;
        writer.json(&request_id)?;
        writer.raw(b",\"data\":{\"nodes\":[")?;
        let mut first = true;
        let mut missing = Vec::new();
        for raw in ids {
            control.check()?;
            if let Some(node) = database.get(id(&raw)?)? {
                if !first {
                    writer.raw(b",")?;
                }
                first = false;
                // 逐节点编码，达到响应预算后不再物化后续节点。
                writer.json(&NodeOutput(&node))?;
            } else {
                missing.push(raw);
            }
        }
        writer.raw(b"],\"missingIds\":")?;
        writer.json(&missing)?;
        writer.raw(b"}}")
    })();
    writer.finish(result, false)
}

struct EdgeOutput<'a> {
    source: Option<u64>,
    target: u64,
    label: &'a str,
    weight: f32,
    metadata: &'a Value,
    typed: bool,
}
impl Serialize for EdgeOutput<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(
            4 + usize::from(self.source.is_some()) + usize::from(self.typed),
        ))?;
        if self.typed {
            map.serialize_entry("type", "edge")?;
        }
        if let Some(source) = self.source {
            map.serialize_entry("source", &source.to_string())?;
        }
        map.serialize_entry("target", &self.target.to_string())?;
        map.serialize_entry("label", self.label)?;
        map.serialize_entry("weight", &f64::from(self.weight))?;
        map.serialize_entry("metadata", self.metadata)?;
        map.end()
    }
}

struct NodeEdges<'a>(&'a [crate::Edge]);
impl Serialize for NodeEdges<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for edge in self.0 {
            seq.serialize_element(&EdgeOutput {
                source: None,
                target: edge.target_id,
                label: &edge.label,
                weight: edge.weight,
                metadata: &edge.metadata,
                typed: false,
            })?;
        }
        seq.end()
    }
}

struct NodeOutput<'a, T>(&'a crate::NodeView<T>);
impl<T: VectorType + Serialize> Serialize for NodeOutput<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let node = self.0;
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("id", &node.id.to_string())?;
        map.serialize_entry("vector", &VectorOutput(&node.vector))?;
        map.serialize_entry("payload", &node.payload)?;
        map.serialize_entry("edges", &NodeEdges(&node.edges))?;
        map.end()
    }
}
struct VectorOutput<'a, T>(&'a [T]);
impl<T: VectorType + Serialize> Serialize for VectorOutput<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for value in self.0 {
            if crate::vector::is_binary_vector::<T>() {
                let raw = serde_json::to_string(value).map_err(serde::ser::Error::custom)?;
                seq.serialize_element(&raw)?;
            } else {
                seq.serialize_element(&f64::from(value.to_f32()))?;
            }
        }
        seq.end()
    }
}
struct ValueOutput<'a, T>(&'a TqlValue<T>);
impl<T: VectorType + Serialize> Serialize for ValueOutput<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self.0 {
            TqlValue::Node(node) => NodeOutput(node).serialize(serializer),
            TqlValue::Edge(edge) => EdgeOutput {
                source: Some(edge.source_id),
                target: edge.target_id,
                label: &edge.label,
                weight: edge.weight,
                metadata: &edge.metadata,
                typed: true,
            }
            .serialize(serializer),
            TqlValue::Int(v) if v.unsigned_abs() > 9_007_199_254_740_991 => {
                v.to_string().serialize(serializer)
            }
            TqlValue::Int(v) => v.serialize(serializer),
            TqlValue::UInt(v) => v.to_string().serialize(serializer),
            TqlValue::Float(v) => v.serialize(serializer),
            TqlValue::String(v) => v.serialize(serializer),
            TqlValue::Bool(v) => v.serialize(serializer),
            TqlValue::Path(v) => v
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .serialize(serializer),
            TqlValue::List(v) => v.serialize(serializer),
            TqlValue::Object(v) => v.serialize(serializer),
            TqlValue::Null => serializer.serialize_none(),
        }
    }
}
struct QueryResult<'a, T> {
    rows: &'a crate::query::tql_executor::TqlValueResult<T>,
}
impl<T: VectorType + Serialize> Serialize for QueryResult<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        struct Rows<'a, T>(&'a crate::query::tql_executor::TqlValueResult<T>);
        impl<T: VectorType + Serialize> Serialize for Rows<'_, T> {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
                for row in self.0 {
                    let row: BTreeMap<_, _> = row
                        .iter()
                        .map(|(key, value)| (key, ValueOutput(value)))
                        .collect();
                    seq.serialize_element(&row)?;
                }
                seq.end()
            }
        }
        let columns: Vec<_> = self
            .rows
            .first()
            .into_iter()
            .flat_map(|row| row.iter())
            .map(|(name, value)| {
                let kind = match value {
                    TqlValue::Node(_) => "node",
                    TqlValue::Edge(_) => "edge",
                    TqlValue::Path(_) => "path",
                    _ => "scalar",
                };
                (name, kind)
            })
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .map(|(name, kind)| json!({"name":name,"kind":kind}))
            .collect();
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("rows", &Rows(self.rows))?;
        map.serialize_entry("rowCount", &self.rows.len())?;
        map.serialize_entry("columns", &columns)?;
        map.end()
    }
}
