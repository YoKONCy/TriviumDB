//! 真实 CLI 验证 JSONL 的数值精度、原子提交、完整边及源文件保护。

use assert_cmd::Command;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use tempfile::TempDir;
use triviumdb::database::{AccessMode, Config, StorageMode};
use triviumdb::{Database, VectorType};

fn tdb() -> Command {
    let mut command = Command::cargo_bin("tdb").unwrap();
    command.args(["--color", "never"]);
    command
}

fn files(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let name = path.file_name().unwrap().to_str().unwrap();
    std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_file())
        .filter_map(|entry| {
            let file_name = entry.file_name().into_string().unwrap();
            let suffix = file_name.strip_prefix(name)?;
            if suffix == ".lock" {
                return None;
            }
            Some((suffix.to_owned(), std::fs::read(entry.path()).unwrap()))
        })
        .collect()
}

fn roundtrip<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    dtype: &str,
    vector: &[T],
    expected_json: Value,
) {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TempDir::new().unwrap();
        let source = root.path().join("source.tdb");
        let target = root.path().join("target.tdb");
        let output = root.path().join("dump.jsonl");
        let cfg = Config {
            dim: vector.len(),
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        };
        let payload = json!({"nested":[1,null,{"value":u64::MAX}],"text":"原始内容"});
        let metadata = json!({"rank":7,"source":"manual","nested":[null,u64::MAX]});
        let mut db = Database::<T>::open_with_config(source.to_str().unwrap(), cfg).unwrap();
        db.insert_with_id(7, vector, payload.clone()).unwrap();
        db.insert_with_id(9, vector, Value::Null).unwrap();
        db.upsert_edge(7, 9, "rel", 0.5, metadata.clone()).unwrap();
        db.close().unwrap();
        std::fs::write(&output, "旧导出内容").unwrap();
        tdb()
            .args([
                "export",
                source.to_str().unwrap(),
                output.to_str().unwrap(),
                "--dtype",
                dtype,
            ])
            .assert()
            .success();
        let records = std::fs::read_to_string(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            records,
            vec![
                json!({"id":7,"vector":expected_json,"payload":payload,"edges":[{"target":9,"label":"rel","weight":0.5,"metadata":metadata}]}),
                json!({"id":9,"vector":expected_json,"payload":null,"edges":[]}),
            ]
        );
        tdb()
            .args([
                "import",
                target.to_str().unwrap(),
                output.to_str().unwrap(),
                "--dtype",
                dtype,
                "--dim",
                &vector.len().to_string(),
            ])
            .assert()
            .success();
        let mut restored =
            Database::<T>::open_read_only(target.to_str().unwrap(), vector.len()).unwrap();
        assert_eq!(restored.all_node_ids().unwrap(), [7, 9]);
        let first = restored.get(7).unwrap().unwrap();
        assert_eq!(first.vector, vector);
        assert_eq!(first.payload, payload);
        assert_eq!(
            serde_json::to_value(first.edges).unwrap(),
            json!([{"target_id":9,"label":"rel","weight":0.5,"metadata":metadata}])
        );
        let second = restored.get(9).unwrap().unwrap();
        assert_eq!(second.vector, vector);
        assert_eq!(second.payload, Value::Null);
        assert!(second.edges.is_empty());
        restored.close().unwrap();
    }
}

#[test]
fn u64_jsonl往返保留全部整数位模式与边元数据() {
    let vector = [0, 16_777_217, 9_007_199_254_740_993, u64::MAX - 1, u64::MAX];
    roundtrip("u64", &vector, json!(vector));
}

#[test]
fn 浮点jsonl往返保留向量payload与边元数据() {
    roundtrip(
        "f32",
        &[1.0f32, -0.5, 0.25, 0.0],
        json!([1.0, -0.5, 0.25, 0.0]),
    );
    let vector = [
        half::f16::MAX,
        half::f16::from_f32(-0.5),
        half::f16::MIN_POSITIVE,
        half::f16::ZERO,
    ];
    roundtrip(
        "f16",
        &vector,
        json!([65504.0, -0.5, 0.00006103515625, 0.0]),
    );
}

#[test]
fn 自动编号与混合显式编号保留前向边及删除后的编号水位() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("target.tdb");
    let input = root.path().join("input.jsonl");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(9, &[0.0, 1.0], json!({"existing":true}))
        .unwrap();
    db.insert_with_id(50, &[0.0, 0.0], Value::Null).unwrap();
    db.delete(50).unwrap();
    db.close().unwrap();
    let metadata = json!({"rank":7,"source":"manual"});
    let records = [
        json!({"vector":[1,0],"payload":{"row":0},"edges":[{"target":200,"label":"future","weight":0.5,"metadata":metadata},{"target":51,"label":"self"}]}),
        json!({"id":200,"vector":[0,1],"payload":{"row":1}}),
        json!({"vector":[1,1],"payload":{"row":2},"edges":[{"target":9,"label":"existing"}]}),
        json!({"id":1,"vector":[0,0],"payload":{"row":3}}),
    ];
    std::fs::write(
        &input,
        records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    tdb()
        .args(["import", path.to_str().unwrap(), input.to_str().unwrap()])
        .assert()
        .success();
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    assert_eq!(db.all_node_ids().unwrap(), [1, 9, 51, 200, 201]);
    for (id, row, vector) in [
        (51, 0, vec![1.0, 0.0]),
        (200, 1, vec![0.0, 1.0]),
        (201, 2, vec![1.0, 1.0]),
        (1, 3, vec![0.0, 0.0]),
    ] {
        let node = db.get(id).unwrap().unwrap();
        assert_eq!(node.vector, vector);
        assert_eq!(node.payload, json!({"row":row}));
    }
    let mut edges = db.get_edges(51).unwrap();
    edges.sort_by_key(|edge| edge.target_id);
    assert_eq!(
        serde_json::to_value(edges).unwrap(),
        json!([
            {"target_id":51,"label":"self","weight":1.0,"metadata":null},
            {"target_id":200,"label":"future","weight":0.5,"metadata":metadata},
        ])
    );
    assert_eq!(
        serde_json::to_value(db.get_edges(201).unwrap()).unwrap(),
        json!([{"target_id":9,"label":"existing","weight":1.0,"metadata":null}])
    );
    assert_eq!(db.insert(&[1.0, 0.0], Value::Null).unwrap(), 202);
    db.close().unwrap();
}

#[test]
fn 导入预检失败保持节点边与全部持久化文件() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("atomic.tdb");
    let input = root.path().join("input.jsonl");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(9, &[0.0, 1.0], json!({"existing":true}))
        .unwrap();
    db.upsert_edge(9, 9, "keep", 0.5, json!({"fact":7}))
        .unwrap();
    db.close().unwrap();
    let before = files(&path);
    for (records, code) in [
        (
            vec![
                json!({"id":1,"vector":[1,0]}),
                json!({"id":2,"vector":[1,0,0]}),
            ],
            "TDB_DIMENSION_MISMATCH",
        ),
        (
            vec![
                json!({"id":1,"vector":[1,0]}),
                json!({"id":1,"vector":[1,0]}),
            ],
            "TDB_NODE_ALREADY_EXISTS",
        ),
        (
            vec![
                json!({"id":1,"vector":[1,0]}),
                json!({"id":9,"vector":[1,0]}),
            ],
            "TDB_NODE_ALREADY_EXISTS",
        ),
        (
            vec![json!({"vector":[1,0],"edges":[{"target":999,"label":"missing"}]})],
            "TDB_NODE_NOT_FOUND",
        ),
    ] {
        std::fs::write(
            &input,
            records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let output = tdb()
            .args([
                "--format=json",
                "import",
                path.to_str().unwrap(),
                input.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], code);
        assert_eq!(error["write"]["outcome"], "not_committed");
        assert_eq!(files(&path), before);
        let mut db = Database::<f32>::open_read_only(path.to_str().unwrap(), 2).unwrap();
        assert_eq!(db.all_node_ids().unwrap(), [9]);
        let node = db.get(9).unwrap().unwrap();
        assert_eq!(node.vector, [0.0, 1.0]);
        assert_eq!(node.payload, json!({"existing":true}));
        assert_eq!(
            serde_json::to_value(node.edges).unwrap(),
            json!([{"target_id":9,"label":"keep","weight":0.5,"metadata":{"fact":7}}])
        );
        db.close().unwrap();
    }
}

#[test]
fn 非法u64与f16分量拒绝整份导入() {
    let root = TempDir::new().unwrap();
    for (dtype, invalid) in [
        ("u64", "1.5"),
        ("u64", "-1"),
        ("u64", "18446744073709551616"),
        ("u64", "9007199254740993.0"),
        ("u64", "1e3"),
        ("f16", "70000"),
    ] {
        let path = root.path().join(format!("{dtype}-{invalid}.tdb"));
        let input = root.path().join("input.jsonl");
        std::fs::write(
            &input,
            format!("{{\"id\":1,\"vector\":[1,0]}}\n{{\"id\":2,\"vector\":[{invalid},0]}}\n"),
        )
        .unwrap();
        let output = tdb()
            .args([
                "--format=json",
                "import",
                path.to_str().unwrap(),
                input.to_str().unwrap(),
                "--dtype",
                dtype,
                "--dim",
                "2",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
            "TDB_INVALID_VECTOR"
        );
        let records = root.path().join("rejected.jsonl");
        tdb()
            .args([
                "export",
                path.to_str().unwrap(),
                records.to_str().unwrap(),
                "--dtype",
                dtype,
                "--dim",
                "2",
            ])
            .assert()
            .success();
        assert_eq!(std::fs::read(records).unwrap(), b"");
    }
}

#[test]
fn 导出发布失败保留io错误类别与原有文件() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("source.tdb");
    let output = root.path().join("destination");
    std::fs::create_dir(&output).unwrap();
    let kept = output.join("keep.bin");
    std::fs::write(&kept, b"original content").unwrap();
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(1, &[1.0, 0.0], json!({"kept":true}))
        .unwrap();
    db.close().unwrap();
    let before = files(&path);
    let mut before_names = std::fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    before_names.sort();
    let rejected = tdb()
        .args([
            "--format=json",
            "export",
            path.to_str().unwrap(),
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    assert!(rejected.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&rejected.stderr).unwrap()["code"],
        "TDB_IO"
    );
    assert_eq!(std::fs::read(kept).unwrap(), b"original content");
    assert_eq!(files(&path), before);
    let mut after_names = std::fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    after_names.sort();
    assert_eq!(after_names, before_names);
}

#[test]
fn 导出保护所有制品名称族及文本硬链接并保留可检索正文() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("source.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(1, &[1.0, 0.0], json!({"fact":"alpha"}))
        .unwrap();
    db.index_text(1, "alpha evidence").unwrap();
    db.close().unwrap();
    let read_config = Config {
        dim: 2,
        access_mode: AccessMode::ReadOnly,
        load_text_index: true,
        auto_build_quiver: false,
        ..Default::default()
    };
    let mut baseline =
        Database::<f32>::open_with_config(path.to_str().unwrap(), read_config).unwrap();
    let hits = baseline
        .tql("TEXT BM25 \"alpha\" TOP 1 AS h WITH h RETURN h.id AS id")
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(matches!(
        hits[0]["id"],
        triviumdb::query::tql_executor::TqlValue::Int(1)
    ));
    baseline.close().unwrap();
    let before = files(&path);
    assert!(before.contains_key(".text"));
    for suffix in [
        "",
        ".vec",
        ".wal",
        ".lock",
        ".flush_ok",
        ".text",
        ".text.meta",
        ".quiver",
        ".quiver.meta",
        ".pidx",
        ".gidx",
        ".manifest.json",
        ".pld.999",
        ".publish.0",
        ".text.tmp",
        ".tmp",
        ".qssd",
        ".qssd-generation",
    ] {
        let output = format!("{}{suffix}", path.display());
        let rejected = tdb()
            .args(["export", path.to_str().unwrap(), &output])
            .output()
            .unwrap();
        assert_eq!(rejected.status.code(), Some(1), "{suffix}");
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains("拒绝导出到数据库相关文件"),
            "{suffix}"
        );
        assert_eq!(files(&path), before, "{suffix}");
    }
    let alias = root.path().join("alias.jsonl");
    std::fs::hard_link(format!("{}.text", path.display()), &alias).unwrap();
    let rejected = tdb()
        .args(["export", path.to_str().unwrap(), alias.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("拒绝导出到数据库相关文件"));
    assert_eq!(std::fs::read(alias).unwrap(), before[".text"]);
    assert_eq!(files(&path), before);
    let mut db = Database::<f32>::open_with_config(path.to_str().unwrap(), read_config).unwrap();
    let rows = db
        .tql("TEXT BM25 \"alpha\" TOP 1 AS h WITH h RETURN h.id AS id")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(matches!(
        rows[0]["id"],
        triviumdb::query::tql_executor::TqlValue::Int(1)
    ));
    assert_eq!(db.get_payload(1).unwrap(), Some(json!({"fact":"alpha"})));
    db.close().unwrap();
}
