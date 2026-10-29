use triviumdb::query::tql_lexer::{TqlLexer, TqlToken};
use triviumdb::query::tql_parser::{
    TqlParser, parse_tql, parse_tql_statement, parse_tql_statement_with_pos, parse_tql_with_pos,
};
use triviumdb::{Database, TriviumError};

#[test]
fn oversized_expressions_are_rejected_in_small_stack_process() {
    let report = std::env::temp_dir().join(format!(
        "tdb-parser-worker-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut child = super::query_child_command()
        .args([
            "--ignored",
            "--exact",
            "parser_limits::small_stack_worker",
            "--nocapture",
        ])
        .env("TDB_PARSER_LIMIT_WORKER", "1")
        .env("TDB_PARSER_WORKER_REPORT", &report)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("小栈子进程超过等待上限");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(status.success(), "小栈子进程必须正常返回错误：{status}");
    // 回执在关键断言全部完成后写出，未选择到子测试也必须失败。
    assert_eq!(
        std::fs::read(&report).unwrap(),
        b"parser-boundaries-completed"
    );
    std::fs::remove_file(report).unwrap();
}

#[test]
#[ignore = "由父用例在独立进程执行"]
fn small_stack_worker() {
    assert_eq!(std::env::var("TDB_PARSER_LIMIT_WORKER").unwrap(), "1");
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let inputs = [
                format!("MATCH (n) WHERE {}n.x == 1 RETURN n", "NOT ".repeat(16_384)),
                format!("MATCH (n) RETURN n.x{} AS v", " + 1".repeat(65_536)),
                format!("MATCH (n) RETURN n.x{} AS v", " * 1".repeat(65_536)),
                format!(
                    "MATCH (n) WHERE {}n.x == 1 RETURN n",
                    "n.x == 1 OR ".repeat(16_384)
                ),
                format!(
                    "MATCH (n) RETURN {}n.x{} AS v",
                    "COALESCE(".repeat(512),
                    ", NULL)".repeat(512)
                ),
            ];
            for input in inputs {
                assert!(parse_tql(&input).is_err());
                assert!(parse_tql_statement(&input).is_err());
                assert!(parse_tql_with_pos(&input).is_err());
                assert!(parse_tql_statement_with_pos(&input).is_err());
            }
        })
        .unwrap()
        .join()
        .unwrap();
    std::fs::write(
        std::env::var("TDB_PARSER_WORKER_REPORT").unwrap(),
        b"parser-boundaries-completed",
    )
    .unwrap();
}

#[test]
fn expression_and_literal_boundaries() {
    // 128 个二元运算仍可构造、检查和析构，129 个在构造前拒绝。
    assert!(parse_tql(&format!("MATCH (n) RETURN n.x{} AS v", " + 1".repeat(128))).is_ok());
    assert!(parse_tql(&format!("MATCH (n) RETURN n.x{} AS v", " + 1".repeat(129))).is_err());
    assert!(
        parse_tql(&format!(
            "MATCH (n) RETURN {}1{} AS v",
            "(".repeat(127),
            ")".repeat(127)
        ))
        .is_ok()
    );
    assert!(
        parse_tql(&format!(
            "MATCH (n) RETURN {}1{} AS v",
            "(".repeat(129),
            ")".repeat(129)
        ))
        .is_err()
    );
    // 负向量字面量的分量不消耗表达式运算预算，覆盖内核最大维度。
    let vector = vec!["-1"; 65_536].join(",");
    assert!(parse_tql(&format!("SEARCH VECTOR [{vector}] TOP 1 RETURN *")).is_ok());
    assert!(
        TqlLexer::new(&" ".repeat(4 * 1024 * 1024 + 1))
            .tokenize()
            .is_err()
    );
    assert!(TqlLexer::new(&"x ".repeat(262_145)).tokenize().is_err());
}

#[test]
fn every_entry_rejects_unconsumed_tail() {
    for input in [
        "MATCH (n) RETURN n LIMIT 1 WHERE n.x == 2",
        "MATCH (n) RETURN n RETURN n",
        "FIND {} RETURN * garbage",
    ] {
        assert!(parse_tql(input).is_err(), "{input}");
        assert!(parse_tql_statement(input).is_err(), "{input}");
        assert!(parse_tql_with_pos(input).is_err(), "{input}");
        assert!(parse_tql_statement_with_pos(input).is_err(), "{input}");
        let mut tokens = TqlLexer::new(input).tokenize().unwrap();
        assert!(TqlParser::new(tokens.clone()).parse_query().is_err());
        tokens.insert(0, TqlToken::Eof);
        assert!(TqlParser::new(tokens).parse_statement().is_err());
    }
}

#[test]
fn invalid_mutation_tail_preserves_all_facts_and_wal() {
    let root = std::env::temp_dir().join(format!(
        "tdb-parser-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("data.tdb").to_str().unwrap().to_owned();
    let mut db = Database::<f32>::open(&path, 1).unwrap();
    db.insert_with_id(1, &[1.0], serde_json::json!({"x":1}))
        .unwrap();
    db.insert_with_id(2, &[2.0], serde_json::json!({"x":2}))
        .unwrap();
    db.flush().unwrap();
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    for query in [
        "MATCH (n) SET n.x == 9 WHERE n.x == 999",
        "MATCH (n) DELETE n WHERE n.x == 999",
        "CREATE (n {x:9}) garbage",
        "MATCH (n) SET VECTOR(n) == [9] RETURN n",
    ] {
        assert!(
            matches!(db.tql_mut(query), Err(TriviumError::QueryParse(_))),
            "{query}"
        );
        assert_eq!(
            db.get(1).unwrap().unwrap().payload,
            serde_json::json!({"x":1})
        );
        assert_eq!(
            db.get(2).unwrap().unwrap().payload,
            serde_json::json!({"x":2})
        );
        assert_eq!(db.node_count().unwrap(), 2);
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    }
    db.close().unwrap();
    drop(db);
    let mut reopened = Database::<f32>::open(&path, 1).unwrap();
    assert_eq!(
        reopened.get(1).unwrap().unwrap().payload,
        serde_json::json!({"x":1})
    );
    assert_eq!(
        reopened.get(2).unwrap().unwrap().payload,
        serde_json::json!({"x":2})
    );
    reopened.close().unwrap();
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}
