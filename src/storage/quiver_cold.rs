//! 单文件QuIVer SSD制品的流式发布、来源绑定与位置读取。
//!
//! 热区采用既有QuIVer快照，冷区按行独立校验。Reader持有文件句柄，发布采用同目录
//! 独占临时文件和原子替换，旧Reader继续访问原文件；打开过程只读取头和热区。

use crate::error::{Result, TriviumError, WriteErrorInfo, WriteOutcome, WritePhase};
use crate::index::quiver::cold::Int8Layout;
use crate::storage::fs::{robust_rename, sync_parent_directory};
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const HEADER_BYTES: usize = 160;
pub(crate) const RECORD_HEADER_BYTES: usize = 32;
const MAGIC: &[u8; 8] = b"QVSSD001";
const VERSION: u32 = 1;

/// 来源身份由宿主的不可变事实清单摘要提供，发布身份标识本次索引代际。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct SsdMetadata {
    pub source: [u8; 32],
    pub generation: [u8; 16],
    pub dimension: usize,
    pub rows: usize,
    pub hot_bytes: u64,
    pub cold_bytes: u64,
    pub record_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RowIdentity {
    pub node_id: u64,
    pub slot: u64,
    pub deleted: bool,
}

struct Header {
    metadata: SsdMetadata,
    layout: Int8Layout,
    hot_crc: u32,
}

impl Header {
    fn cold_offset(&self) -> Result<u64> {
        (HEADER_BYTES as u64)
            .checked_add(self.metadata.hot_bytes)
            .ok_or_else(|| corrupt("SSD热区长度溢出 (SSD hot section length overflow)"))
    }

    fn bytes(&self) -> [u8; HEADER_BYTES] {
        let mut bytes = [0u8; HEADER_BYTES];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes[16..48].copy_from_slice(&self.metadata.source);
        bytes[48..64].copy_from_slice(&self.metadata.generation);
        for (offset, value) in [
            (64, HEADER_BYTES as u64),
            (72, self.metadata.hot_bytes),
            (80, HEADER_BYTES as u64 + self.metadata.hot_bytes),
            (88, self.metadata.rows as u64),
            (96, self.metadata.record_bytes as u64),
        ] {
            bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        bytes[104..108].copy_from_slice(&self.hot_crc.to_le_bytes());
        bytes[112..152].copy_from_slice(&self.layout.to_header());
        let checksum = crc32fast::hash(&bytes[..156]);
        bytes[156..].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    fn parse(
        bytes: &[u8; HEADER_BYTES],
        file_bytes: u64,
        expected_source: [u8; 32],
    ) -> Result<Self> {
        if &bytes[..8] != MAGIC
            || word(bytes, 8) != VERSION
            || word(bytes, 12) != HEADER_BYTES as u32
            || word(bytes, 104 + 4) != 0
            || word(bytes, 152) != 0
            || word(bytes, 156) != crc32fast::hash(&bytes[..156])
        {
            return Err(corrupt(
                "SSD头部身份、版本或CRC无效 (Invalid SSD header identity, version, or CRC)",
            ));
        }
        let source: [u8; 32] = bytes[16..48]
            .try_into()
            .map_err(|_| corrupt("SSD来源长度无效 (Invalid SSD source length)"))?;
        if source != expected_source {
            return Err(TriviumError::ImmutableArtifactInvalid {
                reason: "SSD索引来源代际不匹配 (SSD index source generation mismatch)".into(),
            });
        }
        let layout = Int8Layout::from_header(&bytes[112..152])?;
        let rows = usize::try_from(number(bytes, 88))
            .map_err(|_| corrupt("SSD行数超出平台范围 (SSD row count exceeds platform range)"))?;
        let record_bytes = layout
            .row_stride_bytes()
            .checked_add(RECORD_HEADER_BYTES)
            .ok_or_else(|| corrupt("SSD记录步长溢出 (SSD record stride overflow)"))?;
        let generation: [u8; 16] = bytes[48..64]
            .try_into()
            .map_err(|_| corrupt("SSD代际长度无效 (Invalid SSD generation length)"))?;
        let cold_bytes = (rows as u64)
            .checked_mul(record_bytes as u64)
            .ok_or_else(|| corrupt("SSD冷区长度溢出 (SSD cold section length overflow)"))?;
        let header = Self {
            metadata: SsdMetadata {
                source,
                generation,
                dimension: layout.dimension(),
                rows,
                hot_bytes: number(bytes, 72),
                cold_bytes,
                record_bytes,
            },
            layout,
            hot_crc: word(bytes, 104),
        };
        let cold_offset = header.cold_offset()?;
        if rows == 0
            || rows > u32::MAX as usize
            || generation == [0; 16]
            || header.metadata.hot_bytes < 52
            || number(bytes, 64) != HEADER_BYTES as u64
            || number(bytes, 80) != cold_offset
            || number(bytes, 96) != record_bytes as u64
            || cold_offset.checked_add(cold_bytes) != Some(file_bytes)
        {
            return Err(corrupt("SSD区段几何无效 (Invalid SSD section geometry)"));
        }
        Ok(header)
    }
}

/// 只持有自己创建的临时路径，所有失败路径均在文件关闭后回收该路径。
struct Temporary {
    path: PathBuf,
    lease_path: PathBuf,
    lease: Option<File>,
    owns_path: bool,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let removed = if self.owns_path {
            match std::fs::remove_file(&self.path) {
                Ok(()) => true,
                Err(error) => error.kind() == io::ErrorKind::NotFound,
            }
        } else {
            true
        };
        self.lease.take();
        // 临时路径清理失败时保留已解锁租约，下次可写入口继续回收。
        if removed {
            let _ = std::fs::remove_file(&self.lease_path);
        }
    }
}

/// 同一目标的发布与私有暂存回收共享OS租约；进程退出由OS释放，零写Reader不进入此入口。
fn publication_lease(path: &Path) -> Result<File> {
    use fs2::FileExt;
    let mut name = path.as_os_str().to_os_string();
    name.push(".publish-lock");
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(PathBuf::from(name))?;
    lease.lock_exclusive()?;
    Ok(lease)
}

fn collect_locked(path: &Path) -> Result<usize> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().ok_or_else(|| {
        TriviumError::InvalidInput("SSD路径缺少文件名 (SSD path has no file name)".into())
    })?;
    let mut prefix = name.as_encoded_bytes().to_vec();
    prefix.extend_from_slice(b".qssd-");
    let mut count = 0;
    for entry in std::fs::read_dir(parent)? {
        crate::request::check_current()?;
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name
            .as_encoded_bytes()
            .strip_prefix(prefix.as_slice())
            .and_then(|n| n.strip_suffix(b".tmp.lease"))
        else {
            continue;
        };
        if std::str::from_utf8(id)
            .ok()
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
            .is_some()
            && entry.file_type()?.is_file()
        {
            use fs2::FileExt;
            let lease_path = entry.path();
            let lease = match OpenOptions::new().read(true).write(true).open(&lease_path) {
                Ok(lease) => lease,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            match lease.try_lock_exclusive() {
                Ok(()) => {}
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
                {
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
            let temporary = lease_path.with_extension("");
            match std::fs::symlink_metadata(&temporary) {
                Ok(metadata) if metadata.is_file() => match std::fs::remove_file(&temporary) {
                    Ok(()) => count += 1,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                },
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            drop(lease);
            match std::fs::remove_file(lease_path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(count)
}

pub(crate) fn collect(path: &Path) -> Result<usize> {
    let _lease = publication_lease(path)?;
    collect_locked(path)
}

struct HotWriter<'a> {
    writer: &'a mut BufWriter<File>,
    bytes: u64,
    checksum: crc32fast::Hasher,
    control_error: Option<TriviumError>,
}
impl Write for HotWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Err(error) = crate::request::check_current() {
            self.control_error = Some(error);
            return Err(io::Error::other(
                "SSD热区写入已取消 (SSD hot section write cancelled)",
            ));
        }
        let written = self.writer.write(bytes)?;
        self.bytes = self
            .bytes
            .checked_add(written as u64)
            .ok_or_else(|| io::Error::other("SSD热区长度溢出 (SSD hot section length overflow)"))?;
        self.checksum.update(&bytes[..written]);
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

/// 工作区已由调用方准入，逐行转换和写入，不物化全量浮点或冷编码数组。
pub(crate) fn publish(
    path: &Path,
    source: [u8; 32],
    layout: Int8Layout,
    rows: usize,
    write_hot: impl FnOnce(&mut dyn Write) -> Result<()>,
    mut fill: impl FnMut(usize, &mut [f32]) -> Result<RowIdentity>,
    write: &mut WriteErrorInfo,
) -> Result<SsdMetadata> {
    validate_target(path)?;
    let _lease = publication_lease(path)?;
    collect_locked(path)?;
    let record_bytes = layout
        .row_stride_bytes()
        .checked_add(RECORD_HEADER_BYTES)
        .ok_or_else(|| TriviumError::CapacityAllocationFailed {
            reason: "SSD记录容量溢出 (SSD record capacity overflow)".into(),
        })?;
    let mut record = zeroed(record_bytes)?;
    let mut vector = vec![f32::NAN; layout.dimension()];
    let generation = *uuid::Uuid::new_v4().as_bytes();
    let mut temporary_name = path.as_os_str().to_os_string();
    temporary_name.push(format!(".qssd-{}.tmp", uuid::Uuid::new_v4()));
    let temporary = PathBuf::from(temporary_name);
    let mut lease_name = temporary.as_os_str().to_os_string();
    lease_name.push(".lease");
    let lease_path = PathBuf::from(lease_name);
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&lease_path)?;
    fs2::FileExt::lock_exclusive(&lease)?;
    let mut temporary_guard = Temporary {
        path: temporary.clone(),
        lease_path,
        lease: Some(lease),
        owns_path: false,
    };
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    temporary_guard.owns_path = true;
    drop(_lease);
    let mut writer = BufWriter::with_capacity(64 * 1024, file);
    writer.write_all(&[0u8; HEADER_BYTES])?;
    let mut hot = HotWriter {
        writer: &mut writer,
        bytes: 0,
        checksum: crc32fast::Hasher::new(),
        control_error: None,
    };
    let result = write_hot(&mut hot);
    if let Some(error) = hot.control_error.take() {
        return Err(error);
    }
    result?;
    let hot_bytes = hot.bytes;
    let hot_crc = hot.checksum.finalize();
    for row in 0..rows {
        crate::request::check_current()?;
        vector.fill(f32::NAN);
        let identity = fill(row, &mut vector)?;
        if identity.deleted {
            vector.fill(0.);
        }
        record[..8].copy_from_slice(&identity.node_id.to_le_bytes());
        record[8..16].copy_from_slice(&identity.slot.to_le_bytes());
        record[16..24].copy_from_slice(&(row as u64).to_le_bytes());
        record[24..28].copy_from_slice(&u32::from(identity.deleted).to_le_bytes());
        layout.encode(&vector, &mut record[RECORD_HEADER_BYTES..])?;
        let checksum = record_crc(&record);
        record[28..32].copy_from_slice(&checksum.to_le_bytes());
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::io_result(crate::test_hooks::IoPoint::QuiverColdWrite)?;
        writer.write_all(&record)?;
    }
    let cold_bytes = (rows as u64)
        .checked_mul(record_bytes as u64)
        .ok_or_else(|| corrupt("SSD冷区长度溢出 (SSD cold section length overflow)"))?;
    let header = Header {
        metadata: SsdMetadata {
            source,
            generation,
            dimension: layout.dimension(),
            rows,
            hot_bytes,
            cold_bytes,
            record_bytes,
        },
        layout,
        hot_crc,
    };
    header
        .cold_offset()?
        .checked_add(cold_bytes)
        .ok_or_else(|| corrupt("SSD制品长度溢出 (SSD artifact length overflow)"))?;
    writer.seek(SeekFrom::Start(0))?;
    writer.write_all(&header.bytes())?;
    writer.flush()?;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::QuiverColdSync)?;
    writer.get_ref().sync_all()?;
    drop(writer);
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::hit_for_path(
        crate::test_hooks::ConcurrencyPoint::QuiverColdBeforePublish,
        &path.to_string_lossy(),
    );
    crate::request::check_current()?;
    validate_target(path)?;
    write.phase = WritePhase::Publish;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::QuiverColdPublish)?;
    robust_rename(&temporary, path)?;
    write.outcome = WriteOutcome::Unknown;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::QuiverColdDirectorySync)?;
    sync_parent_directory(path)?;
    write.outcome = WriteOutcome::Committed;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::hit_for_path(
        crate::test_hooks::ConcurrencyPoint::QuiverColdAfterPublish,
        &path.to_string_lossy(),
    );
    Ok(header.metadata)
}

/// 只替换本格式的普通制品文件，防止错误目标路径覆盖事实文件或其他宿主数据。
fn validate_target(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let invalid = || {
        TriviumError::InvalidInput(
            "SSD目标已经包含其他类型的文件 (SSD destination already contains another file type)"
                .into(),
        )
        .at_field("path")
    };
    if !metadata.is_file() {
        return Err(invalid());
    }
    let mut magic = [0u8; 8];
    if metadata.len() < magic.len() as u64 {
        return Err(invalid());
    }
    File::open(path)?.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(invalid());
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct ReadCounters {
    pub calls: AtomicU64,
    pub bytes: AtomicU64,
}

pub(crate) struct ColdFile {
    file: File,
    io: Option<super::quiver_io::AsyncReader>,
    header: Header,
    #[cfg(feature = "test-hooks")]
    path: PathBuf,
    pub counters: ReadCounters,
    pub physical: Arc<super::quiver_io::IoCounters>,
}

impl ColdFile {
    pub fn open(path: &Path, expected_source: [u8; 32]) -> Result<Self> {
        Self::open_bound(path, expected_source, None)
    }

    pub fn open_bound(
        path: &Path,
        expected_source: [u8; 32],
        binding: Option<(u64, u32)>,
    ) -> Result<Self> {
        if !std::fs::metadata(path)?.is_file() {
            return Err(TriviumError::InvalidInput(
                "SSD读取目标必须为普通文件 (SSD read target must be a regular file)".into(),
            )
            .at_field("path"));
        }
        let mut file = File::open(path)?;
        let length = file.metadata()?.len();
        if length < HEADER_BYTES as u64 {
            return Err(corrupt(
                "SSD制品头部被截断 (SSD artifact header is truncated)",
            ));
        }
        let mut bytes = [0u8; HEADER_BYTES];
        file.read_exact(&mut bytes)?;
        let header = Header::parse(&bytes, length, expected_source)?;
        if binding
            .is_some_and(|(bytes, crc)| bytes != header.metadata.hot_bytes || crc != header.hot_crc)
        {
            return Err(TriviumError::ImmutableArtifactInvalid {
                reason: "SSD热区与数据库索引绑定不匹配 (SSD hot section does not match database index binding)".into(),
            });
        }
        Ok(Self {
            file,
            io: None,
            header,
            #[cfg(feature = "test-hooks")]
            path: path.to_path_buf(),
            counters: ReadCounters::default(),
            physical: Arc::default(),
        })
    }

    pub fn metadata(&self) -> SsdMetadata {
        self.header.metadata
    }
    pub fn layout(&self) -> Int8Layout {
        self.header.layout
    }

    pub fn configure_io(
        &mut self,
        mode: super::quiver_io::IoMode,
        depth: usize,
        runtime: &crate::ResourceRuntime,
    ) -> Result<()> {
        let reader = super::quiver_io::AsyncReader::new(
            &self.file,
            mode,
            depth,
            runtime,
            self.physical.clone(),
        )?;
        crate::request::check_current()?;
        self.io = Some(reader);
        Ok(())
    }

    pub fn io_mode(&self) -> super::quiver_io::IoMode {
        self.io
            .as_ref()
            .map_or(super::quiver_io::IoMode::Buffered, |io| io.mode)
    }

    pub fn has_async_io(&self) -> bool {
        self.io.is_some()
    }

    pub fn io_alignment(&self) -> super::quiver_io::IoAlignment {
        self.io
            .as_ref()
            .map_or_else(Default::default, |io| io.alignment)
    }

    pub fn close_io(&mut self) {
        self.io.take();
    }

    pub fn io_workspace_bytes(&self, record_bytes: usize) -> usize {
        match &self.io {
            None => record_bytes,
            Some(io) if io.mode == super::quiver_io::IoMode::Direct => {
                record_bytes.saturating_mul(2).saturating_add(131072)
            }
            Some(_) => record_bytes,
        }
    }

    pub fn read_hot(&self) -> Result<Vec<u8>> {
        let bytes = usize::try_from(self.header.metadata.hot_bytes)
            .map_err(|_| corrupt("SSD热区超出平台范围 (SSD hot section exceeds platform range)"))?;
        let mut buffer = zeroed(bytes)?;
        read_exact_at(&self.file, &mut buffer, HEADER_BYTES as u64, None)?;
        crate::request::check_current()?;
        if crc32fast::hash(&buffer) != self.header.hot_crc {
            return Err(corrupt("SSD热区CRC无效 (Invalid SSD hot section CRC)"));
        }
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit_for_path(
            crate::test_hooks::ConcurrencyPoint::QuiverColdAfterHotRead,
            &self.path.to_string_lossy(),
        );
        crate::request::check_current()?;
        Ok(buffer)
    }

    /// 批内相邻记录及重叠Direct I/O窗口合并读取，全部完成后按原顺序校验每条身份。
    pub fn read_records(&self, requests: &[(usize, RowIdentity)]) -> Result<Vec<Vec<u8>>> {
        let metadata = &self.header.metadata;
        if requests.len() > 4 || requests.iter().any(|&(row, _)| row >= metadata.rows) {
            return Err(TriviumError::InvalidInput(
                "SSD读取批次或范围无效 (Invalid SSD read batch or range)".into(),
            ));
        }
        crate::request::check_current()?;
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit_for_path(
            crate::test_hooks::ConcurrencyPoint::QuiverColdBeforeRead,
            &self.path.to_string_lossy(),
        );
        crate::request::check_current()?;
        let cold_offset = self.header.cold_offset()?;
        let mut order = requests
            .iter()
            .enumerate()
            .map(|(position, &(row, _))| {
                (
                    position,
                    cold_offset + row as u64 * metadata.record_bytes as u64,
                )
            })
            .collect::<Vec<_>>();
        order.sort_by_key(|&(_, offset)| offset);
        let alignment = self.io_alignment().offset as u64;
        let mut ranges: Vec<(u64, u64)> = Vec::new();
        let mut assignments = vec![(0usize, 0usize); requests.len()];
        for (position, offset) in order {
            let end = offset + metadata.record_bytes as u64;
            let merge = ranges.last().is_some_and(|&(_, previous_end)| {
                offset <= previous_end
                    || (alignment > 0 && offset / alignment <= (previous_end - 1) / alignment)
            });
            if merge {
                let last = ranges.len() - 1;
                ranges[last].1 = ranges[last].1.max(end);
                assignments[position] = (last, (offset - ranges[last].0) as usize);
            } else {
                assignments[position] = (ranges.len(), 0);
                ranges.push((offset, end));
            }
        }
        let inputs = ranges
            .iter()
            .map(|&(start, end)| {
                let length = usize::try_from(end - start).map_err(|_| {
                    corrupt("SSD批次区段超出平台范围 (SSD batch section exceeds platform range)")
                })?;
                Ok((start, zeroed(length)?))
            })
            .collect::<Result<Vec<_>>>()?;
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::io_result(crate::test_hooks::IoPoint::QuiverColdRead)?;
        let output = if let Some(io) = &self.io {
            io.read_batch(inputs).map_err(|error| {
                if matches!(&error, TriviumError::Io(cause) if cause.kind() == io::ErrorKind::UnexpectedEof) {
                    corrupt("SSD区段读取不足 (Short SSD section read)")
                } else { error }
            })?
        } else {
            let mut output = Vec::with_capacity(inputs.len());
            for (offset, mut buffer) in inputs {
                let counted = ReadCounters::default();
                let result = read_exact_at(&self.file, &mut buffer, offset, Some(&counted));
                self.physical
                    .calls
                    .fetch_add(counted.calls.load(Ordering::Relaxed), Ordering::Relaxed);
                self.physical
                    .bytes
                    .fetch_add(counted.bytes.load(Ordering::Relaxed), Ordering::Relaxed);
                result?;
                output.push(buffer);
            }
            output
        };
        self.counters
            .calls
            .fetch_add(requests.len() as u64, Ordering::Relaxed);
        self.counters.bytes.fetch_add(
            (requests.len() * metadata.record_bytes) as u64,
            Ordering::Relaxed,
        );
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit_for_path(
            crate::test_hooks::ConcurrencyPoint::QuiverColdAfterRead,
            &self.path.to_string_lossy(),
        );
        crate::request::check_current()?;
        let mut records = Vec::with_capacity(requests.len());
        for (&(row, expected), &(group, start)) in requests.iter().zip(&assignments) {
            let buffer = &output[group][start..start + metadata.record_bytes];
            if number(buffer, 0) != expected.node_id
                || number(buffer, 8) != expected.slot
                || number(buffer, 16) != row as u64
                || word(buffer, 24) != u32::from(expected.deleted)
                || word(buffer, 28) != record_crc(buffer)
            {
                return Err(corrupt(
                    "SSD记录身份或CRC无效 (Invalid SSD record identity or CRC)",
                ));
            }
            self.header.layout.row(&buffer[RECORD_HEADER_BYTES..])?;
            let mut record = zeroed(metadata.record_bytes)?;
            record.copy_from_slice(buffer);
            records.push(record);
        }
        crate::request::check_current()?;
        Ok(records)
    }
}

fn read_exact_at(
    file: &File,
    mut output: &mut [u8],
    mut offset: u64,
    counters: Option<&ReadCounters>,
) -> Result<()> {
    while !output.is_empty() {
        crate::request::check_current()?;
        if let Some(counters) = counters {
            counters.calls.fetch_add(1, Ordering::Relaxed);
        }
        #[cfg(unix)]
        let read = {
            use std::os::unix::fs::FileExt;
            file.read_at(output, offset)
        };
        #[cfg(windows)]
        let read = {
            use std::os::windows::fs::FileExt;
            file.seek_read(output, offset)
        };
        #[cfg(not(any(unix, windows)))]
        let read: io::Result<usize> = Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "当前平台缺少位置读取后端 (Positional read backend is unavailable on this platform)",
        ));
        match read {
            Ok(0) => return Err(corrupt("SSD区段读取不足 (Short SSD section read)")),
            Ok(length) => {
                if let Some(counters) = counters {
                    counters.bytes.fetch_add(length as u64, Ordering::Relaxed);
                }
                offset += length as u64;
                output = &mut output[length..];
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(crate) fn zeroed(length: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| TriviumError::CapacityAllocationFailed {
            reason: "SSD缓冲分配失败 (SSD buffer allocation failed)".into(),
        })?;
    bytes.resize(length, 0);
    Ok(bytes)
}

fn record_crc(bytes: &[u8]) -> u32 {
    let mut checksum = crc32fast::Hasher::new();
    checksum.update(&bytes[..28]);
    checksum.update(&bytes[RECORD_HEADER_BYTES..]);
    checksum.finalize()
}
fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}
fn number(bytes: &[u8], offset: usize) -> u64 {
    u64::from(word(bytes, offset)) | (u64::from(word(bytes, offset + 4)) << 32)
}
fn corrupt(message: &str) -> TriviumError {
    TriviumError::CorruptedFile(message.into())
}
