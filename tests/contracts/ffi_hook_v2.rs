#![allow(non_snake_case)]

use std::path::{Path, PathBuf};
use std::process::Command;
use triviumdb::database::SearchConfig;
use triviumdb::hook::{FfiHook, HookContext, SearchHook};
use triviumdb::node::SearchHit;

fn fixture_source(abi_version: u32) -> String {
    format!(
        r#"
#[repr(C)]
pub struct FfiSearchHit {{ pub id: u64, pub score: f32 }}

#[unsafe(no_mangle)]
pub extern "C" fn trivium_hook_abi_version() -> u32 {{ {abi_version} }}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn trivium_hook_invoke_v2(
    stage: u32,
    query_ptr: *const f32,
    query_len: usize,
    hits_ptr: *mut FfiSearchHit,
    hits_capacity: usize,
    hits_count: *mut usize,
) -> i32 {{
    let query = if query_len == 0 {{ &[][..] }} else {{ unsafe {{ std::slice::from_raw_parts(query_ptr, query_len) }} }};
    if query.first() == Some(&-1.0) {{ return 77; }}
    if query.first() == Some(&-2.0) {{ unsafe {{ *hits_count = hits_capacity + 1; }} return 0; }}
    if hits_capacity == 0 {{ return 78; }}
    let hits = unsafe {{ std::slice::from_raw_parts_mut(hits_ptr, hits_capacity) }};
    if stage == 2 {{
        hits[0] = FfiSearchHit {{ id: 42, score: 0.9 }};
        unsafe {{ *hits_count = 1; }}
    }} else if unsafe {{ *hits_count }} > 0 {{
        hits[0].score = stage as f32;
    }}
    0
}}
"#
    )
}

fn build_fixture(name: &str, abi_version: u32) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "triviumdb_ffi_hook_v2_{}_{}",
        name,
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("fixture.rs");
    std::fs::write(&source, fixture_source(abi_version)).unwrap();
    let library = root.join(format!("{name}.{}", std::env::consts::DLL_EXTENSION));
    let status = Command::new("rustc")
        .arg("--edition=2024")
        .arg("--crate-type=cdylib")
        .arg(&source)
        .arg("-o")
        .arg(&library)
        .status()
        .unwrap();
    assert!(status.success(), "FFI ABI v2 测试动态库编译失败");
    library
}

fn hit(score: f32) -> SearchHit {
    SearchHit {
        id: 1,
        score,
        payload: serde_json::json!({"kept": true}),
    }
}

fn qemu_aarch64环境不支持运行时宿主动态库夹具() -> bool {
    std::env::var_os("TRIVIUM_TEST_QEMU_AARCH64").is_some()
}

#[derive(serde::Deserialize)]
struct SharedFfiContract {
    schema_version: u32,
    ffi_cases: Vec<SharedFfiCase>,
}

#[derive(serde::Deserialize)]
struct SharedFfiCase {
    name: String,
    operation: String,
    expected: serde_json::Value,
}

fn shared_ffi_contract() -> SharedFfiCase {
    let contract: SharedFfiContract =
        serde_json::from_str(include_str!("public_cases.json")).unwrap();
    assert_eq!(contract.schema_version, 1);
    let case = contract.ffi_cases.into_iter().next().unwrap();
    assert_eq!(case.name, "hook_abi_v2");
    assert_eq!(case.operation, "ffi_hook_abi");
    case
}

#[test]
fn FFI_ABI_v2覆盖六阶段并传播错误() {
    let contract = shared_ffi_contract();
    assert_eq!(contract.expected["abi_version"].as_u64(), Some(2));
    assert_eq!(contract.expected["stages"].as_array().unwrap().len(), 6);
    if qemu_aarch64环境不支持运行时宿主动态库夹具() {
        return;
    }
    let library = build_fixture("valid", 2);
    let hook = FfiHook::load(library.to_str().unwrap()).unwrap();
    let mut ctx = HookContext::new();
    let mut config = SearchConfig {
        top_k: 3,
        ..Default::default()
    };
    let mut query = vec![1.0, 0.0];

    hook.on_pre_search(&mut query, &mut config, &mut ctx);
    assert!(ctx.error.is_none());

    let recalled = hook
        .on_custom_recall(&query, &config, &mut ctx)
        .expect("ABI v2 自定义召回必须返回结果");
    assert_eq!(recalled.len(), 1);
    assert_eq!(recalled[0].id, 42);

    let mut hits = vec![hit(0.1)];
    hook.on_post_recall(&mut hits, &mut ctx);
    assert_eq!(hits[0].score, 3.0);
    hook.on_pre_graph_expand(&mut hits, &mut ctx);
    assert_eq!(hits[0].score, 4.0);
    hits = hook.on_rerank(&mut hits, &mut ctx).unwrap();
    assert_eq!(hits[0].score, 5.0);
    hook.on_post_search(&mut hits, &mut ctx);
    assert_eq!(hits[0].score, 6.0);

    let mut failing_query = vec![-1.0];
    hook.on_pre_search(&mut failing_query, &mut config, &mut ctx);
    let expected_status = contract.expected["error_status"].as_i64().unwrap();
    assert!(
        ctx.error
            .as_deref()
            .is_some_and(|error| error.contains(&expected_status.to_string()))
    );
}

#[test]
fn FFI_ABI_v2拒绝版本不匹配() {
    if qemu_aarch64环境不支持运行时宿主动态库夹具() {
        return;
    }
    let library = build_fixture("mismatch", 1);
    let error = match FfiHook::load(library.to_str().unwrap()) {
        Ok(_) => panic!("ABI 版本不匹配必须拒绝加载"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("版本不匹配"));
}

#[test]
fn FFI_ABI_v2拒绝越界返回数量() {
    if qemu_aarch64环境不支持运行时宿主动态库夹具() {
        return;
    }
    let library = build_fixture("overflow", 2);
    let hook = FfiHook::load(library.to_str().unwrap()).unwrap();
    let mut ctx = HookContext::new();
    let mut config = SearchConfig::default();
    let mut query = vec![-2.0];
    hook.on_pre_search(&mut query, &mut config, &mut ctx);
    assert!(
        ctx.error
            .as_deref()
            .is_some_and(|error| error.contains("超过容量"))
    );
}

fn _assert_fixture_is_file(path: &Path) {
    assert!(path.is_file());
}
