//! 运行：cargo run -p triviumdb-server --example embedded_console -- app.tdb
//! 仅宿主打开文件；HTTP 组件借用同一实例，默认只读。

use std::sync::{Arc, Mutex};
use triviumdb::{ConsoleHost, ConsoleOptions, Database};
use triviumdb_server::console::{ConsoleHttpOptions, build_console_app};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "embedded-console.tdb".into());
    let database = Arc::new(Mutex::new(Database::<f32>::open(&path, 2)?));
    let host = ConsoleHost::attach(&database, ConsoleOptions::default())?;
    let session = host.session();
    let app = build_console_app(host, ConsoleHttpOptions::default())?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080").await?;
    eprintln!(
        "嵌入式控制台 RPC 已启动 (Embedded console RPC started): http://127.0.0.1:8080/v1/console"
    );
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                eprintln!("等待关闭信号失败 (Failed to wait for shutdown signal): {error}");
            }
        })
        .await;
    session.detach();
    tokio::task::spawn_blocking(move || {
        database
            .lock()
            .map_err(|_| std::io::Error::other("宿主互斥锁中毒 (Host mutex poisoned)"))?
            .close()
    })
    .await??;
    served?;
    Ok(())
}
