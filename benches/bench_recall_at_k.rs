//! Recall@K 多维度测试
//!
//! 在标准参数 (m=32, ef_c=128, α=1.2) 下，测试不同 K 值的 recall 和 QPS。
//! K = 1, 10, 100, 500
//!
//! 用法:
//!   cargo bench --bench bench_recall_at_k

use rayon::prelude::*;
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::time::Instant;
use triviumdb::index::quiver::{QuIVer, QuIVerConfig, QuIVerSearchConfig};

fn read_f32_bin(path: &str) -> Vec<f32> {
    let mut file = File::open(path).unwrap_or_else(|_| panic!("无法打开 {}", path));
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len() % 4, 0);
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn read_i32_bin(path: &str) -> Vec<i32> {
    let mut file = File::open(path).unwrap_or_else(|_| panic!("无法打开 {}", path));
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len() % 4, 0);
    bytes
        .chunks_exact(4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn env_string(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn main() {
    let dim: usize = std::env::var("TRIVIUM_ANN_DIM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(768);
    let train_path = env_string("TRIVIUM_ANN_TRAIN", "cohere_train.f32");
    let test_path = env_string("TRIVIUM_ANN_TEST", "cohere_test.f32");
    let gt_path = env_string("TRIVIUM_ANN_GT", "cohere_groundtruth.i32");

    println!("======================================================================");
    println!("Recall@K 多维度测试");
    println!("参数: m=32, ef_c=128, α=1.2 (标准配置)");
    println!("======================================================================\n");

    // 加载数据
    let t0 = Instant::now();
    let train_data = read_f32_bin(&train_path);
    let test_data = read_f32_bin(&test_path);
    let gt_data = read_i32_bin(&gt_path);

    let n_train = train_data.len() / dim;
    let n_test = test_data.len() / dim;
    let k_gt = gt_data.len() / n_test;

    println!(
        "加载完成! {} x {} 训练集, {} 测试集, GT K={}, 耗时 {:.2}s\n",
        n_train,
        dim,
        n_test,
        k_gt,
        t0.elapsed().as_secs_f64()
    );

    assert!(k_gt >= 500, "GT 需要至少 top-500, 实际只有 top-{}", k_gt);

    // 构建索引
    let config = QuIVerConfig {
        m: 32,
        ef_construction: 128,
        alpha: 1.2,
    };

    println!("构建 QuIVer 索引 (m=32, ef_c=128, α=1.2)...");
    let mut index = QuIVer::new(dim, &config);
    let ids: Vec<u64> = (0..n_train as u64).collect();
    let slot_idxs: Vec<usize> = (0..n_train).collect();

    let t_build = Instant::now();
    index.batch_build_experimental_v2(&train_data, &ids, &slot_idxs);
    let build_time = t_build.elapsed().as_secs_f64();
    println!(
        "构建完成! 耗时 {:.1}s ({:.0} vecs/s)\n",
        build_time,
        n_train as f64 / build_time
    );

    let stats = index.stats();
    println!("Hot 内存: {} MB\n", stats.hot_bytes / 1024 / 1024);

    // 测试的 K 值
    let k_values = [1, 10, 100, 500];
    // 测试的 ef_search 值（需要覆盖从小到大的范围）
    let ef_values = [8, 16, 32, 64, 128, 256, 512, 768, 1024, 1536, 2048];

    // 预构建每个 K 的 GT 集合
    let gt_sets: Vec<Vec<HashSet<u64>>> = k_values
        .iter()
        .map(|&k| {
            (0..n_test)
                .map(|i| {
                    gt_data[i * k_gt..i * k_gt + k]
                        .iter()
                        .map(|&id| id as u64)
                        .collect::<HashSet<u64>>()
                })
                .collect()
        })
        .collect();

    // 表头
    print!("{:<8}", "ef");
    for &k in &k_values {
        print!("  {:>10}  {:>10}", format!("R@{}", k), format!("QPS@{}", k));
    }
    println!();
    println!("{}", "-".repeat(8 + k_values.len() * 24));

    for &ef in &ef_values {
        print!("{:<8}", ef);

        for (ki, &k) in k_values.iter().enumerate() {
            // ef_search 必须 >= top_k
            let actual_ef = ef.max(k);

            let search_cfg = QuIVerSearchConfig {
                top_k: k,
                ef_search: actual_ef,
                rerank_limit: None,
            };

            let t_search = Instant::now();
            let hits: usize = (0..n_test)
                .into_par_iter()
                .map(|i| {
                    let q = &test_data[i * dim..(i + 1) * dim];
                    let res = index.search_flat(q, &train_data, &search_cfg);
                    let gt_set = &gt_sets[ki][i];
                    res.iter().filter(|&&(id, _)| gt_set.contains(&id)).count()
                })
                .sum();

            let elapsed = t_search.elapsed().as_secs_f64();
            let qps = n_test as f64 / elapsed;
            let recall = hits as f64 / (n_test * k) as f64;

            print!("  {:>9.2}%  {:>10.0}", recall * 100.0, qps);
        }
        println!();
    }

    println!("\n======================================================================");
    println!("测试完成!");
    println!("======================================================================");
}
