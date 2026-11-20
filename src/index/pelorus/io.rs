//! Pelorus 精排行通过 Core 的跨平台异步读取器从当前容器代际读取。

use crate::resources::WorkspaceReservation;
use crate::storage::quiver_io::{AsyncReader, IoCounters, IoMode};
use crate::{ResourceRuntime, Result, TriviumError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::sync::{Arc, Mutex, atomic::Ordering::Relaxed};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PelorusResidency {
    #[default]
    Hot,
    Mmap,
    Ssd,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PelorusRuntimeOptions {
    pub residency: PelorusResidency,
    pub io_mode: IoMode,
    pub io_depth: usize,
    pub cache_bytes: usize,
    pub inflight_bytes: usize,
}

impl Default for PelorusRuntimeOptions {
    fn default() -> Self {
        Self {
            residency: PelorusResidency::Hot,
            io_mode: IoMode::Auto,
            io_depth: 4,
            cache_bytes: 128 * 1024 * 1024,
            inflight_bytes: 32 * 1024 * 1024,
        }
    }
}

type Row = Arc<Vec<u8>>;

#[derive(Default)]
struct Cache {
    order: BTreeMap<u128, usize>,
    rows: HashMap<usize, (u128, Row)>,
    clock: u128,
}

impl Cache {
    fn get(&mut self, slot: usize) -> Option<Row> {
        let (at, row) = self.rows.get_mut(&slot)?;
        self.order.remove(at);
        self.clock += 1;
        *at = self.clock;
        self.order.insert(*at, slot);
        Some(Arc::clone(row))
    }

    fn insert(&mut self, slot: usize, row: Row, capacity: usize) {
        if capacity == 0 || self.rows.contains_key(&slot) {
            return;
        }
        if self.rows.len() == capacity
            && let Some((_, oldest)) = self.order.pop_first()
        {
            self.rows.remove(&oldest);
        }
        self.clock += 1;
        self.order.insert(self.clock, slot);
        self.rows.insert(slot, (self.clock, row));
    }
}

pub(super) struct PelorusSsd {
    reader: AsyncReader,
    offset: u64,
    stride: usize,
    checksums: Vec<u32>,
    capacity: usize,
    inflight: Mutex<usize>,
    inflight_limit: usize,
    _memory: WorkspaceReservation,
    cache: Mutex<Cache>,
}

impl PelorusSsd {
    pub(super) fn reserved_bytes(&self) -> usize {
        self._memory.bytes()
    }

    pub(super) fn new(
        runtime: &ResourceRuntime,
        file: &File,
        offset: u64,
        records: &[u8],
        stride: usize,
        options: PelorusRuntimeOptions,
        available: usize,
    ) -> Result<Self> {
        if stride == 0 || !records.len().is_multiple_of(stride) {
            return Err(TriviumError::CorruptedFile(
                "Pelorus SSD 精排记录不完整 (Incomplete Pelorus SSD rerank records)".into(),
            ));
        }
        if options.inflight_bytes < stride.saturating_add(256) {
            return Err(TriviumError::InvalidInput(
                "在途预算至少容纳一个精排记录 (In-flight budget must cover one rerank record)"
                    .into(),
            )
            .at_field("inflight_bytes"));
        }
        let counted_row_bytes = stride.saturating_add(256);
        let capacity = (options.cache_bytes / counted_row_bytes).min(records.len() / stride);
        let base = capacity
            .saturating_mul(counted_row_bytes)
            .saturating_add(options.inflight_bytes)
            .saturating_add((records.len() / stride).saturating_mul(4));
        let check_budget = |bytes| {
            if bytes > available {
                return Err(TriviumError::ResourceBudgetExceeded {
                    resource: "pelorus_ssd_bytes",
                    requested: bytes,
                    used: 0,
                    limit: available,
                });
            }
            Ok(())
        };
        check_budget(base)?;
        let mut memory = runtime.reserve_workspace(base)?;
        let reader = AsyncReader::new(
            file,
            options.io_mode,
            options.io_depth,
            runtime,
            Arc::new(IoCounters::default()),
            |bytes| {
                let total = base.saturating_add(bytes);
                check_budget(total)?;
                memory.resize(total)
            },
        )?;
        let checksums = records.chunks_exact(stride).map(crc32fast::hash).collect();
        Ok(Self {
            reader,
            offset,
            stride,
            checksums,
            capacity,
            inflight: Mutex::new(0),
            inflight_limit: options.inflight_bytes,
            _memory: memory,
            cache: Mutex::new(Cache::default()),
        })
    }

    pub(super) fn batch_rows(&self) -> usize {
        (self.inflight_limit / (self.stride + 256)).clamp(1, 64)
    }

    pub(super) fn read_rows(
        &self,
        slots: &[usize],
        mut score: impl FnMut(usize, &[u8]),
    ) -> Result<()> {
        let bytes = slots.len().saturating_mul(self.stride.saturating_add(256));
        let mut used = crate::database::lock_or_recover(&self.inflight);
        if used.saturating_add(bytes) > self.inflight_limit {
            return Err(TriviumError::ResourceBudgetExceeded {
                resource: "pelorus_inflight_bytes",
                requested: bytes,
                used: *used,
                limit: self.inflight_limit,
            });
        }
        *used += bytes;
        drop(used);
        let _lease = ReadLease {
            bytes,
            inflight: &self.inflight,
        };
        let meter = crate::request::quiver_io_meter();
        let mut requests = Vec::new();
        let mut misses = Vec::new();
        let mut ready = Vec::new();
        {
            let mut cache = crate::database::lock_or_recover(&self.cache);
            for (position, &slot) in slots.iter().enumerate() {
                if slot >= self.checksums.len() {
                    return Err(invalid_row());
                }
                if let Some(row) = cache.get(slot) {
                    if let Some(meter) = &meter {
                        meter.row_cache_hits.fetch_add(1, Relaxed);
                    }
                    ready.push((position, row));
                } else {
                    let offset = slot
                        .checked_mul(self.stride)
                        .and_then(|start| self.offset.checked_add(start as u64))
                        .ok_or_else(invalid_row)?;
                    requests.push((offset, vec![0; self.stride]));
                    misses.push((position, slot));
                }
            }
        }
        if let Some(meter) = &meter {
            meter
                .cold_read_calls
                .fetch_add(misses.len() as u64, Relaxed);
            meter
                .cold_read_bytes
                .fetch_add((misses.len() * self.stride) as u64, Relaxed);
        }
        let rows = self
            .reader
            .read_batch_during(requests, meter.as_ref(), || {
                for (position, row) in &ready {
                    crate::request::check_current()?;
                    score(*position, row);
                }
                Ok(())
            })?;
        for ((position, slot), bytes) in misses.into_iter().zip(rows) {
            crate::request::check_current()?;
            if crc32fast::hash(&bytes) != self.checksums[slot] {
                return Err(invalid_row());
            }
            let row = Arc::new(bytes);
            score(position, &row);
            crate::database::lock_or_recover(&self.cache).insert(slot, row, self.capacity);
        }
        Ok(())
    }
}

struct ReadLease<'a> {
    bytes: usize,
    inflight: &'a Mutex<usize>,
}

impl Drop for ReadLease<'_> {
    fn drop(&mut self) {
        *crate::database::lock_or_recover(self.inflight) -= self.bytes;
    }
}

fn invalid_row() -> TriviumError {
    TriviumError::CorruptedFile(
        "Pelorus SSD 精排记录校验失败 (Pelorus SSD rerank row verification failed)".into(),
    )
}
