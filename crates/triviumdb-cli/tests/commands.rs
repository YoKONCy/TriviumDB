//! CLI 集成测试：通过 assert_cmd 测试 `tdb` 二进制的端到端行为。
//!
//! 每个测试创建临时数据库，运行子命令，验证输出与退出码。

use std::io::Write;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use tempfile::TempDir;

/// 创建临时数据库并写入 2 个节点 + 1 条边，返回 (TempDir, 数据库路径)。
///
/// `TempDir` 必须保持存活，否则临时目录会被删除。
fn seed_db() -> (TempDir, String) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.tdb");
    let path_str = path.to_str().unwrap().to_string();
    {
        let mut db = triviumdb::Database::<f32>::open(&path_str, 4).unwrap();
        db.insert(
            &[1.0, 0.0, 0.0, 0.0],
            json!({"name": "Alice", "type": "person"}),
        )
        .unwrap();
        db.insert(
            &[0.0, 1.0, 0.0, 0.0],
            json!({"name": "Bob", "type": "person"}),
        )
        .unwrap();
        db.link(1, 2, "knows", 1.0).unwrap();
        db.flush().unwrap();
    }
    (dir, path_str)
}

fn tdb() -> Command {
    Command::cargo_bin("tdb").unwrap()
}

#[test]
fn repair_check明确拒绝只有_wal_的数据库() {
    let directory = TempDir::new().unwrap();
    let source = directory.path().join("source.tdb");
    let source = source.to_str().unwrap();
    let mut db = triviumdb::Database::<f32>::open_with_config(
        source,
        triviumdb::database::Config {
            dim: 2,
            sync_mode: triviumdb::storage::wal::SyncMode::Full,
            ..Default::default()
        },
    )
    .unwrap();
    db.insert_with_id(1, &[0.25, 0.0], json!({"value":1}))
        .unwrap();
    let untyped = directory.path().join("untyped.tdb");
    let untyped = untyped.to_str().unwrap();
    let wal = std::fs::read(format!("{source}.wal")).unwrap();
    let copied_wal = format!("{untyped}.wal");
    std::fs::write(&copied_wal, &wal).unwrap();
    let output = tdb()
        .args(["--format=json", "repair", "check", untyped])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "TDB_CORRUPTED_FILE");
    assert_eq!(std::fs::read(copied_wal).unwrap(), wal);
    assert!(!std::path::Path::new(untyped).exists());
}

#[test]
fn json模式统一命令解析与输入文件错误() {
    let (directory, path) = seed_db();
    let malformed = directory.path().join("malformed.jsonl");
    std::fs::write(&malformed, "{\n").unwrap();
    for (arguments, code) in [
        (vec!["--format", "json", "info"], "TDB_CLI_ARGUMENT"),
        (
            vec!["--format=json", "info", &path, "--unknown"],
            "TDB_CLI_ARGUMENT",
        ),
        (
            vec![
                "--format",
                "json",
                "import",
                &path,
                "missing-input-file.jsonl",
            ],
            "TDB_IO",
        ),
        (
            vec![
                "--format=json",
                "import",
                &path,
                malformed.to_str().unwrap(),
            ],
            "TDB_CLI_COMMAND",
        ),
    ] {
        let output = tdb().args(arguments).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(if code == "TDB_CLI_ARGUMENT" { 2 } else { 1 })
        );
        let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["schema_version"], 1);
        assert_eq!(error["code"], code);
        assert!(error.get("write").is_none());
        assert!(output.stdout.is_empty());
    }
    let mut reopened = triviumdb::Database::<f32>::open_read_only(&path, 4).unwrap();
    assert_eq!(reopened.all_node_ids().unwrap(), vec![1, 2]);
    for (id, vector, name) in [
        (1, vec![1., 0., 0., 0.], "Alice"),
        (2, vec![0., 1., 0., 0.], "Bob"),
    ] {
        let node = reopened.get(id).unwrap().unwrap();
        assert_eq!(node.vector, vector);
        assert_eq!(node.payload, json!({"name":name,"type":"person"}));
        let expected = if id == 1 {
            json!([{"target_id":2,"label":"knows","weight":1.,"metadata":null}])
        } else {
            json!([])
        };
        assert_eq!(serde_json::to_value(node.edges).unwrap(), expected);
    }
    reopened.close().unwrap();
}

#[test]
fn json_error_retains_core_lock_code_and_owner_data() {
    let (_dir, path) = seed_db();
    let mut owner = triviumdb::Database::<f32>::open(&path, 4).unwrap();
    let output = tdb()
        .args(["--format", "json", "info", &path])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["schema_version"], 1);
    assert_eq!(error["code"], "TDB_DATABASE_LOCKED");
    assert!(error["details"]["reason"].is_string());
    assert!(output.stdout.is_empty());
    owner
        .update_payload(1, json!({"after":"rejected_open"}))
        .unwrap();
    owner.close().unwrap();
    let mut reopened = triviumdb::Database::<f32>::open(&path, 4).unwrap();
    assert_eq!(
        reopened.get_payload(1).unwrap(),
        Some(json!({"after":"rejected_open"}))
    );
    assert_eq!(
        reopened.get(2).unwrap().unwrap().vector,
        vec![0., 1., 0., 0.]
    );
    reopened.close().unwrap();
}

#[test]
fn json配置警告保持结构且继续输出真实元数据() {
    let (directory, path) = seed_db();
    std::fs::write(directory.path().join(".triviumdb.toml"), "[invalid\n").unwrap();
    let output = tdb()
        .env("HOME", directory.path())
        .env("USERPROFILE", directory.path())
        .args(["--format=json", "info", &path])
        .output()
        .unwrap();
    assert!(output.status.success());
    let warning: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(warning["schema_version"], 1);
    assert_eq!(warning["code"], "TDB_CLI_CONFIG");
    assert_eq!(warning["level"], "warning");
    assert_eq!(
        warning["details"]["path"],
        directory.path().join(".triviumdb.toml").to_str().unwrap()
    );
    let info: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(info["path"], path);
    assert_eq!(info["node_count"], 2);
    assert_eq!(info["dimension"], 4);
    assert_eq!(info["dtype"], "f32");
}

#[test]
fn json_tql_parse_error_uses_core_error_envelope() {
    let (_dir, path) = seed_db();
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/contracts/query_diagnostics.json"
    ))
    .unwrap();
    for case in cases["cases"].as_array().unwrap() {
        let mut command = tdb();
        command.args([
            "--format",
            "json",
            "exec",
            &path,
            case["query"].as_str().unwrap(),
        ]);
        if case["mutation"] == true {
            command.arg("--mutate");
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{case}");
        let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["schema_version"], 1);
        assert_eq!(error["code"], "TDB_QUERY_PARSE");
        assert_eq!(error["stage"], case["stage"], "{case}");
        assert_eq!(error["target"]["query_span"], case["span"], "{case}");
        if case["mutation"] == true {
            assert_eq!(
                error["write"],
                serde_json::json!({"phase":"validation","outcome":"not_committed"})
            );
        }
        if case["span"].is_null() {
            assert!(error.get("target").is_none());
        }
        assert!(error["details"]["reason"].is_string());
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn json_transaction_error_preserves_write_outcome_and_database_facts() {
    let (_directory, path) = seed_db();
    let mut database = triviumdb::Database::<f32>::open(&path, 4).unwrap();
    database.create_unique_index("name").unwrap();
    database.close().unwrap();
    let output = tdb()
        .args([
            "--format",
            "json",
            "exec",
            &path,
            "MATCH (n) CREATE (m {name: \"duplicate\"})",
            "--mutate",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "TDB_UNIQUE_CONSTRAINT");
    assert_eq!(error["stage"], "execute");
    assert_eq!(
        error["target"],
        json!({"query_span":{"start_byte":10,"end_byte":40,"line":1,"column":11}})
    );
    assert_eq!(
        error["details"],
        json!({"fields":["name"],"existing_id":3,"conflicting_id":4})
    );
    assert_eq!(
        error["write"],
        json!({"phase":"validation","outcome":"not_committed","transaction_consumed":true})
    );
    let mut reopened = triviumdb::Database::<f32>::open(&path, 4).unwrap();
    assert_eq!(reopened.all_node_ids().unwrap(), vec![1, 2]);
    for (id, vector, name) in [
        (1, vec![1., 0., 0., 0.], "Alice"),
        (2, vec![0., 1., 0., 0.], "Bob"),
    ] {
        let node = reopened.get(id).unwrap().unwrap();
        assert_eq!(node.vector, vector);
        assert_eq!(node.payload, json!({"name":name,"type":"person"}));
    }
    let edges = reopened.get_edges(1).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(
        (edges[0].target_id, edges[0].label.as_str(), edges[0].weight),
        (2, "knows", 1.)
    );
    reopened.close().unwrap();
}

#[test]
fn json_tql_planning_rejection_has_uncommitted_outcome() {
    let (_directory, path) = seed_db();
    let output = tdb()
        .args([
            "--format",
            "json",
            "exec",
            &path,
            "MATCH (n) SET VECTOR(n) == [1]",
            "--mutate",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "TDB_DIMENSION_MISMATCH");
    assert_eq!(error["stage"], "plan");
    assert_eq!(
        error["target"],
        json!({"query_span":{"start_byte":10,"end_byte":30,"line":1,"column":11}})
    );
    assert_eq!(error["details"], json!({"expected":4,"got":1}));
    assert_eq!(
        error["write"],
        json!({"phase":"validation","outcome":"not_committed"})
    );
    let mut database = triviumdb::Database::<f32>::open(&path, 4).unwrap();
    assert_eq!(database.all_node_ids().unwrap(), vec![1, 2]);
    for (id, vector, name) in [
        (1, vec![1., 0., 0., 0.], "Alice"),
        (2, vec![0., 1., 0., 0.], "Bob"),
    ] {
        let node = database.get(id).unwrap().unwrap();
        assert_eq!(node.vector, vector);
        assert_eq!(node.payload, json!({"name":name,"type":"person"}));
    }
    let edges = database.get_edges(1).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(
        (edges[0].target_id, edges[0].label.as_str(), edges[0].weight),
        (2, "knows", 1.)
    );
    assert!(database.get_edges(2).unwrap().is_empty());
    database.close().unwrap();
}

#[test]
fn help_uses_tdb_command_name() {
    tdb()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: tdb"))
        .stdout(predicate::str::contains("triviumdb").not());
}

// ── info ───────────────────────────────────────────────────────

#[test]
fn info_table_shows_metadata() {
    let (_dir, path) = seed_db();
    tdb()
        .args(["info", &path, "--color", "never"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Node Count"))
        .stdout(predicate::str::contains("Dimension"));
}

#[test]
fn info_json_is_valid() {
    let (_dir, path) = seed_db();
    let output = tdb()
        .args(["info", &path, "--format", "json", "--color", "never"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["node_count"], 2);
    assert_eq!(v["dimension"], 4);
    assert_eq!(v["dtype"], "f32");
}

// ── exec (read) ────────────────────────────────────────────────

#[test]
fn exec_match_returns_rows() {
    let (_dir, path) = seed_db();
    tdb()
        .args(["exec", &path, "MATCH (n) RETURN n", "--color", "never"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Alice"))
        .stdout(predicate::str::contains("Bob"));
}

#[test]
fn exec_json_format_valid() {
    let (_dir, path) = seed_db();
    let output = tdb()
        .args([
            "exec",
            &path,
            "MATCH (n) RETURN n",
            "--format",
            "json",
            "--color",
            "never",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(v.is_array());
    assert_eq!(v.as_array().unwrap().len(), 2);
}

#[test]
fn exec_csv_format_has_header() {
    let (_dir, path) = seed_db();
    tdb()
        .args([
            "exec",
            &path,
            "MATCH (n) RETURN n",
            "--format",
            "csv",
            "--color",
            "never",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("#,"));
}

#[test]
fn exec_json只输出请求的投影列和聚合值() {
    let (_dir, path) = seed_db();
    let mut db = triviumdb::Database::<f32>::open(&path, 4).unwrap();
    db.update_payload(
        1,
        json!({"wide":9_007_199_254_740_993u64,"private":"hidden","nested":{"keep":true}}),
    )
    .unwrap();
    db.close().unwrap();

    for (query, expected) in [
        (
            "MATCH (n {id:1}) RETURN n.wide AS v, n.nested AS fields, n.missing AS empty",
            json!([{"v":9_007_199_254_740_993u64,"fields":{"keep":true},"empty":null}]),
        ),
        ("MATCH (n) RETURN COUNT(*) AS total", json!([{"total":2}])),
    ] {
        let output = tdb()
            .args(["exec", &path, query, "--format=json"])
            .assert()
            .success();
        let actual: serde_json::Value =
            serde_json::from_slice(&output.get_output().stdout).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn exec_csv完整保留长字段和嵌入的分隔字符() {
    let (_dir, path) = seed_db();
    let mut db = triviumdb::Database::<f32>::open(&path, 4).unwrap();
    let long = "长".repeat(150);
    db.update_payload(1, json!({"line":format!("{long},\"a\"\r\nb")}))
        .unwrap();
    db.close().unwrap();
    let output = tdb()
        .args([
            "exec",
            &path,
            "MATCH (n {id:1}) RETURN n.line AS line",
            "--format=csv",
        ])
        .assert()
        .success();
    let actual = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let expected = format!("#,line\n1,\"{long},\"\"a\"\"\r\nb\"\n");
    assert_eq!(actual, expected);
}

#[test]
fn exec_json写入确认包含实际提交的节点() {
    let (_dir, path) = seed_db();
    let output = tdb()
        .args([
            "exec",
            &path,
            r#"CREATE (a {name:"One"}), (b {name:"Two"})"#,
            "--mutate",
            "--format=json",
        ])
        .assert()
        .success();
    let actual: serde_json::Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    assert_eq!(actual, json!({"affected":2,"created_ids":[3,4]}));
    let mut reopened = triviumdb::Database::<f32>::open_read_only(&path, 4).unwrap();
    assert_eq!(
        reopened.get_payload(3).unwrap(),
        Some(json!({"name":"One"}))
    );
    assert_eq!(
        reopened.get_payload(4).unwrap(),
        Some(json!({"name":"Two"}))
    );
    reopened.close().unwrap();
}

// ── exec (mutate) ──────────────────────────────────────────────

#[test]
fn exec_mutate_create_node() {
    let (_dir, path) = seed_db();
    tdb()
        .args([
            "exec",
            &path,
            r#"CREATE (a {name: "Charlie"})"#,
            "--mutate",
            "--color",
            "never",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("OK"));

    // 验证新节点已写入
    let output = tdb()
        .args(["info", &path, "--format", "json", "--color", "never"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["node_count"], 3);
}

// ── export / import 往返 ───────────────────────────────────────

#[test]
fn export_import_roundtrip() {
    let (dir, src_path) = seed_db();
    let jsonl_path = dir.path().join("dump.jsonl");
    let dst_path = dir.path().join("clone.tdb");

    // export
    tdb()
        .args([
            "export",
            &src_path,
            jsonl_path.to_str().unwrap(),
            "--color",
            "never",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("2"));

    // 验证 JSONL 文件行数
    let content = std::fs::read_to_string(&jsonl_path).unwrap();
    assert_eq!(content.lines().count(), 2);

    // import into new db
    tdb()
        .args([
            "import",
            dst_path.to_str().unwrap(),
            jsonl_path.to_str().unwrap(),
            "--dim",
            "4",
            "--color",
            "never",
        ])
        .assert()
        .success();

    // 验证目标数据库
    let output = tdb()
        .args([
            "info",
            dst_path.to_str().unwrap(),
            "--format",
            "json",
            "--color",
            "never",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["node_count"], 2);
}

// ── compact ────────────────────────────────────────────────────

#[test]
fn compact_succeeds() {
    let (_dir, path) = seed_db();
    tdb()
        .args(["compact", &path, "--color", "never"])
        .assert()
        .success()
        .stdout(predicate::str::contains("压实完成"));
}

// ── repair ─────────────────────────────────────────────────────

#[test]
fn repair_check_valid_db() {
    let (_dir, path) = seed_db();
    tdb()
        .args(["repair", "check", &path, "--color", "never"])
        .assert()
        .success()
        .stdout(predicate::str::contains("完好"));
}

#[test]
fn repair_check_nonexistent() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nope.tdb");
    tdb()
        .args([
            "repair",
            "check",
            path.to_str().unwrap(),
            "--color",
            "never",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("不存在"));
}

#[test]
fn repair_dump_outputs_nodes() {
    let (_dir, path) = seed_db();
    let result = tdb()
        .args([
            "repair", "dump", &path, "--format", "json", "--color", "never",
        ])
        .assert()
        .success();
    let rows: serde_json::Value = serde_json::from_slice(&result.get_output().stdout).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2);
    assert_eq!(rows[0]["node"]["payload"]["name"], "Alice");
    assert_eq!(rows[1]["node"]["payload"]["name"], "Bob");
}

#[test]
fn 修复转储拒绝缺失路径且保持目录为空() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nope.tdb");
    let output = tdb()
        .args(["repair", "dump", path.to_str().unwrap(), "--format=json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "TDB_IO");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn 只读命令与已有读者并行且报告持久化_quiver_组件() {
    let (dir, path) = seed_db();
    let mut writer = triviumdb::Database::<f32>::open(&path, 4).unwrap();
    writer.build_quiver_index(None).unwrap();
    writer.close().unwrap();
    let database_files = || {
        std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), std::fs::read(entry.path()).unwrap())
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let before = database_files();
    let mut reader = triviumdb::Database::<f32>::open_read_only(&path, 4).unwrap();
    let info = tdb()
        .args(["info", &path, "--format=json"])
        .assert()
        .success();
    let info: serde_json::Value = serde_json::from_slice(&info.get_output().stdout).unwrap();
    assert_eq!(info["files"]["quiver_present"], true);
    assert_eq!(info["node_count"], 2);
    let output = tdb()
        .args([
            "exec",
            &path,
            "MATCH (n) RETURN n.id AS id ORDER BY id",
            "--format=json",
        ])
        .assert()
        .success();
    let rows: serde_json::Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    assert_eq!(rows, json!([{"id":1},{"id":2}]));
    let external = TempDir::new().unwrap();
    let output = external.path().join("data.jsonl");
    tdb()
        .args(["export", &path, output.to_str().unwrap()])
        .assert()
        .success();
    let contents = std::fs::read_to_string(output).unwrap();
    let rows = contents
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["payload"]["name"], "Alice");
    assert_eq!(rows[1]["payload"]["name"], "Bob");
    reader.close().unwrap();
    assert_eq!(database_files(), before);
}

#[test]
fn 零行_csv_保留投影定义的完整表头() {
    let (_dir, path) = seed_db();
    for (query, header) in [
        (
            "MATCH (n {id:0}) RETURN n.id AS id, n.name",
            "#,id,n.name\n",
        ),
        ("MATCH (n {id:0}) WITH n AS renamed RETURN *", "#,renamed\n"),
    ] {
        let output = tdb()
            .args(["exec", &path, query, "--format=csv"])
            .assert()
            .success();
        assert_eq!(output.get_output().stdout, header.as_bytes());
    }
}

// ── error cases ────────────────────────────────────────────────

#[test]
fn info_nonexistent_db_fails() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nope.tdb");
    tdb()
        .args(["info", path.to_str().unwrap(), "--color", "never"])
        .assert()
        .failure();
}

#[test]
fn exec_invalid_tql_fails() {
    let (_dir, path) = seed_db();
    tdb()
        .args(["exec", &path, "INVALID QUERY", "--color", "never"])
        .assert()
        .failure();
}

// ── export 格式验证 ────────────────────────────────────────────

#[test]
fn export_jsonl_format_correct() {
    let (dir, path) = seed_db();
    let jsonl = dir.path().join("out.jsonl");

    tdb()
        .args(["export", &path, jsonl.to_str().unwrap(), "--color", "never"])
        .assert()
        .success();

    // 验证每行都是合法 JSON
    let content = std::fs::read_to_string(&jsonl).unwrap();
    for line in content.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(v["id"].is_u64());
        assert!(v["vector"].is_array());
        assert!(v["payload"].is_object());
    }
}

#[test]
fn export_refuses_database_path() {
    let (_dir, path) = seed_db();

    tdb()
        .args(["export", &path, &path, "--color", "never"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("拒绝导出到数据库相关文件"));
}

#[test]
fn export_refuses_database_sidecar_path() {
    let (_dir, path) = seed_db();
    let sidecar = format!("{path}.vec");

    tdb()
        .args(["export", &path, &sidecar, "--color", "never"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("拒绝导出到数据库相关文件"));
}

// ── import 从手写 JSONL ────────────────────────────────────────

#[test]
fn import_from_handwritten_jsonl() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("new.tdb");
    let jsonl = dir.path().join("data.jsonl");

    // 手写 JSONL
    let mut f = std::fs::File::create(&jsonl).unwrap();
    writeln!(
        f,
        r#"{{"id":100,"vector":[1,0,0,0],"payload":{{"k":"v"}}}}"#
    )
    .unwrap();
    writeln!(f, r#"{{"id":200,"vector":[0,1,0,0],"payload":{{"k":"w"}},"edges":[{{"target":100,"label":"ref","weight":0.5}}]}}"#).unwrap();
    drop(f);

    tdb()
        .args([
            "import",
            db_path.to_str().unwrap(),
            jsonl.to_str().unwrap(),
            "--dim",
            "4",
            "--color",
            "never",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("2"));

    // 验证
    let output = tdb()
        .args([
            "info",
            db_path.to_str().unwrap(),
            "--format",
            "json",
            "--color",
            "never",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["node_count"], 2);
}

#[test]
fn import_rejects_missing_vector() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("bad.tdb");
    let jsonl = dir.path().join("bad.jsonl");

    let mut f = std::fs::File::create(&jsonl).unwrap();
    writeln!(f, r#"{{"id":1,"payload":{{"k":"v"}}}}"#).unwrap();
    drop(f);

    tdb()
        .args([
            "import",
            db_path.to_str().unwrap(),
            jsonl.to_str().unwrap(),
            "--dim",
            "4",
            "--color",
            "never",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("缺少 vector 字段"));
}
