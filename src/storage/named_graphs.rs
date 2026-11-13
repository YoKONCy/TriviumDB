//! 命名图事实。不可变记录共享用于提交前投影，边的身份由所属图限定。

use crate::named_graph::*;
use crate::{NodeId, Result, TriviumError};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Default)]
pub(crate) struct NamedGraphStore {
    pub(crate) graphs: BTreeMap<String, Arc<NamedGraphSnapshot>>,
    bytes: usize,
}

impl NamedGraphStore {
    /// 完整记录覆盖允许检查点已发布而WAL尚未清除时重放同一事务。
    pub(crate) fn replay_change(
        &mut self,
        id: &str,
        expected: Option<&str>,
        snapshot: Option<NamedGraphSnapshot>,
    ) -> Result<()> {
        validate_id(id).map_err(corrupt)?;
        if let Some(snapshot) = snapshot {
            validate_stored_snapshot(&snapshot)?;
            let valid_revision = match expected {
                Some(previous) => snapshot.revision == next_revision(previous).map_err(corrupt)?,
                None => revision_parts(&snapshot.revision).map_err(corrupt)?.1 == 1,
            };
            if snapshot.graph.graph_id != id || !valid_revision {
                return Err(corrupt("WAL identity or revision"));
            }
            self.install(snapshot);
        } else {
            revision_parts(expected.ok_or_else(|| corrupt("missing expected revision"))?)
                .map_err(corrupt)?;
            self.remove(id);
        }
        Ok(())
    }
    pub(crate) fn memory_bytes(&self) -> usize {
        self.bytes
    }
    fn record_bytes(id: &str, record: &NamedGraphSnapshot) -> usize {
        id.len()
            .saturating_add(record.graph.memory_bytes())
            .saturating_add(record.revision.capacity())
            .saturating_add(128)
    }
    fn remove(&mut self, id: &str) {
        if let Some(previous) = self.graphs.remove(id) {
            self.bytes = self.bytes.saturating_sub(Self::record_bytes(id, &previous));
        }
    }
    fn install(&mut self, snapshot: NamedGraphSnapshot) {
        self.remove(&snapshot.graph.graph_id);
        self.bytes = self
            .bytes
            .saturating_add(Self::record_bytes(&snapshot.graph.graph_id, &snapshot));
        self.graphs
            .insert(snapshot.graph.graph_id.clone(), Arc::new(snapshot));
    }
    pub(crate) fn projection_bytes(&self) -> usize {
        if self.graphs.is_empty() {
            return 0;
        }
        self.memory_bytes().saturating_mul(3).saturating_add(128)
    }
    pub(crate) fn check(&self, id: &str, expected: Option<&str>) -> Result<()> {
        let actual = self.graphs.get(id).map(|record| record.revision.as_str());
        if actual != expected {
            return Err(TriviumError::GraphRevisionMismatch {
                graph_id: id.into(),
                expected: expected.unwrap_or("absent").into(),
                actual: actual.unwrap_or("absent").into(),
            }
            .at_field("expected_revision"));
        }
        Ok(())
    }
    pub(crate) fn change(
        &mut self,
        id: &str,
        expected: Option<&str>,
        snapshot: Option<&NamedGraphSnapshot>,
    ) -> Result<()> {
        validate_id(id)?;
        self.check(id, expected)?;
        if let Some(snapshot) = snapshot {
            bound(
                "named_graph_count",
                self.graphs
                    .len()
                    .saturating_add(usize::from(expected.is_none())),
                10_000,
            )?;
            validate_snapshot(snapshot)?;
            if snapshot.graph.graph_id != id {
                return Err(invalid(
                    "graph",
                    "图快照身份或版本无效 (Invalid graph snapshot identity or version)",
                ));
            }
            let (_, counter) = revision_parts(&snapshot.revision)?;
            if let Some(previous) = expected {
                if snapshot.revision != next_revision(previous)? {
                    return Err(invalid(
                        "revision",
                        "图修订必须连续递增 (Graph revision must advance consecutively)",
                    ));
                }
            } else if counter != 1 {
                return Err(invalid(
                    "revision",
                    "新图修订必须从1开始 (New graph revision must start at one)",
                ));
            }
            self.install(snapshot.clone());
        } else {
            if expected.is_none() {
                return Err(invalid(
                    "expected_revision",
                    "删除图需要预期修订 (Deleting a graph requires an expected revision)",
                ));
            }
            self.remove(id);
        }
        Ok(())
    }
    pub(crate) fn delete_node(&mut self, id: NodeId) -> Result<()> {
        for (graph_id, record) in &mut self.graphs {
            if let Ok(index) = record.graph.members.binary_search(&id) {
                let revision = next_revision(&record.revision)?;
                let before = Self::record_bytes(graph_id, record);
                let record = Arc::make_mut(record);
                record.graph.members.remove(index);
                record
                    .graph
                    .edges
                    .retain(|edge| edge.source_id != id && edge.target_id != id);
                record.revision = revision;
                self.bytes = self
                    .bytes
                    .saturating_sub(before)
                    .saturating_add(Self::record_bytes(graph_id, record));
            }
        }
        Ok(())
    }
}

fn validate_snapshot(snapshot: &NamedGraphSnapshot) -> Result<()> {
    crate::request::check_current()?;
    if snapshot.schema_version != 1 {
        return Err(invalid(
            "schema_version",
            "图快照版本无效 (Invalid graph snapshot version)",
        ));
    }
    revision_parts(&snapshot.revision)?;
    snapshot.graph.validate_input()?;
    let mut normalized = snapshot.graph.clone();
    normalized.normalize()?;
    if normalized != snapshot.graph {
        return Err(invalid(
            "graph",
            "持久图快照必须按键排序 (Stored graph snapshots must be key-sorted)",
        ));
    }
    Ok(())
}

fn validate_stored_snapshot(snapshot: &NamedGraphSnapshot) -> Result<()> {
    validate_snapshot(snapshot).map_err(|error| {
        // 请求控制保持原类别，持久事实的语义拒绝转换为结构化损坏。
        if matches!(
            error.underlying(),
            TriviumError::QueryCancelled | TriviumError::QueryDeadlineExceeded
        ) {
            error
        } else {
            corrupt(error)
        }
    })
}

pub(crate) fn corrupt(error: impl std::fmt::Display) -> TriviumError {
    TriviumError::CorruptedFile(format!(
        "命名图事实无效 (Invalid named graph facts): {error}"
    ))
}

/// 长度和CRC在解析JSON与分配对象之前验证，单条记录受固定格式上限约束。
pub(crate) fn split_checkpoint(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    let footer = bytes
        .len()
        .checked_sub(16)
        .ok_or_else(|| corrupt("footer"))?;
    if &bytes[footer + 12..] != b"NGRF" {
        return Err(corrupt("magic"));
    }
    let len = usize::try_from(u64::from_le_bytes(
        bytes[footer..footer + 8].try_into().map_err(corrupt)?,
    ))
    .map_err(corrupt)?;
    let start = footer.checked_sub(len).ok_or_else(|| corrupt("length"))?;
    let data = &bytes[start..footer];
    let crc = u32::from_le_bytes(bytes[footer + 8..footer + 12].try_into().map_err(corrupt)?);
    if crc32fast::hash(data) != crc {
        return Err(corrupt("CRC32"));
    }
    Ok((&bytes[..start], data))
}

pub(crate) fn save<W: std::io::Write>(store: &NamedGraphStore, writer: &mut W) -> Result<()> {
    let mut crc = crc32fast::Hasher::new();
    let count = (store.graphs.len() as u64).to_le_bytes();
    writer.write_all(&count)?;
    crc.update(&count);
    let mut total = 8u64;
    for record in store.graphs.values() {
        crate::request::check_current()?;
        let bytes = serde_json::to_vec(record.as_ref()).map_err(corrupt)?;
        bound("named_graph_bytes", bytes.len(), MAX_GRAPH_BYTES + 1024)?;
        let len = (bytes.len() as u64).to_le_bytes();
        writer.write_all(&len)?;
        writer.write_all(&bytes)?;
        crc.update(&len);
        crc.update(&bytes);
        total = total
            .checked_add(8 + bytes.len() as u64)
            .ok_or_else(|| corrupt("length overflow"))?;
    }
    writer.write_all(&total.to_le_bytes())?;
    writer.write_all(&crc.finalize().to_le_bytes())?;
    writer.write_all(b"NGRF")?;
    Ok(())
}

pub(crate) fn restore<T: crate::VectorType>(
    mt: &mut crate::storage::memtable::MemTable<T>,
    mut bytes: &[u8],
) -> Result<()> {
    fn number(bytes: &mut &[u8]) -> Result<usize> {
        let raw = bytes.get(..8).ok_or_else(|| corrupt("truncated length"))?;
        let value = usize::try_from(u64::from_le_bytes(raw.try_into().map_err(corrupt)?))
            .map_err(corrupt)?;
        *bytes = &bytes[8..];
        Ok(value)
    }
    let count = number(&mut bytes)?;
    if count > 10_000 || count > bytes.len() / 8 {
        return Err(corrupt("count"));
    }
    for _ in 0..count {
        let len = number(&mut bytes)?;
        if len > MAX_GRAPH_BYTES + 1024 {
            return Err(corrupt("record length"));
        }
        let record = bytes
            .get(..len)
            .ok_or_else(|| corrupt("truncated record"))?;
        mt.check_load_allocation(len.saturating_mul(64))?;
        let snapshot: NamedGraphSnapshot = serde_json::from_slice(record).map_err(corrupt)?;
        for id in &snapshot.graph.members {
            if !mt.contains(*id) {
                return Err(corrupt("missing member"));
            }
        }
        validate_stored_snapshot(&snapshot)?;
        if mt
            .named_graphs
            .graphs
            .contains_key(&snapshot.graph.graph_id)
        {
            return Err(corrupt("duplicate graph identity"));
        }
        mt.named_graphs.install(snapshot);
        bytes = &bytes[len..];
    }
    if !bytes.is_empty() {
        return Err(corrupt("trailing data"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::memtable::MemTable;

    fn block(body: &[u8]) -> Vec<u8> {
        let mut bytes = body.to_vec();
        bytes.extend_from_slice(&(body.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&crc32fast::hash(body).to_le_bytes());
        bytes.extend_from_slice(b"NGRF");
        bytes
    }

    #[test]
    fn 恢复请求取消保留控制类别且图事实保持原状() {
        let raw = br#"{"schema_version":1,"revision":"8b34f182-5c2e-4ed2-823c-09a4e0be8946:1","graph_id":"cancelled","members":[],"edges":[]}"#;
        let mut body = 1u64.to_le_bytes().to_vec();
        body.extend_from_slice(&(raw.len() as u64).to_le_bytes());
        body.extend_from_slice(raw);
        let mut mt = MemTable::<f32>::new(2);
        let context =
            crate::sdk::RequestContext::new("named-graph-restore", Default::default(), false)
                .unwrap();
        let result: Result<()> = context.run(|| {
            context.cancel();
            let error = restore(&mut mt, &body).unwrap_err();
            assert_eq!(error.code(), "TDB_QUERY_CANCELLED");
            assert!(mt.named_graphs.graphs.is_empty());
            assert_eq!(mt.node_count(), 0);
            Err(error)
        });
        assert_eq!(result.unwrap_err().code(), "TDB_QUERY_CANCELLED");
    }

    #[test]
    fn 独立编码样例校验长度校验和截断与逐字节损坏() {
        let raw = br#"{"schema_version":1,"revision":"8b34f182-5c2e-4ed2-823c-09a4e0be8946:7","graph_id":"empty","metadata":{"k":9},"members":[],"edges":[]}"#;
        let mut body = 1u64.to_le_bytes().to_vec();
        body.extend_from_slice(&(raw.len() as u64).to_le_bytes());
        body.extend_from_slice(raw);
        let encoded = block(&body);
        let (prefix, data) = split_checkpoint(&encoded).unwrap();
        assert!(prefix.is_empty());
        let mut mt = MemTable::<f32>::new(2);
        restore(&mut mt, data).unwrap();
        let record = &mt.named_graphs.graphs["empty"];
        assert_eq!(record.revision, "8b34f182-5c2e-4ed2-823c-09a4e0be8946:7");
        assert_eq!(record.graph.metadata, serde_json::json!({"k":9}));
        assert!(record.graph.members.is_empty());
        assert!(record.graph.edges.is_empty());
        let mut written = Vec::new();
        save(&mt.named_graphs, &mut written).unwrap();
        assert_eq!(written, encoded);
        for end in 0..encoded.len() {
            assert_eq!(
                split_checkpoint(&encoded[..end]).unwrap_err().code(),
                "TDB_CORRUPTED_FILE"
            );
        }
        for index in 0..encoded.len() {
            let mut changed = encoded.clone();
            changed[index] ^= 1;
            assert_eq!(
                split_checkpoint(&changed).unwrap_err().code(),
                "TDB_CORRUPTED_FILE",
                "字节{index}"
            );
        }
    }

    #[test]
    fn 完整校验和的恶意数量长度重复身份与残余字节被拒绝() {
        let invalid = [
            u64::MAX.to_le_bytes().to_vec(),
            [1u64.to_le_bytes(), u64::MAX.to_le_bytes()].concat(),
            [0u64.to_le_bytes().as_slice(), &[0]].concat(),
        ];
        for body in invalid {
            let encoded = block(&body);
            let (_, body) = split_checkpoint(&encoded).unwrap();
            let mut mt = MemTable::<f32>::new(2);
            assert_eq!(
                restore(&mut mt, body).unwrap_err().code(),
                "TDB_CORRUPTED_FILE"
            );
        }
        let raw = br#"{"schema_version":1,"revision":"8b34f182-5c2e-4ed2-823c-09a4e0be8946:1","graph_id":"duplicate","members":[],"edges":[]}"#;
        let mut body = 2u64.to_le_bytes().to_vec();
        for _ in 0..2 {
            body.extend_from_slice(&(raw.len() as u64).to_le_bytes());
            body.extend_from_slice(raw);
        }
        let mut mt = MemTable::<f32>::new(2);
        assert_eq!(
            restore(&mut mt, &body).unwrap_err().code(),
            "TDB_CORRUPTED_FILE"
        );
    }
}
