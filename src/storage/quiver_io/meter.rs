//! 单次请求的冷读计量。计数随作业跨线程传递，与 Reader 累计统计独立。

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct QuiverIoStats {
    pub cold_read_calls: u64,
    pub cold_read_bytes: u64,
    pub physical_read_calls: u64,
    pub physical_read_bytes: u64,
    pub window_reuse_hits: u64,
    pub window_reuse_bytes: u64,
    pub row_cache_hits: u64,
    pub coalesced_row_reads: u64,
    pub coalesced_page_reads: u64,
}

#[derive(Default)]
pub(crate) struct IoMeter {
    pub cold_read_calls: AtomicU64,
    pub cold_read_bytes: AtomicU64,
    pub physical_read_calls: AtomicU64,
    pub physical_read_bytes: AtomicU64,
    pub window_reuse_hits: AtomicU64,
    pub window_reuse_bytes: AtomicU64,
    pub row_cache_hits: AtomicU64,
    pub coalesced_row_reads: AtomicU64,
    pub coalesced_page_reads: AtomicU64,
}

impl IoMeter {
    pub(crate) fn snapshot(&self) -> QuiverIoStats {
        QuiverIoStats {
            cold_read_calls: self.cold_read_calls.load(Relaxed),
            cold_read_bytes: self.cold_read_bytes.load(Relaxed),
            physical_read_calls: self.physical_read_calls.load(Relaxed),
            physical_read_bytes: self.physical_read_bytes.load(Relaxed),
            window_reuse_hits: self.window_reuse_hits.load(Relaxed),
            window_reuse_bytes: self.window_reuse_bytes.load(Relaxed),
            row_cache_hits: self.row_cache_hits.load(Relaxed),
            coalesced_row_reads: self.coalesced_row_reads.load(Relaxed),
            coalesced_page_reads: self.coalesced_page_reads.load(Relaxed),
        }
    }
}
