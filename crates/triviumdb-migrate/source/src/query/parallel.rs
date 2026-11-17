//! 查询级 Rayon 线程池缓存与并行预算。
//!
//! 线程池由共享运行时统一管理，缓存池的总工作线程也必须服从运行时额度。
//! `max_threads=0` 表示自动，但始终受 64 线程硬上限约束；小输入保持串行。调用者
//! 负责在并行前完成内存/遍历预算检查，并在分片后执行确定性归并。

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryParallelismBudget {
    /// 0 表示自动使用当前机器可用并行度；硬上限为 64。
    pub max_threads: usize,
    /// 输入规模低于此值时保持串行，避免 Rayon 调度开销。
    pub min_parallel_rows: usize,
}

impl Default for QueryParallelismBudget {
    fn default() -> Self {
        Self {
            max_threads: 0,
            min_parallel_rows: 4_096,
        }
    }
}

impl QueryParallelismBudget {
    pub fn threads(self, rows: usize) -> usize {
        if rows < self.min_parallel_rows {
            return 1;
        }
        let available = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1);
        if self.max_threads == 0 {
            available.min(64)
        } else {
            self.max_threads.min(available).clamp(1, 64)
        }
    }
}

/// 查询池与扫描、构建和维护任务共用运行时线程额度。
pub(crate) fn query_pool(
    runtime: &crate::resources::ResourceRuntime,
    threads: usize,
) -> Result<crate::resources::ResourcePool> {
    runtime.pool(threads)
}
