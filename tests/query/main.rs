#![allow(non_snake_case)]
//! 按领域聚合的集成测试入口。

mod aggregation_tx;
mod proptest_query;
mod query_memory_budget_dimensions;
mod tql_dml;
mod tql_executor;
mod tql_index;
mod tql_parser;
mod tql_phase2;
mod tql_pipeline_parser;
mod tql_planner;
mod tql_reverse;

mod match_boundaries;
mod parser_limits;

#[path = "../support/mod.rs"]
mod support;

// Cargo 的仿真启动参数不会自动传给测试内部创建的子进程。
fn query_child_command() -> std::process::Command {
    let executable = std::env::current_exe().unwrap();
    if std::env::var_os("TRIVIUM_TEST_QEMU_AARCH64").is_some() {
        let mut command = std::process::Command::new("qemu-aarch64");
        command
            .arg("-L")
            .arg("/usr/aarch64-linux-gnu")
            .arg(executable);
        command
    } else {
        std::process::Command::new(executable)
    }
}
