# 角色资料参考

## 角色查询

- `ik_character(name=…)` 返回匹配角色的设定档案、章节统计、目录首见文档、常同场角色与相关检索入口。
- 已记录的同一角色身份变体合并统计，原文显示名保留。相关名称（如「零号·安比」与「安比」）独立列出，未自动合并。
- 逐句署名、推断署名、正文提及与 TextMap 姓名表关联分别计数；姓名表计数不等于实际台词或出场数。
- 目录首见文档表示目录中的首次匹配位置，实际首次登场时间可能未核验。
- 匹配说话人为空时，当前语料没有对应的可解析署名，仍可能存在正文提及或设定档案。

## 原文与字段

- `ik_search(speaker=…)` 按逐句署名筛选；`keyword=…` 检索正文和所选元数据，`matchIn=body` 仅匹配正文。
- `profiles[].nextRead` 为设定档案原文入口；设定资料与主线对白是不同文档类型。
- `nextCalls` 提供角色署名及姓名检索入口；`chapterCount` / `chaptersTruncated` 表示章节总数与展示截断情况。
- `includeEvidence=true` 展开署名来源、身份关联、未知状态与文档证据预览；`evidencePreviewTruncated` 表示预览是否截断。
- `extractionMode` 标识分支提取范围；`resolved` 表示姓名解析状态，不表示对应场景播放已核验。

## 扮演资料（purpose=roleplay）

- `ik_character(name=…, purpose="roleplay")` 返回 `PersonaResult`；`purpose` 缺省为 `overview`，即上面的出场统计。
- `profile` 为选中的代理人档案：标题名或括号里的全名（忽略空格与「」）与 `name` 精确一致优先，否则标题含名字且只有一篇时使用；其余标题含名字的档案列在 `otherProfiles`。
- `profile.sections[]`：`kind=profile`（开头、HIA 执照、深度身世与机密档案、录像店私密评语）附原文 `text`，每节按整行截断到约 1500 字，`truncated=true` 时 `read` 为续读入口；`kind=dialogue` 的内容见 `dialogueScenes`；`kind=other` 只给整节入口 `read`。
- `dialogueScenes[]` 为档案对话原文片段（信赖邀约、街区日常、随行气泡），按档案行序；`maxScenes` 为段数，默认 6：信赖邀约 1 幕、随行气泡 1 段（等距取最多 10 组）、其余为街区日常（等距取，优先不少于 4 行的幕）；缺少某类时名额让给其余类。每段最多 30 行 / 约 1500 字，`truncated=true` 时 `read` 为续读入口，`sectionRead` 为整节入口。
- 档案对话不逐句署名：`【名字】` 是原文标注的说话人，未标注的行可能是玩家选项、旁白或角色本人。`lines[].text` 保留原文（含行尾来源键），`lines[].number` 是档案内行号，可直接用于 `ik_read(docId, offset)`。
- `storySamples[]` 只取主线逐句署名已解析的对白：不含推断署名、署名未核验、选项、旁白与占位；同文去重，字数 6～120。每篇取较长的句子，按章节分散，按语料行序排列；`maxResults` 为样本数，默认 8、上限 40。`context` 是文档内的前一句，`read` 为样本附近原文入口。
- `partners[].exchanges` 为与该角色相邻的主线署名对白往来次数，`scenes` 为有往来的场景数；前几位对话对象各保底一条往来样本。相关名称（`relatedNames`）不合并、不计为对话对象。
- `until` 接受章节 id、章节名或已核验版本别名，只作用于主线台词与往来，按章节目录顺序截止（含该章主线与补充）；档案不随之过滤，可能含之后的信息。
- 抽样是确定性的，同一快照同一参数结果相同。名字只包含匹配到多个说话人时报错并列出候选；`includeEvidence=true` 不适用于此模式，逐句证据见 `ik_search(speaker=…, includeEvidence=true)`。性格、语气等描述不是语料字段。
