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
            let data = bincode::serialize(entry).map_err(TriviumError::Serialization)?;

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
            Err(TriviumError::WalClosed)
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
                let data = bincode::serialize(entry).map_err(TriviumError::Serialization)?;
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
            Err(TriviumError::WalClosed)
        }
    }

    /// 读取 WAL 文件中的所有条目（用于崩溃恢复）
    ///
    /// 每条记录都会校验 CRC32：
    ///   - 校验通过 → 回放
    ///   - 校验失败 / 截断 → 安全停止，丢弃后续残缺数据
    pub fn read_entries<T: serde::de::DeserializeOwned>(
        db_path: &str,
    ) -> Result<(Vec<WalEntry<T>>, u64)> {
        let wal_path = format!("{}.wal", db_path);
        if !Path::new(&wal_path).exists() {
            return Ok((Vec::new(), 0));
        }

        let file = File::open(&wal_path)?;
        let reader = BufReader::new(file);
        Self::read_entries_from_reader(reader)
    }

    /// 从任意 `Read` 源解析 WAL 条目流
    ///
    /// 此方法是 `read_entries` 的核心实现，独立出来便于：
    /// - 模糊测试（fuzzing）：传入 `Cursor<&[u8]>` 测试畸形数据
    /// - 单元测试：无需真实文件即可验证解析逻辑
    pub fn read_entries_from_reader<T: serde::de::DeserializeOwned>(
        mut reader: impl Read,
    ) -> Result<(Vec<WalEntry<T>>, u64)> {
        let mut entries_with_offset = Vec::new();
        let mut physical_offset: u64 = 0;

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
                    "WAL CRC 校验不匹配 (CRC mismatch)，第 {} 条记录: 存储={:#010x}, 计算={:#010x}。停止恢复 (Stopping recovery)",
                    entries_with_offset.len(),
                    stored_crc,
                    computed_crc
                );
                break;
            }

            // 成功读取一条物理上合法的记录，推进物理游标
            physical_offset += 4 + (len as u64) + 4;

            // 反序列化
            match bincode::deserialize::<WalEntry<T>>(&data) {
                Ok(entry) => entries_with_offset.push((entry, physical_offset)),
                Err(e) => {
                    tracing::error!(
                        "WAL 反序列化错误 (Deserialize error)，第 {} 条记录: {}。停止恢复 (Stopping recovery)",
                        entries_with_offset.len(),
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
        let mut safe_commit_offset = 0;

        for (entry, offset) in entries_with_offset {
            match entry {
                WalEntry::TxBegin { tx_id } => {
                    in_tx = true;
                    current_tx_id = tx_id;
                    pending_tx.clear(); // 清空，准备接纳新事务
                    // 游标不推进：未提交事务不能视为安全边界
                }
                WalEntry::TxCommit { tx_id } => {
                    if in_tx && tx_id == current_tx_id {
                        // 发现正确的封条，安全转正！
                        committed.append(&mut pending_tx);
                        in_tx = false;
                        safe_commit_offset = offset; // 整个事务完美闭环，安全推进物理游标
                    }
                }
                other => {
                    if in_tx {
                        // 如果处于事务包裹区，先暂存在 pending 里，游标暂时不动
                        pending_tx.push(other);
                    } else {
                        // 兼容向后/独立的操作（旧版本数据或单个操作），立刻安全推进！
                        committed.push(other);
                        safe_commit_offset = offset;
                    }
                }
            }
        }

        if in_tx && !pending_tx.is_empty() {
            tracing::warn!(
                "丢弃了一个不完整事务 ({} 个操作)，缺少 TxCommit (Discarded partial transaction, {} ops, missing TxCommit)。WAL 截断至偏移 {}",
                pending_tx.len(),
                pending_tx.len(),
                safe_commit_offset
            );
        }

        Ok((committed, safe_commit_offset))
    }

    /// flush 成功后清除 WAL 文件
    ///
    /// 使用 truncate（截断）语义而非 remove + create：
    ///   - 避免 Windows 杀毒软件对"新建文件"的扫描锁定
    ///   - 文件 inode/句柄不变，减少系统调用次数
    pub fn clear(&mut self) -> Result<()> {
        // 关闭当前 writer
        self.writer.take();
        let mode = self.sync_mode;

        // 截断清空 WAL（而非删除重建）
        // 对于已存在的文件，truncate 不会触发杀软的"新文件扫描"
        {
            let file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&self.wal_path)?;
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
        match std::fs::metadata(&wal_path) {
            Ok(meta) => meta.len() > 0,
            Err(_) => false,
        }
    }
}
