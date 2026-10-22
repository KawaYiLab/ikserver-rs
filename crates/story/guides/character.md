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

- `ik_character(name=…, purpose="roleplay")` 返回 `PersonaResult`：代理人设定档案摘录、常对话对象与台词样本；`purpose` 缺省为 `overview`，即上面的出场统计。
- `samples[]` 只取逐句署名已解析的对白：不含推断署名、署名未核验、选项、旁白与占位；同文去重，字数 6～120。每篇取较长的句子，按章节分散，按语料行序排列；同一快照同一参数结果相同。
- `samples[].context` 是文档内的前一句，`samples[].read` 为样本附近原文入口。`maxResults` 为样本数，默认 24、上限 60；`candidateLines` 为可抽样台词总数。
- `partners[].exchanges` 为与该角色相邻的署名对白往来次数，`scenes` 为有往来的场景数；前几位对话对象各保底一条往来样本。相关名称（`relatedNames`）不合并、不计为对话对象。
- `profiles[]` 为代理人分类里标题含名字的档案，摘录按整行截断到约 1500 字，`truncated=true` 时 `nextRead` 为续读入口；`otherTitledLore` 为其他分类里标题含名字的篇数。
- `until` 接受章节 id、章节名或已核验版本别名，台词与往来只统计到该章（含主线与补充），按章节目录顺序截止；设定档案不随之过滤，可能含之后的信息。
- 名字只包含匹配到多个角色时报错并列出候选；`includeEvidence=true` 不适用于此模式，逐句证据见 `ik_search(speaker=…, includeEvidence=true)`。性格、语气等描述不是语料字段。
