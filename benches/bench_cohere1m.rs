use rayon::prelude::*;
use std::fs::File;
use std::io::Read;
use std::time::Instant;
use triviumdb::index::quiver::{QuIVer, QuIVerConfig, QuIVerSearchConfig};

const DEFAULT_DIM: usize = 768;
const BRUTE_FORCE_QUERIES: usize = 50;
const EXACT_TOP_K: usize = 10;

struct AnnBenchConfig {
    name: String,
    dim: usize,
    train_path: String,
    test_path: String,
    gt_path: String,
}

fn env_string(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
}

/// 数据集预设配置
///
/// 支持的预设：
///   - "cohere-1m"  (默认) — Cohere 1M 真实数据集
///   - "random-1m"          — bench_random1m 生成的随机球面数据
///
/// 用法：
///   $env:TRIVIUM_ANN_NAME="random-1m"
///   cargo bench --bench bench_cohere1m
///
/// 也可通过单独环境变量覆盖任意字段（优先级高于预设）
fn bench_config() -> AnnBenchConfig {
    let name = env_string("TRIVIUM_ANN_NAME", "cohere-1m");

    // 根据预设名称选择默认路径
    let (default_train, default_test, default_gt) = match name.as_str() {
        "random-1m" => (
            "random_train.f32",
            "random_test.f32",
            "random_groundtruth.i32",
        ),
        _ => (
            "cohere_train.f32",
            "cohere_test.f32",
            "cohere_groundtruth.i32",
        ),
    };

    AnnBenchConfig {
        name,
        dim: env_usize("TRIVIUM_ANN_DIM", DEFAULT_DIM),
        train_path: env_string("TRIVIUM_ANN_TRAIN", default_train),
        test_path: env_string("TRIVIUM_ANN_TEST", default_test),
        gt_path: env_string("TRIVIUM_ANN_GT", default_gt),
    }
}

fn train_limit(full_n_train: usize) -> usize {
    let value = std::env::var("TRIVIUM_ANN_TRAIN_LIMIT")
        .or_else(|_| std::env::var("TRIVIUM_COHERE_TRAIN_LIMIT"));

    match value {
        Ok(value) if value.eq_ignore_ascii_case("full") || value == "0" => full_n_train,
        Ok(value) => value
            .parse::<usize>()
            .unwrap_or(full_n_train)
            .min(full_n_train),
        // v2 并发构图 100 秒级完成 1M，默认全量
        Err(_) => full_n_train,
    }
}

fn read_f32_bin(path: &str) -> Vec<f32> {
    let mut file = File::open(path).unwrap_or_else(|_| panic!("无法打开 {}", path));
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len() % 4, 0, "{} 的字节长度不是 4 的倍数", path);

    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn read_i32_bin(path: &str) -> Vec<i32> {
    let mut file = File::open(path).unwrap_or_else(|_| panic!("无法打开 {}", path));
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len() % 4, 0, "{} 的字节长度不是 4 的倍数", path);

    bytes
        .chunks_exact(4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn normalize_vectors(data: &[f32], dim: usize) -> Vec<f32> {
    data.par_chunks(dim)
        .flat_map_iter(|v| {
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
            v.iter().map(move |x| x / norm)
        })
        .collect()
}

#[inline(always)]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

fn exact_flat_topk_normalized(
    train_norm: &[f32],
    query_norm: &[f32],
    dim: usize,
    top_k: usize,
) -> Vec<u64> {
    let n_train = train_norm.len() / dim;
    let mut best: Vec<(u64, f32)> = Vec::with_capacity(top_k + 1);

    for i in 0..n_train {
        let base = &train_norm[i * dim..(i + 1) * dim];
        let sim = dot(query_norm, base);
        if best.len() < top_k {
            best.push((i as u64, sim));
            if best.len() == top_k {
                best.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            }
        } else if sim > best[top_k - 1].1 {
            best[top_k - 1] = (i as u64, sim);
            let mut j = top_k - 1;
            while j > 0 && best[j].1 > best[j - 1].1 {
                best.swap(j, j - 1);
                j -= 1;
            }
        }
    }

    best.into_iter().map(|(id, _)| id).collect()
}

fn brute_force_topk(train_data: &[f32], query: &[f32], dim: usize, top_k: usize) -> Vec<u64> {
    let n_train = train_data.len() / dim;
    let mut scored: Vec<(u64, f32)> = (0..n_train)
        .map(|i| {
            let base = &train_data[i * dim..(i + 1) * dim];
            let sim = triviumdb::index::quiver::cosine_sim(query, base);
            (i as u64, sim)
        })
        .collect();

    scored.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    scored.truncate(top_k);
    scored.into_iter().map(|(id, _)| id).collect()
}

fn main() {
    let bench = bench_config();
    let dim = bench.dim;

    println!("加载 {} 数据集...", bench.name);
    println!("训练集文件: {}", bench.train_path);
    println!("测试集文件: {}", bench.test_path);
    println!("GroundTruth 文件: {}", bench.gt_path);
    println!("向量维度: {}", dim);

    let t0 = Instant::now();
    let train_data = read_f32_bin(&bench.train_path);
    let test_data = read_f32_bin(&bench.test_path);
    let gt_data = read_i32_bin(&bench.gt_path);

    let full_n_train = train_data.len() / dim;
    assert_eq!(train_data.len() % dim, 0, "训练集维度不完整");
    let n_train = train_limit(full_n_train);
    let train_data = &train_data[..n_train * dim];
    let n_test = test_data.len() / dim;
    assert_eq!(test_data.len() % dim, 0, "测试集维度不完整");
    assert_eq!(
        gt_data.len() % n_test,
        0,
        "GroundTruth 行数不能被测试集大小整除"
    );
    let k_gt = gt_data.len() / n_test;
    let use_original_gt = n_train == full_n_train && k_gt >= EXACT_TOP_K;

    println!("加载完成! 耗时: {:.2}s", t0.elapsed().as_secs_f64());
    println!(
        "训练集: {} x {} (原始 {} x {})",
        n_train, dim, full_n_train, dim
    );
    println!("测试集: {} x {}", n_test, dim);
    println!("原始 GroundTruth K: {}", k_gt);
    if use_original_gt {
        println!("使用原始 1M GroundTruth Top-10");
    } else {
        println!(
            "子集 GroundTruth: 对前 {} 条训练数据重新计算 Top-10",
            n_train
        );
    }

    let ids: Vec<u64> = (0..n_train as u64).collect();

    let config = QuIVerConfig {
        m: env_usize("TRIVIUM_ANN_M", 32),
        ef_construction: env_usize("TRIVIUM_ANN_EF_CONSTRUCTION", 128),
        alpha: std::env::var("TRIVIUM_ANN_ALPHA")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(1.2),
    };

    println!(
        "\n开始构建 BQ-Vamana (m={}, ef_c={}, alpha={})...",
        config.m, config.ef_construction, config.alpha
    );
    let mut index = QuIVer::new(dim, &config);

    let t_build = Instant::now();
    let slot_idxs: Vec<usize> = (0..ids.len()).collect();
    match std::env::var("TRIVIUM_BQ_HNSW_EXPERIMENTAL").as_deref() {
        Ok("2-checked") => {
            println!("使用类型化校验并发构图路径");
            index.batch_build_experimental_v2_checked(train_data, &ids, &slot_idxs);
        }
        Ok("1") => {
            println!("使用早期反向剪枝并行构图路径（旧版）");
            index.batch_build_experimental(train_data, &ids, &slot_idxs);
        }
        Ok("serial") => {
            println!("使用串行构图路径（旧版，仅用于对照）");
            index.batch_build(train_data, &ids, &slot_idxs);
        }
        _ => {
            println!("使用并发构图路径（默认）");
            index.batch_build_experimental_v2(train_data, &ids, &slot_idxs);
        }
    }
    let build_time = t_build.elapsed().as_secs_f64();
    println!("构建完成! 耗时: {:.2}s", build_time);

    let stats = index.stats();
    println!(
        "内存统计: Hot {} MB",
        stats.hot_bytes / 1024 / 1024
    );

    let top_k = 10;
    let eval_gts: Vec<Vec<u64>> = if use_original_gt {
        println!("\n加载原始 GroundTruth Top-{} 用于 1M 标准评测...", top_k);
        (0..n_test)
            .map(|i| {
                gt_data[i * k_gt..i * k_gt + top_k]
                    .iter()
                    .map(|&id| id as u64)
                    .collect()
            })
            .collect()
    } else {
        println!(
            "\n重新计算 {} 条测试查询在 {} 条训练子集上的精确 Top-{}...",
            n_test, n_train, top_k
        );
        let t_gt = Instant::now();
        let subset_gts: Vec<Vec<u64>> = (0..n_test)
            .into_par_iter()
            .map(|i| {
                let q = &test_data[i * dim..(i + 1) * dim];
                brute_force_topk(train_data, q, dim, top_k)
            })
            .collect();
        println!(
            "子集 GroundTruth 计算完成! 耗时: {:.2}s",
            t_gt.elapsed().as_secs_f64()
        );
        subset_gts
    };

    println!("\n开始测试 Recall@{}:", top_k);

    for &ef in &[64, 128, 256, 512, 1024] {
        let search_cfg = QuIVerSearchConfig {
            top_k,
            ef_search: ef,
        };

        let t_search = Instant::now();
        let hits: usize = (0..n_test)
            .into_par_iter()
            .map(|i| {
                let q = &test_data[i * dim..(i + 1) * dim];
                let res = index.search(q, train_data, &search_cfg);
                let gt_set = &eval_gts[i];

                res.iter().filter(|&&(id, _)| gt_set.contains(&id)).count()
            })
            .sum();
        let total = n_test * top_k;

        let qps = n_test as f64 / t_search.elapsed().as_secs_f64();
        let recall = hits as f64 / total as f64;

        println!(
            "ef={:<4} | Recall@{}: {:.2}% | QPS: {:.0}",
            ef,
            top_k,
            recall * 100.0,
            qps
        );
    }

    println!(
        "\n计算标准 Exact Flat Cosine 基准 (预归一化 + Top-{} 小数组，无全量排序)...",
        EXACT_TOP_K
    );
    let exact_queries = BRUTE_FORCE_QUERIES.min(n_test);
    let t_norm = Instant::now();
    let train_norm = normalize_vectors(train_data, dim);
    let query_norm = normalize_vectors(&test_data[..exact_queries * dim], dim);
    let norm_time = t_norm.elapsed().as_secs_f64();
    println!("Exact Flat 归一化耗时: {:.2}s", norm_time);

    let t_exact_single = Instant::now();
    for i in 0..exact_queries {
        let q = &query_norm[i * dim..(i + 1) * dim];
        let res = exact_flat_topk_normalized(&train_norm, q, dim, EXACT_TOP_K);
        std::hint::black_box(res);
    }
    let exact_single_time = t_exact_single.elapsed().as_secs_f64();
    let exact_single_qps = exact_queries as f64 / exact_single_time;
    println!("Exact Flat 单线程查询数: {}", exact_queries);
    println!("Exact Flat 单线程耗时: {:.2}s", exact_single_time);
    println!("Exact Flat 单线程 QPS: {:.4}", exact_single_qps);

    let t_exact_parallel = Instant::now();
    (0..exact_queries).into_par_iter().for_each(|i| {
        let q = &query_norm[i * dim..(i + 1) * dim];
        let res = exact_flat_topk_normalized(&train_norm, q, dim, EXACT_TOP_K);
        std::hint::black_box(res);
    });
    let exact_parallel_time = t_exact_parallel.elapsed().as_secs_f64();
    let exact_parallel_qps = exact_queries as f64 / exact_parallel_time;
    println!("Exact Flat 多查询并行耗时: {:.2}s", exact_parallel_time);
    println!("Exact Flat 多查询并行 QPS: {:.4}", exact_parallel_qps);
    println!(
        "ef=64 相对 Exact Flat 单线程加速比请用上方 ef=64 QPS / {:.4} 计算",
        exact_single_qps
    );
    println!(
        "ef=64 相对 Exact Flat 多查询并行加速比请用上方 ef=64 QPS / {:.4} 计算",
        exact_parallel_qps
    );
}
