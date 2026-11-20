//! 对齐签名存储及持久化布局。

use super::*;
// ═════════════════════════════════════════════════════════════
//  64B 缓存行对齐的紧凑型 BQ2 签名存储（默认单块交错布局，支持消融切换）
// ═════════════════════════════════════════════════════════════

/// 严格对齐至 64 字节（单条 x86_64/ARM64 CPU 缓存行）的 8×u64 块。
#[repr(C, align(64))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CacheLineU64(pub(crate) [u64; 8]);
impl Default for CacheLineU64 {
    #[inline]
    fn default() -> Self {
        Self([0u64; 8])
    }
}
/// 紧凑型 BQ2 签名存储。
///
/// 默认将每个节点的 `[pos[0..chunks], strong[0..chunks]]` 连续紧邻排布在 64 字节对齐的
/// 单一平坦缓冲区中（对 768D 即 `24 × u64 = 192B = 3 × 64B` 整缓存行，零尾部跨行分裂、单页单 TLB）。
/// 在 `research` 特性下支持原地切换回传统的双数组分离布局（SoA），用于同图严格测量内存布局收益。
pub struct Bq2Store {
    /// 64B 对齐的主连续交错缓冲区：节点 `i` 占据 `[i * 2 * chunks .. (i + 1) * 2 * chunks]`
    pub(crate) interleaved: Vec<CacheLineU64>,
    /// 仅在消融实验关闭连续布局时启用，确保任意时刻仅驻留单份签名内存
    #[cfg(feature = "research")]
    pub(crate) legacy_soa: Option<(Vec<u64>, Vec<u64>)>,
    /// 回收分离数组后共享合表中的签名，保持只读接口和节点编号有效。
    pub(crate) colocated_store:
        Option<std::sync::Arc<crate::index::pelorus::engine::graph::HotSmStore>>,
    /// 每个签名的 u64 chunk 数量（= dim.div_ceil(64)）
    pub(crate) chunks: usize,
    /// 已存储的签名数量
    pub(crate) n: usize,
}
impl Bq2Store {
    /// 创建空的紧凑存储（默认启用 64B 缓存行对齐的连续节点布局）
    pub fn new(dim: usize) -> Self {
        let chunks = if dim == 0 { 1 } else { dim.div_ceil(64) };
        Self {
            interleaved: Vec::new(),
            #[cfg(feature = "research")]
            legacy_soa: None,
            colocated_store: None,
            chunks,
            n: 0,
        }
    }

    #[inline]
    pub fn chunks(&self) -> usize {
        self.chunks
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.n
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    #[inline]
    pub(crate) fn interleaved_u64_slice(&self) -> &[u64] {
        let total_u64 = self.interleaved.len() * 8;
        unsafe { std::slice::from_raw_parts(self.interleaved.as_ptr() as *const u64, total_u64) }
    }

    #[inline]
    pub(crate) fn interleaved_u64_slice_mut(&mut self) -> &mut [u64] {
        self.restore_interleaved();
        let total_u64 = self.interleaved.len() * 8;
        unsafe {
            std::slice::from_raw_parts_mut(self.interleaved.as_mut_ptr() as *mut u64, total_u64)
        }
    }

    #[inline]
    pub(crate) fn ensure_interleaved_words(&mut self, required_words: usize) {
        self.restore_interleaved();
        let req_lines = required_words.div_ceil(8);
        if self.interleaved.len() < req_lines {
            self.interleaved.resize(req_lines, CacheLineU64::default());
        }
    }

    /// 获取第 `idx` 个节点的 `(pos_ptr, strong_ptr)` 裸指针（零拷贝）。
    #[inline(always)]
    pub(crate) fn node_ptrs(&self, idx: usize) -> (*const u64, *const u64) {
        debug_assert!(idx < self.n);
        if let Some(store) = &self.colocated_store {
            let node = store.get(idx);
            return (node.pos.as_ptr(), node.strong.as_ptr());
        }
        #[cfg(feature = "research")]
        if let Some((pos, strong)) = &self.legacy_soa {
            let off = idx * self.chunks;
            return unsafe { (pos.as_ptr().add(off), strong.as_ptr().add(off)) };
        }
        let stride = self.chunks * 2;
        let base = unsafe { (self.interleaved.as_ptr() as *const u64).add(idx * stride) };
        (base, unsafe { base.add(self.chunks) })
    }

    /// 在消融实验中原地切换 `64B` 对齐单块交错布局（`true`）与传统分离双数组布局（`false`）。
    /// 切换时先释放旧缓冲再建立新缓冲（或搬迁后立即释放旧缓冲），保持常驻内存单份精确计量。
    #[cfg(feature = "research")]
    pub(crate) fn set_contiguous_layout(&mut self, contiguous: bool) {
        self.restore_interleaved();
        if contiguous {
            if let Some((pos, strong)) = self.legacy_soa.take() {
                let stride = self.chunks * 2;
                let total_words = self.n * stride;
                let mut lines = vec![CacheLineU64::default(); total_words.div_ceil(8)];
                let dst = unsafe {
                    std::slice::from_raw_parts_mut(lines.as_mut_ptr() as *mut u64, lines.len() * 8)
                };
                for i in 0..self.n {
                    let src_off = i * self.chunks;
                    let dst_off = i * stride;
                    dst[dst_off..dst_off + self.chunks]
                        .copy_from_slice(&pos[src_off..src_off + self.chunks]);
                    dst[dst_off + self.chunks..dst_off + stride]
                        .copy_from_slice(&strong[src_off..src_off + self.chunks]);
                }
                drop(pos);
                drop(strong);
                self.interleaved = lines;
            }
        } else if self.legacy_soa.is_none() {
            let mut pos = vec![0u64; self.n * self.chunks];
            let mut strong = vec![0u64; self.n * self.chunks];
            let stride = self.chunks * 2;
            let src = self.interleaved_u64_slice();
            for i in 0..self.n {
                let src_off = i * stride;
                let dst_off = i * self.chunks;
                pos[dst_off..dst_off + self.chunks]
                    .copy_from_slice(&src[src_off..src_off + self.chunks]);
                strong[dst_off..dst_off + self.chunks]
                    .copy_from_slice(&src[src_off + self.chunks..src_off + stride]);
            }
            self.interleaved = Vec::new();
            self.interleaved.shrink_to_fit();
            self.legacy_soa = Some((pos, strong));
        }
    }

    /// 预分配空间
    pub fn reserve(&mut self, additional: usize) {
        self.restore_interleaved();
        #[cfg(feature = "research")]
        if let Some((pos, strong)) = &mut self.legacy_soa {
            pos.reserve(additional * self.chunks);
            strong.reserve(additional * self.chunks);
            return;
        }
        let total_words = (self.n + additional) * self.chunks * 2;
        let total_lines = total_words.div_ceil(8);
        if total_lines > self.interleaved.len() {
            self.interleaved
                .reserve(total_lines - self.interleaved.len());
        }
    }

    /// 并行从连续 f32 向量切片一次性构建 64B 对齐紧凑签名存储（零中间 Vec 分配，结果与顺序追加逐位一致）。
    pub(crate) fn from_vectors_parallel(vectors: &[f32], dim: usize) -> Self {
        use rayon::prelude::*;
        let chunks = if dim == 0 { 1 } else { dim.div_ceil(64) };
        if dim == 0 || vectors.is_empty() {
            return Self::new(dim);
        }
        assert!(vectors.len().is_multiple_of(dim));
        let n = vectors.len() / dim;
        let stride = chunks * 2;
        let total_words = n * stride;
        let mut interleaved = vec![CacheLineU64::default(); total_words.div_ceil(8)];

        let encode_one = |i: usize, node_dst: &mut [u64]| {
            let (p_dst, s_dst) = node_dst.split_at_mut(chunks);
            let vec = &vectors[i * dim..(i + 1) * dim];
            let mut sum_abs = 0.0f32;
            for &v in vec {
                sum_abs += v.abs();
            }
            let alpha = sum_abs / dim as f32;
            let full_chunks = dim / 64;
            for c in 0..full_chunks {
                let base = c * 64;
                let mut cp = 0u64;
                let mut cs = 0u64;
                for j in 0..64 {
                    let val = vec[base + j];
                    if val > 0.0 {
                        cp |= 1u64 << j;
                    }
                    if val.abs() > alpha {
                        cs |= 1u64 << j;
                    }
                }
                p_dst[c] = cp;
                s_dst[c] = cs;
            }
            let rem_base = full_chunks * 64;
            if rem_base < dim {
                let mut cp = 0u64;
                let mut cs = 0u64;
                for j in 0..(dim - rem_base) {
                    let val = vec[rem_base + j];
                    if val > 0.0 {
                        cp |= 1u64 << j;
                    }
                    if val.abs() > alpha {
                        cs |= 1u64 << j;
                    }
                }
                p_dst[full_chunks] = cp;
                s_dst[full_chunks] = cs;
            }
        };

        let flat_words = unsafe {
            std::slice::from_raw_parts_mut(interleaved.as_mut_ptr() as *mut u64, total_words)
        };
        if n >= 2048 {
            flat_words
                .par_chunks_mut(stride)
                .enumerate()
                .for_each(|(i, node_dst)| encode_one(i, node_dst));
        } else {
            for (i, node_dst) in flat_words.chunks_exact_mut(stride).enumerate() {
                encode_one(i, node_dst);
            }
        }

        Self {
            interleaved,
            #[cfg(feature = "research")]
            legacy_soa: None,
            colocated_store: None,
            chunks,
            n,
        }
    }

    /// 从向量直接编码并追加（紧凑路径）
    pub fn push_from_vector<T: VectorType>(&mut self, vec: &[T]) {
        let mut sum_abs = 0.0f32;
        for v in vec {
            sum_abs += v.to_f32().abs();
        }
        let alpha = if vec.is_empty() {
            0.0
        } else {
            sum_abs / vec.len() as f32
        };

        #[cfg(feature = "research")]
        if let Some((pos, strong)) = &mut self.legacy_soa {
            for i in 0..self.chunks {
                let mut cp = 0u64;
                let mut cs = 0u64;
                for j in 0..64 {
                    let idx = i * 64 + j;
                    if idx < vec.len() {
                        let val = vec[idx].to_f32();
                        if val > 0.0 {
                            cp |= 1u64 << j;
                        }
                        if val.abs() > alpha {
                            cs |= 1u64 << j;
                        }
                    }
                }
                pos.push(cp);
                strong.push(cs);
            }
            self.n += 1;
            return;
        }

        let stride = self.chunks * 2;
        let base_word = self.n * stride;
        self.ensure_interleaved_words(base_word + stride);
        let chunks = self.chunks;
        let dst = &mut self.interleaved_u64_slice_mut()[base_word..base_word + stride];
        for i in 0..chunks {
            let mut cp = 0u64;
            let mut cs = 0u64;
            for j in 0..64 {
                let idx = i * 64 + j;
                if idx < vec.len() {
                    let val = vec[idx].to_f32();
                    if val > 0.0 {
                        cp |= 1u64 << j;
                    }
                    if val.abs() > alpha {
                        cs |= 1u64 << j;
                    }
                }
            }
            dst[i] = cp;
            dst[chunks + i] = cs;
        }
        self.n += 1;
    }

    /// 追加已有的 Bq2Signature（兼容路径）
    pub fn push_sig(&mut self, sig: &Bq2Signature) {
        #[cfg(feature = "research")]
        if let Some((pos, strong)) = &mut self.legacy_soa {
            pos.extend_from_slice(&sig.pos[..self.chunks]);
            strong.extend_from_slice(&sig.strong[..self.chunks]);
            self.n += 1;
            return;
        }

        let stride = self.chunks * 2;
        let base_word = self.n * stride;
        self.ensure_interleaved_words(base_word + stride);
        let chunks = self.chunks;
        let dst = &mut self.interleaved_u64_slice_mut()[base_word..base_word + stride];
        dst[..chunks].copy_from_slice(&sig.pos[..chunks]);
        dst[chunks..stride].copy_from_slice(&sig.strong[..chunks]);
        self.n += 1;
    }

    /// 原地覆写第 `idx` 个节点的 `(pos, strong)` 切片（供构图完成后将切锥正交残差签名写回常驻存储）。
    #[inline]
    pub(crate) fn set_node_slices(&mut self, idx: usize, pos_src: &[u64], strong_src: &[u64]) {
        assert!(idx < self.n, "节点下标越界");
        let chunks = self.chunks;
        self.restore_interleaved();
        #[cfg(feature = "research")]
        if let Some((pos, strong)) = &mut self.legacy_soa {
            let off = idx * chunks;
            pos[off..off + chunks].copy_from_slice(&pos_src[..chunks]);
            strong[off..off + chunks].copy_from_slice(&strong_src[..chunks]);
            return;
        }
        let stride = chunks * 2;
        let base_word = idx * stride;
        let dst = &mut self.interleaved_u64_slice_mut()[base_word..base_word + stride];
        dst[..chunks].copy_from_slice(&pos_src[..chunks]);
        dst[chunks..stride].copy_from_slice(&strong_src[..chunks]);
    }

    /// 重建完整的动态 BQ2 签名
    #[inline]
    pub fn get_sig(&self, idx: usize) -> Bq2Signature {
        Bq2Signature {
            pos: self.pos_slice(idx).to_vec(),
            strong: self.strong_slice(idx).to_vec(),
        }
    }

    /// 传统双指针预取（保留供消融基线复现与通用调用）
    #[inline]
    pub fn prefetch_sig(&self, idx: usize) {
        let (pos_u64, strong_u64) = self.node_ptrs(idx);
        unsafe {
            #[cfg(target_arch = "x86_64")]
            {
                use std::arch::x86_64::*;
                let pos_ptr = pos_u64 as *const i8;
                let strong_ptr = strong_u64 as *const i8;
                _mm_prefetch(pos_ptr, _MM_HINT_T0);
                _mm_prefetch(strong_ptr, _MM_HINT_T0);
                if self.chunks > 8 {
                    _mm_prefetch(pos_ptr.add(64), _MM_HINT_T0);
                    _mm_prefetch(strong_ptr.add(64), _MM_HINT_T0);
                }
            }
            #[cfg(target_arch = "aarch64")]
            {
                std::arch::asm!("prfm pldl1keep, [{0}]", in(reg) pos_u64, options(nostack, preserves_flags));
                std::arch::asm!("prfm pldl1keep, [{0}]", in(reg) strong_u64, options(nostack, preserves_flags));
            }
        }
    }

    /// 按缓存行精确预取单个节点的完整签名块（对 768D 64B 对齐交错布局恰好预取连续 3 条缓存行 = 192B，零冗余指令）。
    #[inline]
    pub(crate) fn prefetch_node_exact(&self, idx: usize) {
        #[cfg(feature = "research")]
        if self.legacy_soa.is_some() {
            self.prefetch_sig(idx);
            return;
        }
        let stride_bytes = self.chunks * 16;
        let (pos, _) = self.node_ptrs(idx);
        let base_ptr = pos as *const i8;
        unsafe {
            #[cfg(target_arch = "x86_64")]
            {
                use std::arch::x86_64::*;
                _mm_prefetch(base_ptr, _MM_HINT_T0);
                if stride_bytes > 64 {
                    _mm_prefetch(base_ptr.add(64), _MM_HINT_T0);
                }
                if stride_bytes > 128 {
                    _mm_prefetch(base_ptr.add(128), _MM_HINT_T0);
                }
                if stride_bytes > 192 {
                    _mm_prefetch(base_ptr.add(192), _MM_HINT_T0);
                }
            }
            #[cfg(target_arch = "aarch64")]
            {
                std::arch::asm!("prfm pldl1keep, [{0}]", in(reg) base_ptr, options(nostack, preserves_flags));
                if stride_bytes > 64 {
                    let p1 = base_ptr.add(64);
                    std::arch::asm!("prfm pldl1keep, [{0}]", in(reg) p1, options(nostack, preserves_flags));
                }
                if stride_bytes > 128 {
                    let p2 = base_ptr.add(128);
                    std::arch::asm!("prfm pldl1keep, [{0}]", in(reg) p2, options(nostack, preserves_flags));
                }
            }
        }
    }

    /// 计算存储中第 idx 个签名与外部签名的距离（零拷贝）
    #[inline]
    pub fn distance_to_sig(&self, idx: usize, other: &Bq2Signature, dim: usize) -> u32 {
        let (pa, sa) = self.node_ptrs(idx);
        bq2_distance_raw(pa, sa, other.pos.as_ptr(), other.strong.as_ptr(), dim)
    }

    #[inline]
    pub fn distance_to_sig_cheap(&self, idx: usize, other: &Bq2Signature, dim: usize) -> u32 {
        #[cfg(target_arch = "x86_64")]
        {
            let (pa, sa) = self.node_ptrs(idx);
            match *X86_BIT_KERNEL {
                #[cfg(not(coverage))]
                X86BitKernel::Avx512Vpopcnt => {
                    return unsafe {
                        if dim == 768 {
                            bq2_distance_cheap_avx512_768(
                                pa,
                                sa,
                                other.pos.as_ptr(),
                                other.strong.as_ptr(),
                            )
                        } else {
                            bq2_distance_cheap_avx512(
                                pa,
                                sa,
                                other.pos.as_ptr(),
                                other.strong.as_ptr(),
                                dim,
                            )
                        }
                    };
                }
                X86BitKernel::Avx2Popcnt | X86BitKernel::Popcnt => {
                    return unsafe {
                        bq2_distance_cheap_popcnt_any(
                            pa,
                            sa,
                            other.pos.as_ptr(),
                            other.strong.as_ptr(),
                            dim.div_ceil(64),
                        )
                    };
                }
                X86BitKernel::Avx2Only | X86BitKernel::Scalar => {}
            }
        }
        let pos_s = self.pos_slice(idx);
        let strong_s = self.strong_slice(idx);
        let chunks = dim.div_ceil(64);
        let mut acc = 0u32;
        for i in 0..chunks {
            acc += (pos_s[i] ^ other.pos[i]).count_ones();
            acc += (strong_s[i] ^ other.strong[i]).count_ones();
        }
        acc
    }

    #[inline]
    pub(crate) fn distance_to_sig_cheap_384(&self, idx: usize, other: &Bq2Signature) -> u32 {
        #[cfg(target_arch = "x86_64")]
        {
            let (pa, sa) = self.node_ptrs(idx);
            unsafe {
                bq2_distance_cheap_popcnt_384(pa, sa, other.pos.as_ptr(), other.strong.as_ptr())
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            self.distance_to_sig_cheap(idx, other, 384)
        }
    }

    /// 计算两个存储中签名的距离（零拷贝）
    #[inline]
    pub fn distance(&self, i: usize, j: usize, dim: usize) -> u32 {
        let (pi, si) = self.node_ptrs(i);
        let (pj, sj) = self.node_ptrs(j);
        bq2_distance_raw(pi, si, pj, sj, dim)
    }

    /// 跨两个紧凑签名存储直接计算 2-bit 距离（零拷贝、无堆分配，用于极坐标锥路由地标匹配）
    #[inline]
    pub fn distance_between_stores(
        &self,
        idx: usize,
        other: &Self,
        other_idx: usize,
        dim: usize,
    ) -> u32 {
        debug_assert_eq!(self.chunks, other.chunks);
        let (pa, sa) = self.node_ptrs(idx);
        let (pb, sb) = other.node_ptrs(other_idx);
        bq2_distance_raw(pa, sa, pb, sb, dim)
    }

    /// 计算存储中第 idx 个签名与外部签名的 5-popcount 两级符号-幅值（SM）重构位计数。
    #[inline]
    pub(crate) fn sm_counts_to_sig(
        &self,
        idx: usize,
        other: &Bq2Signature,
        dim: usize,
    ) -> [u32; 5] {
        let (pa, sa) = self.node_ptrs(idx);
        sm_dot_counts_raw(pa, sa, other.pos.as_ptr(), other.strong.as_ptr(), dim)
    }

    /// 计算同一存储中两个签名的 5-popcount 两级符号-幅值（SM）重构位计数。
    #[inline]
    pub(crate) fn sm_counts(&self, i: usize, j: usize, dim: usize) -> [u32; 5] {
        let (pi, si) = self.node_ptrs(i);
        let (pj, sj) = self.node_ptrs(j);
        sm_dot_counts_raw(pi, si, pj, sj, dim)
    }

    /// 跨两个紧凑签名存储计算 5-popcount 两级符号-幅值（SM）重构位计数。
    #[inline]
    pub(crate) fn sm_counts_between_stores(
        &self,
        idx: usize,
        other: &Self,
        other_idx: usize,
        dim: usize,
    ) -> [u32; 5] {
        debug_assert_eq!(self.chunks, other.chunks);
        let (pa, sa) = self.node_ptrs(idx);
        let (pb, sb) = other.node_ptrs(other_idx);
        sm_dot_counts_raw(pa, sa, pb, sb, dim)
    }

    /// 获取第 idx 个签名的 pos 符号位切片（零拷贝，用于 Stage-0.5 非对称 SM 守门评估）。
    #[inline]
    pub(crate) fn pos_slice(&self, idx: usize) -> &[u64] {
        let (pa, _) = self.node_ptrs(idx);
        unsafe { std::slice::from_raw_parts(pa, self.chunks) }
    }

    /// 获取第 idx 个签名的 strong 位切片（零拷贝，用于无分配计算 SM 幅值均值）。
    #[inline]
    pub(crate) fn strong_slice(&self, idx: usize) -> &[u64] {
        let (_, sa) = self.node_ptrs(idx);
        unsafe { std::slice::from_raw_parts(sa, self.chunks) }
    }

    /// 统计第 idx 个签名中幅值强位（strong）的置位数
    #[inline]
    pub fn strong_ones(&self, idx: usize) -> u32 {
        slice_popcnt_u64(self.strong_slice(idx))
    }

    /// 紧凑常驻热内存占用（字节，含对齐行在内如实计量）
    pub fn hot_bytes(&self) -> usize {
        #[cfg(feature = "research")]
        if let Some((pos, strong)) = &self.legacy_soa {
            return (pos.len() + strong.len()) * 8;
        }
        self.interleaved.len() * std::mem::size_of::<CacheLineU64>()
    }

    /// 由共享合表接管签名读取，释放分离缓冲并保留逻辑节点数量。
    pub(crate) fn reclaim_for_colocated(
        &mut self,
        store: std::sync::Arc<crate::index::pelorus::engine::graph::HotSmStore>,
    ) {
        assert_eq!(self.chunks, store.chunks(), "合表签名维度必须一致");
        assert_eq!(self.n, store.len(), "合表与签名存储的节点数必须一致");
        self.colocated_store = Some(store);
        self.interleaved = Vec::new();
        #[cfg(feature = "research")]
        {
            self.legacy_soa = None;
        }
    }

    /// 首次写入或切换布局前恢复可变签名，完成复制后再释放共享合表。
    pub(crate) fn restore_interleaved(&mut self) {
        let Some(store) = &self.colocated_store else {
            return;
        };
        let stride = self.chunks * 2;
        let total_words = self.n * stride;
        let mut lines = vec![CacheLineU64::default(); total_words.div_ceil(8)];
        // 每个缓存行恰好包含八个 u64，缓冲已完整初始化且覆盖全部签名。
        let words = unsafe {
            std::slice::from_raw_parts_mut(lines.as_mut_ptr().cast::<u64>(), total_words)
        };
        for (idx, dst) in words.chunks_exact_mut(stride).take(self.n).enumerate() {
            let node = store.get(idx);
            dst[..self.chunks].copy_from_slice(node.pos);
            dst[self.chunks..].copy_from_slice(node.strong);
        }
        self.interleaved = lines;
        self.colocated_store = None;
    }

    /// 按磁盘契约流式写出平面化的全部 `pos` 与全部 `strong` 数组（零额外堆分配，100% 兼容旧版文件格式）。
    pub fn write_planar_to<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        #[cfg(feature = "research")]
        if let Some((pos, strong)) = &self.legacy_soa {
            w.write_all(bytemuck::cast_slice(pos))?;
            w.write_all(bytemuck::cast_slice(strong))?;
            return Ok(());
        }
        for i in 0..self.n {
            w.write_all(bytemuck::cast_slice(self.pos_slice(i)))?;
        }
        for i in 0..self.n {
            w.write_all(bytemuck::cast_slice(self.strong_slice(i)))?;
        }
        Ok(())
    }

    /// 导出平面化的 `pos` 数组（供测试校验或外部检查）
    pub fn collect_pos_data(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.n * self.chunks);
        for i in 0..self.n {
            out.extend_from_slice(self.pos_slice(i));
        }
        out
    }

    /// 导出平面化的 `strong` 数组（供测试校验或外部检查）
    pub fn collect_strong_data(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.n * self.chunks);
        for i in 0..self.n {
            out.extend_from_slice(self.strong_slice(i));
        }
        out
    }

    /// 从平面化裸数据恢复（反序列化用）
    pub fn from_raw(pos: Vec<u64>, strong: Vec<u64>, chunks: usize) -> Self {
        assert_eq!(pos.len(), strong.len(), "pos/strong 长度必须一致");
        let n = pos.len().checked_div(chunks).unwrap_or(0);
        let stride = chunks * 2;
        let total_words = n * stride;
        let mut interleaved = vec![CacheLineU64::default(); total_words.div_ceil(8)];
        let dst = unsafe {
            std::slice::from_raw_parts_mut(interleaved.as_mut_ptr() as *mut u64, total_words)
        };
        for i in 0..n {
            let src_off = i * chunks;
            let dst_off = i * stride;
            dst[dst_off..dst_off + chunks].copy_from_slice(&pos[src_off..src_off + chunks]);
            dst[dst_off + chunks..dst_off + stride]
                .copy_from_slice(&strong[src_off..src_off + chunks]);
        }
        Self {
            interleaved,
            #[cfg(feature = "research")]
            legacy_soa: None,
            colocated_store: None,
            chunks,
            n,
        }
    }

    /// 预分配 `n` 个节点的 64B 缓存行对齐交织零初始化签名仓库，供并行分块直接填充（省去双平面中转拷贝）。
    pub(crate) fn with_zeroed_len(n: usize, dim: usize) -> Self {
        let chunks = dim.div_ceil(64);
        let stride = chunks * 2;
        let total_words = n * stride;
        let interleaved = vec![CacheLineU64::default(); total_words.div_ceil(8)];
        Self {
            interleaved,
            #[cfg(feature = "research")]
            legacy_soa: None,
            colocated_store: None,
            chunks,
            n,
        }
    }

    /// 返回底层交织字数组可变切片及单节点步长 `stride = 2 * chunks`，每个节点前 `chunks` 为 `pos`、后 `chunks` 为 `strong`。
    #[inline]
    pub(crate) fn interleaved_words_mut(&mut self) -> (&mut [u64], usize) {
        self.restore_interleaved();
        let stride = self.chunks * 2;
        let total_words = self.n * stride;
        let words = unsafe {
            std::slice::from_raw_parts_mut(self.interleaved.as_mut_ptr().cast::<u64>(), total_words)
        };
        (words, stride)
    }
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "popcnt")]
pub(crate) unsafe fn bq2_distance_cheap_popcnt_any(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    chunks: usize,
) -> u32 {
    unsafe {
        let mut acc = 0u32;
        for i in 0..chunks {
            acc += (*pos_a.add(i) ^ *pos_b.add(i)).count_ones();
            acc += (*strong_a.add(i) ^ *strong_b.add(i)).count_ones();
        }
        acc
    }
}
