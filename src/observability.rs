//! 查询、存储和 OS 资源观测的数据模型。
//!
//! 观测值用于诊断与 benchmark 报告，不参与正确性控制流。不可获得的平台指标必须明确
//! 标记 unsupported，不能用零值伪装成功；阶段名称和字段保持稳定以便跨语言消费。

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct ProcessMemorySnapshot {
    pub rss_bytes: u64,
    pub minor_faults: u64,
    pub major_faults: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct IndexMemoryStats {
    pub resident_heap_bytes: u64,
    pub mapped_bytes: u64,
    pub hot_bytes: u64,
    pub persisted_bytes: u64,
    pub posting_entries: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct PayloadMemoryStats {
    pub directory_bytes: usize,
    pub delta_raw_bytes: usize,
    pub parsed_cache_bytes: usize,
    pub parsed_cache_entries: usize,
    pub pinned_cache_entries: usize,
    pub mapped_file_bytes: usize,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_evictions: u64,
    pub payload_lookups: u64,
    pub payload_parsed_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct StorageWriteStats {
    pub wal_bytes: u64,
    pub sidecar_bytes: u64,
    pub checkpoint_bytes: u64,
    pub temporary_spill_bytes: u64,
    pub logical_bytes: u64,
}

impl StorageWriteStats {
    pub fn total_written_bytes(self) -> u64 {
        self.wal_bytes
            .saturating_add(self.sidecar_bytes)
            .saturating_add(self.checkpoint_bytes)
            .saturating_add(self.temporary_spill_bytes)
    }

    pub fn write_amplification(self) -> f64 {
        self.total_written_bytes() as f64 / self.logical_bytes.max(1) as f64
    }
}

pub fn process_memory_snapshot() -> Option<ProcessMemorySnapshot> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
        let end = stat.rfind(')')?;
        let fields: Vec<&str> = stat[end + 2..].split_whitespace().collect();
        let minor_faults = fields.get(7)?.parse().ok()?;
        let major_faults = fields.get(9)?.parse().ok()?;
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let rss_kib = status
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?;
        Some(ProcessMemorySnapshot {
            rss_bytes: rss_kib.saturating_mul(1024),
            minor_faults,
            major_faults,
        })
    }
    #[cfg(target_os = "windows")]
    {
        #[repr(C)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
        }
        #[link(name = "psapi")]
        unsafe extern "system" {
            fn GetProcessMemoryInfo(
                process: *mut std::ffi::c_void,
                counters: *mut ProcessMemoryCounters,
                size: u32,
            ) -> i32;
        }
        let mut counters = ProcessMemoryCounters {
            cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
            page_fault_count: 0,
            peak_working_set_size: 0,
            working_set_size: 0,
            quota_peak_paged_pool_usage: 0,
            quota_paged_pool_usage: 0,
            quota_peak_non_paged_pool_usage: 0,
            quota_non_paged_pool_usage: 0,
            pagefile_usage: 0,
            peak_pagefile_usage: 0,
        };
        // SAFETY: Windows API 仅在当前进程有效句柄和正确大小的可写结构体上调用。
        let success = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut counters,
                std::mem::size_of::<ProcessMemoryCounters>() as u32,
            )
        };
        (success != 0).then_some(ProcessMemorySnapshot {
            rss_bytes: counters.working_set_size as u64,
            minor_faults: counters.page_fault_count as u64,
            major_faults: 0,
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 进程内存快照在支持平台返回有效值() {
        #[cfg(target_os = "linux")]
        {
            let snapshot = process_memory_snapshot().unwrap();
            assert!(snapshot.rss_bytes > 0);
        }
        #[cfg(target_os = "windows")]
        assert!(process_memory_snapshot().is_some_and(|snapshot| snapshot.rss_bytes > 0));
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        assert!(process_memory_snapshot().is_none());
    }
}
