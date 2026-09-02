use clap::{Parser, ValueEnum};
use std::{net::SocketAddr, path::PathBuf, time::Duration};
use tracing_subscriber::EnvFilter;
use triviumdb::database::{Config, StorageMode};
use triviumdb_server::{ServerConfig, build_app};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogFormat {
    Pretty,
    Json,
}

#[derive(Debug, Parser)]
#[command(name = "triviumdb-server", version, about = "TriviumDB HTTP Server")]
struct Arguments {
    /// 日志格式
    #[arg(
        long,
        env = "TRIVIUMDB_LOG_FORMAT",
        value_enum,
        default_value = "pretty"
    )]
    log_format: LogFormat,
    /// 数据库文件路径
    #[arg(
        long,
        env = "TRIVIUMDB_DATABASE",
        default_value = "triviumdb-server.tdb"
    )]
    database: PathBuf,
    /// 监听地址
    #[arg(long, env = "TRIVIUMDB_LISTEN", default_value = "127.0.0.1:8080")]
    listen: SocketAddr,
    /// 向量维度
    #[arg(long, env = "TRIVIUMDB_DIM", default_value_t = 1536)]
    dim: usize,
    /// 最大查询结果行数，0 表示不设置默认行上限
    #[arg(long, env = "TRIVIUMDB_MAX_QUERY_ROWS", default_value_t = 10_000)]
    max_query_rows: usize,
    /// TriviumDB 内核内存上限，0 表示不限制
    #[arg(long, env = "TRIVIUMDB_MEMORY_LIMIT", default_value_t = 0)]
    memory_limit: usize,
    /// 写队列容量
    #[arg(long, env = "TRIVIUMDB_WRITE_QUEUE_CAPACITY", default_value_t = 256)]
    write_queue_capacity: usize,
    /// 最大并发读请求数
    #[arg(long, env = "TRIVIUMDB_MAX_CONCURRENT_READS", default_value_t = 8)]
    max_concurrent_reads: usize,
    /// 进程内幂等结果缓存容量，0 表示关闭
    #[arg(long, env = "TRIVIUMDB_IDEMPOTENCY_CAPACITY", default_value_t = 4096)]
    idempotency_capacity: usize,
    /// Group Commit 最大请求数
    #[arg(long, env = "TRIVIUMDB_MAX_WRITE_BATCH_SIZE", default_value_t = 64)]
    max_write_batch_size: usize,
    /// 动态 batching 最大等待微秒数
    #[arg(
        long,
        env = "TRIVIUMDB_MAX_WRITE_BATCH_DELAY_US",
        default_value_t = 500
    )]
    max_write_batch_delay_us: u64,
    /// Prepared 查询缓存容量
    #[arg(
        long,
        env = "TRIVIUMDB_PREPARED_CACHE_CAPACITY",
        default_value_t = 1024
    )]
    prepared_cache_capacity: usize,
    /// 请求执行超时（毫秒）
    #[arg(long, env = "TRIVIUMDB_REQUEST_TIMEOUT_MS", default_value_t = 30_000)]
    request_timeout_ms: u64,
    /// 最大 HTTP 请求体字节数
    #[arg(long, env = "TRIVIUMDB_MAX_BODY_BYTES", default_value_t = 4 * 1024 * 1024)]
    max_body_bytes: usize,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("服务启动失败 (Server startup failed): {error:?}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::parse();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    match arguments.log_format {
        LogFormat::Pretty => tracing_subscriber::fmt().with_env_filter(filter).init(),
        LogFormat::Json => tracing_subscriber::fmt()
            .json()
            .with_current_span(false)
            .with_span_list(false)
            .with_env_filter(filter)
            .init(),
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        listen = %arguments.listen,
        database_path_configured = true,
        dim = arguments.dim,
        max_query_rows = arguments.max_query_rows,
        memory_limit_bytes = arguments.memory_limit,
        write_queue_capacity = arguments.write_queue_capacity,
        max_concurrent_reads = arguments.max_concurrent_reads,
        idempotency_capacity = arguments.idempotency_capacity,
        max_write_batch_size = arguments.max_write_batch_size,
        max_write_batch_delay_us = arguments.max_write_batch_delay_us,
        prepared_cache_capacity = arguments.prepared_cache_capacity,
        request_timeout_ms = arguments.request_timeout_ms,
        max_body_bytes = arguments.max_body_bytes,
        log_format = ?arguments.log_format,
        "TriviumDB Server 生效配置 (Effective configuration)"
    );
    let app = build_app(ServerConfig {
        database_path: arguments.database,
        database: Config {
            dim: arguments.dim,
            storage_mode: StorageMode::Mmap,
            memory_limit: arguments.memory_limit,
            max_query_rows: Some(arguments.max_query_rows),
            ..Config::default()
        },
        write_queue_capacity: arguments.write_queue_capacity,
        max_concurrent_reads: arguments.max_concurrent_reads,
        idempotency_capacity: arguments.idempotency_capacity,
        max_write_batch_size: arguments.max_write_batch_size,
        max_write_batch_delay: Duration::from_micros(arguments.max_write_batch_delay_us),
        prepared_cache_capacity: arguments.prepared_cache_capacity,
        request_timeout: Duration::from_millis(arguments.request_timeout_ms),
        max_body_bytes: arguments.max_body_bytes,
    })
    .await?;
    let listener = tokio::net::TcpListener::bind(arguments.listen).await?;
    tracing::info!(address = %arguments.listen, "TriviumDB Server 已启动 (TriviumDB Server started)");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "无法安装 Ctrl+C 信号处理器 (Failed to install Ctrl+C handler)");
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "无法安装终止信号处理器 (Failed to install termination handler)");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("正在优雅关闭服务 (Graceful shutdown started)");
}
