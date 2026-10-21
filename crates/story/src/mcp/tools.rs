//! 工具清单（JSON Schema）与调用分发。

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::corpus::Corpus;
use crate::engine::{self, ToolError, ToolResult};

fn read_only() -> Value {
    json!({ "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false })
}

pub fn definitions() -> Value {
    json!({ "tools": [
        {
            "name": "ik_catalog",
            "title": "绝区零剧情目录",
            "description": "浏览绝区零剧情库目录。不带参数：列出全部主线章节（游戏顺序，含场景数/台词数/字数）与设定资料分类。\
    path=章节或分类 id（如 ch:Chapter110、lore:agents，也可写章节名「云霞同归处」）：按目录顺序列出该组全部文档，\
    主线按内容类型与可解析的任务分节，给出 docId、句数、逐句署名、独立的 TextMap 姓名表与证据缺口；姓名表不证明实际出场。总结一章前先规划阅读范围。目录相邻不代表剧情先后。\
    path=docId：查看该篇所在位置与前后篇。keyword：按标题/路径筛选。已确认版本别名（3.2/3.3）可作 path 或单独的 keyword，定位该版新增主线；与整份语料快照版本不同。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "分组 id（ch:Chapter110 / lore:agents）、章节名、版本别名（如 3.3版本），或 docId；留空为根目录。" },
                    "keyword": { "type": "string", "description": "可选，只列出标题或路径包含该词的文档；path 留空时在全库标题中查找。" },
                    "offset": { "type": "integer", "minimum": 0, "default": 0, "description": "分页起点（跳过前 N 篇）。" },
                    "maxResults": { "type": "integer", "minimum": 1, "maximum": 500, "default": 100, "description": "最多列出多少篇。" },
                    "includeEvidence": { "type": "boolean", "default": false, "description": "展开每篇前20行的证据摘要与 sourceKey/lineId；完整逐行证据用 ik_read 分页。" }
                },
                "additionalProperties": false
            },
            "annotations": read_only()
        },
        {
            "name": "ik_search",
            "title": "绝区零剧情检索",
            "description": "绝区零（Zenless Zone Zero）游戏文本全文检索：主线剧情台词、过场字幕/漫画、世界观设定（代理人档案、音擎/驱动盘/邦布、\
    绳网委托与私信、录像店影片、街区闲聊）。涉及剧情、人物、阵营、地点、事件、台词出处时先用它找候选，再用 ik_read 读原文。\
    多个关键词用空格分隔，先检索同一篇内全部出现（AND，不要求同一句）；没有完整匹配时默认返回明确标注的部分匹配，按覆盖词数排序并列出缺失词。allowPartial=false 要求严格匹配。\
    已确认版本别名（3.2/3.3，兼容 v3.3、3.3.0、3.3版本）可作 path 或独立关键词，限定该版新增主线；部分匹配不会放宽章节、scope、speaker 或 docId。\
    正文或逐句署名结果优先显示；仅标题/路径/姓名表命中独立为目录建议，不占正文配额。逐词说明正文、说话人、身份、标题、路径及姓名表命中；matchIn=body 要求正文共现。\
    提供 docId 时改为篇内检索。保留占位、推断署名与未知候选；资源关联不确认实际播放。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "keyword": { "type": "string", "description": "关键词，空格分隔多个词；可写 3.3版本 核心，版本词限定章节。只按说话人筛选时可留空。" },
                    "scope": { "type": "string", "enum": ["all", "story", "supplement", "lore"], "default": "all", "description": "范围：all 全部 / story 主线 / supplement 过场与漫画 / lore 设定资料。" },
                    "path": { "type": "string", "description": "可选，限定某章或设定分类（ch:Chapter110 / lore:agents / 章节名）；可用已确认版本别名，如 3.3。" },
                    "docId": { "type": "string", "description": "可选，篇内检索：只在这一篇里找，返回全部命中行。" },
                    "speaker": { "type": "string", "description": "可选，只看这个说话人的台词（如「妮可」「哲」）；已记录角色身份的服装/形态会合并匹配，原文显示名保留。" },
                    "matchIn": { "type": "string", "enum": ["all", "body"], "default": "all", "description": "all：正文及元数据可参与同篇AND；body：仅正文，不把姓名/标题/路径算为正文共现。" },
                    "includeEvidence": { "type": "boolean", "default": false, "description": "展开命中行证据摘要；目录建议每篇最多20行。" },
                    "directoryOffset": { "type": "integer", "minimum": 0, "default": 0, "description": "仅元数据目录建议的独立分页起点，须保留原查询与筛选条件。" },
                    "maxDirectoryResults": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10, "description": "目录建议配额，独立于 maxResults。" },
                    "maxResults": { "type": "integer", "minimum": 1, "maximum": 500, "description": "最多返回多少篇（默认 10，上限 50）；篇内检索时为最多多少行（默认 50）。" },
                    "hitsPerDoc": { "type": "integer", "minimum": 1, "maximum": 20, "default": 3, "description": "每篇展示的命中行数。" },
                    "allowPartial": { "type": "boolean", "default": true, "description": "无同篇完整匹配时返回部分匹配，标出缺失词；设 false 要求严格 AND。全库和篇内检索都适用，筛选范围保持不变。" },
                    "contextLines": { "type": "integer", "minimum": 0, "maximum": 3, "description": "全库检索每个命中前后展示几行同篇上下文，合并去重并按行号排列。默认 1；有 speaker 筛选时默认 0。上下文可以含其他说话人。" }
                },
                "additionalProperties": false
            },
            "annotations": read_only()
        },
        {
            "name": "ik_read",
            "title": "读取剧情原文",
            "description": "读取绝区零剧情/设定原文。docId 必须原样使用 ik_search / ik_catalog 返回的 doc_…；\
    也可以传章节/分类 id（如 ch:Chapter110）从该组第一篇开始顺读。每行格式「L行号 说话人：台词」，【选项】为玩家选项。\
    按 offset/limit 分页并受 maxChars 字数预算约束；follow=true 时读完一篇自动接着读同章目录下一篇，适合整章覆盖阅读，不能据此推定剧情时间线。\
    结尾总会给出精确的续读调用，照抄即可继续。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "docId": { "type": "string", "description": "doc_…、分组 id（ch:Chapter110 / lore:agents），或已确认版本别名（如 3.3版本）。" },
                    "offset": { "type": "integer", "minimum": 1, "default": 1, "description": "起始行号（1 起）。" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 5000, "default": 2000, "description": "本次最多读多少行（通常由 maxChars 先生效）。" },
                    "maxChars": { "type": "integer", "minimum": 500, "maximum": 60000, "default": 16000, "description": "标题、来源与正文的字数预算；首行过长时仍输出一行以推进游标，末尾续读提示不计入。" },
                    "follow": { "type": "boolean", "description": "读完本篇后是否接着读同组下一篇；docId 为分组 id 时默认 true，否则默认 false。" },
                    "lineNumbers": { "type": "boolean", "default": true, "description": "是否带行号（引用出处需要行号）。" },
                    "includeSources": { "type": "boolean", "default": false, "description": "附上原始 sourceKey 与稳定 lineId，便于回查提取器；旧包缺少字段时省略。" },
                    "includeEvidence": { "type": "boolean", "default": false, "description": "逐行附内容、署名、配音、局部顺序与Perform/Timeline回查摘要，同时包含sourceKey/lineId。缺省或未知状态保持原样，不确认播放。" }
                },
                "required": ["docId"],
                "additionalProperties": false
            },
            "annotations": read_only()
        },
        {
            "name": "ik_character",
            "title": "角色速览",
            "description": "绝区零角色速览：设定档案 docId、按章节统计逐句署名场景/台词/被提及次数、姓名表关联及推断署名独立统计，后两者不算实际台词或首次登场。已知服装/形态按语料身份合并，目录首见不证明实际首次登场。\
    写角色小传、角色弧线或判断某角色在哪些章节有戏份时先用它，再按给出的 docId 用 ik_read 读原文。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "角色名（如「妮可」「星见雅」「零号·安比」）。" },
                    "maxResults": { "type": "integer", "minimum": 1, "maximum": 200, "default": 40, "description": "最多列出多少章及多少关联入口。" },
                    "includeEvidence": { "type": "boolean", "default": false, "description": "展开逐句署名与姓名表/推断关联入口，每篇前20行证据摘要；更多用ik_read分页。" }
                },
                "required": ["name"],
                "additionalProperties": false
            },
            "annotations": read_only()
        },
        {
            "name": "ik_guide",
            "title": "任务方法指引",
            "description": "读取剧情类任务的方法指引：summary（章节/任务剧情总结）、character（角色弧线）、timeline（事件时间线）、verify（原文查证）。\
    开始一项较大的总结或考据任务前读一次对应指引；同一任务不要反复读取。闲聊不需要。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mode": { "type": "string", "enum": ["summary", "character", "timeline", "verify"], "description": "指引模式。" }
                },
                "required": ["mode"],
                "additionalProperties": false
            },
            "annotations": read_only()
        }
    ]})
}

/// 未知工具（协议错误，而非工具执行错误）。
pub struct UnknownTool;

fn parse<T: DeserializeOwned + Default>(args: Option<Value>) -> Result<T, ToolError> {
    match args {
        None | Some(Value::Null) => Ok(T::default()),
        Some(v) => serde_json::from_value(v).map_err(|e| ToolError(format!("参数错误：{e}"))),
    }
}

pub fn call(corpus: &Corpus, name: &str, args: Option<Value>) -> Result<ToolResult, UnknownTool> {
    let result = match name {
        "ik_catalog" => parse(args).and_then(|a| engine::catalog::run(corpus, &a)),
        "ik_search" => parse(args).and_then(|a| engine::search::run(corpus, &a)),
        "ik_read" => parse(args).and_then(|a| engine::read::run(corpus, &a)),
        "ik_character" => parse(args).and_then(|a| engine::character::run(corpus, &a)),
        "ik_guide" => parse(args).and_then(|a| engine::guide::run(&a)),
        _ => return Err(UnknownTool),
    };
    Ok(result)
}
