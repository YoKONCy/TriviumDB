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
            "exec", &path, "MATCH (n) RETURN n", "--format", "json", "--color", "never",
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
            "exec", &path, "MATCH (n) RETURN n", "--format", "csv", "--color", "never",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("#,"));
}

// ── exec (mutate) ──────────────────────────────────────────────

#[test]
fn exec_mutate_create_node() {
    let (_dir, path) = seed_db();
    tdb()
        .args([
            "exec", &path, r#"CREATE (a {name: "Charlie"})"#, "--mutate", "--color", "never",
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
        .args(["export", &src_path, jsonl_path.to_str().unwrap(), "--color", "never"])
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
        .args(["repair", "check", path.to_str().unwrap(), "--color", "never"])
        .assert()
        .success()
        .stdout(predicate::str::contains("不存在"));
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
    writeln!(f, r#"{{"id":100,"vector":[1,0,0,0],"payload":{{"k":"v"}}}}"#).unwrap();
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
