//! 使用 v0.9.1 原始源码生成文档结构 1、片段标签和已确认回执。
use serde_json::{Value, json};
use triviumdb::database::{Config, StorageMode};
use triviumdb::{ConsoleSession, Database};

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let path = std::path::Path::new(&args[1]);
    let mut db = Database::<f32>::open_with_config(&args[1], Config {
        dim: 2, auto_build_quiver: false,
        storage_mode: if args[2] == "rom" { StorageMode::Rom } else { StorageMode::Mmap },
        ..Default::default()
    }).unwrap();
    let document = serde_json::from_value(json!({
        "source_id":"plain.txt","source_revision":"r1","parser_id":"plain","content":"甲乙",
        "root":{"id":"7","vector":[1,0],"payload":{"key":"root","rank":1}},
        "blocks":[{"key":"paragraph","parent":null,"kind":"paragraph","start_byte":0,"end_byte":6,
            "node":{"id":"8","vector":[0,1],"payload":{"key":"block","rank":2}}}]
    })).unwrap();
    let snapshot = db.put_document(document,None).unwrap();
    assert_eq!(snapshot.schema_version,1);
    db.index_text(7,"显式正文 alpha").unwrap();
    db.index_keyword(8,"结构锚点").unwrap();
    for (id, code) in [(7,"root"),(8,"block")] {
        let mut payload=db.get_payload(id).unwrap().unwrap();
        payload["code"]=json!(code);
        db.update_payload(id,payload).unwrap();
    }
    db.create_unique_index("code").unwrap();
    db.insert_with_id(9,&[1.,1.],json!({"state":"before"})).unwrap();
    let session=ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    let fetched: Value=serde_json::from_str(&db.console_request(&session,&json!({
        "version":1,"requestId":"get","command":{"operation":"getNodes","ids":["9"],"withRevision":true}
    }).to_string()).unwrap()).unwrap();
    let command=json!({"operation":"guardedTransaction","expectedRevision":fetched["data"]["revision"],
        "idempotencyKey":"schema1-migration","operations":[{"op":"updatePayload","id":"9","payload":{"state":"after"}}]});
    let response:Value=serde_json::from_str(&db.console_request(&session,&json!({
        "version":1,"requestId":"update","command":command
    }).to_string()).unwrap()).unwrap();
    let expected=json!({"snapshot":snapshot,"root":db.get_payload(7).unwrap().unwrap(),
        "fragment":db.get_payload(8).unwrap().unwrap(),"edges":db.get_edges(7).unwrap(),
        "command":command,"acknowledged":response["data"]});
    db.close().unwrap();
    std::fs::write(path.parent().unwrap().join("expected.json"),serde_json::to_vec_pretty(&expected).unwrap()).unwrap();
}
