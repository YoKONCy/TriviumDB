use rayon::prelude::*;
use std::fs::File;
use std::io::Read;
use std::time::Instant;
use usearch::{
    Index, IndexOptions, MetricKind, ScalarKind, hardware_acceleration_available,
    hardware_acceleration_compiled,
};

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

fn bench_config() -> AnnBenchConfig {
    AnnBenchConfig {
        name: env_string("TRIVIUM_ANN_NAME", "cohere-1m"),
        dim: env_usize("TRIVIUM_ANN_DIM", DEFAULT_DIM),
        train_path: env_string("TRIVIUM_ANN_TRAIN", "cohere_train.f32"),
        test_path: env_string("TRIVIUM_ANN_TEST", "cohere_test.f32"),
        gt_path: env_string("TRIVIUM_ANN_GT", "cohere_groundtruth.i32"),
    }
}

fn train_limit(full_n_train: usize) -> usize {
    let value = std::env::var("TRIVIUM_ANN_TRAIN_LIMIT")
        .or_else(|_| std::env::var("TRIVIUM_COHERE_TRAIN_LIMIT"));

    match value {
        Ok(value) if value.eq_ignore_ascii_case("full") || value == "0" => full_n_train,
        Ok(value) => value.parse::<usize>().unwrap_or(full_n_train).min(full_n_train),
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
    println!("USearch 编译 ISA: {}", hardware_acceleration_compiled());
    println!("USearch 运行时 ISA: {}", hardware_acceleration_available());

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

    let mut options = IndexOptions::default();
    options.dimensions = dim;
    options.metric = MetricKind::Cos;
    options.quantization = ScalarKind::F32;
    options.connectivity = env_usize("TRIVIUM_USEARCH_M", 8);
    options.expansion_add = env_usize("TRIVIUM_USEARCH_EF_CONSTRUCTION", 32);
    options.expansion_search = env_usize("TRIVIUM_USEARCH_EF_SEARCH", 16);

    println!(
        "\n开始构建 USearch (m={}, ef_c={}, quantization=F32)...",
        options.connectivity, options.expansion_add
    );
    let index = Index::new(&options).expect("USearch 索引创建失败");
    index.reserve(n_train).expect("USearch 预留容量失败");

    let t_build = Instant::now();
    for i in 0..n_train {
        let vector = &train_data[i * dim..(i + 1) * dim];
        index
            .add(i as u64, vector)
            .unwrap_or_else(|err| panic!("USearch 插入第 {} 个向量失败: {}", i, err));
    }
    let build_time = t_build.elapsed().as_secs_f64();
    println!("构建完成! 耗时: {:.2}s", build_time);
    println!("索引大小: {}", index.size());
    println!("索引容量: {}", index.capacity());
    println!("索引连接度: {}", index.connectivity());
    println!("索引默认 ef_search: {}", index.expansion_search());
    println!("索引内存占用: {} MB", index.memory_usage() / 1024 / 1024);
    println!(
        "序列化大小估计: {} MB",
        index.serialized_length() / 1024 / 1024
    );
    println!("运行时硬件加速: {}", index.hardware_acceleration());

    println!("\n开始测试 Recall@{}:", top_k);
    for &ef in &[32, 64, 128] {
        index.change_expansion_search(ef);

        let t_search = Instant::now();
        let hits: usize = (0..n_test)
            .into_par_iter()
            .map(|i| {
                let q = &test_data[i * dim..(i + 1) * dim];
                let res = index.search(q, top_k).expect("USearch 查询失败");
                let gt_set = &eval_gts[i];
                res.keys
                    .iter()
                    .filter(|&&id| gt_set.contains(&id))
                    .count()
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
