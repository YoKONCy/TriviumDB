use crate::VectorType;
use crate::error::{Result, TriviumError};
use std::path::{Path, PathBuf};

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
                return Err(TriviumError::Generic(format!(
                    "向量文件大小不匹配: 文件 {} 字节, 预期最少 {} 字节",
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
                        .map_err(|e| TriviumError::Io(e))?
                };

                pool.mmap = Some(mmap);
                pool.mmap_count = expected_count;
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
        if self.dim == 0 {
            0
        } else {
            self.delta.len() / self.dim
        }
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
            // mmap 基础层：COW 写入
            if let Some(ref mut mmap) = self.mmap {
                let elem_size = std::mem::size_of::<T>();
                let byte_offset = offset * elem_size;
                let byte_len = self.dim * elem_size;
                let slice = &mut mmap[byte_offset..byte_offset + byte_len];
                // 置零（T: Zeroable 由 VectorType 保证）
                for b in slice.iter_mut() {
                    *b = 0;
                }
            }
        } else {
            // 增量层
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
            // mmap 基础层：COW 写入
            if let Some(ref mut mmap) = self.mmap {
                let elem_size = std::mem::size_of::<T>();
                let byte_offset = offset * elem_size;
                let src_bytes = bytemuck::cast_slice(vector);
                mmap[byte_offset..byte_offset + src_bytes.len()].copy_from_slice(src_bytes);
            }
        } else {
            // 增量层
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
                if (ptr as usize) % std::mem::align_of::<T>() == 0 {
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
        }
    }

    /// 将基础层 + 增量层合并写入新的 .vec 文件
    ///
    /// 写入策略（原子安全）：
    ///   1. 写入 .vec.tmp 临时文件
    ///   2. fsync 确保落盘
    ///   3. rename 原子替换
    ///   4. 重新 mmap 映射新文件
    ///   5. 清空增量层
    pub fn flush(&mut self, vec_path: &Path) -> Result<usize> {
        let total = self.total_count();
        if total == 0 {
            // 无数据时删除旧文件
            if vec_path.exists() {
                std::fs::remove_file(vec_path)?;
            }
            self.mmap = None;
            self.mmap_count = 0;
            self.delta.clear();
            self.invalidate_cache();
            return Ok(0);
        }

        let tmp_path = vec_path.with_extension("vec.tmp");
        let elem_size = std::mem::size_of::<T>();

        // 1. 写入临时文件
        {
            let mut file = std::fs::File::create(&tmp_path)?;

            // 写入基础层向量（从 mmap COW 页读取，包含修改）
            if let Some(ref mmap) = self.mmap {
                let base_bytes = self.mmap_count * self.dim * elem_size;
                std::io::Write::write_all(&mut file, &mmap[..base_bytes])?;
            }

            // 写入增量层向量
            if !self.delta.is_empty() {
                let delta_bytes = bytemuck::cast_slice(&self.delta);
                std::io::Write::write_all(&mut file, delta_bytes)?;
            }

            // 2. fsync 落盘
            file.sync_all()?;
        }

        // 3. 原子替换
        std::fs::rename(&tmp_path, vec_path)?;

        // 4. 重新映射新文件
        let new_total = total;
        let file = std::fs::File::open(vec_path)?;
        let new_mmap = unsafe {
            memmap2::MmapOptions::new()
                .map_copy(&file)
                .map_err(|e| TriviumError::Io(e))?
        };
        self.mmap = Some(new_mmap);
        self.mmap_count = new_total;

        // 5. 清空增量层
        self.delta.clear();
        self.delta.shrink_to_fit(); // 释放增量内存

        self.vec_path = Some(vec_path.to_path_buf());
        self.invalidate_cache();

        Ok(new_total)
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

            if (ptr as usize) % std::mem::align_of::<T>() == 0 {
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
