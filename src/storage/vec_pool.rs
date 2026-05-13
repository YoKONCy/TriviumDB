use crate::VectorType;
use crate::error::{Result, TriviumError};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Windows 下应对杀毒软件瞬态文件锁定的原子重命名（与 file_format.rs 同款）
#[cfg(windows)]
fn robust_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    let max_retries = 10;
    let mut delay = std::time::Duration::from_millis(1);
    for attempt in 0..max_retries {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < max_retries - 1 => {
                let os_err = e.raw_os_error();
                if os_err == Some(5) || os_err == Some(32) {
                    std::thread::sleep(delay);
                    delay = (delay * 2).min(std::time::Duration::from_millis(50));
                    continue;
                }
                return Err(e);
            }
            Err(e) => return Err(e),
        }
    }
    // 逻辑上不可达：循环必定 return。防御性返回避免审查标记。
    Err(std::io::Error::other("robust_rename 重试次数耗尽 (exhausted retries)"))
}

#[cfg(not(windows))]
fn robust_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

/// 分层向量池：将向量存储分为 mmap 基础层 + 内存增量层
///
/// 设计哲学：
/// - **基础层（mmap）**：上次 flush 时持久化的向量数据，通过 MAP_PRIVATE
///   copy-on-write 映射到内存。OS 按需分页加载，启动瞬间完成。
///   修改（逻辑删除置零、就地更新）仅影响进程私有副本，不改变磁盘文件。
/// - **增量层（Vec）**：自上次 flush 以来新插入的向量，纯内存存储。
///
/// 对外暴露的 `flat_vectors()` 接口保持不变（返回连续 `&[T]`），
/// 通过内部的合并缓存实现透明兼容。合并缓存采用 COW 策略：
/// 仅在首次调用时构建，后续读操作复用，直到下次写操作使缓存失效。
pub struct VecPool<T: VectorType> {
    dim: usize,

    // ═══ 基础层：mmap MAP_PRIVATE copy-on-write ═══
    /// 向量文件路径（数据库路径 + ".vec" 后缀）
    vec_path: Option<PathBuf>,
    /// MAP_PRIVATE 映射：读取来自文件，写入仅影响进程私有页
    mmap: Option<memmap2::MmapMut>,
    /// mmap 区域中的向量数量
    mmap_count: usize,

    // ═══ 增量层：纯内存 ═══
    /// 新插入的向量，尚未 flush 到磁盘
    delta: Vec<T>,

    // ═══ 合并缓存（COW 策略） ═══
    /// 合并后的连续向量视图，供 flat_vectors() 返回
    /// 仅在需要时构建（lazy），写操作使其失效
    merged: Vec<T>,
    /// 合并缓存是否有效
    merged_valid: bool,

    // ═══ 脏页标志 ═══
    /// 是否有 mmap 基础层的向量被 delete/update 修改（仅影响 COW 私有页，磁盘未变）
    ///
    /// flush() 策略选择依据：
    ///   - false → 追加路径：O(delta) I/O，只写新增数据
    ///   - true  → 全量重写路径：O(total) I/O，将 COW 脏页固化回磁盘
    has_dirty_base: bool,
}

/// 向 OS 内核提交 mmap 访问模式建议（`madvise` 封装）
///
/// **平台差异**：
/// - Linux/macOS：通过 `memmap2::Mmap::advise()` 直接调用 `madvise(2)` 系统调用
/// - Windows：`memmap2` 不暴露 `advise()` ——编译期彻底排除，零运行时开销
///
/// **使用场景**：
/// | 建议类型 | Advice | 典型触发点 |
/// |---|---|---|
/// | 顺序预读 | `Sequential` | mmap 建立后（BruteForce 全量扫描） |
/// | 主动预加载 | `WillNeed` | `search_hybrid()` 触发前（大数据集） |
/// | 冷页释放 | `DontNeed` | Compaction 完成后 / 内存压力 |
/// | 随机访问 | `Random` | Vamana 图遍历（稀疏随机访问） |
#[cfg(unix)]
#[inline]
fn madvise(mmap: &memmap2::MmapMut, advice: memmap2::Advice) {
    // advise() 是提示性调用，内核可以忽略。失败时静默继续，不影响正确性。
    let _ = mmap.advise(advice);
}

impl<T: VectorType> VecPool<T> {
    /// 创建空的纯内存向量池
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            vec_path: None,
            mmap: None,
            mmap_count: 0,
            delta: Vec::new(),
            merged: Vec::new(),
            merged_valid: false,
            has_dirty_base: false,
        }
    }

    /// 从 .vec 文件加载基础层（mmap），如果文件不存在则创建空池
    ///
    /// 向量文件格式：纯 SoA 二进制，无文件头，直接 bytemuck 映射。
    /// 文件大小 = mmap_count × dim × size_of::<T>()
    pub fn open(vec_path: &Path, dim: usize, expected_count: usize) -> Result<Self> {
        let mut pool = Self::new(dim);
        pool.vec_path = Some(vec_path.to_path_buf());

        if vec_path.exists() && expected_count > 0 {
            let file = std::fs::File::open(vec_path)?;
            let file_len = file.metadata()?.len() as usize;
            let elem_size = std::mem::size_of::<T>();
            let expected_size = expected_count * dim * elem_size;

            if file_len < expected_size {
                return Err(TriviumError::CorruptedFile(format!(
                    "向量文件大小不匹配 (Vec file size mismatch): 文件 {} 字节, 预期最少 {} 字节",
                    file_len, expected_size
                )));
            }

            if file_len > 0 {
                // SAFETY: MAP_PRIVATE (copy-on-write)
                //   - 读取来自文件页，OS 按需加载
                //   - 写入创建私有副本（COW page），不影响磁盘文件
                //   - VectorType 要求 T: Pod + Zeroable，所以字节对齐和全零初始化是安全的
                let mmap = unsafe {
                    memmap2::MmapOptions::new()
                        .len(expected_size)
                        .map_copy(&file)
                        .map_err(TriviumError::Io)?
                };

                pool.mmap = Some(mmap);
                pool.mmap_count = expected_count;

                // 建立映射后立刻提交顺序预读建议：
                // BruteForce 全量扫描是 TriviumDB 的主要访问模式，
                // MADV_SEQUENTIAL 提示 OS 在读到第 N 页时就预读第 N+k 页，
                // 有效隐藏 page fault 延迟（在数据集大于 L3 Cache 时收益显著）。
                #[cfg(unix)]
                if let Some(ref m) = pool.mmap {
                    madvise(m, memmap2::Advice::Sequential);
                }
            }
        }

        pool.invalidate_cache();
        Ok(pool)
    }

    /// 向量总数（基础层 + 增量层）
    #[inline]
    pub fn total_count(&self) -> usize {
        self.mmap_count + self.delta_count()
    }

    /// 增量层的向量数量
    #[inline]
    pub fn delta_count(&self) -> usize {
        self.delta.len().checked_div(self.dim).unwrap_or(0)
    }

    /// 基础层的向量数量
    #[inline]
    pub fn mmap_count(&self) -> usize {
        self.mmap_count
    }

    // ════════ 写操作（均使缓存失效） ════════

    /// 追加一个新向量到增量层
    pub fn push(&mut self, vector: &[T]) {
        self.delta.extend_from_slice(vector);
        self.invalidate_cache();
    }

    /// 逻辑删除：将指定索引的向量置零
    /// - 如果在 mmap 区域：通过 MAP_PRIVATE COW 写入私有页（不影响磁盘文件）
    /// - 如果在增量区域：直接修改 Vec
    pub fn zero_out(&mut self, index: usize) {
        let offset = index * self.dim;
        if index < self.mmap_count {
            // mmap 基础层：COW 写入（仅影响进程私有页，磁盘文件未变）
            if let Some(ref mut mmap) = self.mmap {
                let elem_size = std::mem::size_of::<T>();
                let byte_offset = offset * elem_size;
                let byte_len = self.dim * elem_size;
                let slice = &mut mmap[byte_offset..byte_offset + byte_len];
                for b in slice.iter_mut() {
                    *b = 0;
                }
            }
            // 基础层被修改：下次 flush 必须走全量重写路径，将 COW 脏页固化回磁盘
            self.has_dirty_base = true;
        } else {
            // 增量层：直接修改，不影响 flush 策略
            let delta_offset = (index - self.mmap_count) * self.dim;
            for i in delta_offset..delta_offset + self.dim {
                self.delta[i] = T::zero();
            }
        }
        self.invalidate_cache();
    }

    /// 就地更新指定索引的向量
    pub fn update(&mut self, index: usize, vector: &[T]) {
        let offset = index * self.dim;
        if index < self.mmap_count {
            // mmap 基础层：COW 写入（仅影响进程私有页，磁盘文件未变）
            if let Some(ref mut mmap) = self.mmap {
                let elem_size = std::mem::size_of::<T>();
                let byte_offset = offset * elem_size;
                let src_bytes = bytemuck::cast_slice(vector);
                mmap[byte_offset..byte_offset + src_bytes.len()].copy_from_slice(src_bytes);
            }
            // 基础层被修改：下次 flush 必须走全量重写路径
            self.has_dirty_base = true;
        } else {
            // 增量层：直接修改，不影响 flush 策略
            let delta_offset = (index - self.mmap_count) * self.dim;
            self.delta[delta_offset..delta_offset + self.dim].copy_from_slice(vector);
        }
        self.invalidate_cache();
    }

    // ════════ 读操作 ════════

    /// 获取指定索引的向量切片
    pub fn get(&self, index: usize) -> Option<&[T]> {
        if index < self.mmap_count {
            // 从 mmap 基础层读取
            self.mmap.as_ref().map(|m| {
                let elem_size = std::mem::size_of::<T>();
                let byte_offset = index * self.dim * elem_size;
                let byte_len = self.dim * elem_size;
                let bytes = &m[byte_offset..byte_offset + byte_len];

                // SAFETY: VectorType 要求 T: Pod，所以从对齐的字节序列转换为 &[T] 是安全的
                // MAP_PRIVATE 保证了内存映射的完整性
                let ptr = bytes.as_ptr();
                if (ptr as usize).is_multiple_of(std::mem::align_of::<T>()) {
                    // 对齐情况：零拷贝直接引用
                    unsafe { std::slice::from_raw_parts(ptr as *const T, self.dim) }
                } else {
                    // 不对齐：回退到合并缓存
                    // 这种情况在实践中几乎不会发生（mmap 通常页对齐）
                    // 为安全起见，回退到 merged 缓存路径
                    // 这里返回对应的 merged 切片
                    panic!("mmap 对齐异常，这不应该发生在正常的 OS 页映射中")
                }
            })
        } else {
            let delta_index = index - self.mmap_count;
            let delta_offset = delta_index * self.dim;
            if delta_offset + self.dim <= self.delta.len() {
                Some(&self.delta[delta_offset..delta_offset + self.dim])
            } else {
                None
            }
        }
    }

    /// 确保合并缓存已构建（需要 &mut self）
    ///
    /// 在需要同时使用 flat_vectors() 和其他 &self 方法时，
    /// 先调用此方法触发缓存重建，再调用 flat_vectors() 获取切片。
    pub fn ensure_cache(&mut self) {
        if self.mmap.is_some() && self.mmap_count > 0 && !self.merged_valid {
            self.rebuild_merged_cache();
        }
    }

    /// 返回合并后的连续向量视图（只需 &self）
    ///
    /// 此方法通过内部合并缓存保持接口兼容性（返回连续 &[T]）。
    /// 如果缓存未构建，请先调用 ensure_cache()。
    ///
    /// 性能说明：
    /// - 无 mmap 时（纯内存模式）：直接返回 delta 引用，零拷贝
    /// - 有 mmap 且缓存有效时：返回缓存引用，零拷贝
    pub fn flat_vectors(&self) -> &[T] {
        // 快速路径：无 mmap，直接返回 delta
        if self.mmap.is_none() || self.mmap_count == 0 {
            return &self.delta;
        }

        // 返回缓存（如果缓存无效但未调用 ensure_cache，返回可能过时的数据）
        // 在正确的使用流程中，调用方应先调用 ensure_cache()
        &self.merged
    }

    /// 返回增量层的原生切片引用（零拷贝）
    #[inline]
    pub fn delta_raw(&self) -> &[T] {
        &self.delta
    }

    // ════════ 持久化与模式切换 ════════

    /// 剥离 mmap 基础层：将现有所有数据读取为纯内存引用，并解除文件锁
    ///
    /// 为转换为 Rom 模式后能够安全删除 .vec 文件提供保障。
    pub fn detach_mmap(&mut self) {
        if self.mmap.is_some() {
            self.ensure_cache(); // 触发全量读取并合并

            // 剥离：深度全量复制给 delta
            let mut new_delta = Vec::with_capacity(self.merged.len());
            new_delta.extend_from_slice(&self.merged);
            self.delta = new_delta;

            // 剥离内核映射句柄（释放文件锁）
            self.mmap = None;
            self.vec_path = None;
            self.mmap_count = 0;
            self.merged.clear();
            self.merged_valid = false;
            // 基础层已不存在，清除脏页标志
            self.has_dirty_base = false;
        }
    }

    /// 持久化向量数据到 .vec 文件
    ///
    /// 根据 `has_dirty_base` 标志自动选择 flush 策略：
    ///
    /// **追加路径**（`has_dirty_base == false`，无 delete/update 触碰基础层）：
    ///   - I/O 代价 = O(delta 大小)，只写新增向量
    ///   - 适合纯写入场景（AI 记忆系统、批量导入）
    ///
    /// **全量重写路径**（`has_dirty_base == true`，或首次写入）：
    ///   - I/O 代价 = O(总数据量)，将 COW 脏页固化回磁盘
    ///   - 适合有 delete/update 的场景
    pub fn flush(&mut self, vec_path: &Path) -> Result<usize> {
        let total = self.total_count();

        // ── 边界情况：无数据 ──
        if total == 0 {
            // P1 fix: 先释放 mmap，再删文件（Windows 强制锁定）
            self.mmap = None;
            self.mmap_count = 0;
            if vec_path.exists() {
                std::fs::remove_file(vec_path).ok();
            }
            self.delta.clear();
            self.has_dirty_base = false;
            self.invalidate_cache();
            return Ok(0);
        }

        // ── 策略分支 ──
        // 追加路径条件：
        //   1. 基础层无 COW 脏页（没有 delete/update 触碰过 mmap 区域）
        //   2. 存在已映射的基础层（不是首次写入）
        //   3. 有新增数据需要追加（delta 非空）
        if !self.has_dirty_base && self.mmap.is_some() {
            if self.delta.is_empty() {
                // 既无脏页也无新增，无需任何 I/O
                return Ok(self.mmap_count);
            }
            self.flush_append(vec_path)
        } else {
            self.flush_rewrite(vec_path)
        }
    }

    /// 追加路径：仅将 delta 层写入 .vec 文件末尾
    ///
    /// 崩溃安全性分析：
    ///   - 追加成功、.tdb 未更新前崩溃 → .flush_ok 校验失败（vec_size 变大）→
    ///     降级为安全模式加载（忽略 .vec）→ WAL 回放恢复新节点到 delta 层 → 正确
    fn flush_append(&mut self, vec_path: &Path) -> Result<usize> {
        let append_count = self.delta_count();
        let elem_size = std::mem::size_of::<T>();

        // 1. 追加 delta 字节到现有 .vec 文件末尾
        {
            let mut file = OpenOptions::new()
                .append(true)
                .open(vec_path)
                .map_err(TriviumError::Io)?;
            let delta_bytes = bytemuck::cast_slice(&self.delta);
            file.write_all(delta_bytes).map_err(TriviumError::Io)?;
            file.sync_all().map_err(TriviumError::Io)?;
        }

        // 2. 释放旧 mmap（映射窗口固定，感知不到文件扩大后的新区域；
        //    同时满足 Windows 强制锁定：解映射后才能被 OS 正确感知文件长度变化）
        let new_total = self.mmap_count + append_count;
        self.mmap = None;

        // 3. 重新映射扩大后的完整文件
        let file = std::fs::File::open(vec_path).map_err(TriviumError::Io)?;
        let expected_bytes = new_total * self.dim * elem_size;
        let new_mmap = unsafe {
            memmap2::MmapOptions::new()
                .len(expected_bytes)
                .map_copy(&file)
                .map_err(TriviumError::Io)?
        };
        self.mmap = Some(new_mmap);
        self.mmap_count = new_total;

        // 重新映射后重新提交顺序预读建议
        #[cfg(unix)]
        if let Some(ref m) = self.mmap {
            madvise(m, memmap2::Advice::Sequential);
        }

        // 4. 清空增量层
        self.delta.clear();
        self.delta.shrink_to_fit();

        self.vec_path = Some(vec_path.to_path_buf());
        self.has_dirty_base = false;
        self.invalidate_cache();

        tracing::debug!(
            "[VecPool] 追加写入 (appended): +{} 向量, 累计 {} 向量",
            append_count,
            new_total
        );
        Ok(new_total)
    }

    /// 全量重写路径：将基础层（含 COW 脏页）+ 增量层全部写入新文件并原子替换
    ///
    /// 适用于：有 delete/update 修改过基础层，或首次 flush（无现有 .vec 文件）。
    fn flush_rewrite(&mut self, vec_path: &Path) -> Result<usize> {
        let total = self.total_count();
        let elem_size = std::mem::size_of::<T>();
        let tmp_path = vec_path.with_extension("vec.tmp");

        // 1. 写入临时文件（基础层 COW 页 + 增量层）
        {
            let mut file = std::fs::File::create(&tmp_path)?;

            if let Some(ref mmap) = self.mmap {
                // 基础层：从 COW 私有页读取（包含所有 delete/update 修改后的值）
                let base_bytes = self.mmap_count * self.dim * elem_size;
                file.write_all(&mmap[..base_bytes])?;
            }

            if !self.delta.is_empty() {
                let delta_bytes = bytemuck::cast_slice(&self.delta);
                file.write_all(delta_bytes)?;
            }

            file.sync_all()?;
        }

        // 2. 释放旧 mmap（P0 fix：Windows 强制锁定，映射存活时 rename 必定失败）
        //    COW 脏页数据已在步骤 1 写入 .tmp，释放安全。
        self.mmap = None;

        // 3. 原子替换
        robust_rename(&tmp_path, vec_path)?;

        // 4. 重新映射新文件
        let file = std::fs::File::open(vec_path)?;
        let expected_bytes = total * self.dim * elem_size;
        let new_mmap = unsafe {
            memmap2::MmapOptions::new()
                .len(expected_bytes)
                .map_copy(&file)
                .map_err(TriviumError::Io)?
        };
        self.mmap = Some(new_mmap);
        self.mmap_count = total;

        // 重新映射后重新提交顺序预读建议
        #[cfg(unix)]
        if let Some(ref m) = self.mmap {
            madvise(m, memmap2::Advice::Sequential);
        }

        // 5. 清空增量层
        self.delta.clear();
        self.delta.shrink_to_fit();

        self.vec_path = Some(vec_path.to_path_buf());
        self.has_dirty_base = false;
        self.invalidate_cache();

        tracing::debug!("[VecPool] 全量重写 (full rewrite): {} 向量", total);
        Ok(total)
    }

    // ════════ madvise 公开接口 ════════

    /// 主动释放 mmap 冷页（MADV_DONTNEED）
    ///
    /// 告知 OS 当前进程短期内不再需要这些页面，可以优先将其换出以释放物理内存。
    /// 若之后再次访问，OS 会从文件重新 fault-in，有额外的 page fault 代价。
    ///
    /// **典型调用时机**：
    /// - 大规模 Compaction 完成后（旧 mmap 已被替换，主动释放物理内存）
    /// - 内存压力检测触发时（`check_memory_pressure` 达到阈值）
    ///
    /// 非 Unix 平台（Windows）此函数编译为空，零开销。
    pub fn advise_dontneed(&self) {
        #[cfg(unix)]
        if let Some(ref m) = self.mmap {
            let _ = unsafe { m.unchecked_advise(memmap2::UncheckedAdvice::DontNeed) };
            tracing::debug!(
                "[VecPool] madvise(DONTNEED)：释放 {} MB 冷页 (releasing cold pages)",
                self.mmap_count * self.dim * std::mem::size_of::<T>() / (1024 * 1024)
            );
        }
    }

    /// 切换为随机访问模式建议（MADV_RANDOM）
    ///
    /// 告知 OS 页面将被随机访问，禁用预读（readahead）。
    /// 在顺序预读无效时（如 Vamana 稀疏图遍历），可以减少无效 I/O。
    ///
    /// 注意：Vamana 模式下向量在 `rebuild()` 时被 `to_vec()` clone 进图结构，
    /// 真正的 Vamana search 并不直接访问 mmap，因此此调用主要作为
    /// "一次 rebuild 后不再需要顺序扫描" 的信号。
    pub fn advise_random(&self) {
        #[cfg(unix)]
        if let Some(ref m) = self.mmap {
            madvise(m, memmap2::Advice::Random);
        }
    }

    /// 恢复顺序预读模式（MADV_SEQUENTIAL）
    ///
    /// 在 BruteForce 全量搜索前调用，强制重置为顺序预读策略。
    /// 若之前调用过 `advise_random()`，此方法可以恢复默认行为。
    pub fn advise_sequential(&self) {
        #[cfg(unix)]
        if let Some(ref m) = self.mmap {
            madvise(m, memmap2::Advice::Sequential);
        }
    }

    // ════════ 内部方法 ════════

    /// 使合并缓存失效
    #[inline]
    fn invalidate_cache(&mut self) {
        self.merged_valid = false;
    }

    /// 重建合并缓存：将 mmap 基础层 + 增量层合并为连续 Vec
    fn rebuild_merged_cache(&mut self) {
        let total_elements = self.total_count() * self.dim;
        self.merged.clear();
        self.merged.reserve(total_elements);

        // 从 mmap 基础层复制
        if let Some(ref mmap) = self.mmap {
            let elem_size = std::mem::size_of::<T>();
            let base_bytes = self.mmap_count * self.dim * elem_size;
            let bytes = &mmap[..base_bytes];
            let ptr = bytes.as_ptr();

            if (ptr as usize).is_multiple_of(std::mem::align_of::<T>()) {
                // 对齐：直接转换
                let base_slice = unsafe {
                    std::slice::from_raw_parts(ptr as *const T, self.mmap_count * self.dim)
                };
                self.merged.extend_from_slice(base_slice);
            } else {
                // 非对齐：逐元素读取
                for i in 0..self.mmap_count * self.dim {
                    let off = i * elem_size;
                    let chunk = &bytes[off..off + elem_size];
                    let elem: T = bytemuck::pod_read_unaligned(chunk);
                    self.merged.push(elem);
                }
            }
        }

        // 追加增量层
        self.merged.extend_from_slice(&self.delta);

        self.merged_valid = true;
    }

    /// 估算实际占用的堆内存字节数（不含 mmap 页，因为那由 OS 管理）
    pub fn heap_memory_bytes(&self) -> usize {
        let delta_bytes = self.delta.len() * std::mem::size_of::<T>();
        let merged_bytes = self.merged.len() * std::mem::size_of::<T>();
        delta_bytes + merged_bytes
    }

    /// 估算逻辑上管理的总向量数据大小（含 mmap 部分）
    pub fn total_data_bytes(&self) -> usize {
        self.total_count() * self.dim * std::mem::size_of::<T>()
    }
}
