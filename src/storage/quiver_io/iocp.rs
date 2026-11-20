//! Windows直读调度器。用完成端口回收批次，每个固定槽位独占缓冲和OVERLAPPED。

use super::{
    Aligned, IoAlignment, IoCounters, IoMeter, Job, WINDOW_LIMIT,
    fixed::{self, BufferSlot, PoolLayout, Read, Span},
};
use std::{
    cell::UnsafeCell,
    fs::File,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
    sync::mpsc,
};
use windows_sys::Win32::{
    Foundation::{ERROR_HANDLE_EOF, ERROR_IO_PENDING},
    Storage::FileSystem::ReadFile,
    System::{
        IO::{
            CancelIoEx, CreateIoCompletionPort, GetOverlappedResult, GetQueuedCompletionStatus,
            OVERLAPPED,
        },
        Threading::{CreateEventW, INFINITE},
    },
};

struct Pending {
    job_index: usize,
    buffer_offset: usize,
    span: Span,
    overflow: Option<Aligned>,
}

struct Slot {
    overlapped: Box<UnsafeCell<OVERLAPPED>>,
    event: OwnedHandle,
    pending: Option<Pending>,
}

// SAFETY: ReadFile与完成回收由同一调度线程独占。移交线程前，启动探测已排空全部请求。
// OS可写成员位于UnsafeCell和固定地址的堆中，槽位在请求完成前保持独占。
unsafe impl Send for Slot {}

impl Slot {
    fn new() -> io::Result<Self> {
        // SAFETY: 没有外部安全属性和名称，成功返回的事件句柄唯一转移给OwnedHandle。
        let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            // SAFETY: OVERLAPPED为允许全零初始化的Win32 POD。
            overlapped: Box::new(UnsafeCell::new(unsafe { std::mem::zeroed() })),
            // SAFETY: 事件创建成功且未被移交给其他所有者。
            event: unsafe { OwnedHandle::from_raw_handle(event) },
            pending: None,
        })
    }
}

pub(super) struct Reader {
    port: OwnedHandle,
    file: File,
    buffers: Aligned,
    slots: Box<[Slot]>,
    layout: PoolLayout,
    depth: usize,
    alignment: IoAlignment,
}

impl Reader {
    pub(super) fn new(
        file: File,
        depth: usize,
        alignment: IoAlignment,
        layout: PoolLayout,
    ) -> io::Result<Self> {
        // SAFETY: file是存活的OVERLAPPED句柄，端口由本Reader独占。
        let port = unsafe { CreateIoCompletionPort(file.as_raw_handle(), ptr::null_mut(), 0, 1) };
        if port.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateIoCompletionPort创建成功，端口所有权唯一移交。
        let port = unsafe { OwnedHandle::from_raw_handle(port) };
        let buffers = Aligned::new(layout.bytes(), alignment.memory)?;
        let slots = (0..layout.slots())
            .map(|_| Slot::new())
            .collect::<io::Result<Box<[_]>>>()?;
        let mut reader = Self {
            port,
            file,
            buffers,
            slots,
            layout,
            depth,
            alignment,
        };
        let (job, probe) = Read::probe();
        reader.wave(&mut [job], &IoCounters::default(), false)?;
        probe.recv().map_err(|_| io::ErrorKind::BrokenPipe)??;
        Ok(reader)
    }

    pub(super) fn run(mut self, receiver: mpsc::Receiver<Job>, counters: &IoCounters) {
        let mut batch = Vec::with_capacity(self.depth);
        while let Ok(first) = receiver.recv() {
            batch.push(Some(Read::new(first)));
            batch.extend(
                receiver
                    .try_iter()
                    .take(self.depth - 1)
                    .map(|job| Some(Read::new(job))),
            );
            if let Err(error) = self.wave(&mut batch, counters, true) {
                let depth = self.depth;
                drop(self);
                fixed::fail_batch(batch, receiver, depth, error);
                return;
            }
            batch.clear();
        }
    }

    fn submit(
        &mut self,
        slot: BufferSlot,
        job_index: usize,
        span: Span,
        counters: &IoCounters,
        meter: Option<&IoMeter>,
    ) -> io::Result<()> {
        let overflow = (span.aligned_len > WINDOW_LIMIT)
            .then(|| Aligned::new(span.aligned_len, self.alignment.memory))
            .transpose()?;
        let entry = &mut self.slots[slot.index];
        let overlapped = entry.overlapped.get();
        // SAFETY: 当前槽位空闲且堆地址固定。直到对应完成包回收，文件、事件、缓冲和结构体均存活。
        let started = unsafe {
            let mut state: OVERLAPPED = std::mem::zeroed();
            state.hEvent = entry.event.as_raw_handle();
            state.Anonymous.Anonymous.Offset = span.file_offset as u32;
            state.Anonymous.Anonymous.OffsetHigh = (span.file_offset >> 32) as u32;
            overlapped.write(state);
            counters.record_call(meter);
            ReadFile(
                self.file.as_raw_handle(),
                overflow
                    .as_ref()
                    .map_or_else(|| self.buffers.ptr.add(slot.offset), |buffer| buffer.ptr),
                span.aligned_len as u32,
                ptr::null_mut(),
                overlapped,
            )
        };
        if started == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(eof(error));
            }
        }
        // 同步成功也会投递完成包，槽位在收到完成包之前保持占用。
        entry.pending = Some(Pending {
            job_index,
            buffer_offset: slot.offset,
            span,
            overflow,
        });
        Ok(())
    }

    pub(super) fn wave(
        &mut self,
        batch: &mut [Option<Read>],
        counters: &IoCounters,
        notify: bool,
    ) -> io::Result<()> {
        let limit = (u32::MAX as usize / self.alignment.offset) * self.alignment.offset;
        loop {
            let mut in_flight = 0;
            let mut available = self.layout.batch();
            for (index, item) in batch.iter_mut().enumerate() {
                let Some(read) = item else { continue };
                if read.done() {
                    fixed::finish(item, Ok(()), notify);
                    continue;
                }
                let span = match read.span(self.alignment, limit) {
                    Ok(span) => span,
                    Err(error) => {
                        fixed::finish(item, Err(error), notify);
                        continue;
                    }
                };
                let Some(slot) = available.take(span.aligned_len.min(WINDOW_LIMIT)) else {
                    continue;
                };
                match self.submit(slot, index, span, counters, read.job.meter.as_deref()) {
                    Ok(()) => in_flight += 1,
                    Err(error) => fixed::finish(item, Err(error), notify),
                }
            }
            if in_flight == 0 {
                if batch.iter().any(Option::is_some) {
                    // 本波的槽位可能全被同步失败占用，剩余工作由下一波重新准入。
                    continue;
                }
                return Ok(());
            }
            for _ in 0..in_flight {
                self.complete(batch, counters, notify)?;
            }
        }
    }

    fn complete(
        &mut self,
        batch: &mut [Option<Read>],
        counters: &IoCounters,
        notify: bool,
    ) -> io::Result<()> {
        let mut bytes = 0;
        let mut key = 0;
        let mut overlapped = ptr::null_mut();
        // SAFETY: 端口由当前线程持有，输出参数可写；完成包中的指针只用于匹配已提交槽位。
        let status = unsafe {
            GetQueuedCompletionStatus(
                self.port.as_raw_handle(),
                &mut bytes,
                &mut key,
                &mut overlapped,
                INFINITE,
            )
        };
        let error = (status == 0).then(io::Error::last_os_error);
        let invalid_completion = || {
            io::Error::other("IOCP完成项没有对应的在途请求 (IOCP completion has no matching read)")
        };
        if overlapped.is_null() {
            return Err(error.unwrap_or_else(invalid_completion));
        }
        let entry = self
            .slots
            .iter_mut()
            .find(|slot| slot.overlapped.get() == overlapped)
            .ok_or_else(invalid_completion)?;
        let pending = entry.pending.take().ok_or_else(invalid_completion)?;
        let item = batch
            .get_mut(pending.job_index)
            .ok_or_else(invalid_completion)?;
        let read = item.as_mut().ok_or_else(invalid_completion)?;
        let bytes = bytes as usize;
        if bytes > pending.span.aligned_len {
            fixed::finish(item, Err(io::ErrorKind::InvalidData.into()), notify);
            return Ok(());
        }
        // 已匹配的失败完成包也可携带实际传输字节。
        counters.record_bytes(read.job.meter.as_deref(), bytes);
        if let Some(error) = error {
            fixed::finish(item, Err(eof(error)), notify);
            return Ok(());
        }
        // SAFETY: 已回收此槽位完成包，读取范围由系统返回长度限定，其他请求只写独立槽位。
        let buffer = unsafe {
            let pointer = pending.overflow.as_ref().map_or_else(
                || self.buffers.ptr.add(pending.buffer_offset),
                |buffer| buffer.ptr,
            );
            std::slice::from_raw_parts(pointer, bytes)
        };
        let result = read.copy_completed(pending.span, buffer);
        if result.is_err() || read.done() {
            fixed::finish(item, result, notify);
        }
        Ok(())
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // 完成端口异常或线程展开时，先请求取消，再按各槽位等待实际完成。
        for slot in self.slots.iter().filter(|slot| slot.pending.is_some()) {
            // SAFETY: 文件、缓冲、OVERLAPPED及各自的事件仍存活，按这个槽位的地址取消。
            unsafe {
                CancelIoEx(self.file.as_raw_handle(), slot.overlapped.get());
            }
        }
        for slot in self.slots.iter().filter(|slot| slot.pending.is_some()) {
            let mut bytes = 0;
            // SAFETY: 每个槽位拥有独立事件，内核完成前不释放任何资源。失败状态表示请求已结束。
            unsafe {
                GetOverlappedResult(
                    self.file.as_raw_handle(),
                    slot.overlapped.get(),
                    &mut bytes,
                    1,
                );
            }
        }
    }
}

fn eof(error: io::Error) -> io::Error {
    if error.raw_os_error() == Some(ERROR_HANDLE_EOF as i32) {
        return io::ErrorKind::UnexpectedEof.into();
    }
    error
}
