//! 检索召回评测：在本地语料上跑 `eval/recall.json` 的题目，打印每题是否命中与总分。
//!
//! 用法：`cargo run -p ik-story --release --example recall_eval -- <语料目录或包> [题目文件]`
//!
//! 每题在输出的前 `top` 条结果里（不写 `top` 时为整段输出）找 `expect` 中任一字符串。
//! `needs_speakers` 的题依赖逐句署名，语料没有署名时记为跳过。
use std::io::Write as _;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Suite {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    tool: String,
    args: Value,
    #[serde(default)]
    top: Option<usize>,
    expect: Vec<String>,
    #[serde(default)]
    needs_speakers: bool,
}

/// 结果条目以「1. doc_…」开头；取前 `top` 条的文字。
fn top_results(text: &str, top: usize) -> String {
    let mut out = String::new();
    let mut seen = 0usize;
    let mut inside = false;
    for line in text.lines() {
        let numbered = line
            .split_once(". ")
            .is_some_and(|(n, rest)| n.parse::<usize>().is_ok() && rest.starts_with("doc_"));
        if numbered {
            seen += 1;
            inside = seen <= top;
        } else if line.starts_with("目录建议") || line.starts_with("继续正文") {
            inside = false;
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let source = args
        .next()
        .context("用法：recall_eval <语料目录或包> [题目文件]")?;
    let cases = args
        .next()
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/eval/recall.json").to_owned());
    let suite: Suite = serde_json::from_str(&std::fs::read_to_string(&cases)?)?;
    let corpus = ik_story::load_source(std::path::Path::new(&source))?;
    let has_speakers = corpus.speakers.len() > 1;
    let mut stdout = std::io::stdout().lock();
    let (mut pass, mut total) = (0usize, 0usize);
    for case in &suite.cases {
        if case.needs_speakers && !has_speakers {
            writeln!(stdout, "SKIP {}（语料没有逐句署名）", case.id)?;
            continue;
        }
        total += 1;
        let text = match ik_story::mcp::tools::call(&corpus, &case.tool, Some(case.args.clone())) {
            Ok(Ok(text)) => text,
            Ok(Err(error)) => error.0,
            Err(_) => bail!("未知工具：{}", case.tool),
        };
        let scope = case
            .top
            .map_or_else(|| text.clone(), |k| top_results(&text, k));
        let ok = case.expect.iter().any(|e| scope.contains(e.as_str()));
        pass += usize::from(ok);
        writeln!(stdout, "{} {}", if ok { "PASS" } else { "FAIL" }, case.id)?;
        if !ok && std::env::var_os("RECALL_EVAL_VERBOSE").is_some() {
            writeln!(
                stdout,
                "{}\n",
                text.lines().take(14).collect::<Vec<_>>().join("\n")
            )?;
        }
    }
    writeln!(stdout, "\n{pass}/{total} 通过")?;
    Ok(())
}
