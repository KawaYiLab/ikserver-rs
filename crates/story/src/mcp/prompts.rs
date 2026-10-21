//! MCP prompts：用户选用的任务文本，不附带工具工作流或回答模板。

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

struct PromptSpec {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    arg: &'static str,
    arg_description: &'static str,
    task: &'static str,
}

const PROMPTS: &[PromptSpec] = &[
    PromptSpec {
        name: "chapter_summary",
        title: "剧情总结",
        description: "总结指定主线章节的剧情",
        arg: "chapter",
        arg_description: "章节 id 或章节名，如 ch:Chapter110、云霞同归处",
        task: "请总结绝区零主线章节「{}」的剧情。",
    },
    PromptSpec {
        name: "character_arc",
        title: "角色弧线",
        description: "梳理指定角色在主线中的角色弧线",
        arg: "name",
        arg_description: "角色名，如 妮可、星见雅",
        task: "请梳理角色「{}」在绝区零主线中的角色弧线。",
    },
    PromptSpec {
        name: "event_timeline",
        title: "事件时间线",
        description: "整理指定事件、组织或谜团的时间线",
        arg: "topic",
        arg_description: "事件、组织或谜团，如 旧都陷落、赫利俄斯机关",
        task: "请整理「{}」相关的事件时间线。",
    },
    PromptSpec {
        name: "fact_check",
        title: "原文查证",
        description: "查证指定台词或说法的原文出处",
        arg: "claim",
        arg_description: "要查证的台词或说法",
        task: "请查证这个说法是否出自绝区零原文：{}",
    },
];

pub fn list() -> Value {
    let prompts: Vec<Value> = PROMPTS
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "title": p.title,
                "description": p.description,
                "arguments": [{ "name": p.arg, "description": p.arg_description, "required": true }]
            })
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
    let value = params
        .arguments
        .get(spec.arg)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let value = value.ok_or_else(|| format!("缺少参数 {}", spec.arg))?;
    let text = spec.task.replace("{}", value);
    Ok(GetResult {
        description: spec.description,
        messages: vec![json!({ "role": "user", "content": { "type": "text", "text": text } })],
    })
}
