//! 文本及索引维护的共享准备、真实调用与事实读取，预期来自独立 JSON。

use serde_json::{Value, json};
use triviumdb::index::text::TextIndexDocument;
use triviumdb::{Database, Result, VectorType};

pub fn contract() -> Value {
    serde_json::from_str(include_str!("../contracts/index_maintenance_cases.json")).unwrap()
}

pub fn seed<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(db: &mut Database<T>) {
    for node in contract()["nodes"].as_array().unwrap() {
        let vector: Vec<T> = node["vector"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| T::from_f32(value.as_f64().unwrap() as f32))
            .collect();
        db.insert_with_id(
            node["id"].as_u64().unwrap(),
            &vector,
            node["payload"].clone(),
        )
        .unwrap();
    }
    db.link(1, 2, "related", 1.).unwrap();
    db.index_text(1, "oldalpha").unwrap();
    db.index_text(2, "oldbeta").unwrap();
    db.index_keyword(1, "oldanchor").unwrap();
    db.publish_generation_manifest("maintenance-seed").unwrap();
}

pub fn execute<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    db: &mut Database<T>,
    case: &Value,
) -> Result<()> {
    let field = || case["field"].as_str().unwrap();
    let fields = || {
        case["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    match case["api"].as_str().unwrap() {
        "index_text" => db.index_text(case["id"].as_u64().unwrap(), case["text"].as_str().unwrap()),
        "index_keyword" => db.index_keyword(
            case["id"].as_u64().unwrap(),
            case["keyword"].as_str().unwrap(),
        ),
        "build_text_index" => db.build_text_index(),
        "rebuild_text_index" => {
            let documents: Vec<TextIndexDocument> = contract()["rebuilt_documents"]
                .as_array()
                .unwrap()
                .iter()
                .map(|document| TextIndexDocument {
                    id: document["id"].as_u64().unwrap(),
                    text: document["text"].as_str().unwrap().to_owned(),
                    keywords: document["keywords"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|value| value.as_str().unwrap().to_owned())
                        .collect(),
                })
                .collect();
            db.rebuild_text_index(&documents)
        }
        "create_index" => db.create_index(field()),
        "create_composite_index" => db.create_composite_index(&fields()),
        "create_bitmap_index" => db.create_bitmap_index(field()),
        "create_ordered_index" => db.create_ordered_index(field()),
        "create_ngram_index" => db.create_ngram_index(field()),
        "create_unique_index" => db.create_unique_index(field()),
        "create_unique_composite_index" => db.create_unique_composite_index(&fields()),
        "drop_index" => db.drop_index(field()),
        "drop_composite_index" => db.drop_composite_index(&fields()),
        "drop_bitmap_index" => db.drop_bitmap_index(field()),
        "drop_ordered_index" => db.drop_ordered_index(field()),
        "drop_ngram_index" => db.drop_ngram_index(field()),
        "build_quiver_index" => db.build_quiver_index(None),
        "flush" => db.flush(),
        "compact" => db.compact(),
        "set_sync_mode" => db.set_sync_mode(
            triviumdb::storage::wal::SyncMode::parse(case["mode"].as_str().unwrap()).unwrap(),
        ),
        "reserve_nodes" => db.reserve_nodes(case["additional"].as_u64().unwrap() as usize),
        "repair_graph_indexes" => db.repair_graph_indexes().map(|_| ()),
        "set_auto_build_quiver" => db.set_auto_build_quiver(case["enabled"].as_bool().unwrap()),
        "enable_auto_compaction" => db.enable_auto_compaction(std::time::Duration::from_secs(
            case["interval_secs"].as_u64().unwrap(),
        )),
        "enable_quiver_corrected_sm" => db.enable_quiver_corrected_sm(&[]),
        "enable_quiver_sm_l2" => db.enable_quiver_sm_l2(),
        "enable_quiver_sm_native_l2" => db.enable_quiver_sm_native_l2(),
        "prepare_auto_quiver_build" => db.prepare_auto_quiver_build().map(|_| ()),
        other => panic!("未知维护操作 {other}"),
    }
}

pub fn facts<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    db: &Database<T>,
) -> Value {
    let mut ids = db.all_node_ids().unwrap();
    ids.sort_unstable();
    let nodes: Vec<Value> = ids.into_iter().map(|id| {
        let node = db.get(id).unwrap().unwrap();
        let mut edges = node.edges;
        edges.sort_by(|a, b| (a.target_id, &a.label).cmp(&(b.target_id, &b.label)));
        json!({
            "id":id, "vector":node.vector.iter().map(|value| value.to_f32()).collect::<Vec<_>>(),
            "payload":node.payload,
            "edges":edges.iter().map(|edge| json!({"target":edge.target_id,"label":edge.label,
                "weight":edge.weight,"metadata":edge.metadata})).collect::<Vec<_>>(),
        })
    }).collect();
    let queries: Vec<Value> = contract()["queries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|query| {
            let statement = format!(
                "TEXT {} \"{}\" TOP 10 AS hits WITH hits RETURN hits",
                query["mode"].as_str().unwrap(),
                query["term"].as_str().unwrap()
            );
            let mut ids: Vec<u64> = db
                .tql_values(&statement)
                .unwrap()
                .iter()
                .map(|row| row["hits"].as_node().unwrap().id)
                .collect();
            ids.sort_unstable();
            json!(ids)
        })
        .collect();
    let indexes: Vec<Value> = db.index_info().unwrap().iter()
        .map(|index| json!({"kind":format!("{:?}", index.kind),"fields":index.fields,"unique":index.unique})).collect();
    json!({"nodes":nodes,"queries":queries,"indexes":indexes})
}

pub fn expected(after: bool, indexes: Value) -> Value {
    let contract = contract();
    let queries: Vec<Value> = contract["queries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|query| query[if after { "after" } else { "before" }].clone())
        .collect();
    json!({"nodes":contract["nodes"],"queries":queries,"indexes":indexes})
}
