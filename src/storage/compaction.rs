//! 后台压实调度器与生命周期控制。
//!
//! 调度线程只负责按阈值触发数据库提供的串行压实回调，不绕过 Writer 锁、WAL 或原子
//! 发布协议。停止信号和 join 保证关闭时无悬挂线程；ReadOnly/Immutable 不启动该服务。

use std::sync::{
    Arc, Mutex, RwLock,
    atomic::{AtomicBool, Ordering},
};
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
        memtable: Arc<RwLock<MemTable<T>>>,
        wal: Arc<Mutex<Wal>>,
        db_path: String,
        storage_mode: crate::database::StorageMode,
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

                // 1. 取出短命锁（Short-lived Lock），提取构建所需的内存副本快照
                {
                    let mut mt = memtable.write().unwrap_or_else(|p| {
                        tracing::warn!("Compaction 线程: MemTable 互斥锁中毒，正在恢复 (MemTable Mutex poisoned, recovering)");
                        p.into_inner()
                    });
                    // 后台压实只准备持久化需要的 BQ，不触发 ANN 或 merged 全量物化。
                    mt.prepare_persistence_cache(false);
                } // 👑👑👑 锁在此刻被丢弃，前台彻底解放！

                // 2. 长时间无锁计算区（原旧版索引用，已废除，这里留出空白阶段）

                // 3. 次级落盘锁阶段（用于写文件和热插拔指针）
                let mut mt = memtable.write().unwrap_or_else(|p| {
                    tracing::warn!("Compaction 线程：MemTable 互斥锁中毒，正在恢复 (Compaction thread: MemTable mutex poisoned, recovering)");
                    p.into_inner()
                });
                tracing::info!(
                    "Compaction I/O 开始，前台查询将被阻塞 (Compaction I/O started, foreground blocked): {}",
                    db_path.clone()
                );
                #[cfg(feature = "test-hooks")]
                crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeCompactionSave);

                match file_format::save(&mut mt, &db_path, storage_mode) {
                    Ok(_) => {
                        // 💀 绝对不能在这里先 `drop(mt)`！
                        // 必须在此之前拿到 WAL 锁，然后一起释放，防止前台乘虚而入写入 WAL 然后被下面 clear!
                        let mut w = wal.lock().unwrap_or_else(|p| {
                            tracing::warn!("Compaction 线程: WAL 互斥锁中毒，正在恢复 (WAL Mutex poisoned, recovering)");
                            p.into_inner()
                        });
                        #[cfg(feature = "test-hooks")]
                        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeWalClear);
                        let _ = w.clear();

                        drop(w); // 优先释放 WAL 写锁
                        drop(mt); // 其次释放 内存大锁
                        tracing::debug!("自动压实完成 (Auto-compaction completed): {}", db_path);
                    }
                    Err(e) => {
                        tracing::error!(
                            "自动压实失败 (Auto-compaction failed): {}: {}",
                            db_path,
                            e
                        );
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
