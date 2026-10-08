use std::io::Write;

use anyhow::Context;
use clap::{Parser, Subcommand};
use ik_kernel::config::{Config, LogFormat};
use ik_server::{AppState, build_app, openapi};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "ik-server", version, about = "InterKnot 后端")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 启动 HTTP 服务（默认）
    Serve,
    /// 把 OpenAPI 文档（JSON）输出到标准输出
    Openapi,
    /// 对 DATABASE_URL 指向的库执行全部待应用的迁移
    Migrate,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Openapi => {
            let json = openapi().to_pretty_json()?;
            writeln!(std::io::stdout().lock(), "{json}")?;
            Ok(())
        }
        Command::Serve => serve().await,
        Command::Migrate => migrate().await,
    }
}

async fn migrate() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let config = Config::load()?;
    init_tracing(config.log.format);
    let db = ik_kernel::db::connect(&config.database)
        .await
        .context("连接数据库失败")?;
    ik_server::MIGRATOR.run(&db).await.context("执行迁移失败")?;
    tracing::info!("迁移完成");
    Ok(())
}

async fn serve() -> anyhow::Result<()> {
    // .env 只用于本机开发；生产由 systemd EnvironmentFile 注入，缺失不是错误。
    let _ = dotenvy::dotenv();
    let config = Config::load()?;
    init_tracing(config.log.format);

    let db = ik_kernel::db::connect(&config.database)
        .await
        .context("连接数据库失败")?;
    let addr = config.http.addr;
    let app = build_app(AppState::new(config, db)?)?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("监听 {addr} 失败"))?;
    tracing::info!(%addr, "ik-server 已启动");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    tracing::info!("ik-server 已退出");
    Ok(())
}

fn init_tracing(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match format {
        LogFormat::Json => builder.json().init(),
        LogFormat::Pretty => builder.pretty().init(),
    }
}

/// Ctrl+C 或（Unix 上的）SIGTERM 触发优雅停机：停止接收新连接，等待在途请求完成。
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "无法监听 Ctrl+C");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "无法监听 SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("收到停机信号，开始优雅停机");
}
