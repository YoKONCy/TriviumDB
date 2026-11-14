//! 文档来源、图作用域及统一物理管线的独立内容契约。
use super::support::TestDir;
use serde_json::{Value, json};
use triviumdb::Database;
use triviumdb::document::*;
use triviumdb::named_graph::*;
use triviumdb::query::tql_executor::TqlValue;

fn fixture() -> Value {
    serde_json::from_str(include_str!("../contracts/tql_documents_graphs_cases.json")).unwrap()
}

fn write_document<T: triviumdb::VectorType + serde::de::DeserializeOwned>(
    row: &Value,
    binary: bool,
) -> DocumentWrite<T> {
    serde_json::from_value(json!({
        "source_id": format!("notes://{}", row["id"].as_str().unwrap()),
        "source_revision": "v1", "parser_id": "plain-v1", "content": "alpha",
        "root": {"id":row["id"], "vector":if binary { json!([0,0]) } else { row["root_vector"].clone() }, "payload":{"active":true}},
        "blocks":[{"key":"body", "kind":"body", "start_byte":0, "end_byte":5,
            "node":{"id":row["node"], "vector":row[if binary { "binary_vector" } else { "vector" }], "payload":{"active":true}}}]
    })).unwrap()
}

fn database<T: triviumdb::VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    path: &str,
    dtype: &str,
) -> Database<T> {
    let mut db = Database::<T>::open_with_runtime(
        path,
        triviumdb::database::Config {
            dim: 2,
            storage_mode: triviumdb::database::StorageMode::Mmap,
            auto_build_quiver: false,
            ..Default::default()
        },
        triviumdb::ResourceRuntime::new(Default::default()).unwrap(),
    )
    .unwrap();
    let fixture = fixture();
    for row in fixture["documents"].as_array().unwrap() {
        db.put_document(write_document(row, dtype == "u64"), None)
            .unwrap();
    }
    db.link(11, 31, "rel", 50.).unwrap();
    for graph in fixture["graphs"].as_array().unwrap() {
        db.put_named_graph(serde_json::from_value(graph.clone()).unwrap(), None)
            .unwrap();
    }
    db
}

fn json_rows<T: std::fmt::Debug>(
    rows: Vec<std::collections::HashMap<String, TqlValue<T>>>,
) -> Vec<Value> {
    rows.into_iter()
        .map(|row| {
            Value::Object(
                row.into_iter()
                    .map(|(name, value)| {
                        let value = match value {
                            TqlValue::Int(value) => value.into(),
                            TqlValue::UInt(value) => value.into(),
                            TqlValue::Float(value) => json!(value),
                            TqlValue::String(value) => value.into(),
                            TqlValue::Bool(value) => value.into(),
                            TqlValue::Object(value) => Value::Object(value),
                            TqlValue::Null => Value::Null,
                            TqlValue::Path(path) => {
                                json!(path.iter().map(ToString::to_string).collect::<Vec<_>>())
                            }
                            other => panic!("用例出现非预期值 {other:?}"),
                        };
                        let value =
                            if ["id", "a", "b"].contains(&name.as_str()) && value.is_number() {
                                value.to_string().into()
                            } else {
                                value
                            };
                        (name, value)
                    })
                    .collect(),
            )
        })
        .collect()
}

fn assert_content(actual: &Value, expected: &Value) {
    match (actual, expected) {
        (Value::Number(actual), Value::Number(expected)) if expected.is_f64() => {
            assert!(
                (actual.as_f64().unwrap() - expected.as_f64().unwrap()).abs() < 1e-6,
                "{actual} != {expected}"
            );
        }
        (Value::Array(actual), Value::Array(expected)) => {
            assert_eq!(actual.len(), expected.len());
            for (actual, expected) in actual.iter().zip(expected) {
                assert_content(actual, expected);
            }
        }
        (Value::Object(actual), Value::Object(expected)) => {
            assert_eq!(
                actual.keys().collect::<Vec<_>>(),
                expected.keys().collect::<Vec<_>>()
            );
            for (key, expected) in expected {
                assert_content(&actual[key], expected);
            }
        }
        _ => assert_eq!(actual, expected),
    }
}

#[test]
fn 文档来源与命名边在统一TQL中返回完整身份和独立分数() {
    fn run<
        T: triviumdb::VectorType + serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug,
    >(
        dtype: &str,
    ) {
        let root = TestDir::new("tql-documents-graphs");
        let file = root.file("data.tdb");
        let mut db = database::<T>(&file, dtype);
        for case in fixture()["cases"].as_array().unwrap() {
            if !case["dtypes"].as_array().unwrap().contains(&json!(dtype)) {
                continue;
            }
            let query = case["query"].as_str().unwrap();
            let rows = db
                .tql(query)
                .unwrap_or_else(|error| panic!("{} {dtype}: {error:?}", case["name"]));
            assert_content(&json!(json_rows(rows)), &case["expected"]);
        }
        db.close().unwrap();
    }
    run::<f32>("f32");
    run::<half::f16>("f16");
    run::<u64>("u64");
}

fn plan(db: &Database<f32>, query: &str) -> Value {
    let mut rows = db.tql(&format!("EXPLAIN {query}")).unwrap();
    assert_eq!(rows.len(), 1);
    match rows.remove(0).remove("plan").unwrap() {
        TqlValue::Node(node) => node.payload,
        _ => panic!("规划入口必须返回完整计划"),
    }
}

#[test]
fn 控制台查询协议保留跨图边身份及完整内容() {
    let root = TestDir::new("tql-graph-console");
    let mut db = database::<f32>(&root.file("data.tdb"), "f32");
    let session = triviumdb::ConsoleSession::from_json("{}").unwrap();
    let request = json!({"version":1,"requestId":"graph-query","command":{
        "operation":"query",
        "query":"GRAPH {graph_ids:['words','events']} MATCH (a)-[e:rel]->(b) RETURN e ORDER BY e.graph_id,a.id,b.id"
    }});
    let output: Value =
        serde_json::from_str(&db.console_request(&session, &request.to_string()).unwrap()).unwrap();
    assert_eq!(output["requestId"], "graph-query");
    assert_eq!(output["data"]["rowCount"], 3);
    assert_eq!(
        output["data"]["rows"],
        json!([
            {"e":{"type":"edge","source":"11","target":"21","label":"rel","weight":9.0,"metadata":{"owner":"events"},"graph_id":"events"}},
            {"e":{"type":"edge","source":"21","target":"31","label":"rel","weight":3.0,"metadata":{},"graph_id":"events"}},
            {"e":{"type":"edge","source":"11","target":"21","label":"rel","weight":2.0,"metadata":{"owner":"words"},"graph_id":"words"}}
        ])
    );
    db.close().unwrap();
}

#[test]
fn 文档与图规划保留同域统计且Prepared绑定配置叶节点() {
    use triviumdb::query::tql_prepared::TqlParamValue as Param;
    let root = TestDir::new("tql-document-plan");
    let mut db = database::<f32>(&root.file("data.tdb"), "f32");
    let query = "GRAPH {graph_ids:[$graph]} DOCUMENTS {operation:'search',query:{text:$text,document_filter:{active:$active}}} WITH _ AS seed,document AS evidence EXPAND seed [:rel*1..1] AS related RETURN related.id AS id,evidence.document_id AS doc ORDER BY id,doc";
    let prepared = db.prepare_tql(query).unwrap();
    assert_eq!(prepared.parameter_names(), ["active", "graph", "text"]);
    let parameters = std::collections::HashMap::from([
        ("graph".into(), Param::String("words".into())),
        ("text".into(), Param::String("alpha".into())),
        ("active".into(), Param::Bool(true)),
    ]);
    assert_content(
        &json!(json_rows(
            db.execute_prepared_tql(&prepared, &parameters).unwrap()
        )),
        &json!([{"id":"21","doc":"10"}]),
    );
    let error = db
        .execute_prepared_tql(&prepared, &std::collections::HashMap::new())
        .unwrap_err();
    assert_eq!(error.code(), "TDB_MISSING_QUERY_PARAMETER");
    let target = serde_json::to_value(error.info().target).unwrap();
    assert_eq!(target["parameter"], "active");
    assert_eq!(
        target["query_span"]["start_byte"],
        query.find("$active").unwrap()
    );

    let graph_plan = plan(
        &db,
        "GRAPH {graph_ids:['words']} MATCH (n) WITH n AS seed degree seed AS ranked RETURN ranked.id AS id,graph_score(ranked) AS score",
    );
    assert_eq!(graph_plan["selected_graph_stats"]["node_count"], 3);
    assert_eq!(graph_plan["selected_graph_stats"]["edge_count"], 1);
    assert_eq!(graph_plan["access_path"]["kind"], "named_graph_members");
    assert_eq!(graph_plan["estimated_rows"], 3);
    assert!(
        graph_plan["graph_revisions"]["words"]
            .as_str()
            .unwrap()
            .ends_with(":1")
    );
    let document_plan = plan(
        &db,
        "DOCUMENTS {operation:'search_vector',query:{max_documents:3},vector:[1,0],options:{strategy:'exact'}} RETURN document",
    );
    assert_eq!(document_plan["entry"], "DOCUMENTS");
    assert_eq!(
        document_plan["pipeline_stages"][0]["operator"],
        "document_exact_search"
    );
    assert_eq!(document_plan["pipeline_stages"][0]["exact"], true);
    let analyzed = plan(
        &db,
        "ANALYZE DOCUMENTS {operation:'search',query:{text:'alpha',max_documents:3,hits_per_document:1}} RETURN document",
    );
    assert_eq!(analyzed["actual_rows"], 3);
    assert_eq!(
        analyzed["pipeline_stage_actuals"][0]["operator"],
        "document_search"
    );
    assert_eq!(analyzed["pipeline_stage_actuals"][0]["actual_rows"], 3);
    db.close().unwrap();
}

#[test]
fn 文档及图错误保留阶段范围修订与失败重试() {
    let root = TestDir::new("tql-document-errors");
    let mut db = database::<f32>(&root.file("data.tdb"), "f32");
    for (query, code, stage, selected) in [
        (
            "GRAPH {graph_ids:['缺失']} MATCH (n) RETURN n",
            "TDB_GRAPH_NOT_FOUND",
            "plan",
            "GRAPH {graph_ids:['缺失']}",
        ),
        (
            "GRAPH {graph_ids:['words','words']} MATCH (n) RETURN n",
            "TDB_INVALID_INPUT",
            "plan",
            "GRAPH {graph_ids:['words','words']}",
        ),
        (
            "DOCUMENTS {operation:'search',query:{text:''}} RETURN document",
            "TDB_INVALID_INPUT",
            "plan",
            "DOCUMENTS {operation:'search',query:{text:''}}",
        ),
        (
            "DOCUMENTS {operation:'search_vector',query:{limits:{max_vector_scores:0}},vector:[1,0],options:{strategy:'exact'}} RETURN document",
            "QueueFull",
            "execute",
            "DOCUMENTS {operation:'search_vector',query:{limits:{max_vector_scores:0}},vector:[1,0],options:{strategy:'exact'}}",
        ),
    ] {
        let error = db.tql(query).unwrap_err();
        let info = serde_json::to_value(error.info()).unwrap();
        assert_eq!(error.code(), code, "{query}");
        assert_eq!(info["stage"], stage);
        assert_eq!(
            info["target"]["query_span"]["start_byte"],
            query.find(selected).unwrap()
        );
        assert_eq!(
            info["target"]["query_span"]["end_byte"],
            query.find(selected).unwrap() + selected.len()
        );
    }
    let old = db
        .get_named_graph("words", &NamedGraphLimits::default())
        .unwrap()
        .unwrap();
    let stale = format!(
        "GRAPH {{graph_ids:['words'],expected_revisions:{{words:'{}'}}}} MATCH (n) RETURN n.id AS id",
        old.revision
    );
    db.edit_named_graph(
        "words",
        &old.revision,
        vec![NamedGraphEdit::SetMetadata {
            metadata: json!({"v":2}),
        }],
    )
    .unwrap();
    assert_eq!(
        db.tql(&stale).unwrap_err().code(),
        "TDB_GRAPH_REVISION_MISMATCH"
    );
    assert_content(
        &json!(json_rows(
            db.tql("GRAPH {graph_ids:['words']} MATCH (n) RETURN n.id AS id ORDER BY id")
                .unwrap()
        )),
        &json!([{"id":"11"},{"id":"21"},{"id":"31"}]),
    );
    db.close().unwrap();
}

#[test]
fn 图作用域内存准入拒绝后归还工作区并支持重试() {
    let root = TestDir::new("tql-graph-budget");
    let mut db = database::<f32>(&root.file("data.tdb"), "f32");
    let query = "GRAPH {graph_ids:['words']} MATCH (n) RETURN n.id AS id ORDER BY id";
    // 只保留一字节额外空间，足以接纳配置，无法物化任何成员工作区。
    db.set_memory_limit(db.estimated_memory() + 1).unwrap();
    let baseline = db.resource_stats().reserved_workspace_bytes;
    let error = db.tql(query).unwrap_err();
    assert_eq!(error.code(), "TDB_CAPACITY_RESERVATION_REJECTED");
    let info = serde_json::to_value(error.info()).unwrap();
    assert_eq!(info["stage"], "plan");
    assert_eq!(info["target"]["query_span"]["start_byte"], 0);
    assert_eq!(
        info["target"]["query_span"]["end_byte"],
        "GRAPH {graph_ids:['words']}".len()
    );
    assert_eq!(db.resource_stats().reserved_workspace_bytes, baseline);
    db.set_memory_limit(0).unwrap();
    assert_content(
        &json!(json_rows(db.tql(query).unwrap())),
        &json!([{"id":"11"},{"id":"21"},{"id":"31"}]),
    );
    assert_eq!(db.resource_stats().reserved_workspace_bytes, baseline);
    db.close().unwrap();
}

#[test]
fn 文档压缩来源共用冷热精度并在重开后保留证据与只读零写() {
    use triviumdb::index::quiver::ssd::SsdRuntimeOptions;
    let root = TestDir::new("tql-document-compressed");
    let file = root.file("data.tdb");
    let mut db = database::<f32>(&file, "f32");
    let query = "DOCUMENTS {operation:'search_vector',query:{max_documents:3,hits_per_document:1},vector:[1,0],options:{strategy:'ann',rerank:'compressed'}} RETURN _.id AS id,document.document_id AS doc,document.score AS score,similarity(_) AS cosine";
    let expected = fixture()["cases"][0]["expected"].clone();
    for encoding in ["int8", "centered_int8", "centered_int4"] {
        for precision in ["fp32", "int8", "int4"] {
            for residency in ["ssd", "hot"] {
                db.configure_quiver(serde_json::from_value(json!({"encoding":encoding,"query_precision":precision,"residency":residency,"cache_bytes":8192,"io_depth":1,"use_for_ann":false})).unwrap()).unwrap();
                assert_content(&json!(json_rows(db.tql(query).unwrap())), &expected);
                let compressed_plan = plan(&db, query);
                assert_eq!(
                    compressed_plan["pipeline_stages"][0]["operator"],
                    "document_ann_search"
                );
                assert_eq!(compressed_plan["pipeline_stages"][0]["exact"], false);
            }
        }
    }
    db.configure_quiver(SsdRuntimeOptions::compact()).unwrap();
    let evidence = db.tql("DOCUMENTS {operation:'search',query:{text:'alpha',max_documents:1,include_matches:true}} RETURN document.evidence AS evidence,document.matches AS windows").unwrap();
    match &evidence[0]["evidence"] {
        TqlValue::Object(value) => {
            assert_eq!(value["text"], "alpha");
            assert_eq!(value["reference"]["start_byte"], 0);
            assert_eq!(value["reference"]["end_byte"], 5);
        }
        _ => panic!("必须返回原文证据"),
    }
    match &evidence[0]["windows"] {
        TqlValue::List(matches) => {
            assert_eq!(matches.len(), 1);
            assert_eq!(
                matches[0]["terms"],
                json!([{"term":"alpha","position":0,"start_byte":0,"end_byte":5}])
            );
            match &evidence[0]["evidence"] {
                TqlValue::Object(expected) => {
                    assert_eq!(matches[0]["evidence"], Value::Object(expected.clone()))
                }
                _ => panic!("必须返回原文证据"),
            }
        }
        _ => panic!("必须返回完整词项窗口"),
    }
    let graph = db
        .get_named_graph("words", &NamedGraphLimits::default())
        .unwrap()
        .unwrap();
    db.edit_named_graph(
        "words",
        &graph.revision,
        vec![NamedGraphEdit::SetMetadata {
            metadata: json!({"checked":true}),
        }],
    )
    .unwrap();
    db.close().unwrap();
    let snapshot = || {
        std::fs::read_dir(&root.0)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), std::fs::read(entry.path()).unwrap())
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let before = snapshot();
    let mut reader = Database::<f32>::open_read_only(&file, 2).unwrap();
    reader
        .configure_quiver(SsdRuntimeOptions::compact())
        .unwrap();
    assert_content(&json!(json_rows(reader.tql(query).unwrap())), &expected);
    reader.close().unwrap();
    assert_eq!(snapshot(), before);
    std::fs::rename(&file, root.file("moved.tdb")).unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn 直接执行器控制句柄在图物化阶段传播取消() {
    use triviumdb::query::tql_executor::{QueryControl, TqlLimits, execute_tql_values_with_limits};
    use triviumdb::query::tql_parser::parse_tql;
    use triviumdb::storage::memtable::MemTable;
    use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};
    let query = parse_tql("GRAPH {include_default:true} MATCH (n) RETURN n").unwrap();
    let control = QueryControl::default();
    let worker_control = control.clone();
    let gate = pause_at(ConcurrencyPoint::NamedGraphRead);
    let worker = std::thread::spawn(move || {
        let mt = MemTable::<f32>::new(2);
        let error = execute_tql_values_with_limits(
            &query,
            &mt,
            TqlLimits {
                query_control: Some(worker_control),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), "TDB_QUERY_CANCELLED");
        assert_eq!(
            error.info().stage,
            Some(triviumdb::error::QueryErrorStage::Plan)
        );
        assert!(
            execute_tql_values_with_limits(&query, &mt, TqlLimits::default())
                .unwrap()
                .is_empty()
        );
    });
    gate.wait_until_arrived();
    control.cancel();
    drop(gate);
    worker.join().unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn 组合查询在文档图读取与导航处取消均归还额度且允许完整重试() {
    use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};
    for (point, query) in [
        (
            ConcurrencyPoint::NamedGraphRead,
            "GRAPH {graph_ids:['events']} MATCH (n) RETURN n.id AS id ORDER BY id",
        ),
        (
            ConcurrencyPoint::TqlPathStep,
            "GRAPH {graph_ids:['events']} MATCH (n {id:11})-[:rel*1..2]->(b) RETURN b.id AS id ORDER BY id",
        ),
        (
            ConcurrencyPoint::DocumentVectorStep,
            "DOCUMENTS {operation:'search_vector',query:{max_documents:3,hits_per_document:1},vector:[1,0],options:{strategy:'exact'}} RETURN _.id AS id,document.score AS score",
        ),
        (
            ConcurrencyPoint::BeforeTqlProjection,
            "DOCUMENTS {operation:'search_vector',query:{max_documents:3,hits_per_document:1},vector:[1,0],options:{strategy:'exact'}} RETURN _.id AS id,document.score AS score",
        ),
    ] {
        let root = TestDir::new("tql-document-cancel");
        let db = database::<f32>(&root.file("data.tdb"), "f32");
        let expected = json_rows(db.tql(query).unwrap());
        let baseline = db.resource_stats().reserved_workspace_bytes;
        let context =
            triviumdb::sdk::RequestContext::new("tql-document-cancel", Default::default(), false)
                .unwrap();
        let gate = pause_at(point);
        let db = std::sync::Arc::new(db);
        let worker_db = db.clone();
        let worker_context = context.clone();
        let worker = std::thread::spawn(move || worker_context.run(|| worker_db.tql(query)));
        gate.wait_until_arrived();
        let held = db.resource_stats().reserved_workspace_bytes;
        context.cancel();
        drop(gate);
        assert_eq!(
            worker.join().unwrap().unwrap_err().code(),
            "TDB_QUERY_CANCELLED"
        );
        assert!(
            held > baseline,
            "{point:?} 的持有额度 {held}，基线 {baseline}"
        );
        assert_eq!(db.resource_stats().reserved_workspace_bytes, baseline);
        assert_eq!(json_rows(db.tql(query).unwrap()), expected);
        let mut db = std::sync::Arc::into_inner(db).unwrap();
        db.close().unwrap();
        std::fs::rename(root.file("data.tdb"), root.file("moved.tdb")).unwrap();
    }
}
