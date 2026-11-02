//! Model-facing tool contracts and dispatch.
use crate::corpus::Corpus;
use crate::engine::{self, ToolError, ToolResult};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
fn read_only() -> Value {
    json!({"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false})
}

/// 一个工具的完整契约：`properties` 是服务端接受的全部参数，`visible` 是写进 `tools/list` 给模型看的那部分。
/// 其余参数（分页细节、证据展开、输出格式等）照常接受，由结果里的 nextCall 与 `ik_guide` 的进阶参数说明引出，
/// 不占模型每轮都要读的工具定义。
struct Spec {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    properties: Value,
    visible: &'static [&'static str],
    listed: bool,
}

fn specs() -> &'static [Spec] {
    static SPECS: std::sync::LazyLock<Vec<Spec>> = std::sync::LazyLock::new(build_specs);
    &SPECS
}

fn build_specs() -> Vec<Spec> {
    use utoipa::PartialSchema;
    let cursor = json!({"type":"string","maxLength":16384,"description":"翻页用：原样传入上一次结果里 nextCall 给的 cursor，不要和其他参数一起改。"});
    let detail = json!({"type":"string","enum":["compact","full"],"default":"compact","description":"compact 简明结果；full 展开顺序、署名与来源状态。"});
    let scope = json!({"type":"string","enum":["all","story","supplement","lore","event"],"description":"资料范围：story 主线剧情、supplement 过场字幕与漫画、lore 设定与角色档案、event 活动与支线，默认 all。"});
    let evidence = json!({"type":"boolean","default":false,"description":"展开逐行来源证据，未知状态保持未知。"});
    let integer =
        |min, max, default| json!({"type":"integer","minimum":min,"maximum":max,"default":default});
    vec![
        Spec {
            name: "ik_search",
            title: "剧情检索",
            description: "绝区零剧情与设定的全文检索。回答剧情、角色、台词出处、设定考据类问题时，先用它找到候选文档，再用 ik_read 读原文确认后作答，不要只凭检索片段下结论。keyword 写空格分隔的实体词（角色名、地名、组织、事件名、台词短句），不要整句提问；多个词在同一篇里都出现才算完整命中，没有完整命中时会给出部分匹配或可直接使用的改写调用。speaker 只看某人说的台词。每条结果附原文入口 ik_read(docId, offset)，结果多时用 nextCall 翻页。例：ik_search(keyword=\"安比 零号空洞\")；ik_search(speaker=\"妮可\", keyword=\"欠债\")；ik_search(keyword=\"核心\", path=\"3.3版本\") 只搜 3.3 版本新增主线。",
            properties: json!({"keyword":{"type":"string","description":"空格分隔的检索词，建议用角色名、地名、事件名或台词短句；同一篇全部出现才算完整命中。"},"speaker":{"type":"string","description":"只看这个角色说的台词（按逐句署名，已知的身份变体会合并）；可与 keyword 同用，也可单独用。"},"scope":scope,"path":{"type":"string","description":"限定范围：章节 ID（如 ch:Chapter110）、章节名或版本别名（如 3.3版本，表示该版新增主线）。"},"docId":{"type":"string","description":"只在这一篇文档里找，返回篇内命中行号。"},"purpose":{"type":"string","enum":["browse","locate","verify"],"description":"verify 查证原话：只匹配正文、要求全部词命中；locate 定位出处；browse 一般浏览（默认）。"},"matchIn":{"type":"string","enum":["all","body"],"description":"all允许正文及元数据参与同篇AND；body仅正文。"},"matchUnit":{"type":"string","enum":["doc","window"],"default":"doc"},"windowLines":integer(1,100,12),"includeEvidence":evidence,"detail":detail,"cursor":cursor,"offset":integer(0,usize::MAX,0),"directoryOffset":integer(0,usize::MAX,0),"maxDirectoryResults":integer(1,50,10),"maxResults":{"type":"integer","minimum":1,"maximum":500,"description":"每页最多几篇，默认 10、上限 50；指定 docId 时为篇内命中行数，默认 50。"},"hitsPerDoc":integer(1,20,3),"allowPartial":{"type":"boolean","description":"默认true；verify默认false；显式值优先。未显式指定时，有词零命中或没有任何两词同篇则不合并。"},"contextLines":{"type":"integer","minimum":0,"maximum":3,"description":"展示邻行，不参与窗口匹配；默认1，speaker筛选默认0。"},"pagePart":{"type":"string","enum":["both","body","directory"],"description":"指定正文/目录分页部分；返回游标包含此值。"}}),
            visible: &[
                "keyword",
                "speaker",
                "scope",
                "path",
                "docId",
                "purpose",
                "maxResults",
                "cursor",
            ],
            listed: true,
        },
        Spec {
            name: "ik_read",
            title: "读取原文",
            description: "读取原文。拿到 ik_search 或 ik_catalog 给出的 docId 后用它核对原文；引用台词或下结论前应当读过对应原文。docId 可以是文档 ID（doc_…）、章节分组（如 ch:Chapter110，按目录顺序连读整章）或版本别名（如 3.3版本）。一次返回一段，没读完时结果末尾有 nextCall，原样调用即可续读；docIds 可一次读 1～16 篇。注意目录顺序不一定是剧情发生顺序。例：ik_read(docId=\"doc_b7c84bb35d58\", offset=120, limit=60)；ik_read(docId=\"ch:Chapter110\")。",
            properties: json!({"docId":{"type":"string","description":"文档 ID（doc_…）、章节分组 ID（ch:…）或版本别名。"},"docIds":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"string"},"description":"一次读多篇文档，与 docId、follow 不能同时用。"},"offset":{"type":"integer","minimum":1,"default":1,"description":"从第几行开始读（检索结果里的行号可以直接填）。"},"limit":{"type":"integer","minimum":1,"maximum":5000,"default":2000,"description":"最多读多少行。"},"maxChars":integer(500,60000,16000),"follow":{"type":"boolean","description":"读完这篇后接着读同一章节里的下一篇。"},"lineNumbers":{"type":"boolean","default":true},"includeSources":{"type":"boolean","default":false},"includeEvidence":evidence,"cursor":cursor,"detail":detail,"batchIndex":integer(0,15,0)}),
            visible: &["docId", "docIds", "offset", "limit", "follow", "cursor"],
            listed: true,
        },
        Spec {
            name: "ik_catalog",
            title: "剧情目录",
            description: "浏览目录：主线章节、活动与支线、代理人档案等设定分类。不知道从哪读起、要总结整章、或想知道某类资料有哪些时用。不带参数返回顶层目录；path 填分组 ID、章节名或版本别名进入下一层；keyword 按标题筛选。每一项都附 ik_read 入口。目录相邻不代表剧情先后。例：ik_catalog()；ik_catalog(path=\"ch:Chapter110\")；ik_catalog(keyword=\"狡兔屋\")。",
            properties: json!({"path":{"type":"string","description":"分组 ID（如 ch:Chapter110、lore:agents）、章节名、版本别名或文档 ID；不填返回顶层目录。"},"keyword":{"type":"string","description":"按标题或路径筛选。"},"offset":integer(0,usize::MAX,0),"maxResults":integer(1,500,100),"cursor":cursor,"detail":detail,"includeEvidence":evidence,"manifest":{"type":"boolean","default":false},"scope":scope,"excludePlaceholders":{"type":"boolean","default":false,"description":"仅排除整篇全部为占位的文档，混合文档保留。"},"excludeUnlinked":{"type":"boolean","default":false,"description":"仅排除整篇全部为未关联候选的文档，混合文档保留。"}}),
            visible: &["path", "keyword", "scope", "cursor"],
            listed: true,
        },
        Spec {
            name: "ik_character",
            title: "角色资料",
            description: "角色资料。问到某个角色是谁、在哪些章节出场、和谁对话最多，或要扮演、分析某个角色的说话风格时用。purpose=overview（默认）返回档案入口、各章台词与被提及次数和相关检索入口；purpose=roleplay 返回代理人档案原文、档案里的日常对话和主线台词样本（附上一句与原文入口）。until 把主线台词限定到某章或某版本为止，避免剧透。资料里只有原文，性格和语气要你根据原文自己归纳。例：ik_character(name=\"星见雅\")；ik_character(name=\"妮可\", purpose=\"roleplay\", until=\"3.2版本\")。",
            properties: json!({"name":{"type":"string","description":"角色名，如 妮可、星见雅。"},"purpose":{"type":"string","enum":["overview","roleplay"],"default":"overview","description":"overview 出场概览；roleplay 扮演与说话风格素材。"},"until":{"type":"string","description":"仅 roleplay：主线台词截止到这一章或这个版本（含），可填章节 ID、章节名或版本别名；档案不受影响。"},"maxResults":{"type":"integer","minimum":1,"maximum":200,"description":"overview为章节行数，默认40、上限200；roleplay为主线台词样本数，默认8、上限40。"},"maxScenes":{"type":"integer","minimum":1,"maximum":20,"description":"仅roleplay：档案对话段数，默认6；随行气泡整体算一段。"},"includeEvidence":evidence,"detail":detail}),
            visible: &["name", "purpose", "until"],
            listed: true,
        },
        Spec {
            name: "ik_guide",
            title: "做法指引",
            description: "取一份做法指引，较复杂的任务开始前调用一次即可，同一任务不要反复取：verify 原文查证与出处考据，summary 章节剧情总结，character 角色分析与扮演，timeline 事件先后与时间线。指引给出推荐的检索与阅读步骤、怎样判断证据是否可靠，以及各工具的进阶参数。闲聊或一次检索就能回答的简单问题不需要。例：ik_guide(mode=\"verify\")。",
            properties: json!({"mode":{"type":"string","enum":["verify","summary","character","timeline"],"description":"verify 查证、summary 章节总结、character 角色、timeline 时间线。"}}),
            visible: &["mode"],
            listed: true,
        },
        Spec {
            name: "ik_coverage",
            title: "资料覆盖核对",
            description: "对照 catalog.manifest 核对提交的 read.coverageDelta（每段可只填 docId/start/end），合并重复和交叠区间，局部读取不计整篇。返回 ledger、完整/部分/未读统计、缺口 nextRead 和分页 nextCall。统计依据仅为提交的返回行记录，不表示模型理解或服务器请求历史；服务端不保存 ledger。",
            properties: json!({"manifestId":{"type":"string","maxLength":16384},"ranges":{"type":"array","maxItems":512,"items":engine::coverage::RangeInput::schema()},"offset":integer(0,usize::MAX,0),"maxResults":integer(1,100,20)}),
            visible: &["manifestId", "ranges", "offset", "maxResults"],
            listed: false,
        },
    ]
}

/// `tools/list` 的内容：只列 `listed` 的工具，每个工具只写 `visible` 参数。
/// 不写 `additionalProperties:false`：进阶参数虽不在清单里，nextCall 会带上它们，严格校验的客户端不能拒绝。
/// 未知参数由服务端拦下（见 [`check_arguments`]）。
pub fn definitions() -> Result<Value, serde_json::Error> {
    let mut entries = Vec::new();
    for spec in specs().iter().filter(|s| s.listed) {
        let properties: serde_json::Map<String, Value> = spec
            .visible
            .iter()
            .filter_map(|k| {
                spec.properties
                    .get(*k)
                    .map(|v| ((*k).to_owned(), v.clone()))
            })
            .collect();
        let mut schema = json!({"type":"object","properties":properties});
        match spec.name {
            "ik_read" => {
                schema["anyOf"] =
                    json!([{"required":["docId"]},{"required":["docIds"]},{"required":["cursor"]}])
            }
            "ik_character" => schema["required"] = json!(["name"]),
            "ik_guide" => schema["required"] = json!(["mode"]),
            _ => {}
        }
        entries.push(json!({"name":spec.name,"title":spec.title,"description":spec.description,"inputSchema":schema,"outputSchema":engine::response::output_schema(spec.name)?,"annotations":read_only()}));
    }
    Ok(json!({"tools":entries}))
}

/// 指引末尾的进阶参数一览：清单里没写、但服务端接受的参数及其说明（由 [`specs`] 生成，与实际契约一致）。
fn advanced_parameters(mode: &str) -> String {
    let tools: &[&str] = match mode {
        "summary" => &["ik_catalog", "ik_read"],
        "character" => &["ik_character", "ik_search"],
        "timeline" => &["ik_search", "ik_read"],
        _ => &["ik_search", "ik_read"],
    };
    let mut out = String::from("\n## 进阶参数一览\n");
    for spec in specs().iter().filter(|s| tools.contains(&s.name)) {
        let Some(properties) = spec.properties.as_object() else {
            continue;
        };
        out.push_str(&format!("\n{}：\n", spec.name));
        for (key, schema) in properties
            .iter()
            .filter(|(k, _)| !spec.visible.contains(&k.as_str()))
        {
            let mut line = format!("- `{key}`");
            if let Some(values) = schema.get("enum").and_then(Value::as_array) {
                let values = values.iter().filter_map(Value::as_str).collect::<Vec<_>>();
                line.push_str(&format!("（{}）", values.join(" / ")));
            } else if let Some(kind) = schema.get("type").and_then(Value::as_str) {
                line.push_str(&format!("（{kind}）"));
            }
            if let Some(description) = schema.get("description").and_then(Value::as_str) {
                line.push_str(&format!("：{description}"));
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// 拦下工具不认识的参数（含拼错的名字），给出最接近的正确名，让模型改了重试。
fn check_arguments(name: &str, args: &Value) -> Result<(), ToolError> {
    let Some(map) = args.as_object() else {
        return Ok(());
    };
    let Some(spec) = specs().iter().find(|s| s.name == name) else {
        return Ok(());
    };
    let known = spec
        .properties
        .as_object()
        .map(|p| p.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let unknown: Vec<&String> = map.keys().filter(|k| !known.contains(k)).collect();
    if unknown.is_empty() {
        return Ok(());
    }
    let hints = unknown
        .iter()
        .map(|k| match closest(k, &known) {
            Some(near) => format!("{k}（是不是想写 {near}？）"),
            None => (*k).clone(),
        })
        .collect::<Vec<_>>()
        .join("、");
    Err(ToolError(format!(
        "参数错误：{name} 没有参数 {hints}。常用参数：{}。",
        spec.visible.join("、")
    )))
}

/// 名字最接近的已知参数：忽略大小写与下划线后编辑距离不超过 2，或一方包含另一方。
fn closest<'a>(given: &str, known: &'a [String]) -> Option<&'a str> {
    let norm = |s: &str| s.to_ascii_lowercase().replace(['_', '-'], "");
    let g = norm(given);
    known
        .iter()
        .map(|k| {
            let n = norm(k);
            let d = if n == g || (g.len() >= 4 && (n.contains(&g) || g.contains(&n))) {
                0
            } else {
                levenshtein(&g, &n)
            };
            (d, k.as_str())
        })
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| k)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// 未知工具（协议错误，而非工具执行错误）。
pub struct UnknownTool;

fn parse<T: DeserializeOwned + Default>(args: Option<&Value>) -> Result<T, ToolError> {
    match args {
        None | Some(Value::Null) => Ok(T::default()),
        Some(v) => T::deserialize(v).map_err(|e| ToolError(format!("参数错误：{e}"))),
    }
}

/// Local text callers retain the detailed presentation by default.
pub fn call(
    corpus: &Corpus,
    name: &str,
    mut args: Option<Value>,
) -> Result<ToolResult, UnknownTool> {
    if matches!(
        name,
        "ik_catalog" | "ik_search" | "ik_read" | "ik_character"
    ) {
        let value = args.get_or_insert_with(|| json!({}));
        if value.is_null() {
            *value = json!({});
        }
        if value.is_object()
            && value.get("cursor").is_none_or(Value::is_null)
            && value.get("detail").is_none_or(Value::is_null)
        {
            value["detail"] = json!("full");
        }
    }
    call_output(corpus, name, args).map(|r| r.map(|o| o.text))
}
pub fn call_output(
    corpus: &Corpus,
    name: &str,
    args: Option<Value>,
) -> Result<engine::response::OutputResult, UnknownTool> {
    if !matches!(
        name,
        "ik_catalog" | "ik_search" | "ik_read" | "ik_character" | "ik_guide" | "ik_coverage"
    ) {
        return Err(UnknownTool);
    }
    let args = args.filter(|v| !v.is_null()).unwrap_or_else(|| json!({}));
    if let Err(error) = check_arguments(name, &args) {
        return Ok(Err(error));
    }
    let arguments = engine::cursor::resolve(corpus, name, args);
    Ok(arguments.and_then(|args| {
        let result = match name {
            "ik_catalog" => parse(Some(&args)).and_then(|a| engine::catalog::execute(corpus, &a)),
            "ik_search" => parse(Some(&args)).and_then(|a| engine::search::execute(corpus, &a)),
            "ik_read" => parse(Some(&args)).and_then(|a| engine::read::execute(corpus, &a)),
            "ik_character" => {
                parse(Some(&args)).and_then(|a| engine::character::execute(corpus, &a))
            }
            "ik_coverage" => parse(Some(&args)).and_then(|a| engine::coverage::execute(corpus, &a)),
            _ => parse(Some(&args)).and_then(|a: engine::guide::GuideArgs| {
                engine::guide::run(&a).map(|mut text| {
                    if let Some(g) = engine::guide::find(&a.mode) {
                        text.push_str(&advanced_parameters(g.mode));
                    }
                    engine::response::ToolOutput::new(
                        corpus,
                        text.clone(),
                        engine::response::ToolData::Guide(engine::response::GuideResult {
                            mode: engine::guide::find(&a.mode)
                                .map(|g| g.mode)
                                .unwrap_or_default()
                                .into(),
                            text,
                        }),
                    )
                })
            }),
        };
        result.map_err(|error| engine::suggestions::append(corpus, name, &args, error))
    }))
}
