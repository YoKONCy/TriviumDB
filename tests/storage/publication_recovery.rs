//! 默认 cargo test 即执行的发布恢复契约；独立构造日志字节，不依赖 test-hooks。

use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use triviumdb::TriviumError;
use triviumdb::database::{Config, Database, StorageMode};
use triviumdb::storage::wal::SyncMode;

const DIM: usize = 17;

struct Fixture {
    directory: PathBuf,
    path: String,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "triviumdb-publication-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("data.tdb").to_str().unwrap().to_owned();
        Self { directory, path }
    }

    fn open(&self, mode: StorageMode) -> Database<f32> {
        Database::open_with_config(
            &self.path,
            Config {
                dim: DIM,
                storage_mode: mode,
                sync_mode: SyncMode::Full,
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn seed(&self, mode: StorageMode) {
        let mut database = self.open(mode);
        for id in 1..=32 {
            database
                .insert_with_id(id, &vector(id), json!({"id": id, "版本": "旧"}))
                .unwrap();
        }
        database.flush().unwrap();
    }

    fn files(&self) -> BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(&self.directory)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read(entry.path()).unwrap(),
                )
            })
            .collect()
    }

    fn stage_old_generation(&self) -> Vec<u8> {
        // TPUB v1 独立规格 oracle：固定 56 字节，文件顺序为主文件、向量、marker。
        let generation = std::fs::read(format!("{}.flush_ok", self.path))
            .ok()
            .map(|marker| u64::from_le_bytes(marker[5..13].try_into().unwrap()) + 1)
            .unwrap_or(1);
        let mut bytes = vec![0u8; 56];
        bytes[..5].copy_from_slice(b"TPUB\x01");
        bytes[5..13].copy_from_slice(&generation.to_le_bytes());
        for (index, suffix) in ["", ".vec", ".flush_ok"].iter().enumerate() {
            if let Ok(data) = std::fs::read(format!("{}{suffix}", self.path)) {
                std::fs::write(format!("{}.publish.{index}", self.path), &data).unwrap();
                let offset = 13 + index * 13;
                bytes[offset] = 1;
                bytes[offset + 1..offset + 9].copy_from_slice(&(data.len() as u64).to_le_bytes());
                bytes[offset + 9..offset + 13]
                    .copy_from_slice(&crc32fast::hash(&data).to_le_bytes());
            }
        }
        let crc = crc32fast::hash(&bytes[..52]);
        bytes[52..].copy_from_slice(&crc.to_le_bytes());
        std::fs::write(format!("{}.publish", self.path), &bytes).unwrap();
        bytes
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // 目录由本测试独占创建，只删除其直接子文件，不递归处理未知目录。
        for entry in std::fs::read_dir(&self.directory).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).ok();
        }
        std::fs::remove_dir(&self.directory).ok();
    }
}

fn vector(id: u64) -> Vec<f32> {
    (0..DIM)
        .map(|axis| id as f32 + axis as f32 * 0.125)
        .collect()
}

#[test]
fn 发布前中断必须恢复旧基础与全部增量并支持再次持久化() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let fixture = Fixture::new("restore");
        fixture.seed(mode);
        {
            let mut database = fixture.open(mode);
            database
                .insert_with_id(33, &vector(33), json!({"id": 33, "版本": "新"}))
                .unwrap();
            database.update_vector(1, &[-1.0; DIM]).unwrap();
            database
                .update_payload(1, json!({"id": 1, "版本": "新"}))
                .unwrap();
            database.delete(2).unwrap();
            database.link(1, 3, "恢复边", 2.5).unwrap();
        }
        fixture.stage_old_generation();
        // 模拟新文件已覆盖旧事实，而旧 marker 尚未提交；包含 Rom → Mmap 首次切换。
        std::fs::write(&fixture.path, b"TVDB").unwrap();
        std::fs::write(format!("{}.vec", fixture.path), [1, 2, 3]).unwrap();
        let before = fixture.files();
        assert!(matches!(
            Database::<f32>::open_read_only(&fixture.path, DIM),
            Err(TriviumError::RecoveryRequired { .. })
        ));
        assert!(matches!(
            Database::<f32>::open_immutable(&fixture.path, DIM),
            Err(TriviumError::RecoveryRequired { .. })
        ));
        assert_eq!(fixture.files(), before, "拒绝只读打开时必须保持字节级零写");
        assert!(matches!(
            Database::<half::f16>::open(&fixture.path, DIM),
            Err(TriviumError::DtypeMismatch {
                stored: "f32",
                requested: "f16"
            })
        ));
        assert_eq!(
            fixture.files(),
            before,
            "错误类型不得触发备份恢复或文件清理"
        );
        let mut database = fixture.open(StorageMode::Mmap);
        for _ in 0..2 {
            let mut ids = database.all_node_ids().unwrap();
            ids.sort_unstable();
            assert_eq!(ids, (1..=33).filter(|id| *id != 2).collect::<Vec<_>>());
            for id in ids {
                assert_eq!(
                    database.get(id).unwrap().unwrap().vector,
                    if id == 1 { vec![-1.0; DIM] } else { vector(id) }
                );
                assert_eq!(
                    database.get_payload(id).unwrap().unwrap(),
                    json!({"id": id, "版本": if id == 1 || id == 33 { "新" } else { "旧" }})
                );
            }
            let edges = database.get_edges(1).unwrap();
            assert_eq!(edges.len(), 1);
            assert_eq!(
                (edges[0].target_id, edges[0].label.as_str(), edges[0].weight),
                (3, "恢复边", 2.5)
            );
            database.flush().unwrap();
            drop(database);
            database = fixture.open(StorageMode::Mmap);
        }
    }
}

#[test]
fn 日志或备份损坏必须返回结构化损坏错误且不覆盖事实文件() {
    for corruption in [
        "journal_short",
        "journal_crc",
        "backup_missing",
        "backup_crc",
        "marker_crc",
    ] {
        let fixture = Fixture::new(corruption);
        fixture.seed(StorageMode::Mmap);
        fixture.stage_old_generation();
        let suffix = match corruption {
            "journal_short" | "journal_crc" => ".publish",
            "backup_missing" | "backup_crc" => ".publish.1",
            _ => ".flush_ok",
        };
        let target = format!("{}{suffix}", fixture.path);
        match corruption {
            "backup_missing" => std::fs::remove_file(&target).unwrap(),
            "journal_short" => std::fs::write(&target, b"TPUB").unwrap(),
            _ => {
                let mut bytes = std::fs::read(&target).unwrap();
                bytes[0] ^= 1;
                std::fs::write(&target, bytes).unwrap();
            }
        }
        let before = fixture.files();
        assert!(
            matches!(
                Database::<f32>::open(&fixture.path, DIM),
                Err(TriviumError::CorruptedFile(_))
            ),
            "损坏场景必须拒绝: {corruption}"
        );
        assert_eq!(fixture.files(), before, "损坏备份不得覆盖当前事实文件");
    }
}

#[test]
fn marker提交后必须保留新代_新代损坏也不得回滚旧代() {
    for corrupt in [false, true] {
        let fixture = Fixture::new("committed");
        fixture.seed(StorageMode::Mmap);
        let journal = fixture.stage_old_generation();
        std::fs::remove_file(format!("{}.publish", fixture.path)).unwrap();
        {
            let mut database = fixture.open(StorageMode::Mmap);
            database
                .insert_with_id(33, &vector(33), json!({"id": 33}))
                .unwrap();
            database.flush().unwrap();
        }
        // 模拟新 marker 已持久化、清理旧日志前断电；此时 WAL 已清空。
        std::fs::write(format!("{}.publish", fixture.path), journal).unwrap();
        if corrupt {
            std::fs::write(format!("{}.vec", fixture.path), [0]).unwrap();
            let before = fixture.files();
            assert!(matches!(
                Database::<f32>::open(&fixture.path, DIM),
                Err(TriviumError::CorruptedFile(_))
            ));
            assert_eq!(fixture.files(), before, "不能用旧代掩盖已提交新代的损坏");
        } else {
            let database = fixture.open(StorageMode::Mmap);
            assert_eq!(database.node_count().unwrap(), 33);
            assert_eq!(database.get(33).unwrap().unwrap().vector, vector(33));
        }
    }
}
