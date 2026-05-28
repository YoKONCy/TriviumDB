//! TriviumDB 统一命令行工具入口。
//!
//! 三种模式：
//! - 非交互子命令（info / exec / export / import / repair / compact）
//! - REPL（`open`）
//! - TUI 可视化面板（`ui`）

mod commands;
mod config;
mod db_handle;
mod diagnostics;
mod formatter;
mod repl;
mod tql_highlight;
mod tui;
mod util;

use clap::{Args, Parser, Subcommand, ValueEnum};
use colored::Colorize;

use db_handle::DType;
use formatter::OutputFormat;

/// CLI 统一返回类型：错误装箱后由 [`main`] 统一打印。
pub type CliResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(
    name = "tdb",
    version,
    about = "TriviumDB CLI & TUI — 交互式 REPL、非交互命令与终端可视化面板",
    long_about = None,
    propagate_version = true
)]
struct Cli {
    /// 输出格式（缺省时取配置文件 defaults.format，再缺省为 table）
    #[arg(long, global = true)]
    format: Option<OutputFormat>,

    /// 彩色输出控制
    #[arg(long, global = true, value_enum, default_value_t = ColorWhen::Auto)]
    color: ColorWhen,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ColorWhen {
    Auto,
    Always,
    Never,
}

/// 打开数据库通用参数（被多个子命令复用）。
#[derive(Args, Clone)]
struct DbArgs {
    /// 数据库路径 (.tdb)
    path: String,

    /// 向量维度（默认从 .tdb 文件头自动嗅探）
    #[arg(long)]
    dim: Option<usize>,

    /// 数据类型（缺省时取配置文件 defaults.dtype，再缺省为 f32）
    #[arg(long)]
    dtype: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// 打开数据库进入交互式 REPL
    Open(DbArgs),

    /// 打开数据库进入 TUI 可视化面板
    Ui(DbArgs),

    /// 显示数据库元信息
    Info(DbArgs),

    /// 非交互执行单条 TQL 语句
    Exec {
        #[command(flatten)]
        db: DbArgs,
        /// 要执行的 TQL 语句
        query: String,
        /// 以写模式执行（CREATE / SET / DELETE）
        #[arg(long)]
        mutate: bool,
    },

    /// 数据库诊断与修复
    Repair {
        #[command(subcommand)]
        action: RepairAction,
    },

    /// 导出全部节点为 JSONL
    Export {
        #[command(flatten)]
        db: DbArgs,
        /// 输出文件路径（.jsonl）
        output: String,
    },

    /// 从 JSONL 批量导入节点
    Import {
        #[command(flatten)]
        db: DbArgs,
        /// 输入文件路径（.jsonl）
        input: String,
    },

    /// 手动触发压缩（compaction）
    Compact(DbArgs),
}

#[derive(Subcommand)]
enum RepairAction {
    /// 快速检查文件头与维度指纹
    Check {
        /// 数据库路径 (.tdb)
        path: String,
    },
    /// 强制挂载并导出全部节点
    Dump {
        /// 数据库路径 (.tdb)
        path: String,
        /// 数据类型
        #[arg(long)]
        dtype: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    apply_color(cli.color);
    let cfg = config::Config::load();

    let result = run(cli, cfg);
    if let Err(e) = result {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    }
}

fn apply_color(when: ColorWhen) {
    match when {
        ColorWhen::Always => colored::control::set_override(true),
        ColorWhen::Never => colored::control::set_override(false),
        ColorWhen::Auto => {}
    }
}

fn run(cli: Cli, cfg: config::Config) -> CliResult {
    let format = resolve_format(cli.format, &cfg);
    match cli.command {
        Commands::Open(db) => {
            let handle = open(&db, &cfg)?;
            repl::run(handle, &db.path, format)
        }
        Commands::Ui(db) => {
            let handle = open(&db, &cfg)?;
            let limit = cfg.tui.default_limit.unwrap_or(50);
            let marker = resolve_graph_marker(cfg.tui.graph_marker.as_deref());
            tui::run(handle, &db.path, limit, marker)
        }
        Commands::Info(db) => {
            commands::info::run(&db.path, db.dim, resolve_dtype(&db.dtype, &cfg)?, format)
        }
        Commands::Exec {
            db,
            query,
            mutate,
        } => {
            let mut handle = open(&db, &cfg)?;
            commands::exec::run(&mut handle, &query, mutate, format)
        }
        Commands::Repair { action } => match action {
            RepairAction::Check { path } => commands::repair::check(&path),
            RepairAction::Dump { path, dtype } => {
                commands::repair::dump(&path, resolve_dtype(&dtype, &cfg)?, format)
            }
        },
        Commands::Export { db, output } => {
            let handle = open(&db, &cfg)?;
            commands::export::run(&handle, &db.path, &output)
        }
        Commands::Import { db, input } => {
            let mut handle = open(&db, &cfg)?;
            commands::import::run(&mut handle, &input)
        }
        Commands::Compact(db) => {
            let mut handle = open(&db, &cfg)?;
            commands::compact::run(&mut handle)
        }
    }
}

/// 解析输出格式：CLI > 配置 > 默认(table)。
fn resolve_format(cli: Option<OutputFormat>, cfg: &config::Config) -> OutputFormat {
    if let Some(f) = cli {
        return f;
    }
    if let Some(s) = cfg.defaults.format.as_deref()
        && let Ok(f) = OutputFormat::parse(s)
    {
        return f;
    }
    OutputFormat::Table
}

/// 解析 dtype：CLI > 配置 > 默认(f32)。
fn resolve_dtype(cli: &Option<String>, cfg: &config::Config) -> Result<DType, Box<dyn std::error::Error>> {
    let s = cli
        .clone()
        .or_else(|| cfg.defaults.dtype.clone())
        .unwrap_or_else(|| "f32".to_string());
    Ok(DType::parse(&s)?)
}

/// 按 [`DbArgs`] 打开数据库（自动嗅探维度，dtype 走配置优先级）。
fn open(db: &DbArgs, cfg: &config::Config) -> Result<db_handle::DbHandle, Box<dyn std::error::Error>> {
    let dtype = resolve_dtype(&db.dtype, cfg)?;
    Ok(db_handle::DbHandle::open_auto(&db.path, db.dim, dtype)?)
}

/// 解析图字符渲染模式：配置缺失或不识别值都回退到 Auto。
fn resolve_graph_marker(s: Option<&str>) -> tui::GraphMarker {
    match s.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("braille") => tui::GraphMarker::Braille,
        Some("dot") => tui::GraphMarker::Dot,
        Some("block") => tui::GraphMarker::Block,
        Some("half_block") | Some("halfblock") => tui::GraphMarker::HalfBlock,
        _ => tui::GraphMarker::Auto,
    }
}
