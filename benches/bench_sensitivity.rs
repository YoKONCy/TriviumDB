//! QuIVer 参数敏感性 + 多线程查询扩展性 benchmark
//!
//! 覆盖三大实验：
//!   1. 参数敏感性：m, ef_c, alpha 网格扫描
//!   2. 单线程 vs 多线程查询 QPS
//!   3. 构图时间 vs 核数 scalability
//!
//! 用法：
//!   $env:TRIVIUM_ANN_NAME="cohere-1m"
//!   cargo bench --bench bench_sensitivity
//!
//!   # 只跑参数敏感性
//!   $env:TRIVIUM_SENSITIVITY_MODE="params"
//!
//!   # 只跑多线程查询
//!   $env:TRIVIUM_SENSITIVITY_MODE="threads"
//!
//!   # 全跑（默认）
//!   $env:TRIVIUM_SENSITIVITY_MODE="all"
//!
//!   # 跳过已完成的子实验，从 1c 开始
//!   $env:TRIVIUM_SENSITIVITY_START="1c"

use rayon::prelude::*;
use std::fs::File;
use std::io::Read;
use std::time::Instant;
use triviumdb::index::quiver::{QuIVer, QuIVerConfig, QuIVerSearchConfig};

// ============================================================
//  数据加载（复用 bench_cohere1m 的逻辑）
// ============================================================

fn env_string(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

struct DataSet {
    name: String,
    dim: usize,
    train: Vec<f32>,
    test: Vec<f32>,
    n_train: usize,
    n_test: usize,
    /// GroundTruth Top-K 用于 Recall 计算
    gts: Vec<Vec<u64>>,
}

fn load_dataset() -> DataSet {
    let name = env_string("TRIVIUM_ANN_NAME", "cohere-1m");
    let (train_path, test_path, gt_path, dim) = match name.as_str() {
        "minilm-384" => (
            "minilm_train.f32",
            "minilm_test.f32",
            "minilm_groundtruth.i32",
            384,
        ),
        "dbpedia-1536" => (
            "dbpedia_openai_train.f32",
            "dbpedia_openai_test.f32",
            "dbpedia_openai_groundtruth.i32",
            1536,
        ),
        "wolt-clip-512" => (
            "wolt_clip_train.f32",
            "wolt_clip_test.f32",
            "wolt_clip_groundtruth.i32",
            512,
        ),
        "sift-128" => (
            "sift128_train.f32",
            "sift128_test.f32",
            "sift128_groundtruth.i32",
            128,
        ),
        "gist-960" => (
            "gist960_train.f32",
            "gist960_test.f32",
            "gist960_groundtruth.i32",
            960,
        ),
        "glove-100" => (
            "glove100_train.f32",
            "glove100_test.f32",
            "glove100_groundtruth.i32",
            100,
        ),
        "dbpedia-3072" => (
            "dbpedia_openai_3072_train.f32",
            "dbpedia_openai_3072_test.f32",
            "dbpedia_openai_3072_groundtruth.i32",
            3072,
        ),
        "bge-m3-1024" => (
            "bge_m3_train.f32",
            "bge_m3_test.f32",
            "bge_m3_groundtruth.i32",
            1024,
        ),
        "random-1m" => (
            "random_train.f32",
            "random_test.f32",
            "random_groundtruth.i32",
            768,
        ),
        "sphere-1m" => (
            "sphere_train.f32",
            "sphere_test.f32",
            "sphere_groundtruth.i32",
            768,
        ),
        // VIBE 768-d 同维数据集
        "arxiv-nomic" => (
            "arxiv_nomic_train.f32",
            "arxiv_nomic_test.f32",
            "arxiv_nomic_groundtruth.i32",
            768,
        ),
        "ccnews-nomic" => (
            "ccnews_nomic_train.f32",
            "ccnews_nomic_test.f32",
            "ccnews_nomic_groundtruth.i32",
            768,
        ),
        "coco-nomic" => (
            "coco_nomic_train.f32",
            "coco_nomic_test.f32",
            "coco_nomic_groundtruth.i32",
            768,
        ),
        "codesearch-jina" => (
            "codesearch_jina_train.f32",
            "codesearch_jina_test.f32",
            "codesearch_jina_groundtruth.i32",
            768,
        ),
        "gooaq-roberta" => (
            "gooaq_roberta_train.f32",
            "gooaq_roberta_test.f32",
            "gooaq_roberta_groundtruth.i32",
            768,
        ),
        "landmark-nomic" => (
            "landmark_nomic_train.f32",
            "landmark_nomic_test.f32",
            "landmark_nomic_groundtruth.i32",
            768,
        ),
        "landmark-dino" => (
            "landmark_dino_train.f32",
            "landmark_dino_test.f32",
            "landmark_dino_groundtruth.i32",
            768,
        ),
        // MSMARCO 论文 scalability 数据集
        "msmarco-cohere" => (
            "msmarco_cohere_train.f32",
            "msmarco_cohere_test.f32",
            "msmarco_cohere_groundtruth.i32",
            1024,
        ),
        _ => (
            "cohere_train.f32",
            "cohere_test.f32",
            "cohere_groundtruth.i32",
            768,
        ),
    };

    println!("加载 {} 数据集...", name);
    let t0 = Instant::now();
    let train = read_f32_bin(train_path);
    let test = read_f32_bin(test_path);
    let gt_data = read_i32_bin(gt_path);

    let n_train = train.len() / dim;
    let n_test = test.len() / dim;
    let k_gt = gt_data.len() / n_test;

    let top_k = 10usize;
    let gts: Vec<Vec<u64>> = (0..n_test)
        .map(|i| {
            gt_data[i * k_gt..i * k_gt + top_k.min(k_gt)]
                .iter()
                .map(|&id| id as u64)
                .collect()
        })
        .collect();

    println!(
        "加载完成! {} x {} 训练集, {} 测试集, 耗时 {:.2}s",
        n_train,
        dim,
        n_test,
        t0.elapsed().as_secs_f64()
    );

    DataSet {
        name,
        dim,
        train,
        test,
        n_train,
        n_test,
        gts,
    }
}

fn read_f32_bin(path: &str) -> Vec<f32> {
    let mut file = File::open(path).unwrap_or_else(|_| panic!("无法打开 {}", path));
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect()
}

fn read_i32_bin(path: &str) -> Vec<i32> {
    let mut file = File::open(path).unwrap_or_else(|_| panic!("无法打开 {}", path));
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| i32::from_le_bytes(*bytes))
        .collect()
}

// ============================================================
//  核心测量函数
// ============================================================

/// 构建索引，返回 (索引, 构图时间秒, hot内存MB, vecs/s)
fn build_index(
    train: &[f32],
    dim: usize,
    m: usize,
    ef_c: usize,
    alpha: f32,
) -> (QuIVer, f64, usize, f64) {
    let config = QuIVerConfig {
        m,
        ef_construction: ef_c,
        alpha,
    };
    let mut index = QuIVer::new(dim, &config);
    let n = train.len() / dim;
    let ids: Vec<u64> = (0..n as u64).collect();
    let slots: Vec<usize> = (0..n).collect();

    let t = Instant::now();
    index.batch_build_experimental_v2(train, &ids, &slots);
    let build_secs = t.elapsed().as_secs_f64();
    let hot_mb = index.stats().hot_bytes / 1024 / 1024;
    let vecs_per_sec = n as f64 / build_secs;

    (index, build_secs, hot_mb, vecs_per_sec)
}

struct SearchEval<'a> {
    test: &'a [f32],
    train: &'a [f32],
    dim: usize,
    n_test: usize,
    gts: &'a [Vec<u64>],
    top_k: usize,
}

/// 单线程串行查询，返回 (recall, qps)
fn measure_single_thread(index: &QuIVer, eval: &SearchEval<'_>, ef: usize) -> (f64, f64) {
    let cfg = QuIVerSearchConfig {
        top_k: eval.top_k,
        ef_search: ef,
        rerank_limit: None,
    };

    let t = Instant::now();
    let mut hits = 0usize;
    for i in 0..eval.n_test {
        let q = &eval.test[i * eval.dim..(i + 1) * eval.dim];
        let res = index.search_flat(q, eval.train, &cfg);
        hits += res
            .iter()
            .filter(|&&(id, _)| eval.gts[i].contains(&id))
            .count();
    }
    let elapsed = t.elapsed().as_secs_f64();
    let recall = hits as f64 / (eval.n_test * eval.top_k) as f64;
    let qps = eval.n_test as f64 / elapsed;
    (recall, qps)
}

/// 多线程并行查询，返回 (recall, qps)
fn measure_multi_thread(index: &QuIVer, eval: &SearchEval<'_>, ef: usize) -> (f64, f64) {
    let cfg = QuIVerSearchConfig {
        top_k: eval.top_k,
        ef_search: ef,
        rerank_limit: None,
    };

    let t = Instant::now();
    let hits: usize = (0..eval.n_test)
        .into_par_iter()
        .map(|i| {
            let q = &eval.test[i * eval.dim..(i + 1) * eval.dim];
            let res = index.search_flat(q, eval.train, &cfg);
            res.iter()
                .filter(|&&(id, _)| eval.gts[i].contains(&id))
                .count()
        })
        .sum();
    let elapsed = t.elapsed().as_secs_f64();
    let recall = hits as f64 / (eval.n_test * eval.top_k) as f64;
    let qps = eval.n_test as f64 / elapsed;
    (recall, qps)
}

// ============================================================
//  实验 1：参数敏感性网格扫描
// ============================================================

fn experiment_param_sensitivity(ds: &DataSet) {
    println!("\n{}", "=".repeat(70));
    println!("实验 1: 参数敏感性分析");
    println!("数据集: {} ({} x {})", ds.name, ds.n_train, ds.dim);
    println!("{}", "=".repeat(70));

    let top_k = 10;
    let eval = SearchEval {
        test: &ds.test,
        train: &ds.train,
        dim: ds.dim,
        n_test: ds.n_test,
        gts: &ds.gts,
        top_k,
    };

    // 支持跳过子实验：
    //   TRIVIUM_SENSITIVITY_START=1d 则跳过 1a、1b、1c
    //   TRIVIUM_SENSITIVITY_END=1e   则跳过 1f（m×α 交叉，已证明 α 影响极小）
    let start = env_string("TRIVIUM_SENSITIVITY_START", "1a");
    let end = env_string("TRIVIUM_SENSITIVITY_END", "1e");
    let should_run = |tag: &str| tag >= start.as_str() && tag <= end.as_str();

    // 1a/1b/1c 共用的探测 ef 列表
    let ef_probes = [32, 64, 128, 256, 512, 1024];

    // 先测 brute-force baseline（限制最多 1000 queries，避免高维数据集耗时爆炸）
    let bf_n = ds.n_test.min(1000);
    println!("\n计算 brute-force 基准 QPS ({}q)...", bf_n);
    let bf_qps = {
        let t = Instant::now();
        let _: usize = (0..bf_n)
            .into_par_iter()
            .map(|i| {
                let q = &ds.test[i * ds.dim..(i + 1) * ds.dim];
                let mut best: Vec<(u64, f32)> = Vec::with_capacity(top_k + 1);
                let n = ds.train.len() / ds.dim;
                for j in 0..n {
                    let v = &ds.train[j * ds.dim..(j + 1) * ds.dim];
                    let sim: f32 = q.iter().zip(v).map(|(a, b)| a * b).sum();
                    if best.len() < top_k {
                        best.push((j as u64, sim));
                        if best.len() == top_k {
                            best.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
                        }
                    } else if sim > best[top_k - 1].1 {
                        best[top_k - 1] = (j as u64, sim);
                        let mut k = top_k - 1;
                        while k > 0 && best[k].1 > best[k - 1].1 {
                            best.swap(k, k - 1);
                            k -= 1;
                        }
                    }
                }
                best.len()
            })
            .sum();
        let elapsed = t.elapsed().as_secs_f64();
        let qps = bf_n as f64 / elapsed;
        let lat_ms = elapsed / bf_n as f64 * 1000.0;
        println!(
            "Brute-force 基准: QPS={:.1}, latency={:.2}ms/q",
            qps, lat_ms
        );
        qps
    };

    if should_run("1a") {
        // ── 1a: m 的影响（固定 ef_c=128, alpha=1.2）──
        let m_values = [4, 8, 12, 16, 20, 24, 28, 32, 40, 48, 56, 64];

        println!("\n--- 1a: m 的影响 (ef_c=128, α=1.2) ---");
        print!(
            "{:<6} {:>10} {:>10} {:>10}",
            "m", "Build(s)", "vecs/s", "Hot(MB)"
        );
        for &ef in &ef_probes {
            print!("  ef={:<4}R  ef={:<4}Q", ef, ef);
        }
        println!();
        println!("{}", "-".repeat(6 + 10 + 10 + 10 + ef_probes.len() * 22));

        for &m in &m_values {
            let (index, build_s, hot_mb, vps) = build_index(&ds.train, ds.dim, m, 128, 1.2);
            print!("{:<6} {:>10.1} {:>10.0} {:>10}", m, build_s, vps, hot_mb);
            for &ef in &ef_probes {
                let (recall, qps) = measure_multi_thread(&index, &eval, ef);
                print!("  {:>6.2}%  {:>6.0}", recall * 100.0, qps);
            }
            println!();
        }
    } // end 1a

    if should_run("1b") {
        // ── 1b: ef_construction 的影响（固定 m=32, alpha=1.2）──
        let ef_c_values = [16, 32, 48, 64, 96, 128, 160, 200, 256, 384, 512];

        println!("\n--- 1b: ef_construction 的影响 (m=32, α=1.2) ---");
        print!(
            "{:<8} {:>10} {:>10} {:>10}",
            "ef_c", "Build(s)", "vecs/s", "Hot(MB)"
        );
        for &ef in &ef_probes {
            print!("  ef={:<4}R  ef={:<4}Q", ef, ef);
        }
        println!();
        println!("{}", "-".repeat(8 + 10 + 10 + 10 + ef_probes.len() * 22));

        for &ef_c in &ef_c_values {
            let (index, build_s, hot_mb, vps) = build_index(&ds.train, ds.dim, 32, ef_c, 1.2);
            print!("{:<8} {:>10.1} {:>10.0} {:>10}", ef_c, build_s, vps, hot_mb);
            for &ef in &ef_probes {
                let (recall, qps) = measure_multi_thread(&index, &eval, ef);
                print!("  {:>6.2}%  {:>6.0}", recall * 100.0, qps);
            }
            println!();
        }
    } // end 1b

    if should_run("1c") {
        // ── 1c: alpha 的影响（固定 m=32, ef_c=128）──
        let alpha_values = [1.0f32, 1.05, 1.1, 1.15, 1.2, 1.25];

        println!("\n--- 1c: α 的影响 (m=32, ef_c=128) ---");
        print!(
            "{:<8} {:>10} {:>10} {:>10}",
            "alpha", "Build(s)", "vecs/s", "Hot(MB)"
        );
        for &ef in &ef_probes {
            print!("  ef={:<4}R  ef={:<4}Q", ef, ef);
        }
        println!();
        println!("{}", "-".repeat(8 + 10 + 10 + 10 + ef_probes.len() * 22));

        for &alpha in &alpha_values {
            let (index, build_s, hot_mb, vps) = build_index(&ds.train, ds.dim, 32, 128, alpha);
            print!(
                "{:<8.2} {:>10.1} {:>10.0} {:>10}",
                alpha, build_s, vps, hot_mb
            );
            for &ef in &ef_probes {
                let (recall, qps) = measure_multi_thread(&index, &eval, ef);
                print!("  {:>6.2}%  {:>6.0}", recall * 100.0, qps);
            }
            println!();
        }
    } // end 1c

    if should_run("1d") {
        // ── 1d: 精细 ef_search Recall-QPS 曲线（默认参数）──
        let ef_search_fine = [
            8, 12, 16, 20, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
            448, 512, 640, 768, 896, 1024,
        ];

        println!("\n--- 1d: ef_search 精细 Recall-QPS 曲线 (m=32, ef_c=128, α=1.2) ---");
        println!(
            "{:<8} {:>10} {:>12} {:>12} {:>10} {:>10}",
            "ef", "R@10(%)", "MT-QPS", "1T-QPS", "lat(ms)", "vs BF"
        );
        println!("{}", "-".repeat(66));

        let (index, _, _, _) = build_index(&ds.train, ds.dim, 32, 128, 1.2);
        for &ef in &ef_search_fine {
            let (recall, qps_mt) = measure_multi_thread(&index, &eval, ef);
            let (_, qps_st) = measure_single_thread(&index, &eval, ef);
            let lat_ms = 1000.0 / qps_st;
            let speedup = qps_mt / bf_qps;
            println!(
                "{:<8} {:>9.2}% {:>12.0} {:>12.0} {:>9.2} {:>9.1}x",
                ef,
                recall * 100.0,
                qps_mt,
                qps_st,
                lat_ms,
                speedup
            );
        }
    } // end 1d

    if should_run("1e") {
        // ── 1e: 不同 m 的完整 Pareto 曲线（论文 Figure 级别）──
        let pareto_m = [8, 16, 32, 48, 64];
        let pareto_ef = [32, 64, 128, 256, 512, 1024];

        println!("\n--- 1e: 不同 m 的 Pareto 曲线 (ef_c=128, α=1.2) ---");
        println!(
            "{:<6} {:<8} {:>10} {:>12} {:>10}",
            "m", "ef", "R@10(%)", "MT-QPS", "vs BF"
        );
        println!("{}", "-".repeat(50));

        for &m in &pareto_m {
            let (index, _, _, _) = build_index(&ds.train, ds.dim, m, 128, 1.2);
            for &ef in &pareto_ef {
                let (recall, qps) = measure_multi_thread(&index, &eval, ef);
                let speedup = qps / bf_qps;
                println!(
                    "{:<6} {:<8} {:>9.2}% {:>12.0} {:>9.1}x",
                    m,
                    ef,
                    recall * 100.0,
                    qps,
                    speedup
                );
            }
            println!();
        }
    } // end 1e

    if should_run("1f") {
        // ── 1f: m × alpha 交叉实验 (ef_c=128, ef_search=64)──
        let cross_m = [8, 16, 32, 48, 64];
        let cross_alpha = [1.0f32, 1.1, 1.15, 1.2, 1.25];

        println!("\n--- 1f: m × α 交叉实验 (ef_c=128, ef_search=64) ---");
        print!("{:<6}", "m\\α");
        for &a in &cross_alpha {
            print!(" {:>8.2}", a);
        }
        println!();
        println!("{}", "-".repeat(6 + cross_alpha.len() * 9));

        for &m in &cross_m {
            print!("{:<6}", m);
            for &alpha in &cross_alpha {
                let (index, _, _, _) = build_index(&ds.train, ds.dim, m, 128, alpha);
                let (recall, _) = measure_multi_thread(&index, &eval, 64);
                print!(" {:>7.2}%", recall * 100.0);
            }
            println!();
        }
    } // end 1f
}

// ============================================================
//  实验 2：单线程 vs 多线程查询
// ============================================================

fn experiment_thread_scaling(ds: &DataSet) {
    println!("\n{}", "=".repeat(70));
    println!("实验 2: 单线程 vs 多线程查询扩展性");
    println!("数据集: {} ({} x {})", ds.name, ds.n_train, ds.dim);
    println!("{}", "=".repeat(70));

    let top_k = 10;
    let eval = SearchEval {
        test: &ds.test,
        train: &ds.train,
        dim: ds.dim,
        n_test: ds.n_test,
        gts: &ds.gts,
        top_k,
    };
    let (index, build_s, hot_mb, vps) = build_index(&ds.train, ds.dim, 32, 128, 1.2);
    println!(
        "索引构建: {:.1}s ({:.0} vecs/s), Hot {} MB",
        build_s, vps, hot_mb
    );

    println!("\n--- 2a: 不同 ef 下的 单线程 vs 多线程 QPS ---");
    println!(
        "{:<8} {:>12} {:>12} {:>12} {:>10}",
        "ef", "1T-QPS", "MT-QPS", "R@10(%)", "加速比"
    );
    println!("{}", "-".repeat(58));

    for &ef in &[64, 128, 256, 512, 1024] {
        let (recall, qps_st) = measure_single_thread(&index, &eval, ef);
        let (_, qps_mt) = measure_multi_thread(&index, &eval, ef);
        let speedup = qps_mt / qps_st;
        println!(
            "{:<8} {:>12.0} {:>12.0} {:>11.2}% {:>9.1}x",
            ef,
            qps_st,
            qps_mt,
            recall * 100.0,
            speedup
        );
    }

    // --- 2b: 固定 ef=64，用 rayon ThreadPool 控制线程数 ---
    println!("\n--- 2b: 线程数扩展性 (ef=64) ---");
    println!("{:<10} {:>12} {:>12}", "线程数", "QPS", "相对1线程");
    println!("{}", "-".repeat(38));

    let ef = 64;
    // 先测单线程基准
    let (_, qps_1t) = measure_single_thread(&index, &eval, ef);
    println!("{:<10} {:>12.0} {:>11.1}x", 1, qps_1t, 1.0);

    let available = rayon::current_num_threads();
    let thread_counts: Vec<usize> = vec![2, 4, 8, 16, 32, 64, 96, 128]
        .into_iter()
        .filter(|&t| t <= available)
        .collect();

    for &num_threads in &thread_counts {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build()
            .unwrap();

        let cfg = QuIVerSearchConfig {
            top_k,
            ef_search: ef,
            rerank_limit: None,
        };

        let t = Instant::now();
        let _hits: usize = pool.install(|| {
            (0..ds.n_test)
                .into_par_iter()
                .map(|i| {
                    let q = &ds.test[i * ds.dim..(i + 1) * ds.dim];
                    let res = index.search_flat(q, &ds.train, &cfg);
                    res.iter()
                        .filter(|&&(id, _)| ds.gts[i].contains(&id))
                        .count()
                })
                .sum()
        });
        let qps = ds.n_test as f64 / t.elapsed().as_secs_f64();
        println!("{:<10} {:>12.0} {:>11.1}x", num_threads, qps, qps / qps_1t);
    }
}

// ============================================================
//  主入口
// ============================================================

fn main() {
    let mode = env_string("TRIVIUM_SENSITIVITY_MODE", "all");
    let ds = load_dataset();

    match mode.as_str() {
        "params" => experiment_param_sensitivity(&ds),
        "threads" => experiment_thread_scaling(&ds),
        _ => {
            experiment_param_sensitivity(&ds);
            experiment_thread_scaling(&ds);
        }
    }

    println!("\n全部实验完成!");
}
