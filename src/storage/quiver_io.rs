//! 有界异步冷区读取。任务拥有缓冲直到内核完成，关闭排空队列并回收工作线程。

use crate::error::{Result, TriviumError};
use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;

#[cfg(any(windows, target_os = "linux"))]
mod alignment;
#[cfg(any(windows, target_os = "linux"))]
mod fixed;
#[cfg(windows)]
mod iocp;
#[cfg(target_os = "linux")]
mod linux_aio;
mod meter;
mod pages;
pub(crate) use meter::IoMeter;
pub use meter::QuiverIoStats;
#[cfg(target_os = "linux")]
mod uring;

/// 单个固定槽位或复用窗口的容量上限。
const WINDOW_LIMIT: usize = 64 * 1024;
pub(crate) const STARTUP_BYTES: usize = 2 * WINDOW_LIMIT;

fn shared_window_budget(lanes: usize) -> usize {
    if lanes > 1 {
        lanes.min(32) * (WINDOW_LIMIT + 256)
    } else {
        0
    }
}

#[cfg(any(windows, target_os = "linux"))]
fn fixed_workspace_bytes(layout: fixed::PoolLayout, lanes: usize) -> usize {
    // 额度覆盖对齐余量、线程栈和队列；Linux另计匿名映射及ring映射的页对齐余量。
    let overhead = if cfg!(target_os = "linux") { 8 } else { 4 } * WINDOW_LIMIT;
    layout.bytes() + overhead + shared_window_budget(lanes)
}

#[derive(Clone, Copy, Default)]
pub(crate) struct IoAlignment {
    pub offset: usize,
    pub memory: usize,
    pub detected: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IoMode {
    #[default]
    Buffered,
    Auto,
    Direct,
    /// 显式启用Linux io_uring，设备和内核须支持固定缓冲直读。
    IoUring,
}

impl IoMode {
    pub(crate) fn is_direct(self) -> bool {
        matches!(self, Self::Direct | Self::IoUring)
    }

    pub(crate) fn validate_depth(self, depth: usize) -> Result<()> {
        let maximum = if self == Self::IoUring { 256 } else { 32 };
        if (1..=maximum).contains(&depth) {
            return Ok(());
        }
        Err(TriviumError::InvalidInput(format!(
            "I/O深度须在1至{maximum}之间 (I/O depth must be between 1 and {maximum})"
        ))
        .at_field("io_depth"))
    }
}

#[derive(Default)]
pub(crate) struct IoCounters {
    pub calls: AtomicU64,
    pub bytes: AtomicU64,
    pub window_bytes: AtomicU64,
    pub window_hits: AtomicU64,
    pub window_hit_bytes: AtomicU64,
}

impl IoCounters {
    pub(super) fn record_call(&self, meter: Option<&IoMeter>) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if let Some(meter) = meter {
            meter.physical_read_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn record_bytes(&self, meter: Option<&IoMeter>, bytes: usize) {
        let bytes = bytes as u64;
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        if let Some(meter) = meter {
            meter
                .physical_read_bytes
                .fetch_add(bytes, Ordering::Relaxed);
        }
    }

    fn record_window(&self, meter: Option<&IoMeter>, bytes: usize, coalesced: bool) {
        let bytes = bytes as u64;
        self.window_hits.fetch_add(1, Ordering::Relaxed);
        self.window_hit_bytes.fetch_add(bytes, Ordering::Relaxed);
        if let Some(meter) = meter {
            meter.window_reuse_hits.fetch_add(1, Ordering::Relaxed);
            meter.window_reuse_bytes.fetch_add(bytes, Ordering::Relaxed);
            if coalesced {
                meter.coalesced_page_reads.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

struct Job {
    offset: u64,
    bytes: Vec<u8>,
    complete: mpsc::Sender<io::Result<Vec<u8>>>,
    meter: Option<Arc<IoMeter>>,
}

impl Job {
    fn finish(self, result: io::Result<()>, _notify: bool) {
        #[cfg(feature = "test-hooks")]
        if _notify {
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::QuiverColdIoCompleted);
        }
        let _ = self.complete.send(result.map(|()| self.bytes));
    }
}

pub(crate) struct AsyncReader {
    sender: Option<mpsc::SyncSender<Job>>,
    workers: Vec<JoinHandle<()>>,
    pub mode: IoMode,
    pub alignment: IoAlignment,
    pub fixed_buffer_bytes: usize,
    local: Option<Mutex<Backend>>,
    counters: Arc<IoCounters>,
    _threads: Option<crate::resources::IoThreadReservation>,
    pages: Option<pages::SharedPages>,
}

impl AsyncReader {
    pub fn new(
        file: &File,
        mode: IoMode,
        depth: usize,
        runtime: &crate::ResourceRuntime,
        counters: Arc<IoCounters>,
        mut admit: impl FnMut(usize) -> Result<()>,
    ) -> Result<Self> {
        mode.validate_depth(depth)?;
        if mode == IoMode::IoUring {
            #[cfg(target_os = "linux")]
            return Self::new_uring(file, depth, runtime, counters, admit);
            #[cfg(not(target_os = "linux"))]
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "io_uring需要Linux内核 (io_uring requires Linux)",
            )
            .into());
        }
        #[cfg(windows)]
        if depth > 1
            && matches!(mode, IoMode::Direct | IoMode::Auto)
            && let Some(reader) = Self::try_iocp(file, depth, runtime, &counters, &mut admit)?
        {
            return Ok(reader);
        }
        // 为CPU维护保留一个线程额度；单线程Runtime由请求线程提交并等待原生I/O。
        let depth = depth.min(runtime.max_threads().saturating_sub(1));
        let threads = if depth > 0 {
            Some(runtime.reserve_io_threads(depth)?)
        } else {
            None
        };
        let depth = threads.as_ref().map_or(0, |threads| threads.count());
        let mut backends = Vec::new();
        let actual = if mode == IoMode::Buffered {
            IoMode::Buffered
        } else {
            match Backend::direct(file) {
                Ok(mut backend) => {
                    // 实际探测文件系统和内核能力，严格模式保留原始系统错误。
                    let mut probe = [0u8; 8];
                    match backend.read(0, &mut probe, &Arc::default(), None) {
                        Ok(()) => {
                            backend.clear_window();
                            backends.push(backend);
                            IoMode::Direct
                        }
                        Err(error) if mode == IoMode::Auto && unsupported(&error) => {
                            IoMode::Buffered
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                Err(error) if mode == IoMode::Auto && unsupported(&error) => IoMode::Buffered,
                Err(error) => return Err(error.into()),
            }
        };
        let share_pages = actual == IoMode::Direct && depth > 1;
        admit(
            depth.max(1) * (256 * 1024)
                + if share_pages {
                    shared_window_budget(depth)
                } else {
                    0
                },
        )?;
        for _ in backends.len()..depth.max(1) {
            backends.push(if actual == IoMode::Direct {
                Backend::direct(file)?
            } else {
                Backend::Buffered(file.try_clone()?)
            });
        }
        let alignment = backends
            .first()
            .map_or_else(Default::default, Backend::alignment);
        if depth == 0 {
            return Ok(Self {
                sender: None,
                workers: Vec::new(),
                mode: actual,
                alignment,
                fixed_buffer_bytes: 0,
                local: backends.pop().map(Mutex::new),
                counters,
                _threads: threads,
                pages: None,
            });
        }
        let (sender, receiver) = mpsc::sync_channel::<Job>(depth);
        let receiver = Arc::new(Mutex::new(receiver));
        let mut reader = Self {
            sender: Some(sender),
            workers: Vec::new(),
            mode: actual,
            alignment,
            fixed_buffer_bytes: 0,
            local: None,
            counters: counters.clone(),
            _threads: threads,
            pages: share_pages
                .then(|| {
                    Ok::<_, io::Error>(pages::SharedPages::new(
                        alignment.offset,
                        file.metadata()?.len(),
                        depth,
                        counters.clone(),
                    ))
                })
                .transpose()?,
        };
        for mut backend in backends {
            let receiver = receiver.clone();
            let runtime = runtime.clone();
            let counters = reader.counters.clone();
            let worker = std::thread::Builder::new()
                .name("tdb-quiver-io".into())
                .stack_size(128 * 1024)
                .spawn(move || {
                    let _worker = runtime.enter_io_worker();
                    loop {
                        let job = crate::database::lock_or_recover(&receiver).recv();
                        let Ok(mut job) = job else { break };
                        if share_pages {
                            backend.invalidate_window();
                        }
                        let result = backend.read(
                            job.offset,
                            &mut job.bytes,
                            &counters,
                            job.meter.as_deref(),
                        );
                        job.finish(result, true);
                    }
                })?;
            reader.workers.push(worker);
        }
        Ok(reader)
    }

    #[cfg(windows)]
    fn try_iocp(
        file: &File,
        depth: usize,
        runtime: &crate::ResourceRuntime,
        counters: &Arc<IoCounters>,
        admit: &mut impl FnMut(usize) -> Result<()>,
    ) -> Result<Option<Self>> {
        let threads = runtime.reserve_io_threads(1)?;
        if threads.count() == 0 {
            return Ok(None);
        }
        let file = match open_windows_direct(file) {
            Ok(file) => file,
            Err(error) if unsupported(&error) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let alignment = match alignment::query(&file) {
            Ok(value) => value,
            Err(error) if unsupported(&error) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let layout = fixed::PoolLayout::new(depth, alignment);
        admit(fixed_workspace_bytes(layout, depth))?;
        let file_len = file.metadata()?.len();
        let port = match iocp::Reader::new(file, depth, alignment, layout) {
            Ok(port) => port,
            Err(error) if unsupported(&error) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let (sender, receiver) = mpsc::sync_channel(depth);
        let runtime = runtime.clone();
        let worker_counters = counters.clone();
        let worker = std::thread::Builder::new()
            .name("tdb-quiver-iocp".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                let _worker = runtime.enter_io_worker();
                port.run(receiver, &worker_counters);
            })?;
        Ok(Some(Self {
            sender: Some(sender),
            workers: vec![worker],
            mode: IoMode::Direct,
            alignment,
            fixed_buffer_bytes: layout.bytes(),
            local: None,
            counters: counters.clone(),
            _threads: Some(threads),
            pages: Some(pages::SharedPages::new(
                alignment.offset,
                file_len,
                depth,
                counters.clone(),
            )),
        }))
    }

    #[cfg(target_os = "linux")]
    fn new_uring(
        file: &File,
        depth: usize,
        runtime: &crate::ResourceRuntime,
        counters: Arc<IoCounters>,
        mut admit: impl FnMut(usize) -> Result<()>,
    ) -> Result<Self> {
        let threads = runtime.reserve_io_threads(1)?;
        if threads.count() == 0 {
            return Err(TriviumError::ResourceBudgetExceeded {
                resource: "worker_threads",
                requested: 1,
                used: runtime.stats().allocated_worker_threads,
                limit: runtime.max_threads(),
            }
            .at_field("io_mode"));
        }
        let file = open_linux_direct(file)?;
        let alignment = alignment::query(&file)?;
        let layout = fixed::PoolLayout::new(depth, alignment);
        admit(fixed_workspace_bytes(layout, depth.max(2)))?;
        let pages = pages::SharedPages::new(
            alignment.offset,
            file.metadata()?.len(),
            depth.max(2),
            counters.clone(),
        );
        let ring = uring::Reader::new(&file, depth, alignment, layout)?;
        let (sender, receiver) = mpsc::sync_channel(depth);
        let runtime = runtime.clone();
        let worker_counters = counters.clone();
        let worker = std::thread::Builder::new()
            .name("tdb-quiver-uring".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                let _worker = runtime.enter_io_worker();
                ring.run(receiver, &worker_counters);
            })?;
        Ok(Self {
            sender: Some(sender),
            workers: vec![worker],
            mode: IoMode::IoUring,
            alignment,
            fixed_buffer_bytes: layout.bytes(),
            local: None,
            counters,
            _threads: Some(threads),
            pages: Some(pages),
        })
    }

    pub fn shared_window_budget(&self) -> usize {
        self.pages.as_ref().map_or(0, pages::SharedPages::budget)
    }

    pub fn discard_shared_pages(&self) {
        if let Some(pages) = &self.pages {
            pages.clear_ready();
        }
    }

    /// 共用同代际的对齐窗口，并在提交后处理已经就绪的行。
    pub fn read_batch_during(
        &self,
        requests: Vec<(u64, Vec<u8>)>,
        meter: Option<&Arc<IoMeter>>,
        ready: impl FnOnce() -> Result<()>,
    ) -> Result<Vec<Vec<u8>>> {
        if let Some(pages) = &self.pages {
            return pages.read_during(self, requests, meter, ready);
        }
        self.read_ranges_during(requests, meter, ready)
    }

    /// 失败和取消先排空已提交的原生请求，再归还本批缓冲。
    fn read_ranges_during(
        &self,
        requests: Vec<(u64, Vec<u8>)>,
        meter: Option<&Arc<IoMeter>>,
        ready: impl FnOnce() -> Result<()>,
    ) -> Result<Vec<Vec<u8>>> {
        crate::request::check_current()?;
        if let Some(backend) = &self.local {
            ready()?;
            let mut backend = crate::database::lock_or_recover(backend);
            let mut output = Vec::with_capacity(requests.len());
            for (offset, mut bytes) in requests {
                crate::request::check_current()?;
                backend.read(offset, &mut bytes, &self.counters, meter.map(Arc::as_ref))?;
                #[cfg(feature = "test-hooks")]
                crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::QuiverColdIoCompleted);
                crate::request::check_current()?;
                output.push(bytes);
            }
            return Ok(output);
        }
        let sender = self.sender.as_ref().ok_or(TriviumError::DatabaseClosed)?;
        let mut completions = Vec::with_capacity(requests.len());
        let mut failure = None;
        let mut ready = Some(ready);
        for (offset, bytes) in requests {
            if let Err(error) = crate::request::check_current() {
                failure = Some(error);
                break;
            }
            let (complete, receiver) = mpsc::channel();
            let job = Job {
                offset,
                bytes,
                complete,
                meter: meter.cloned(),
            };
            let sent = match sender.try_send(job) {
                Ok(()) => true,
                Err(mpsc::TrySendError::Full(job)) => {
                    if let Some(ready) = ready.take()
                        && let Err(error) = ready()
                    {
                        failure = Some(error);
                        break;
                    }
                    sender.send(job).is_ok()
                }
                Err(mpsc::TrySendError::Disconnected(_)) => false,
            };
            if !sent {
                failure = Some(TriviumError::DatabaseClosed);
                break;
            }
            completions.push(receiver);
        }
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::QuiverColdBatchSubmitted);
        if failure.is_none()
            && let Some(ready) = ready
        {
            failure = ready().err();
        }
        let mut output = Vec::with_capacity(completions.len());
        for receiver in completions {
            match receiver.recv() {
                Ok(Ok(bytes)) => output.push(bytes),
                Ok(Err(error)) => {
                    failure.get_or_insert(TriviumError::Io(error));
                }
                Err(_) => {
                    failure.get_or_insert(TriviumError::DatabaseClosed);
                }
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        crate::request::check_current()?;
        Ok(output)
    }
}

impl Drop for AsyncReader {
    fn drop(&mut self) {
        self.sender.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn unsupported(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::Unsupported {
        return true;
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(1 | 50 | 87))
    }
    #[cfg(target_os = "linux")]
    {
        // 原文件句柄仍有效；ENOENT在此表示/proc/self/fd能力不可用。
        matches!(error.raw_os_error(), Some(1 | 2 | 22 | 38 | 95))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

enum Backend {
    Buffered(File),
    #[cfg(any(windows, target_os = "linux"))]
    Direct(DirectReader),
}

impl Backend {
    #[cfg(windows)]
    fn direct(file: &File) -> io::Result<Self> {
        let file = open_windows_direct(file)?;
        Ok(Self::Direct(DirectReader {
            alignment: alignment::query(&file)?,
            file,
            window: None,
        }))
    }

    #[cfg(target_os = "linux")]
    fn direct(file: &File) -> io::Result<Self> {
        let direct = open_linux_direct(file)?;
        Ok(Self::Direct(DirectReader {
            alignment: alignment::query(&direct)?,
            file: direct,
            context: linux_aio::NativeAio::new()?,
            window: None,
        }))
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    fn direct(_file: &File) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "当前平台缺少异步Direct I/O后端 (Async Direct I/O backend unavailable on this platform)",
        ))
    }

    fn alignment(&self) -> IoAlignment {
        match self {
            #[cfg(any(windows, target_os = "linux"))]
            Self::Direct(reader) => reader.alignment,
            _ => IoAlignment::default(),
        }
    }

    fn clear_window(&mut self) {
        #[cfg(any(windows, target_os = "linux"))]
        if let Self::Direct(reader) = self {
            reader.window = None;
        }
    }

    fn invalidate_window(&mut self) {
        #[cfg(any(windows, target_os = "linux"))]
        if let Self::Direct(reader) = self
            && let Some(window) = &mut reader.window
        {
            window.valid_bytes = 0;
        }
    }

    fn read(
        &mut self,
        offset: u64,
        output: &mut [u8],
        counters: &Arc<IoCounters>,
        meter: Option<&IoMeter>,
    ) -> io::Result<()> {
        match self {
            Self::Buffered(file) => {
                let mut done = 0;
                while done < output.len() {
                    counters.record_call(meter);
                    #[cfg(unix)]
                    let result = {
                        use std::os::unix::fs::FileExt;
                        file.read_at(&mut output[done..], offset + done as u64)
                    };
                    #[cfg(windows)]
                    let result = {
                        use std::os::windows::fs::FileExt;
                        file.seek_read(&mut output[done..], offset + done as u64)
                    };
                    #[cfg(not(any(unix, windows)))]
                    let result: io::Result<usize> = Err(io::ErrorKind::Unsupported.into());
                    match result {
                        Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                        Ok(bytes) => {
                            counters.record_bytes(meter, bytes);
                            done += bytes;
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) => return Err(error),
                    }
                }
                Ok(())
            }
            #[cfg(any(windows, target_os = "linux"))]
            Self::Direct(reader) => reader.read(offset, output, counters, meter),
        }
    }
}

#[cfg(windows)]
fn open_windows_direct(file: &File) -> io::Result<File> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, Storage::FileSystem::*};
    // SAFETY: 从仍存活的普通文件重新打开同一文件对象，返回句柄由File独占并最终关闭。
    let handle = unsafe {
        ReOpenFile(
            file.as_raw_handle(),
            0x80000000,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: 成功返回的新句柄所有权在此唯一转移给File。
    Ok(unsafe { File::from_raw_handle(handle) })
}

#[cfg(target_os = "linux")]
fn open_linux_direct(file: &File) -> io::Result<File> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECT)
        .open(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

#[cfg(any(windows, target_os = "linux"))]
struct DirectReader {
    file: File,
    alignment: IoAlignment,
    window: Option<ReadWindow>,
    #[cfg(target_os = "linux")]
    context: linux_aio::NativeAio,
}

#[cfg(any(windows, target_os = "linux"))]
impl DirectReader {
    fn read(
        &mut self,
        offset: u64,
        output: &mut [u8],
        counters: &Arc<IoCounters>,
        meter: Option<&IoMeter>,
    ) -> io::Result<()> {
        if output.is_empty() {
            return Ok(());
        }
        offset
            .checked_add(output.len() as u64)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        if let Some(window) = &self.window
            && let Some(skip) = window.contains(offset, output.len())
        {
            window.copy(skip, output);
            counters.record_window(meter, output.len(), false);
            return Ok(());
        }
        // 新读取前失效旧内容，失败或短读不能暴露被部分覆盖的窗口。
        if let Some(window) = &mut self.window {
            window.valid_bytes = 0;
        }
        let alignment = self.alignment.offset;
        let start = offset / alignment as u64 * alignment as u64;
        let skip = (offset - start) as usize;
        let length = skip
            .checked_add(output.len())
            .and_then(|n| n.checked_add(alignment - 1))
            .map(|n| n / alignment * alignment)
            .filter(|&n| n <= u32::MAX as usize)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        start
            .checked_add(length as u64)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let retain = length <= WINDOW_LIMIT;
        let mut window = if retain {
            let previous = self
                .window
                .take()
                .filter(|window| window.buffer.layout.size() >= length);
            match previous {
                Some(window) => window,
                None => ReadWindow::new(length, self.alignment.memory, counters.clone())?,
            }
        } else {
            ReadWindow::temporary(length, self.alignment.memory)?
        };
        let result = self
            .submit(start, &mut window.buffer, length, counters, meter)
            .and_then(|bytes| {
                counters.record_bytes(meter, bytes);
                if bytes < skip + output.len() {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                window.offset = start;
                window.valid_bytes = bytes;
                window.copy(skip, output);
                Ok(())
            });
        if retain {
            self.window = Some(window);
        }
        result
    }

    #[cfg(windows)]
    fn submit(
        &mut self,
        offset: u64,
        buffer: &mut Aligned,
        length: usize,
        counters: &IoCounters,
        meter: Option<&IoMeter>,
    ) -> io::Result<usize> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::*,
            Storage::FileSystem::ReadFile,
            System::{
                IO::{GetOverlappedResult, OVERLAPPED},
                Threading::CreateEventW,
            },
        };
        let file = &self.file;
        // SAFETY: 初始化每次请求独有的OVERLAPPED和事件，事件在完成之后关闭。
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        overlapped.hEvent = event;
        overlapped.Anonymous.Anonymous.Offset = offset as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
        counters.record_call(meter);
        // SAFETY: 句柄存活，缓冲满足文件内存对齐，length为扇区倍数且不超过分配，保留至完成。
        let started = unsafe {
            ReadFile(
                file.as_raw_handle(),
                buffer.ptr,
                length as u32,
                std::ptr::null_mut(),
                &mut overlapped,
            )
        };
        let result = if started == 0 && unsafe { GetLastError() } != ERROR_IO_PENDING {
            Err(io::Error::last_os_error())
        } else {
            let mut bytes = 0;
            // SAFETY: 本线程等待此请求的实际完成，然后才读取或释放缓冲。
            if unsafe { GetOverlappedResult(file.as_raw_handle(), &overlapped, &mut bytes, 1) } == 0
            {
                Err(io::Error::last_os_error())
            } else {
                Ok(bytes as usize)
            }
        };
        // SAFETY: 原生读取已经完成或在提交前失败，本次请求独有的事件句柄由此唯一关闭。
        unsafe {
            CloseHandle(event);
        }
        result
    }

    #[cfg(target_os = "linux")]
    fn submit(
        &mut self,
        offset: u64,
        buffer: &mut Aligned,
        length: usize,
        counters: &IoCounters,
        meter: Option<&IoMeter>,
    ) -> io::Result<usize> {
        use std::os::fd::AsRawFd;
        // SAFETY: 对齐分配在此调用返回前保持独占，File句柄仍存活；NativeAio等待完成或同步销毁上下文。
        unsafe {
            self.context.read(
                self.file.as_raw_fd(),
                offset,
                buffer.ptr,
                length,
                counters,
                meter,
            )
        }
    }
}

#[cfg(any(windows, target_os = "linux"))]
struct ReadWindow {
    buffer: Aligned,
    offset: u64,
    valid_bytes: usize,
    counters: Option<Arc<IoCounters>>,
}

#[cfg(any(windows, target_os = "linux"))]
impl ReadWindow {
    fn new(bytes: usize, alignment: usize, counters: Arc<IoCounters>) -> io::Result<Self> {
        let mut window = Self::temporary(bytes, alignment)?;
        counters
            .window_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
        window.counters = Some(counters);
        Ok(window)
    }

    fn temporary(bytes: usize, alignment: usize) -> io::Result<Self> {
        Ok(Self {
            buffer: Aligned::new(bytes, alignment)?,
            offset: 0,
            valid_bytes: 0,
            counters: None,
        })
    }

    fn contains(&self, offset: u64, bytes: usize) -> Option<usize> {
        let skip = usize::try_from(offset.checked_sub(self.offset)?).ok()?;
        (skip.checked_add(bytes)? <= self.valid_bytes).then_some(skip)
    }

    fn copy(&self, skip: usize, output: &mut [u8]) {
        // SAFETY: 仅在完成读取且有效内容覆盖请求后复制，缓冲由当前后端独占，输出无重叠。
        output.copy_from_slice(unsafe {
            std::slice::from_raw_parts(self.buffer.ptr.add(skip), output.len())
        });
    }
}

#[cfg(any(windows, target_os = "linux"))]
impl Drop for ReadWindow {
    fn drop(&mut self) {
        if let Some(counters) = &self.counters {
            counters
                .window_bytes
                .fetch_sub(self.buffer.layout.size() as u64, Ordering::Relaxed);
        }
    }
}

#[cfg(any(windows, target_os = "linux"))]
struct Aligned {
    ptr: *mut u8,
    layout: std::alloc::Layout,
}
// SAFETY: 分配唯一归属后端，迁移线程发生于请求提交之前；每次原生I/O均等待完成再访问或释放。
#[cfg(any(windows, target_os = "linux"))]
unsafe impl Send for Aligned {}
#[cfg(any(windows, target_os = "linux"))]
impl Aligned {
    fn new(bytes: usize, alignment: usize) -> io::Result<Self> {
        let layout = std::alloc::Layout::from_size_align(bytes, alignment)
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: 有效的非零大小与对齐，分配失败以错误返回，Drop使用同一Layout释放。
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err(io::ErrorKind::OutOfMemory.into());
        }
        Ok(Self { ptr, layout })
    }
}
#[cfg(any(windows, target_os = "linux"))]
impl Drop for Aligned {
    fn drop(&mut self) {
        // SAFETY: 唯一分配所有权，所有OS请求均已完成，Layout与分配时一致。
        unsafe {
            std::alloc::dealloc(self.ptr, self.layout);
        }
    }
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests;
