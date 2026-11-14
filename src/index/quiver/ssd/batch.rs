//! 有界批次先完成自己持有的读取，再等待其他请求，避免交叉Pending形成等待环。
use super::*;

struct PendingRead<'a> {
    reader: &'a SsdQuiver,
    position: usize,
    row: usize,
    flight: Option<Arc<Flight>>,
    memory: Option<MemoryLease>,
    charge: usize,
}

impl PendingRead<'_> {
    fn complete(mut self, bytes: Vec<u8>) -> Result<SsdRow> {
        let mut memory = self.memory.take().ok_or_else(capacity)?;
        memory.resize(self.charge)?;
        let value = SsdRow {
            layout: self.reader.active()?.file.layout(),
            center: self.reader.active()?.center.clone(),
            data: Arc::new(RowData {
                bytes,
                charge: self.charge,
                _memory: memory,
            }),
        };
        let mut cache = crate::database::lock_or_recover(&self.reader.cache);
        cache.pending.remove(&self.row);
        if self.charge <= self.reader.runtime_options.cache_bytes {
            cache.trim(self.reader.runtime_options.cache_bytes - self.charge);
            cache.clock = cache.clock.wrapping_add(1);
            let tick = cache.clock;
            cache.rows.insert(self.row, (value.clone(), tick));
            cache.lru.insert((tick, self.row));
            cache.bytes += self.charge;
        }
        if let Some(flight) = self.flight.take() {
            *crate::database::lock_or_recover(&flight.state) = (true, Some(value.clone()));
            flight.ready.notify_all();
        }
        Ok(value)
    }
}

impl Drop for PendingRead<'_> {
    fn drop(&mut self) {
        if let Some(flight) = self.flight.take() {
            let mut cache = crate::database::lock_or_recover(&self.reader.cache);
            cache.pending.remove(&self.row);
            *crate::database::lock_or_recover(&flight.state) = (true, None);
            flight.ready.notify_all();
        }
    }
}

impl SsdQuiver {
    pub(super) fn load_record_routed(&self, row: usize, route: RoutingView<'_>) -> Result<SsdRow> {
        self.load_records_routed(&[row], route)?
            .pop()
            .ok_or_else(capacity)
    }

    #[cfg(test)]
    pub(super) fn load_records(&self, rows: &[usize]) -> Result<Vec<SsdRow>> {
        self.load_records_routed(rows, self.routing(None)?)
    }

    pub(super) fn load_records_routed(
        &self,
        rows: &[usize],
        route: RoutingView<'_>,
    ) -> Result<Vec<SsdRow>> {
        let state = self.active()?;
        if rows.len() > 64 || rows.iter().any(|&row| row >= self.metadata.rows) {
            return Err(invalid("SSD批次范围无效 (Invalid SSD batch range)"));
        }
        crate::request::check_current()?;
        if let Some(hot) = &self.hot_rows {
            return Ok(rows.iter().map(|&row| hot[row].clone()).collect());
        }
        // 批次目录有固定上限；行、传输副本和对齐窗口按每条缺失记录另行准入。
        let _directory = self
            .memory
            .reserve(4096 + rows.len().saturating_sub(4) * 256)?;
        let _task = self.memory.runtime.enter_task()?;
        let charge = self
            .metadata
            .record_bytes
            .checked_add(512)
            .ok_or_else(capacity)?;
        let peak = charge
            .checked_add(state.file.io_workspace_bytes(self.metadata.record_bytes))
            .ok_or_else(capacity)?;
        let mut loaded = vec![None; rows.len()];
        while loaded.iter().any(Option::is_none) {
            crate::request::check_current()?;
            let mut reads = Vec::with_capacity(rows.len());
            let mut waiters = Vec::with_capacity(rows.len());
            {
                let mut cache = crate::database::lock_or_recover(&self.cache);
                for (position, &row) in rows.iter().enumerate() {
                    if loaded[position].is_some() {
                        continue;
                    }
                    if let Some((value, tick)) = cache.rows.get(&row).cloned() {
                        cache.hits += 1;
                        cache.lru.remove(&(tick, row));
                        cache.clock = cache.clock.wrapping_add(1);
                        let tick = cache.clock;
                        cache.lru.insert((tick, row));
                        cache.rows.insert(row, (value.clone(), tick));
                        loaded[position] = Some(value);
                    } else if let Some(flight) = cache.pending.get(&row).cloned() {
                        cache.coalesced += 1;
                        waiters.push((position, flight));
                    } else {
                        cache.trim(self.runtime_options.cache_bytes.saturating_sub(charge));
                        let memory = self.memory.reserve(peak)?;
                        let flight = Arc::new(Flight::default());
                        cache.pending.insert(row, flight.clone());
                        cache.misses += 1;
                        reads.push(PendingRead {
                            reader: self,
                            position,
                            row,
                            flight: Some(flight),
                            memory: Some(memory),
                            charge,
                        });
                    }
                }
            }
            if !reads.is_empty() {
                let requests = reads
                    .iter()
                    .map(|read| Ok((read.row, route.identity(read.row)?)))
                    .collect::<Result<Vec<_>>>()?;
                let buffers = state.file.read_records(&requests, &state.center.values)?;
                if buffers.len() != reads.len() {
                    return Err(capacity());
                }
                for (read, bytes) in reads.into_iter().zip(buffers) {
                    let position = read.position;
                    loaded[position] = Some(read.complete(bytes)?);
                }
            }
            for (position, flight) in waiters {
                #[cfg(feature = "test-hooks")]
                crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::QuiverColdCoalesced);
                let mut completed = crate::database::lock_or_recover(&flight.state);
                while !completed.0 {
                    completed = flight
                        .ready
                        .wait_timeout(completed, std::time::Duration::from_millis(10))
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .0;
                    crate::request::check_current()?;
                }
                loaded[position] = completed.1.clone();
            }
        }
        crate::request::check_current()?;
        loaded
            .into_iter()
            .map(|row| row.ok_or_else(capacity))
            .collect()
    }
}
