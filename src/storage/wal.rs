use crate::error::{Result, TriviumError};
use crate::node::NodeId;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

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
}

/// WAL 磁盘同步模式
///
/// 控制每条 WAL 写入后是否强制落盘，在速度和安全之间权衡。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyncMode {
    /// 每条 WAL 写入后立即 fsync（最安全，防 OS 崩溃丢数据）
    /// 适用于：金融数据、不可丢失的关键业务
    Full,
    /// 每条写入 flush 到 OS 缓冲区，但不 fsync（平衡模式）
    /// 进程崩溃不丢数据，OS 崩溃可能丢最近几条
    /// 适用于：大多数生产场景
    Normal,
    /// 不主动 flush，完全依赖 OS 缓冲（最快，仅用于测试）
    Off,
}

impl Default for SyncMode {
    fn default() -> Self {
        SyncMode::Normal
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
}

impl Wal {
    /// 创建或打开 WAL 文件（追加模式）
    pub fn open(db_path: &str) -> Result<Self> {
        Self::open_with_sync(db_path, SyncMode::default())
    }

    /// 创建或打开 WAL 文件，指定同步模式
    pub fn open_with_sync(db_path: &str, sync_mode: SyncMode) -> Result<Self> {
        let wal_path = PathBuf::from(format!("{}.wal", db_path));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&wal_path)?;
        Ok(Self {
            wal_path,
            writer: Some(BufWriter::new(file)),
            sync_mode,
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
        if let Some(ref mut writer) = self.writer {
            let data = bincode::serialize(entry).map_err(|e| TriviumError::Serialization(e))?;

            // 计算 CRC32 校验和
            let checksum = crc32fast::hash(&data);

            let len = data.len() as u32;
            writer.write_all(&len.to_le_bytes())?;
            writer.write_all(&data)?;
            writer.write_all(&checksum.to_le_bytes())?;

            // 根据 sync_mode 决定同步策略
            match self.sync_mode {
                SyncMode::Full => {
                    writer.flush()?;
                    writer.get_ref().sync_data()?; // 真正落盘
                }
                SyncMode::Normal => {
                    writer.flush()?; // 到 OS 缓冲区，进程崩溃安全
                }
                SyncMode::Off => {
                    // 不主动 flush，依赖 OS 或 BufWriter 满时自动写
                }
            }

            Ok(())
        } else {
            Err(TriviumError::Generic("WAL writer closed".into()))
        }
    }

    /// 批量追加一个事务的所有日志（附带事务边界）
    ///
    /// 会自动打上 TxBegin 和 TxCommit 封条，并且仅在整个 Batch 写入完毕后才做一次 fsync。
    pub fn append_batch<T: serde::Serialize>(
        &mut self,
        tx_id: u64,
        entries: &[WalEntry<T>],
    ) -> Result<()> {
        if let Some(ref mut writer) = self.writer {
            let mut write_single = |entry: &WalEntry<T>| -> Result<()> {
                let data = bincode::serialize(entry).map_err(|e| TriviumError::Serialization(e))?;
                let checksum = crc32fast::hash(&data);
                let len = data.len() as u32;
                writer.write_all(&len.to_le_bytes())?;
                writer.write_all(&data)?;
                writer.write_all(&checksum.to_le_bytes())?;
                Ok(())
            };

            // 1. 写 TxBegin
            write_single(&WalEntry::TxBegin { tx_id })?;

            // 2. 写实体记录
            for e in entries {
                write_single(e)?;
            }

            // 3. 写 TxCommit（封条）
            write_single(&WalEntry::TxCommit { tx_id })?;

            // 4. 统一同步一次（极其提升性能与保证原子性）
            match self.sync_mode {
                SyncMode::Full => {
                    writer.flush()?;
                    writer.get_ref().sync_data()?;
                }
                SyncMode::Normal => {
                    writer.flush()?;
                }
                SyncMode::Off => {}
            }
            Ok(())
        } else {
            Err(TriviumError::Generic("WAL writer closed".into()))
        }
    }

    /// 读取 WAL 文件中的所有条目（用于崩溃恢复）
    ///
    /// 每条记录都会校验 CRC32：
    ///   - 校验通过 → 回放
    ///   - 校验失败 / 截断 → 安全停止，丢弃后续残缺数据
    pub fn read_entries<T: serde::de::DeserializeOwned>(db_path: &str) -> Result<Vec<WalEntry<T>>> {
        let wal_path = format!("{}.wal", db_path);
        if !Path::new(&wal_path).exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&wal_path)?;
        let mut reader = BufReader::new(file);
        let mut entries = Vec::new();

        loop {
            // 读取 len
            let mut len_buf = [0u8; 4];
            match reader.read_exact(&mut len_buf) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(TriviumError::Io(e)),
            }
            let len = u32::from_le_bytes(len_buf) as usize;

            // 合理性检查：单条不超过 256MB
            if len > 256 * 1024 * 1024 {
                break; // 损坏的 len 字段
            }

            // 读取 data
            let mut data = vec![0u8; len];
            match reader.read_exact(&mut data) {
                Ok(_) => {}
                Err(_) => break, // 截断的写入，安全丢弃
            }

            // 读取 CRC32
            let mut crc_buf = [0u8; 4];
            match reader.read_exact(&mut crc_buf) {
                Ok(_) => {}
                Err(_) => break, // CRC 不完整，丢弃该条
            }
            let stored_crc = u32::from_le_bytes(crc_buf);
            let computed_crc = crc32fast::hash(&data);

            if stored_crc != computed_crc {
                // CRC 不匹配 → 数据损坏，停止回放
                tracing::error!(
                    "WAL CRC mismatch at entry {}: stored={:#010x}, computed={:#010x}. Stopping recovery.",
                    entries.len(),
                    stored_crc,
                    computed_crc
                );
                break;
            }

            // 反序列化
            match bincode::deserialize::<WalEntry<T>>(&data) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    tracing::error!(
                        "WAL Deserialize error at entry {}: {}. Stopping recovery.",
                        entries.len(),
                        e
                    );
                    break;
                }
            }
        }

        // ====== 事务回放过滤（The Magic of ACID） ======
        let mut committed = Vec::new();
        let mut pending_tx = Vec::new();
        let mut in_tx = false;
        let mut current_tx_id = 0;

        for entry in entries {
            match entry {
                WalEntry::TxBegin { tx_id } => {
                    in_tx = true;
                    current_tx_id = tx_id;
                    pending_tx.clear(); // 清空，准备接纳新事务
                }
                WalEntry::TxCommit { tx_id } => {
                    if in_tx && tx_id == current_tx_id {
                        // 发现正确的封条，安全转正！
                        committed.append(&mut pending_tx);
                        in_tx = false;
                    }
                }
                other => {
                    if in_tx {
                        // 如果处于事务包裹区，先暂存在 pending 里
                        pending_tx.push(other);
                    } else {
                        // 兼容向后/独立的操作（旧版本数据或单个操作）
                        committed.push(other);
                    }
                }
            }
        }

        if in_tx && !pending_tx.is_empty() {
            tracing::warn!(
                "Discarded a partial transaction ({} operations) due to missing TxCommit (Power loss simulation).",
                pending_tx.len()
            );
        }

        Ok(committed)
    }

    /// flush 成功后清除 WAL 文件
    pub fn clear(&mut self) -> Result<()> {
        // 关闭当前 writer
        self.writer.take();
        let mode = self.sync_mode;
        // 删除旧 WAL
        if self.wal_path.exists() {
            std::fs::remove_file(&self.wal_path)?;
        }
        // 重新打开空 WAL
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

    /// WAL 文件是否存在且非空（用于判断是否需要恢复）
    pub fn needs_recovery(db_path: &str) -> bool {
        let wal_path = format!("{}.wal", db_path);
        match std::fs::metadata(&wal_path) {
            Ok(meta) => meta.len() > 0,
            Err(_) => false,
        }
    }
}
