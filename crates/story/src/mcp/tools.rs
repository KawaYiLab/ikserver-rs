//! Model-facing tool contracts and dispatch.
use crate::corpus::Corpus;
use crate::engine::{self, ToolError, ToolResult};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
fn read_only() -> Value {
    json!({"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false})
}
pub fn definitions() -> Result<Value, serde_json::Error> {
    let cursor = json!({"type":"string","maxLength":16384,"description":"快照绑定的分页游标，包含原查询条件；与其他查询参数互斥。"});
    let detail = json!({"type":"string","enum":["compact","full"],"default":"compact","description":"compact 简明结果；full 展开说明。逐行证据由 includeEvidence 控制。"});
    let scope = json!({"type":"string","enum":["all","story","supplement","lore","event"]});
    let evidence = json!({"type":"boolean","default":false,"description":"展开逐行来源证据，未知状态保持未知。"});
    let integer =
        |min, max, default| json!({"type":"integer","minimum":min,"maximum":max,"default":default});
    let tools = vec![
        (
            "ik_catalog",
            "剧情目录",
            "返回章节、活动与支线、设定目录。path 支持分组ID、章节名、已核验版本别名或docId。manifest=true 返回固定范围清单；nextRead 为原文入口。目录相邻不表示剧情先后。",
            json!({"path":{"type":"string"},"keyword":{"type":"string","description":"标题/路径筛选；标题筛选与manifest=true互斥，path未指定时支持已核验版本别名。"},"offset":integer(0,usize::MAX,0),"maxResults":integer(1,500,100),"cursor":cursor,"detail":detail,"includeEvidence":evidence,"manifest":{"type":"boolean","default":false},"scope":scope,"excludePlaceholders":{"type":"boolean","default":false,"description":"仅排除整篇全部为占位的文档，混合文档保留。"},"excludeUnlinked":{"type":"boolean","default":false,"description":"仅排除整篇全部为未关联候选的文档，混合文档保留。"}}),
        ),
        (
            "ik_search",
            "剧情检索",
            "检索剧情与设定，空格分词，同篇AND；无完整匹配时，只有至少两词同篇出现才默认返回部分匹配（按命中词数排序，页眉报各词篇数）；有词零命中或没有任何两词同篇时不合并，suggestions 给出可直接使用的检索调用。回退保留范围。purpose=locate 定位出处，verify 默认正文严格匹配，browse 浏览。window 匹配表示文档行窗口内共现，不表示连续播放。正文与仅元数据目录建议分别分页，返回 nextCall、nextDirectoryCall 和原文入口 nextRead。",
            json!({"keyword":{"type":"string"},"scope":scope,"path":{"type":"string","description":"章节/分类/已核验版本别名；版本别名定位该版新增主线，区别于语料快照版本。"},"docId":{"type":"string"},"speaker":{"type":"string","description":"逐句署名；已记录身份变体合并。正文提及属于keyword匹配范围。"},"purpose":{"type":"string","enum":["browse","locate","verify"]},"matchIn":{"type":"string","enum":["all","body"],"description":"all允许正文及元数据参与同篇AND；body仅正文。"},"matchUnit":{"type":"string","enum":["doc","window"],"default":"doc"},"windowLines":integer(1,100,12),"includeEvidence":evidence,"detail":detail,"cursor":cursor,"offset":integer(0,usize::MAX,0),"directoryOffset":integer(0,usize::MAX,0),"maxDirectoryResults":integer(1,50,10),"maxResults":{"type":"integer","minimum":1,"maximum":500,"description":"全库文档页默认10、上限50；篇内命中行页默认50、上限500。"},"hitsPerDoc":integer(1,20,3),"allowPartial":{"type":"boolean","description":"默认true；verify默认false；显式值优先。未显式指定时，有词零命中或没有任何两词同篇则不合并。"},"contextLines":{"type":"integer","minimum":0,"maximum":3,"description":"展示邻行，不参与窗口匹配；默认1，speaker筛选默认0。"},"pagePart":{"type":"string","enum":["both","body","directory"],"description":"指定正文/目录分页部分；返回游标包含此值。"}}),
        ),
        (
            "ik_read",
            "读取原文",
            "按行数与字数预算返回原文，保留占位、未知与推断署名状态。docId 支持文档ID、分组或已核验版本别名；分组按目录顺读，目录顺序不表示剧情时间线。docIds 批量读取1～16篇，共用预算。nextCall 为续读入口；coverageDelta 记录本次完整返回行。",
            json!({"docId":{"type":"string"},"docIds":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"string"},"description":"批量文档ID；与docId/follow=true互斥。"},"offset":integer(1,usize::MAX,1),"limit":integer(1,5000,2000),"maxChars":integer(500,60000,16000),"follow":{"type":"boolean"},"lineNumbers":{"type":"boolean","default":true},"includeSources":{"type":"boolean","default":false},"includeEvidence":evidence,"cursor":cursor,"detail":detail,"batchIndex":integer(0,15,0)}),
        ),
        (
            "ik_character",
            "角色速览",
            "返回角色档案及章节统计，逐句署名、提及、姓名表和推断署名分别计数。已知身份变体合并，目录首见不表示实际首次登场。nextCalls 提供相关检索入口。purpose=roleplay 返回扮演参考资料：代理人档案的身份与性格小节原文、按幕抽取的档案对话原文（信赖邀约、街区日常、随行气泡），以及主线常对话对象与逐句署名台词样本（附前一句与原文入口）；until 将主线台词限定到某章节或已核验版本（含）。",
            json!({"name":{"type":"string"},"purpose":{"type":"string","enum":["overview","roleplay"],"default":"overview"},"until":{"type":"string","description":"仅roleplay：章节id/章节名/已核验版本别名，主线台词截止到该章（含）；档案不随之过滤。"},"maxResults":{"type":"integer","minimum":1,"maximum":200,"description":"overview为章节行数，默认40、上限200；roleplay为主线台词样本数，默认8、上限40。"},"maxScenes":{"type":"integer","minimum":1,"maximum":20,"description":"仅roleplay：档案对话段数，默认6；随行气泡整体算一段。"},"includeEvidence":evidence,"detail":detail}),
        ),
        (
            "ik_guide",
            "契约参考",
            "返回工具参数与结果字段的参考说明。summary 为章节资料，character 为角色资料，timeline 为顺序与时间，verify 为检索与出处。",
            json!({"mode":{"type":"string","enum":["summary","character","timeline","verify"]}}),
        ),
    ];
    let mut entries = Vec::new();
    for (name, title, description, properties) in tools {
        let mut schema =
            json!({"type":"object","properties":properties,"additionalProperties":false});
        match name {
            "ik_read" => {
                schema["anyOf"] =
                    json!([{"required":["docId"]},{"required":["docIds"]},{"required":["cursor"]}])
            }
            "ik_character" => schema["required"] = json!(["name"]),
            "ik_guide" => schema["required"] = json!(["mode"]),
            _ => {}
        }
        entries.push(json!({"name":name,"title":title,"description":description,"inputSchema":schema,"outputSchema":engine::response::output_schema(name)?,"annotations":read_only()}));
    }
    use utoipa::PartialSchema;
    entries.push(json!({"name":"ik_coverage","title":"资料覆盖核对","description":"对照 catalog.manifest 核对提交的 read.coverageDelta（每段可只填 docId/start/end），合并重复和交叠区间，局部读取不计整篇。返回 ledger、完整/部分/未读统计、缺口 nextRead 和分页 nextCall。统计依据仅为提交的返回行记录，不表示模型理解或服务器请求历史；服务端不保存 ledger。","inputSchema":{"type":"object","properties":{"manifestId":{"type":"string","maxLength":16384},"ranges":{"type":"array","maxItems":512,"items":engine::coverage::RangeInput::schema()},"offset":integer(0,usize::MAX,0),"maxResults":integer(1,100,20)},"required":["manifestId"],"additionalProperties":false},"outputSchema":engine::response::output_schema("ik_coverage")?,"annotations":read_only()}));
    Ok(json!({"tools":entries}))
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
    let arguments = engine::cursor::resolve(
        corpus,
        name,
        args.filter(|v| !v.is_null()).unwrap_or_else(|| json!({})),
    );
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
                engine::guide::run(&a).map(|text| {
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
