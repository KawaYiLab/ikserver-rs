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
    /// 入站考试题库运维
    #[command(subcommand)]
    Exam(ExamCommand),
    /// 搜索索引运维
    #[command(subcommand)]
    Search(SearchCommand),
    /// 装扮目录运维
    #[command(subcommand)]
    Cosmetics(CosmeticsCommand),
    /// AI 助手配置运维
    #[command(subcommand)]
    Ai(AiCommand),
    /// 从旧 Strapi 库一次性迁移数据（先把旧库恢复到本库的 legacy schema，见 crates/legacy/README.md）
    #[command(subcommand)]
    Legacy(LegacyCommand),
}

#[derive(Debug, Subcommand)]
enum LegacyCommand {
    /// 只读预检：列出导入会拒绝的问题、会自动修正的数据和时区判断依据
    Check,
    /// 预先读取旧文件（对象存储）算哈希与真实类型，可续跑；文件不搬家，新旧系统用同一个桶
    Media {
        /// 旧文件 URL 的公开前缀，去掉后剩下对象 key；可重复。默认取 storage.public_base_url
        #[arg(long = "url-prefix")]
        url_prefixes: Vec<String>,
        /// 同时读取的文件数
        #[arg(long, default_value_t = 8)]
        concurrency: usize,
        /// 重新读取之前失败的文件
        #[arg(long)]
        retry_failed: bool,
    },
    /// 在一个事务里导入全部数据；目标库必须是刚迁移好的空库
    Import {
        /// 旧库时间列所用的时区（旧服务进程的时区），例如 Asia/Shanghai 或 UTC；没有默认值
        #[arg(long)]
        legacy_timezone: String,
    },
}

#[derive(Debug, Subcommand)]
enum AiCommand {
    /// 从 JSON 文件导入 AI 配置：全局上限、模型（按 key）、角色卡（按 slug）新增或更新；
    /// 整份一个事务。角色卡绑定的账号会被标为 AI 账号
    Import {
        /// `{"settings":{…},"models":[…],"characters":[…]}`，字段见 `ik_ai::dto::ImportFile`
        path: std::path::PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum CosmeticsCommand {
    /// 从 JSON 文件导入系统头像与论坛名片（按名字新增或更新；整份一个事务，有一项不合格就都不写入）
    Import {
        /// JSON 数组，每项 `{"type":"avatar","name","kind","imageId"}` 或
        /// `{"type":"card","name","kind","description","story","imageId"}`
        path: std::path::PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum ExamCommand {
    /// 从 JSON 文件导入题目（数组，每项字段同 QuestionInput：question、kind、options、correctKeys……）
    Import {
        /// JSON 文件路径
        path: std::path::PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum SearchCommand {
    /// 推送索引设置并全量重建（删除库里已经没有的残留文档），等处理完再退出
    Reindex,
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
        Command::Exam(cmd) => exam_command(cmd).await,
        Command::Cosmetics(CosmeticsCommand::Import { path }) => {
            let state = connect_for_cli().await?;
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("读取 {} 失败", path.display()))?;
            let items: Vec<ik_cosmetics::dto::CatalogItem> =
                serde_json::from_str(&raw).context("装扮文件不是合法的 JSON 数组")?;
            let report = ik_cosmetics::service::import(&state.cosmetics, items)
                .await
                .map_err(|p| {
                    let fields: Vec<String> = p
                        .field_errors()
                        .iter()
                        .map(|e| format!("{}：{}", e.field, e.detail))
                        .collect();
                    anyhow::anyhow!("{p} {}", fields.join("；"))
                })?;
            tracing::info!(
                avatars = report.avatars,
                cards = report.cards,
                refreshed_users = report.refreshed_users,
                "装扮目录已导入"
            );
            Ok(())
        }
        Command::Ai(AiCommand::Import { path }) => {
            let state = connect_for_cli().await?;
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("读取 {} 失败", path.display()))?;
            let file: ik_ai::dto::ImportFile =
                serde_json::from_str(&raw).context("AI 配置文件不是合法的 JSON")?;
            let report = ik_ai::service::import(&state.ai, file).await.map_err(|p| {
                let fields: Vec<String> = p
                    .field_errors()
                    .iter()
                    .map(|e| format!("{}：{}", e.field, e.detail))
                    .collect();
                anyhow::anyhow!("{p} {}", fields.join("；"))
            })?;
            tracing::info!(
                models = report.models,
                characters = report.characters,
                "AI 配置已导入"
            );
            Ok(())
        }
        Command::Legacy(cmd) => legacy_command(cmd).await,
        Command::Search(SearchCommand::Reindex) => {
            let state = connect_for_cli().await?;
            state.search.reindex().await?;
            tracing::info!("搜索索引已重建");
            Ok(())
        }
    }
}

/// 逐题校验并写入；遇到不合格的题立即停止并报出是第几题（前面的题已经写入）。
async fn exam_command(cmd: ExamCommand) -> anyhow::Result<()> {
    let state = connect_for_cli().await?;
    match cmd {
        ExamCommand::Import { path } => {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("读取 {} 失败", path.display()))?;
            let questions: Vec<ik_exam::dto::QuestionInput> =
                serde_json::from_str(&raw).context("题目文件不是合法的 JSON 数组")?;
            let mut imported = 0usize;
            for (index, question) in questions.into_iter().enumerate() {
                ik_exam::service::create_question(&state.exam, question)
                    .await
                    .map_err(|p| {
                        let fields: Vec<String> = p
                            .field_errors()
                            .iter()
                            .map(|e| format!("{}：{}", e.field, e.detail))
                            .collect();
                        anyhow::anyhow!("第 {} 题：{p} {}", index + 1, fields.join("；"))
                    })?;
                imported += 1;
            }
            tracing::info!(imported, "题目已导入");
        }
    }
    Ok(())
}

async fn legacy_command(cmd: LegacyCommand) -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let config = Config::load()?;
    init_tracing(config.log.format);
    let db = ik_kernel::db::connect(&config.database)
        .await
        .context("连接数据库失败")?;
    let mut out = std::io::stdout().lock();
    match cmd {
        LegacyCommand::Check => {
            for finding in ik_legacy::check(&db).await? {
                writeln!(
                    out,
                    "[{:?}] {} = {}  {}",
                    finding.severity, finding.key, finding.count, finding.message
                )?;
            }
        }
        LegacyCommand::Media {
            mut url_prefixes,
            concurrency,
            retry_failed,
        } => {
            if url_prefixes.is_empty() {
                let base = config.storage.public_base_url.trim_end_matches('/');
                anyhow::ensure!(
                    !base.is_empty(),
                    "没有配置 storage.public_base_url，请用 --url-prefix 指定旧文件 URL 的前缀"
                );
                url_prefixes.push(format!("{base}/"));
            }
            let storage = ik_media::storage::Storage::new(&config.storage);
            let options = ik_legacy::media::MediaOptions {
                url_prefixes,
                concurrency,
                retry_failed,
            };
            let report = ik_legacy::media::fetch_digests(&db, &storage, &options).await?;
            writeln!(out, "读取成功 {}，失败 {}", report.fetched, report.failed)?;
        }
        LegacyCommand::Import { legacy_timezone } => {
            let options = ik_legacy::ImportOptions {
                legacy_timezone,
                now: chrono::Utc::now(),
            };
            let report = ik_legacy::import(&db, &options).await?;
            for step in &report.steps {
                writeln!(out, "{:<24} {}", step.step, step.rows)?;
            }
            writeln!(
                out,
                "导入完成。接着运行 `ik-server search reindex` 重建搜索索引。"
            )?;
        }
    }
    Ok(())
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
    ik_kernel::jobs::Jobs::new(db.clone()).migrate().await?;
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
    let state = AppState::new(config, db)?;
    state
        .search
        .start_indexer(&state.bus, &state.config.search)?;
    let hub = state.realtime.hub.clone();
    let worker = if state.config.jobs.run_worker {
        let worker = std::sync::Arc::new(
            ik_server::jobs::worker_options(&state)?
                .init()
                .await
                .map_err(|e| anyhow::anyhow!("后台任务执行器启动失败：{e:?}"))?,
        );
        let runner = worker.clone();
        let handle = tokio::spawn(async move {
            if let Err(error) = runner.run().await {
                tracing::error!(?error, "后台任务执行器异常退出");
            }
        });
        Some((worker, handle))
    } else {
        None
    };
    let app = build_app(state)?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("监听 {addr} 失败"))?;
    tracing::info!(%addr, "ik-server 已启动");
    if !ik_server::admin_ui::is_bundled() {
        tracing::warn!(
            "这个版本没有打包管理后台，/admin 不可用（发布前先在 admin/ 下 pnpm build）"
        );
    }
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        // WebSocket 是长连接，不主动关掉的话优雅停机会一直等下去。
        hub.shutdown();
    })
    .await?;
    if let Some((worker, handle)) = worker {
        // 不再领取新任务，等在途任务做完。
        worker.request_shutdown();
        if let Err(error) = handle.await {
            tracing::error!(?error, "后台任务执行器退出失败");
        }
    }
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
