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
    backend.read(start as u64, &mut output, &counters).unwrap();
    assert_eq!(output, fixture.bytes[start..start + 31]);
    assert_eq!(counters.calls.load(Ordering::Relaxed), 1);
    assert_eq!(counters.bytes.load(Ordering::Relaxed), span as u64);
    backend
        .read((start + 3) as u64, &mut output[..17], &counters)
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
            .read((fixture.bytes.len() - 10) as u64, &mut output, &counters)
            .unwrap_err()
            .kind(),
        io::ErrorKind::UnexpectedEof
    );
    backend.read(start as u64, &mut output, &counters).unwrap();
    assert_eq!(output, fixture.bytes[start..start + 31]);
    assert_eq!(counters.calls.load(Ordering::Relaxed), 3);
    backend
        .read((fixture.bytes.len() - 31) as u64, &mut output, &counters)
        .unwrap();
    assert_eq!(output, fixture.bytes[fixture.bytes.len() - 31..]);
    let calls = counters.calls.load(Ordering::Relaxed);
    assert_eq!(
        backend
            .read(u64::MAX - 7, &mut output, &counters)
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
        backend.read(offset as u64, &mut output, &counters).unwrap();
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
    backend.read(0, &mut whole, &counters).unwrap();
    assert_eq!(whole, fixture.bytes[..WINDOW_LIMIT]);
    let mut part = [0u8; 31];
    let start = WINDOW_LIMIT * 2;
    backend.read(start as u64, &mut part, &counters).unwrap();
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
    old.read(13, &mut output, &counters).unwrap();
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
    new.read(13, &mut output, &counters).unwrap();
    assert_eq!(output, replacement[13..44]);
    old.read(13, &mut output, &counters).unwrap();
    assert_eq!(output, fixture.bytes[13..44]);
    assert_eq!(counters.calls.load(Ordering::Relaxed), 2);
    assert_eq!(counters.window_hits.load(Ordering::Relaxed), 1);
    drop((old, new));
    assert_eq!(counters.window_bytes.load(Ordering::Relaxed), 0);
}
