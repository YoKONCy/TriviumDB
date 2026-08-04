// ══════════════════════════════════════════════════════════════
//  SSD Cold/Hot 分离实验
//
//  验证 QuIVer 的冷热分离架构：
//    Hot path (RAM):  BQ 签名 + 邻接表 (~675MB/1M vecs)
//    Cold path (SSD): float32 原始向量 (仅 rerank 阶段读取)
//
//  三种模式对比：
//    1. All-RAM:   f32 向量在堆内存
//    2. SSD-warm:  f32 向量在 mmap 文件 (page cache 已预热)
//    3. SSD-cold:  f32 向量在 mmap 文件 (page cache 已清除)
//
//  page cache 清除方式：Windows NtSetSystemInformation
//  需要管理员权限运行。
//
//  用法：cargo bench --bench bench_ssd_cold_hot --features ablation
// ══════════════════════════════════════════════════════════════

use std::collections::HashSet;
use std::io::{Read, Write};
use std::time::Instant;
use triviumdb::index::quiver::{QuIVer, QuIVerConfig, QuIVerSearchConfig};

// ── 参数 ──
const DIM: usize = 768;
const TOP_K: usize = 10;
const WARMUP: usize = 50;
const ROUNDS: usize = 3;
const EF_TESTS: [usize; 4] = [32, 64, 128, 256];

// ── 文件读取（和 bench_cohere1m 一致） ──

fn read_f32_bin(path: &str) -> Vec<f32> {
    let mut file = std::fs::File::open(path).unwrap_or_else(|_| panic!("无法打开 {}", path));
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len() % 4, 0);
    bytes.chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn read_i32_bin(path: &str) -> Vec<i32> {
    let mut file = std::fs::File::open(path).unwrap_or_else(|_| panic!("无法打开 {}", path));
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len() % 4, 0);
    bytes.chunks_exact(4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn recall_at_k(gt: &[u64], res: &[(u64, f32)]) -> f64 {
    let gt_set: HashSet<u64> = gt.iter().copied().collect();
    res.iter().filter(|x| gt_set.contains(&x.0)).count() as f64 / gt.len().max(1) as f64
}

// ══════════════════════════════════════════════════════════════
//  Windows Standby List 清除
//
//  等效 Linux 的 `echo 3 > /proc/sys/vm/drop_caches`
//  需要 Administrator 权限
// ══════════════════════════════════════════════════════════════

/// 清除 Windows standby list（page cache）
/// 返回 true 表示成功
fn clear_standby_list() -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::ffi::c_void;
        use std::mem;

        // Win32 常量
        const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
        const TOKEN_QUERY: u32 = 0x0008;
        const SE_PRIVILEGE_ENABLED: u32 = 0x00000002;

        // LUID_AND_ATTRIBUTES + TOKEN_PRIVILEGES 布局
        #[repr(C)]
        struct Luid { low: u32, high: i32 }
        #[repr(C)]
        struct LuidAndAttributes { luid: Luid, attributes: u32 }
        #[repr(C)]
        struct TokenPrivileges { count: u32, privileges: [LuidAndAttributes; 1] }

        // 动态加载函数签名
        type NtSetSystemInformationFn = unsafe extern "system" fn(u32, *mut c_void, u32) -> i32;
        type OpenProcessTokenFn = unsafe extern "system" fn(isize, u32, *mut isize) -> i32;
        type LookupPrivilegeValueAFn = unsafe extern "system" fn(*const u8, *const u8, *mut Luid) -> i32;
        type AdjustTokenPrivilegesFn = unsafe extern "system" fn(isize, i32, *const TokenPrivileges, u32, *mut TokenPrivileges, *mut u32) -> i32;
        type GetCurrentProcessFn = unsafe extern "system" fn() -> isize;
        type CloseHandleFn = unsafe extern "system" fn(isize) -> i32;

        unsafe {
            // 加载 DLL
            let ntdll = match libloading::Library::new("ntdll.dll") {
                Ok(lib) => lib,
                Err(e) => { eprintln!("  ❌ 无法加载 ntdll.dll: {}", e); return false; }
            };
            let advapi32 = match libloading::Library::new("advapi32.dll") {
                Ok(lib) => lib,
                Err(e) => { eprintln!("  ❌ 无法加载 advapi32.dll: {}", e); return false; }
            };
            let kernel32 = match libloading::Library::new("kernel32.dll") {
                Ok(lib) => lib,
                Err(e) => { eprintln!("  ❌ 无法加载 kernel32.dll: {}", e); return false; }
            };

            // 获取函数指针
            let nt_set: libloading::Symbol<NtSetSystemInformationFn> = ntdll.get(b"NtSetSystemInformation").unwrap();
            let open_token: libloading::Symbol<OpenProcessTokenFn> = advapi32.get(b"OpenProcessToken").unwrap();
            let lookup_priv: libloading::Symbol<LookupPrivilegeValueAFn> = advapi32.get(b"LookupPrivilegeValueA").unwrap();
            let adjust_priv: libloading::Symbol<AdjustTokenPrivilegesFn> = advapi32.get(b"AdjustTokenPrivileges").unwrap();
            let get_process: libloading::Symbol<GetCurrentProcessFn> = kernel32.get(b"GetCurrentProcess").unwrap();
            let close_handle: libloading::Symbol<CloseHandleFn> = kernel32.get(b"CloseHandle").unwrap();

            // Step 1: 打开当前进程 token
            let mut token: isize = 0;
            if open_token(get_process(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) == 0 {
                eprintln!("  ❌ OpenProcessToken 失败");
                return false;
            }

            // Step 2: 查找 SeProfileSingleProcessPrivilege 的 LUID
            let priv_name = b"SeProfileSingleProcessPrivilege\0";
            let mut luid = Luid { low: 0, high: 0 };
            if lookup_priv(std::ptr::null(), priv_name.as_ptr(), &mut luid) == 0 {
                eprintln!("  ❌ LookupPrivilegeValue 失败");
                close_handle(token);
                return false;
            }

            // Step 3: 启用特权
            let tp = TokenPrivileges {
                count: 1,
                privileges: [LuidAndAttributes { luid, attributes: SE_PRIVILEGE_ENABLED }],
            };
            adjust_priv(token, 0, &tp, mem::size_of::<TokenPrivileges>() as u32, std::ptr::null_mut(), std::ptr::null_mut());
            close_handle(token);

            eprintln!("  🔑 已启用 SeProfileSingleProcessPrivilege");

            // Step 4: 清除 standby list
            let mut command: u32 = 4; // MemoryPurgeStandbyList
            let status = nt_set(
                80, // SystemMemoryListInformation
                &mut command as *mut u32 as *mut c_void,
                mem::size_of::<u32>() as u32,
            );

            if status == 0 {
                eprintln!("  ✅ Standby list 已清除 (page cache dropped)");
                true
            } else {
                eprintln!("  ❌ NtSetSystemInformation 失败: NTSTATUS=0x{:08X}", status as u32);
                eprintln!("     请确保以管理员身份运行！");
                false
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("  ⚠️ 非 Windows 平台，尝试 /proc/sys/vm/drop_caches...");
        let result = std::process::Command::new("sh")
            .args(["-c", "echo 3 > /proc/sys/vm/drop_caches"])
            .status();
        match result {
            Ok(s) if s.success() => {
                eprintln!("  ✅ Page cache 已清除");
                true
            }
            _ => {
                eprintln!("  ❌ 无法清除 page cache, 需要 root 权限");
                false
            }
        }
    }
}

/// 将 f32 向量写入二进制文件，返回文件路径
fn write_vectors_to_file(vecs: &[f32], path: &str) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    // 直接写入 f32 的字节
    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(
            vecs.as_ptr() as *const u8,
            std::mem::size_of_val(vecs),
        )
    };
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

/// 用 mmap 映射向量文件，返回 &[f32] 视图
fn mmap_vectors(path: &str) -> (memmap2::Mmap, usize) {
    let file = std::fs::File::open(path).expect("无法打开向量文件");
    let mmap = unsafe { memmap2::Mmap::map(&file).expect("mmap 失败") };
    let n_floats = mmap.len() / std::mem::size_of::<f32>();
    (mmap, n_floats)
}

/// 将 mmap 的字节转为 &[f32] 切片
fn mmap_as_f32(mmap: &memmap2::Mmap) -> &[f32] {
    let ptr = mmap.as_ptr() as *const f32;
    let len = mmap.len() / std::mem::size_of::<f32>();
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

// ── 搜索 + 计时 ──

struct BenchResult {
    ef: usize,
    recall: f64,
    qps: f64,
    avg_latency_us: f64,
    p99_latency_us: f64,
}

fn run_search_bench(
    index: &QuIVer,
    ext_vectors: &[f32],
    queries: &[Vec<f32>],
    warmup_queries: &[Vec<f32>],
    gts: &[Vec<u64>],
    ef: usize,
    do_warmup: bool,
) -> BenchResult {
    let n_q = queries.len();
    let search_cfg = QuIVerSearchConfig {
        top_k: TOP_K,
        ef_search: ef,
        rerank_limit: None,
    };

    if do_warmup {
        for q in warmup_queries {
            let _ = index.search_flat(q, ext_vectors, &search_cfg);
        }
    }

    let mut all_latencies: Vec<f64> = Vec::with_capacity(n_q * ROUNDS);
    let mut total_recall = 0.0;

    for _ in 0..ROUNDS {
        let mut round_recall = 0.0;
        for (qi, q) in queries.iter().enumerate() {
            let t0 = Instant::now();
            let res = index.search_flat(q, ext_vectors, &search_cfg);
            let lat = t0.elapsed().as_secs_f64() * 1e6;
            all_latencies.push(lat);
            round_recall += recall_at_k(&gts[qi], &res);
        }
        total_recall += round_recall / n_q as f64;
    }

    let avg_recall = total_recall / ROUNDS as f64 * 100.0;
    let total_time: f64 = all_latencies.iter().sum();
    let avg_qps = all_latencies.len() as f64 / (total_time / 1e6);
    let avg_lat = total_time / all_latencies.len() as f64;

    all_latencies.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let p99_idx = (all_latencies.len() as f64 * 0.99) as usize;
    let p99_lat = all_latencies[p99_idx.min(all_latencies.len() - 1)];

    BenchResult {
        ef,
        recall: avg_recall,
        qps: avg_qps,
        avg_latency_us: avg_lat,
        p99_latency_us: p99_lat,
    }
}

fn main() {
    eprintln!("═══════════════════════════════════════════════════════════════");
    eprintln!("  SSD Cold/Hot 分离实验 — QuIVer Paper §Revision");
    eprintln!("  验证 BQ-native 冷热分离架构的实际性能");
    eprintln!("═══════════════════════════════════════════════════════════════");

    // ── 加载数据（完整 Cohere-1M + 预计算 GT） ──
    eprintln!("  📂 加载 Cohere-1M 数据集...");
    let train_data = read_f32_bin("cohere_train.f32");
    let test_data = read_f32_bin("cohere_test.f32");
    let gt_data = read_i32_bin("cohere_groundtruth.i32");

    let n_train = train_data.len() / DIM;
    let n_test = test_data.len() / DIM;
    let k_gt = gt_data.len() / n_test;
    let vec_bytes = n_train * DIM * 4;
    eprintln!("  ✅ 训练集: {} × {}d | 测试集: {} × {}d | GT K: {}",
              n_train, DIM, n_test, DIM, k_gt);
    eprintln!("  向量数据大小: {:.1} MB", vec_bytes as f64 / 1048576.0);

    // 解析 Ground Truth
    let eval_gts: Vec<Vec<u64>> = (0..n_test)
        .map(|i| {
            gt_data[i * k_gt..i * k_gt + TOP_K]
                .iter()
                .map(|&id| id as u64)
                .collect()
        })
        .collect();

    // 测试查询
    let queries: Vec<Vec<f32>> = (0..n_test)
        .map(|i| test_data[i * DIM..(i + 1) * DIM].to_vec())
        .collect();
    let warmup_queries: Vec<Vec<f32>> = queries[..WARMUP.min(n_test)].to_vec();

    // ── 构建 QuIVer 索引 ──
    eprintln!("\n  🔨 构建 QuIVer 索引...");
    let config = QuIVerConfig {
        m: 32,
        ef_construction: 128,
        alpha: 1.2,
    };
    let mut index = QuIVer::new(DIM, &config);
    let ids: Vec<u64> = (0..n_train as u64).collect();
    let slots: Vec<usize> = (0..n_train).collect();
    let t0 = Instant::now();
    index.batch_build_experimental_v2(&train_data, &ids, &slots);
    let build_s = t0.elapsed().as_secs_f64();
    let stats = index.stats();
    eprintln!(
        "  构建完成: {:.2}s | 节点: {} | 平均度数: {:.1}",
        build_s, stats.n, stats.avg_degree_l0
    );

    // ── 写入向量文件 ──
    let vec_file = "bench_ssd_vectors.bin";
    eprintln!("\n  💾 写入向量到 {}...", vec_file);
    write_vectors_to_file(&train_data, vec_file).expect("写文件失败");
    eprintln!("  ✅ 写入完成: {:.1} MB", vec_bytes as f64 / 1048576.0);

    // ══════════════════════════════════════════════════════════════
    //  Phase 1: All-RAM baseline
    // ══════════════════════════════════════════════════════════════
    eprintln!("\n┌────────────────────────────────────────────────────────────────────┐");
    eprintln!("│  Phase 1: All-RAM (f32 向量在堆内存)                              │");
    eprintln!("│  数据集: Cohere-1M ({} vecs × {}d)                               │", n_train, DIM);
    eprintln!("├────────────────────────────────────────────────────────────────────┤");
    eprintln!(
        "  {:<8} {:>10} {:>10} {:>12} {:>12}",
        "ef", "Recall@10", "QPS", "Avg(μs)", "P99(μs)"
    );

    let mut ram_results = Vec::new();
    for &ef in &EF_TESTS {
        let r = run_search_bench(&index, &train_data, &queries, &warmup_queries, &eval_gts, ef, true);
        eprintln!(
            "  ef={:<5} {:>8.1}% {:>8.0} {:>10.1} {:>10.1}",
            r.ef, r.recall, r.qps, r.avg_latency_us, r.p99_latency_us
        );
        ram_results.push(r);
    }
    eprintln!("└────────────────────────────────────────────────────────────────────┘");

    // ══════════════════════════════════════════════════════════════
    //  Phase 2: SSD-warm (mmap, page cache 预热)
    // ══════════════════════════════════════════════════════════════
    eprintln!("\n┌────────────────────────────────────────────────────────────────────┐");
    eprintln!("│  Phase 2: SSD-warm (f32 向量 mmap, page cache 预热)               │");
    eprintln!("├────────────────────────────────────────────────────────────────────┤");

    let (mmap, _n_floats) = mmap_vectors(vec_file);
    let mmap_vecs = mmap_as_f32(&mmap);
    eprintln!("  mmap 已映射: {} 个 float", mmap_vecs.len());

    // 预热：顺序读一遍，确保页面全部缓存
    eprintln!("  🔥 预热 page cache (顺序读)...");
    let mut checksum = 0u64;
    for chunk in mmap_vecs.chunks(1024) {
        checksum = checksum.wrapping_add(chunk[0].to_bits() as u64);
    }
    std::hint::black_box(checksum);
    eprintln!("  ✅ 预热完成");

    eprintln!(
        "  {:<8} {:>10} {:>10} {:>12} {:>12}",
        "ef", "Recall@10", "QPS", "Avg(μs)", "P99(μs)"
    );

    let mut warm_results = Vec::new();
    for &ef in &EF_TESTS {
        let r = run_search_bench(&index, mmap_vecs, &queries, &warmup_queries, &eval_gts, ef, true);
        eprintln!(
            "  ef={:<5} {:>8.1}% {:>8.0} {:>10.1} {:>10.1}",
            r.ef, r.recall, r.qps, r.avg_latency_us, r.p99_latency_us
        );
        warm_results.push(r);
    }
    eprintln!("└────────────────────────────────────────────────────────────────────┘");

    // ══════════════════════════════════════════════════════════════
    //  Phase 3: SSD-cold (mmap, page cache 清除后)
    // ══════════════════════════════════════════════════════════════
    eprintln!("\n┌────────────────────────────────────────────────────────────────────┐");
    eprintln!("│  Phase 3: SSD-cold (f32 向量 mmap, page cache 已清除)             │");
    eprintln!("├────────────────────────────────────────────────────────────────────┤");

    let cache_cleared = clear_standby_list();
    if !cache_cleared {
        eprintln!("  ❌ 无法清除 page cache，拒绝生成无效的 SSD-cold 结果");
        eprintln!("  ❌ 请使用管理员或 root 权限重新运行");
        let _ = std::fs::remove_file(vec_file);
        std::process::exit(1);
    }

    // 等待 1 秒，确保 OS 完成清除
    std::thread::sleep(std::time::Duration::from_secs(1));

    eprintln!(
        "  {:<8} {:>10} {:>10} {:>12} {:>12} {:>8}",
        "ef", "Recall@10", "QPS", "Avg(μs)", "P99(μs)", "vs RAM"
    );

    let mut cold_results = Vec::new();
    for (i, &ef) in EF_TESTS.iter().enumerate() {
        // 每个 ef 之前都清一次 cache，确保每次都是 cold start
        if i > 0 {
            if !clear_standby_list() {
                eprintln!("  ❌ 清除 page cache 失败，SSD-cold 实验中止");
                let _ = std::fs::remove_file(vec_file);
                std::process::exit(1);
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }

        // cold 模式不做 warmup，只跑 1 轮来捕捉真实 cold start
        let r = run_search_bench(&index, mmap_vecs, &queries, &[], &eval_gts, ef, false);

        let vs_ram = if ram_results[i].qps > 0.0 {
            format!("{:.2}×", ram_results[i].qps / r.qps)
        } else {
            "N/A".to_string()
        };

        eprintln!(
            "  ef={:<5} {:>8.1}% {:>8.0} {:>10.1} {:>10.1} {:>8}",
            r.ef, r.recall, r.qps, r.avg_latency_us, r.p99_latency_us, vs_ram
        );
        cold_results.push(r);
    }
    eprintln!("└────────────────────────────────────────────────────────────────────┘");

    // ══════════════════════════════════════════════════════════════
    //  Summary
    // ══════════════════════════════════════════════════════════════
    eprintln!("\n═══════════════════════════════════════════════════════════════");
    eprintln!("  📊 Summary: SSD Cold/Hot 分离实验");
    eprintln!("───────────────────────────────────────────────────────────────");
    eprintln!("  数据集:    Cohere-1M ({} vecs × {}d)", n_train, DIM);
    eprintln!("  向量大小:  {:.1} MB (f32)", vec_bytes as f64 / 1048576.0);
    let bq_bytes = n_train * DIM / 4; // 2 bits per dim = D/4 bytes per vec
    eprintln!(
        "  BQ 签名:   {:.1} MB (hot path)",
        bq_bytes as f64 / 1048576.0
    );
    eprintln!(
        "  压缩比:    {:.0}× (f32 → BQ hot path)",
        vec_bytes as f64 / bq_bytes as f64
    );
    eprintln!("───────────────────────────────────────────────────────────────");
    eprintln!(
        "  {:<8} {:>10} {:>10} {:>10} {:>10}",
        "ef", "RAM QPS", "Warm QPS", "Cold QPS", "Cold/RAM"
    );
    for i in 0..EF_TESTS.len() {
        let ratio = if ram_results[i].qps > 0.0 {
            format!("{:.1}%", cold_results[i].qps / ram_results[i].qps * 100.0)
        } else {
            "N/A".to_string()
        };
        eprintln!(
            "  ef={:<5} {:>8.0} {:>8.0} {:>8.0} {:>10}",
            EF_TESTS[i], ram_results[i].qps, warm_results[i].qps, cold_results[i].qps, ratio
        );
    }
    eprintln!("═══════════════════════════════════════════════════════════════");
    eprintln!("  ✅ SSD Cold/Hot 分离实验完成");

    // 清理临时文件
    let _ = std::fs::remove_file(vec_file);
}
