# 剧情库检索与 MCP 工具

本文说明当前六个只读工具的检索、证据、分页与资料覆盖契约。服务部署与访问配置见
[部署说明](../deploy/README.md#剧情库mcp)。

## 工具与模型的职责

MCP 提供资料、参数语义、分页入口与来源状态。模型自行决定工具调用顺序、阅读范围、分析方法和回答形式。
初始化的 `instructions` 只介绍知识库范围、工具能力与当前提取模式；工具描述只说明输入输出及数据含义。
来源标识、未知署名、占位说明、未关联片段和顺序状态随资料返回。

`ik_guide` 保留原工具名及 `summary` / `character` / `timeline` / `verify` 模式，返回各主题的参数与结果字段参考。
它不再包含角色设定、强制搜索/阅读流程、分层压缩、引用格式或最终回答模板。
MCP prompts 保留 `chapter_summary` / `character_arc` / `event_timeline` / `fact_check` 名称及原参数，
仅返回用户选用的任务文本，不自动附加 guide 内容。

`mcp_contract_leaves_workflow_and_answer_format_to_client` 与 `mcp_prompts_only_contain_the_selected_task`
回归检查初始化、工具描述、guide、工具返回文本与 prompts 的这项边界。

## 检索与版本别名

多词检索先在同一篇文档中找全词匹配（AND）；词可分布在不同行、标题、路径或姓名元数据中。
无完整匹配时默认返回部分匹配，优先按正文词覆盖、有效署名覆盖排序，再比较总覆盖与相关度，逐篇列出缺失词。
`allowPartial=false` 保持严格 AND。未显式指定 `allowPartial` 时，若有词在当前范围内零命中则不退化为并集（那只等于单独检索其余词），
改为页眉说明并给出去掉该词的建议调用；显式 `allowPartial=true` 仍按要求返回部分匹配。部分匹配不能证明全部词在同篇共现，也不提供概念同义词推理。
回退始终保留 `path`、`scope`、`speaker`、`docId` 和版本限定，不会通过移除范围来扩大命中。
检索词超过 8 个时提示截断；版本选择器先解析为范围，不占检索词配额。
当前范围内零命中的词会再按同一 `matchIn`、不带范围筛选查一次全库：范围外仍有时写「当前范围内无命中：X（全库 N 篇）」并给出去掉范围筛选的建议调用，
全库也没有时写「全库无此词：X」；`structuredContent.missingTermCounts` 给出对应篇数。

```text
ik_search(keyword="天空 核心 秘密", path="3.3版本")
ik_search(keyword="3.3版本 核心", allowPartial=false)
ik_catalog(path="v3.3")
ik_catalog(keyword="3.3.0")
ik_read(docId="3.3版本", limit=20)
```

版本别名只定位该版本新增的主线章节，语料快照还包含历史章节、补充和设定。
目录、顺读和检索返回别名对应的章节与快照版本；目录根表列出已确认的版本别名。
只搜索版本词时列出该章候选场景，不把别名当成原文关键词命中。

| 版本 | 新增主线分组 | 章节标题 | 核查依据 |
| --- | --- | --- | --- |
| 3.2 | ch:Chapter230 | 第三季 第三章 她与她的隐秘往事 | 提取器 `docs/review-2026-10-21.md` 的本机 3.2 快照及 Chapter230 记录 |
| 3.3 | ch:Chapter240 | 第三季 第四章 重返天空的旅程 | 提取器 `MODLOG.md` 的 3.2/3.3 来源核验与新增记录对比：新增均属 Chapter240 |

接受完整版本表达式 `3.3`、`3.3.0`、`v3.3`、`v3.3.0`、`3.3版本`、`版本 3.3` 和全角写法。
检索关键词与目录标题筛选里，未核验的裸小数（如 `1.5`、`2.0`）按普通文本处理；
带 `版本` / `版` / `v` 等显式标记的未核验版本（如 `1.5版本`、`v1.5`）仍然报错，不会退化为文本检索。
检索的版本词须独立出现，可在其他检索词之前或之后；`3.3%` 等文字仍按原文检索。
多个不同版本、与 path/docId 冲突的章节、当前语料缺失的已知章节均明确报错。
未知版本或补丁没有映射，不按季/章编号及迭代周期猜测；调用者可以改用章节精确 id。

映射维护在 `crates/story/src/engine/releases.rs`。补充新版本须先核查来源、再加入映射，
不能直接把语料 `source_version` 赋给所有章节。此表目前覆盖两个已核对版本，未声称覆盖历代发布。

## 证据与命中分类

默认 `matchIn=all` 兼容原来的同篇 AND：正文、逐句说话人、身份、标题、路径、姓名表可参与匹配；
结果逐词说明字段，正文缺失词单列。`matchIn=body` 只让正文参与 AND 与部分回退，仍保留原筛选条件。
完整匹配优先；无完整匹配时优先比较正文覆盖，再比较有效署名覆盖、总覆盖及 BM25，稳定 docId 兜底。

正文或逐句署名命中使用 `maxResults`；仅元数据命中放入独立目录建议，用 `directoryOffset`（0 起）
和 `maxDirectoryResults`（默认10，上限50）分页，不占正文配额。目录建议返回精确读取入口及未显示数量，
续查保留全部参数。快照绑定 cursor 与 structuredContent 见下方。

`catalog/search/read/character` 默认报告影响理解的证据缺口；`includeEvidence=true` 展开内容状态、
原始资源署名、署名状态/来源/缺口/槽位、配音关联、局部行序、Perform 节/节点/资源路径与 Timeline 引用摘要。
目录及角色入口最多展开每篇前20行，其余经 `ik_read` 分页；读取续接保留证据偏好与既有参数。
`sourceKey` / `lineId` 可回查语料包中的完整资源 JSON。
占位显示为 `【占位说明】`，未知候选不改为旁白，推断署名独立标注。TextMap 姓名表、推断署名与实际台词分开统计。

工具输出提供 `corpusRevision`：摘要包含规范化有效语料、稳定身份/行序、证据、影响结果的 manifest 字段、
查询排序契约及发布映射；构建/提取时间、JSON 空白变化不影响它。证据或筛选模式变化会改变摘要。
旧包缺失字段仍加载并明确提示未提供核验证据；未知状态字面值保留，详细证据可查看。
提取清单用 `chapter_order_scale=10` 声明章节顺序的整数比例；旧小数按全包统一迁移，显式新比例拒绝小数。

## 结构化响应与分页

六个工具保留 MCP text content，并提供生成自 Rust 类型的 outputSchema 和类型化 structuredContent。
共同字段为 `schemaVersion`、`corpusRevision`、`snapshotVersion`、`extractionMode` 和工具数据 `data`。

MCP 默认 `detail="compact"`，需要展开解释时用 `full`，逐行证据由 `includeEvidence=true` 控制。CLI 的旧 `tools::call` 默认保留详细文本。客户端给模型提供一种主要结果表示，避免把 text 与 structuredContent 中同一批原文重复塞入上下文；具体 token 成本取决于客户端。游标是校验过的只读续接参数，不是访问凭证，鉴权仍由 HTTP 层处理。

```text
ik_search(keyword="核心", path="3.3版本", purpose="locate")
ik_search(keyword="天空 核心", path="3.3版本", purpose="verify", matchUnit="window", windowLines=12)
ik_catalog(path="3.3版本", manifest=true)
ik_read(docIds=["返回的docId1", "返回的docId2"], maxChars=16000)
```

- `purpose=browse` 保持原检索行为；`locate` 默认每篇2个命中及1行邻行；`verify` 默认 `matchIn=body`、严格AND及2行邻行。显式参数优先。
- 正文 `nextCall` 与目录 `nextDirectoryCall` 独立。只传返回的 `arguments={cursor:…}`；游标保留查询、范围、分页大小与偏好。损坏、超长、参数冲突、错工具或换快照均提示重查。
- `matchUnit=window` 在1～100行的文档行距内求最小全词覆盖区间；无完整窗口时按原范围返回明确的部分窗口。展示邻行不会补足窗口条件，`orderSource/localOrderStatus`保留来源依据，`orderStatus=playback_unverified`；跨文档关系仍为未知。
- 篇内搜索页按命中行/窗口分页；`isContext` 区分本页邻行。搜索原文超过180字时截为片段并设 `complete=false`，`matches.linesTruncated` 报告行号列表截断，`windowsTruncated/nextMatches`标出未展示窗口并给出篇内分页入口；搜索片段不产出coverageDelta。
- `docIds` 每次最多16个实际文档ID，与 `docId/follow=true` 互斥，共用读取预算；分块保留文档身份。读取 `complete` 表示本次指定续读目标已到末尾，不证明章首或过去的请求已覆盖。
- catalog 的 `manifest` 固定范围与整篇排除选项；只有全部行均为占位/未关联候选的文档才会被对应选项排除，混合文档保留。总行数、排除数、占位和候选计数分别报告，状态计数可能交叉。
- 角色 `chapterCount/chaptersTruncated`、文档 `evidencePreviewTruncated`明确展示上限；更多实际署名结果使用角色返回的搜索入口。
- 客户端累计 read 的 `coverageDelta`，提交 `ik_coverage(manifestId=…, ranges=…)`，返回合并 `ledger`、完整/部分/未读统计和缺口 `pending[].nextRead`。最多512个输入区间，缺口文档按20篇默认页、最多100篇分页；更大任务先合并记录或拆分范围。缺口 `missing` 使用独立类型，不会伪装成已返回回执。

查询与阅读覆盖核对不使用数据库会话；HTTP 接口使用动态 JSON-RPC 请求契约。

## 默认输出与章节导航

六个工具的成功compact文本现在统一带一次快照头：`语料=3.3.0 | 模式=default | corpusRevision=<完整值>`。
版本缺失显示“未声明”；full已有完整快照头的工具保留旧格式。结构化快照字段仍保留，公共新增字段均为附加字段，`schemaVersion=1`不变。
这些行为适用于所有MCP客户端，不按AstrBot、Gemini或其他客户端提供专用分支；客户端是否把文本或
structuredContent交给模型，以及是否截断大结果，仍须在实际客户端验证。

catalog/search/read新增`rangeInfo`，区分原请求`selector`、实际`groupIds`、`selectionMode`和别名的
`releaseVersion`；发布别名使用`selectionMode=addedMainStory`，只表示该版新增主线，既有`resolvedRange`
语义不变。根目录展示已核验的`releaseAliases`及当前收录状态，`manifest=true`时展示`manifestId`。
full手动续读使用返回的具体docId；快照cursor保存原别名及读取偏好，因此续页仍可说明最初请求的发布范围。

公共文档卡增加整篇`order`、`evidenceSummary`、`speakerSummary`和`speakerCoverage`。
篇序来源、行序来源与计数、局部核验状态、剧情先后和上下文关联分别保留；混合与未知状态不会取首行代替。
署名摘要compact最多展示6个逐句姓名，推断名单独立，full展开完整名单；旧包缺少核验证据单列。
搜索命中窗口的局部状态与整篇摘要采用不同范围，不以窗口代表整篇。

署名覆盖提供总行数、有名字行数及“resolved、inferred、旧包未声明状态、其他/未知状态、无名字”五类互斥计数。
有名字率的分母是所选范围的全部行，包含占位、选项和旁白；根目录明确标记为主线全行署名率。
设定及零分母显示不适用。证据缺口可与署名分类交叉，不能直接相加，也不把有名字率称为对白准确率或阅读覆盖率。

单篇读取结束后可返回`nextDocument`及`nextRead`，关系标作`catalogAdjacent`，只提供同组目录相邻入口。
`complete=true`仍表示当前指定续读目标已到末尾，可能同时存在下一篇；整章是否读全仍由`ik_coverage`核对。
未读完、follow和batch使用各自`nextCall`，读取预算统一计入实际快照头、整篇摘要及正文；首行过长仍完整返回以保证推进。
每块文本提供docId和`coverageDelta`区间，目录与coverage提供清单标识及缺口入口。
续读保留显式行数、字数、行号、来源、证据、detail 和 follow 参数，末尾续读提示不计入正文预算。

无效或歧义的章节选择器保持`isError=true`，给至多5个确定排序的候选及可执行重试；重试只替换错误范围，保留其他筛选。
裸`ch:`/`lore:`说明缺少名称并给目录入口；未知版本只列已核验映射；损坏或过期cursor沿用重查错误。
超大参数的重试文本有长度上限，超限时保留候选ID并提示缩短查询，不把模糊候选自动当作成功范围。

未限定内容类型的根目录增加`chapterRelations`：当前标签相同的组标记`sharedLabel/labelOnly`，
已核查的补充与间章归属标记`reviewedExtractorMapping`并保留提取器来源。
这些是导航线索；原始groupId/docId、精确选择器和目录成员不合并，同名或归属关系不证明剧情先后。
关系只展示当前可见成员，范围筛选和整篇排除不额外暴露其他分组。
已核查归属的上游提交及资源位置保存在 [章节关联映射](../crates/story/src/engine/chapter_relations.rs) 中。

## 验证与边界

发布映射目前只核验 3.2/3.3。扩充发布目录、跨文档播放顺序、章节等价与可选聚合入口仍需来源证据。
客户端负责保存 cursor、manifestId 与 coverageDelta，并累计 ledger；真实客户端的上下文注入、模型输入
token、回答质量与 A/B 效果尚未验证。

服务端同时返回 text 与 structuredContent，默认文档摘要会增加完整 RPC 的传输和 JSON 解析成本。
已有本地测量未统一达到“所有查询 p95 增幅不超过 20%”与“相同预算下整章调用次数不增加”的原定目标，
不能据 compact 文本长度推断整体提速或模型 token 节省。重新测量时应固定语料、构建模式、机器、参数与
测量入口，分别记录文本、完整 RPC、工具定义、任务调用次数及模型实际用量。

相关回归位于 [剧情库测试](../crates/story/tests/)，覆盖新旧语料、证据与未知状态、检索范围、快照游标、
窗口、读取预算、资料覆盖、章节纠错与生成 schema。本地按 [开发说明](dev-setup.md) 只运行受影响用例；
完整测试与 clippy 由 CI 执行。

[工作流示例](../crates/story/examples/usability.rs) 可测量实际语料的查询响应与整章读取，
[schema 校验脚本](../crates/story/tests/validate_schemas.py) 使用 Python 的 `jsonschema` 包核对生成 schema 与响应。
该示例固定查询 3.3 新增主线与 Fairy，需要包含 `ch:Chapter240` 的语料，并调用 Windows PowerShell
采样内存。在仓库根目录运行下列命令，将占位路径替换为实际绝对路径；报告包含原文，应保存在仓库外：

```powershell
$env:CARGO_INCREMENTAL = '0'
$env:STORY_SCHEMA_REPORT = '<仓库外的schema报告.json>'
cargo nextest run -p ik-story -E 'test(generated_schemas_are_self_contained_and_match_results)'
cargo run -p ik-story --example usability -- '<包含3.3新增主线的语料目录或包>' '<仓库外的工作流报告.json>'
python crates/story/tests/validate_schemas.py '<仓库外的schema报告.json>' '<仓库外的工作流报告.json>'
```
