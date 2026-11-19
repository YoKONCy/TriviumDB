//! 原生读取以独立字节样例核对对齐、窗口失效及文件代际。
use super::*;

struct Fixture {
    directory: std::path::PathBuf,
    bytes: Vec<u8>,
}
impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("tdb-io-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let bytes = (0..WINDOW_LIMIT * 4 + 73)
            .map(|i| ((i * 37 + i / 997) % 251) as u8)
            .collect::<Vec<_>>();
        std::fs::write(directory.join("data"), &bytes).unwrap();
        Self { directory, bytes }
    }
    fn file(&self) -> File {
        File::open(self.directory.join("data")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn legacy_linux_alignment_reads_the_pinned_file_at_a_nonzero_offset() {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::FileExt;

    let fixture = Fixture::new();
    let direct = open_linux_direct(&fixture.file()).unwrap();
    let alignment = alignment::xfs_or_conservative(&direct).unwrap();

    // SAFETY: fstatfs独占写入POD，文件与返回结构均在调用中存活。
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::fstatfs(direct.as_raw_fd(), &mut fs) }, 0);
    if fs.f_type == libc::XFS_SUPER_MAGIC {
        assert!(alignment.detected);
        assert!(alignment.offset >= fs.f_bsize as usize);
    } else {
        assert!(!alignment.detected);
        assert_eq!(alignment.offset, WINDOW_LIMIT);
    }
    let grain = alignment.offset;
    let buffer = Aligned::new(grain, alignment.memory).unwrap();
    // SAFETY: 分配已对齐且覆盖完整切片，本测试独占缓冲直到同步读取完成。
    let bytes = unsafe { std::slice::from_raw_parts_mut(buffer.ptr, grain) };
    assert_eq!(direct.read_at(bytes, grain as u64).unwrap(), grain);
    assert_eq!(bytes, &fixture.bytes[grain..2 * grain]);
}

#[test]
fn native_window_reuses_complete_bytes_and_invalidates_after_short_read() {
    let fixture = Fixture::new();
    let mut backend = Backend::direct(&fixture.file()).unwrap();
    let geometry = backend.alignment();
    assert!(geometry.offset.is_power_of_two() && geometry.offset <= WINDOW_LIMIT);
    assert!(geometry.memory.is_power_of_two() && geometry.memory <= WINDOW_LIMIT);
    let counters = Arc::new(IoCounters::default());
    let start = geometry.offset * 2 + 13;
    let span = (start % geometry.offset + 31).div_ceil(geometry.offset) * geometry.offset;
    let mut output = [0u8; 31];
    backend
        .read(start as u64, &mut output, &counters, None)
        .unwrap();
    assert_eq!(output, fixture.bytes[start..start + 31]);
    assert_eq!(counters.calls.load(Ordering::Relaxed), 1);
    assert_eq!(counters.bytes.load(Ordering::Relaxed), span as u64);
    backend
        .read((start + 3) as u64, &mut output[..17], &counters, None)
        .unwrap();
    assert_eq!(output[..17], fixture.bytes[start + 3..start + 20]);
    assert_eq!(counters.calls.load(Ordering::Relaxed), 1);
    assert_eq!(counters.window_hits.load(Ordering::Relaxed), 1);
    assert_eq!(counters.window_hit_bytes.load(Ordering::Relaxed), 17);
    let Backend::Direct(reader) = &backend else {
        panic!("需要原生后端")
    };
    let buffer = &reader.window.as_ref().unwrap().buffer;
    assert_eq!(buffer.ptr as usize % geometry.memory, 0);
    assert_eq!(counters.window_bytes.load(Ordering::Relaxed), span as u64);

    // 文件末尾只有10字节，覆盖中的失败读取必须使旧窗口失效。
    assert_eq!(
        backend
            .read(
                (fixture.bytes.len() - 10) as u64,
                &mut output,
                &counters,
                None
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::UnexpectedEof
    );
    backend
        .read(start as u64, &mut output, &counters, None)
        .unwrap();
    assert_eq!(output, fixture.bytes[start..start + 31]);
    assert_eq!(counters.calls.load(Ordering::Relaxed), 3);
    backend
        .read(
            (fixture.bytes.len() - 31) as u64,
            &mut output,
            &counters,
            None,
        )
        .unwrap();
    assert_eq!(output, fixture.bytes[fixture.bytes.len() - 31..]);
    let calls = counters.calls.load(Ordering::Relaxed);
    assert_eq!(
        backend
            .read(u64::MAX - 7, &mut output, &counters, None)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(counters.calls.load(Ordering::Relaxed), calls);
    drop(backend);
    assert_eq!(counters.window_bytes.load(Ordering::Relaxed), 0);
}

#[test]
fn large_and_cross_boundary_reads_keep_only_bounded_windows() {
    let fixture = Fixture::new();
    let mut backend = Backend::direct(&fixture.file()).unwrap();
    let counters = Arc::new(IoCounters::default());
    let grain = backend.alignment().offset;
    for (offset, length) in [
        (grain.saturating_sub(7), 31),
        (0, WINDOW_LIMIT),
        (17, 19),
        (31, WINDOW_LIMIT + 17),
        (fixture.bytes.len() - 73, 73),
    ] {
        let mut output = vec![0; length];
        backend
            .read(offset as u64, &mut output, &counters, None)
            .unwrap();
        assert_eq!(output, fixture.bytes[offset..offset + length]);
        assert!(counters.window_bytes.load(Ordering::Relaxed) <= WINDOW_LIMIT as u64);
    }
    assert!(counters.window_hits.load(Ordering::Relaxed) >= 1);
    drop(backend);
    assert_eq!(counters.window_bytes.load(Ordering::Relaxed), 0);
}

#[test]
fn retained_allocation_submits_only_the_current_aligned_span() {
    let fixture = Fixture::new();
    let mut backend = Backend::direct(&fixture.file()).unwrap();
    let counters = Arc::new(IoCounters::default());
    let grain = backend.alignment().offset;
    let mut whole = vec![0u8; WINDOW_LIMIT];
    backend.read(0, &mut whole, &counters, None).unwrap();
    assert_eq!(whole, fixture.bytes[..WINDOW_LIMIT]);
    let mut part = [0u8; 31];
    let start = WINDOW_LIMIT * 2;
    backend
        .read(start as u64, &mut part, &counters, None)
        .unwrap();
    assert_eq!(part, fixture.bytes[start..start + 31]);
    assert_eq!(counters.calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        counters.bytes.load(Ordering::Relaxed),
        (WINDOW_LIMIT + 31usize.div_ceil(grain) * grain) as u64
    );
    assert_eq!(
        counters.window_bytes.load(Ordering::Relaxed),
        WINDOW_LIMIT as u64
    );
    drop(backend);
    assert_eq!(counters.window_bytes.load(Ordering::Relaxed), 0);
}

#[test]
fn cached_windows_follow_pinned_handles_across_path_replacement() {
    let fixture = Fixture::new();
    let mut old = Backend::direct(&fixture.file()).unwrap();
    let counters = Arc::new(IoCounters::default());
    let mut output = [0u8; 31];
    old.read(13, &mut output, &counters, None).unwrap();
    assert_eq!(output, fixture.bytes[13..44]);
    std::fs::rename(
        fixture.directory.join("data"),
        fixture.directory.join("held"),
    )
    .unwrap();
    let replacement = fixture
        .bytes
        .iter()
        .map(|byte| byte ^ 255)
        .collect::<Vec<_>>();
    std::fs::write(fixture.directory.join("data"), &replacement).unwrap();
    let mut new = Backend::direct(&fixture.file()).unwrap();
    new.read(13, &mut output, &counters, None).unwrap();
    assert_eq!(output, replacement[13..44]);
    old.read(13, &mut output, &counters, None).unwrap();
    assert_eq!(output, fixture.bytes[13..44]);
    assert_eq!(counters.calls.load(Ordering::Relaxed), 2);
    assert_eq!(counters.window_hits.load(Ordering::Relaxed), 1);
    drop((old, new));
    assert_eq!(counters.window_bytes.load(Ordering::Relaxed), 0);
}

#[cfg(all(windows, feature = "test-hooks"))]
#[test]
fn iocp_batch_drains_valid_reads_alongside_a_short_read_and_reuses_slots() {
    use crate::test_hooks::{ConcurrencyPoint, pause_at};
    let fixture = Fixture::new();
    let file = open_windows_direct(&fixture.file()).unwrap();
    let geometry = alignment::query(&file).unwrap();
    let layout = fixed::PoolLayout::new(3, geometry);
    let mut driver = iocp::Reader::new(file, 3, geometry, layout).unwrap();
    let counters = IoCounters::default();
    let make_job = |offset: usize| {
        let (complete, receive) = mpsc::channel();
        let meter = Arc::new(IoMeter::default());
        (
            Some(fixed::Read::new(Job {
                offset: offset as u64,
                bytes: vec![0; 19],
                complete,
                meter: Some(meter.clone()),
            })),
            receive,
            meter,
        )
    };
    let offsets = [
        geometry.offset + 5,
        fixture.bytes.len() - 7,
        2 * geometry.offset - 5,
    ];
    let [
        (first, first_result, first_io),
        (short, short_result, short_io),
        (last, last_result, last_io),
    ] = offsets.map(make_job);
    let (repeat, repeat_result, repeat_io) = make_job(offsets[0]);
    let completed = pause_at(ConcurrencyPoint::QuiverColdIoCompleted);
    std::thread::scope(|scope| {
        let counters = &counters;
        let worker = scope.spawn(move || {
            driver
                .wave(&mut [first, short, last], counters, true)
                .unwrap();
            driver.wave(&mut [repeat], counters, false).unwrap();
        });
        completed.wait_until_arrived();
        let submitted = counters.calls.load(Ordering::Relaxed);
        completed.release();
        assert_eq!(submitted, 3);
        worker.join().unwrap();
    });
    assert_eq!(
        first_result.recv().unwrap().unwrap(),
        fixture.bytes[offsets[0]..offsets[0] + 19]
    );
    assert_eq!(
        short_result.recv().unwrap().unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
    assert_eq!(
        last_result.recv().unwrap().unwrap(),
        fixture.bytes[offsets[2]..offsets[2] + 19]
    );
    assert_eq!(
        repeat_result.recv().unwrap().unwrap(),
        fixture.bytes[offsets[0]..offsets[0] + 19]
    );
    assert_eq!(counters.calls.load(Ordering::Relaxed), 4);
    let grain = geometry.offset;
    for (meter, bytes) in [
        (first_io, grain),
        (short_io, fixture.bytes.len() % grain),
        (last_io, 2 * grain),
        (repeat_io, grain),
    ] {
        assert_eq!(meter.snapshot().physical_read_calls, 1);
        assert_eq!(meter.snapshot().physical_read_bytes, bytes as u64);
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn concurrent_disjoint_ranges_share_one_physical_page() {
    use crate::test_hooks::{ConcurrencyPoint, pause_at};
    let fixture = Fixture::new();
    let runtime = crate::ResourceRuntime::new(crate::ResourceLimits {
        max_threads: 3,
        ..Default::default()
    })
    .unwrap();
    let counters = Arc::new(IoCounters::default());
    let reader = AsyncReader::new(
        &fixture.file(),
        IoMode::Direct,
        2,
        &runtime,
        counters.clone(),
        |_| Ok(()),
    )
    .unwrap();
    #[cfg(windows)]
    {
        assert_eq!(runtime.stats().allocated_worker_threads, 1);
        assert!(reader.fixed_buffer_bytes > 0);
    }
    let start = reader.alignment.offset as u64;
    let completed = pause_at(ConcurrencyPoint::QuiverColdIoCompleted);
    let coalesced = pause_at(ConcurrencyPoint::QuiverColdPageCoalesced);
    let spans = [(start + 5, 11), (start + 28, 19)];
    std::thread::scope(|scope| {
        let read = |(offset, len)| {
            let meter = Arc::new(IoMeter::default());
            let bytes = reader
                .read_batch_during(vec![(offset, vec![0; len])], Some(&meter), || Ok(()))
                .unwrap();
            (bytes, meter.snapshot())
        };
        let owner = scope.spawn(move || read(spans[0]));
        completed.wait_until_arrived();
        let waiter = scope.spawn(move || read(spans[1]));
        coalesced.wait_until_arrived();
        completed.release();
        coalesced.release();
        let (owner_bytes, owner_io) = owner.join().unwrap();
        let (waiter_bytes, waiter_io) = waiter.join().unwrap();
        let (ready_bytes, ready_io) = read(spans[1]);
        for ((offset, len), bytes) in
            [spans[0], spans[1], spans[1]]
                .into_iter()
                .zip([owner_bytes, waiter_bytes, ready_bytes])
        {
            let offset = offset as usize;
            assert_eq!(bytes, [fixture.bytes[offset..offset + len].to_vec()]);
        }
        assert_eq!(
            owner_io,
            QuiverIoStats {
                physical_read_calls: 1,
                physical_read_bytes: reader.alignment.offset as u64,
                ..Default::default()
            }
        );
        assert_eq!(
            waiter_io,
            QuiverIoStats {
                window_reuse_hits: 1,
                window_reuse_bytes: 19,
                coalesced_page_reads: 1,
                ..Default::default()
            }
        );
        assert_eq!(
            ready_io,
            QuiverIoStats {
                window_reuse_hits: 1,
                window_reuse_bytes: 19,
                ..Default::default()
            }
        );
    });
    assert_eq!(counters.calls.load(Ordering::Relaxed), 1);
    assert_eq!(counters.window_hits.load(Ordering::Relaxed), 2);
    assert_eq!(counters.window_hit_bytes.load(Ordering::Relaxed), 38);
    drop(reader);
    assert_eq!(counters.window_bytes.load(Ordering::Relaxed), 0);
    assert_eq!(runtime.stats().live_worker_threads, 0);
}

#[cfg(feature = "test-hooks")]
#[test]
fn cancelled_page_owner_leaves_other_request_retryable() {
    use crate::test_hooks::{ConcurrencyPoint, pause_at};
    let fixture = Fixture::new();
    let runtime = crate::ResourceRuntime::new(crate::ResourceLimits {
        max_threads: 3,
        ..Default::default()
    })
    .unwrap();
    let counters = Arc::new(IoCounters::default());
    let reader = AsyncReader::new(
        &fixture.file(),
        IoMode::Direct,
        2,
        &runtime,
        counters.clone(),
        |_| Ok(()),
    )
    .unwrap();
    let offset = reader.alignment.offset as u64 + 7;
    let completed = pause_at(ConcurrencyPoint::QuiverColdIoCompleted);
    let coalesced = pause_at(ConcurrencyPoint::QuiverColdPageCoalesced);
    let control = crate::sdk::RequestContext::with_timeout(
        "ssd-page-owner",
        std::time::Duration::ZERO,
        false,
    )
    .unwrap();
    std::thread::scope(|scope| {
        let read = |at| {
            let meter = Arc::new(IoMeter::default());
            let result = reader.read_batch_during(vec![(at, vec![0; 13])], Some(&meter), || Ok(()));
            (result, meter.snapshot())
        };
        let context = &control;
        let owner = scope.spawn(move || context.run(|| Ok(read(offset))).unwrap());
        completed.wait_until_arrived();
        let waiter = scope.spawn(move || read(offset + 31));
        coalesced.wait_until_arrived();
        control.cancel();
        completed.release();
        coalesced.release();
        let (owner, owner_io) = owner.join().unwrap();
        assert_eq!(owner.unwrap_err().code(), "TDB_QUERY_CANCELLED");
        let (waiter, waiter_io) = waiter.join().unwrap();
        let start = offset as usize + 31;
        assert_eq!(waiter.unwrap(), [fixture.bytes[start..start + 13].to_vec()]);
        let expected = QuiverIoStats {
            physical_read_calls: 1,
            physical_read_bytes: reader.alignment.offset as u64,
            ..Default::default()
        };
        assert_eq!(owner_io, expected);
        assert_eq!(waiter_io, expected);
    });
    assert_eq!(counters.calls.load(Ordering::Relaxed), 2);
    drop(reader);
    assert_eq!(counters.window_bytes.load(Ordering::Relaxed), 0);
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "需要支持固定缓冲直读并允许 io_uring 的 Linux 环境"]
fn linux_ring_reads_cross_window_and_tail_ranges() {
    let fixture = Fixture::new();
    let runtime = crate::ResourceRuntime::new(crate::ResourceLimits {
        max_threads: 2,
        ..Default::default()
    })
    .unwrap();
    let counters = Arc::new(IoCounters::default());
    let reader = AsyncReader::new(
        &fixture.file(),
        IoMode::IoUring,
        8,
        &runtime,
        counters.clone(),
        |_| Ok(()),
    )
    .unwrap();
    let spans = [
        (WINDOW_LIMIT - 17, WINDOW_LIMIT + 61),
        (fixture.bytes.len() - 31, 31),
        (5, 23),
    ];
    let meter = Arc::new(IoMeter::default());
    let mut reads = reader
        .read_batch_during(
            spans
                .iter()
                .map(|&(offset, len)| (offset as u64, vec![0; len]))
                .collect(),
            Some(&meter),
            || Ok(()),
        )
        .unwrap()
        .into_iter();
    for (offset, len) in spans {
        assert_eq!(reads.next().unwrap(), fixture.bytes[offset..offset + len]);
    }
    assert_eq!(reads.next(), None);
    assert_eq!(meter.snapshot().physical_read_calls, 4);
    let grain = reader.alignment.offset;
    let full_bytes = if grain == WINDOW_LIMIT {
        3 * WINDOW_LIMIT
    } else {
        WINDOW_LIMIT + 3 * grain
    };
    assert_eq!(
        meter.snapshot().physical_read_bytes,
        (full_bytes + fixture.bytes.len() % grain) as u64
    );
    drop(reader);
    assert_eq!(runtime.stats().allocated_worker_threads, 0);
    assert_eq!(runtime.stats().live_worker_threads, 0);
    assert_eq!(counters.window_bytes.load(Ordering::Relaxed), 0);
}
