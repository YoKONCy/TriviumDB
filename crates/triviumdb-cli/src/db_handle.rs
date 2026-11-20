//! dtype 动态分发层。
//!
//! `triviumdb::Database<T>` 是泛型的（`f32` / `f16` / `u64`）。CLI 在运行时
//! 才知道用户要打开哪种 dtype 的数据库，因此用 [`DbHandle`] 枚举把三种具体
//! 类型擦除成一个统一句柄，并通过 [`dispatch!`] 宏把方法调用转发到内部的
//! 具体 `Database<T>` 实例。
//!
//! 展示查询使用 dtype 无关的 [`CliNode`]，数据导出通过原始 dtype 编码 JSON 数值。

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;

use half::f16;
use serde_json::Value;
use triviumdb::node::{Edge, NodeId, NodeView};
use triviumdb::query::tql_executor::{TqlValue, TqlValueResult};
use triviumdb::{Database, Result, TriviumError, VectorType};

/// 数据库底层数值类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DType {
    F32,
    F16,
    U64,
}

impl DType {
    /// 从字符串解析 dtype（大小写不敏感）。
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "f32" | "float32" | "float" => Ok(DType::F32),
            "f16" | "float16" | "half" => Ok(DType::F16),
            "u64" | "uint64" | "bits" => Ok(DType::U64),
            other => Err(TriviumError::InvalidInput(format!(
                "未知 dtype: '{other}' (支持: f32 / f16 / u64)"
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            DType::F32 => "f32",
            DType::F16 => "f16",
            DType::U64 => "u64",
        }
    }
}

impl std::fmt::Display for DType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// dtype 无关的节点视图，专供 CLI/TUI 展示使用。
///
/// 向量统一通过 [`VectorType::to_f32`] 转换为 `f32`，屏蔽底层 dtype 差异。
#[derive(Debug, Clone)]
pub struct CliNode {
    pub id: NodeId,
    pub vector: Vec<f32>,
    pub payload: Value,
    pub edges: Vec<Edge>,
}

impl CliNode {
    fn from_view<T: VectorType>(v: NodeView<T>) -> Self {
        CliNode {
            id: v.id,
            vector: v.vector.iter().map(|x| x.to_f32()).collect(),
            payload: v.payload,
            edges: v.edges,
        }
    }
}

/// TQL 投影值保留实体和普通 JSON 值的区别。
#[derive(Debug, Clone)]
pub enum CliValue {
    Node(CliNode),
    Json(Value),
}

impl CliValue {
    pub fn as_node(&self) -> Option<&CliNode> {
        match self {
            Self::Node(node) => Some(node),
            Self::Json(_) => None,
        }
    }

    fn from_tql<T: VectorType>(value: TqlValue<T>) -> Self {
        let json = match value {
            TqlValue::Node(node) => return Self::Node(CliNode::from_view(node)),
            TqlValue::Edge(edge) => {
                let mut value = serde_json::json!({
                    "type": "edge", "source": edge.source_id, "target": edge.target_id,
                    "label": edge.label, "weight": edge.weight, "metadata": edge.metadata,
                });
                if let Some(graph_id) = edge.graph_id {
                    value["graph_id"] = graph_id.into();
                }
                value
            }
            TqlValue::Int(value) => value.into(),
            TqlValue::UInt(value) => value.into(),
            TqlValue::Float(value) => value.into(),
            TqlValue::String(value) => value.into(),
            TqlValue::Bool(value) => value.into(),
            TqlValue::Path(ids) => serde_json::json!(ids),
            TqlValue::List(values) => Value::Array(values),
            TqlValue::Object(object) => Value::Object(object),
            TqlValue::Null => Value::Null,
        };
        Self::Json(json)
    }
}

/// TQL 查询的一行：列名 → 投影值。
pub type CliRow = HashMap<String, CliValue>;
/// TQL 查询结果：多行绑定。
pub type CliRows = Vec<CliRow>;

pub fn primary_node(row: &CliRow) -> Option<&CliNode> {
    row.iter()
        .filter_map(|(name, value)| value.as_node().map(|node| (name, node)))
        .min_by_key(|(name, _)| *name)
        .map(|(_, node)| node)
}

/// TQL 写操作结果。
#[derive(Debug, Clone, Default)]
pub struct MutSummary {
    pub affected: usize,
    pub created_ids: Vec<NodeId>,
}

/// dtype 擦除后的统一数据库句柄。
pub enum DbHandle {
    F32(Database<f32>),
    F16(Database<f16>),
    U64(Database<u64>),
}

/// 把方法调用转发到内部具体的 `Database<T>`。
///
/// 利用 match 人体工学（match ergonomics），无论 `$self` 是 `&self` 还是
/// `&mut self`，`$db` 都会被正确地绑定为 `&Database<T>` 或 `&mut Database<T>`。
macro_rules! dispatch {
    ($self:expr, $db:ident => $body:expr) => {
        match $self {
            DbHandle::F32($db) => $body,
            DbHandle::F16($db) => $body,
            DbHandle::U64($db) => $body,
        }
    };
}

fn convert_rows<T: VectorType>(rows: TqlValueResult<T>) -> CliRows {
    rows.into_iter()
        .map(|row| {
            row.into_iter()
                .map(|(k, v)| (k, CliValue::from_tql(v)))
                .collect()
        })
        .collect()
}

impl DbHandle {
    /// 打开（或创建）数据库。
    pub fn open(path: &str, dim: usize, dtype: DType) -> Result<Self> {
        Ok(match dtype {
            DType::F32 => DbHandle::F32(Database::<f32>::open(path, dim)?),
            DType::F16 => DbHandle::F16(Database::<f16>::open(path, dim)?),
            DType::U64 => DbHandle::U64(Database::<u64>::open(path, dim)?),
        })
    }

    /// 打开数据库，`dim` 缺省时尝试从 `.tdb` 文件头嗅探。
    ///
    /// 若文件不存在且未显式给出维度，则报错提示用户用 `--dim` 指定。
    pub fn open_auto(path: &str, dim: Option<usize>, dtype: DType) -> Result<Self> {
        let dim = match dim {
            Some(d) => d,
            None => sniff_header(path).map(|h| h.dim).map_err(|_| {
                TriviumError::InvalidInput(format!(
                    "无法从 '{path}' 嗅探维度（文件不存在或损坏），请用 --dim 显式指定"
                ))
            })?,
        };
        Self::open(path, dim, dtype)
    }

    pub fn open_read_only(path: &str, dim: Option<usize>, dtype: DType) -> Result<Self> {
        let dim = match dim {
            Some(dim) => dim,
            None => sniff_header(path)?.dim,
        };
        Ok(match dtype {
            DType::F32 => DbHandle::F32(Database::<f32>::open_read_only(path, dim)?),
            DType::F16 => DbHandle::F16(Database::<f16>::open_read_only(path, dim)?),
            DType::U64 => DbHandle::U64(Database::<u64>::open_read_only(path, dim)?),
        })
    }

    pub fn has_persisted_quiver(&self) -> Result<bool> {
        dispatch!(self, db => db.has_persisted_quiver())
    }

    pub fn build_pelorus(
        &mut self,
        config: triviumdb::index::pelorus::PelorusConfig,
    ) -> Result<()> {
        dispatch!(self, db => db.build_pelorus_index(config))
    }

    /// 当前句柄的 dtype。
    pub fn dtype(&self) -> DType {
        match self {
            DbHandle::F32(_) => DType::F32,
            DbHandle::F16(_) => DType::F16,
            DbHandle::U64(_) => DType::U64,
        }
    }

    pub fn node_count(&self) -> Result<usize> {
        dispatch!(self, db => db.node_count())
    }

    pub fn dim(&self) -> usize {
        dispatch!(self, db => db.dim())
    }

    // 以下读方法构成 DbHandle 的完整读 API：部分由 REPL/TUI 使用，
    // 其余（contains / get_edges / neighbors）为后续图谱可视化功能预留。
    #[allow(dead_code)]
    pub fn contains(&self, id: NodeId) -> Result<bool> {
        dispatch!(self, db => db.contains(id))
    }

    pub fn get(&self, id: NodeId) -> Result<Option<CliNode>> {
        dispatch!(self, db => db.get(id).map(|node| node.map(CliNode::from_view)))
    }

    /// 导出直接读取原始 dtype，整数指纹通过 JSON 整数保留完整位模式。
    pub fn export_node(&self, id: NodeId) -> Result<Option<Value>> {
        dispatch!(self, db => {
            db.get(id)?.map(|node| {
                let vector = node.vector.into_iter().map(VectorType::to_json_number)
                    .collect::<Result<Vec<_>>>()?;
                Ok(serde_json::json!({
                    "id": node.id, "vector": vector, "payload": node.payload,
                    "edges": node.edges.into_iter().map(|edge| serde_json::json!({
                        "target": edge.target_id, "label": edge.label,
                        "weight": edge.weight, "metadata": edge.metadata,
                    })).collect::<Vec<_>>(),
                }))
            }).transpose()
        })
    }

    #[allow(dead_code)]
    pub fn get_payload(&self, id: NodeId) -> Result<Option<Value>> {
        dispatch!(self, db => db.get_payload(id))
    }

    #[allow(dead_code)]
    pub fn get_edges(&self, id: NodeId) -> Result<Vec<Edge>> {
        dispatch!(self, db => db.get_edges(id))
    }

    pub fn get_all_ids(&self) -> Result<Vec<NodeId>> {
        dispatch!(self, db => db.get_all_ids())
    }

    #[allow(dead_code)]
    pub fn neighbors(&self, id: NodeId, depth: usize) -> Result<Vec<NodeId>> {
        dispatch!(self, db => db.neighbors(id, depth))
    }

    /// 插入节点，向量以 `f32` 给出并按 dtype 自动转换。
    #[cfg(test)]
    pub fn insert_f32(&mut self, vector: &[f32], payload: Value) -> Result<NodeId> {
        dispatch!(self, db => {
            let v: Vec<_> = vector.iter().map(|x| VectorType::from_f32(*x)).collect();
            db.insert(&v, payload)
        })
    }

    /// 创建一条有向带权边。
    #[cfg(test)]
    pub fn link(&mut self, src: NodeId, dst: NodeId, label: &str, weight: f32) -> Result<()> {
        dispatch!(self, db => db.link(src, dst, label, weight))
    }

    /// 向量相似度检索（query 以 f32 给出，按 dtype 转换）。
    ///
    /// 返回按得分降序的 `(id, score, payload)` 列表。
    pub fn search_f32(
        &self,
        vector: &[f32],
        top_k: usize,
        expand: usize,
        min_score: f32,
    ) -> Result<Vec<(NodeId, f32, Value)>> {
        dispatch!(self, db => {
            let v: Vec<_> = vector.iter().map(|x| VectorType::from_f32(*x)).collect();
            let hits = db.search(&v, top_k, expand, min_score)?;
            Ok(hits.into_iter().map(|h| (h.id, h.score, h.payload)).collect())
        })
    }

    pub fn estimated_memory(&self) -> usize {
        dispatch!(self, db => db.estimated_memory())
    }

    /// 执行只读 TQL 查询，结果转换为 dtype 无关的 [`CliRows`]。
    pub fn tql(&self, query: &str) -> Result<CliRows> {
        dispatch!(self, db => {
            let rows = db.tql(query)?;
            Ok(convert_rows(rows))
        })
    }

    /// 执行写 TQL 语句（CREATE / SET / DELETE）。
    pub fn tql_mut(&mut self, query: &str) -> Result<MutSummary> {
        dispatch!(self, db => {
            let r = db.tql_mut(query)?;
            Ok(MutSummary { affected: r.affected, created_ids: r.created_ids })
        })
    }

    pub fn flush(&mut self) -> Result<()> {
        dispatch!(self, db => db.flush())
    }

    pub fn compact(&mut self) -> Result<()> {
        dispatch!(self, db => db.compact())
    }
}

/// 从 `.tdb` 文件头嗅探维度（不挂载数据库）。
///
/// 文件头布局：`[0..4] MAGIC "TVDB"`, `[4..6] version (u16 LE)`,
/// `[6..10] dim (u32 LE)`。
pub fn sniff_header(tdb_path: &str) -> Result<HeaderInfo> {
    let mut file = File::open(tdb_path)?;
    let mut header = [0u8; 10];
    file.read_exact(&mut header)?;

    if &header[0..4] != b"TVDB" {
        return Err(TriviumError::CorruptedFile(format!(
            "非法魔数: '{}' 不是 TriviumDB 文件",
            tdb_path
        )));
    }

    let version = u16::from_le_bytes([header[4], header[5]]);
    if version != triviumdb::storage::file_format::CURRENT_VERSION {
        return Err(TriviumError::UnsupportedDatabaseVersion {
            found: version,
            minimum_supported: triviumdb::storage::file_format::CURRENT_VERSION,
            current: triviumdb::storage::file_format::CURRENT_VERSION,
        });
    }
    let dim = u32::from_le_bytes([header[6], header[7], header[8], header[9]]) as usize;

    Ok(HeaderInfo { version, dim })
}

/// `.tdb` 文件头信息。
#[derive(Debug, Clone, Copy)]
pub struct HeaderInfo {
    pub version: u16,
    pub dim: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── DType ──────────────────────────────────────────────────

    #[test]
    fn dtype_parse_f32_aliases() {
        assert_eq!(DType::parse("f32").unwrap(), DType::F32);
        assert_eq!(DType::parse("float32").unwrap(), DType::F32);
        assert_eq!(DType::parse("FLOAT").unwrap(), DType::F32);
    }

    #[test]
    fn dtype_parse_f16_aliases() {
        assert_eq!(DType::parse("f16").unwrap(), DType::F16);
        assert_eq!(DType::parse("float16").unwrap(), DType::F16);
        assert_eq!(DType::parse("half").unwrap(), DType::F16);
    }

    #[test]
    fn dtype_parse_u64_aliases() {
        assert_eq!(DType::parse("u64").unwrap(), DType::U64);
        assert_eq!(DType::parse("uint64").unwrap(), DType::U64);
        assert_eq!(DType::parse("bits").unwrap(), DType::U64);
    }

    #[test]
    fn dtype_parse_invalid() {
        assert!(DType::parse("i32").is_err());
        assert!(DType::parse("").is_err());
    }

    #[test]
    fn dtype_as_str_roundtrip() {
        for dt in [DType::F32, DType::F16, DType::U64] {
            assert_eq!(DType::parse(dt.as_str()).unwrap(), dt);
        }
    }

    #[test]
    fn dtype_display() {
        assert_eq!(format!("{}", DType::F32), "f32");
        assert_eq!(format!("{}", DType::F16), "f16");
        assert_eq!(format!("{}", DType::U64), "u64");
    }

    // ── DbHandle ───────────────────────────────────────────────

    #[test]
    fn dbhandle_open_insert_get() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.tdb").to_string_lossy().to_string();
        let mut h = DbHandle::open(&path, 4, DType::F32).unwrap();

        let id = h
            .insert_f32(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "Alice"}))
            .unwrap();
        assert_eq!(h.node_count().unwrap(), 1);
        assert_eq!(h.dim(), 4);
        assert_eq!(h.dtype(), DType::F32);

        let node = h.get(id).unwrap().unwrap();
        assert_eq!(node.payload["name"], "Alice");
        assert_eq!(node.vector.len(), 4);
    }

    #[test]
    fn dbhandle_tql_match_returns_all() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.tdb").to_string_lossy().to_string();
        let mut h = DbHandle::open(&path, 4, DType::F32).unwrap();
        h.insert_f32(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "Alice"}))
            .unwrap();
        h.insert_f32(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "Bob"}))
            .unwrap();

        let rows = h.tql("MATCH (n) RETURN n").unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].contains_key("n"));
    }

    #[test]
    fn dbhandle_search_f32_top1() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.tdb").to_string_lossy().to_string();
        let mut h = DbHandle::open(&path, 4, DType::F32).unwrap();
        h.insert_f32(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "Alice"}))
            .unwrap();
        h.insert_f32(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "Bob"}))
            .unwrap();

        let hits = h.search_f32(&[1.0, 0.0, 0.0, 0.0], 1, 0, 0.0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].2["name"], "Alice");
    }

    #[test]
    fn dbhandle_link_and_get_edges() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.tdb").to_string_lossy().to_string();
        let mut h = DbHandle::open(&path, 4, DType::F32).unwrap();
        let a = h
            .insert_f32(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
        let b = h
            .insert_f32(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
        h.link(a, b, "knows", 1.0).unwrap();

        let edges = h.get_edges(a).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target_id, b);
        assert_eq!(edges[0].label, "knows");
    }

    #[test]
    fn dbhandle_flush_and_compact() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.tdb").to_string_lossy().to_string();
        let mut h = DbHandle::open(&path, 4, DType::F32).unwrap();
        h.insert_f32(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
        h.flush().unwrap();
        h.compact().unwrap();
        assert_eq!(h.node_count().unwrap(), 1);
    }

    // ── sniff_header ───────────────────────────────────────────

    #[test]
    fn sniff_header_on_flushed_db() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.tdb").to_string_lossy().to_string();
        {
            let mut h = DbHandle::open(&path, 8, DType::F32).unwrap();
            h.insert_f32(&[0.0; 8], serde_json::json!({})).unwrap();
            h.flush().unwrap();
        }
        let info = sniff_header(&path).unwrap();
        assert_eq!(info.dim, 8);
        assert_eq!(
            info.version,
            triviumdb::storage::file_format::CURRENT_VERSION
        );
    }

    #[test]
    fn sniff_header拒绝不支持的数据库版本() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("unsupported.tdb");
        let mut bytes = [0u8; 10];
        bytes[0..4].copy_from_slice(b"TVDB");
        bytes[4..6]
            .copy_from_slice(&(triviumdb::storage::file_format::CURRENT_VERSION + 1).to_le_bytes());
        bytes[6..10].copy_from_slice(&8u32.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert!(matches!(
            sniff_header(path.to_str().unwrap()),
            Err(TriviumError::UnsupportedDatabaseVersion { .. })
        ));
    }

    #[test]
    fn sniff_header_nonexistent_file() {
        assert!(sniff_header("__nonexistent__.tdb").is_err());
    }

    #[test]
    fn open_auto_with_explicit_dim() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("new.tdb").to_string_lossy().to_string();
        let h = DbHandle::open_auto(&path, Some(4), DType::F32).unwrap();
        assert_eq!(h.dim(), 4);
    }

    #[test]
    fn open_auto_without_dim_on_nonexistent_fails() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("no.tdb").to_string_lossy().to_string();
        assert!(DbHandle::open_auto(&path, None, DType::F32).is_err());
    }
}
