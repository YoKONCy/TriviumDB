#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessMemorySnapshot {
    pub rss_bytes: u64,
    pub minor_faults: u64,
    pub major_faults: u64,
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
    #[cfg(not(target_os = "linux"))]
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
        #[cfg(not(target_os = "linux"))]
        assert!(process_memory_snapshot().is_none());
    }
}
