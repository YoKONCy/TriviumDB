//! 查询级 Rayon 线程池缓存与并行预算。
//!
//! 每个并发度复用一个独立线程池，避免修改 Rayon 全局池并防止嵌套查询互相污染。
//! `max_threads=0` 表示自动，但始终受 64 线程硬上限约束；小输入保持串行。调用者
//! 负责在并行前完成内存/遍历预算检查，并在分片后执行确定性归并。

use crate::error::{Result, TriviumError};
use rayon::ThreadPool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

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

/// 按线程数复用查询线程池，避免每个算子重复创建工作线程。
pub(crate) fn query_pool(threads: usize) -> Result<Arc<ThreadPool>> {
    static POOLS: OnceLock<Mutex<HashMap<usize, Arc<ThreadPool>>>> = OnceLock::new();
    let pools = POOLS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut pools = pools
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(pool) = pools.get(&threads) {
        return Ok(Arc::clone(pool));
    }
    let pool = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads.clamp(1, 64))
            .thread_name(move |index| format!("triviumdb-query-{threads}-{index}"))
            .build()
            .map_err(|error| {
                TriviumError::QueryExecution(format!(
                    "创建查询并行线程池失败：{error} (Failed to create query parallel thread pool: {error})"
                ))
            })?,
    );
    pools.insert(threads, Arc::clone(&pool));
    Ok(pool)
}
