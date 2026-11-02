//! ik_guide：按任务返回做法指引（检索与阅读步骤、证据判断），后附该主题的进阶参数与结果字段参考。

use serde::Deserialize;

use super::{ToolResult, err};

pub struct Guide {
    pub mode: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// 做法：步骤与证据判断。
    pub method: &'static str,
    /// 进阶参数与结果字段参考。
    pub body: &'static str,
}

pub const GUIDES: &[Guide] = &[
    Guide {
        mode: "summary",
        title: "章节总结",
        description: "顺读整章、补充材料与总结写法",
        method: include_str!("../../guides/method/summary.md"),
        body: include_str!("../../guides/summary.md"),
    },
    Guide {
        mode: "character",
        title: "角色分析与扮演",
        description: "角色档案、台词归纳与扮演素材",
        method: include_str!("../../guides/method/character.md"),
        body: include_str!("../../guides/character.md"),
    },
    Guide {
        mode: "timeline",
        title: "事件时间线",
        description: "时间线索、排序依据与先后的确定程度",
        method: include_str!("../../guides/method/timeline.md"),
        body: include_str!("../../guides/timeline.md"),
    },
    Guide {
        mode: "verify",
        title: "原文查证",
        description: "证据分量、检索与精读步骤、出处写法",
        method: include_str!("../../guides/method/verify.md"),
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
            "[ik_guide] {} | {}\n\n{}\n---\n\n以下是进阶参数与结果字段，需要时再查：\n\n{}",
            g.mode, g.title, g.method, g.body
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
