use std::io::Write;

use anyhow::Context;
use clap::{Parser, Subcommand};
use ik_kernel::Role;
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
    /// 用户运维
    #[command(subcommand)]
    User(UserCommand),
    /// 分区运维
    #[command(subcommand)]
    Category(CategoryCommand),
}

#[derive(Debug, Subcommand)]
enum UserCommand {
    /// 设置用户角色（例如创建第一个管理员）
    SetRole {
        /// 用户邮箱
        email: String,
        /// user / moderator / admin
        role: Role,
    },
}

#[derive(Debug, Subcommand)]
enum CategoryCommand {
    /// 新建分区
    Create {
        /// URL 用的标识，小写字母、数字、连字符
        #[arg(long)]
        slug: String,
        /// 展示名
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        description: String,
        /// 排序（小的在前）
        #[arg(long, default_value_t = 0)]
        order: i32,
        /// 只有管理员能发帖
        #[arg(long)]
        admin_only: bool,
        /// 允许帖主采纳评论
        #[arg(long)]
        allow_accept: bool,
    },
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
        Command::User(cmd) => user_command(cmd).await,
        Command::Category(cmd) => category_command(cmd).await,
    }
}

async fn connect_for_cli() -> anyhow::Result<AppState> {
    let _ = dotenvy::dotenv();
    let config = Config::load()?;
    init_tracing(config.log.format);
    let db = ik_kernel::db::connect(&config.database)
        .await
        .context("连接数据库失败")?;
    AppState::new(config, db)
}

async fn user_command(cmd: UserCommand) -> anyhow::Result<()> {
    let state = connect_for_cli().await?;
    match cmd {
        UserCommand::SetRole { email, role } => {
            let found = ik_auth::service::set_role_by_email(&state.db, &email, role)
                .await
                .map_err(|p| anyhow::anyhow!("{p}"))?;
            anyhow::ensure!(found, "找不到该邮箱的用户");
            tracing::info!(role = %role, "角色已更新");
        }
    }
    Ok(())
}

async fn category_command(cmd: CategoryCommand) -> anyhow::Result<()> {
    let state = connect_for_cli().await?;
    match cmd {
        CategoryCommand::Create {
            slug,
            name,
            description,
            order,
            admin_only,
            allow_accept,
        } => {
            let created = ik_content::service::create_category(
                &state.content,
                ik_content::service::NewCategoryInput {
                    slug,
                    name,
                    description,
                    sort_order: order,
                    admin_only,
                    allow_comment_accept: allow_accept,
                },
            )
            .await
            .map_err(|p| anyhow::anyhow!("{p}"))?;
            tracing::info!(id = %created.id, slug = %created.slug, "分区已创建");
        }
    }
    Ok(())
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
