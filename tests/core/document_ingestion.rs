//! 解析、原文证据和候选版本的持久化公共契约。
use super::support::TestDir;
use serde_json::{Value, json};
use triviumdb::database::{AccessMode, Config, StorageMode};
use triviumdb::document::*;
use triviumdb::{Database, VectorType};

fn config(mode: StorageMode) -> Config {
    Config {
        dim: 2,
        storage_mode: mode,
        auto_build_quiver: false,
        ..Config::default()
    }
}
fn request<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    db: &mut Database<T>,
    value: Value,
) -> Value {
    serde_json::from_str(
        &triviumdb::sdk::documents::request(db, &value.to_string())
            .unwrap_or_else(|error| panic!("{value}: {error:?}")),
    )
    .unwrap()
}
fn parse(db: &mut Database<f32>, name: &str, content: &str) -> Value {
    request(
        db,
        json!({"operation":"parse","input":{"source_id":name,"source_revision":"source-1","content":content}}),
    )
}

#[test]
fn 内置五种解析保留完整正文与独立预期来源范围() {
    let dir = TestDir::new("document-parsers");
    let mut db =
        Database::<f32>::open_with_config(&dir.file("sample.tdb"), config(StorageMode::Rom))
            .unwrap();
    for (name, raw, text) in [
        ("a.txt", "甲\r\n乙\r\n\r\n尾", "甲\n乙\n\n尾"),
        (
            "a.md",
            "# 标题\n\nHello **世界** &amp; ok.\n",
            "标题\nHello 世界 & ok.\n",
        ),
        (
            "a.html",
            "<h1>甲&amp;乙</h1><script>hidden</script><p>尾🐾</p>",
            "甲&乙\n尾🐾\n",
        ),
        (
            "a.json",
            r#"{"name":"猫\u0020🐾","count":2}"#,
            "name: 猫 🐾\ncount: 2\n",
        ),
        ("a.jsonl", "{\"a\":1}\n{\"a\":2}\n", "a: 1\na: 2\n"),
    ] {
        let parsed = parse(&mut db, name, raw);
        assert_eq!(parsed["text"], text, "{name}");
        assert_eq!(parsed["source"]["original_text"], raw);
        let typed: parsing::ParsedDocument = serde_json::from_value(parsed.clone()).unwrap();
        typed.validate().unwrap();
        let snapshot = request(
            &mut db,
            json!({"operation":"put_parsed","document":{"parsed":parsed,"root":{"id":"10","vector":[1,0],"payload":{"source":name}}},"expected_revision":null}),
        );
        assert_eq!(snapshot["content"], text);
        let evidence = request(
            &mut db,
            json!({"operation":"resolve","reference":{"document_id":"10","revision":snapshot["revision"],"start_byte":0,"end_byte":text.len()}}),
        );
        assert_eq!(evidence["text"], text);
        let spans = evidence["source_spans"].as_array().unwrap();
        assert!(!spans.is_empty());
        for span in spans {
            let first = span["source_start"].as_u64().unwrap() as usize;
            let last = span["source_end"].as_u64().unwrap() as usize;
            assert_eq!(span["text"], &raw[first..last]);
        }
        if name == "a.html" {
            let entity = spans.iter().find(|span| span["kind"] == "cover").unwrap();
            assert_eq!(entity["text"], "甲&amp;乙");
            assert_eq!(
                (
                    entity["source_start"].as_u64().unwrap(),
                    entity["source_end"].as_u64().unwrap()
                ),
                (4, 15)
            );
        }
        request(
            &mut db,
            json!({"operation":"delete","document_id":"10","expected_revision":snapshot["revision"]}),
        );
        assert!(db.list_document_uploads(None).unwrap().is_empty());
    }
}

fn begin(id: &str, document_id: &str, content: &str, expected: Value) -> Value {
    use sha2::{Digest, Sha256};
    json!({"operation":"begin_upload","upload":{"upload_id":id,"source_id":"note.txt","source_revision":"file-2","parser_id":"external-v1","content_hash":format!("sha256:{:x}",Sha256::digest(content.as_bytes())),"root":{"id":document_id,"vector":[1,0],"payload":{"title":"published"}},"expected_revision":expected}})
}

#[test]
fn 分批候选跨重开幂等发布并保留旧版本可见性() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let dir = TestDir::new("document-upload");
        let path = dir.file("sample.tdb");
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        let old:DocumentWrite<f32>=serde_json::from_value(json!({"source_id":"note.txt","source_revision":"file-1","parser_id":"manual","content":"old","root":{"id":"10","vector":[0,1]},"blocks":[]})).unwrap();
        let old = db.put_document(old, None).unwrap();
        let id = "3dc0b6d9-0543-42e8-a292-5e466d67d1ab";
        let start = begin(id, "10", "new 猫", json!(old.revision));
        let first = request(&mut db, start.clone());
        assert_eq!(first["next_sequence"], 0);
        assert_eq!(request(&mut db, start), first);
        let batch = json!({"operation":"append_upload","upload_id":id,"batch":{"sequence":0,"content":"new ","blocks":[{"key":"p","kind":"paragraph","start_byte":0,"end_byte":7,"node":{"id":"20","vector":[1,1],"payload":{"chunk":1}}}]}});
        let ack = request(&mut db, batch.clone());
        assert_eq!(ack["next_sequence"], 1);
        assert_eq!(request(&mut db, batch.clone()), ack);
        assert_eq!(db.all_node_ids().unwrap(), [10]);
        assert_eq!(db.get_document(10).unwrap().unwrap(), old);
        let mut conflict = batch.clone();
        conflict["batch"]["content"] = json!("other");
        let error = triviumdb::sdk::documents::request(&mut db, &conflict.to_string()).unwrap_err();
        assert_eq!(error.code(), "TDB_DOCUMENT_UPLOAD_CONFLICT");
        db.flush().unwrap();
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        assert_eq!(db.document_upload_status(id).unwrap().next_sequence, 1);
        assert_eq!(db.get_document(10).unwrap().unwrap(), old);
        request(
            &mut db,
            json!({"operation":"append_upload","upload_id":id,"batch":{"sequence":1,"content":"猫"}}),
        );
        let published = request(
            &mut db,
            json!({"operation":"publish_upload","upload_id":id}),
        );
        assert_eq!(published["content"], "new 猫");
        assert_eq!(db.all_node_ids().unwrap(), [10, 20]);
        assert_eq!(db.get(20).unwrap().unwrap().payload["text"], "new 猫");
        assert_eq!(
            request(
                &mut db,
                json!({"operation":"publish_upload","upload_id":id})
            ),
            published
        );
        assert_eq!(request(&mut db, batch)["state"], "published");
        db.flush().unwrap();
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        assert_eq!(
            serde_json::to_value(db.get_document(10).unwrap()).unwrap(),
            published
        );
        assert!(db.document_upload_status(id).unwrap().visible);
        db.delete(10).unwrap();
        assert!(db.list_document_uploads(None).unwrap().is_empty());
        db.flush().unwrap();
        db.close().unwrap();
        let mut read = config(mode);
        read.access_mode = AccessMode::ReadOnly;
        let db = Database::<f32>::open_with_config(&path, read).unwrap();
        assert!(db.list_document_uploads(None).unwrap().is_empty());
    }
}

#[test]
fn 候选竞争由根修订拒绝且完整候选可终止() {
    let dir = TestDir::new("document-upload-cas");
    let mut db =
        Database::<f32>::open_with_config(&dir.file("data.tdb"), config(StorageMode::Mmap))
            .unwrap();
    let ids = [
        "5073159d-a3c2-47ce-96d5-06e2ebda85a5",
        "5d788a32-4e20-4227-bc81-34454e71384e",
    ];
    for id in ids {
        request(&mut db, begin(id, "9007199254740993", "猫", Value::Null));
        request(
            &mut db,
            json!({"operation":"append_upload","upload_id":id,"batch":{"sequence":0,"content":"猫"}}),
        );
    }
    let first = request(
        &mut db,
        json!({"operation":"publish_upload","upload_id":ids[0]}),
    );
    let error = triviumdb::sdk::documents::request(
        &mut db,
        &json!({"operation":"publish_upload","upload_id":ids[1]}).to_string(),
    )
    .unwrap_err();
    assert_eq!(error.code(), "TDB_DOCUMENT_REVISION_MISMATCH");
    assert_eq!(
        request(
            &mut db,
            json!({"operation":"get","document_id":"9007199254740993"})
        ),
        first
    );
    assert_eq!(
        db.document_upload_status(ids[1]).unwrap().state,
        DocumentUploadState::Pending
    );
    db.abort_document_upload(ids[1]).unwrap();
    assert_eq!(
        db.document_upload_status(ids[1]).unwrap_err().code(),
        "TDB_DOCUMENT_UPLOAD_NOT_FOUND"
    );
    assert_eq!(
        db.abort_document_upload(ids[0]).unwrap_err().code(),
        "TDB_DOCUMENT_UPLOAD_CONFLICT"
    );
}

#[test]
fn 外部来源页码区域证据与映射拒绝() {
    let dir = TestDir::new("document-external-map");
    let mut db =
        Database::<f32>::open_with_config(&dir.file("data.tdb"), config(StorageMode::Mmap))
            .unwrap();
    let mut parsed = parse(&mut db, "ocr.txt", "猫🐾");
    parsed["parser_id"] = json!("host-ocr-v3");
    parsed["source"]["media_type"] = json!("application/pdf");
    parsed["source"]["artifact_hash"] = json!(format!("sha256:{}", "0".repeat(64)));
    parsed["source"]["mappings"][0]["location"] = json!({"page":2,"paragraph":"p-7","region":{"x":100,"y":200,"width":300,"height":400,"scale":1000,"unit":"normalized"}});
    let snapshot = request(
        &mut db,
        json!({"operation":"put_parsed","document":{"parsed":parsed,"root":{"id":"1","vector":[1,0]}}}),
    );
    let result = request(
        &mut db,
        json!({"operation":"resolve","reference":{"document_id":"1","revision":snapshot["revision"],"start_byte":3,"end_byte":7}}),
    );
    assert_eq!(
        result["source_spans"],
        json!([{"text_start":3,"text_end":7,"source_start":3,"source_end":7,"kind":"exact","content_hash":snapshot["source"]["content_hash"],"text":"🐾","line":1,"column":2,"end_line":1,"end_column":3,"location":{"page":2,"paragraph":"p-7","region":{"x":100,"y":200,"width":300,"height":400,"scale":1000,"unit":"normalized"}}}])
    );
    let before = db.list_document_uploads(None).unwrap();
    for (field, value) in [
        ("/source/mappings/0/text_end", json!(4)),
        ("/source/mappings/0/location/page", json!(0)),
        ("/source/mappings/0/location/region/width", json!(1001)),
        (
            "/source/content_hash",
            json!(format!("sha256:{}", "f".repeat(64))),
        ),
    ] {
        let mut bad = parsed.clone();
        *bad.pointer_mut(field).unwrap() = value;
        let error=triviumdb::sdk::documents::request(&mut db,&json!({"operation":"put_parsed","document":{"parsed":bad,"root":{"id":"2","vector":[1,0]}}}).to_string()).unwrap_err();
        assert_eq!(error.code(), "TDB_INVALID_INPUT", "{field}");
        assert_eq!(db.list_document_uploads(None).unwrap(), before);
        assert!(db.get(2).unwrap().is_none());
    }
}

#[test]
fn 大于单Payload限制的正文重开检索与再次持久化() {
    let dir = TestDir::new("document-large");
    let path = dir.file("data.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    // 九MiB超过公开的单Payload八MiB上限，正文只保留一个可独立断言的词项。
    let content = format!("{}needle", " ".repeat(9 * 1024 * 1024));
    let document:DocumentWrite<f32>=serde_json::from_value(json!({"source_id":"large.txt","source_revision":"1","parser_id":"plain","content":content,"root":{"id":"1","vector":[1,0]},"blocks":[]})).unwrap();
    let expected = db.put_document(document, None).unwrap();
    assert_eq!(expected.content, content);
    assert_eq!(expected.schema_version, 2);
    assert!(db.get(1).unwrap().unwrap().payload.to_string().len() < 512);
    assert_eq!(db.list_document_uploads(Some(1)).unwrap().len(), 1);
    db.flush().unwrap();
    db.close().unwrap();
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    assert_eq!(db.get_document(1).unwrap(), Some(expected.clone()));
    let result = request(
        &mut db,
        json!({"operation":"search","query":{"text":"needle","level":"document","max_documents":1,"limits":{"max_return_bytes":16*1024*1024}}}),
    );
    assert_eq!(result["groups"][0]["document_id"], "1");
    db.flush().unwrap();
    db.close().unwrap();
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Rom)).unwrap();
    assert_eq!(db.get_document(1).unwrap(), Some(expected));
    db.close().unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn 检查点覆盖批次后旧日志重放保持最终发布和回收() {
    use triviumdb::test_hooks::{IoPoint, fail_io_at};
    let dir = TestDir::new("document-checkpoint-overlap");
    let path = dir.file("data.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    let id = "0484b56c-60dc-4327-b4ee-8a30dc0d61da";
    request(&mut db, begin(id, "1", "first", Value::Null));
    db.flush().unwrap();
    request(
        &mut db,
        json!({"operation":"append_upload","upload_id":id,"batch":{"sequence":0,"content":"first"}}),
    );
    let first = request(
        &mut db,
        json!({"operation":"publish_upload","upload_id":id}),
    );
    let next = "047b9dbd-cc30-4bda-88ce-57022039f934";
    request(
        &mut db,
        begin(next, "1", "second", first["revision"].clone()),
    );
    request(
        &mut db,
        json!({"operation":"append_upload","upload_id":next,"batch":{"sequence":0,"content":"second"}}),
    );
    let expected = request(
        &mut db,
        json!({"operation":"publish_upload","upload_id":next}),
    );
    assert_eq!(db.list_document_uploads(None).unwrap().len(), 1);
    let failure = fail_io_at(IoPoint::CheckpointWalClear);
    assert_eq!(db.flush().unwrap_err().code(), "TDB_IO");
    drop(db);
    drop(failure);
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    assert_eq!(
        request(&mut db, json!({"operation":"get","document_id":"1"})),
        expected
    );
    assert_eq!(db.list_document_uploads(None).unwrap().len(), 1);
    assert_eq!(
        db.document_upload_status(id).unwrap_err().code(),
        "TDB_DOCUMENT_UPLOAD_NOT_FOUND"
    );
    db.flush().unwrap();
    db.close().unwrap();
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    assert_eq!(
        request(&mut db, json!({"operation":"get","document_id":"1"})),
        expected
    );
}

#[test]
fn 历史十二版与五版命名图日志完整升级() {
    fn run<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(dtype: &str) {
        let dir = TestDir::new("document-format12-history");
        let path = dir.file("sample.tdb");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/pre_document_publication_format12")
            .join(dtype);
        let expected: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("case.json")).unwrap()).unwrap();
        for entry in std::fs::read_dir(&fixture).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), dir.0.join(entry.file_name())).unwrap();
        }
        assert_eq!(&std::fs::read(&path).unwrap()[4..6], &12u16.to_le_bytes());
        assert_eq!(
            &std::fs::read(format!("{path}.wal")).unwrap()[4..6],
            &5u16.to_le_bytes()
        );
        let mut db = Database::<T>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
        assert_eq!(db.all_node_ids().unwrap(), [11, 22, 33]);
        assert_eq!(
            serde_json::to_value(
                db.get_named_graph("历史命名图", &Default::default())
                    .unwrap()
            )
            .unwrap(),
            expected["after"]
        );
        for (id, vector) in [(11, [1., 0.]), (22, [0., 1.]), (33, [1., 1.])] {
            let node = db.get(id).unwrap().unwrap();
            assert_eq!(node.vector, vector.map(T::from_f32));
            assert_eq!(node.payload, json!({"id":id}));
        }
        assert!(db.list_document_uploads(None).unwrap().is_empty());
        db.flush().unwrap();
        db.close().unwrap();
        assert_eq!(&std::fs::read(&path).unwrap()[4..6], &13u16.to_le_bytes());
        let mut db = Database::<T>::open_with_config(&path, config(StorageMode::Rom)).unwrap();
        assert_eq!(
            serde_json::to_value(
                db.get_named_graph("历史命名图", &Default::default())
                    .unwrap()
            )
            .unwrap(),
            expected["after"]
        );
        db.close().unwrap();
    }
    run::<f32>("f32");
    run::<half::f16>("f16");
    run::<u64>("u64");
}

#[cfg(feature = "test-hooks")]
#[test]
fn 最终发布同步未知结果恢复完整候选表示并再次持久化() {
    use triviumdb::test_hooks::{IoPoint, fail_io_at};
    let dir = TestDir::new("document-publish-wal-failure");
    let path = dir.file("data.tdb");
    let settings = Config {
        sync_mode: triviumdb::storage::wal::SyncMode::Full,
        ..config(StorageMode::Mmap)
    };
    let mut db = Database::<f32>::open_with_config(&path, settings).unwrap();
    let old:DocumentWrite<f32>=serde_json::from_value(json!({"source_id":"note.txt","source_revision":"1","parser_id":"plain","content":"old","root":{"id":"1","vector":[0,1]},"blocks":[]})).unwrap();
    let old = db.put_document(old, None).unwrap();
    db.flush().unwrap();
    let id = "7d45a1af-890d-4cfb-947f-a71666d8f21a";
    request(&mut db, begin(id, "1", "new", json!(old.revision)));
    request(
        &mut db,
        json!({"operation":"append_upload","upload_id":id,"batch":{"sequence":0,"content":"new","blocks":[{"key":"p","kind":"paragraph","start_byte":0,"end_byte":3,"node":{"id":"2","vector":[1,1],"payload":{"evidence":true}}}]}}),
    );
    let pending = db.document_upload_status(id).unwrap();
    let failure = fail_io_at(IoPoint::WalSync);
    let error = db.publish_document_upload(id).unwrap_err();
    drop(failure);
    assert_eq!(error.code(), "TDB_IO");
    assert_eq!(
        error.info().write.unwrap().outcome,
        triviumdb::error::WriteOutcome::Unknown
    );
    assert_eq!(db.get_document(1).unwrap(), Some(old));
    assert!(db.get(2).unwrap().is_none());
    drop(db);
    let mut db = Database::<f32>::open_with_config(&path, settings).unwrap();
    let recovered = db.get_document(1).unwrap().unwrap();
    assert_eq!(recovered.revision, pending.revision);
    assert_eq!(recovered.content, "new");
    assert_eq!(db.get(1).unwrap().unwrap().vector, [1., 0.]);
    let fragment = db.get(2).unwrap().unwrap();
    assert_eq!(fragment.vector, [1., 1.]);
    assert_eq!(fragment.payload["text"], "new");
    assert_eq!(fragment.payload["data"], json!({"evidence":true}));
    assert_eq!(
        db.document_upload_status(id).unwrap().state,
        DocumentUploadState::Published
    );
    db.flush().unwrap();
    db.close().unwrap();
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Rom)).unwrap();
    assert_eq!(db.get_document(1).unwrap(), Some(recovered));
    db.close().unwrap();
}

#[test]
fn 零解析额度和候选取消保持WAL并归还工作区() {
    let dir = TestDir::new("document-ingestion-cancel");
    let path = dir.file("data.tdb");
    let runtime = triviumdb::resources::ResourceRuntime::new(Default::default()).unwrap();
    let mut db =
        Database::<f32>::open_with_runtime(&path, config(StorageMode::Mmap), runtime.clone())
            .unwrap();
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    for field in [
        "max_input_bytes",
        "max_output_bytes",
        "max_blocks",
        "max_mappings",
    ] {
        let mut options = json!({});
        options[field] = json!(0);
        let error=triviumdb::sdk::documents::request(&mut db,&json!({"operation":"parse","input":{"source_id":"a.txt","source_revision":"1","content":"猫","config":options}}).to_string()).unwrap_err();
        assert_eq!(error.code(), "QueueFull", "{field}");
    }
    let context =
        triviumdb::sdk::RequestContext::new("cancel-upload", Default::default(), false).unwrap();
    context.cancel();
    let input: DocumentUploadBegin<f32> = serde_json::from_value(
        begin(
            "7d45a1af-890d-4cfb-947f-a71666d8f21a",
            "1",
            "new",
            Value::Null,
        )["upload"]
            .clone(),
    )
    .unwrap();
    let error = context.run(|| db.begin_document_upload(input)).unwrap_err();
    assert_eq!(error.code(), "TDB_QUERY_CANCELLED");
    assert!(db.list_document_uploads(None).unwrap().is_empty());
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    let invalid_identity = db
        .document_upload_status(&"x".repeat(1024 * 1024))
        .unwrap_err();
    assert_eq!(invalid_identity.code(), "TDB_DOCUMENT_UPLOAD_CONFLICT");
    assert_eq!(
        invalid_identity.info().details,
        json!({"field":"upload_id"})
    );
    assert!(invalid_identity.info().write.is_none());
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    db.close().unwrap();
    assert_eq!(runtime.stats().reserved_database_bytes, 0);
}

#[test]
fn 只读候选查询零写且拥有结果在关闭后允许移动文件组() {
    let dir = TestDir::new("document-upload-readonly");
    let path = dir.file("data.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    let id = "b7f025de-cd53-4d32-849e-d06a8c1c2b60";
    request(&mut db, begin(id, "1", "new", Value::Null));
    request(
        &mut db,
        json!({"operation":"append_upload","upload_id":id,"batch":{"sequence":0,"content":"new"}}),
    );
    let snapshot = db.publish_document_upload(id).unwrap();
    db.close().unwrap();
    triviumdb::storage::snapshot::write_manifest(&path, "document-readonly", "f32", 2, 1).unwrap();
    fn files(dir: &TestDir) -> std::collections::BTreeMap<std::ffi::OsString, Vec<u8>> {
        std::fs::read_dir(&dir.0)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), std::fs::read(entry.path()).unwrap())
            })
            .collect()
    }
    let before = files(&dir);
    for access_mode in [AccessMode::ReadOnly, AccessMode::Immutable] {
        let mut db = Database::<f32>::open_with_config(
            &path,
            Config {
                access_mode,
                ..config(StorageMode::Mmap)
            },
        )
        .unwrap();
        assert_eq!(db.get_document(1).unwrap(), Some(snapshot.clone()));
        let status = db.document_upload_status(id).unwrap();
        assert!(status.visible);
        assert_eq!(
            db.publish_document_upload(id).unwrap_err().code(),
            "TDB_READ_ONLY"
        );
        assert_eq!(
            db.abort_document_upload(id).unwrap_err().code(),
            "TDB_READ_ONLY"
        );
        let parsed = parse(&mut db, "plain.txt", "猫");
        assert_eq!(parsed["text"], "猫");
        db.close().unwrap();
        assert_eq!(files(&dir), before);
    }
    let held = snapshot;
    let moved = dir.0.with_extension("moved");
    std::fs::rename(&dir.0, &moved).unwrap();
    std::fs::rename(&moved, &dir.0).unwrap();
    assert_eq!(held.content, "new");
}

#[test]
fn 候选快照恢复可续传且已发布归档维度迁移保持来源() {
    let dir = TestDir::new("document-stage-snapshot");
    let path = dir.file("source.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    let id = "e9737987-d4d4-47aa-a097-58e588350feb";
    request(&mut db, begin(id, "1", "new 猫", Value::Null));
    request(
        &mut db,
        json!({"operation":"append_upload","upload_id":id,"batch":{"sequence":0,"content":"new "}}),
    );
    assert_eq!(
        db.migrate_to(&dir.file("blocked.tdb"), 3)
            .err()
            .unwrap()
            .code(),
        "TDB_INVALID_INPUT"
    );
    let snapshot_dir = dir.file("snapshot");
    db.create_snapshot(&snapshot_dir, "document-stage").unwrap();
    let restored = dir.file("restored");
    triviumdb::storage::snapshot::restore_snapshot(&snapshot_dir, &restored).unwrap();
    let restored_path = std::path::Path::new(&restored).join("database.tdb");
    let mut restored = Database::<f32>::open_with_config(
        restored_path.to_str().unwrap(),
        config(StorageMode::Mmap),
    )
    .unwrap();
    assert_eq!(
        restored.document_upload_status(id).unwrap().next_sequence,
        1
    );
    assert!(restored.all_node_ids().unwrap().is_empty());
    request(
        &mut restored,
        json!({"operation":"append_upload","upload_id":id,"batch":{"sequence":1,"content":"猫"}}),
    );
    let expected = restored.publish_document_upload(id).unwrap();
    assert_eq!(expected.content, "new 猫");
    let (mut migrated, ids) = restored.migrate_to(&dir.file("migrated.tdb"), 3).unwrap();
    assert_eq!(ids, [1]);
    assert_eq!(migrated.get_document(1).unwrap(), Some(expected.clone()));
    assert_eq!(migrated.get(1).unwrap().unwrap().vector, [0., 0., 0.]);
    migrated.close().unwrap();
    let mut migrated = Database::<f32>::open(&dir.file("migrated.tdb"), 3).unwrap();
    assert_eq!(migrated.get_document(1).unwrap(), Some(expected));
    migrated.close().unwrap();
    assert_eq!(db.document_upload_status(id).unwrap().next_sequence, 1);
    db.abort_document_upload(id).unwrap();
}

#[test]
fn 解析行级结构和拆分后的块上限具有独立预期() {
    let dir = TestDir::new("document-parse-lines");
    let mut db = Database::<f32>::open(&dir.file("data.tdb"), 2).unwrap();
    let parsed = request(
        &mut db,
        json!({"operation":"parse","input":{"source_id":"lines.txt","source_revision":"1","content":"甲\n乙\n","config":{"include_lines":true}}}),
    );
    assert_eq!(
        parsed["blocks"],
        json!([
            {"key":"block-0","parent":null,"kind":"paragraph","start_byte":0,"end_byte":7,"node_id":null},
            {"key":"block-0/line-0","parent":"block-0","kind":"line","start_byte":0,"end_byte":3,"node_id":null},
            {"key":"block-0/line-1","parent":"block-0","kind":"line","start_byte":4,"end_byte":7,"node_id":null}
        ])
    );
    let command = json!({"operation":"parse","input":{"source_id":"split.txt","source_revision":"1","content":"abcdefgh\n\nx","config":{"max_block_bytes":4,"max_blocks":3}}});
    assert_eq!(
        triviumdb::sdk::documents::request(&mut db, &command.to_string())
            .unwrap_err()
            .code(),
        "QueueFull"
    );
    assert!(db.all_node_ids().unwrap().is_empty());
}

#[test]
fn 已发布文档数量与候选并发上限独立() {
    let dir = TestDir::new("document-archive-count");
    let mut db =
        Database::<f32>::open_with_config(&dir.file("data.tdb"), config(StorageMode::Rom)).unwrap();
    // 129篇已发布文档越过128个同时待发布候选的上限。
    for id in 1..=129 {
        let upload_id = uuid::Uuid::new_v4().to_string();
        request(
            &mut db,
            begin(&upload_id, &id.to_string(), "new", Value::Null),
        );
        request(
            &mut db,
            json!({"operation":"append_upload","upload_id":upload_id,"batch":{"sequence":0,"content":"new"}}),
        );
        let snapshot = db.publish_document_upload(&upload_id).unwrap();
        assert_eq!(snapshot.document_id, id);
        assert_eq!(snapshot.content, "new");
    }
    assert_eq!(db.list_document_uploads(None).unwrap().len(), 129);
    for id in 1..=129 {
        let status = db.list_document_uploads(Some(id)).unwrap();
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].document_id, id);
        assert_eq!(status[0].state, DocumentUploadState::Published);
        assert!(status[0].visible);
        assert_eq!(db.get_document(id).unwrap().unwrap().content, "new");
    }
}

#[test]
fn Markdown中的HTML保留标题父级和原文换行() {
    let dir = TestDir::new("document-mixed-markdown");
    let mut db = Database::<f32>::open(&dir.file("data.tdb"), 2).unwrap();
    let parsed = parse(&mut db, "mixed.md", "# Title\n\n<div><p>body</p></div>\n");
    assert_eq!(parsed["text"], "Title\nbody\n");
    let blocks = parsed["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 4);
    assert_eq!(blocks[2]["parent"], blocks[0]["key"]);
    assert_eq!(blocks[3]["parent"], blocks[2]["key"]);
    assert_eq!(
        (
            blocks[2]["start_byte"].as_u64().unwrap(),
            blocks[2]["end_byte"].as_u64().unwrap()
        ),
        (6, 11)
    );
}

#[test]
fn 格式选择按显式格式媒体类型和扩展名确定正文() {
    let dir = TestDir::new("document-format-selection");
    let mut db = Database::<f32>::open(&dir.file("data.tdb"), 2).unwrap();
    for (name, media, format, raw, expected) in [
        (
            "name.md",
            "TEXT/HTML; charset=utf-8",
            "auto",
            "**literal**",
            "**literal**",
        ),
        (
            "name.html",
            "application/json",
            "auto",
            r#"{"name":"cat"}"#,
            "name: cat\n",
        ),
        ("name.md", "text/plain", "auto", "# literal", "# literal"),
        (
            "name.pdf",
            "application/pdf",
            "plain",
            "# literal",
            "# literal",
        ),
        (
            "name.md",
            "application/custom",
            "auto",
            "# title",
            "title\n",
        ),
        ("opaque", "text/plain", "markdown", "# title", "title\n"),
    ] {
        let parsed = request(
            &mut db,
            json!({"operation":"parse","input":{"source_id":name,"source_revision":"1","format":format,"media_type":media,"content":raw}}),
        );
        assert_eq!(parsed["text"], expected, "{name} {media} {format}");
    }
    for media in [
        "application/pdf",
        "application/msword",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ] {
        let error=triviumdb::sdk::documents::request(&mut db,&json!({"operation":"parse","input":{"source_id":"name.md","source_revision":"1","media_type":media,"content":"UTF-8占位材料"}}).to_string()).unwrap_err();
        assert_eq!(error.code(), "TDB_INVALID_INPUT");
        assert_eq!(
            error.info().target,
            Some(triviumdb::error::ErrorTarget::Field {
                field: "format".into()
            })
        );
        assert!(error.info().write.is_none());
    }
    assert!(db.all_node_ids().unwrap().is_empty());
}
