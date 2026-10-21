# 章节资料参考

## 目录与范围

- `ik_catalog(path=…)` 返回章节或分类的文档目录；`path` 支持分组 ID、章节名、文档 ID 和已核验版本别名。
- 版本别名（如 `3.3`）对应该版新增主线；`snapshotVersion` 是整个语料快照的版本，包含历史章节。
- `manifest=true` 返回固定范围清单，包括文档数、行数、排除数、占位与未关联候选计数。状态计数可能交叉。
- `excludePlaceholders` / `excludeUnlinked` 只排除整篇全部属于对应状态的文档，混合文档保留。

## 原文与分页

- `ik_read(docId=…)` 返回指定文档的原文；分组与版本别名按目录跨文档读取，文档 ID 可用 `follow=true` 续读同组后续文档。
- `docIds` 支持一次读取 1～16 篇文档，与 `docId` / `follow=true` 互斥，共用行数及字数预算。
- `nextCall.arguments` 包含快照绑定的续读 `cursor`；游标保留范围、分页大小与输出偏好。
- `complete` 表示当前指定续读目标已到末尾；`nextDocument.relation=catalogAdjacent` 表示同组目录相邻。
- `coverageDelta` 仅记录本次完整返回的原文行。`ik_coverage` 对照 `manifestId` 合并提交区间，返回 `ledger`、覆盖统计和 `pending` 缺口；服务端不保存调用者的读取历史。

## 内容与来源

- 原文行保留 `number`、`speaker`、`kind`、`status` 及可用的 `sourceKey` / `lineId`；文本中的 `【选项】` 表示可选回复。
- 占位说明属于提取资料中的说明文本；未知署名可能来自尚未解析的角色，空署名本身不确定是否为旁白。
- 剧情演出、聊天、气泡、关卡内对话按内容类型与来源目录组织；跨类型与跨文档播放顺序可能未知。
- 补充资料包含过场字幕与漫画；未关联 Comic 的完整场景、说话人与时间可能未知。设定资料属于独立文档类型。
- `extractionMode`：`default` 为默认哲视角，不确定分支保留；`integrity` 保留双方分支候选；`legacy` 为旧语料未声明。
- `includeEvidence=true` 展开内容、署名、配音关联、行序与资源引用的来源状态；姓名表关联与资源引用不表示实际播放已核验。
