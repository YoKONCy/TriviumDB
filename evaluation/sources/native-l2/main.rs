//! QuIVer二十一库正式测量包装；使用冻结主库生产入口。
use rayon::prelude::*;
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, Write},
    path::Path,
    time::Instant,
};
use triviumdb::{
    index::bq::{Bq2Signature, Bq2Store},
    index::quiver::{
        BqNavigationScorer, NavigationScorer, QuIVer, QuIVerConfig, QuIVerSearchConfig,
    },
    storage::{memtable::MemTable, vec_pool::VecPool},
};

fn sign(v: &[f32]) -> [u64; 12] {
    let mut out = [0u64; 12];
    for (i, &x) in v.iter().enumerate() {
        if x > 0. {
            out[i / 64] |= 1 << (i % 64);
        }
    }
    out
}
fn scalar(v: &[f32]) -> Vec<u8> {
    let tau = v.iter().map(|x| x.abs()).sum::<f32>() / v.len() as f32;
    v.iter()
        .map(|&x| {
            if x <= -tau {
                0
            } else if x <= 0. {
                1
            } else if x <= tau {
                2
            } else {
                3
            }
        })
        .collect()
}
struct SignScore<'a>(&'a [[u64; 12]], [u64; 12]);
impl NavigationScorer for SignScore<'_> {
    fn score(&mut self, id: u64, _raw: u32) -> u32 {
        self.0[id as usize - 1]
            .iter()
            .zip(self.1)
            .map(|(&a, b)| (a ^ b).count_ones())
            .sum()
    }
}
struct ScalarScore<'a>(&'a [Vec<u8>], Vec<u8>);
impl NavigationScorer for ScalarScore<'_> {
    fn score(&mut self, id: u64, _raw: u32) -> u32 {
        self.0[id as usize - 1]
            .iter()
            .zip(&self.1)
            .map(|(&a, &b)| a.abs_diff(b) as u32)
            .sum()
    }
}

#[repr(C)]
#[derive(Default)]
struct MallInfo { arena: usize, ordblks: usize, smblks: usize, hblks: usize, hblkhd: usize,
    usmblks: usize, fsmblks: usize, uordblks: usize, fordblks: usize, keepcost: usize }
fn allocator() -> Value {
    let m = unsafe { mallinfo2() };
    json!({"arena_bytes":m.arena,"mmap_allocator_bytes":m.hblkhd,"in_use_arena_bytes":m.uordblks,
        "free_arena_bytes":m.fordblks,"in_use_plus_allocator_mmap_bytes":m.uordblks+m.hblkhd,
        "note":"glibc分配器计数，包含未逐对象归属的运行时；不含显式FP32文件映射"})
}
unsafe extern "C" {
    fn mallinfo2() -> MallInfo;
    fn mincore(addr: *mut std::ffi::c_void, len: usize, vec: *mut u8) -> i32;
    fn posix_fadvise(fd: i32, offset: i64, len: i64, advice: i32) -> i32;
    fn malloc_trim(pad: usize) -> i32;
}
fn floats(path: &Path) -> Vec<f32> {
    let file = fs::File::open(path).expect("打开向量失败");
    let map = unsafe {
        memmap2::MmapOptions::new()
            .map(&file)
            .expect("映射向量失败")
    };
    assert_eq!(map.len() % 4, 0);
    map.chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}
fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}
fn residency(pool: &VecPool<f32>, n: usize, dim: usize) -> Value {
    if pool.mmap_count() == 0 {
        return Value::Null;
    }
    let addr = pool.get(0).unwrap().as_ptr() as usize;
    assert_eq!(addr % 4096, 0);
    let bytes = n * dim * 4;
    let mut pages = vec![0u8; bytes.div_ceil(4096)];
    let rc = unsafe { mincore(addr as *mut std::ffi::c_void, bytes, pages.as_mut_ptr()) };
    assert_eq!(rc, 0, "驻留页检查失败");
    let resident = pages.iter().filter(|&&p| p & 1 != 0).count();
    json!({"pages":pages.len(),"resident_pages":resident,"resident_fraction":resident as f64/pages.len() as f64})
}
fn evict(pool: &VecPool<f32>, path: &Path) {
    use std::os::fd::AsRawFd;
    assert!(pool.mmap_count() > 0, "冷页试验只允许独立的文件映射副本");
    pool.advise_dontneed();
    let file = fs::File::open(path).unwrap();
    let rc = unsafe { posix_fadvise(file.as_raw_fd(), 0, 0, 4) };
    assert_eq!(rc, 0, "独立副本的冷页释放失败");
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    let cfgfile = Path::new(args.get(2).ok_or("缺少任务配置")?);
    let c: Value = serde_json::from_slice(&fs::read(cfgfile)?)?;
    let dim = c["dim"].as_u64().unwrap() as usize;
    let n = c["n"].as_u64().unwrap() as usize;
    let out = Path::new(c["out"].as_str().unwrap());
    let basepath = Path::new(c["base"].as_str().unwrap());
    if args[1] == "smoke" {
        return smoke(&c);
    }
    if args[1] == "build" {
        let begin = Instant::now();
        assert!(cfg!(target_endian = "little"), "输入要求小端平台");
        let input = fs::File::open(basepath)?;
        let input_map = unsafe { memmap2::MmapOptions::new().map(&input)? };
        assert_eq!(input_map.len(), n * dim * 4);
        let base = unsafe { std::slice::from_raw_parts(input_map.as_ptr() as *const f32, n * dim) };
        let load_s = begin.elapsed().as_secs_f64();
        let mut mt = MemTable::<f32>::new(dim);
        mt.set_auto_build_quiver(false);
        let begin = Instant::now();
        for (u, row) in base.chunks_exact(dim).enumerate() {
            assert_eq!(mt.insert(row, json!({}))?, u as u64 + 1);
        }
        let ingest_s = begin.elapsed().as_secs_f64();
        drop(input_map);
        let threads = c["threads"].as_u64().unwrap() as usize;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()?;
        let config = QuIVerConfig {
            m: 32,
            ef_construction: 128,
            alpha: 1.2,
        };
        eprintln!(
            "开始正式构图：节点{}，线程{}，剪枝{}",
            n, threads, c["prune"]
        );
        let begin = Instant::now();
        if c["encoding"] == "sign" {
            let mut store = Bq2Store::new(dim);
            store.reserve(n);
            for row in mt.vec_pool().delta_raw().chunks_exact(dim) {
                let mut sig = Bq2Signature {
                    pos: [0u64; 48],
                    strong: [0u64; 48],
                };
                for (j, &x) in row.iter().enumerate() {
                    if x > 0. {
                        sig.pos[j / 64] |= 1 << (j % 64);
                    }
                }
                store.push_sig(&sig);
            }
            let mut graph = QuIVer::new(dim, &config);
            let ids = (1..=n as u64).collect::<Vec<_>>();
            let slots = (0..n).collect::<Vec<_>>();
            pool.install(|| {
                graph.batch_build_with_store(mt.vec_pool().delta_raw(), &ids, &slots, store)
            });
            let build_s = begin.elapsed().as_secs_f64();
            let save_begin = Instant::now();
            graph.save_to_file(&out.join("index.bin"))?;
            let st = graph.stats();
            let record = json!({"config":c,"input_load_s":load_s,"ingest_s":ingest_s,
                "build_s":build_s,"save_s":save_begin.elapsed().as_secs_f64(),"count":graph.total_count(),"avg_degree":st.avg_degree_l0,
                "timing":"1-bit质量诊断图；包含签名构建；与2-bit同一batch_build_from_store并发剪枝，查询另测"});
            write_json(&out.join("result.json"), &record);
            return Ok(());
        } else {
            pool.install(|| mt.build_quiver(&config));
        }
        let build_s = begin.elapsed().as_secs_f64();
        let index = mt.quiver().ok_or("构图没有返回索引")?;
        let before = Instant::now();
        index.save_to_file(&out.join("index.bin"))?;
        let save_s = before.elapsed().as_secs_f64();
        let st = index.stats();
        let before = Instant::now();
        if c["route"] == "l2" {
            mt.enable_quiver_sm_l2()?;
        } else if c["route"] == "native_l2" {
            mt.enable_quiver_sm_native_l2()?;
        }
        let install_s = before.elapsed().as_secs_f64();
        let q = floats(Path::new(c["queries"].as_str().unwrap()));
        let search = QuIVerSearchConfig {
            top_k: 10,
            ef_search: 128,
            rerank_limit: Some(64),
        };
        let index = mt.quiver().unwrap();
        let neighbors = pool.install(|| {
            q.par_chunks_exact(dim)
                .map(|v| {
                    index.search(
                        v,
                        |slot, buffer| {
                            buffer.clear();
                            buffer.extend_from_slice(mt.vec_pool().get(slot).unwrap());
                            true
                        },
                        &search,
                    )
                })
                .collect::<Vec<_>>()
        });
        let record = json!({"config":c,"input_load_s":load_s,"ingest_s":ingest_s,"build_s":build_s,
            "save_s":save_s,"install_s":install_s,"count":index.total_count(),"hot_stats_bytes":st.hot_bytes,
            "avg_degree":st.avg_degree_l0,"routed_memory":index.corrected_sm_memory_components(),
            "labels":neighbors.iter().map(|r|r.iter().map(|p|p.0-1).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "score_bits":neighbors.iter().map(|r|r.iter().map(|p|p.1.to_bits()).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "timing":"build_s仅包括实际MemTable::build_quiver调用；不含输入、导入、保存及查询派生状态安装"});
        write_json(&out.join("result.json"), &record);
        return Ok(());
    }
    assert_eq!(args[1], "serve");
    let mut vectors = if c["storage"] == "mmap" {
        VecPool::open(basepath, dim, n)?
    } else {
        let base = floats(basepath);
        assert_eq!(base.len(), n * dim);
        let mut pool = VecPool::new(dim);
        for row in base.chunks_exact(dim) {
            pool.push(row);
        }
        pool
    };
    if vectors.mmap_count() > 0 {
        vectors.advise_random();
    }
    let quality = c["quality_encodings"].as_bool().unwrap_or(false);
    let signs = if quality {
        (0..n)
            .map(|u| sign(vectors.get(u).unwrap()))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let scalars = if quality {
        (0..n)
            .map(|u| scalar(vectors.get(u).unwrap()))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let begin = Instant::now();
    let mut index = QuIVer::load_from_file(Path::new(c["index"].as_str().unwrap()))?;
    let load_s = begin.elapsed().as_secs_f64();
    assert_eq!(index.total_count(), n);
    let loaded_status = fs::read_to_string("/proc/self/status")?;
    let loaded_allocator = allocator();
    let before = index.stats().hot_bytes;
    let begin = Instant::now();
    if c["route"] == "l2" {
        index.enable_sm_l2_with_vectors(|slot, buffer| {
            buffer.clear();
            buffer.extend_from_slice(vectors.get(slot).unwrap());
            true
        })?;
        assert!(index.sm_l2_enabled());
    }
    if c["route"] == "native_l2" {
        index.enable_sm_native_l2_with_vectors(|slot, buffer| {
            buffer.clear();
            buffer.extend_from_slice(vectors.get(slot).unwrap());
            true
        })?;
        assert!(index.sm_native_l2_enabled());
    }
    let install_s = begin.elapsed().as_secs_f64();
    unsafe {
        malloc_trim(0);
    }
    let metadata = json!({"ready":true,"pid":std::process::id(),"config":c,"index_load_s":load_s,
        "install_s":install_s,"raw_hot_stats_bytes":before,"routed_hot_stats_bytes":index.stats().hot_bytes,
        "loaded_status":loaded_status,"loaded_allocator":loaded_allocator,"installed_allocator":allocator(),"installed_status":fs::read_to_string("/proc/self/status")?,"routed_memory":index.corrected_sm_memory_components(),"vector_heap_logical_bytes":vectors.heap_memory_bytes(),
        "vector_mmap_bytes":vectors.mmap_bytes(),"residency":residency(&vectors,n,dim),
        "timing":"真实QuIVer查询入口和VecPool冷向量回调；不含Database融合层与TQL"});
    println!("{}", metadata);
    io::stdout().flush()?;
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line == "EXIT" {
            break;
        }
        let request: Value = serde_json::from_str(&line)?;
        let query = floats(Path::new(request["qfile"].as_str().unwrap()));
        assert_eq!(query.len() % dim, 0);
        let threads = request["threads"].as_u64().unwrap() as usize;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()?;
        let search = QuIVerSearchConfig {
            top_k: 10,
            ef_search: request["ef"].as_u64().unwrap() as usize,
            rerank_limit: Some(request["budget"].as_u64().unwrap_or(64) as usize),
        };
        let mode = request["mode"].as_str().unwrap();
        let kind = request["kind"].as_str().unwrap();
        let one = |q: &[f32]| {
            let fill = |slot: usize, buffer: &mut Vec<f32>| {
                buffer.clear();
                buffer.extend_from_slice(vectors.get(slot).unwrap());
                true
            };
            if mode == "sign" {
                assert!(quality);
                index.search_with_scorer(q, fill, &search, &mut SignScore(&signs, sign(q)))
            } else if mode == "scalar" {
                assert!(quality);
                index.search_with_scorer(q, fill, &search, &mut ScalarScore(&scalars, scalar(q)))
            } else if mode == "bq" {
                index.search_with_scorer(q, fill, &search, &mut BqNavigationScorer)
            } else {
                assert!(index.sm_l2_enabled());
                assert_eq!(index.sm_native_l2_enabled(), mode == "native_l2");
                index.search(q, fill, &search)
            }
        };
        let batch = || pool.install(|| query.par_chunks_exact(dim).map(one).collect::<Vec<_>>());
        let mut latencies = Vec::new();
        let mut cold_residencies = Vec::new();
        let mut count = 0usize;
        let mut elapsed = 0.;
        let mut before = Value::Null;
        let mut os_before = String::new();
        let neighbors = if kind == "cold_query" {
            assert_eq!(threads, 1);
            let mut results = Vec::new();
            for q in query.chunks_exact(dim) {
                evict(&vectors, basepath);
                cold_residencies.push(residency(&vectors, n, dim));
                assert!(
                    cold_residencies.last().unwrap()["resident_fraction"]
                        .as_f64()
                        .unwrap()
                        < 0.01,
                    "冷页未释放，停止计量"
                );
                let start = Instant::now();
                let r = one(q);
                latencies.push(start.elapsed().as_nanos() as u64);
                results.push(r);
            }
            results
        } else if kind == "cold_batch" {
            evict(&vectors, basepath);
            before = residency(&vectors, n, dim);
            assert!(
                before["resident_fraction"].as_f64().unwrap() < 0.01,
                "冷页未释放，停止计量"
            );
            os_before = fs::read_to_string("/proc/self/io")?;
            let start = Instant::now();
            let r = batch();
            elapsed = start.elapsed().as_secs_f64();
            count = r.len();
            r
        } else if kind == "latency" {
            pool.install(|| {
                query
                    .par_chunks_exact(dim)
                    .map(|q| {
                        let start = Instant::now();
                        let result = one(q);
                        (result, start.elapsed().as_nanos() as u64)
                    })
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .map(|(r, ns)| {
                latencies.push(ns);
                r
            })
            .collect::<Vec<_>>()
        } else if kind == "throughput" {
            std::hint::black_box(batch());
            let start = Instant::now();
            loop {
                let r = batch();
                count += r.len();
                std::hint::black_box(&r);
                if start.elapsed().as_secs_f64() >= request["seconds"].as_f64().unwrap_or(0.25) {
                    elapsed = start.elapsed().as_secs_f64();
                    break r;
                }
            }
        } else {
            assert_eq!(kind, "diagnostic");
            batch()
        };
        println!(
            "{}",
            json!({"labels":neighbors.iter().map(|r|r.iter().map(|p|p.0-1).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "score_bits":neighbors.iter().map(|r|r.iter().map(|p|p.1.to_bits()).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "latency_ns":latencies,"seconds":elapsed,"queries_executed":count,"request":request,
            "residency_before":before,"residency_after":residency(&vectors,n,dim),
            "cold_query_residencies":cold_residencies,"io_before":os_before,"io_after":fs::read_to_string("/proc/self/io")?,
            "allocator_after":allocator(),"status_after":fs::read_to_string("/proc/self/status")?,"stat_after":fs::read_to_string("/proc/self/stat")?})
        );
        io::stdout().flush()?;
    }
    // 防止误把包装同时保留的对照状态作为多个部署实例的内存。
    vectors.advise_dontneed();
    Ok(())
}

fn smoke(c: &Value) -> Result<(), Box<dyn std::error::Error>> {
    use triviumdb::index::quiver::corrected_sm::{CorrectedSmIndex, SmSearchMode};
    let dim = c["dim"].as_u64().unwrap() as usize;
    let n = c["n"].as_u64().unwrap() as usize;
    let base = floats(Path::new(c["base"].as_str().unwrap()));
    let queries = floats(Path::new(c["queries"].as_str().unwrap()));
    assert_eq!(base.len(), n * dim);
    let mut graph = QuIVer::load_from_file(Path::new(c["index"].as_str().unwrap()))?;
    let mut reference = graph.corrected_sm_snapshot(&base, &[])?;
    reference.use_reconstructed_l2();
    graph.enable_sm_l2_with_vectors(|slot, b| {
        b.extend_from_slice(&base[slot * dim..(slot + 1) * dim]);
        true
    })?;
    assert!(graph.sm_l2_enabled());
    let cfg = QuIVerSearchConfig {
        top_k: 10,
        ef_search: 128,
        rerank_limit: Some(64),
    };
    let signature = |r: &[(u64, f32)]| {
        r.iter()
            .map(|&(id, s)| (id, s.to_bits()))
            .collect::<Vec<_>>()
    };
    let mut expected = Vec::new();
    let mut traces = Vec::new();
    for q in queries.chunks_exact(dim) {
        let scalar = reference.search_traced(q, &base, &cfg, SmSearchMode::Reference)?;
        let simd = reference.search_traced(q, &base, &cfg, SmSearchMode::Combined)?;
        assert_eq!(signature(&scalar.neighbors), signature(&simd.neighbors));
        assert_eq!(scalar.trace, simd.trace);
        assert_eq!(scalar.counts, simd.counts);
        expected.push(signature(&scalar.neighbors));
        traces.push(json!({"counts":scalar.counts,"expanded":scalar.trace.expanded,"scored":scalar.trace.scored,"candidates":scalar.trace.candidates,"exact":scalar.trace.exact}));
    }
    let path = Path::new(c["out"].as_str().unwrap()).join("portable.sm");
    reference.save_portable(&path)?;
    let restored = CorrectedSmIndex::load(&path, &base)?;
    assert_eq!(restored.dimension(), dim);
    for (i, q) in queries.chunks_exact(dim).enumerate() {
        assert_eq!(
            signature(
                &restored
                    .search(q, &base, &cfg, SmSearchMode::Combined)?
                    .neighbors
            ),
            expected[i]
        );
    }
    for threads in [1usize, 8, 16, 32] {
        let actual = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()?
            .install(|| {
                queries
                    .par_chunks_exact(dim)
                    .map(|q| {
                        let mut access = Vec::new();
                        let r = graph.search(
                            q,
                            |slot, b| {
                                access.push(slot as u32);
                                b.extend_from_slice(&base[slot * dim..(slot + 1) * dim]);
                                true
                            },
                            &cfg,
                        );
                        (signature(&r), access)
                    })
                    .collect::<Vec<_>>()
            });
        for (i, (labels, access)) in actual.iter().enumerate() {
            assert_eq!(*labels, expected[i]);
            assert_eq!(json!(access), traces[i]["exact"]);
            assert_eq!(access.len(), 64);
        }
    }
    let report = json!({"status":"passed","dim":dim,"n":n,"queries":expected.len(),"threads":[1,8,16,32],"labels_and_score_bits":expected,"traces":traces,"memory":graph.corrected_sm_memory_components(),"static_memory":reference.memory_components(),"portable_bytes":fs::metadata(path)?.len(),"note":"真实数据前缀仅作工程正确性；完整标量与SIMD轨迹、公共TDB路径、精排访问及重载一致，不作为正式召回或QPS成绩"});
    write_json(
        &Path::new(c["out"].as_str().unwrap()).join("smoke-result.json"),
        &report,
    );
    println!("真实1536维公共路径与静态快照完整门禁通过");
    Ok(())
}
