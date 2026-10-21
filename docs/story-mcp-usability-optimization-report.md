# 公共剧情 MCP 默认输出优化实施报告

日期：2026-10-22。U0/U1/U2、U3导航和公共响应兼容核验已实施并本地验证。默认调用已补齐版本、顺序、
署名缺口与续读入口；性能和调用次数目标未全部达到，真实模型与token验收未执行。所有客户端使用同一契约，
没有AstrBot或Gemini专用分支。

## 已实施

- 六工具成功响应都有一次真实快照摘要，保留完整corpusRevision、语料版本和提取模式。compact新增短头，
  full已有完整头的工具保留格式；头部由渲染路径明确指定，用户查询文字不能抑制真实头。
- catalog/search/read新增rangeInfo，发布别名明确表示该版新增主线和实际groupId；根目录恢复3.2/3.3
  已核验别名、当前收录状态及manifestId。范围筛选、空结果和游标续读保留实际范围。
- 公共文档卡新增order、evidenceSummary、speakerSummary、speakerCoverage。篇序、行序、局部核验、
  剧情先后、上下文关联分别表达；混合来源、未知状态、占位和缺口不改写为已确认。
- compact每篇摘要最多180个Unicode字符；逐句姓名预览最多6人，full展开名单和完整状态计数。
  极限长度下保留已知顺序来源与缺口标记，截短未知长状态并指向full；超大计数可显示明确下界，类型化计数始终精确。
  统计按整篇计算；搜索窗口另外说明局部范围。五类署名计数互斥，总和等于全行分母；缺口计数允许交叉。
  根目录默认为主线全行署名率，设定不适用；有名字不表示播放或对白准确性已核验。
- 单篇结束新增轻量nextDocument与可执行nextRead，明确catalogAdjacent。complete语义保持当前目标结束；
  batch只推进请求清单，章末不自动越章。预算计入实际头、摘要和正文，首行过长时仍完整返回以保证推进。
  read每块文本显示docId与完整返回区间，结构化coverageDelta保留原语义。
- 裸ch:/lore:、错字、歧义和未收录章节保持isError，并给至多5个稳定候选。重试保留其他参数和过滤；
  超16KiB的完整重试省略并提示缩短参数。未知版本只列已核验映射，坏cursor不做模糊恢复。
- 根目录新增chapterRelations：同名组仅标记sharedLabel/labelOnly；02/025、05/055的间章及028、052/058
  的补充归属有提取器来源。当前ID、标签及可见范围必须匹配才展示；不合并原groupId、不改变排序或清单。

摘要在加载时存入独立侧表，compact文本共享缓存；revision计算不包含展示缓存。目录避免非根页无用的分组统计，
full范围目录的重复摘要已修复。SQL、迁移、.sqlx、HTTP路由及DTO未改变；MCP outputSchema仍由Rust/utoipa生成，
schemaVersion保持1，新展示字段为可选附加字段。

## 验证

本地使用Rust 1.99.0 GNU，与CI固定版本一致；CARGO_INCREMENTAL=0。nextest未安装，使用对应cargo test。
依赖已缓存于rsproxy，命令级指定镜像后离线运行，不改仓库源配置。

相关库测试及chapter_relations、chapter_suggestions、compact_contract、compact_summaries、read_navigation、
usability、e2e、improvements共108项通过；一个需要真实语料的ignored基线用例由下述真实工作流验证补充。
新增bug均先确认回归失败再修复。覆盖了旧包/gzip、未知枚举、版本与过滤冲突、
身份和候选保留、游标篡改/跨快照、预算临界、批量及单篇导航、伪造快照行和超长纠错参数。
六份生成schema与19份记录响应通过Draft 2020-12标准校验，包含所有工具、full/compact、非空章节关系及完整署名。
cargo check -p ik-story --all-targets、格式及差异空白检查通过。工作区全量测试、clippy和发布CI未执行。

真实语料为提取器out/zzz_story_3.3.0_chs：8,649篇、155,708行、12,553,056正文UTF-8字节。
包SHA-256为8a48142a277c31d9a889ce412618e009f0aa19b47f2bd501fe32d105ebdec9ba。
与基线相同的revision为：

```text
corpus_v1_7f9b051811aacbdf865de27b04a17267f850191f9f1273e1489db06bc5fdc56d
```

七个真实查询的命中/候选/文档身份与基线一致。目录建议5次分页取得244篇，无重复；3.3章完整读取257篇、
2,149行，partial=0、unread=0，5条占位和44条未关联行均保留。展示与导航没有改变语料身份或检索范围。
最后的快照头和极限摘要修正后再次读取真实语料：七份compact RPC、full文本大小及完整工作流结果与测量版本相同。

## 成本与未达标项

使用与基线5691f5a相同的usability示例、dev构建和本机语料，连续三轮，每查询每轮25次，表中为三轮p95的中位数。
延迟包括工具计算、完整RPC序列化与JSON解析，不能把全部开销归于检索算法。原基线只有一次保存的工作流测量，
绝对值受机器噪声影响；响应字节和调用次数为确定性结果。详细数据见[指标](story-mcp-usability-optimization-metrics.json)。

| 固定调用 | p95基线→当前(ms) | text基线→当前(字节) | RPC基线→当前(字节) |
| --- | ---: | ---: | ---: |
| 天空 核心 秘密，3.3版本 | 7.265→7.390 | 9,998→14,851 | 37,478→58,514 |
| 3.3版本 核心，locate | 1.656→1.933 | 2,255→3,409 | 8,591→13,155 |
| 天空 核心，window verify | 0.272→0.352 | 713→876 | 1,796→2,083 |
| 核，3.3版本 | 3.935→5.602 | 8,384→10,804 | 30,830→41,435 |
| 3.3目录，100篇默认页 | 2.468→7.797 | 16,573→40,007 | 48,736→153,197 |
| Fairy角色 | 6.714→8.274 | 1,949→8,062 | 20,381→60,484 |
| 3.3顺读，20行 | 0.548→0.607 | 2,389→2,977 | 9,766→11,344 |

驻留内存抽样中位数115,150,848→123,105,280字节，增6.9%；该指标不是峰值。加载耗时10,151→9,529ms，
但不足以证明加载算法加速。tools/list定义25,853→42,093字节，新增schema字段也增加工具定义成本。

整章相同30,000字预算下，读取3→5次，累计读取RPC975,247→1,269,070字节（增30.1%）。新增每篇元数据和
准确计入实际头部占用预算，不能靠超预算或隐藏缺口维持旧次数。因此“调用不增加”和“所有p95增幅不超过20%”
均未通过；内存抽样增幅目标通过。未实测模型输入token，不能声称节省token或M7验收完成。

最大增量来自100篇目录完整结构化摘要：order、署名名单、证据和覆盖计数约增加74KB，加上默认文本摘要。
缓存消除了每次重扫原文和反复渲染，但不能消除完整响应的传输与JSON解析成本。后续可评估内部共享对象减少分配，
以及经公共契约设计的按需展开；任何调整仍须保持默认关键状态可见，不能再次删去溯源或不确定性。

## 边界与复现

04/040、05/050等同名编号仅建立导航关联，章节等价及可选聚合入口尚需证据。已有extractor归属依据为
[f1b85ab的extract.py](https://github.com/KawaYiLab/zzz-story-extractor/blob/f1b85abfb4efc19018526ca72107d8c09d2c84a0/zzz_story/extract.py#L127)
及chapters.py标签；这些来源不证明跨组剧情顺序。发布目录仍仅核验3.2/3.3。

full手写续读保持旧具体docId格式；结构化nextCall的cursor保存原别名与偏好。调用者按返回arguments续页时保留
最初范围说明；另开手写请求时按其新选择器说明范围。模型自动保存ledger、客户端上下文注入和真实模型A/B未经验证，
不属于本轮公共MCP专用适配工作。

```powershell
$env:CARGO_INCREMENTAL='0'
cargo test -p ik-story --lib --test chapter_relations --test chapter_suggestions --test compact_contract --test compact_summaries --test read_navigation --test usability --test e2e --test improvements
cargo check -p ik-story --all-targets
cargo fmt --all -- --check
git diff --check
$env:STORY_SCHEMA_REPORT='../zzz-story-extractor/out/story_mcp_usability_schemas.json'
cargo test -p ik-story --test usability generated_schemas_are_self_contained_and_match_results
cargo run -p ik-story --example usability -- ../zzz-story-extractor/out/zzz_story_3.3.0_chs ../zzz-story-extractor/out/story_mcp_usability.json
python crates/story/tests/validate_schemas.py ../zzz-story-extractor/out/story_mcp_usability_schemas.json ../zzz-story-extractor/out/story_mcp_usability.json
```

标准schema验证需要jsonschema。本机使用提取器.venv中的4.26.0；含原文的工作流响应留在提取器out，仓库指标只含
统计量。当前代码为未提交工作树，未推送或部署，报告不证明线上服务已更新。
