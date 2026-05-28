//! TriviumDB 统一命令行工具入口。
//!
//! 三种模式：
//! - 非交互子命令（info / exec / export / import / repair / compact）
//! - REPL（`open`）
//! - TUI 可视化面板（`ui`）

mod commands;
mod db_handle;
mod formatter;
mod repl;
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
    name = "triviumdb",
    version,
    about = "TriviumDB CLI & TUI — 交互式 REPL、非交互命令与终端可视化面板",
    long_about = None,
    propagate_version = true
)]
struct Cli {
    /// 输出格式
    #[arg(long, global = true, default_value = "table")]
    format: OutputFormat,

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

    /// 数据类型
    #[arg(long, default_value = "f32")]
    dtype: String,
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
        #[arg(long, default_value = "f32")]
        dtype: String,
    },
}

fn main() {
    let cli = Cli::parse();
    apply_color(cli.color);

    let result = run(cli);
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

fn run(cli: Cli) -> CliResult {
    let format = cli.format;
    match cli.command {
        Commands::Open(db) => {
            let handle = open(&db)?;
            repl::run(handle, &db.path, format)
        }
        Commands::Ui(db) => {
            let handle = open(&db)?;
            tui::run(handle, &db.path)
        }
        Commands::Info(db) => commands::info::run(&db.path, db.dim, parse_dtype(&db.dtype)?, format),
        Commands::Exec {
            db,
            query,
            mutate,
        } => {
            let mut handle = open(&db)?;
            commands::exec::run(&mut handle, &query, mutate, format)
        }
        Commands::Repair { action } => match action {
            RepairAction::Check { path } => commands::repair::check(&path),
            RepairAction::Dump { path, dtype } => {
                commands::repair::dump(&path, parse_dtype(&dtype)?, format)
            }
        },
        Commands::Export { db, output } => {
            let handle = open(&db)?;
            commands::export::run(&handle, &output)
        }
        Commands::Import { db, input } => {
            let mut handle = open(&db)?;
            commands::import::run(&mut handle, &input)
        }
        Commands::Compact(db) => {
            let mut handle = open(&db)?;
            commands::compact::run(&mut handle)
        }
    }
}

fn parse_dtype(s: &str) -> Result<DType, Box<dyn std::error::Error>> {
    Ok(DType::parse(s)?)
}

/// 按 [`DbArgs`] 打开数据库（自动嗅探维度）。
fn open(db: &DbArgs) -> Result<db_handle::DbHandle, Box<dyn std::error::Error>> {
    let dtype = parse_dtype(&db.dtype)?;
    Ok(db_handle::DbHandle::open_auto(&db.path, db.dim, dtype)?)
}
