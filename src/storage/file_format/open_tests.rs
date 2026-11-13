//! 核对同次打开的校验复用与不同打开之间的重新校验。
use super::*;
use crate::database::{AccessMode, Config, Database, MissingIndexPolicy};
use std::cell::RefCell;
use std::path::PathBuf;

use crate::test_support as support;

thread_local! {
    static CHECKSUM_READS: RefCell<Option<Vec<PathBuf>>> = const { RefCell::new(None) };
}

pub(super) fn record_checksum(path: &Path) {
    CHECKSUM_READS.with(|reads| {
        if let Some(reads) = reads.borrow_mut().as_mut() {
            reads.push(path.to_owned());
        }
    });
}

struct ChecksumTrace(Option<Vec<PathBuf>>);

impl ChecksumTrace {
    fn start() -> Self {
        Self(CHECKSUM_READS.with(|reads| reads.replace(Some(Vec::new()))))
    }

    fn paths(&self) -> Vec<PathBuf> {
        CHECKSUM_READS.with(|reads| {
            reads
                .borrow()
                .as_ref()
                .unwrap()
                .iter()
                .map(|path| std::fs::canonicalize(path).unwrap())
                .collect()
        })
    }
}

impl Drop for ChecksumTrace {
    fn drop(&mut self) {
        CHECKSUM_READS.with(|reads| reads.replace(self.0.take()));
    }
}

#[test]
fn 重开对事实文件各校验一次并在下次打开重新检查内容() {
    let root = support::TestDir::new("open_checksums");
    let path = root.file("data.tdb");
    let config = Config {
        dim: 2,
        auto_build_quiver: false,
        ..Config::default()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    db.insert_with_id(7, &[3., 0.], serde_json::json!({"body":"原文"}))
        .unwrap();
    db.insert_with_id(11, &[0., 4.], serde_json::json!({"body":"证据"}))
        .unwrap();
    db.build_quiver_index(None).unwrap();
    db.close().unwrap();
    let main_path = std::fs::canonicalize(&path).unwrap();
    let vector_path = std::fs::canonicalize(format!("{path}.vec")).unwrap();
    let original = std::fs::read(&vector_path).unwrap();
    let readonly = Config {
        access_mode: AccessMode::ReadOnly,
        missing_index_policy: MissingIndexPolicy::Error,
        ..config
    };
    for _ in 0..2 {
        let trace = ChecksumTrace::start();
        let mut reader = Database::<f32>::open_with_config(&path, readonly).unwrap();
        let paths = trace.paths();
        assert_eq!(
            paths.iter().filter(|actual| **actual == main_path).count(),
            1
        );
        assert_eq!(
            paths
                .iter()
                .filter(|actual| **actual == vector_path)
                .count(),
            1
        );
        let hits = reader.search(&[1., 0.], 2, 0, -1.).unwrap();
        assert_eq!(
            hits.iter()
                .map(|hit| (hit.id, hit.score, hit.payload.clone()))
                .collect::<Vec<_>>(),
            [
                (7, 1., serde_json::json!({"body":"原文"})),
                (11, 0., serde_json::json!({"body":"证据"})),
            ]
        );
        assert_eq!(reader.get(7).unwrap().unwrap().vector, [3., 0.]);
        assert_eq!(reader.get(11).unwrap().unwrap().vector, [0., 4.]);
        reader.close().unwrap();
    }
    let mut damaged = original.clone();
    damaged[0] ^= 1;
    std::fs::write(&vector_path, damaged).unwrap();
    assert!(matches!(
        Database::<f32>::open_with_config(&path, readonly),
        Err(TriviumError::CorruptedFile(_))
    ));
    assert_eq!(std::fs::read(&vector_path).unwrap()[0], original[0] ^ 1);
    std::fs::write(&vector_path, &original).unwrap();
    let mut reader = Database::<f32>::open_with_config(&path, readonly).unwrap();
    assert_eq!(reader.get(7).unwrap().unwrap().vector, [3., 0.]);
    assert_eq!(reader.get(11).unwrap().unwrap().vector, [0., 4.]);
    reader.close().unwrap();
}
