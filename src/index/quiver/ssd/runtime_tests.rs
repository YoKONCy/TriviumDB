//! 缓存租约、驻留转换与真实平台I/O的数据和资源契约。
use super::*;

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn zero_row_cache_reuses_bounded_io_windows_in_local_and_worker_backends() {
    for max_threads in [1, 2] {
        let directory = TestDir::new("ssd_io_window");
        let path = directory.0.join("index.qssd");
        let runtime = ResourceRuntime::new(crate::ResourceLimits {
            max_threads,
            ..Default::default()
        })
        .unwrap();
        let (index, vectors) = fixture(129);
        publish(&path, &index, &vectors, SOURCE, &runtime);
        let mut reader =
            SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
        let options = SsdRuntimeOptions {
            io_mode: IoMode::Direct,
            io_depth: 1,
            cache_bytes: 0,
            ..Default::default()
        };
        reader.configure(options).unwrap();
        let baseline = reader.stats();
        assert_eq!(baseline.io_window_bytes, 0);
        let query = Int8Query::new(&vectors[1]).unwrap();
        let first = reader.row(77).unwrap();
        assert_eq!(first.score(&query).unwrap(), -1.);
        drop(first);
        let before = reader.stats();
        assert_eq!(before.cache_used_bytes, 0);
        assert_eq!(before.reserved_bytes, baseline.reserved_bytes);
        assert!(before.io_window_bytes > 0 && before.io_window_bytes <= 65536);
        let pinned = reader.row(77).unwrap();
        assert_eq!(pinned.score(&query).unwrap(), -1.);
        let after = reader.stats();
        assert_eq!(after.physical_read_calls, before.physical_read_calls);
        assert_eq!(after.physical_read_bytes, before.physical_read_bytes);
        assert_eq!(after.window_reuse_hits, before.window_reuse_hits + 1);
        assert_eq!(
            after.window_reuse_bytes,
            before.window_reuse_bytes + after.metadata.record_bytes as u64
        );
        assert_eq!(after.cold_read_calls, before.cold_read_calls + 1);
        assert_eq!(after.cache_used_bytes, 0);
        drop(pinned);
        reader
            .configure(SsdRuntimeOptions {
                residency: Residency::Hot,
                ..options
            })
            .unwrap();
        assert_eq!(reader.stats().io_window_bytes, 0);
        assert_eq!(runtime.stats().allocated_worker_threads, 0);
        assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
        reader.configure(options).unwrap();
        assert_eq!(reader.stats().io_window_bytes, 0);
        let pinned = reader.row(77).unwrap();
        reader.close();
        assert_eq!(reader.stats().io_window_bytes, 0);
        std::fs::remove_file(path).unwrap();
        assert_eq!(pinned.score(&query).unwrap(), -1.);
        drop(pinned);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        assert_eq!(runtime.stats().live_worker_threads, 0);
    }
}

#[test]
fn batched_reranking_merges_reads_and_preserves_independent_scores() {
    for mode in [
        IoMode::Buffered,
        #[cfg(any(windows, target_os = "linux"))]
        IoMode::Direct,
    ] {
        let directory = TestDir::new("ssd_batch");
        let path = directory.0.join("index.qssd");
        let runtime = runtime();
        let (index, vectors) = fixture(129);
        publish(&path, &index, &vectors, SOURCE, &runtime);
        let mut reader =
            SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
        reader
            .configure(SsdRuntimeOptions {
                io_mode: mode,
                io_depth: 2,
                ..Default::default()
            })
            .unwrap();
        let batch = reader.load_records(&[3, 1, 0, 2]).unwrap();
        let query = Int8Query::new(&vectors[1]).unwrap();
        assert_eq!(
            batch
                .iter()
                .map(|row| row.score(&query).unwrap())
                .collect::<Vec<_>>(),
            [0., -1., 1., 1.]
        );
        drop(batch);
        let stats = reader.stats();
        assert_eq!(stats.cold_read_calls, 4);
        assert_eq!(
            stats.cold_read_bytes,
            4 * stats.metadata.record_bytes as u64
        );
        assert_eq!(stats.physical_read_calls, 1);
        assert!(stats.physical_read_bytes >= stats.cold_read_bytes);
        assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
        let before = reader.stats().cold_read_calls;
        let pinned = reader.load_records(&[1, 1, 0, 0]).unwrap();
        let scores = pinned
            .iter()
            .map(|row| row.score(&query).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(scores, [-1., -1., 1., 1.]);
        assert_eq!(reader.stats().cold_read_calls - before, 2);
        reader.close();
        std::fs::remove_file(path).unwrap();
        drop(pinned);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    }
}

#[test]
fn rejected_batch_preparation_releases_all_pending_rows_and_can_retry() {
    let directory = TestDir::new("ssd_batch_budget");
    let path = directory.0.join("index.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(129);
    publish(&path, &index, &vectors, SOURCE, &runtime);
    let mut reader =
        SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    let baseline = reader.stats().reserved_bytes;
    let peak = reader.metadata.record_bytes * 2 + 512;
    reader.set_database_capacity(baseline + 4096 + peak);
    assert_eq!(
        reader.load_records(&[0, 1]).err().unwrap().code(),
        "QueueFull"
    );
    assert!(
        crate::database::lock_or_recover(&reader.cache)
            .pending
            .is_empty()
    );
    assert_eq!(reader.stats().reserved_bytes, baseline);
    reader.set_database_capacity(0);
    assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
    reader.close();
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn hot_residency_releases_io_workers_and_failed_ssd_switch_keeps_hot_queries_available() {
    let directory = TestDir::new("ssd_hot_io_release");
    let path = directory.0.join("index.qssd");
    let runtime = ResourceRuntime::new(crate::ResourceLimits {
        max_threads: 2,
        ..Default::default()
    })
    .unwrap();
    let (index, vectors) = fixture(129);
    publish(&path, &index, &vectors, SOURCE, &runtime);
    let mut reader =
        SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    let ssd = SsdRuntimeOptions {
        io_depth: 1,
        ..Default::default()
    };
    reader.configure(ssd).unwrap();
    assert_eq!(runtime.stats().allocated_worker_threads, 1);
    reader
        .configure(SsdRuntimeOptions {
            residency: Residency::Hot,
            ..ssd
        })
        .unwrap();
    assert!(!reader.stats().io_active);
    assert_eq!(runtime.stats().allocated_worker_threads, 0);
    assert_eq!(runtime.stats().live_worker_threads, 0);
    let cold_reads = reader.stats().cold_read_calls;
    let cpu = runtime.pool(2).unwrap();
    assert_eq!(
        reader.configure(ssd).unwrap_err().details()["resource"],
        "worker_threads"
    );
    assert_eq!(reader.stats().residency, Residency::Hot);
    assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
    assert_eq!(reader.stats().cold_read_calls, cold_reads);
    drop(cpu);
    reader.configure(ssd).unwrap();
    assert!(reader.stats().io_active);
    assert_eq!(reader.stats().residency, Residency::Ssd);
    assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
    reader.close();
    std::fs::remove_file(path).unwrap();
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    assert_eq!(runtime.stats().allocated_worker_threads, 0);
}

#[cfg(all(feature = "test-hooks", any(windows, target_os = "linux")))]
#[test]
fn cancelled_multi_range_batch_drains_every_submission_and_releases_pending_rows() {
    use crate::test_hooks::{ConcurrencyPoint, pause_at};
    let directory = TestDir::new("ssd_batch_cancel");
    let path = directory.0.join("index.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(129);
    SsdQuiver::publish(
        &index,
        &path,
        SOURCE,
        SsdOptions {
            row_alignment: 65536,
            ..Default::default()
        },
        &runtime,
        |slot, output| {
            output.copy_from_slice(&vectors[slot]);
            Ok(())
        },
    )
    .unwrap();
    let mut reader =
        SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    reader
        .configure(SsdRuntimeOptions {
            io_mode: IoMode::Direct,
            io_depth: 2,
            ..Default::default()
        })
        .unwrap();
    let baseline = reader.stats().reserved_bytes;
    let reader = Arc::new(reader);
    let completed = pause_at(ConcurrencyPoint::QuiverColdIoCompleted);
    let submitted = pause_at(ConcurrencyPoint::QuiverColdBatchSubmitted);
    let context = crate::sdk::RequestContext::with_timeout(
        "ssd-batch-cancel",
        std::time::Duration::ZERO,
        false,
    )
    .unwrap();
    let worker_context = context.clone();
    let worker_reader = reader.clone();
    let worker =
        std::thread::spawn(move || worker_context.run(|| worker_reader.load_records(&[0, 3])));
    submitted.wait_until_arrived();
    completed.wait_until_arrived();
    context.cancel();
    drop(submitted);
    drop(completed);
    assert_eq!(
        worker.join().unwrap().err().unwrap().code(),
        "TDB_QUERY_CANCELLED"
    );
    assert_eq!(reader.stats().physical_read_calls, 2);
    assert_eq!(reader.stats().reserved_bytes, baseline);
    assert!(
        crate::database::lock_or_recover(&reader.cache)
            .pending
            .is_empty()
    );
    assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
    let mut reader = Arc::try_unwrap(reader).unwrap_or_else(|_| panic!("批次须归还Reader所有权"));
    reader.close();
    std::fs::remove_file(path).unwrap();
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    assert_eq!(runtime.stats().live_worker_threads, 0);
}

#[test]
fn cache_retains_rows_and_shrinking_keeps_external_leases_accounted() {
    let directory = TestDir::new("ssd_cache");
    let path = directory.0.join("cache.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(129);
    let metadata = publish(&path, &index, &vectors, SOURCE, &runtime);
    let charge = metadata.record_bytes + 512;
    let mut reader =
        SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    reader
        .configure(SsdRuntimeOptions {
            cache_bytes: charge * 2,
            io_depth: 1,
            ..Default::default()
        })
        .unwrap();
    let baseline = reader.stats().reserved_bytes;
    let pinned = reader.row(9).unwrap();
    assert_eq!(
        pinned.score(&Int8Query::new(&vectors[1]).unwrap()).unwrap(),
        1.
    );
    drop(reader.row(9).unwrap());
    assert_eq!(reader.stats().cold_read_calls, 1);
    assert_eq!(reader.stats().cache_hits, 1);
    drop(reader.row(77).unwrap());
    drop(reader.row(40).unwrap());
    assert_eq!(reader.stats().cache_used_bytes, charge * 2);
    assert_eq!(reader.stats().reserved_bytes, baseline + charge * 3);
    reader
        .configure(SsdRuntimeOptions {
            io_depth: 1,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(reader.stats().cache_used_bytes, 0);
    assert_eq!(reader.stats().reserved_bytes, baseline + charge);
    reader.close();
    assert_eq!(runtime.stats().allocated_worker_threads, 0);
    assert_eq!(runtime.stats().live_worker_threads, 0);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        pinned.score(&Int8Query::new(&vectors[1]).unwrap()).unwrap(),
        1.
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, charge);
    drop(pinned);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn residency_roundtrip_preserves_scores_and_hot_queries_use_no_cold_reads() {
    for dim in [1, 129, 768, 3072] {
        let directory = TestDir::new("ssd_residency");
        let path = directory.0.join("index.qssd");
        let runtime = runtime();
        let (index, vectors) = fixture(dim);
        publish(&path, &index, &vectors, SOURCE, &runtime);
        let mut reader =
            SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
        reader
            .configure(SsdRuntimeOptions {
                io_depth: 1,
                ..Default::default()
            })
            .unwrap();
        let ssd_baseline = reader.stats().reserved_bytes;
        reader
            .configure(SsdRuntimeOptions {
                residency: Residency::Hot,
                io_depth: 1,
                ..Default::default()
            })
            .unwrap();
        let reads = reader.stats().cold_read_calls;
        assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
        assert_eq!(reader.stats().cold_read_calls, reads);
        reader
            .configure(SsdRuntimeOptions {
                io_depth: 1,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(reader.stats().reserved_bytes, ssd_baseline);
        assert!(reader.stats().io_active);
        assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
        assert_eq!(reader.stats().cold_read_calls, reads + 4);
        drop(reader);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        assert_eq!(runtime.stats().live_worker_threads, 0);
    }
}

#[test]
fn failed_warmup_preserves_residency_and_can_retry_after_budget_release() {
    let directory = TestDir::new("ssd_warm_budget");
    let path = directory.0.join("index.qssd");
    let runtime = ResourceRuntime::new(crate::ResourceLimits {
        memory_limit: 2 * 1024 * 1024,
        ..Default::default()
    })
    .unwrap();
    let (index, vectors) = fixture(129);
    publish(&path, &index, &vectors, SOURCE, &runtime);
    let mut reader =
        SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    reader
        .configure(SsdRuntimeOptions {
            io_depth: 1,
            ..Default::default()
        })
        .unwrap();
    let baseline = reader.stats().reserved_bytes;
    let available = runtime.stats().available_foreground_bytes.unwrap();
    let blocker = runtime.reserve_workspace(available - 1).unwrap();
    let error = reader
        .configure(SsdRuntimeOptions {
            residency: Residency::Hot,
            io_depth: 1,
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(error.code(), "QueueFull");
    assert_eq!(reader.stats().residency, Residency::Ssd);
    assert_eq!(reader.stats().reserved_bytes, baseline);
    drop(blocker);
    assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
    reader
        .configure(SsdRuntimeOptions {
            residency: Residency::Hot,
            io_depth: 1,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
    reader.close();
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn strict_direct_io_reads_real_tail_records_and_pins_the_open_generation() {
    for dim in [1, 129, 3072] {
        let directory = TestDir::new("ssd_direct");
        let path = directory.0.join("index.qssd");
        let runtime = runtime();
        let (index, vectors) = fixture(dim);
        publish(&path, &index, &vectors, SOURCE, &runtime);
        let mut reader =
            SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
        reader
            .configure(SsdRuntimeOptions {
                io_mode: IoMode::Direct,
                io_depth: 2,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(reader.stats().io_mode, IoMode::Direct);
        std::fs::rename(&path, directory.0.join("held.qssd")).unwrap();
        assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
        let row = reader.row(77).unwrap();
        reader.close();
        std::fs::remove_file(directory.0.join("held.qssd")).unwrap();
        assert_eq!(
            row.score(&Int8Query::new(&vectors[1]).unwrap()).unwrap(),
            -1.
        );
        drop(row);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        assert_eq!(runtime.stats().live_worker_threads, 0);
        assert_eq!(runtime.stats().allocated_worker_threads, 0);
    }
}

#[test]
fn temporary_collection_skips_active_publishers_and_foreign_files() {
    let directory = TestDir::new("ssd_collect");
    let path = directory.0.join("index.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(129);
    let orphan = directory
        .0
        .join(format!("index.qssd.qssd-{}.tmp", uuid::Uuid::new_v4()));
    let lease = Path::new(&format!("{}.lease", orphan.display())).to_path_buf();
    std::fs::write(&orphan, b"private partial candidate").unwrap();
    std::fs::write(&lease, []).unwrap();
    let foreign = directory.0.join("index.qssd.qssd-not-a-uuid.tmp.lease");
    std::fs::write(&foreign, b"foreign").unwrap();
    assert_eq!(SsdQuiver::collect_temporary(&path).unwrap(), 1);
    assert!(!orphan.exists());
    assert!(!lease.exists());
    SsdQuiver::publish(
        &index,
        &path,
        SOURCE,
        SsdOptions::default(),
        &runtime,
        |slot, output| {
            assert_eq!(SsdQuiver::collect_temporary(&path).unwrap(), 0);
            output.copy_from_slice(&vectors[slot]);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(std::fs::read(&foreign).unwrap(), b"foreign");
    let reader = SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime).unwrap();
    assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
}

#[cfg(feature = "test-hooks")]
#[test]
fn zero_cache_coalesces_pending_reads() {
    use crate::test_hooks::{ConcurrencyPoint, pause_at, pause_at_path};
    let directory = TestDir::new("ssd_pending");
    let path = directory.0.join("index.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(129);
    publish(&path, &index, &vectors, SOURCE, &runtime);
    let reader =
        Arc::new(SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap());
    let owner_gate = pause_at_path(
        ConcurrencyPoint::QuiverColdBeforeRead,
        path.to_str().unwrap(),
    );
    let owner_reader = reader.clone();
    let owner = std::thread::spawn(move || owner_reader.row(9));
    owner_gate.wait_until_arrived();
    let waiter_gate = pause_at(ConcurrencyPoint::QuiverColdCoalesced);
    let waiter_reader = reader.clone();
    let waiter = std::thread::spawn(move || waiter_reader.row(9));
    waiter_gate.wait_until_arrived();
    drop(owner_gate);
    let owned = owner.join().unwrap().unwrap();
    drop(waiter_gate);
    let waited = waiter.join().unwrap().unwrap();
    assert_eq!(reader.stats().cold_read_calls, 1);
    assert_eq!(reader.stats().cache_used_bytes, 0);
    assert_eq!(reader.stats().coalesced_reads, 1);
    assert_eq!(
        owned.score(&Int8Query::new(&vectors[1]).unwrap()).unwrap(),
        1.
    );
    assert_eq!(
        waited.score(&Int8Query::new(&vectors[1]).unwrap()).unwrap(),
        1.
    );
    drop(reader);
    drop(owned);
    drop(waited);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn io_workers_share_runtime_thread_admission_and_release_on_close() {
    let directory = TestDir::new("ssd_threads");
    let path = directory.0.join("index.qssd");
    let runtime = ResourceRuntime::new(crate::ResourceLimits {
        max_threads: 2,
        ..Default::default()
    })
    .unwrap();
    let (index, vectors) = fixture(129);
    publish(&path, &index, &vectors, SOURCE, &runtime);
    let mut reader =
        SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    let cpu = runtime.pool(2).unwrap();
    let before = reader.stats().reserved_bytes;
    let error = reader
        .configure(SsdRuntimeOptions {
            io_depth: 1,
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(error.details()["resource"], "worker_threads");
    assert_eq!(reader.stats().reserved_bytes, before);
    assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
    drop(cpu);
    reader
        .configure(SsdRuntimeOptions {
            io_depth: 1,
            ..Default::default()
        })
        .unwrap();
    let cpu = runtime.pool(2).unwrap();
    assert_eq!(cpu.install(rayon::current_num_threads), 1);
    assert_eq!(runtime.stats().allocated_worker_threads, 2);
    assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
    reader.close();
    assert_eq!(runtime.stats().allocated_worker_threads, 1);
    drop(cpu);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn multiple_readers_preserve_shared_cpu_capacity_at_the_io_thread_limit() {
    let directory = TestDir::new("ssd_shared_io_capacity");
    let path = directory.0.join("index.qssd");
    let runtime = ResourceRuntime::new(crate::ResourceLimits {
        max_threads: 2,
        ..Default::default()
    })
    .unwrap();
    let (index, vectors) = fixture(129);
    publish(&path, &index, &vectors, SOURCE, &runtime);
    let mut first = SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    first
        .configure(SsdRuntimeOptions {
            io_depth: 1,
            ..Default::default()
        })
        .unwrap();
    let mut second =
        SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    second
        .configure(SsdRuntimeOptions {
            io_depth: 1,
            ..Default::default()
        })
        .unwrap();
    let cpu = runtime.pool(2).unwrap();
    assert_eq!(cpu.install(rayon::current_num_threads), 1);
    assert_eq!(runtime.stats().allocated_worker_threads, 2);
    assert_eq!(first.search(&vectors[1], &config()).unwrap(), expected());
    assert_eq!(second.search(&vectors[1], &config()).unwrap(), expected());
    first.close();
    second.close();
    assert_eq!(runtime.stats().allocated_worker_threads, 1);
    drop(cpu);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[cfg(feature = "test-hooks")]
#[test]
fn cancelled_waiter_leaves_the_shared_read_available_to_its_owner() {
    use crate::test_hooks::{ConcurrencyPoint, pause_at, pause_at_path};
    let directory = TestDir::new("ssd_pending_cancel");
    let path = directory.0.join("index.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(129);
    publish(&path, &index, &vectors, SOURCE, &runtime);
    let reader =
        Arc::new(SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap());
    let owner_gate = pause_at_path(
        ConcurrencyPoint::QuiverColdBeforeRead,
        path.to_str().unwrap(),
    );
    let owner_reader = reader.clone();
    let owner = std::thread::spawn(move || owner_reader.row(9));
    owner_gate.wait_until_arrived();
    let waiter_gate = pause_at(ConcurrencyPoint::QuiverColdCoalesced);
    let context =
        crate::sdk::RequestContext::with_timeout("ssd-waiter", std::time::Duration::ZERO, false)
            .unwrap();
    let waiter_context = context.clone();
    let waiter_reader = reader.clone();
    let waiter = std::thread::spawn(move || waiter_context.run(|| waiter_reader.row(9)));
    waiter_gate.wait_until_arrived();
    context.cancel();
    drop(waiter_gate);
    assert_eq!(
        waiter.join().unwrap().err().unwrap().code(),
        "TDB_QUERY_CANCELLED"
    );
    drop(owner_gate);
    let row = owner.join().unwrap().unwrap();
    assert_eq!(
        row.score(&Int8Query::new(&vectors[1]).unwrap()).unwrap(),
        1.
    );
    assert_eq!(reader.stats().cold_read_calls, 1);
    drop(row);
    drop(reader);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[cfg(all(feature = "test-hooks", any(windows, target_os = "linux")))]
#[test]
fn cancellation_after_native_completion_drains_workers_and_releases_files() {
    use crate::test_hooks::{ConcurrencyPoint, pause_at};
    let directory = TestDir::new("ssd_completion_cancel");
    let path = directory.0.join("index.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(129);
    publish(&path, &index, &vectors, SOURCE, &runtime);
    let mut reader =
        SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    reader
        .configure(SsdRuntimeOptions {
            io_mode: IoMode::Direct,
            io_depth: 1,
            ..Default::default()
        })
        .unwrap();
    let baseline = reader.stats().reserved_bytes;
    let reader = Arc::new(reader);
    let gate = pause_at(ConcurrencyPoint::QuiverColdIoCompleted);
    let context = crate::sdk::RequestContext::with_timeout(
        "ssd-completion",
        std::time::Duration::ZERO,
        false,
    )
    .unwrap();
    let worker_context = context.clone();
    let worker_reader = reader.clone();
    let worker = std::thread::spawn(move || worker_context.run(|| worker_reader.row(9)));
    gate.wait_until_arrived();
    assert!(reader.stats().reserved_bytes > baseline);
    context.cancel();
    drop(gate);
    assert_eq!(
        worker.join().unwrap().err().unwrap().code(),
        "TDB_QUERY_CANCELLED"
    );
    assert_eq!(reader.stats().reserved_bytes, baseline);
    assert_eq!(reader.stats().cache_used_bytes, 0);
    let mut reader = Arc::try_unwrap(reader).unwrap_or_else(|_| panic!("请求必须归还Reader所有权"));
    assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
    reader.close();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    assert_eq!(runtime.stats().live_worker_threads, 0);
}
