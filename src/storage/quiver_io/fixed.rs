//! 固定缓冲的分档与读请求进度，供两个平台的完成队列复用。

use super::{IoAlignment, Job, WINDOW_LIMIT};
use std::io;
use std::sync::mpsc;

#[derive(Clone, Copy)]
pub(super) struct PoolLayout {
    small_stride: usize,
    small_slots: usize,
    large_slots: usize,
}

impl PoolLayout {
    pub(super) fn new(depth: usize, alignment: IoAlignment) -> Self {
        let small_stride = alignment.offset.max(alignment.memory);
        let large_slots = if small_stride == WINDOW_LIMIT {
            0
        } else {
            depth.div_ceil(8)
        };
        Self {
            small_stride,
            small_slots: depth,
            large_slots,
        }
    }

    pub(super) fn bytes(self) -> usize {
        self.small_bytes() + self.large_slots * WINDOW_LIMIT
    }

    pub(super) fn slots(self) -> usize {
        self.small_slots + self.large_slots
    }

    fn small_bytes(self) -> usize {
        self.small_slots * self.small_stride
    }

    pub(super) fn batch(self) -> BatchSlots {
        BatchSlots {
            layout: self,
            small: 0,
            large: 0,
        }
    }
}

pub(super) struct BatchSlots {
    layout: PoolLayout,
    small: usize,
    large: usize,
}

pub(super) struct BufferSlot {
    pub index: usize,
    pub offset: usize,
}

impl BatchSlots {
    pub(super) fn take(&mut self, bytes: usize) -> Option<BufferSlot> {
        if bytes <= self.layout.small_stride && self.small < self.layout.small_slots {
            let index = self.small;
            self.small += 1;
            return Some(BufferSlot {
                index,
                offset: index * self.layout.small_stride,
            });
        }
        if self.large == self.layout.large_slots || bytes > WINDOW_LIMIT {
            return None;
        }
        let index = self.large;
        self.large += 1;
        Some(BufferSlot {
            index: self.layout.small_slots + index,
            offset: self.layout.small_bytes() + index * WINDOW_LIMIT,
        })
    }
}

pub(super) struct Read {
    pub job: Job,
    copied: usize,
}

pub(super) struct Span {
    pub file_offset: u64,
    pub skip: usize,
    pub payload: usize,
    pub aligned_len: usize,
}

impl Read {
    pub(super) fn new(job: Job) -> Self {
        Self { job, copied: 0 }
    }

    pub(super) fn probe() -> (Option<Self>, mpsc::Receiver<io::Result<Vec<u8>>>) {
        let (complete, receiver) = mpsc::channel();
        (
            Some(Self::new(Job {
                offset: 0,
                bytes: vec![0; 8],
                complete,
                meter: None,
            })),
            receiver,
        )
    }

    pub(super) fn done(&self) -> bool {
        self.copied == self.job.bytes.len()
    }

    pub(super) fn span(&self, alignment: IoAlignment, limit: usize) -> io::Result<Span> {
        let grain = alignment.offset;
        self.job
            .offset
            .checked_add(self.job.bytes.len() as u64)
            .ok_or(io::ErrorKind::InvalidInput)?;
        let start = self.job.offset + self.copied as u64;
        let file_offset = start / grain as u64 * grain as u64;
        let skip = (start - file_offset) as usize;
        let payload = (self.job.bytes.len() - self.copied).min(limit - skip);
        let aligned_len = (skip + payload).div_ceil(grain) * grain;
        file_offset
            .checked_add(aligned_len as u64)
            .ok_or(io::ErrorKind::InvalidInput)?;
        Ok(Span {
            file_offset,
            skip,
            payload,
            aligned_len,
        })
    }

    pub(super) fn copy_completed(&mut self, span: Span, buffer: &[u8]) -> io::Result<()> {
        let end = span.skip + span.payload;
        let source = buffer
            .get(span.skip..end)
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        let copied = self.copied + source.len();
        self.job.bytes[self.copied..copied].copy_from_slice(source);
        self.copied = copied;
        Ok(())
    }
}

pub(super) fn finish(item: &mut Option<Read>, result: io::Result<()>, notify: bool) {
    if let Some(read) = item.take() {
        read.job.finish(result, notify);
    }
}

pub(super) fn fail_batch(
    batch: Vec<Option<Read>>,
    receiver: mpsc::Receiver<Job>,
    depth: usize,
    error: io::Error,
) {
    for job in batch
        .into_iter()
        .flatten()
        .map(|read| read.job)
        .chain(receiver.try_iter().take(depth))
    {
        let cause = error.raw_os_error().map_or_else(
            || io::Error::new(error.kind(), error.to_string()),
            io::Error::from_raw_os_error,
        );
        job.finish(Err(cause), false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_slots_fit_small_reads_and_keep_large_offsets_aligned() {
        let geometry = IoAlignment {
            offset: 4096,
            memory: 4096,
            detected: true,
        };
        let layout = PoolLayout::new(256, geometry);
        let mut batch = layout.batch();
        assert_eq!(layout.bytes(), 3 * 1024 * 1024);
        for expected in 0..256 {
            let slot = batch.take(4096).unwrap();
            assert_eq!(slot.index, expected);
            assert_eq!(slot.offset, expected * 4096);
        }
        for expected in 0..32 {
            let slot = batch.take(8192).unwrap();
            assert_eq!(slot.index, 256 + expected);
            assert_eq!(slot.offset, 1024 * 1024 + expected * WINDOW_LIMIT);
        }
        assert!(batch.take(8192).is_none());
        let conservative = IoAlignment {
            offset: WINDOW_LIMIT,
            memory: WINDOW_LIMIT,
            detected: false,
        };
        let layout = PoolLayout::new(2, conservative);
        assert_eq!(layout.bytes(), 2 * WINDOW_LIMIT);
        assert_eq!(layout.slots(), 2);
    }
}
