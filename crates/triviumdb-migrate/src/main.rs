use std::path::PathBuf;
use triviumdb_migrate::{Failure, Options, PROTOCOL, TOOL_ID};

fn parse() -> std::result::Result<Option<Options>, String> {
    let mut args = std::env::args().skip(1);
    let mut source = None;
    let mut source_dtype = None;
    let mut destination = None;
    let mut check = true;
    let mut memory = 256usize;
    let mut max_source = 1024u64;
    let mut max_work = 8192u64;
    let mut mmap = true;
    let mut seen = std::collections::BTreeSet::new();
    while let Some(arg) = args.next() {
        if !seen.insert(arg.clone()) {
            return Err(format!("参数重复 (Duplicate option): {arg}"));
        }
        if arg == "--protocol" {
            if seen.len() != 1 || args.next().is_some() {
                return Err("--protocol 必须独立使用 (Use --protocol alone)".into());
            }
            return Ok(None);
        }
        match arg.as_str() {
            "--execute" => check = false,
            "--check" => check = true,
            "--source-dtype" => {
                source_dtype = Some(match args.next().as_deref() {
                    Some("f32") => 1,
                    Some("f16") => 2,
                    Some("u64") => 3,
                    _ => {
                        return Err(
                            "来源类型须为 f32/f16/u64 (Source dtype must be f32/f16/u64)".into(),
                        );
                    }
                });
            }
            "--source" => {
                source = Some(PathBuf::from(
                    args.next()
                        .ok_or("--source 缺少路径 (Missing source path)")?,
                ))
            }
            "--destination" => {
                destination = Some(PathBuf::from(
                    args.next()
                        .ok_or("--destination 缺少目录 (Missing destination directory)")?,
                ))
            }
            "--memory-mib" => {
                memory = args
                    .next()
                    .ok_or("缺少内存预算 (Missing memory budget)")?
                    .parse()
                    .map_err(|_| "内存预算无效 (Invalid memory budget)")?
            }
            "--max-source-mib" => {
                max_source = args
                    .next()
                    .ok_or("缺少来源预算 (Missing source budget)")?
                    .parse()
                    .map_err(|_| "来源预算无效 (Invalid source budget)")?
            }
            "--max-work-mib" => {
                max_work = args
                    .next()
                    .ok_or("缺少工作区预算 (Missing workspace budget)")?
                    .parse()
                    .map_err(|_| "工作区预算无效 (Invalid workspace budget)")?
            }
            "--storage" => {
                mmap = match args.next().as_deref() {
                    Some("mmap") => true,
                    Some("rom") => false,
                    _ => {
                        return Err("存储模式须为 rom 或 mmap (Storage must be rom or mmap)".into());
                    }
                }
            }
            _ => return Err(format!("未知参数 (Unknown option): {arg}")),
        }
    }
    if seen.contains("--check") && seen.contains("--execute") {
        return Err("--check 与 --execute 互斥 (Check and execute are mutually exclusive)".into());
    }
    Ok(Some(Options {
        source_dtype,
        source: source.ok_or("缺少 --source (Missing --source)")?,
        destination: destination.ok_or("缺少 --destination (Missing --destination)")?,
        check,
        memory_bytes: memory
            .checked_mul(1024 * 1024)
            .ok_or("内存预算溢出 (Memory budget overflow)")?,
        max_source_bytes: max_source
            .checked_mul(1024 * 1024)
            .ok_or("来源预算溢出 (Source budget overflow)")?,
        max_work_bytes: max_work
            .checked_mul(1024 * 1024)
            .ok_or("工作区预算溢出 (Workspace budget overflow)")?,
        mmap,
    }))
}

fn main() {
    let parsed = parse();
    if matches!(parsed, Ok(None)) {
        println!(
            "{}",
            serde_json::json!({"protocol": PROTOCOL, "tool": TOOL_ID, "target_format": triviumdb::storage::file_format::CURRENT_VERSION, "source_formats": [5,6,7,8,9,10,11,12,13,14,15]})
        );
        return;
    }
    let result = match parsed {
        Ok(Some(options)) => match rayon::ThreadPoolBuilder::new().num_threads(2).build() {
            Ok(pool) => pool.install(|| triviumdb_migrate::run(&options)),
            Err(error) => Err(Failure {
                code: "TDB_CAPACITY_ALLOCATION_FAILED".into(),
                message: format!(
                    "迁移工作线程创建失败 (Migration worker creation failed): {error}"
                ),
                phase: "admission",
                outcome: "not_committed",
                details: serde_json::json!({"reason":error.to_string()}),
            }),
        },
        Err(message) => Err(Failure {
            code: "TDB_INVALID_INPUT".into(),
            details: serde_json::json!({"reason":message}),
            message,
            phase: "admission",
            outcome: "not_committed",
        }),
        Ok(None) => return,
    };
    match result {
        Ok(report) => match serde_json::to_string(&report) {
            Ok(report) => println!("{report}"),
            Err(error) => {
                eprintln!("迁移报告编码失败 (Migration report encoding failed): {error}");
                std::process::exit(2);
            }
        },
        Err(error) => {
            eprintln!("{}", serde_json::json!({"error": error}));
            std::process::exit(1);
        }
    }
}
