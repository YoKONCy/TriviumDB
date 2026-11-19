//! QuIVer 使用的二值/多比特量化签名与 Hamming 预筛。
//!
//! 导航编码保存紧凑近似信号，最终排序读取所选精度的冷向量。位布局和 chunk 数属于磁盘
//! 契约。BQ2 使用动态 chunk，不设置固定维度上限，并正确处理尾部掩码。

use crate::index::pelorus::engine::vector::VectorType;

/// 设置 `QUIVER_DISABLE_AVX512=1` 可选择不使用 AVX-512 的运行路径。
#[cfg(all(target_arch = "x86_64", not(coverage)))]
pub(crate) static FORCE_NO_AVX512: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
    crate::index::pelorus::engine::config::environment_flag(
        "QUIVER_DISABLE_AVX512",
        "TRIVIUM_NO_AVX512",
    )
});
pub(crate) static FORCE_NO_384_KERNEL: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
    crate::index::pelorus::engine::config::environment_flag(
        "QUIVER_DISABLE_SPECIALIZED_KERNEL",
        "TRIVIUM_DISABLE_384_KERNEL",
    )
});

/// x86_64 运行时位计数指令集等级缓存，消除热循环中重复的特性原子探测开销。
#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum X86BitKernel {
    Scalar,
    Popcnt,
    Avx2Popcnt,
    Avx2Only,
    #[cfg(not(coverage))]
    Avx512Vpopcnt,
}

#[cfg(target_arch = "x86_64")]
static X86_BIT_KERNEL: std::sync::LazyLock<X86BitKernel> = std::sync::LazyLock::new(|| {
    #[cfg(not(coverage))]
    if !*FORCE_NO_AVX512
        && is_x86_feature_detected!("avx512vpopcntdq")
        && is_x86_feature_detected!("avx512f")
        && is_x86_feature_detected!("popcnt")
    {
        return X86BitKernel::Avx512Vpopcnt;
    }
    let has_avx2 = is_x86_feature_detected!("avx2");
    let has_popcnt = is_x86_feature_detected!("popcnt");
    match (has_avx2, has_popcnt) {
        (true, true) => X86BitKernel::Avx2Popcnt,
        (true, false) => X86BitKernel::Avx2Only,
        (false, true) => X86BitKernel::Popcnt,
        (false, false) => X86BitKernel::Scalar,
    }
});
mod signature;
pub(crate) use signature::*;

mod store;
pub(crate) use store::*;

mod reconstruction;
pub(crate) use reconstruction::*;

mod asymmetric;
pub(crate) use asymmetric::*;

#[cfg(feature = "research")]
pub use asymmetric::set_asym_sm_lut_override;
