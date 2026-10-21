# 剧情库检索与版本别名

后续六项改进的阶段、依赖和验收标准见 [剧情 MCP 改进实施计划](story-mcp-improvement-plan.md)。
M3～M5之后的默认输出补齐与章节纠错（U0～U3）已实现，契约及验证边界见 [调用体验优化实施报告](story-mcp-usability-optimization-report.md)；后续客户端与模型验收见 [调用体验优化方案](story-mcp-usability-optimization-plan.md)。

多词检索先在同一篇文档中找全词匹配（AND）；词可分布在不同行、标题、路径或姓名元数据中。
无完整匹配时默认返回部分匹配，优先按正文词覆盖、有效署名覆盖排序，再比较总覆盖与相关度，逐篇列出缺失词。
`allowPartial=false` 保持严格 AND。部分匹配不能证明全部词在同篇共现，也不提供概念同义词推理。
回退始终保留 `path`、`scope`、`speaker`、`docId` 和版本限定，不会通过移除范围来扩大命中。

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
检索的版本词须独立出现，可在其他检索词之前或之后；`3.3%` 等文字仍按原文检索。
多个不同版本、与 path/docId 冲突的章节、当前语料缺失的已知章节均明确报错。
未知版本或补丁没有映射，不按季/章编号及迭代周期猜测；调用者可以改用章节精确 id。

映射维护在 `crates/story/src/engine/releases.rs`。补充新版本须先核查来源、再加入映射，
不能直接把语料 `source_version` 赋给所有章节。此表目前覆盖两个已核对版本，未声称覆盖历代发布。

验证使用本地语料、MCP 工具分发和小型隔离语料测试，覆盖默认/严格匹配、分散于多行的全词匹配、
覆盖率排序、姓名与范围筛选、篇内查询、别名规范化、旧章隔离、冲突/缺失/未知映射及词数截断。

2026-10-22：31 项剧情库端到端测试通过，`ik-story --all-targets` 编译及格式检查通过。
本机未安装 nextest，使用 `cargo test -p ik-story --test e2e`；未运行工作区全量或数据库测试。
本地读取提取器当前 `out/zzz_story_3.3.0_chs` 的 3.3.0 语料，确认：
“天空 核心 秘密”在 3.3 章节严格匹配为 0，默认返回标注部分匹配的候选；
“3.3版本 核心”命中 4 篇、5 行；版本目录和顺读均解析为 Chapter240。
只搜索“3.3”列出该章 257 个候选场景，明确说明没有正文词匹配。
查询输出保存在提取器 `out/search_usability_3.3.0.json`，没有上传语料或部署服务。

## 首批改进（M0～M2）

2026-10-22 已实现证据传递与搜索分类，契约及实测见 [阶段报告](story-mcp-m0-m2-report.md)。
默认 `matchIn=all` 兼容原来的同篇 AND：正文、逐句说话人、身份、标题、路径、姓名表可参与匹配；
结果逐词说明字段，正文缺失词单列。`matchIn=body` 只让正文参与 AND 与部分回退，仍保留原筛选条件。
完整匹配优先；无完整匹配时优先比较正文覆盖，再比较有效署名覆盖、总覆盖及 BM25，稳定 docId 兜底。

正文或逐句署名命中使用 `maxResults`；仅元数据命中放入独立目录建议，用 `directoryOffset`（0 起）
和 `maxDirectoryResults`（默认10，上限50）分页，不占正文配额。目录建议返回精确读取入口及未显示数量，
续查保留全部参数。M3 已加入快照绑定 cursor 与 structuredContent，见下方。

`catalog/search/read/character` 默认报告影响理解的证据缺口；`includeEvidence=true` 展开内容状态、
原始资源署名、署名状态/来源/缺口/槽位、配音关联、局部行序、Perform 节/节点/资源路径与 Timeline 引用摘要。
目录及角色入口最多展开每篇前20行，其余经 `ik_read` 分页；读取续接保留证据偏好与既有参数。
`sourceKey` / `lineId` 可回查语料包中的完整资源 JSON。
占位显示为 `【占位说明】`，未知候选不改为旁白，推断署名独立标注。TextMap 姓名表、推断署名与实际台词分开统计。

工具输出提供 `corpusRevision`：摘要包含规范化有效语料、稳定身份/行序、证据、影响结果的 manifest 字段、
查询排序契约及发布映射；构建/提取时间、JSON 空白变化不影响它。证据或筛选模式变化会改变摘要。
旧包缺失字段仍加载并明确提示未提供核验证据；未知状态字面值保留，详细证据可查看。


## 模型调用体验改进（M3～M5）

2026-10-22 已加入类型化结构化响应、快照游标、篇内窗口、批量读取及覆盖核对，见 [实施与实测报告](story-mcp-m3-m5-report.md)。六个工具保留 MCP text content，并提供生成自 Rust 类型的 outputSchema。共同字段为 `schemaVersion`、`corpusRevision`、`snapshotVersion`、`extractionMode` 和工具数据 `data`。

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

本轮未增加数据库表或修改 HTTP 路由/DTO；OpenAPI 的既有动态 JSON-RPC 请求契约不变。版本目录扩充、带执行依据的跨文档关系和真实模型效果评测仍需后续资料与验证。

## 默认输出补齐与章节纠错（U0～U3）

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

无效或歧义的章节选择器保持`isError=true`，给至多5个确定排序的候选及可执行重试；重试只替换错误范围，保留其他筛选。
裸`ch:`/`lore:`说明缺少名称并给目录入口；未知版本只列已核验映射；损坏或过期cursor沿用重查错误。
超大参数的重试文本有长度上限，超限时保留候选ID并提示缩短查询，不把模糊候选自动当作成功范围。

未限定内容类型的根目录增加`chapterRelations`：当前标签相同的组标记`sharedLabel/labelOnly`，
已核查的补充与间章归属标记`reviewedExtractorMapping`并保留提取器来源。
这些是导航线索；原始groupId/docId、精确选择器和目录成员不合并，同名或归属关系不证明剧情先后。
关系只展示当前可见成员，范围筛选和整篇排除不额外暴露其他分组。版本目录扩充、可选聚合选择器和真实模型效果验收仍属后续工作。
