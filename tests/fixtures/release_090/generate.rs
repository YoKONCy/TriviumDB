//! 使用不可变 v0.9.0 源码编译本例，生成迁移独立 oracle。
use triviumdb::{Database, VectorType};
use triviumdb::database::{Config, StorageMode};
use serde_json::json;

fn generate<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(path: &str, vectors: [Vec<T>; 2], mode: StorageMode, wal: bool) {
    let mut db = Database::<T>::open_with_config(path, Config { dim: 2, storage_mode: mode, auto_build_quiver: false, ..Default::default() }).unwrap();
    db.insert_with_id(7, &vectors[0], json!({"code":"alpha","rank":10,"text":"原始甲"})).unwrap();
    db.index_text(7, "迁移正文甲").unwrap();
    db.index_keyword(7, "独立关键词").unwrap();
    db.create_unique_index("code").unwrap();
    db.create_ordered_index("rank").unwrap();
    db.flush().unwrap();
    db.insert_with_id(9, &vectors[1], json!({"code":"beta","rank":20,"text":"原始乙"})).unwrap();
    db.upsert_edge(7, 9, "关系", 0.5, json!({"source":"release-0.9.0","rank":7})).unwrap();
    if wal {
        // 已确认操作保留在 WAL，进程退出不执行 Drop 检查点。
        std::process::exit(0);
    }
    db.close().unwrap();
}

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let mode = match args[3].as_str() { "rom" => StorageMode::Rom, "mmap" => StorageMode::Mmap, _ => panic!("存储模式无效") };
    let wal = args[4] == "wal";
    match args[2].as_str() {
        "f32" => generate(&args[1], [vec![1.0f32,-0.0],vec![0.25,1.0]], mode, wal),
        "f16" => generate(&args[1], [vec![half::f16::from_f32(1.0),half::f16::NEG_ZERO],vec![half::f16::from_f32(0.25),half::f16::from_f32(1.0)]], mode, wal),
        "u64" => generate(&args[1], [vec![9_007_199_254_740_993u64,u64::MAX-1],vec![0,u64::MAX]], mode, wal),
        _ => panic!("类型无效"),
    }
}
