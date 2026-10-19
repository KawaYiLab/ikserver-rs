//! MCP prompts：把方法指引包装成可直接选用的提示模板。

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::engine::guide::{GUIDES, find};

struct PromptSpec {
    name: &'static str,
    mode: &'static str,
    arg: &'static str,
    arg_description: &'static str,
    task: &'static str,
}

const PROMPTS: &[PromptSpec] = &[
    PromptSpec {
        name: "chapter_summary",
        mode: "summary",
        arg: "chapter",
        arg_description: "章节 id 或章节名，如 ch:Chapter110、云霞同归处",
        task: "请按上面的方法，总结绝区零主线章节「{}」的剧情。",
    },
    PromptSpec {
        name: "character_arc",
        mode: "character",
        arg: "name",
        arg_description: "角色名，如 妮可、星见雅",
        task: "请按上面的方法，梳理角色「{}」在绝区零主线中的角色弧线。",
    },
    PromptSpec {
        name: "event_timeline",
        mode: "timeline",
        arg: "topic",
        arg_description: "事件、组织或谜团，如 旧都陷落、赫利俄斯机关",
        task: "请按上面的方法，整理「{}」相关的事件时间线。",
    },
    PromptSpec {
        name: "fact_check",
        mode: "verify",
        arg: "claim",
        arg_description: "要查证的台词或说法",
        task: "请按上面的方法，查证这个说法是否出自绝区零原文：{}",
    },
];

pub fn list() -> Value {
    let prompts: Vec<Value> = PROMPTS
        .iter()
        .filter_map(|p| {
            let guide = GUIDES.iter().find(|g| g.mode == p.mode)?;
            Some(json!({
                "name": p.name,
                "title": guide.title,
                "description": guide.description,
                "arguments": [{ "name": p.arg, "description": p.arg_description, "required": true }]
            }))
        })
        .collect();
    json!({ "prompts": prompts })
}

#[derive(Deserialize)]
pub struct GetParams {
    pub name: String,
    #[serde(default)]
    pub arguments: std::collections::HashMap<String, String>,
}

#[derive(Serialize)]
pub struct GetResult {
    description: &'static str,
    messages: Vec<Value>,
}

pub fn get(params: &GetParams) -> Result<GetResult, String> {
    let spec = PROMPTS
        .iter()
        .find(|p| p.name == params.name)
        .ok_or_else(|| format!("未知提示: {}", params.name))?;
    let guide = find(spec.mode).ok_or_else(|| format!("缺少指引: {}", spec.mode))?;
    let value = params
        .arguments
        .get(spec.arg)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let value = value.ok_or_else(|| format!("缺少参数 {}", spec.arg))?;
    let text = format!(
        "{}\n\n---\n\n{}",
        guide.body.trim_end(),
        spec.task.replace("{}", value)
    );
    Ok(GetResult {
        description: guide.description,
        messages: vec![json!({ "role": "user", "content": { "type": "text", "text": text } })],
    })
}
