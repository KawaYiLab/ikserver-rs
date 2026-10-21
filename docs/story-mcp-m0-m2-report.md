# 剧情 MCP 首批实施与核验（M0～M2）

日期：2026-10-22。首批实现已完成；M3～M7 继续按 [实施计划](story-mcp-improvement-plan.md) 推进。
本轮未上传语料、修改提取器或执行线上操作。代码改动限于 story 域，未改变数据库/HTTP接口结构。

## 固定输入与身份契约

实际存在的输入目录：`C:/Users/Administrator/Documents/GitHub/zzz-story-extractor/out/zzz_story_3.3.0_chs`。
manifest 为 `source_version=3.3.0`、`local_game_version=CNPRODWin3.3.0`、`language=CHS`、
`accuracy_first=false`、`generated_at=2026-10-22T00:41:10`。
加载结果为 8,649 篇、155,708 行、12,553,056 字节文本；包含历史主线、补充与设定。

SHA-256 为 `8a48142a277c31d9a889ce412618e009f0aa19b47f2bd501fe32d105ebdec9ba`，
对象是 `serde_json::to_vec(Bundle::from_dir(...))`，包含 manifest、原始JSONL和按路径排序的 lore。
这用于固定输入包；与忽略无关时间和JSON空白的 `corpusRevision` 区分。
本轮结果摘要为 `corpus_v1_75bb462224345c6bb322af8862f7d3268822bfe6439d566342c4074f5ffd23da`。

docId 继续使用既有 `story|chapter|family|scene` / `sup|...` / lore 键的48位哈希。
`sourceKey` / `lineId` 原样保留，不按相同文本删除剧情候选。`offset` 保持1基，目录分页保持0基；
`limit/maxChars/follow/lineNumbers/includeSources` 继续生效，新证据偏好参与读取续接。

`corpusRevision` 的 v1 摘要包括：有效文档及分组排列、文本和稳定来源、署名及身份归一、
证据摘要、source/local版本、语言、提取模式、查询排序契约与现有发布映射。
不包括 built_at/generated_at、构建路径及JSON空白。gzip包与目录加载共用构建器。
排序、映射解析或证据解释变化须更新查询契约；M3 游标将绑定该摘要。

## 已交付行为

- 内容、署名、配音、局部行序和资源依据进入独立证据表；相同证据去重，重复字符串共享。
  Perform 保留资源路径、节/节点/字段及局部时间；Timeline 保留引用名、text_key、avatar、voice_keys和文本差异。
  完整资源JSON仍经 sourceKey/lineId 回查输入包。
- 占位为独立类型，资源中的安比署名只作为 `source_attribution`，不转成正式对白。
  未知候选、推断署名及未知状态保持明确；`resolved` 不代表实际播放核实。
- 角色的逐句署名、推断署名、TextMap姓名表分别统计，后两项不产生实际台词或首见出场。
  旧包有署名但缺状态时保留原显示名并报告证据缺失；隐藏名不补写。
- 搜索按正文、逐句说话人、角色身份、标题、章节/路径、姓名表、推断署名逐词报告匹配。
  `matchIn=all` 保留同篇元数据AND；`matchIn=body` 只匹配正文。严格/部分回退均保留范围。
- 正文/逐句署名与仅元数据目录建议独立配额；目录建议用 `directoryOffset/maxDirectoryResults`
  分页，报告未显示数量和保留全部参数的续查JSON。当前仍是文本响应，快照绑定cursor属于M3。
- 四个资料工具默认提示缺口；`includeEvidence=true` 展开证据。目录/角色每篇最多20行，
  完整证据通过read分页获取；read字数预算包含证据及来源。

## 真实查询核验

“天空 核心 秘密”限定3.3：候选257篇、正文/逐句署名13篇、目录建议244篇、命中18行，
明确标为部分匹配并逐篇说明正文与元数据词。它不再把257篇描述为正文结果。
“3.3版本 核心”仍为4篇、5行；目录和顺读仍解析Chapter240。

| 范围 | 稳定docId | 保留行数 | 核验 |
| --- | --- | --- | --- |
| 第32幕 | doc_4d4e6a7e32ae | 3 | 3条占位，资源署名在证据里 |
| 第33幕 | doc_0adfacfaa595 | 1 | 1条占位 |
| 第34幕 | doc_4c777317dcde | 1 | 1条占位 |
| 第36幕 | doc_6ec2367cef5f | 9 | 未知署名、姓名表、无结构化关联 |
| 第37幕 | doc_eceac8a603dd | 17 | 同上，保留铃/哲称呼版本 |

36～37幕合计26行，输入中的5组相邻同文均保留；两个称呼版本各有一处。
真实输出含 sourceKey/lineId 和证据，报告保存于提取器本机 `out/story_mcp_m0_before.json`
及 `out/story_mcp_m0_m2_after.json`，不把整章游戏原文提交到本仓库。

## 性能观察与尚未通过的目标

同一输入，Windows GNU Rust 1.99.0，dev构建，逐查询25次取p50/p95。
基线构建4.991秒，最终构建10.826秒；Windows WorkingSet64从153.008 MiB增至183.453 MiB（+19.9%）。
这是进程驻留快照，包含分配器保留空间，不是精确堆大小；同机重测有抖动。

| 查询 | 基线p50/p95 ms | 本轮p50/p95 ms | 基线/本轮响应字节 |
| --- | --- | --- | --- |
| 天空 核心 秘密（3.3） | 2.673 / 2.742 | 3.436 / 3.834 | 8597 / 21849 |
| 3.3版本 核心 | 0.871 / 0.890 | 0.904 / 1.142 | 3106 / 5704 |
| 天空严格查询（3.3） | 0.944 / 0.977 | 1.066 / 1.262 | 5578 / 11493 |
| Fairy（3.3） | 1.020 / 1.047 | 1.246 / 1.732 | 8819 / 16895 |
| 单字核（3.3） | 2.202 / 2.252 | 2.209 / 2.730 | 9409 / 16958 |
| 3.3目录（100篇） | 0.117 / 0.128 | 0.222 / 0.249 | 9483 / 26190 |
| 3.3顺读20行 | 0.013 / 0.020 | 0.016 / 0.026 | 2713 / 3160 |

内存已用共享字符串、可选JSON装箱、证据去重、提前释放草稿降低；它接近20%边界。
目录逐篇缺口与姓名表说明、逐词字段及来源让响应变大，增加序列化与格式化；
导入证据、去重序列化和内容摘要使启动构建更慢。p95增幅仍未达到统一20%目标，**不算M7性能验收通过**，
也没有修改目标。后续应在M3统一类型化响应时减少重复文本格式化，增加优化构建的重复测量，
并补标题独有查询、全章与长游标场景。服务保持启动/换语料时一次构建，查询仍为毫秒级；发布前需重新验收。

## 验证与复现

9项隔离改进测试、31项现有端到端测试通过；真实语料测量测试显式运行通过。
两条缺陷测试先在修复前失败，修复后通过。覆盖字段分类、正文AND、版本/scope/docId/speaker约束、
重复与称呼保留、未知状态、gzip重载与时间/空白稳定、证据分页、MCP JSON-RPC文本兼容和HTTP新参数分发。
`cargo check -p ik-story --all-targets`、`cargo fmt --all -- --check` 通过。
未跑工作区全量/数据库测试或完整clippy；未修改SQL，无需更新.sqlx。

本机没有nextest，使用相关 `cargo test -p ik-story --test improvements` 与 `--test e2e`。
本机缓存来自rsproxy，失效代理导致默认crates.io下载失败；验证采用临时命令行配置
`cargo --offline --config 'source.crates-io.replace-with="local-rsproxy"' --config 'source.local-rsproxy.registry="sparse+https://rsproxy.cn/index/"' ...`，未改仓库源配置。

真实测量：设 `CARGO_INCREMENTAL=0`、`STORY_BASELINE_SOURCE=<上述目录>`、
`STORY_BASELINE_REPORT=<本机报告路径>`，执行
`cargo test -p ik-story --test improvements real_corpus_baseline -- --ignored`。
固定输入包SHA必须一致，否则不作前后性能比较。M3～M7的结构化结果、游标、窗口、覆盖工具、
新增核验版本及完整发布验收仍待实施；线上运行版本尚未核对。
