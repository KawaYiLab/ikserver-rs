//! ik_guide：按主题返回工具参数与结果字段的参考说明。

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
        title: "章节资料参考",
        description: "章节选择、原文读取、分页与返回行覆盖字段",
        body: include_str!("../../guides/summary.md"),
    },
    Guide {
        mode: "character",
        title: "角色资料参考",
        description: "角色档案、身份变体、署名与提及统计字段",
        body: include_str!("../../guides/character.md"),
    },
    Guide {
        mode: "timeline",
        title: "顺序与时间参考",
        description: "目录排列、文档行序与剧情时间的区别及相关字段",
        body: include_str!("../../guides/timeline.md"),
    },
    Guide {
        mode: "verify",
        title: "原文查证",
        description: "检索匹配、来源标识与证据状态字段",
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
        Some(g) => Ok(format!("[ik_guide] {} | {}\n\n{}", g.mode, g.title, g.body)),
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
