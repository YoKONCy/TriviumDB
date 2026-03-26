use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::Duration;

use crate::storage::file_format;
use crate::storage::memtable::MemTable;
use crate::storage::wal::Wal;

/// 后台 Compaction 守护线程
/// 定期将内存中的 MemTable 落盘为 .tdb 文件并清空 WAL，
/// 全程顺序写入，对 SSD 零磨损。
pub struct CompactionThread {
    handle: Option<thread::JoinHandle<()>>,
    stop_flag: Arc<AtomicBool>,
}

impl CompactionThread {
    /// 启动后台 Compaction 线程
    ///
    /// - `interval`: 两次 compaction 之间的间隔
    /// - `memtable`: 共享的 MemTable 引用（Arc<Mutex>）
    /// - `wal`: 共享的 WAL 引用
    /// - `db_path`: .tdb 文件路径
    pub fn spawn<T: crate::VectorType>(
        interval: Duration,
        memtable: Arc<Mutex<MemTable<T>>>,
        wal: Arc<Mutex<Wal>>,
        db_path: String,
    ) -> Self {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop = stop_flag.clone();

        let handle = thread::spawn(move || {
            loop {
                // 用短间隔轮询 stop_flag，而不是一次性 sleep 整个 interval，
                // 这样可以在 stop() 时快速响应退出。
                let mut elapsed = Duration::ZERO;
                let tick = Duration::from_millis(200);
                while elapsed < interval {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(tick);
                    elapsed += tick;
                }

                if stop.load(Ordering::Relaxed) {
                    return;
                }

                // 执行 Compaction：锁 -> 写 .tdb -> 清 WAL -> 释放锁
                let mt = memtable.lock().unwrap_or_else(|p| {
                    tracing::warn!("Compaction thread: MemTable Mutex poisoned, recovering...");
                    p.into_inner()
                });
                match file_format::save(&mt, &db_path) {
                    Ok(_) => {
                        drop(mt); // 先释放 memtable 锁
                        let mut w = wal.lock().unwrap_or_else(|p| {
                            tracing::warn!("Compaction thread: WAL Mutex poisoned, recovering...");
                            p.into_inner()
                        });
                        let _ = w.clear();
                        tracing::debug!("Auto-compaction completed for {}", db_path);
                    }
                    Err(e) => {
                        tracing::error!("Auto-compaction failed for {}: {}", db_path, e);
                    }
                }
            }
        });

        Self {
            handle: Some(handle),
            stop_flag,
        }
    }

    /// 优雅停止后台线程
    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CompactionThread {
    fn drop(&mut self) {
        self.stop();
    }
}
