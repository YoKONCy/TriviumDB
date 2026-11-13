//! 独立历史布局、默认缓存与截断输入的持久化契约。
use super::*;
use std::path::Path;

use crate::test_support as support;

fn legacy_unit_index() -> Vec<u8> {
    // 按QUIV v1协议独立构造单节点图，M=2、m0=4、无上层和邻边。
    let mut bytes = b"QUIV".to_vec();
    for word in [1u32, 1, 1, 2, 4, 16, 1.2f32.to_bits(), 0, 0, 0, 0, 1] {
        bytes.extend(word.to_le_bytes());
    }
    bytes.extend(1u64.to_le_bytes());
    bytes.extend(0u64.to_le_bytes());
    bytes.extend(0u32.to_le_bytes());
    for _ in 0..8 {
        bytes.extend(u32::MAX.to_le_bytes());
    }
    bytes.push(0);
    bytes.extend(7u64.to_le_bytes());
    bytes.extend(0u64.to_le_bytes());
    bytes.push(0);
    bytes.extend(0u32.to_le_bytes());
    assert_eq!(bytes.len(), 126);
    bytes
}

#[test]
fn 独立旧图与空缓存均从原始事实恢复完整查询结果() {
    for version in [1u32, 2] {
        let mut bytes = legacy_unit_index();
        if version == 2 {
            bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
            bytes.extend(0u64.to_le_bytes());
        }
        let index = QuIVer::load_from_bytes(&bytes).unwrap();
        assert!(!index.default_sm_l2_enabled());
        let mut table = crate::storage::memtable::MemTable::<f32>::new(1);
        table
            .insert_with_id(7, &[1.], serde_json::json!({"body":"原文"}))
            .unwrap();
        assert!(index.matches_fact_slots(&table));
        table.set_quiver_index(index);
        let restored = table.quiver().unwrap();
        assert!(restored.default_sm_l2_enabled());
        assert_eq!(
            restored.search_flat(
                &[1.],
                &[1.],
                &QuIVerSearchConfig {
                    top_k: 1,
                    ef_search: 4,
                    rerank_limit: None
                }
            ),
            [(7, 1.)]
        );
        let root = support::TestDir::new("quiver_persistence");
        let path = root.file("index.quiver");
        restored.save_to_file(Path::new(&path)).unwrap();
        let persisted = std::fs::read(&path).unwrap();
        assert_eq!(&persisted[4..8], &2u32.to_le_bytes());
        let cached = QuIVer::load_from_bytes(&persisted).unwrap();
        assert!(cached.default_sm_l2_enabled());
        assert_eq!(
            cached.search_flat(
                &[1.],
                &[1.],
                &QuIVerSearchConfig {
                    top_k: 1,
                    ef_search: 4,
                    rerank_limit: None
                }
            ),
            [(7, 1.)]
        );
        for length in 0..persisted.len() {
            let result = std::panic::catch_unwind(|| QuIVer::load_from_bytes(&persisted[..length]));
            assert!(result.unwrap().is_err(), "版本{version}，截断长度{length}");
        }
        let mut oversized = persisted.clone();
        oversized[126..134].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(QuIVer::load_from_bytes(&oversized).is_err());
        let mut trailing = persisted;
        trailing.push(0);
        assert!(QuIVer::load_from_bytes(&trailing).is_err());
    }
}

#[test]
fn 公开安装入口根据当前向量重新生成精算比例() {
    let mut original = QuIVer::load_from_bytes(&legacy_unit_index()).unwrap();
    original
        .prepare_default_sm(|_, buffer| {
            buffer.push(1.);
            true
        })
        .unwrap();
    let mut table = crate::storage::memtable::MemTable::<f32>::new(1);
    table
        .insert_with_id(7, &[2.], serde_json::json!({"body":"新事实"}))
        .unwrap();
    table.set_quiver_index(original);
    assert_eq!(
        table.quiver().unwrap().search_flat(
            &[1.],
            &[2.],
            &QuIVerSearchConfig {
                top_k: 1,
                ef_search: 4,
                rerank_limit: None
            }
        ),
        [(7, 1.)]
    );
}

#[test]
fn 墓碑及复用slot的缓存加载与历史流式恢复逐项一致() {
    for dim in [65, 768] {
        let mut base = (0..32 * dim)
            .map(|axis| (axis as f32 * 0.13).cos())
            .collect::<Vec<_>>();
        let mut index = QuIVer::new(
            dim,
            &QuIVerConfig {
                m: 4,
                ef_construction: 16,
                alpha: 1.2,
            },
        );
        index.batch_build_experimental_v2(
            &base,
            &(1..=32).collect::<Vec<_>>(),
            &(0..32).collect::<Vec<_>>(),
        );
        assert!(index.soft_delete(1));
        base[..dim].fill(0.);
        base[0] = 3.;
        let mut seed = 37;
        index.insert(&base[..dim], 1000, 0, &mut seed);
        let root = support::TestDir::new("quiver_cached_tombstone");
        let path = root.file("index.quiver");
        index.save_to_file(Path::new(&path)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let cached = QuIVer::load_from_bytes(&bytes).unwrap();
        // v2沿用v1的图主体，独立按公开帧布局截取旧格式以比较其恢复语义。
        let snapshot_length = 36 + 33 * (16 * dim.div_ceil(64) + 20);
        let mut legacy_bytes = bytes[..bytes.len() - snapshot_length].to_vec();
        legacy_bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
        let mut legacy = QuIVer::load_from_bytes(&legacy_bytes).unwrap();
        let mut slots = Vec::new();
        legacy
            .prepare_default_sm(|slot, buffer| {
                slots.push(slot);
                buffer.extend_from_slice(&base[slot * dim..(slot + 1) * dim]);
                true
            })
            .unwrap();
        assert_eq!(slots.len(), 32);
        assert_eq!(slots.iter().filter(|&&slot| slot == 0).count(), 1);
        for width in [8, 24, 64] {
            let config = QuIVerSearchConfig {
                top_k: 10,
                ef_search: width,
                rerank_limit: None,
            };
            for slot in [0, 7, 13, 29] {
                let query = &base[slot * dim..(slot + 1) * dim];
                let expected = legacy.search_flat(query, &base, &config);
                let actual = cached.search_flat(query, &base, &config);
                assert_eq!(
                    actual
                        .iter()
                        .map(|(id, score)| (*id, score.to_bits()))
                        .collect::<Vec<_>>(),
                    expected
                        .iter()
                        .map(|(id, score)| (*id, score.to_bits()))
                        .collect::<Vec<_>>(),
                    "维度{dim}，候选宽度{width}，查询槽位{slot}"
                );
            }
        }
        let hits = cached.search_flat(
            &base[..dim],
            &base,
            &QuIVerSearchConfig {
                top_k: 1,
                ef_search: 64,
                rerank_limit: None,
            },
        );
        assert_eq!(hits, [(1000, 1.)]);
    }
}

#[test]
fn 同节点身份向量更新在两版布局保留单一活跃行并拒绝双活跃记录() {
    let mut index = QuIVer::new(
        1,
        &QuIVerConfig {
            m: 2,
            ef_construction: 16,
            alpha: 1.2,
        },
    );
    let mut seed = 37;
    index.insert(&[1.], 1, 0, &mut seed);
    index.insert(&[-1.], 2, 1, &mut seed);
    assert!(index.soft_delete(1));
    index.insert(&[2.], 1, 0, &mut seed);
    let root = support::TestDir::new("quiver_updated_id");
    let path = root.file("index.quiver");
    index.save_to_file(Path::new(&path)).unwrap();
    let persisted = std::fs::read(path).unwrap();
    // QUIV头部含BQ行宽共52字节，三节点BQ共48字节，m0=4时每个L0行36字节。
    assert_eq!(u32::from_le_bytes(persisted[20..24].try_into().unwrap()), 4);
    let tombstone_offset = 52 + 48 + 3 * 36;
    assert_eq!(
        &persisted[tombstone_offset..tombstone_offset + 3],
        &[1, 0, 0]
    );
    for version in [1u32, 2] {
        let mut bytes = persisted.clone();
        if version == 1 {
            bytes.truncate(bytes.len() - (36 + 3 * 36));
            bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
        }
        let mut loaded = QuIVer::load_from_bytes(&bytes).unwrap();
        assert_eq!(loaded.default_sm_l2_enabled(), version == 2);
        assert_eq!(
            loaded.search_flat(
                &[1.],
                &[2., -1.],
                &QuIVerSearchConfig {
                    top_k: 2,
                    ef_search: 8,
                    rerank_limit: None
                }
            ),
            [(1, 1.), (2, -1.)]
        );
        assert!(loaded.soft_delete(1));
        assert_eq!(loaded.active_count(), 1);
        assert_eq!(
            loaded.search_flat(
                &[1.],
                &[2., -1.],
                &QuIVerSearchConfig {
                    top_k: 2,
                    ef_search: 8,
                    rerank_limit: None
                }
            ),
            [(2, -1.)]
        );
        bytes[tombstone_offset] = 0;
        let error = QuIVer::load_from_bytes(&bytes)
            .err()
            .expect("双活跃身份须拒绝");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
