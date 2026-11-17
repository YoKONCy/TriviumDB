//! 使用不可变 v0.9.1 源码生成文档、命名图与持久回执样本。
use triviumdb::{Database, database::{Config, StorageMode}};
use serde_json::{Value,json};
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let source = std::path::PathBuf::from(&args[1]);
    let mut db = Database::<f32>::open_with_config(
        source.to_str().unwrap(),
        Config {
            dim: 2,
            storage_mode: if args[2] == "rom" { StorageMode::Rom } else { StorageMode::Mmap },
            auto_build_quiver: false,
            ..Default::default()
        },
    )
    .unwrap();
    let call = |db: &mut Database<f32>, request: Value| -> Value {
        serde_json::from_str(&triviumdb::sdk::documents::request(db, &request.to_string()).unwrap())
            .unwrap()
    };
    let parsed = call(
        &mut db,
        json!({"operation":"parse","input":{"source_id":"note.md","source_revision":"r1","content":"# 标题\n\n甲乙"}}),
    );
    let document = call(
        &mut db,
        json!({"operation":"put_parsed","document":{"parsed":parsed,"root":{"id":"10","vector":[1,0],"payload":{"kind":"document"}}},"expected_revision":null}),
    );
    db.insert_with_id(20, &[0., 1.], json!({"kind":"before"}))
        .unwrap();
    let graph: triviumdb::named_graph::NamedGraphWrite = serde_json::from_value(
        json!({"graph_id":"词","members":["10","20"],"edges":[{"source_id":"10","target_id":"20","label":"关联","weight":0.5,"metadata":{"source":"migration"}}],"metadata":{"kind":"shared"}}),
    )
    .unwrap();
    let graph = db.put_named_graph(graph, None).unwrap();
    let session = triviumdb::ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    let fetch = json!({"version":1,"requestId":"读取","command":{"operation":"getNodes","ids":["20"],"withRevision":true}});
    let revision: Value =
        serde_json::from_str(&db.console_request(&session, &fetch.to_string()).unwrap()).unwrap();
    let command = json!({"version":1,"requestId":"修改","command":{"operation":"guardedTransaction","expectedRevision":revision["data"]["revision"],"idempotencyKey":"migration-receipt","operations":[{"op":"updatePayload","id":"20","payload":{"kind":"updated"}}]}});
    let acknowledged = db.console_request(&session, &command.to_string()).unwrap();
    db.close().unwrap();
    drop(db);
    let expected = json!({"document":document,"graph":graph,"command":command,"acknowledged":serde_json::from_str::<Value>(&acknowledged).unwrap()});
    std::fs::write(source.parent().unwrap().join("expected.json"),serde_json::to_vec_pretty(&expected).unwrap()).unwrap();
}
