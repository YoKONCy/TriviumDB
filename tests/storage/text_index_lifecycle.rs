#![allow(non_snake_case)]

use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use triviumdb::database::{AccessMode, Config, Database, SearchConfig, StorageMode};
use triviumdb::index::text::TextIndex;

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "triviumdb_text_lifecycle_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        // 测试仅清理自己创建的目录中的直接文件。
        if let Ok(entries) = std::fs::read_dir(&self.0) {
            for entry in entries.flatten() {
                std::fs::remove_file(entry.path()).ok();
            }
        }
        std::fs::remove_dir(&self.0).ok();
    }
}

fn open(path: &str, mode: StorageMode, load: bool) -> Database<f32> {
    Database::open_with_config(
        path,
        Config {
            dim: 3,
            storage_mode: mode,
            load_text_index: load,
            auto_build_quiver: false,
            ..Default::default()
        },
    )
    .unwrap()
}

fn seed(path: &str, mode: StorageMode) {
    let mut db = open(path, mode, false);
    db.insert_with_id(1, &[1., 0., 0.], json!({"body":"不应自动索引的字段"}))
        .unwrap();
    db.insert_with_id(2, &[0., 1., 0.], json!({})).unwrap();
    db.index_text(1, "alpha original 苏晚").unwrap();
    db.index_text(2, "beta survivor 林川").unwrap();
    db.index_keyword(1, "旧别名").unwrap();
    db.index_keyword(2, "共同别名").unwrap();
    db.close().unwrap();
}

fn search(db: &Database<f32>, query: &str) -> Vec<u64> {
    let config = SearchConfig {
        top_k: 20,
        recall_k: 20,
        rerank_k: 20,
        expand_depth: 0,
        min_score: -1.,
        enable_text_hybrid_search: true,
        enable_advanced_pipeline: false,
        ..Default::default()
    };
    let mut ids: Vec<_> = db
        .search_hybrid(Some(query), None, &config)
        .unwrap()
        .iter()
        .map(|hit| hit.id)
        .collect();
    ids.sort_unstable();
    ids
}

#[test]
fn 未加载文本索引的flush与close保留文件并绑定新主文件() {
    let dir = TestDir::new();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = dir.path(&format!("preserve_{mode:?}.tdb"));
        seed(&path, mode);
        let bytes = std::fs::read(format!("{path}.text")).unwrap();
        let mut db = open(&path, mode, false);
        assert!(
            search(&db, "alpha").is_empty(),
            "未加载时不隐式启用全文检索"
        );
        db.insert_with_id(
            3,
            &[0., 0., 1.],
            json!({"long":"改变主文件大小".repeat(100)}),
        )
        .unwrap();
        db.flush().unwrap();
        assert_eq!(std::fs::read(format!("{path}.text")).unwrap(), bytes);
        assert!(search(&db, "alpha").is_empty());
        db.close().unwrap();
        assert_eq!(std::fs::read(format!("{path}.text")).unwrap(), bytes);
        let db = open(&path, mode, true);
        assert_eq!(search(&db, "alpha"), vec![1]);
        assert_eq!(search(&db, "共同别名"), vec![2]);
        assert_eq!(db.node_count().unwrap(), 3);
    }
}

#[test]
fn 延期加载后显式修改合并原索引并替换正文() {
    let dir = TestDir::new();
    let path = dir.path("merge.tdb");
    seed(&path, StorageMode::Mmap);
    let mut db = open(&path, StorageMode::Mmap, false);
    db.index_text(1, "updated 苏晚").unwrap();
    db.index_keyword(1, "共同别名").unwrap();
    db.index_keyword(1, "共同别名").unwrap();
    assert!(search(&db, "alpha").is_empty());
    assert_eq!(search(&db, "updated"), vec![1]);
    assert_eq!(search(&db, "beta"), vec![2]);
    assert_eq!(search(&db, "共同别名"), vec![1, 2]);
    db.close().unwrap();
    let db = open(&path, StorageMode::Mmap, true);
    assert!(search(&db, "alpha").is_empty());
    assert_eq!(search(&db, "updated"), vec![1]);
    assert_eq!(search(&db, "beta"), vec![2]);
}

#[test]
fn 未加载时删除并复用相同ID不得继承旧正文或别名() {
    let dir = TestDir::new();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = dir.path(&format!("reuse_{mode:?}.tdb"));
        seed(&path, mode);
        let mut db = open(&path, mode, false);
        let mut tx = db.begin_tx();
        tx.delete(1);
        tx.insert_with_id(1, &[1., 0., 0.], json!({"replacement":true}));
        tx.commit().unwrap();
        db.close().unwrap();
        let db = open(&path, mode, true);
        assert!(search(&db, "alpha 旧别名").is_empty());
        assert_eq!(search(&db, "beta 共同别名"), vec![2]);
    }
}

#[test]
fn 已加载索引删除即时清理且空正文只移除BM25() {
    let dir = TestDir::new();
    let path = dir.path("loaded.tdb");
    seed(&path, StorageMode::Mmap);
    let mut db = open(&path, StorageMode::Mmap, true);
    db.index_text(1, "").unwrap();
    assert!(search(&db, "alpha").is_empty());
    assert_eq!(search(&db, "旧别名"), vec![1]);
    db.delete(1).unwrap();
    assert!(search(&db, "旧别名").is_empty());
    db.delete(2).unwrap();
    db.close().unwrap();
    assert!(!Path::new(&format!("{path}.text")).exists());
    assert!(!Path::new(&format!("{path}.text.meta")).exists());
}

#[test]
fn WAL回放删除同步文本清理且拒绝事务不泄漏删除() {
    let dir = TestDir::new();
    let path = dir.path("replay.tdb");
    seed(&path, StorageMode::Mmap);
    {
        let mut db = open(&path, StorageMode::Mmap, false);
        let mut tx = db.begin_tx();
        tx.delete(2);
        tx.delete(999);
        assert!(tx.commit().is_err());
        db.delete(1).unwrap();
        // 仅丢弃句柄，保留 WAL，模拟未 flush 的恢复路径。
    }
    let mut db = open(&path, StorageMode::Mmap, true);
    assert!(search(&db, "alpha 旧别名").is_empty());
    assert_eq!(search(&db, "beta 共同别名"), vec![2]);
    db.close().unwrap();
    assert_eq!(
        search(&open(&path, StorageMode::Mmap, true), "beta"),
        vec![2]
    );
}

#[test]
fn 未加载索引损坏不得被flush重算CRC认可() {
    let dir = TestDir::new();
    let path = dir.path("corrupt.tdb");
    seed(&path, StorageMode::Mmap);
    let mut db = open(&path, StorageMode::Mmap, false);
    let text_path = format!("{path}.text");
    let mut bytes = std::fs::read(&text_path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&text_path, &bytes).unwrap();
    let old_meta = std::fs::read(format!("{path}.text.meta")).unwrap();
    db.insert_with_id(3, &[0., 0., 1.], json!({"payload":"不同大小".repeat(100)}))
        .unwrap();
    db.flush().unwrap();
    let new_meta = std::fs::read(format!("{path}.text.meta")).unwrap();
    assert_eq!(
        &old_meta[24..28],
        &new_meta[24..28],
        "不得重算并认可损坏后的 CRC"
    );
    assert!(db.index_text(1, "replacement").is_err());
    assert_eq!(std::fs::read(&text_path).unwrap(), bytes);
}

#[test]
fn 延期索引元数据发布失败后同句柄重试可恢复() {
    let dir = TestDir::new();
    let path = dir.path("retry.tdb");
    seed(&path, StorageMode::Mmap);
    let mut db = open(&path, StorageMode::Mmap, false);
    let obstacle = format!("{path}.text.meta.tmp");
    std::fs::create_dir(&obstacle).unwrap();
    db.insert_with_id(
        3,
        &[0., 0., 1.],
        json!({"large":"使新代大小变化".repeat(50)}),
    )
    .unwrap();
    assert!(db.flush().is_err());
    std::fs::remove_dir(&obstacle).unwrap();
    db.flush().unwrap();
    db.index_keyword(2, "重试别名").unwrap();
    assert_eq!(search(&db, "alpha"), vec![1]);
    assert_eq!(search(&db, "重试别名"), vec![2]);
    db.close().unwrap();
    assert_eq!(
        search(&open(&path, StorageMode::Mmap, true), "alpha"),
        vec![1]
    );
}

#[test]
fn 文本已写入但属性发布失败后重开恢复完整文件组() {
    let dir = TestDir::new();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = dir.path(&format!("group_{mode:?}.tdb"));
        seed(&path, mode);
        let old_text = std::fs::read(format!("{path}.text")).unwrap();
        let mut db = open(&path, mode, true);
        db.create_index("kind").unwrap();
        db.insert_with_id(3, &[0., 0., 1.], json!({"kind":"新增"}))
            .unwrap();
        db.update_payload(1, json!({"kind":"更新"})).unwrap();
        db.delete(2).unwrap();
        db.index_text(1, "uncommitted replacement").unwrap();
        let obstacle = format!("{path}.pidx.tmp");
        std::fs::create_dir(&obstacle).unwrap();
        assert!(db.flush().is_err());
        assert!(Path::new(&format!("{path}.publish")).exists());
        drop(db);
        std::fs::remove_dir(&obstacle).unwrap();
        assert!(matches!(
            Database::<f32>::open_read_only(&path, 3),
            Err(triviumdb::TriviumError::RecoveryRequired { .. })
        ));
        let mut db = open(&path, mode, true);
        assert_eq!(db.node_count().unwrap(), 2);
        assert_eq!(db.get_payload(1).unwrap().unwrap(), json!({"kind":"更新"}));
        assert_eq!(db.get_payload(3).unwrap().unwrap(), json!({"kind":"新增"}));
        assert_eq!(std::fs::read(format!("{path}.text")).unwrap(), old_text);
        assert_eq!(search(&db, "alpha 旧别名"), vec![1]);
        assert!(search(&db, "replacement beta 共同别名").is_empty());
        db.flush().unwrap();
        db.close().unwrap();
        let db = open(&path, mode, true);
        assert_eq!(search(&db, "alpha 旧别名"), vec![1]);
        assert_eq!(db.node_count().unwrap(), 2);
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn 延期保存失败后新增文本修改仍有完整撤销备份() {
    use triviumdb::test_hooks::{IoPoint, fail_io_at};
    let dir = TestDir::new();
    let path = dir.path("enlist.tdb");
    seed(&path, StorageMode::Mmap);
    let mut db = open(&path, StorageMode::Mmap, false);
    db.insert_with_id(3, &[0., 0., 1.], json!({})).unwrap();
    {
        let _failure = fail_io_at(IoPoint::TextMetadataWrite);
        assert!(db.flush().is_err());
    }
    db.index_text(1, "replacement").unwrap();
    {
        let _failure = fail_io_at(IoPoint::GraphSidecarWrite);
        assert!(db.flush().is_err());
    }
    drop(db);
    let mut db = open(&path, StorageMode::Mmap, true);
    assert_eq!(db.node_count().unwrap(), 3);
    assert_eq!(search(&db, "alpha 旧别名"), vec![1]);
    assert!(search(&db, "replacement").is_empty());
    db.flush().unwrap();
    db.close().unwrap();
}

#[test]
fn 文本重建验证完整输入与预算后替换并遵守只读权限() {
    use triviumdb::index::text::TextIndexDocument;
    let dir = TestDir::new();
    let path = dir.path("rebuild-validation.tdb");
    seed(&path, StorageMode::Mmap);
    let mut db = open(&path, StorageMode::Mmap, true);
    let document = || TextIndexDocument {
        id: 1,
        text: "rebuilt".into(),
        keywords: vec!["新别名".into()],
    };
    assert!(db.rebuild_text_index(&[document(), document()]).is_err());
    assert!(
        db.rebuild_text_index(&[TextIndexDocument {
            keywords: vec![String::new()],
            ..document()
        }])
        .is_err()
    );
    db.set_memory_limit(1024 * 1024).unwrap();
    assert!(
        db.rebuild_text_index(&[TextIndexDocument {
            text: "大正文".repeat(20000),
            ..document()
        }])
        .is_err()
    );
    assert_eq!(search(&db, "alpha 旧别名"), vec![1]);
    db.close().unwrap();
    let mut reader = Database::<f32>::open_read_only(&path, 3).unwrap();
    assert!(matches!(
        reader.rebuild_text_index(&[]).unwrap_err().underlying(),
        triviumdb::TriviumError::ReadOnlyViolation { .. }
    ));
    reader.close().unwrap();
}

#[test]
fn 延期文本解码执行共享工作区准入且拒绝后保持原文件() {
    use triviumdb::resources::{ResourceLimits, ResourceRuntime};
    let dir = TestDir::new();
    let path = dir.path("text-runtime.tdb");
    let mut db = open(&path, StorageMode::Mmap, false);
    db.insert_with_id(1, &[1., 0., 0.], json!({})).unwrap();
    let text = (0..4000)
        .map(|index| format!("term{index:04}"))
        .collect::<Vec<_>>()
        .join(" ");
    db.index_text(1, &text).unwrap();
    db.close().unwrap();
    let original = std::fs::read(format!("{path}.text")).unwrap();
    let runtime = ResourceRuntime::new(ResourceLimits {
        memory_limit: 12 * 1024 * 1024,
        maintenance_memory_limit: 1024 * 1024,
        ..Default::default()
    })
    .unwrap();
    let mut db = Database::<f32>::open_with_runtime(
        &path,
        Config {
            dim: 3,
            memory_limit: 8 * 1024 * 1024,
            load_text_index: false,
            ..Default::default()
        },
        runtime.clone(),
    )
    .unwrap();
    assert!(matches!(
        db.build_text_index().unwrap_err().underlying(),
        triviumdb::TriviumError::ResourceBudgetExceeded { .. }
    ));
    assert_eq!(std::fs::read(format!("{path}.text")).unwrap(), original);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    assert!(runtime.stats().peak_reserved_bytes <= runtime.stats().memory_limit);
    db.close().unwrap();
}

#[test]
fn 只读未加载句柄保持全部文件字节不变() {
    let dir = TestDir::new();
    let path = dir.path("readonly.tdb");
    seed(&path, StorageMode::Mmap);
    let snapshot = || -> BTreeMap<_, _> {
        std::fs::read_dir(&dir.0)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), std::fs::read(entry.path()).unwrap())
            })
            .collect()
    };
    let before = snapshot();
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: 3,
            load_text_index: false,
            access_mode: AccessMode::ReadOnly,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(search(&db, "alpha").is_empty());
    assert!(db.index_text(1, "new").is_err());
    db.close().unwrap();
    drop(db);
    assert_eq!(snapshot(), before);
}

#[test]
fn 关键词幂等且新增词查询自动刷新_同词支持多节点() {
    let mut index = TextIndex::new();
    index.add_keyword(1, "Alpha");
    index.add_keyword(1, "ALPHA");
    assert_eq!(index.search_ac("alpha").get(&1), Some(&1.));
    index.add_keyword(2, "alpha");
    index.add_keyword(2, "beta");
    assert_eq!(index.search_ac("alpha").len(), 2);
    assert_eq!(index.search_ac("beta").get(&2), Some(&1.));
    index.remove_node(1);
    assert_eq!(index.search_ac("alpha"), HashMap::from([(2, 1.)]));
    index.remove_node(2);
    assert!(index.search_ac("alpha beta").is_empty());
}

#[test]
fn 正文替换删除后的BM25等同独立词袋reference() {
    fn verify(index: &TextIndex, documents: &BTreeMap<u64, Vec<&str>>) {
        let n = documents.len() as f64;
        let avg = documents.values().map(|tokens| tokens.len()).sum::<usize>() as f64 / n.max(1.);
        for word in ["alpha", "beta", "gamma", "苏晚", "林川"] {
            let actual = index.search_bm25(word, 1.2, 0.75);
            let df = documents
                .values()
                .filter(|tokens| tokens.contains(&word))
                .count() as f64;
            let mut expected = HashMap::new();
            for (&id, tokens) in documents {
                let tf = tokens.iter().filter(|token| **token == word).count() as f64;
                if tf > 0. {
                    let score = ((n - df + 0.5) / (df + 0.5) + 1.).ln() * tf * 2.2
                        / (tf + 1.2 * (0.25 + 0.75 * tokens.len() as f64 / avg));
                    expected.insert(id, score);
                }
            }
            assert_eq!(actual.len(), expected.len());
            for (id, score) in expected {
                assert!((f64::from(actual[&id]) - score).abs() < 0.00001);
            }
        }
    }
    let mut index = TextIndex::new();
    let mut documents = BTreeMap::from([
        (1, vec!["alpha", "alpha", "苏晚"]),
        (2, vec!["beta", "gamma", "林川"]),
    ]);
    index.add_text(1, "alpha alpha 苏晚");
    index.add_text(2, "beta gamma 林川");
    verify(&index, &documents);
    index.add_text(1, "beta 苏晚");
    documents.insert(1, vec!["beta", "苏晚"]);
    verify(&index, &documents);
    index.remove_node(2);
    documents.remove(&2);
    verify(&index, &documents);
    index.add_text(1, "");
    documents.clear();
    verify(&index, &documents);
}

#[test]
fn 历史v2快照去重并保留现有字节布局() {
    let dir = TestDir::new();
    let path = dir.path("legacy.text");
    // 使用独立数据结构生成历史布局，不调用生产 writer。
    let snapshot = (
        BTreeMap::from([("alpha".to_owned(), vec![1u64, 1, 2])]),
        BTreeMap::from([("alpha".to_owned(), BTreeMap::from([(1u64, 1usize)]))]),
        BTreeMap::from([(1u64, 1usize)]),
    );
    let mut bytes = b"TIDX".to_vec();
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend(bincode::serialize(&snapshot).unwrap());
    std::fs::write(&path, bytes).unwrap();
    let mut index = TextIndex::load_from_file(Path::new(&path)).unwrap();
    assert_eq!(index.search_ac("alpha"), HashMap::from([(1, 1.), (2, 1.)]));
    index.add_text(1, "beta");
    index.save_to_file(Path::new(&path)).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..8], b"TIDX\x02\x00\x00\x00");
    type LegacySnapshot = (
        BTreeMap<String, Vec<u64>>,
        BTreeMap<String, BTreeMap<u64, usize>>,
        BTreeMap<u64, usize>,
    );
    let (_, terms, lengths): LegacySnapshot = bincode::deserialize(&bytes[8..]).unwrap();
    assert!(!terms.contains_key("alpha"));
    assert_eq!(terms["beta"][&1], 1);
    assert_eq!(lengths[&1], 1);
}

#[test]
fn 发布版0810文件组升级后可写且保留正文关键词() {
    let dir = TestDir::new();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v0810_text_mmap");
    for missing_meta in [false, true] {
        let path = dir.path(&format!("upgrade_{missing_meta}.tdb"));
        for suffix in [
            "",
            ".vec",
            ".flush_ok",
            ".pld.2",
            ".text",
            ".text.meta",
            ".wal",
        ] {
            if missing_meta && suffix == ".text.meta" {
                continue;
            }
            std::fs::copy(
                source.join(format!("sample.tdb{suffix}")),
                format!("{path}{suffix}"),
            )
            .unwrap();
        }
        let mut db = Database::<f32>::open(&path, 4).unwrap();
        assert_eq!(db.node_count().unwrap(), 4);
        db.insert_with_id(5, &[0., 0., 1., 0.], json!({"content":"新增"}))
            .unwrap();
        db.flush().unwrap();
        db.build_text_index().unwrap();
        assert_eq!(search(&db, "alpha"), vec![1, 2, 3, 4]);
        assert_eq!(
            db.tql_nodes(r#"TEXT AC "测试别名0" TOP 10 AS hits WITH hits RETURN hits"#)
                .unwrap()
                .iter()
                .map(|row| row["hits"].id)
                .collect::<Vec<_>>(),
            vec![1]
        );
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 4,
                load_text_index: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(db.node_count().unwrap(), 5);
        assert_eq!(
            db.tql_nodes(r#"TEXT AC "测试别名3" TOP 10 AS hits WITH hits RETURN hits"#)
                .unwrap()
                .iter()
                .map(|row| row["hits"].id)
                .collect::<Vec<_>>(),
            vec![4]
        );
        db.delete(2).unwrap();
        db.flush().unwrap();
        db.close().unwrap();
        let db = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 4,
                load_text_index: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(search(&db, "alpha"), vec![1, 3, 4]);
    }
}

#[test]
fn 历史元数据缺失与主库长度变化均可加载合并和清理() {
    let dir = TestDir::new();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for load in [false, true] {
            for missing in [false, true] {
                let path = dir.path(&format!("legacy_{mode:?}_{load}_{missing}.tdb"));
                seed(&path, mode);
                let meta_path = format!("{path}.text.meta");
                if missing {
                    std::fs::remove_file(&meta_path).unwrap();
                } else {
                    let mut bytes = std::fs::read(&meta_path).unwrap();
                    bytes[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
                    std::fs::write(&meta_path, bytes).unwrap();
                }
                let mut db = open(&path, mode, load);
                db.insert_with_id(3, &[0., 0., 1.], json!({"正文":"独立载荷"}))
                    .unwrap();
                db.delete(1).unwrap();
                db.flush().unwrap();
                db.build_text_index().unwrap();
                assert_eq!(search(&db, "beta 共同别名"), vec![2]);
                assert!(search(&db, "alpha 旧别名").is_empty());
                db.close().unwrap();
                let db = open(&path, mode, true);
                assert_eq!(db.node_count().unwrap(), 2);
                assert_eq!(search(&db, "beta 共同别名"), vec![2]);
            }
        }
    }
}

#[test]
fn 显式加载历史文本后关闭补齐元数据且只读打开保持原文件() {
    let dir = TestDir::new();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = dir.path(&format!("legacy-close-{mode:?}.tdb"));
        seed(&path, mode);
        let meta = format!("{path}.text.meta");
        std::fs::remove_file(&meta).unwrap();
        let original = std::fs::read(format!("{path}.text")).unwrap();
        let mut reader = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 3,
                access_mode: AccessMode::ReadOnly,
                load_text_index: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(search(&reader, "alpha 旧别名"), vec![1]);
        reader.close().unwrap();
        assert!(!Path::new(&meta).exists());
        assert_eq!(std::fs::read(format!("{path}.text")).unwrap(), original);
        let mut db = open(&path, mode, true);
        db.close().unwrap();
        let bytes = std::fs::read(&meta).unwrap();
        assert_eq!(bytes.len(), 28);
        assert_eq!(
            &bytes[8..16],
            &std::fs::metadata(&path).unwrap().len().to_le_bytes()
        );
        assert_eq!(
            &bytes[24..28],
            &crc32fast::hash(&std::fs::read(format!("{path}.text")).unwrap()).to_le_bytes()
        );
        assert_eq!(search(&open(&path, mode, true), "alpha 旧别名"), vec![1]);
    }
}

#[test]
fn 损坏索引原样保留且显式重建可恢复正文和关键词() {
    use triviumdb::index::text::TextIndexDocument;
    let dir = TestDir::new();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for corrupt in ["crc", "size", "metadata", "trailing", "truncated"] {
            let path = dir.path(&format!("rebuild_{mode:?}_{corrupt}.tdb"));
            seed(&path, mode);
            let text = format!("{path}.text");
            let meta = format!("{path}.text.meta");
            match corrupt {
                "metadata" => std::fs::write(&meta, b"TMET").unwrap(),
                "crc" => {
                    let mut bytes = std::fs::read(&text).unwrap();
                    let end = bytes.len() - 1;
                    bytes[end] ^= 1;
                    std::fs::write(&text, bytes).unwrap();
                }
                "size" => {
                    let mut bytes = std::fs::read(&text).unwrap();
                    bytes.push(0);
                    std::fs::write(&text, bytes).unwrap();
                }
                _ => {
                    std::fs::remove_file(&meta).unwrap();
                    let mut bytes = std::fs::read(&text).unwrap();
                    if corrupt == "trailing" {
                        bytes.push(0);
                    } else {
                        bytes.truncate(10);
                    }
                    std::fs::write(&text, bytes).unwrap();
                }
            }
            let original = std::fs::read(&text).unwrap();
            let mut db = open(&path, mode, true);
            assert_eq!(std::fs::read(&text).unwrap(), original);
            let error = db.build_text_index().unwrap_err();
            assert!(matches!(
                error.underlying(),
                triviumdb::TriviumError::ImmutableArtifactInvalid { .. }
            ));
            db.insert_with_id(3, &[0., 0., 1.], json!({})).unwrap();
            db.flush().unwrap();
            assert_eq!(std::fs::read(&text).unwrap(), original);
            db.rebuild_text_index(&[
                TextIndexDocument {
                    id: 1,
                    text: "恢复正文 repaired".into(),
                    keywords: vec!["恢复别名".into()],
                },
                TextIndexDocument {
                    id: 2,
                    text: "beta survivor".into(),
                    keywords: vec!["共同别名".into()],
                },
            ])
            .unwrap();
            assert_eq!(search(&db, "repaired 恢复别名"), vec![1]);
            assert_eq!(search(&db, "beta"), vec![2]);
            assert!(search(&db, "alpha 旧别名").is_empty());
            let invalid = TextIndexDocument {
                id: 999,
                text: "无效".into(),
                keywords: vec![],
            };
            assert!(db.rebuild_text_index(&[invalid]).is_err());
            assert_eq!(search(&db, "恢复别名"), vec![1]);
            db.close().unwrap();
            let mut db = open(&path, mode, true);
            assert_eq!(search(&db, "repaired 恢复别名"), vec![1]);
            db.rebuild_text_index(&[]).unwrap();
            db.close().unwrap();
            assert!(!Path::new(&text).exists());
            assert!(!Path::new(&meta).exists());
            assert_eq!(open(&path, mode, true).node_count().unwrap(), 3);
        }
    }
}

#[test]
fn 两字中文支持BM25与CONTAINS扫描_索引过滤先于截断() {
    let dir = TestDir::new();
    let path = dir.path("short.tdb");
    let mut db = open(&path, StorageMode::Mmap, false);
    for id in 1..=12 {
        db.insert_with_id(id, &[1., 0., 0.], json!({"name":"苏晚","active":id==12}))
            .unwrap();
        db.index_text(id, "苏晚").unwrap();
    }
    db.create_ngram_index("name").unwrap();
    assert_eq!(
        db.tql_nodes(r#"FIND {name: {$contains: "苏晚"}} RETURN *"#)
            .unwrap()
            .len(),
        12
    );
    let config = SearchConfig {
        top_k: 1,
        recall_k: 1,
        rerank_k: 1,
        expand_depth: 0,
        min_score: -1.,
        enable_text_hybrid_search: true,
        enable_advanced_pipeline: false,
        payload_filter: Some(triviumdb::Filter::eq("active", json!(true))),
        ..Default::default()
    };
    let hits = db.search_hybrid(Some("苏晚"), None, &config).unwrap();
    assert_eq!(hits.iter().map(|hit| hit.id).collect::<Vec<_>>(), vec![12]);
    assert!(db.index_keyword(12, "").is_err());
    assert!(db.index_text(999, "missing").is_err());
}

#[test]
fn 延期加载与删除预留遵守预算且失败不追加WAL() {
    let dir = TestDir::new();
    let path = dir.path("budget.tdb");
    seed(&path, StorageMode::Mmap);
    let mut db = open(&path, StorageMode::Mmap, false);
    let before_wal = std::fs::read(format!("{path}.wal")).unwrap();
    let before_text = std::fs::read(format!("{path}.text")).unwrap();
    db.set_memory_limit(db.estimated_memory() + 1024).unwrap();
    assert!(matches!(
        db.build_text_index().unwrap_err().underlying(),
        triviumdb::TriviumError::CapacityReservationRejected { .. }
    ));
    db.set_memory_limit(1).unwrap();
    let mut tx = db.begin_tx();
    tx.delete(1);
    assert!(matches!(
        tx.commit().unwrap_err().underlying(),
        triviumdb::TriviumError::CapacityReservationRejected { .. }
    ));
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before_wal);
    assert_eq!(std::fs::read(format!("{path}.text")).unwrap(), before_text);
    assert!(db.get(1).unwrap().is_some());
    db.set_memory_limit(0).unwrap();
    db.build_text_index().unwrap();
    assert_eq!(search(&db, "alpha"), vec![1]);
}
