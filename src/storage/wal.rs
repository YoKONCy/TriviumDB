//! Write-Ahead Log 的版本化格式、写入与崩溃恢复。
//!
//! 每条记录采用 `len + bincode + CRC32`，事务通过 Begin/Commit 封条过滤未提交操作。
//! 物理未写完的尾帧只恢复完整前缀；完整帧的 CRC、长度或解码损坏返回错误并保留日志。
//! 恢复不跨过损坏边界猜测后续字节。旧版本头只允许持有 Writer 排他锁的打开流程升级；
//! WAL v2 通过独立历史类型解码并原子转写为 v3，未知非空版本始终 fail-closed。

use crate::error::{Result, TriviumError};
use crate::node::NodeId;
use crate::storage::fs::robust_rename_and_sync;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;

const WAL_MAGIC: &[u8; 4] = b"TVWL";
pub const WAL_VERSION: u16 = 3;
const LEGACY_WAL_VERSION_V2: u16 = 2;
const WAL_HEADER_SIZE: u64 = 6;
const MAX_WAL_ENTRY_SIZE: usize = 256 * 1024 * 1024;

/// WAL 条目：记录每一次变更操作
///
/// 注意：payload 使用 String（JSON 字符串）而非 serde_json::Value，
/// 因为 bincode 不支持 serde_json::Value 的 deserialize_any 方法。
#[derive(Debug, Serialize, Deserialize)]
pub enum WalEntry<T> {
    TxBegin {
        tx_id: u64,
    },
    TxCommit {
        tx_id: u64,
    },
    Insert {
        id: NodeId,
        vector: Vec<T>,
        payload: String, // JSON 字符串
    },
    Link {
        src: NodeId,
        dst: NodeId,
        label: String,
        weight: f32,
        #[serde(default)]
        metadata: String,
    },
    Delete {
        id: NodeId,
    },
    Unlink {
        src: NodeId,
        dst: NodeId,
    },
    UpdatePayload {
        id: NodeId,
        payload: String, // JSON 字符串
    },
    UpdateVector {
        id: NodeId,
        vector: Vec<T>,
    },
    /// 仅删除指定标签边；追加在旧变体之后，保持历史 discriminant。
    UnlinkLabel {
        src: NodeId,
        dst: NodeId,
        label: String,
    },
}

/// WAL v2 的精确磁盘枚举布局；仅用于迁移，禁止用于新写入。
#[derive(Debug, Deserialize)]
enum WalEntryV2<T> {
    TxBegin {
        tx_id: u64,
    },
    TxCommit {
        tx_id: u64,
    },
    Insert {
        id: NodeId,
        vector: Vec<T>,
        payload: String,
    },
    Link {
        src: NodeId,
        dst: NodeId,
        label: String,
        weight: f32,
    },
    Delete {
        id: NodeId,
    },
    Unlink {
        src: NodeId,
        dst: NodeId,
    },
    UpdatePayload {
        id: NodeId,
        payload: String,
    },
    UpdateVector {
        id: NodeId,
        vector: Vec<T>,
    },
    UnlinkLabel {
        src: NodeId,
        dst: NodeId,
        label: String,
    },
}

impl<T> From<WalEntryV2<T>> for WalEntry<T> {
    fn from(entry: WalEntryV2<T>) -> Self {
        match entry {
            WalEntryV2::TxBegin { tx_id } => Self::TxBegin { tx_id },
            WalEntryV2::TxCommit { tx_id } => Self::TxCommit { tx_id },
            WalEntryV2::Insert {
                id,
                vector,
                payload,
            } => Self::Insert {
                id,
                vector,
                payload,
            },
            WalEntryV2::Link {
                src,
                dst,
                label,
                weight,
            } => Self::Link {
                src,
                dst,
                label,
                weight,
                metadata: "null".into(),
            },
            WalEntryV2::Delete { id } => Self::Delete { id },
            WalEntryV2::Unlink { src, dst } => Self::Unlink { src, dst },
            WalEntryV2::UpdatePayload { id, payload } => Self::UpdatePayload { id, payload },
            WalEntryV2::UpdateVector { id, vector } => Self::UpdateVector { id, vector },
            WalEntryV2::UnlinkLabel { src, dst, label } => Self::UnlinkLabel { src, dst, label },
        }
    }
}

fn decode_legacy_v2_frames<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
) -> Result<Vec<WalEntry<T>>> {
    let mut offset = 0usize;
    let mut entries = Vec::new();
    while offset < bytes.len() {
        let len_end = offset
            .checked_add(4)
            .ok_or_else(|| TriviumError::CorruptedFile("WAL v2 帧偏移溢出".into()))?;
        let len_bytes = bytes
            .get(offset..len_end)
            .ok_or_else(|| TriviumError::CorruptedFile("WAL v2 尾部长度字段被截断".into()))?;
        let len = u32::from_le_bytes(
            len_bytes
                .try_into()
                .map_err(|_| TriviumError::CorruptedFile("WAL v2 长度字段无效".into()))?,
        ) as usize;
        if len > MAX_WAL_ENTRY_SIZE {
            return Err(TriviumError::CorruptedFile(format!(
                "WAL v2 单条记录超过上限：{} 字节",
                len
            )));
        }
        let data_end = len_end
            .checked_add(len)
            .ok_or_else(|| TriviumError::CorruptedFile("WAL v2 数据长度溢出".into()))?;
        let crc_end = data_end
            .checked_add(4)
            .ok_or_else(|| TriviumError::CorruptedFile("WAL v2 CRC 偏移溢出".into()))?;
        let data = bytes
            .get(len_end..data_end)
            .ok_or_else(|| TriviumError::CorruptedFile("WAL v2 记录数据被截断".into()))?;
        let crc_bytes = bytes
            .get(data_end..crc_end)
            .ok_or_else(|| TriviumError::CorruptedFile("WAL v2 记录 CRC 被截断".into()))?;
        let stored_crc = u32::from_le_bytes(
            crc_bytes
                .try_into()
                .map_err(|_| TriviumError::CorruptedFile("WAL v2 CRC 字段无效".into()))?,
        );
        if crc32fast::hash(data) != stored_crc {
            return Err(TriviumError::CorruptedFile(
                "WAL v2 记录 CRC 不匹配，拒绝迁移".into(),
            ));
        }
        let entry = bincode::deserialize::<WalEntryV2<T>>(data).map_err(|error| {
            TriviumError::CorruptedFile(format!("WAL v2 记录无法解码，拒绝迁移：{error}"))
        })?;
        entries.push(entry.into());
        offset = crc_end;
    }
    Ok(entries)
}

fn write_framed_entry<T: serde::Serialize>(
    writer: &mut impl Write,
    entry: &WalEntry<T>,
) -> Result<()> {
    let data = bincode::serialize(entry).map_err(TriviumError::Serialization)?;
    let len = u32::try_from(data.len())
        .map_err(|_| TriviumError::InvalidInput("WAL 记录超过 u32 长度上限".into()))?;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&data)?;
    writer.write_all(&crc32fast::hash(&data).to_le_bytes())?;
    Ok(())
}

/// WAL 磁盘同步模式
///
/// 控制每条 WAL 写入后是否强制落盘，在速度和安全之间权衡。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SyncMode {
    /// 每条 WAL 写入后立即 fsync（最安全，防 OS 崩溃丢数据）
    /// 适用于：金融数据、不可丢失的关键业务
    Full,
    /// 每条写入 flush 到 OS 缓冲区，但不 fsync（平衡模式）
    /// 进程崩溃不丢数据，OS 崩溃可能丢最近几条
    /// 适用于：大多数生产场景
    #[default]
    Normal,
    /// 不主动 flush，完全依赖 OS 缓冲（最快，仅用于测试）
    Off,
}

impl SyncMode {
    /// 从字符串解析同步模式（"full" / "normal" / "off"）
    ///
    /// 供 Python/Node.js 绑定层统一调用，避免各绑定各自维护一份解析逻辑。
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        match s {
            "full" => Ok(SyncMode::Full),
            "normal" => Ok(SyncMode::Normal),
            "off" => Ok(SyncMode::Off),
            other => Err(format!(
                "不支持的同步模式 (Unsupported sync_mode): '{}'。请使用 'full', 'normal' 或 'off'",
                other
            )),
        }
    }
}

/// Write-Ahead Logger
/// 每次变更先追加写入 .wal 文件，保证崩溃时可恢复。
///
/// 磁盘格式（每条记录）：
///   [len: u32][bincode data: len bytes][crc32: u32]
pub struct Wal {
    wal_path: PathBuf,
    writer: Option<BufWriter<File>>,
    sync_mode: SyncMode,
    cumulative_written_bytes: u64,
    cumulative_logical_bytes: u64,
    group_commit_active: bool,
    group_commit_pending: bool,
    cumulative_sync_count: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WalStats {
    pub written_bytes: u64,
    pub logical_bytes: u64,
    pub sync_count: u64,
}

impl Wal {
    pub(crate) fn ensure_writable(&self) -> Result<()> {
        if self.writer.is_some() {
            Ok(())
        } else {
            Err(TriviumError::WalClosed)
        }
    }

    /// 同步成功才释放句柄；失败时保留写入器供关闭协议报告原始错误。
    pub(crate) fn close_writer(&mut self) -> Result<()> {
        let writer = self.writer.as_mut().ok_or(TriviumError::WalClosed)?;
        writer.flush()?;
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::io_result(crate::test_hooks::IoPoint::WalSync)?;
        writer.get_ref().sync_all()?;
        self.release_writer();
        Ok(())
    }

    /// 无法确认同步结果时关闭写入能力，后续写入必须显式失败。
    pub(crate) fn release_writer(&mut self) {
        self.writer.take();
        self.group_commit_active = false;
        self.group_commit_pending = false;
    }

    pub fn disabled(db_path: &str, sync_mode: SyncMode) -> Self {
        Self {
            wal_path: PathBuf::from(format!("{}.wal", db_path)),
            writer: None,
            sync_mode,
            cumulative_written_bytes: 0,
            cumulative_logical_bytes: 0,
            group_commit_active: false,
            group_commit_pending: false,
            cumulative_sync_count: 0,
        }
    }
    /// 将受支持的旧 WAL 原子迁移到当前版本。
    ///
    /// WAL v2 与 v3 共享帧、CRC 和事务封条格式，唯一记录差异是 v3 的 Link
    /// 新增 metadata。迁移必须完整验证每个物理帧和反序列化结果；任何截断、CRC
    /// 错误或未知记录都会拒绝迁移并保持原文件不变。
    pub fn upgrade_legacy_wal<T>(db_path: &str) -> Result<bool>
    where
        T: serde::de::DeserializeOwned + serde::Serialize,
    {
        let wal_path = PathBuf::from(format!("{}.wal", db_path));
        let mut file = match File::open(&wal_path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        // 当前版本只需检查固定头部，避免每次开库先把整份 WAL 复制入堆。
        let mut header = [0; WAL_HEADER_SIZE as usize];
        match file.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
            Err(error) => return Err(error.into()),
        }
        if &header[..4] != WAL_MAGIC {
            return Ok(false);
        }
        let found = u16::from_le_bytes([header[4], header[5]]);
        if found == WAL_VERSION {
            return Ok(false);
        }
        if found != LEGACY_WAL_VERSION_V2 {
            return Ok(false);
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let entries = decode_legacy_v2_frames::<T>(&bytes)?;
        drop(file);
        let tmp_path = PathBuf::from(format!("{}.upgrade.tmp", wal_path.to_string_lossy()));
        let write_result = (|| -> Result<()> {
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::io_result(crate::test_hooks::IoPoint::WalMigrationCreate)?;
            let mut tmp = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)?;
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::io_result(crate::test_hooks::IoPoint::WalMigrationWrite)?;
            tmp.write_all(WAL_MAGIC)?;
            tmp.write_all(&WAL_VERSION.to_le_bytes())?;
            for entry in &entries {
                write_framed_entry(&mut tmp, entry)?;
            }
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::io_result(crate::test_hooks::IoPoint::WalMigrationSync)?;
            tmp.sync_all()?;
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::io_result(crate::test_hooks::IoPoint::WalMigrationRename)?;
            robust_rename_and_sync(&tmp_path, &wal_path)?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(error);
        }
        tracing::info!(
            found_version = found,
            current_version = WAL_VERSION,
            entry_count = entries.len(),
            wal_path = %wal_path.display(),
            "已原子迁移旧 WAL (Atomically migrated legacy WAL)"
        );
        Ok(true)
    }

    /// 兼容旧调用名；只处理历史空 WAL，非空迁移必须携带真实向量类型。
    pub fn upgrade_empty_legacy_wal(db_path: &str) -> Result<bool> {
        let wal_path = PathBuf::from(format!("{}.wal", db_path));
        if std::fs::metadata(&wal_path).map(|meta| meta.len()).ok() != Some(WAL_HEADER_SIZE) {
            return Ok(false);
        }
        Self::upgrade_legacy_wal::<u8>(db_path)
    }

    /// 创建或打开 WAL 文件（追加模式）
    pub fn open(db_path: &str) -> Result<Self> {
        Self::open_with_sync(db_path, SyncMode::default())
    }

    /// 创建或打开 WAL 文件，指定同步模式
    pub fn open_with_sync(db_path: &str, sync_mode: SyncMode) -> Result<Self> {
        let wal_path = PathBuf::from(format!("{}.wal", db_path));
        let existing_len = std::fs::metadata(&wal_path)
            .map(|meta| meta.len())
            .unwrap_or(0);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&wal_path)?;
        if existing_len == 0 {
            file.write_all(WAL_MAGIC)?;
            file.write_all(&WAL_VERSION.to_le_bytes())?;
            file.flush()?;
        }
        Ok(Self {
            wal_path,
            writer: Some(BufWriter::new(file)),
            sync_mode,
            cumulative_written_bytes: if existing_len == 0 {
                WAL_HEADER_SIZE
            } else {
                0
            },
            cumulative_logical_bytes: 0,
            group_commit_active: false,
            group_commit_pending: false,
            cumulative_sync_count: 0,
        })
    }

    /// 动态修改同步模式
    pub fn set_sync_mode(&mut self, mode: SyncMode) {
        self.sync_mode = mode;
    }

    /// 追加一条操作日志
    ///
    /// 格式: [len: u32][bincode bytes][crc32: u32]
    /// 写入后立即 fsync，保证即使 OS 崩溃数据也不丢失
    pub fn append<T: serde::Serialize>(&mut self, entry: &WalEntry<T>) -> Result<()> {
        let result = self.append_inner(entry);
        if result.is_err() {
            // 写入结果不确定时隔离 writer，保留日志供重新打开恢复。
            self.release_writer();
        }
        result
    }

    fn append_inner<T: serde::Serialize>(&mut self, entry: &WalEntry<T>) -> Result<()> {
        let data = bincode::serialize(entry).map_err(TriviumError::Serialization)?;
        let checksum = crc32fast::hash(&data);
        let len = data.len() as u32;
        {
            let writer = self.writer.as_mut().ok_or(TriviumError::WalClosed)?;
            writer.write_all(&len.to_le_bytes())?;
            writer.write_all(&data)?;
            writer.write_all(&checksum.to_le_bytes())?;
        }
        self.cumulative_written_bytes = self
            .cumulative_written_bytes
            .saturating_add(8u64.saturating_add(data.len() as u64));
        self.cumulative_logical_bytes = self
            .cumulative_logical_bytes
            .saturating_add(data.len() as u64);
        if self.group_commit_active {
            self.group_commit_pending = true;
        } else {
            self.sync_writer()?;
        }
        Ok(())
    }

    /// 批量追加一个事务的所有日志（附带事务边界）
    ///
    /// 会自动打上 TxBegin 和 TxCommit 封条，并且仅在整个 Batch 写入完毕后才做一次 fsync。
    pub fn append_batch<T: serde::Serialize>(
        &mut self,
        tx_id: u64,
        entries: &[WalEntry<T>],
    ) -> Result<()> {
        let result = self.append_batch_inner(tx_id, entries);
        if result.is_err() {
            self.release_writer();
        }
        result
    }

    fn append_batch_inner<T: serde::Serialize>(
        &mut self,
        tx_id: u64,
        entries: &[WalEntry<T>],
    ) -> Result<()> {
        if self.writer.is_none() {
            return Err(TriviumError::WalClosed);
        }
        let batch_bytes = {
            let writer = self.writer.as_mut().ok_or(TriviumError::WalClosed)?;
            let mut write_single = |entry: &WalEntry<T>| -> Result<u64> {
                let data = bincode::serialize(entry).map_err(TriviumError::Serialization)?;
                let checksum = crc32fast::hash(&data);
                let len = data.len() as u32;
                writer.write_all(&len.to_le_bytes())?;
                writer.write_all(&data)?;
                writer.write_all(&checksum.to_le_bytes())?;
                Ok(8u64.saturating_add(data.len() as u64))
            };

            let mut batch_bytes = write_single(&WalEntry::TxBegin { tx_id })?;
            for entry in entries {
                batch_bytes = batch_bytes.saturating_add(write_single(entry)?);
            }
            batch_bytes.saturating_add(write_single(&WalEntry::TxCommit { tx_id })?)
        };

        // 字节已经写入 BufWriter；必须先更新指标，避免后续 flush/sync 失败时漏报。
        self.cumulative_written_bytes = self.cumulative_written_bytes.saturating_add(batch_bytes);
        self.cumulative_logical_bytes = self
            .cumulative_logical_bytes
            .saturating_add(batch_bytes.saturating_sub(8 * (entries.len() as u64 + 2)));

        // Group Commit 中只追加完整事务帧，组尾统一执行同步。
        if self.group_commit_active {
            self.group_commit_pending = true;
        } else {
            self.sync_writer()?;
        }
        Ok(())
    }

    /// 开始一个多事务 Group Commit。组内每个 `append_batch` 仍写入独立事务封条。
    pub fn begin_group_commit(&mut self) -> Result<()> {
        if self.writer.is_none() {
            return Err(TriviumError::WalClosed);
        }
        if self.group_commit_active {
            return Err(TriviumError::InvalidInput(
                "WAL Group Commit 不允许嵌套 (WAL group commit cannot be nested)".into(),
            ));
        }
        self.group_commit_active = true;
        self.group_commit_pending = false;
        Ok(())
    }

    /// 同步并结束当前 Group Commit；成功返回是否执行了物理同步调用。
    pub fn finish_group_commit(&mut self) -> Result<bool> {
        if !self.group_commit_active {
            return Err(TriviumError::InvalidInput(
                "WAL Group Commit 尚未开始 (WAL group commit is not active)".into(),
            ));
        }
        self.group_commit_active = false;
        let pending = std::mem::take(&mut self.group_commit_pending);
        if !pending {
            return Ok(false);
        }
        self.sync_writer()
    }

    pub fn stats(&self) -> WalStats {
        WalStats {
            written_bytes: self.cumulative_written_bytes,
            logical_bytes: self.cumulative_logical_bytes,
            sync_count: self.cumulative_sync_count,
        }
    }

    fn sync_writer(&mut self) -> Result<bool> {
        let result = self.sync_writer_inner();
        if result.is_err() {
            self.release_writer();
        }
        result
    }

    fn sync_writer_inner(&mut self) -> Result<bool> {
        let Some(writer) = self.writer.as_mut() else {
            return Err(TriviumError::WalClosed);
        };
        match self.sync_mode {
            SyncMode::Full => {
                #[cfg(feature = "test-hooks")]
                crate::test_hooks::io_result(crate::test_hooks::IoPoint::WalFlush)?;
                writer.flush()?;
                #[cfg(feature = "test-hooks")]
                crate::test_hooks::io_result(crate::test_hooks::IoPoint::WalSync)?;
                writer.get_ref().sync_data()?;
                self.cumulative_sync_count = self.cumulative_sync_count.saturating_add(1);
                Ok(true)
            }
            SyncMode::Normal => {
                #[cfg(feature = "test-hooks")]
                crate::test_hooks::io_result(crate::test_hooks::IoPoint::WalFlush)?;
                writer.flush()?;
                Ok(false)
            }
            SyncMode::Off => Ok(false),
        }
    }

    /// 读取 WAL 文件中的所有条目（用于崩溃恢复）
    ///
    /// 每条记录都会校验 CRC32：
    ///   - 校验通过 → 回放
    ///   - 完整帧校验失败 → 返回损坏错误，调用方保留原日志
    ///   - 尾帧物理截断 → 只恢复完整提交前缀
    pub fn read_entries<T: serde::de::DeserializeOwned>(
        db_path: &str,
    ) -> Result<(Vec<WalEntry<T>>, u64)> {
        let mut entries = Vec::new();
        let offset = Self::replay_entries(db_path, |entry| {
            entries.push(entry);
            Ok(())
        })?;
        Ok((entries, offset))
    }

    /// 逐条交付已提交操作；只暂存当前事务，回调失败时立即停止。
    pub(crate) fn replay_entries<T: serde::de::DeserializeOwned>(
        db_path: &str,
        visit: impl FnMut(WalEntry<T>) -> Result<()>,
    ) -> Result<u64> {
        Self::replay_entries_limited(db_path, 0, &std::cell::Cell::new(0), visit)
    }

    /// 旧 WAL 先在预算内转换并回放；持久化迁移由打开流程全部准入后执行。
    pub(crate) fn replay_for_open<T: serde::de::DeserializeOwned + serde::Serialize>(
        db_path: &str,
        limit: usize,
        current: &std::cell::Cell<usize>,
        visit: impl FnMut(WalEntry<T>) -> Result<()>,
    ) -> Result<u64> {
        let path = format!("{db_path}.wal");
        let mut file = File::open(&path)?;
        let mut header = [0u8; WAL_HEADER_SIZE as usize];
        if file.read_exact(&mut header).is_ok()
            && &header[..4] == WAL_MAGIC
            && u16::from_le_bytes([header[4], header[5]]) == LEGACY_WAL_VERSION_V2
        {
            Self::admit_legacy_migration(db_path, limit, current.get())?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            let entries = decode_legacy_v2_frames::<T>(&bytes)?;
            let mut converted = Vec::new();
            for entry in &entries {
                write_framed_entry(&mut converted, entry)?;
            }
            return Self::replay_entries_from_reader_limited(
                std::io::Cursor::new(converted),
                visit,
                limit,
                current,
            )
            .map(|offset| offset.saturating_add(WAL_HEADER_SIZE));
        }
        Self::replay_entries_limited(db_path, limit, current, visit)
    }

    pub(crate) fn admit_legacy_migration(
        db_path: &str,
        limit: usize,
        current: usize,
    ) -> Result<()> {
        let mut file = match File::open(format!("{db_path}.wal")) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let mut header = [0u8; WAL_HEADER_SIZE as usize];
        if file.read_exact(&mut header).is_ok()
            && &header[..4] == WAL_MAGIC
            && u16::from_le_bytes([header[4], header[5]]) == LEGACY_WAL_VERSION_V2
        {
            let extra = usize::try_from(file.metadata()?.len())
                .unwrap_or(usize::MAX)
                .saturating_mul(64);
            if limit > 0 && current.saturating_add(extra) > limit {
                return Err(TriviumError::CapacityReservationRejected {
                    requested_nodes: 0,
                    estimated_bytes: extra,
                    current_bytes: current,
                    memory_limit: limit,
                });
            }
        }
        Ok(())
    }

    pub(crate) fn replay_entries_limited<T: serde::de::DeserializeOwned>(
        db_path: &str,
        limit: usize,
        current: &std::cell::Cell<usize>,
        visit: impl FnMut(WalEntry<T>) -> Result<()>,
    ) -> Result<u64> {
        let wal_path = format!("{}.wal", db_path);
        let mut file = match File::open(&wal_path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error.into()),
        };
        let mut prefix = [0u8; WAL_HEADER_SIZE as usize];
        let mut read = 0;
        while read < prefix.len() {
            match file.read(&mut prefix[read..]) {
                Ok(0) => break,
                Ok(count) => read += count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
        if read >= 4 && &prefix[0..4] == WAL_MAGIC {
            if read < WAL_HEADER_SIZE as usize {
                return Err(TriviumError::CorruptedFile(
                    "WAL 版本头被截断 (WAL header truncated)".into(),
                ));
            }
            let version = u16::from_le_bytes([prefix[4], prefix[5]]);
            if version != WAL_VERSION {
                return Err(TriviumError::UnsupportedWalVersion {
                    found: version,
                    supported: WAL_VERSION,
                });
            }
            let offset = Self::replay_entries_from_reader_limited(
                BufReader::new(file),
                visit,
                limit,
                current,
            )?;
            return offset
                .checked_add(WAL_HEADER_SIZE)
                .ok_or_else(|| TriviumError::CorruptedFile("WAL 安全边界溢出".into()));
        }

        Err(TriviumError::UnsupportedWalVersion {
            found: 0,
            supported: WAL_VERSION,
        })
    }

    /// 从任意 `Read` 源解析 WAL 条目流
    ///
    /// 此方法是 `read_entries` 的核心实现，独立出来便于：
    /// - 模糊测试（fuzzing）：传入 `Cursor<&[u8]>` 测试畸形数据
    /// - 单元测试：无需真实文件即可验证解析逻辑
    pub fn read_entries_from_reader<T: serde::de::DeserializeOwned>(
        reader: impl Read,
    ) -> Result<(Vec<WalEntry<T>>, u64)> {
        let mut entries = Vec::new();
        let offset = Self::replay_entries_from_reader(reader, |entry| {
            entries.push(entry);
            Ok(())
        })?;
        Ok((entries, offset))
    }

    fn replay_entries_from_reader<T: serde::de::DeserializeOwned>(
        reader: impl Read,
        visit: impl FnMut(WalEntry<T>) -> Result<()>,
    ) -> Result<u64> {
        Self::replay_entries_from_reader_limited(reader, visit, 0, &std::cell::Cell::new(0))
    }

    fn replay_entries_from_reader_limited<T: serde::de::DeserializeOwned>(
        mut reader: impl Read,
        mut visit: impl FnMut(WalEntry<T>) -> Result<()>,
        limit: usize,
        current: &std::cell::Cell<usize>,
    ) -> Result<u64> {
        let admit = |extra: usize| -> Result<()> {
            if limit > 0 && current.get().saturating_add(extra) > limit {
                return Err(TriviumError::CapacityReservationRejected {
                    requested_nodes: 0,
                    estimated_bytes: extra,
                    current_bytes: current.get(),
                    memory_limit: limit,
                });
            }
            Ok(())
        };
        let mut pending_encoded_bytes = 0usize;
        let mut data = Vec::new();
        let mut physical_offset: u64 = 0;
        let mut record_count = 0usize;
        let mut pending_tx = Vec::new();
        let mut current_tx_id = None;
        let mut safe_commit_offset = 0;
        let mut chunk = [0u8; 8192];

        'frames: loop {
            // 读取 len
            let mut len_buf = [0u8; 4];
            match reader.read_exact(&mut len_buf) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(TriviumError::Io(e)),
            }
            let len = u32::from_le_bytes(len_buf) as usize;

            // 合理性检查：单条不超过 256MB
            if len > MAX_WAL_ENTRY_SIZE {
                return Err(TriviumError::CorruptedFile(format!(
                    "WAL 帧长度超过上限，偏移 {physical_offset} (WAL frame length exceeds limit): {len}"
                )));
            }

            // 同时计入帧、事务暂存、JSON 解码及应用副本，准入先于扩容和反序列化。
            admit(
                pending_encoded_bytes
                    .saturating_add(len)
                    .saturating_mul(64)
                    .saturating_add(data.capacity()),
            )?;
            // 复用帧缓冲，并按实际读入量扩容；截断的恶意长度不会触发整帧分配。
            data.clear();
            while data.len() < len {
                let wanted = (len - data.len()).min(chunk.len());
                match reader.read(&mut chunk[..wanted]) {
                    Ok(0) => break 'frames,
                    Ok(count) => {
                        data.try_reserve(count).map_err(|error| {
                            TriviumError::CapacityAllocationFailed {
                                reason: format!("WAL 帧缓冲预留失败：{error}"),
                            }
                        })?;
                        data.extend_from_slice(&chunk[..count]);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                        break 'frames;
                    }
                    Err(error) => return Err(error.into()),
                }
            }

            // 读取 CRC32
            let mut crc_buf = [0u8; 4];
            match reader.read_exact(&mut crc_buf) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(error) => return Err(error.into()),
            }
            let stored_crc = u32::from_le_bytes(crc_buf);
            let computed_crc = crc32fast::hash(&data);

            if stored_crc != computed_crc {
                return Err(TriviumError::CorruptedFile(format!(
                    "WAL 完整帧 CRC 不匹配，记录 {record_count}、偏移 {physical_offset} (Complete WAL frame CRC mismatch): stored={stored_crc:#010x}, computed={computed_crc:#010x}"
                )));
            }

            // 成功读取一条物理上合法的记录，推进物理游标
            physical_offset = physical_offset
                .checked_add(8 + len as u64)
                .ok_or_else(|| TriviumError::CorruptedFile("WAL 帧偏移溢出".into()))?;

            // 反序列化
            use bincode::Options;
            let entry = bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .with_limit(len as u64)
                .deserialize::<WalEntry<T>>(&data)
                .map_err(|error| TriviumError::CorruptedFile(format!(
                    "WAL 完整帧无法解码，记录 {record_count} (Cannot decode complete WAL frame): {error}"
                )))?;
            record_count = record_count.saturating_add(1);
            match entry {
                WalEntry::TxBegin { tx_id } => {
                    current_tx_id = Some(tx_id);
                    pending_tx.clear();
                    pending_encoded_bytes = 0;
                }
                WalEntry::TxCommit { tx_id } => {
                    if current_tx_id == Some(tx_id) {
                        for operation in pending_tx.drain(..) {
                            visit(operation)?;
                        }
                        current_tx_id = None;
                        pending_encoded_bytes = 0;
                        safe_commit_offset = physical_offset;
                    }
                }
                other => {
                    if current_tx_id.is_some() {
                        pending_tx.try_reserve(1).map_err(|error| {
                            TriviumError::CapacityAllocationFailed {
                                reason: format!("WAL 待提交事务预留失败：{error}"),
                            }
                        })?;
                        pending_tx.push(other);
                        pending_encoded_bytes = pending_encoded_bytes.saturating_add(len);
                    } else {
                        visit(other)?;
                        safe_commit_offset = physical_offset;
                    }
                }
            }
        }

        if current_tx_id.is_some() && !pending_tx.is_empty() {
            tracing::warn!(
                "丢弃了一个不完整事务 ({} 个操作)，缺少 TxCommit (Discarded partial transaction, {} ops, missing TxCommit)。WAL 截断至偏移 {}",
                pending_tx.len(),
                pending_tx.len(),
                safe_commit_offset
            );
        }

        Ok(safe_commit_offset)
    }

    pub fn cumulative_written_bytes(&self) -> u64 {
        self.cumulative_written_bytes
    }

    pub fn cumulative_logical_bytes(&self) -> u64 {
        self.cumulative_logical_bytes
    }

    /// flush 成功后清除 WAL 文件
    ///
    /// 使用 truncate（截断）语义而非 remove + create：
    ///   - 避免 Windows 杀毒软件对"新建文件"的扫描锁定
    ///   - 文件 inode/句柄不变，减少系统调用次数
    pub fn clear(&mut self) -> Result<()> {
        self.ensure_writable()?;
        // 关闭当前 writer
        self.writer.take();
        let mode = self.sync_mode;

        // 截断清空 WAL（而非删除重建）
        // 对于已存在的文件，truncate 不会触发杀软的"新文件扫描"
        {
            let mut file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&self.wal_path)?;
            file.write_all(WAL_MAGIC)?;
            file.write_all(&WAL_VERSION.to_le_bytes())?;
            file.sync_all()?;
        }

        // 重新以追加模式打开
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.wal_path)?;
        self.writer = Some(BufWriter::new(file));
        self.sync_mode = mode;
        Ok(())
    }

    /// 显式 flush BufWriter 中缓冲的数据到磁盘
    ///
    /// 在 Database 的 Drop 中调用，确保已写入的 WAL 条目不会因为
    /// BufWriter 的析构链（Arc<Mutex<Wal>>）而静默丢失。
    pub fn flush_writer(&mut self) {
        if let Some(ref mut writer) = self.writer {
            let _ = writer.flush();
            let _ = writer.get_ref().sync_all();
        }
    }

    #[cfg(test)]
    pub(crate) fn close_writer_for_test(&mut self) {
        self.writer = None;
    }

    /// WAL 文件是否存在且非空（用于判断是否需要恢复）
    pub fn needs_recovery(db_path: &str) -> bool {
        let wal_path = format!("{}.wal", db_path);
        let Ok(meta) = std::fs::metadata(&wal_path) else {
            return false;
        };
        if meta.len() == 0 {
            return false;
        }
        if meta.len() != WAL_HEADER_SIZE {
            return true;
        }
        let Ok(bytes) = std::fs::read(&wal_path) else {
            return true;
        };
        bytes.len() != WAL_HEADER_SIZE as usize
            || &bytes[0..4] != WAL_MAGIC
            || u16::from_le_bytes([bytes[4], bytes[5]]) != WAL_VERSION
    }
}

#[cfg(test)]
mod streaming_tests {
    use super::*;
    use std::cell::Cell;
    use std::io::Cursor;

    thread_local! {
        static LIVE: Cell<usize> = const { Cell::new(0) };
        static PEAK: Cell<usize> = const { Cell::new(0) };
    }

    struct Tracked;

    impl<'de> Deserialize<'de> for Tracked {
        fn deserialize<D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> std::result::Result<Self, D::Error> {
            let _ = f32::deserialize(deserializer)?;
            LIVE.with(|live| {
                live.set(live.get() + 1);
                PEAK.with(|peak| peak.set(peak.get().max(live.get())));
            });
            Ok(Self)
        }
    }

    impl Drop for Tracked {
        fn drop(&mut self) {
            LIVE.with(|live| live.set(live.get() - 1));
        }
    }

    fn frame(bytes: &mut Vec<u8>, entry: WalEntry<f32>) -> usize {
        let data = bincode::serialize(&entry).unwrap();
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        bytes.extend_from_slice(&crc32fast::hash(&data).to_le_bytes());
        bytes.len()
    }

    #[test]
    fn 独立写入逐条释放解码向量且事务只保留当前批次() {
        for transactional in [false, true] {
            let mut bytes = Vec::new();
            let batch = 7;
            let dim = 32;
            for group in 0..40 {
                if transactional {
                    frame(&mut bytes, WalEntry::TxBegin { tx_id: group });
                }
                for id in 0..batch {
                    frame(
                        &mut bytes,
                        WalEntry::Insert {
                            id: group * batch + id + 1,
                            vector: vec![1.0; dim],
                            payload: "{}".into(),
                        },
                    );
                }
                if transactional {
                    frame(&mut bytes, WalEntry::TxCommit { tx_id: group });
                }
            }
            LIVE.with(|live| assert_eq!(live.get(), 0));
            PEAK.with(|peak| peak.set(0));
            let mut delivered = 0;
            let offset = Wal::replay_entries_from_reader::<Tracked>(Cursor::new(&bytes), |entry| {
                assert!(matches!(entry, WalEntry::Insert { .. }));
                delivered += 1;
                Ok(())
            })
            .unwrap();
            assert_eq!(delivered, 40 * batch);
            assert_eq!(offset, bytes.len() as u64);
            LIVE.with(|live| assert_eq!(live.get(), 0));
            PEAK.with(|peak| {
                assert_eq!(
                    peak.get(),
                    dim * if transactional { batch as usize } else { 1 }
                )
            });
        }
    }

    #[test]
    fn 逐字节截断及坏帧遵循独立指定的事务提交边界() {
        let mut bytes = Vec::new();
        let first = frame(&mut bytes, WalEntry::Delete { id: 1 });
        frame(&mut bytes, WalEntry::TxBegin { tx_id: 7 });
        frame(&mut bytes, WalEntry::Delete { id: 2 });
        frame(&mut bytes, WalEntry::TxCommit { tx_id: 99 });
        frame(&mut bytes, WalEntry::TxBegin { tx_id: 8 });
        frame(&mut bytes, WalEntry::Delete { id: 3 });
        frame(&mut bytes, WalEntry::TxCommit { tx_id: 7 });
        frame(&mut bytes, WalEntry::Delete { id: 4 });
        let committed = frame(&mut bytes, WalEntry::TxCommit { tx_id: 8 });
        frame(&mut bytes, WalEntry::TxCommit { tx_id: 7 });
        let standalone = frame(&mut bytes, WalEntry::Delete { id: 5 });
        frame(&mut bytes, WalEntry::TxBegin { tx_id: 10 });
        let empty = frame(&mut bytes, WalEntry::TxCommit { tx_id: 10 });
        frame(&mut bytes, WalEntry::TxBegin { tx_id: 9 });
        frame(&mut bytes, WalEntry::Delete { id: 6 });
        let expected = |length| {
            let ids = if length >= standalone {
                vec![1, 3, 4, 5]
            } else if length >= committed {
                vec![1, 3, 4]
            } else if length >= first {
                vec![1]
            } else {
                vec![]
            };
            let offset = [first, committed, standalone, empty]
                .into_iter()
                .filter(|&offset| offset <= length)
                .max()
                .unwrap_or(0);
            (ids, offset as u64)
        };
        let parse = |data: &[u8]| {
            let mut ids = Vec::new();
            let offset = Wal::replay_entries_from_reader::<f32>(Cursor::new(data), |entry| {
                if let WalEntry::Delete { id } = entry {
                    ids.push(id);
                } else {
                    panic!("事务标记不应交付给回放回调");
                }
                Ok(())
            });
            offset.map(|offset| (ids, offset))
        };
        for cut in 0..=bytes.len() {
            assert_eq!(
                parse(&bytes[..cut]).unwrap(),
                expected(cut),
                "截断位置：{cut}"
            );
        }
        let mut start = 0;
        while start < bytes.len() {
            let len = u32::from_le_bytes(bytes[start..start + 4].try_into().unwrap()) as usize;
            let mut corrupted = bytes.clone();
            corrupted[start + 4 + len] ^= 1;
            assert!(
                matches!(parse(&corrupted), Err(TriviumError::CorruptedFile(_))),
                "坏帧位置：{start}"
            );
            // CRC 合法但完整枚举无法解码时，也必须拒绝整个恢复操作。
            corrupted[start + 4..start + 8].copy_from_slice(&u32::MAX.to_le_bytes());
            let crc = crc32fast::hash(&corrupted[start + 4..start + 4 + len]);
            corrupted[start + 4 + len..start + 8 + len].copy_from_slice(&crc.to_le_bytes());
            assert!(matches!(
                parse(&corrupted),
                Err(TriviumError::CorruptedFile(_))
            ));
            start += 8 + len;
        }
    }

    struct FailingReader<'a> {
        data: &'a [u8],
        remaining: usize,
    }

    #[test]
    fn 跨多个读取块的向量和中文正文允许短读及中断重试() {
        struct Fragmented {
            data: Cursor<Vec<u8>>,
            interrupted: bool,
        }
        impl Read for Fragmented {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                self.interrupted = !self.interrupted;
                if self.interrupted {
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                let count = buffer.len().min(37);
                self.data.read(&mut buffer[..count])
            }
        }
        let vector = (0..4097).map(|axis| axis as f32 / 17.).collect::<Vec<_>>();
        let payload = serde_json::json!({"正文":"分块恢复".repeat(4097)}).to_string();
        let mut bytes = Vec::new();
        frame(
            &mut bytes,
            WalEntry::Insert {
                id: 1,
                vector: vector.clone(),
                payload: payload.clone(),
            },
        );
        frame(&mut bytes, WalEntry::Delete { id: 2 });
        let length = bytes.len() as u64;
        let mut delivered = 0;
        let offset = Wal::replay_entries_from_reader::<f32>(
            Fragmented {
                data: Cursor::new(bytes),
                interrupted: false,
            },
            |entry| {
                match (delivered, entry) {
                    (
                        0,
                        WalEntry::Insert {
                            id: 1,
                            vector: actual,
                            payload: text,
                        },
                    ) => {
                        assert_eq!(actual, vector);
                        assert_eq!(text, payload);
                    }
                    (1, WalEntry::Delete { id: 2 }) => {}
                    _ => panic!("分块回放交付了错误的内容或顺序"),
                }
                delivered += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(delivered, 2);
        assert_eq!(offset, length);
    }

    #[test]
    fn 超大声明配短尾部只请求固定大小读取缓冲() {
        struct ShortFrame(Cursor<Vec<u8>>);
        impl Read for ShortFrame {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                assert!(buffer.len() <= 8192, "损坏帧不得按声明长度请求大缓冲");
                self.0.read(buffer)
            }
        }
        let mut bytes = (MAX_WAL_ENTRY_SIZE as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(&[1, 2, 3]);
        let offset = Wal::replay_entries_from_reader::<f32>(ShortFrame(Cursor::new(bytes)), |_| {
            panic!("截断帧不得交付回放");
        })
        .unwrap();
        assert_eq!(offset, 0);
    }

    impl Read for FailingReader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.remaining == 0 {
                return Err(std::io::Error::other("模拟设备读取失败"));
            }
            let count = self.data.len().min(buffer.len()).min(self.remaining);
            buffer[..count].copy_from_slice(&self.data[..count]);
            self.data = &self.data[count..];
            self.remaining -= count;
            Ok(count)
        }
    }

    #[test]
    fn 帧内真实_io错误向上传播且回调失败不读取后续帧() {
        let mut bytes = Vec::new();
        frame(&mut bytes, WalEntry::Delete { id: 1 });
        for cut in 0..bytes.len() {
            let mut called = false;
            let result = Wal::replay_entries_from_reader::<f32>(
                FailingReader {
                    data: &bytes,
                    remaining: cut,
                },
                |_| {
                    called = true;
                    Ok(())
                },
            );
            assert!(
                matches!(result, Err(TriviumError::Io(_))),
                "读取失败位置：{cut}"
            );
            assert!(!called);
        }
        let result = Wal::replay_entries_from_reader::<f32>(
            FailingReader {
                data: &bytes,
                remaining: bytes.len(),
            },
            |_| Err(TriviumError::InvalidInput("模拟回放失败".into())),
        );
        assert!(matches!(result, Err(TriviumError::InvalidInput(_))));
    }
}
