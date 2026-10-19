//! zzz_guide：任务方法指引（同时以 MCP prompts 形式提供）。

use serde::Deserialize;

use super::{ToolResult, err};

pub struct Guide {
    pub mode: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub body: &'static str,
}

pub const GUIDES: &[Guide] = &[
    Guide {
        mode: "summary",
        title: "剧情总结",
        description: "总结一章、一个任务或若干场景：规划阅读范围、顺读、分层压缩、带出处输出",
        body: include_str!("../../guides/summary.md"),
    },
    Guide {
        mode: "character",
        title: "角色弧线",
        description: "梳理一个角色的登场、关系与变化，输出有出处的人物小传",
        body: include_str!("../../guides/character.md"),
    },
    Guide {
        mode: "timeline",
        title: "事件时间线",
        description: "围绕事件、组织或谜团按剧情顺序整理时间线",
        body: include_str!("../../guides/timeline.md"),
    },
    Guide {
        mode: "verify",
        title: "原文查证",
        description: "确认台词或说法是否出自原文并给出精确出处",
        body: include_str!("../../guides/verify.md"),
    },
];

pub fn find(mode: &str) -> Option<&'static Guide> {
    let m = mode.trim().to_ascii_lowercase();
    let alias = match m.as_str() {
        "总结" | "剧情总结" | "summarize" => "summary",
        "角色" | "角色弧线" | "人物" => "character",
        "时间线" | "事件" => "timeline",
        "查证" | "考据" | "search" | "fact_check" => "verify",
        other => other,
    };
    GUIDES.iter().find(|g| g.mode == alias)
}

#[derive(Debug, Default, Deserialize)]
pub struct GuideArgs {
    #[serde(default)]
    pub mode: String,
}

pub fn run(args: &GuideArgs) -> ToolResult {
    match find(&args.mode) {
        Some(g) => Ok(format!(
            "[zzz_guide] {} | {}\n\n{}",
            g.mode, g.title, g.body
        )),
        None => {
            let modes: Vec<String> = GUIDES
                .iter()
                .map(|g| format!("{}（{}）", g.mode, g.title))
                .collect();
            err(format!(
                "未知模式「{}」。可选：{}",
                args.mode,
                modes.join("、")
            ))
        }
    }
}
