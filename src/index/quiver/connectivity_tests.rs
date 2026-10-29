//! 独立 BFS 验证入口可达性，覆盖剪枝、退化输入和增量生命周期。

use super::*;
use std::collections::VecDeque;

fn reachable(index: &QuIVer) -> usize {
    if index.n == 0 {
        return 0;
    }
    let mut seen = vec![false; index.n];
    let mut queue = VecDeque::from([index.entry_point]);
    seen[index.entry_point as usize] = true;
    while let Some(node) = queue.pop_front() {
        for &neighbor in index.layer0.neighbors(node) {
            if !seen[neighbor as usize] {
                seen[neighbor as usize] = true;
                queue.push_back(neighbor);
            }
        }
    }
    seen.into_iter().filter(|&value| value).count()
}

fn check_tree(index: &QuIVer) {
    assert_eq!(reachable(index), index.n, "所有节点必须从实际入口可达");
    assert_eq!(index.connectivity_parents.len(), index.n);
    for node in 0..index.n {
        let parent = index.connectivity_parents[node];
        if node == index.entry_point as usize {
            assert_eq!(parent, EMPTY_NB, "入口必须是导航树的根");
        } else {
            assert!(parent < index.n as u32, "每个非入口节点都必须有父节点");
            assert!(index.layer0.contains(parent, node as u32), "树边不能被剪掉");
        }
    }
    let mut children = vec![Vec::new(); index.n];
    for (node, &parent) in index.connectivity_parents.iter().enumerate() {
        if parent != EMPTY_NB {
            children[parent as usize].push(node);
        }
    }
    let mut pending = vec![index.entry_point as usize];
    let mut tree_nodes = 0;
    while let Some(node) = pending.pop() {
        tree_nodes += 1;
        assert!(tree_nodes <= index.n, "导航树不能包含环");
        pending.extend(&children[node]);
    }
    assert_eq!(tree_nodes, index.n, "被保护的树边本身必须覆盖全图");
    index.validate_layer0();
}

fn vectors(n: usize, dim: usize, degenerate: bool, copies: usize) -> Vec<f32> {
    let mut seed = 918273_u64;
    let mut rows = Vec::with_capacity(n * dim);
    for id in 1..=n {
        let angle = id as f32 * 0.013;
        let pattern = [angle.cos(), angle.sin(), 0.25];
        for column in 0..dim {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            rows.push(if degenerate {
                pattern[column % 3]
            } else {
                ((seed >> 32) as u32 as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32
            });
        }
    }
    for row in 1..copies {
        for column in 0..dim {
            rows[row * dim + column] =
                rows[column] + row as f32 * 1e-7 * (column + 1) as f32 / dim as f32;
        }
    }
    rows
}

fn build(rows: &[f32], dim: usize) -> QuIVer {
    let n = rows.len() / dim;
    let mut store = Bq2Store::new(dim);
    for row in rows.chunks_exact(dim) {
        store.push_from_vector(row);
    }
    let mut index = QuIVer::new(dim, &QuIVerConfig::default());
    index.batch_build_from_store(
        &(1..=n as u64).collect::<Vec<_>>(),
        &(0..n).collect::<Vec<_>>(),
        store,
    );
    index
}

#[test]
fn final_prune_retains_signature_exits_and_repair_restores_orphaned_nodes() {
    let n = 66;
    let config = QuIVerConfig::default();
    let mut index = QuIVer::new(65, &config);
    index.n = n;
    index.layer0.reset_full(n);
    let mut rows = vec![1.0f32; n * 65];
    rows[64 * 65..].fill(-1.0);
    for row in rows.as_chunks::<65>().0 {
        index.bq_store.push_from_vector(row);
    }
    let locks = StripedSpinLocks::new(n, index.m0);
    let adj = ConcurrentFlatAdj::new(n, index.layer0.stride);
    adj.set_neighbors_raw(0, &(1..=64).collect::<Vec<_>>());
    for node in 1..64 {
        adj.set_neighbors_raw(node, &[0]);
    }
    adj.set_neighbors_raw(64, &[65]);
    adj.set_neighbors_raw(65, &[0]);
    let profile = BuildProfile::new();
    let view = ExperimentalBuildView {
        adj: &adj,
        n,
        dim: 65,
        m0: index.m0,
        ef: 128,
        alpha: config.alpha,
        sigs: &index.bq_store,
        locks: &locks,
        profile: &profile,
        eager: false,
    };
    let mut candidates = (1..=64)
        .map(|node| {
            (
                index.bq_store.distance_in_bounds(0, node as usize, 65),
                node,
            )
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|&(distance, _)| distance);
    let legacy = QuIVer::vamana_select(&index.bq_store, 0, &candidates, 32, 65, 1.2);
    assert!(!legacy.contains(&64), "原剪枝会删除唯一跨簇出边");
    view.final_prune(0);
    assert!(
        adj.neighbors_raw(0).contains(&64),
        "修复必须保留不同签名的出口"
    );
    adj.freeze_into_flat(&mut index.layer0);
    assert!(reachable(&index) < n, "应保留对历史剪枝缺陷的复现");
    let original = (0..n)
        .map(|node| index.layer0.neighbors(node as u32).to_vec())
        .collect::<Vec<_>>();
    assert!(index.repair_layer0_connectivity() > 0);
    check_tree(&index);
    for (node, neighbors) in original.iter().enumerate() {
        for &neighbor in neighbors {
            assert!(
                index.layer0.contains(node as u32, neighbor),
                "修复不应删除原近邻"
            );
        }
    }
}

#[test]
fn signature_comparison_includes_every_tail_chunk() {
    for dim in [3, 65, 768, 3072] {
        let original = vec![1.0f32; dim];
        let mut changed = original.clone();
        changed[dim - 1] = -1.0;
        let mut changed_magnitude = original.clone();
        changed_magnitude[dim - 1] = 2.0;
        let mut store = Bq2Store::new(dim);
        store.push_from_vector(&original);
        store.push_from_vector(&original);
        store.push_from_vector(&changed);
        store.push_from_vector(&changed_magnitude);
        assert!(store.same_signature(0, 1), "相同向量的签名必须相同");
        assert!(!store.same_signature(0, 2), "不能忽略尾部的签名差异");
        assert!(!store.same_signature(0, 3), "不能只比较符号而忽略强弱位");
    }
}

#[test]
fn low_intrinsic_dimension_and_near_duplicates_are_reachable_in_one_and_four_threads() {
    for threads in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            for dim in [3, 65, 768, 3072] {
                for degenerate in [false, true] {
                    check_tree(&build(&vectors(2048, dim, degenerate, 0), dim));
                }
            }
            check_tree(&build(&vectors(2048, 65, false, 64), 65));
        });
    }
}

#[test]
fn already_connected_graph_is_unchanged_and_full_rows_keep_tree_edges() {
    let mut index = QuIVer::new(3, &QuIVerConfig::default());
    index.n = 80;
    index.layer0.reset_full(index.n);
    for node in 0..index.n {
        let neighbors = (1..=64)
            .map(|offset| ((node + offset) % index.n) as u32)
            .collect::<Vec<_>>();
        index.layer0.set_neighbors(node as u32, &neighbors);
    }
    let original = index.layer0.data.clone();
    assert_eq!(index.repair_layer0_connectivity(), 0);
    assert_eq!(index.layer0.data, original, "正常图的拓扑必须逐边不变");
    check_tree(&index);
    // 根节点的 64 条边全是树边；更近的新候选不能挤掉任何一条。
    for node in 0..index.n {
        let value = if node == 0 || node == 79 { 1.0 } else { -1.0 };
        index.bq_store.push_from_vector(&[value; 3]);
    }
    let mut candidates = (1..=64)
        .chain([79])
        .map(|node| (index.bq_store.distance_in_bounds(0, node as usize, 3), node))
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|&(distance, _)| distance);
    let selected = index.select_layer0_neighbors(0, &candidates);
    assert_eq!(selected.len(), 64, "保护树边不能扩大原邻接表容量");
    assert!(!selected.contains(&79), "容量用尽时必须先移除非树边");
    index.layer0.set_neighbors(0, &selected);
    check_tree(&index);
    index.layer0.set_neighbors(0, &[1]);
    index.layer0.set_neighbors(1, &[0]);
    assert!(index.repair_layer0_connectivity() > 0);
    check_tree(&index);
}

#[test]
fn incremental_insertion_promoted_entry_and_tombstones_preserve_the_navigation_tree() {
    let dim = 65;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let original = vectors(512, dim, false, 64);
    let mut index = pool.install(|| build(&original, dim));
    index
        .prepare_default_sm(|slot, buffer| {
            buffer.clear();
            buffer.extend_from_slice(&original[slot * dim..(slot + 1) * dim]);
            true
        })
        .unwrap();
    let mut lcg = 12345;
    let mut promotions = 0;
    let mut appended = Vec::new();
    for offset in 0..160 {
        let row = vectors(1, dim, false, 0);
        let previous_entry = index.entry_point;
        index.insert(&row, 513 + offset as u64, 512 + offset, &mut lcg);
        promotions += usize::from(index.entry_point != previous_entry);
        check_tree(&index);
        appended.extend(row);
    }
    assert!(promotions > 0, "必须覆盖插入导致入口升层的情况");
    for id in 1..=96 {
        assert!(index.soft_delete(id));
    }
    check_tree(&index);
    index.batch_build(
        &appended[..dim * 32],
        &(1001..=1032).collect::<Vec<_>>(),
        &(1000..1032).collect::<Vec<_>>(),
    );
    check_tree(&index);
}

#[test]
fn rebuilt_index_roundtrip_preserves_topology_and_incremental_insertion() {
    let index = build(&vectors(96, 3, true, 0), 3);
    assert_eq!(index.n, 96);
    assert_eq!(index.ids, (1..=96).collect::<Vec<_>>());
    assert_eq!(index.slot_indices, (0..96).collect::<Vec<_>>());
    check_tree(&index);
    let path = std::env::temp_dir().join(format!(
        "triviumdb-connectivity-{}-{}.quiver",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    index.save_to_file(&path).unwrap();
    let saved = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(&saved[..8], b"QUIV\x01\x00\x00\x00", "磁盘格式保持 v1");
    let mut restored = QuIVer::load_from_bytes(&saved).unwrap();
    assert_eq!(
        index.layer0.data, restored.layer0.data,
        "加载不得修改已构建的图拓扑"
    );
    check_tree(&restored);
    let mut lcg = 12345;
    restored.insert(&[0.0, 1.0, 0.25], 97, 96, &mut lcg);
    check_tree(&restored);
}

#[test]
fn disconnected_index_requires_rebuild_instead_of_load_time_repair() {
    let mut index = build(&vectors(96, 3, true, 0), 3);
    index.layer0.set_neighbors(index.entry_point, &[]);
    let topology = index.layer0.data.clone();
    let error = index.restore_navigation_tree().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("需要重建索引"));
    assert_eq!(index.layer0.data, topology, "验证失败也不能补边");
    let path = std::env::temp_dir().join(format!(
        "triviumdb-disconnected-{}-{}.quiver",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    index.save_to_file(&path).unwrap();
    let saved = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let error = QuIVer::load_from_bytes(&saved).err().unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("需要重建索引"));
}

#[test]
fn default_sm_full_width_search_matches_independent_cosine_reference() {
    for (dim, degenerate, copies) in [(3, true, 0), (65, false, 64)] {
        let rows = vectors(512, dim, degenerate, copies);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let mut index = pool.install(|| build(&rows, dim));
        index
            .prepare_default_sm(|slot, buffer| {
                buffer.clear();
                buffer.extend_from_slice(&rows[slot * dim..(slot + 1) * dim]);
                true
            })
            .unwrap();
        let query = &rows[400 * dim..401 * dim];
        let cosine = |row: &[f32]| {
            let (mut dot, mut norm_a, mut norm_b) = (0., 0., 0.);
            for (&a, &b) in query.iter().zip(row) {
                let (a, b) = (a as f64, b as f64);
                dot += a * b;
                norm_a += a * a;
                norm_b += b * b;
            }
            dot / (norm_a * norm_b).sqrt()
        };
        let mut reference = rows.chunks_exact(dim).map(cosine).collect::<Vec<_>>();
        reference.sort_unstable_by(|a, b| b.total_cmp(a));
        let mut fetched = 0;
        let found = index.search(
            query,
            |slot, buffer| {
                fetched += 1;
                buffer.clear();
                buffer.extend_from_slice(&rows[slot * dim..(slot + 1) * dim]);
                true
            },
            &QuIVerSearchConfig {
                top_k: 10,
                ef_search: 512,
                rerank_limit: Some(512),
            },
        );
        assert_eq!(fetched, 512, "全宽度查询必须真正遍历并精排全部节点");
        assert_eq!(found.len(), 10);
        for (id, _) in found {
            let score = cosine(&rows[(id as usize - 1) * dim..id as usize * dim]);
            assert!(
                score + 1e-6 >= reference[9],
                "返回结果必须满足独立余弦 Top-10 边界"
            );
        }
    }
}

#[test]
fn near_duplicate_cluster_does_not_block_bounded_sm_search() {
    let dim = 65;
    let rows = vectors(2048, dim, false, 64);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let mut index = pool.install(|| build(&rows, dim));
    index
        .prepare_default_sm(|slot, buffer| {
            buffer.clear();
            buffer.extend_from_slice(&rows[slot * dim..(slot + 1) * dim]);
            true
        })
        .unwrap();
    let mut hits = 0;
    let samples = [0, 16, 63, 64, 128, 256, 512, 1000, 1536, 2047];
    for slot in samples {
        let query = &rows[slot * dim..(slot + 1) * dim];
        let cosine = |row: &[f32]| {
            let mut dot = 0.;
            let mut aa = 0.;
            let mut bb = 0.;
            for (&a, &b) in query.iter().zip(row) {
                let (a, b) = (a as f64, b as f64);
                dot += a * b;
                aa += a * a;
                bb += b * b;
            }
            dot / (aa * bb).sqrt()
        };
        let mut scores = rows.chunks_exact(dim).map(cosine).collect::<Vec<_>>();
        scores.sort_unstable_by(|a, b| b.total_cmp(a));
        let mut fetched = 0;
        let found = index.search(
            query,
            |slot, buffer| {
                fetched += 1;
                buffer.clear();
                buffer.extend_from_slice(&rows[slot * dim..(slot + 1) * dim]);
                true
            },
            &QuIVerSearchConfig {
                top_k: 10,
                ef_search: 128,
                rerank_limit: Some(64),
            },
        );
        assert!(fetched <= 64, "修复不能通过扩大精排预算伪装召回改善");
        for (id, _) in found {
            hits += usize::from(
                cosine(&rows[(id as usize - 1) * dim..id as usize * dim]) + 1e-6 >= scores[9],
            );
        }
    }
    assert!(
        hits >= 90,
        "近重复簇内外查询的并列感知 Recall@10 应至少为 90%，实际命中 {hits}/100"
    );
}
