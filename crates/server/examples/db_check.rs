//! 开发库诊断：`cargo run -p ik-server --example db_check`
//!
//! 打印服务器版本、TLS 情况与当前账号权限，判断能否跑迁移与 `#[sqlx::test]`。
//! 只读查询；不输出连接串与密码。

use std::io::Write;

use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let url = std::env::var("DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&url)
        .await?;

    let (version,): (String,) = sqlx::query_as("SELECT version()").fetch_one(&pool).await?;
    let (server_ssl,): (String,) = sqlx::query_as("SHOW ssl").fetch_one(&pool).await?;
    let (conn_ssl,): (bool,) = sqlx::query_as(
        "SELECT COALESCE((SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()), false)",
    )
    .fetch_one(&pool)
    .await?;
    let (user, superuser, createdb): (String, bool, bool) = sqlx::query_as(
        "SELECT rolname::text, rolsuper, rolcreatedb FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(&pool)
    .await?;
    let (owner, encoding, collate): (String, String, String) = sqlx::query_as(
        "SELECT pg_get_userbyid(datdba)::text, pg_encoding_to_char(encoding)::text, datcollate::text \
         FROM pg_database WHERE datname = current_database()",
    )
    .fetch_one(&pool)
    .await?;
    let (can_create_public,): (bool,) =
        sqlx::query_as("SELECT has_schema_privilege('public', 'CREATE')")
            .fetch_one(&pool)
            .await?;
    let (user_tables,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM pg_tables WHERE schemaname NOT IN ('pg_catalog', 'information_schema')",
    )
    .fetch_one(&pool)
    .await?;
    let (timezone,): (String,) = sqlx::query_as("SHOW TimeZone").fetch_one(&pool).await?;

    let mut out = std::io::stdout().lock();
    writeln!(out, "server_version     {version}")?;
    writeln!(out, "server_ssl         {server_ssl}")?;
    writeln!(out, "this_conn_tls      {conn_ssl}")?;
    writeln!(
        out,
        "current_user       {user} (superuser={superuser}, createdb={createdb})"
    )?;
    writeln!(
        out,
        "database_owner     {owner}  encoding={encoding}  collate={collate}"
    )?;
    writeln!(out, "create_in_public   {can_create_public}")?;
    writeln!(out, "user_tables        {user_tables}")?;
    writeln!(out, "timezone           {timezone}")?;
    Ok(())
}
