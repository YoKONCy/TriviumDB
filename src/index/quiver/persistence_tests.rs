//! 当前图快照、默认缓存与截断输入的持久化契约。
use super::*;
use std::path::Path;

use crate::test_support as support;

fn current_unit_index() -> QuIVer {
    let mut index = QuIVer::new(
        1,
        &QuIVerConfig {
            m: 2,
            ef_construction: 16,
            alpha: 1.2,
        },
    );
    index.batch_build(&[1.], &[7], &[0]);
    index
}

#[test]
fn 当前图快照往返且拒绝截断与额外字节() {
    let root = support::TestDir::new("quiver_persistence");
    let path = root.file("index.quiver");
    current_unit_index().save_to_file(Path::new(&path)).unwrap();
    let bytes = std::fs::read(path).unwrap();
    let loaded = QuIVer::load_from_bytes(&bytes).unwrap();
    assert_eq!(
        loaded.search_flat(
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
    for length in 0..bytes.len() {
        assert!(
            QuIVer::load_from_bytes(&bytes[..length]).is_err(),
            "截断长度{length}"
        );
    }
    let mut trailing = bytes;
    trailing.push(0);
    assert!(QuIVer::load_from_bytes(&trailing).is_err());
}

#[test]
fn 公开安装入口根据当前向量重新生成精算比例() {
    let mut original = current_unit_index();
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
fn 同节点身份向量更新保留单一活跃行并拒绝双活跃记录() {
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
    {
        let mut bytes = persisted.clone();
        let mut loaded = QuIVer::load_from_bytes(&bytes).unwrap();
        assert!(loaded.default_sm_l2_enabled());
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
