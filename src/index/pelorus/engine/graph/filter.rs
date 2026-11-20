//! 属性候选的索引槽位缓存。

use super::*;
pub(crate) const DEFAULT_PREFILTER_SLOT_CACHE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) type PrefilterSlotPosting = Arc<Vec<(u64, u32)>>;
pub(crate) struct PrefilterSlotCacheEntry {
    pub(crate) posting: PrefilterSlotPosting,
    pub(crate) bytes: usize,
    pub(crate) last_used: u64,
}
pub(crate) struct PrefilterSlotCacheState {
    pub(crate) entries: std::collections::HashMap<(u64, u64), PrefilterSlotCacheEntry>,
    pub(crate) bytes: usize,
    pub(crate) max_bytes: usize,
    pub(crate) clock: u64,
}
impl PrefilterSlotCacheState {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub(crate) fn get(&mut self, key: (u64, u64)) -> Option<PrefilterSlotPosting> {
        self.clock = self.clock.wrapping_add(1);
        let entry = self.entries.get_mut(&key)?;
        entry.last_used = self.clock;
        Some(Arc::clone(&entry.posting))
    }

    pub(crate) fn insert(&mut self, key: (u64, u64), posting: PrefilterSlotPosting) {
        let bytes = posting
            .len()
            .saturating_mul(std::mem::size_of::<(u64, u32)>());
        if bytes > self.max_bytes {
            return;
        }
        if let Some(previous) = self.entries.remove(&key) {
            self.bytes = self.bytes.saturating_sub(previous.bytes);
        }
        while self.bytes.saturating_add(bytes) > self.max_bytes {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(removed.bytes);
            }
        }
        self.clock = self.clock.wrapping_add(1);
        self.entries.insert(
            key,
            PrefilterSlotCacheEntry {
                posting,
                bytes,
                last_used: self.clock,
            },
        );
        self.bytes = self.bytes.saturating_add(bytes);
    }
}
impl Default for PrefilterSlotCacheState {
    fn default() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            bytes: 0,
            max_bytes: DEFAULT_PREFILTER_SLOT_CACHE_BYTES,
            clock: 0,
        }
    }
}
pub(crate) type PrefilterSlotCache = Mutex<PrefilterSlotCacheState>;
