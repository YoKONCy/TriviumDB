//! 页对齐热节点存储。

use super::*;
/// 合表头缓存行，后续紧接当前维度的符号位与强幅值位。
#[repr(C, align(64))]
#[derive(Clone, Copy, Default)]
pub(crate) struct HotSmHeader {
    pub(crate) meta: PolarNodeMeta,
    pub(crate) sketch256: [u64; 4],
}
const _: () = assert!(std::mem::size_of::<HotSmHeader>() == 64);
/// 当前节点的只读视图，切片借用合表页且不复制签名。
pub(crate) struct HotSmNode<'a> {
    pub(crate) meta: &'a PolarNodeMeta,
    pub(crate) sketch256: &'a [u64; 4],
    pub(crate) pos: &'a [u64],
    pub(crate) strong: &'a [u64],
}
/// 节点步长取二的幂并整除页长，保证每条记录完整位于一个虚拟页内。
#[repr(C, align(4096))]
#[derive(Clone, Copy)]
pub(crate) struct HotSmPage {
    pub(crate) words: [u64; 512],
}
impl Default for HotSmPage {
    fn default() -> Self {
        Self { words: [0; 512] }
    }
}
const _: () = assert!(std::mem::size_of::<HotSmPage>() == 4096);
/// 4KiB 页对齐的单基址融合节点仓库：将原本跨两个独立大数组的元数据与 2-bit SM 签名物理合表。
pub(crate) struct HotSmStore {
    pub(crate) pages: Vec<HotSmPage>,
    pub(crate) len: usize,
    pub(crate) chunks: usize,
    pub(crate) record_bytes: usize,
}
impl HotSmStore {
    pub(crate) fn supports_dim(dim: usize) -> bool {
        dim > 0 && dim.div_ceil(64) <= (4096 - 64) / 16
    }

    pub(crate) fn build(
        space: &NativeSpace<'_>,
        bq_store: &Bq2Store,
        slot_indices: &[usize],
        node_meta: &[PolarNodeMeta],
    ) -> Self {
        let dim = space.dimensions;
        assert!(Self::supports_dim(dim), "合表记录必须能够完整容纳于单页");
        let chunks = dim.div_ceil(64);
        let record_bytes = (64 + chunks * 16).next_power_of_two();
        let nodes_per_page = 4096 / record_bytes;
        let len = slot_indices.len();
        let num_pages = len.div_ceil(nodes_per_page);
        let mut pages = vec![HotSmPage::default(); num_pages];
        pages.par_iter_mut().enumerate().for_each(|(p_idx, page)| {
            let base = p_idx * nodes_per_page;
            let count = (len - base).min(nodes_per_page);
            for off in 0..count {
                let idx = base + off;
                let slot = slot_indices[idx];
                let vec = space.vector(slot);
                let meta = node_meta.get(idx).copied().unwrap_or_else(|| {
                    PolarNodeMeta::from_node(vec, space.boundary_factors[slot], bq_store, idx, dim)
                });
                let start = off * record_bytes / 8;
                // 页基址与节点步长均满足头部的 64B 对齐，页内空间已完整初始化。
                let header =
                    unsafe { &mut *page.words.as_mut_ptr().add(start).cast::<HotSmHeader>() };
                *header = HotSmHeader {
                    meta,
                    sketch256: pack_strided_sketch256(vec),
                };
                page.words[start + 8..start + 8 + chunks].copy_from_slice(bq_store.pos_slice(idx));
                page.words[start + 8 + chunks..start + 8 + 2 * chunks]
                    .copy_from_slice(bq_store.strong_slice(idx));
            }
        });
        // 在支持 madvise 的 Linux 宿主机上尽力提示透明大页合并（当内核 THP=[never] 时安全无副作用）
        #[cfg(target_os = "linux")]
        if !pages.is_empty() {
            unsafe extern "C" {
                fn madvise(addr: *mut std::ffi::c_void, length: usize, advice: i32) -> i32;
            }
            const MADV_HUGEPAGE: i32 = 14;
            unsafe {
                let _ = madvise(
                    pages.as_mut_ptr() as *mut std::ffi::c_void,
                    pages.len() * std::mem::size_of::<HotSmPage>(),
                    MADV_HUGEPAGE,
                );
            }
        }
        Self {
            pages,
            len,
            chunks,
            record_bytes,
        }
    }

    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// 从已校验的热状态恢复页布局，直接复用保存的草图并保持残差签名。
    pub(crate) fn restore(
        dim: usize,
        signatures: &Bq2Store,
        metadata: &[PolarNodeMeta],
        sketches: &[[u64; 4]],
    ) -> Self {
        let chunks = dim.div_ceil(64);
        let record_bytes = (64 + chunks * 16).next_power_of_two();
        let per_page = 4096 / record_bytes;
        let len = metadata.len();
        let mut pages = vec![HotSmPage::default(); len.div_ceil(per_page)];
        for node in 0..len {
            let page = &mut pages[node / per_page];
            let offset = node % per_page * record_bytes / 8;
            // SAFETY：页基址按4096字节对齐，步长至少64字节且整除页长，头部及位平面完全位于当前页。
            let header = unsafe { &mut *page.words.as_mut_ptr().add(offset).cast::<HotSmHeader>() };
            *header = HotSmHeader {
                meta: metadata[node],
                sketch256: sketches[node],
            };
            page.words[offset + 8..offset + 8 + chunks].copy_from_slice(signatures.pos_slice(node));
            page.words[offset + 8 + chunks..offset + 8 + chunks * 2]
                .copy_from_slice(signatures.strong_slice(node));
        }
        Self {
            pages,
            len,
            chunks,
            record_bytes,
        }
    }

    #[inline(always)]
    pub(crate) fn get(&self, idx: usize) -> HotSmNode<'_> {
        debug_assert!(idx < self.len);
        // 步长整除 4096，节点及签名的全部字节均位于已分配的同一页内。
        unsafe {
            let ptr = self
                .pages
                .as_ptr()
                .cast::<u8>()
                .add(idx * self.record_bytes);
            let header = &*ptr.cast::<HotSmHeader>();
            let body = ptr.add(64).cast::<u64>();
            HotSmNode {
                meta: &header.meta,
                sketch256: &header.sketch256,
                pos: std::slice::from_raw_parts(body, self.chunks),
                strong: std::slice::from_raw_parts(body.add(self.chunks), self.chunks),
            }
        }
    }

    pub(crate) fn chunks(&self) -> usize {
        self.chunks
    }

    /// 预取头行与签名占用的缓存行，跳过节点末尾的页对齐填充。
    #[inline(always)]
    pub(crate) fn prefetch_full(&self, idx: usize) {
        self.prefetch_lines(idx, 0, 64 + self.chunks * 16);
    }

    /// 仅预取第 0 条头缓存行（64B = `meta` + `sketch256`），单节点仅占 1 个硬件 LFB。
    #[inline(always)]
    pub(crate) fn prefetch_header(&self, idx: usize) {
        self.prefetch_lines(idx, 0, 64);
    }

    /// 仅预取通过头行早筛幸存节点的后三条签名体缓存行（64..256B）。
    #[inline(always)]
    pub(crate) fn prefetch_body(&self, idx: usize) {
        self.prefetch_lines(idx, 64, 64 + self.chunks * 16);
    }

    #[inline(always)]
    pub(crate) fn prefetch_lines(&self, idx: usize, start: usize, end: usize) {
        if idx >= self.len {
            return;
        }
        for offset in (start..end).step_by(64) {
            let ptr = unsafe {
                self.pages
                    .as_ptr()
                    .cast::<u8>()
                    .add(idx * self.record_bytes + offset)
            };
            #[cfg(target_arch = "x86_64")]
            unsafe {
                use std::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};
                _mm_prefetch(ptr.cast::<i8>(), _MM_HINT_T0);
            }
            #[cfg(target_arch = "aarch64")]
            unsafe {
                std::arch::asm!("prfm pldl1keep, [{0}]", in(reg) ptr, options(nostack, preserves_flags));
            }
        }
    }

    #[inline]
    pub(crate) fn hot_bytes(&self) -> usize {
        self.pages.len() * std::mem::size_of::<HotSmPage>()
    }
}
/// 快速计算两个同维度 f32 向量的双精度内积（便于编译器自动生成 AVX2/AVX-512 FMA 归约）。
#[inline]
pub(crate) fn dot_f32_f64(a: &[f32], b: &[f32]) -> f64 {
    debug_assert_eq!(a.len(), b.len());
    let mut s0 = 0.0f64;
    let mut s1 = 0.0f64;
    let mut s2 = 0.0f64;
    let mut s3 = 0.0f64;
    let chunks = a.len() / 4;
    for k in 0..chunks {
        let base = k * 4;
        s0 += f64::from(a[base]) * f64::from(b[base]);
        s1 += f64::from(a[base + 1]) * f64::from(b[base + 1]);
        s2 += f64::from(a[base + 2]) * f64::from(b[base + 2]);
        s3 += f64::from(a[base + 3]) * f64::from(b[base + 3]);
    }
    let mut total = (s0 + s1) + (s2 + s3);
    for idx in (chunks * 4)..a.len() {
        total += f64::from(a[idx]) * f64::from(b[idx]);
    }
    total
}
/// 快速计算两个同维度单位 f32 向量的单精度内积（用于大批量切锥主轴最近邻分配）。
#[inline]
pub(crate) fn dot_f32_fast(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let mut s0 = 0.0f32;
    let mut s1 = 0.0f32;
    let mut s2 = 0.0f32;
    let mut s3 = 0.0f32;
    let chunks = a.len() / 4;
    for k in 0..chunks {
        let base = k * 4;
        s0 = f32::mul_add(a[base], b[base], s0);
        s1 = f32::mul_add(a[base + 1], b[base + 1], s1);
        s2 = f32::mul_add(a[base + 2], b[base + 2], s2);
        s3 = f32::mul_add(a[base + 3], b[base + 3], s3);
    }
    let mut total = (s0 + s1) + (s2 + s3);
    for idx in (chunks * 4)..a.len() {
        total = f32::mul_add(a[idx], b[idx], total);
    }
    total
}
