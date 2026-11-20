//! 同一已打开文件代际的对齐页共享。先完成本批持有的读取，再等其他批次的页。

use super::{AsyncReader, IoCounters, IoMeter};
use crate::database::lock_or_recover;
use crate::error::{Result, TriviumError};
use crate::storage::quiver_cold::zeroed;
use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const PAGE_ENTRY_BYTES: usize = 256;

pub(super) struct SharedPages {
    grain: usize,
    file_len: u64,
    bytes: usize,
    counters: Arc<IoCounters>,
    cache: Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    ready: BTreeMap<u64, Arc<Vec<u8>>>,
    fifo: VecDeque<u64>,
    pending: BTreeMap<u64, Arc<Flight>>,
}

enum Page {
    Ready(Arc<Vec<u8>>),
    InFlight(Arc<Flight>),
}

#[derive(Default)]
struct Flight {
    state: Mutex<FlightState>,
    changed: Condvar,
}

#[derive(Default)]
enum FlightState {
    #[default]
    Reading,
    Complete(Option<Arc<Vec<u8>>>),
}

impl Flight {
    fn wait(&self) -> Result<Option<Arc<Vec<u8>>>> {
        let mut state = lock_or_recover(&self.state);
        loop {
            if let FlightState::Complete(bytes) = &*state {
                return Ok(bytes.clone());
            }
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(10))
                .unwrap_or_else(|error| error.into_inner())
                .0;
            crate::request::check_current()?;
        }
    }
}

struct Claim<'a> {
    pages: &'a SharedPages,
    offset: u64,
    flight: Option<Arc<Flight>>,
}

impl Claim<'_> {
    fn finish(&mut self, bytes: Option<Arc<Vec<u8>>>) {
        let Some(flight) = self.flight.take() else {
            return;
        };
        let mut cache = lock_or_recover(&self.pages.cache);
        cache.pending.remove(&self.offset);
        if let Some(bytes) = &bytes {
            if cache.ready.len() == self.pages.bytes / (self.pages.grain + PAGE_ENTRY_BYTES)
                && let Some(oldest) = cache.fifo.pop_front()
                && let Some(old) = cache.ready.remove(&oldest)
            {
                self.pages
                    .counters
                    .window_bytes
                    .fetch_sub(old.len() as u64, Relaxed);
            }
            cache.fifo.push_back(self.offset);
            cache.ready.insert(self.offset, Arc::clone(bytes));
            self.pages
                .counters
                .window_bytes
                .fetch_add(bytes.len() as u64, Relaxed);
        }
        *lock_or_recover(&flight.state) = FlightState::Complete(bytes);
        flight.changed.notify_all();
    }
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.finish(None);
    }
}

struct Fragment {
    offset: u64,
    skip: usize,
    len: usize,
    page: Page,
    reused: bool,
}

enum Plan {
    Contiguous(usize),
    Pages(Vec<u8>, Vec<Fragment>),
}

impl SharedPages {
    pub(super) fn new(
        grain: usize,
        file_len: u64,
        workers: usize,
        counters: Arc<IoCounters>,
    ) -> Self {
        Self {
            grain,
            file_len,
            bytes: super::shared_window_budget(workers),
            counters,
            cache: Mutex::new(Cache::default()),
        }
    }

    pub(super) fn budget(&self) -> usize {
        self.bytes
    }

    pub(super) fn clear_ready(&self) {
        let mut cache = lock_or_recover(&self.cache);
        let bytes: usize = cache.ready.values().map(|page| page.len()).sum();
        cache.ready.clear();
        cache.fifo.clear();
        self.counters.window_bytes.fetch_sub(bytes as u64, Relaxed);
    }

    fn page_len(&self, offset: u64) -> usize {
        self.file_len.saturating_sub(offset).min(self.grain as u64) as usize
    }

    fn lookup(&self, offset: u64) -> (Page, Option<Claim<'_>>) {
        let mut cache = lock_or_recover(&self.cache);
        if let Some(bytes) = cache.ready.get(&offset) {
            return (Page::Ready(Arc::clone(bytes)), None);
        }
        if let Some(flight) = cache.pending.get(&offset) {
            return (Page::InFlight(Arc::clone(flight)), None);
        }
        let flight = Arc::new(Flight::default());
        cache.pending.insert(offset, Arc::clone(&flight));
        (
            Page::InFlight(Arc::clone(&flight)),
            Some(Claim {
                pages: self,
                offset,
                flight: Some(flight),
            }),
        )
    }

    pub(super) fn read_during(
        &self,
        reader: &AsyncReader,
        requests: Vec<(u64, Vec<u8>)>,
        meter: Option<&Arc<IoMeter>>,
        ready: impl FnOnce() -> Result<()>,
    ) -> Result<Vec<Vec<u8>>> {
        crate::request::check_current()?;
        let mut plans = Vec::with_capacity(requests.len());
        let mut reads = Vec::with_capacity(requests.len());
        let mut owned = Vec::new();
        for (start, output) in requests {
            // 大范围保留现有的合并读，避免把连续传输拆成多个小请求。
            if output.is_empty() || output.len() > 2 * self.grain {
                plans.push(Plan::Contiguous(reads.len()));
                reads.push((start, output));
                continue;
            }
            let end = start
                .checked_add(output.len() as u64)
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            if end > self.file_len {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
            }
            let mut fragments = Vec::with_capacity(3);
            let mut position = start;
            while position < end {
                let offset = position / self.grain as u64 * self.grain as u64;
                let skip = (position - offset) as usize;
                let len = (end - position).min((self.grain - skip) as u64) as usize;
                let (page, claim) = self.lookup(offset);
                let reused = claim.is_none();
                #[cfg(feature = "test-hooks")]
                if reused && matches!(page, Page::InFlight(_)) {
                    crate::test_hooks::hit(
                        crate::test_hooks::ConcurrencyPoint::QuiverColdPageCoalesced,
                    );
                }
                if let Some(claim) = claim {
                    owned.push((reads.len(), claim));
                    reads.push((offset, zeroed(self.page_len(offset))?));
                }
                fragments.push(Fragment {
                    offset,
                    skip,
                    len,
                    page,
                    reused,
                });
                position += len as u64;
            }
            plans.push(Plan::Pages(output, fragments));
        }
        let mut completed = reader.read_ranges_during(reads, meter, ready)?;
        for (index, mut claim) in owned {
            claim.finish(Some(Arc::new(std::mem::take(&mut completed[index]))));
        }
        plans
            .into_iter()
            .map(|plan| match plan {
                Plan::Contiguous(index) => Ok(std::mem::take(&mut completed[index])),
                Plan::Pages(mut output, fragments) => {
                    let mut copied = 0;
                    for fragment in fragments {
                        crate::request::check_current()?;
                        let coalesced = matches!(fragment.page, Page::InFlight(_));
                        let bytes = match fragment.page {
                            Page::Ready(bytes) => Some(bytes),
                            Page::InFlight(flight) => flight.wait()?,
                        };
                        let range = fragment.skip..fragment.skip + fragment.len;
                        let target = &mut output[copied..copied + fragment.len];
                        if let Some(bytes) = bytes {
                            let source = bytes
                                .get(range)
                                .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
                            target.copy_from_slice(source);
                            if fragment.reused {
                                self.counters.record_window(
                                    meter.map(Arc::as_ref),
                                    fragment.len,
                                    coalesced,
                                );
                            }
                        } else {
                            // 原批取消或失败时由当前请求重试自己的逻辑范围。
                            let bytes = reader.read_ranges_during(
                                vec![(
                                    fragment.offset + fragment.skip as u64,
                                    zeroed(fragment.len)?,
                                )],
                                meter,
                                || Ok(()),
                            )?;
                            target.copy_from_slice(
                                bytes.first().ok_or(TriviumError::DatabaseClosed)?,
                            );
                        }
                        copied += fragment.len;
                    }
                    Ok(output)
                }
            })
            .collect()
    }
}

impl Drop for SharedPages {
    fn drop(&mut self) {
        self.clear_ready();
    }
}
