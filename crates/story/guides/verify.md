# 原文查证参考

## 匹配参数

- `purpose=verify` 默认使用 `matchIn=body`、严格 AND 与两行邻行；显式参数优先。
- `purpose=locate` 默认每篇两个命中与一行邻行；`browse` 保持一般检索行为。
- `matchIn=all` 允许正文、逐句署名、标题、路径及姓名元数据参与匹配；`body` 仅允许正文参与匹配。
- 无完整匹配时一般检索默认返回部分匹配及缺失词；`allowPartial=false` 关闭部分匹配回退。
- `speaker` 按逐句署名筛选，`path` / `docId` 限定范围。已核验版本别名限定该版新增主线。
- `matchUnit=window` 匹配指定行距内的共现，窗口状态与原文播放顺序分别记录。

## 结果与出处

- 正文或逐句署名命中与仅元数据目录建议分别计数、分页；逐词匹配字段及正文缺失词独立返回。
- `nextCall` / `nextDirectoryCall` 提供快照绑定的分页游标；`document.nextRead` 提供对应原文读取入口。
- 原文定位字段包括 `docId`、文档行号及可用的 `sourceKey` / `lineId`。
- 搜索长行可能截为片段，`complete=false` 标记截断；邻行通过 `isContext` 标识，搜索片段不生成 `coverageDelta`。
- `includeEvidence=true` 展开内容状态、署名来源、配音关联、局部行序与资源摘要。
- 占位说明与正式对白分别标记；未知署名、推断署名和姓名表关联独立记录。`resolved`、`has_voice` 与资源引用均不表示实际播放已核验。
- 未找到表示当前语料快照与查询范围内无匹配，不表示游戏中不存在。快照版本与分支范围由 `snapshotVersion` / `extractionMode` 标识。
