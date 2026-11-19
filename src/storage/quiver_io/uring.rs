//! Linux固定文件和缓冲池直读。单线程提交一批请求并按完成事件交还各个范围。

use super::{
    IoAlignment, IoCounters, Job, WINDOW_LIMIT,
    fixed::{self, PoolLayout, Read, Span},
};
use io_uring::{IoUring, opcode, types};
use memmap2::MmapMut;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::sync::{atomic::Ordering::Relaxed, mpsc};

pub(super) struct Reader {
    // 先关闭ring再释放映射。已注册的匿名页由内核固定，关闭异常时也不会重用为其他分配。
    ring: IoUring,
    buffer: MmapMut,
    buffer_offset: usize,
    depth: usize,
    alignment: IoAlignment,
    layout: PoolLayout,
    pending: Vec<Option<Pending>>,
}

struct Pending {
    job_index: usize,
    buffer_offset: usize,
    span: Span,
}

impl Reader {
    pub(super) fn new(
        file: &File,
        depth: usize,
        alignment: IoAlignment,
        layout: PoolLayout,
    ) -> io::Result<Self> {
        let ring = IoUring::new(depth.next_power_of_two() as u32)?;
        ring.submitter().register_files(&[file.as_raw_fd()])?;
        let bytes = layout.bytes();
        let mut buffer = MmapMut::map_anon(bytes + alignment.memory)?;
        let buffer_offset = buffer.as_ptr().align_offset(alignment.memory);
        // SAFETY: 匿名映射覆盖对齐后的整个固定池，注册后地址不变。每个在途请求独占一个窗口。
        // ring及其固定文件先释放，之后才解除映射。已经固定的物理页持有内核引用直至请求排空。
        unsafe {
            ring.submitter().register_buffers(&[libc::iovec {
                iov_base: buffer.as_mut_ptr().add(buffer_offset).cast(),
                iov_len: bytes,
            }])?;
        }
        let mut reader = Self {
            ring,
            buffer,
            buffer_offset,
            depth,
            alignment,
            layout,
            pending: std::iter::repeat_with(|| None)
                .take(layout.slots())
                .collect(),
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

    fn wave(
        &mut self,
        batch: &mut [Option<Read>],
        counters: &IoCounters,
        notify: bool,
    ) -> io::Result<()> {
        let base = self.buffer.as_mut_ptr();
        loop {
            let mut in_flight = 0;
            let mut available = self.layout.batch();
            {
                let mut queue = self.ring.submission();
                for (job_index, item) in batch.iter_mut().enumerate() {
                    let Some(read) = item else { continue };
                    if read.done() {
                        fixed::finish(item, Ok(()), notify);
                        continue;
                    }
                    let span = match read.span(self.alignment, WINDOW_LIMIT) {
                        Ok(span) => span,
                        Err(error) => {
                            fixed::finish(item, Err(error), notify);
                            continue;
                        }
                    };
                    let Some(slot) = available.take(span.aligned_len) else {
                        continue;
                    };
                    let position = self.buffer_offset + slot.offset;
                    // SAFETY: 槽位独占且容量覆盖当前范围，地址满足文件对齐。在完成前不访问。
                    let entry = opcode::ReadFixed::new(
                        types::Fixed(0),
                        unsafe { base.add(position) },
                        span.aligned_len as u32,
                        0,
                    )
                    .offset(span.file_offset)
                    .build()
                    .user_data(slot.index as u64);
                    // SAFETY: 文件及固定内存在ring存活时保持注册，slot仅在对应CQE回收后才会复用。
                    unsafe { queue.push(&entry) }.map_err(|_| io::ErrorKind::WouldBlock)?;
                    if let Some(meter) = &read.job.meter {
                        meter.physical_read_calls.fetch_add(1, Relaxed);
                    }
                    self.pending[slot.index] = Some(Pending {
                        job_index,
                        buffer_offset: position,
                        span,
                    });
                    in_flight += 1;
                }
            }
            if in_flight == 0 {
                return Ok(());
            }
            counters.calls.fetch_add(in_flight as u64, Relaxed);
            while in_flight > 0 {
                match self.ring.submit_and_wait(1) {
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
                for event in self.ring.completion() {
                    let slot = event.user_data() as usize;
                    let invalid = || {
                        io::Error::other(
                            "io_uring完成项无对应读取 (io_uring completion has no matching read)",
                        )
                    };
                    let pending = self
                        .pending
                        .get_mut(slot)
                        .and_then(Option::take)
                        .ok_or_else(invalid)?;
                    let item = batch.get_mut(pending.job_index).ok_or_else(invalid)?;
                    let read = item.as_mut().ok_or_else(invalid)?;
                    in_flight -= 1;
                    if event.flags() != 0 {
                        return Err(io::Error::other(
                            "固定直读返回异常完成标志 (Fixed direct read returned unexpected completion flags)",
                        ));
                    }
                    let received = match event.result() {
                        value if value < 0 => {
                            fixed::finish(item, Err(io::Error::from_raw_os_error(-value)), notify);
                            continue;
                        }
                        bytes => bytes as usize,
                    };
                    counters.record_bytes(read.job.meter.as_deref(), received);
                    if received > pending.span.aligned_len {
                        fixed::finish(item, Err(io::ErrorKind::InvalidData.into()), notify);
                        continue;
                    }
                    // SAFETY: 已回收此CQE，读取限定于本槽位实际完成的范围。
                    let bytes = unsafe {
                        std::slice::from_raw_parts(base.add(pending.buffer_offset), received)
                    };
                    let result = read.copy_completed(pending.span, bytes);
                    if result.is_err() || read.done() {
                        fixed::finish(item, result, notify);
                    }
                }
            }
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // 先同步解除注册并等固定缓冲引用归还，再关闭ring和匿名映射。
        // 如果内核拒绝解除注册，内核仍持有匿名物理页引用直至ring关闭后完成排空。
        while self
            .ring
            .submitter()
            .unregister_buffers()
            .is_err_and(|error| error.kind() == io::ErrorKind::Interrupted)
        {}
    }
}
