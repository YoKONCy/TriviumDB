//! 独立消融服务：复用冻结图，完整预算、轨迹、原始标签和五轮查询时钟。
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    time::Instant,
};
use triviumdb::index::quiver::{
    QuIVer, QuIVerSearchConfig,
    corrected_sm::{
        CorrectedSmIndex, SmBuildConfig, SmSearchMode,
        ablation::{ExperimentIndex, Options, Score},
    },
};
const ROOT: &str = "evaluation-data";
fn floats(p: &Path) -> Vec<f32> {
    fs::read(p)
        .expect("读取浮点文件失败")
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}
fn options(v: &Value) -> Options {
    let flag = |k: &str, d: bool| v[k].as_bool().unwrap_or(d);
    Options {
        score: match v["score"].as_str().unwrap_or("dot") {
            "bq" => Score::Bq,
            "cos" => Score::Cosine,
            "l2" => Score::L2,
            "dot" => Score::Dot,
            _ => panic!("未知评分"),
        },
        navigation: flag("navigation", true),
        retention: flag("retention", true),
        anchor: flag("anchor", true),
        sm_simd: flag("sm_simd", true),
        reuse: flag("reuse", true),
        batch: flag("batch", true),
        prefetch: flag("prefetch", true),
        heap_replace: flag("heap_replace", true),
        fp32_batch: flag("fp32_batch", true),
        scalar_exact: flag("scalar_exact", false),
        wide: flag("wide", false),
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let out = Path::new(&args[1]);
    let mode = &args[2];
    let base = floats(Path::new(&format!(
        "{ROOT}/adaptation/quiver_cohere200k_ablation_20261017/train.f32"
    )));
    assert_eq!(base.len(), 200000 * 768);
    if mode == "topology" {
        let cfg = SmBuildConfig::default();
        let start = Instant::now();
        let model = CorrectedSmIndex::build_controlled(&base, &[], &cfg, true)?;
        let seconds = start.elapsed().as_secs_f64();
        model.export_research_graph(&out.join("controlled-fp32.bin"))?;
        fs::write(
            out.join("topology-build.json"),
            serde_json::to_vec_pretty(&json!({"n":200000,"bits":32,"seconds":seconds,"threads":1,
            "degree":64,"ef_construction":128,"seed":20260916,"passes":[1.0,1.2],"prune":"alpha*d(v,w)<=d(u,v)","backfill":true,
            "scope":"与冻结2-bit串行研究图完全相同的初始化、节点顺序、双轮、候选与反向更新策略；只替换距离为单位向量FP32 L2"}))?,
        )?;
        return Ok(());
    }
    let prior = PathBuf::from(format!("{ROOT}/adaptation/quiver_tdb_fastpath_20261017_v1"));
    let mut models = std::collections::HashMap::new();
    let index = CorrectedSmIndex::from_research_graph(&prior.join("fast-graph.bin"), &base, &[])?;
    models.insert("fast".to_owned(), ExperimentIndex::new(index));
    if mode == "gate" {
        let queries = floats(Path::new(&format!(
            "{ROOT}/adaptation/quiver_cohere200k_ablation_20261017/test.f32"
        )));
        let e = models.get_mut("fast").unwrap();
        let math_error = e.verify_math(&base, &queries)?;
        let coeff = floats(&prior.join("coeff.f32"));
        e.configure(&coeff, 0)?;
        let reference =
            CorrectedSmIndex::from_research_graph(&prior.join("fast-graph.bin"), &base, &coeff)?;
        let mut verified = 0;
        for q in queries.chunks_exact(768).take(16) {
            for budget in [16, 32, 64, 128] {
                let cfg = QuIVerSearchConfig {
                    top_k: 10,
                    ef_search: 128,
                    rerank_limit: Some(budget),
                };
                let expected = reference.search_traced(q, &base, &cfg, SmSearchMode::Combined)?;
                for change in 0..9 {
                    let mut o = Options::default();
                    match change {
                        1 => o.reuse = false,
                        2 => o.batch = false,
                        3 => o.prefetch = false,
                        4 => o.heap_replace = false,
                        5 => o.fp32_batch = false,
                        6 => o.wide = true,
                        7 => {
                            o.sm_simd = false;
                        }
                        8 => {
                            o.sm_simd = false;
                            o.reuse = false;
                            o.batch = false;
                            o.heap_replace = false;
                            o.fp32_batch = false;
                        }
                        _ => {}
                    }
                    let actual = e.search_experiment(q, &base, &cfg, o, true)?;
                    assert_eq!(
                        actual.neighbors, expected.neighbors,
                        "逐组开关改变标签或分数"
                    );
                    assert_eq!(actual.trace, expected.trace, "逐组开关改变搜索轨迹");
                    assert_eq!(actual.counts, expected.counts, "逐组开关改变工作量");
                    verified += 1;
                }
            }
        }
        let small = &base[..384 * 768];
        let cfg = SmBuildConfig {
            degree: 32,
            ef_construction: 64,
            ..Default::default()
        };
        let old = CorrectedSmIndex::build(small, &[], &cfg)?;
        let controlled = CorrectedSmIndex::build_controlled(small, &[], &cfg, false)?;
        old.export_research_graph(&out.join("gate-original-2bit.bin"))?;
        controlled.export_research_graph(&out.join("gate-controlled-2bit.bin"))?;
        assert_eq!(
            fs::read(out.join("gate-original-2bit.bin"))?,
            fs::read(out.join("gate-controlled-2bit.bin"))?
        );
        fs::write(
            out.join("gate.json"),
            serde_json::to_vec_pretty(&json!({"status":"passed","math_max_error":math_error,
            "full_output_trace_budget_checks":verified,"controlled_2bit_matches_original":true,"gate_nodes":384,
            "notes":"显式原BQ、重建内积、余弦、L2数学式独立核对；组合与逐组关闭全轨迹和预算一致"}))?,
        )?;
        return Ok(());
    }
    if mode == "topology-serve" {
        for (name, path) in [
            (
                "controlled_bq",
                PathBuf::from(format!(
                    "{ROOT}/adaptation/quiver_cohere200k_ablation_20261017/graph.bin"
                )),
            ),
            ("controlled_fp32", out.join("controlled-fp32.bin")),
        ] {
            models.insert(
                name.to_owned(),
                ExperimentIndex::new(CorrectedSmIndex::from_research_graph(&path, &base, &[])?),
            );
        }
    }
    let actual_bq = QuIVer::load_from_file(&prior.join("fast-raw.index"))?;
    println!(
        "{}",
        json!({"ready":true,"models":models.keys().collect::<Vec<_>>(),"nodes":200000})
    );
    io::stdout().flush()?;
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line == "EXIT" {
            break;
        }
        let v: Value = serde_json::from_str(&line)?;
        let q = floats(Path::new(v["qfile"].as_str().ok_or("缺查询文件")?));
        let coeff = floats(Path::new(v["coefficient"].as_str().ok_or("缺系数文件")?));
        let model = models
            .get_mut(v["graph"].as_str().unwrap_or("fast"))
            .ok_or("缺图")?;
        model.configure(&coeff, v["anchor_id"].as_u64().unwrap_or(0) as u32)?;
        let o = options(&v["options"]);
        let cfg = QuIVerSearchConfig {
            top_k: 10,
            ef_search: v["ef"].as_u64().ok_or("缺窗口")? as usize,
            rerank_limit: Some(v["budget"].as_u64().unwrap_or(64) as usize),
        };
        let diagnostic = v["diagnostic"].as_bool().unwrap_or(false);
        let trace = v["trace"].as_bool().unwrap_or(false);
        let original = v["actual_bq"].as_bool().unwrap_or(false);
        let seconds = v["seconds"].as_f64().unwrap_or(0.);
        assert!((0. ..=2.).contains(&seconds));
        let one = |query: &[f32]| {
            if original {
                let mut calls = 0;
                let neighbors = actual_bq.search(
                    query,
                    |slot, buffer| {
                        calls += 1;
                        buffer.clear();
                        buffer.extend_from_slice(&base[slot * 768..(slot + 1) * 768]);
                        true
                    },
                    &cfg,
                );
                (
                    neighbors
                        .into_iter()
                        .map(|(id, s)| (id - 1, s))
                        .collect::<Vec<_>>(),
                    [0, 0, 0, 0, calls],
                    json!({}),
                )
            } else {
                let r = model
                    .search_experiment(query, &base, &cfg, o, trace)
                    .expect("消融查询失败");
                (
                    r.neighbors,
                    r.counts,
                    if trace {
                        json!({"expanded":r.trace.expanded,"scored":r.trace.scored,"candidates":r.trace.candidates,"exact":r.trace.exact})
                    } else {
                        Value::Null
                    },
                )
            }
        };
        let batch = || q.chunks_exact(768).map(one).collect::<Vec<_>>();
        let (last, elapsed, count) = if diagnostic {
            (batch(), 0., 0)
        } else {
            std::hint::black_box(batch());
            let begin = Instant::now();
            let mut count = 0;
            let last = loop {
                let out = batch();
                count += out.len();
                std::hint::black_box(&out);
                if begin.elapsed().as_secs_f64() >= seconds {
                    break out;
                }
            };
            (last, begin.elapsed().as_secs_f64(), count)
        };
        println!(
            "{}",
            json!({"labels":last.iter().map(|r|r.0.iter().map(|v|v.0).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "score_bits":last.iter().map(|r|r.0.iter().map(|v|v.1.to_bits()).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "work":last.iter().map(|r|r.1).collect::<Vec<_>>(),"trace":last.iter().map(|r|&r.2).collect::<Vec<_>>(),
            "seconds":elapsed,"queries_executed":count})
        );
        io::stdout().flush()?;
    }
    Ok(())
}
