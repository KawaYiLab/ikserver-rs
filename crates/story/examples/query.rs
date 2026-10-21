//! 从本地语料运行 MCP 工具，无需启动服务或连接数据库。
use anyhow::{Context, Result, bail};
use serde_json::Value;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(source) = args.next() else {
        bail!("用法：query <语料目录或包> <工具名> [JSON 参数]");
    };
    let Some(tool) = args.next() else {
        bail!("缺少工具名，如 ik_catalog / ik_search / ik_read");
    };
    let arguments: Value = serde_json::from_str(&args.next().unwrap_or_else(|| "{}".into()))
        .context("工具参数必须是 JSON")?;
    let corpus = ik_story::load_source(std::path::Path::new(&source))?;
    let result = ik_story::mcp::tools::call(&corpus, &tool, Some(arguments))
        .map_err(|_| anyhow::anyhow!("未知工具：{tool}"))?;
    match result {
        Ok(text) => println!("{text}"),
        Err(error) => bail!("{}", error.0),
    }
    Ok(())
}
