# 业务规则：举报与审核、处罚、入站考试、管理员权力

## 范围

- 用户侧举报：创建、批量查询是否已举报、我的举报列表。
- 审核侧举报处理：按目标聚合列表、领取 / 释放、结案（驳回 / 隐藏 / 删除 / 警告 / 封禁）、多人举报自动隐藏、删除任务 outbox 与 cron 重试、通知文案。
- 举报管理后台（report-panel 插件）。
- 入站考试：状态、开考、交卷、回顾、冷却、奖励、写操作门控 `EXAM_REQUIRED`、存量用户豁免。
- 入站题库管理后台（exam-question-panel 插件）。
- 管理员能力（论坛用户 `isAdmin`、AI 账号 `isAiAgent`、后台管理员 `admin::user`）与处罚效果（`blocked`、`accountStatus`）。

所有路径相对于旧后端根目录 `C:\Users\Administrator\Documents\GitHub\InterKnot-web\server`，另有注明的除外。

## 来源文件

- `src/api/report/content-types/report/schema.json`
- `src/api/report/controllers/report.ts`
- `src/api/report/routes/report-custom.ts`
- `src/api/report/services/report.ts`
- `src/api/report/utils/report-core.ts`
- `src/api/report/utils/report-actions.ts`
- `src/api/report/utils/report-lock.ts`
- `src/api/report-action/content-types/report-action/schema.json`
- `config/cron-tasks.ts`（`drain-report-actions`）
- `src/plugins/report-panel/strapi-server.js`、`admin/src/{index.jsx,constants.js,pages/HomePage.jsx,components/ReportDetailDrawer.jsx,components/ResolutionModal.jsx}`
- `src/api/exam/{routes,controllers,services}/exam.ts`
- `src/api/exam-question/content-types/exam-question/schema.json`、`src/api/exam-question/controllers/exam-question.ts`
- `src/api/exam-attempt/content-types/exam-attempt/schema.json`
- `src/utils/exam.ts`、`src/middlewares/require-exam-passed.ts`、`src/index.ts`（存量豁免）
- `src/plugins/exam-question-panel/strapi-server.js`、`admin/src/pages/App.jsx`
- `src/extensions/users-permissions/content-types/user/schema.json`、`src/extensions/users-permissions/strapi-server.ts`
- `src/middlewares/{require-auth,optional-auth,error-format}.ts`、`src/utils/{account-lock,auth-revocation,benefits,level,content-visibility}.ts`
- `src/api/auth-session/services/auth-session.ts`
- 测试：`tests/unit/{report,report-claim-race,report-resolution-database,exam,exam-question,users-permissions-extension,account-deletion}.test.ts`

---

## 1. 举报（用户侧）RPT

### 1.1 数据与约束

- **RPT-001** 举报记录字段：`reporter`（论坛用户）、`targetType` ∈ {`article`,`comment`,`user`}（必填）、三选一关系 `article` / `comment` / `targetUser`、`targetKey`（私有字符串）、`reason`、`detail`（≤500）、`reportStatus`（默认 `pending`）、`resolution`、`resolutionNote`（≤500）、`targetResolutionNote`（≤500）、`handledBy`（字符串）、`handledAt`、`assignedTo`（后台管理员 `admin::user`）、`assignedAt`、`actionPending`（默认 false）、`snapshot`（JSON）。`src/api/report/content-types/report/schema.json:13-84`
- **RPT-002** `reason` 枚举：`spam`,`abuse`,`porn`,`illegal`,`privacy`,`misinfo`,`plagiarism`,`other`。`src/api/report/utils/report-core.ts:59-68`
- **RPT-003** `reportStatus` 枚举：`pending`,`reviewing`,`accepted`,`rejected`；`resolution` 枚举：`none`,`content_hidden`,`content_deleted`,`user_warned`,`user_blocked`。`src/api/report/content-types/report/schema.json:50-58`
- **RPT-004** `targetKey` 格式为 `${targetType}:${id}`，其中 article/comment 用 `documentId`，user 用数字用户 id 的字符串；它是内容被删、关系断开后仍能聚合同一目标的稳定标识。`src/api/report/controllers/report.ts:180`、`src/api/report/utils/report-core.ts:19-28`
- **RPT-005** 旧数据缺 `targetKey` 时按关系推导（user 用 `targetUser.id ?? snapshot.userId`，内容用关系 `documentId ?? snapshot.documentId`），都没有时退化为 `report:${id}`，只代表这一条举报自身。`src/api/report/utils/report-core.ts:19-28`
- **RPT-006** 同目标分组条件 = `id = 自身` OR `targetKey = key` OR 关系匹配（key 为 `report:` 时不加关系条件）；`targetWhere` 在 documentId 为空时抛错，防止退化成全表匹配。`src/api/report/utils/report-core.ts:31-43,153-161`
- **RPT-007** 入参归一化：`targetType`、`reason` 先 trim 再转小写；`targetId`、`detail` 做 trim。`src/api/report/controllers/report.ts:35-48,128-132`
- **RPT-008** 校验顺序和错误：targetType 非法返回 `Invalid targetType`；缺 targetId 返回 `Missing targetId`；reason 非法返回 `Invalid reason`；`reason=other` 且 detail 为空返回 `detail is required for reason "other"`；detail 超过 500 字符返回 `detail exceeds 500 characters`；以上均为 400。`src/api/report/controllers/report.ts:134-141`
- **RPT-009** 目标必须当前公开可见：article 需满足未隐藏且已发布（`PUBLIC_ARTICLE_FILTER`）；comment 需未隐藏、所属文章公开、父评论（若有）也公开且是顶层；user 只要求用户存在且 id 为正整数（不检查 blocked/deleted）。不满足返回 404 `Target not found`。`src/api/report/services/report.ts:63-121`、`src/utils/content-visibility.ts:10-35`
- **RPT-010** 禁止举报自己：目标 `ownerUserId`（内容作者的 userId，user 目标就是其本人）等于当前用户时返回 400 `不能举报自己的内容`。`src/api/report/controllers/report.ts:145-147`
- **RPT-011** 去重：同一举报人对同一目标已有 `pending`/`reviewing` 举报时返回 400 `你已举报过该内容，正在处理中`；已结案（accepted/rejected）的不拦，可再次举报。`src/api/report/controllers/report.ts:149-160`
- **RPT-012** 新建的举报 `reportStatus='pending'`，`detail` 为空时存 null，并写入 `snapshot`；返回 201 `{documentId,targetType,reason,reportStatus,createdAt}`。`src/api/report/controllers/report.ts:176-215`
- **RPT-013** 快照内容：article 为 `{targetType,documentId,title,excerpt,images,isAnonymous,ownerUserId}`；comment 为 `{targetType,documentId,excerpt,images,isAnonymous,ownerUserId,article:{documentId,title}|null}`；user 为 `{targetType:'user',userId,username}`。`src/api/report/utils/report-core.ts:121-151`
- **RPT-014** `excerpt` 规则：去掉 HTML 标签并 trim，超过 200 字符时截断为前 200 字符再加 `…`；图片取 article 的 cover 或 comment 的 images，最多 9 张，每张存 `{url (canonical), thumbUrl (宽 360)}`。`src/api/report/utils/report-core.ts:84-106`

### 1.2 权限

- **RPT-020** `POST /reports`、`GET /reports/check`、`GET /reports/my-list` 仅要求登录（`require-auth`）；**刻意不要求通过入站考试**。`src/api/report/routes/report-custom.ts:1-35`
- **RPT-021** 被封禁或已注销的用户会被 `require-auth` 拒绝（401），因此不能举报。`src/middlewares/require-auth.ts:36,45-47`

### 1.3 查询接口

- **RPT-030** check：`targetType` 必填（非法返回 400）；`targetIds` 可以是数组或逗号分隔字符串，trim 后去重，最多取 100 个；为空时返回 `{data:{}}`。`src/api/report/controllers/report.ts:50-55,218-229`
- **RPT-031** check 返回 `{id: boolean}`，只有当前用户对该目标有 `pending`/`reviewing` 举报时才为 true；user 类型 id 需能解析为正整数。`src/api/report/controllers/report.ts:231-268`
- **RPT-032** my-list：只列当前用户的举报，按 `createdAt` 倒序；分页参数 `start`/`limit` 默认 20、最大 50；返回 `documentId,targetType,reason,detail,reportStatus,resolution,createdAt,handledAt,target{documentId,title,excerpt,username}`，target 字段全部取自快照。`src/api/report/controllers/report.ts:271-330`、`src/utils/common.ts:269-285`

### 1.4 限流

- **RPT-040** 举报创建采用 Redis 固定窗口：`report:rl:m:{userId}` 窗口 60 秒、上限 `REPORT_RATE_PER_MIN`（默认 3）；`report:rl:d:{userId}` 窗口 86400 秒、上限 `REPORT_RATE_PER_DAY`（默认 20）；环境变量非正整数时取默认值。`src/api/report/controllers/report.ts:26-33,68-89`
- **RPT-041** 自增与 TTL 必须原子写入（`incrWithTtl`），避免留下永不过期的计数 key；分钟窗口先判；超限返回 429，带 `Retry-After` 头，body 为 `{error:{code:'TOO_MANY_REQUESTS',message:'举报太频繁了，请稍后再试',retryAfterSec}}`，retryAfterSec 取 key 剩余 TTL（TTL ≤0 时回退到窗口长度）。`src/api/report/controllers/report.ts:61-85,162-174`
- **RPT-042** Redis 出错时 fail-open（不限流）。`src/api/report/controllers/report.ts:86-88`
- **RPT-043** 限流检查位于参数校验、目标存在、自举报、去重之后，所以这些失败不消耗额度；而被 429 拒绝的请求仍会让计数自增。`src/api/report/controllers/report.ts:134-174`

### 1.5 副作用：多人举报自动隐藏

- **RPT-050** 创建成功后异步执行（`setImmediate`）`evaluateAutoAction`，失败只记 warn，不影响举报本身。`src/api/report/controllers/report.ts:189-202`
- **RPT-051** 只对 article/comment 生效，user 目标直接跳过。`src/api/report/services/report.ts:180`
- **RPT-052** 在组锁内统计该目标 **`pending`** 举报的去重举报人数，达到 `REPORT_AUTO_HIDE_THRESHOLD`（默认 5）时：如果内容存在且尚未隐藏，就以 `hiddenReason='auto_report_threshold'` 隐藏内容，同批 pending 举报 `updateMany` 为 `reviewing`（不写 assignedTo/assignedAt，因此仍可领取），并通知作者 `你的${label}因收到多人举报已被暂时隐藏，等待管理员复核。`，其中 label 在 article 时为 `委托《${title}》`，comment 时为 `评论`。`src/api/report/services/report.ts:173-227`、`src/api/report/utils/report-core.ts:78-82`
- **RPT-053** 隐藏内容（自动和管理员操作通用）：article 置 `isHidden=true, hiddenReason, isPinned=false, pinnedAt=null` 并清空 `pinnedBy`；comment 置 `isHidden=true, hiddenReason, pinnedAt=null` 并清空 `pinnedBy`；作用于同一 documentId 的所有行，在一个事务内完成。`src/api/report/services/report.ts:123-166`

### 1.6 隐私

- **RPT-060** `targetKey` 是私有字段；my-list 不返回 `resolutionNote`、`handledBy`、`assignedTo`，举报人只能通过系统通知看到处理备注。`src/api/report/content-types/report/schema.json:14`、`src/api/report/controllers/report.ts:284-317`
- **RPT-061** 被举报人收到的通知不包含举报人身份。`src/api/report/services/report.ts:280-336`

### 1.7 边界与已知坑

- **RPT-070** 已隐藏的内容不能再被举报（RPT-009），所以自动隐藏之后同一内容不会再新增举报；user 目标则没有这层限制。
- **RPT-071** 自动隐藏只统计 pending 状态的举报人，已领取（reviewing）的那部分不计入阈值。`src/api/report/services/report.ts:183`
- **RPT-072** 隐藏文章时没有调用置顶缓存失效（`invalidatePinnedCache`），而管理员 pin/unpin 时会调用。待确认重写时是否需要同步刷新缓存。`src/api/report/services/report.ts:123-147`
- **RPT-073** 举报行没有任何清理任务，已结案的行会一直累积，同一目标可能超过 500 条。`src/api/report/controllers/report.ts:765-771`

---

## 2. 举报处理（审核侧）RPTADM

### 2.1 权限

- **RPTADM-001** 审核接口挂在后台管理员路由下（需要 `admin::user` 的 JWT），与论坛用户的 `isAdmin` 无关。RBAC 动作：`plugin::report-panel.read`（查看列表，同时控制菜单可见）和 `plugin::report-panel.resolve`（结案、领取、释放）。`src/plugins/report-panel/strapi-server.js:3-83`、`src/plugins/report-panel/admin/src/index.jsx:30-31`
- **RPTADM-002** 路由：`GET /list`（read），`POST /:documentId/resolve`、`POST /:documentId/reviewing`（即领取）、`POST /:documentId/unclaim`（三者都需要 resolve）。`src/plugins/report-panel/strapi-server.js:31-82`
- **RPTADM-003** 领取、释放、结案都必须取得当前管理员的整数 id，取不到返回 401。`src/api/report/controllers/report.ts:690-692,742-744,900-902`

### 2.2 列表（按目标聚合）

- **RPTADM-010** `status` 参数：`pending`（默认）对应 [`pending`]，`reviewing` 对应 [`reviewing`]，`closed` 对应 [`accepted`,`rejected`]；其他取值一律按 pending 处理。`src/api/report/controllers/report.ts:348-354`
- **RPTADM-011** 可选筛选条件：`targetType`、`reason`、`from`/`to`（作用于举报 `createdAt`，无效日期忽略）；`mine=1|true` 只在 reviewing tab 生效，表示仅看 `assignedTo=当前管理员`。`src/api/report/controllers/report.ts:356-379`
- **RPTADM-012** `targetKey` 参数用于精确定位（详情用）：`user:<id>`、`article|comment:<documentId>`（同时匹配 targetKey 或关系）、`report:<数字id>`；传了 targetKey 时不做分页。`src/api/report/controllers/report.ts:381-400,613`
- **RPTADM-013** 一次最多取回 5000 条举报行，按 createdAt 倒序，在内存中按 targetKey 聚合后再分页；`limit` 默认 20、最大 100，分页按目标组计数。`src/api/report/controllers/report.ts:365,404-416,612-613`
- **RPTADM-014** 单条举报行的输出字段：`documentId,targetType,targetKey,reason,detail,reportStatus,actionPending,resolution,resolutionNote,targetResolutionNote,handledBy,handledAt,assignedTo{id,displayName},assignedAt,createdAt,reporter(用户名),targetExists,targetHidden,targetBlocked,contentUrl,authorProfileUrl,reporterProfileUrl,snapshot{documentId,title,excerpt,images,username,isAnonymous,article}`。`src/api/report/controllers/report.ts:483-515`
- **RPTADM-015** 链接拼法（基础地址为 `PUBLIC_SITE_URL`，默认 `https://interk.net`，去掉末尾斜杠）：文章为 `/post/{docId}`；评论为 `/post/{父文章docId}?comment={评论docId}`（快照里没有父文章时不给链接）；用户主页为 `/profile/{author.documentId}`，需要先把 userId 映射到 author。`src/api/report/controllers/report.ts:418-479`、`src/api/report/utils/report-core.ts:46-49`
- **RPTADM-016** 作者主页链接对应的用户：user 目标取 `targetUser.id ?? snapshot.userId`；内容目标取 `snapshot.ownerUserId`。匿名内容的作者主页同样会给出（仅后台可见）。`src/api/report/controllers/report.ts:471-478`
- **RPTADM-017** 聚合规则：`reportCount` 为条数；`firstReportAt`/`latestReportAt` 取时间最小值和最大值；组状态优先级为 pending > reviewing > 首行状态；`actionPending`、`targetExists`、`targetHidden`、`targetBlocked` 在组内任一行为真即为真；`assignedTo` 取组内第一个非空值；`snapshot` 和链接取组内首行（即最新一条）。`src/api/report/controllers/report.ts:540-584`
- **RPTADM-018** 关键词（转小写后做 contains 匹配）只匹配组快照的 `title/excerpt/username`，或组内任一举报人的用户名。`src/api/report/controllers/report.ts:588-603`
- **RPTADM-019** 组按 `latestReportAt` 倒序；返回 `{data:[{…组字段, reports:[行]}], meta.pagination{start,limit,total,pageCount}}`。`src/api/report/controllers/report.ts:605-645`
- **RPTADM-020** 管理员展示名：`firstname lastname` → `username` → `email` → `admin:<id>`。`src/api/report/controllers/report.ts:104-112`

### 2.3 领取 claim

- **RPTADM-030** 举报不存在返回 404 `Report not found`；已结案返回 400 `该举报已结案`；`actionPending` 为真返回 400 `内容删除任务正在处理中`。`src/api/report/controllers/report.ts:746-759`
- **RPTADM-031** 锁集 = 同组且未结案（pending/reviewing）的行（在事务外查询，按 id 升序）再并上当前这条，统一按 id 升序 `SELECT … FOR UPDATE`，避免死锁。已结案行不进锁集，防止它们挤占窗口。`src/api/report/controllers/report.ts:774-807`
- **RPTADM-032** 在锁内判定：任一行 `action_pending` 为真则返回 `内容删除任务正在处理中`；任一行是 `reviewing` 且被**他人**领取，则整体拒绝（不做部分领取），返回 **409** `{error:{code:'ALREADY_CLAIMED',message:'该举报已被他人领取',assignedTo:{id,displayName}|null}}`。`src/api/report/controllers/report.ts:809-827,867-886`
- **RPTADM-033** 可领取的行 = `pending`，或 `reviewing` 且 `assigned_at` 为空（自动隐藏转入 reviewing 的行）；逐行改为 `reviewing`，写 `assignedTo=当前管理员`、`assignedAt=now`，更新语句的 where 中再次带上可领取条件。返回 `{success:true, claimedCount}`。`src/api/report/controllers/report.ts:829-864,888`
- **RPTADM-034** 管理员重复领取自己已领取的目标是幂等成功（claimedCount 可以为 0），不返回 409。`tests/unit/report-claim-race.test.ts`
- **RPTADM-035** `assignedTo` 与 `assignedAt` 只在 claim/unclaim 时成对写入，`assignedAt` 可以当作「已被领取」的标量标记。`src/api/report/controllers/report.ts:829-830`

### 2.4 释放 unclaim

- **RPTADM-040** 在组锁内，把同组中 `reviewing` 且 `assignedTo=当前管理员` 且 `actionPending` 为 false 或 null 的行改回 `pending`，并清空 `assignedTo`、`assignedAt`；返回 `{success:true, releasedCount}`。不校验是否已结案（已结案行本来就不会匹配）。`src/api/report/controllers/report.ts:895-933`
- **RPTADM-041** 释放操作会等待并发进行中的结案完成，且不会把已结案（accepted）的行改回 pending。`tests/unit/report-claim-race.test.ts`（unclaim waits for a concurrent resolution）

### 2.5 结案 resolve

- **RPTADM-050** 请求体为 `{action, note?, targetNote?}`，其中 action ∈ `reject`,`hide_content`,`delete_content`,`warn_user`,`block_user`，非法返回 400 `Invalid action`；note 和 targetNote 先 trim，再截断到 500 字符（超长截断而不报错）。`src/api/report/controllers/report.ts:652-664`、`src/api/report/utils/report-core.ts:70-76`
- **RPTADM-051** 前置校验：不存在返回 404；已结案返回 `该举报已结案`；`actionPending` 为真且 action 不是 `delete_content` 时返回 `内容删除任务正在处理中`；user 目标使用 hide/delete 时返回 `对用户的举报不支持内容处理动作`；当前行为 reviewing 且被他人领取时返回 `该举报已被他人领取，仅处理人可结案`。`src/api/report/controllers/report.ts:675-701`
- **RPTADM-052** pending 状态，或 reviewing 但未分配的举报，任何有 resolve 权限的管理员都能直接结案，无需先领取。`src/api/report/controllers/report.ts:694-701`
- **RPTADM-053** `handledBy` = 管理员 `email || username || admin:<id>`。`src/api/report/controllers/report.ts:703-706`
- **RPTADM-054** 服务层在组锁内复查：当前行已不是 pending/reviewing 时抛 `该举报已结案`；同组任一未结案行被其他管理员领取时抛 `该举报已被他人领取，仅处理人可结案`；任一行 `actionPending` 为真且 action 不是 delete 时抛 `内容删除任务正在处理中`。服务层抛出的错误统一以 400 返回 message。`src/api/report/services/report.ts:256-273`、`src/api/report/controllers/report.ts:718-721`
- **RPTADM-055** **一次结案作用于整个组**：同目标所有 pending/reviewing 举报一起结案，返回 `{success:true, action, resolvedCount, actionPending?}`。`src/api/report/services/report.ts:261-264,371-380`
- **RPTADM-056** 目标标识回退：内容被删、关系断开后，用 `snapshot.documentId`（user 目标用 `snapshot.userId`）兜底；`ownerUserId` 对内容目标取 `snapshot.ownerUserId`，对 user 目标取用户 id。`src/api/report/services/report.ts:240-251`

#### 各动作效果（同组所有未结案行）

- **RPTADM-060** `reject`：结果为 `reportStatus='rejected'`、`resolution='none'`；如果内容是**自动隐藏**的（`hiddenReason='auto_report_threshold'`），则恢复为 `isHidden=false, hiddenReason=null`；管理员手动隐藏（`admin`）的不恢复。不通知被举报人。`src/api/report/services/report.ts:337-342`、`tests/unit/report-resolution-database.test.ts`（reject only restores automatic restrictions）
- **RPTADM-061** `hide_content`：以 `hiddenReason='admin'` 隐藏内容（规则见 RPT-053），`resolution='content_hidden'`，通知作者 `你的${label}因违反绳网社区规范已被隐藏。如有异议请联系管理员。`。`src/api/report/services/report.ts:284-291`
- **RPTADM-062** `warn_user`：只发通知，不改用户或内容；`resolution='user_warned'`；通知作者或被举报用户 `请注意：你的${label}收到举报且经核实存在不当之处，请遵守绳网社区规范。`。`src/api/report/services/report.ts:317-324`
- **RPTADM-063** `block_user`：把 `ownerUserId` 对应用户的 `blocked` 置为 true（只改这一个字段，不改 `accountStatus`，不隐藏、不删除内容）；`resolution='user_blocked'`；通知 `你的账号因违反绳网社区规范已被封禁。如有异议请联系管理员。`。内容目标的举报也可以封禁作者，匿名内容同样适用。`src/api/report/services/report.ts:325-336`
- **RPTADM-064** `delete_content`：分两阶段执行，见 2.6。
- **RPTADM-065** 非删除动作的落库字段：`reportStatus`（reject 时为 `rejected`，否则为 `accepted`）、`actionPending=false`、`resolution`、`resolutionNote=note??null`、`targetResolutionNote=targetNote??null`、`handledBy`、`handledAt=now`。`src/api/report/services/report.ts:344-358`
- **RPTADM-066** label：article 为 `委托《${snapshot.title ?? snapshot.excerpt ?? ''}》`，comment 为 `评论`，user 为 `账号`。`src/api/report/services/report.ts:252-254`
- **RPTADM-067** 举报人回执（按举报人去重，每人一条）：accepted 时为 `你举报的${label}已处理完毕，感谢你维护绳网环境！`；rejected 时为 `你举报的${label}经核实暂未发现违规，感谢你维护绳网环境。`；有 note 时追加 `\n处理备注：${note}` 后整体截断到 500。被举报人的通知有 targetNote 时同样追加 `\n处理备注：${targetNote}` 并截断到 500。`src/api/report/services/report.ts:280-282,360-370`

### 2.6 删除内容与 report_actions outbox

- **RPTADM-070** 第一阶段（在组锁事务内）：documentId 为空时抛 `举报目标缺少稳定标识`；以 `hiddenReason='admin_delete_pending'` 先隐藏内容；入队删除任务；把同组行标为 `reportStatus='reviewing', actionPending=true`；此时不发任何通知。`src/api/report/services/report.ts:292-316`
- **RPTADM-071** 任务表 `report_actions` 的字段：`operationKey`（唯一，取值 `delete:${targetKey}`）、`targetKey`、`payload`（JSON，私有）、`state` ∈ {`pending`,`processing`,`complete`}、`claimToken`、`leaseUntil`、`nextAttemptAt`、`attempts`（默认 0）、`lastError`（私有）。`src/api/report-action/content-types/report-action/schema.json`
- **RPTADM-072** 入队按 operationKey 幂等：已存在就复用原任务；并发插入撞唯一约束时，回读胜出的那条。payload 包含 `report` 标识、`documentId`、`targetType`、note、targetNote、handledBy、`ownerNotice`（`你的${label}因违反绳网社区规范已被删除。如有异议请联系管理员。` 加备注）和 `reporterMessage`（accepted 回执加备注）。`src/api/report/utils/report-actions.ts:52-75`、`src/api/report/services/report.ts:296-311`
- **RPTADM-073** 第二阶段在事务提交后立即执行一次 `executeReportDeletion`：用条件 UPDATE 认领（`state=pending`，或 `state=processing` 且 `lease_until<now`），设置 `state=processing`、写入随机 `claim_token`、`lease_until=now+5min`；认领失败时返回「state 是否为 complete」。`src/api/report/utils/report-actions.ts:78-99`
- **RPTADM-074** 执行时先校验 payload 身份（类型必须是 article/comment，documentId 非空，`reportTargetKey(payload.report) === ${targetType}:${documentId}`），不符则按失败处理；然后在**审核事务之外**调用文档删除（这样外部索引等副作用在审核事务之外发生）。`src/api/report/utils/report-actions.ts:102-114`
- **RPTADM-075** 收尾在组锁内另开事务：给任务行加锁，要求 `claim_token` 匹配且 `state=processing`，否则放弃收尾；向作者和同组未结案举报的举报人（去重）发通知；把这些行写为 `accepted`、`resolution='content_deleted'`、`actionPending=false`，并写 note、handledBy、handledAt；任务置为 `complete`，同时清空 token、lease、lastError，**payload 置 null**。`src/api/report/utils/report-actions.ts:115-157`
- **RPTADM-076** 失败处理：`attempts+1`，状态回到 `pending`，清空 token 和 lease，`next_attempt_at = now + min(3600, 30·2^min(attempts,7))` 秒（依次约为 60s、120s……，上限 1 小时），`last_error` 截断到 2000 字符；**不设最大重试次数**，也没有死信。`src/api/report/utils/report-actions.ts:162-179`
- **RPTADM-077** 返回值 `actionPending = !completed`；删除失败或收尾回滚时，举报保持 `reviewing + actionPending`，内容保持隐藏，并且不会发出任何成功通知。`src/api/report/services/report.ts:373-380`、`tests/unit/report-resolution-database.test.ts`
- **RPTADM-078** cron `drain-report-actions` 每分钟运行一次（规则 `* * * * *`，时区 Asia/Shanghai），按 id 升序最多取 10 条，条件为「`pending` 且 `nextAttemptAt` 为空或已到期」或「`processing` 且租约已过期」，然后逐条执行。`config/cron-tasks.ts:543-549`、`src/api/report/utils/report-actions.ts:182-199`
- **RPTADM-079** 当 actionPending 为真时，管理员可以再次提交 `delete_content`：复用原任务并立即尝试执行（直接调用时不检查 `nextAttemptAt`，相当于手动重试）；其他动作和领取都会被拒绝，释放也会跳过这些行。`src/api/report/controllers/report.ts:680-682,759`、`src/api/report/utils/report-actions.ts:78-93`
- **RPTADM-080** 任务完成后再执行同一 id 不会重复删除，也不会重复发通知（直接返回 true）。`tests/unit/report-resolution-database.test.ts`

### 2.7 通知投递

- **RPTADM-090** 所有举报相关通知都写入 `notification`，`type='system'`、`isRead=false`、message 截断到 500；收件人为空、非正整数，或 `accountStatus` 为 `deleted`/`merged` 时跳过；被封禁用户**仍会收到**。通知写入与审核写操作在同一事务内，实时推送在提交之后。`src/api/report/utils/report-actions.ts:20-50`

### 2.8 并发与锁

- **RPTADM-095** 所有审核命令（自动隐藏、结案、释放、删除收尾）都使用同一种组锁：先在事务外查出同组未结案 id，再与自身合并后升序排列，在事务内首先执行 `FOR UPDATE`，锁内再重读状态。领取用的是同样思路的专用实现。锁读放在事务外，是为了应对 MySQL RR 下一致性快照的建立时机。`src/api/report/utils/report-lock.ts:5-27`、`src/api/report/controllers/report.ts:772-807`（「Strapi 特有」的部分是 relation 存放在 `*_lnk` 联表，以及只能逐条 update 才能写 relation；重写时可以直接用 `UPDATE … WHERE assigned_to IS NULL`）

### 2.9 举报状态机（reportStatus）

```
           create
             │
             ▼
        ┌─────────┐  claim(写 assignedTo/assignedAt)          ┌────────────┐
        │ pending │ ───────────────────────────────────────▶ │ reviewing  │
        │         │  auto-hide(不写 assignee，仍可领取)        │            │
        │         │  delete_content 入队(actionPending=true)  │            │
        │         │ ◀─────────────────────────────────────── │            │
        └─────────┘  unclaim(仅领取人，且 actionPending=false) └────────────┘
          │    │                                             │      │
 reject   │    │ hide/warn/block                    reject   │      │ hide/warn/block
          ▼    ▼                                             ▼      ▼  删除任务 complete
     rejected  accepted ◀────────────────────────────── rejected  accepted(content_deleted)
     （终态，不可重开；同目标可产生新的 pending 举报）
```

- pending 或「reviewing 且未分配」：任意 resolve 管理员可以结案。
- 「reviewing 且已分配」：只有领取人可以结案或释放，其他人领取返回 409。
- reviewing + actionPending：只允许再次提交 `delete_content`，直到删除任务 complete 后转为 accepted。

### 2.10 删除任务状态机（report_actions.state）

```
pending ──claim(token, lease 5min)──▶ processing ──成功收尾──▶ complete（payload=null）
   ▲                                     │
   └──失败：attempts+1，指数退避 nextAttemptAt──┘
processing 且 lease 过期 ──▶ 可被 drain 或直接调用重新认领
```

### 2.11 边界与已知坑

- **RPTADM-100** 封禁没有对应的解封入口（举报后台没有），旧系统只能通过 Strapi 内容管理器手改 `blocked`。重写时需要提供解封功能，待确认。
- **RPTADM-101** 驳回不会撤销 `admin_delete_pending` 或 `admin` 隐藏；结案后不能撤销（没有 reopen 功能）。
- **RPTADM-102** 列表一次最多读 5000 条后在内存中聚合，超过时会丢数据（Strapi 实现限制）；重写时应改为在数据库中 GROUP BY。`src/api/report/controllers/report.ts:404-408`
- **RPTADM-103** 管理端的 409 响应必须真正带上 409 状态码（旧代码里 Strapi 的 `ctx.send` 默认参数会覆盖状态码，这是 Strapi 特有问题，重写时不需要处理），但前端依赖 `error.code`、`error.assignedTo.displayName` 这两个字段。`src/api/report/controllers/report.ts:869-884`
- **RPTADM-104** `updateMany` 的 where 带关系过滤会丢 data，所以先查出 id 列表再更新（Strapi 特有，重写时不需要）。`src/api/report/services/report.ts:345`

---

## 3. 入站考试 EXAM

### 3.1 数据与约束

- **EXAM-001** 题目字段：`question`（text，必填）、`type` ∈ {`single`,`multiple`,`boolean`}（默认 single）、`options`（JSON 数组 `{key,text}`）、`correctKeys`（JSON，**私有**）、`explanation`（text，**私有**）、`weight`（整数，≥1，默认 1）、`enabled`（默认 true）、`tag`（字符串）。`src/api/exam-question/content-types/exam-question/schema.json`
- **EXAM-002** 考试记录字段：`user`、`questionsSnapshot`（私有，必填）、`answers`（私有）、`score`、`totalScore`、`scorePercent`、`passed`、`startedAt`、`submittedAt`、`expiresAt`、`requestIp`（私有）；该表在 Strapi 后台不可见。`src/api/exam-attempt/content-types/exam-attempt/schema.json`
- **EXAM-003** 用户字段：`examPassed`（默认 false）、`examPassedAt`；新注册的邮箱账号和米游社账号都写 `level:0, examPassed:false`。`src/extensions/users-permissions/content-types/user/schema.json:182-190`、`src/api/register-verification/services/register-verification.ts:495`、`src/api/mihoyo-auth/services/mihoyo-auth.ts:427`
- **EXAM-004** 配置常量（环境变量，**必须为正整数**，否则取默认值，因此无法通过配置设为 0）：`EXAM_QUESTION_COUNT=100`、`EXAM_PASS_SCORE_PERCENT=60`、`EXAM_TIME_LIMIT_SECONDS=3600`、`EXAM_MAX_FAILS_BEFORE_COOLDOWN=3`、`EXAM_FAIL_COOLDOWN_SECONDS=1800`、`EXAM_REWARD_DENNY=50`、`EXAM_REWARD_EXP=50`。`src/utils/exam.ts:8-25`
- **EXAM-005** 选项归一化：必须是至少 2 项的数组，key 和 text 都 trim 后非空，key 不能重复，否则整题无效。`src/utils/exam.ts:66-78`
- **EXAM-006** 正确答案归一化：非空数组；转字符串、trim、去重、排序；每个 key 都必须在选项中，否则无效。`src/utils/exam.ts:81-87`

### 3.2 权限与门控

- **EXAM-010** `GET /exam/status`、`POST /exam/start`、`POST /exam/submit`、`GET /exam/review` 只要求登录（require-auth，封禁或注销用户返回 401）。`src/api/exam/routes/exam.ts:9-41`
- **EXAM-011** 写操作门控中间件 `require-exam-passed`：未登录返回 401 `authentication required`；每次请求都按 id 重新查询用户，满足 `examPassed || isAdmin || isAiAgent` 才放行，否则返回 **403** `{error:{code:'EXAM_REQUIRED',message:'需要先通过入站考试才能进行此操作'}}`。通过考试后无需重新登录即可生效。`src/middlewares/require-exam-passed.ts:16-39`
- **EXAM-012** 需要门控的写操作：文章的创建、更新、发布、撤回发布、丢弃草稿、置顶；评论的创建、置顶、采纳；点赞；关注；收藏；私信写操作；投币（user-denny）；签到；直传上传。举报和考试本身**不门控**。`src/api/{article,comment,like,follow,favorite,conversation,user-denny,check-in,direct-upload}/routes/*`（grep `global::require-exam-passed`）
- **EXAM-013** 未通过考试的普通用户等级固定为 0（`addExp` 中 `examExempt = examPassed || isAiAgent || isAdmin`，否则 `newLevel=0`）；等级 0 的权益上限全部为 0（发帖图片、评论图片、正文字数都是 0）。`src/utils/level.ts:156-158`、`src/utils/benefits.ts:8-15`
- **EXAM-014** 存量用户豁免：启动时执行一次（用 core_store 记录 `plugin/exam.legacyExemptionDone` 标记），把所有 `exam_passed` 为 false 或 null 的用户改为 true，并写入 `exam_passed_at=now`。此后注册的用户不受影响。`src/index.ts:167-187`（Strapi 特有的 core_store 标记，重写时用一次性迁移即可）

### 3.3 流程

- **EXAM-020** status：用户不存在返回 404 `USER_NOT_FOUND`/`用户不存在`。已通过时返回 `{passed:true, passedAt, config}`；未通过时返回 `{passed:false, passedAt:null, cooldownRemaining, activeAttempt:{attemptId,startedAt,expiresAt,questionCount}|null, config}`，其中 config 包含 7 项常量。`src/api/exam/services/exam.ts:48-56,88-120`
- **EXAM-021** start 在账号锁内执行（对 `up_users` 行 `FOR UPDATE`），按以下顺序检查：`requireActiveAccount`（封禁或非 active 时返回 403 `ACCOUNT_BLOCKED`/`该账号已被封禁或已注销`）→ 已通过返回 400 `EXAM_ALREADY_PASSED`/`你已通过入站考试` → 冷却中返回 **429** `EXAM_COOLDOWN`/`考试失败次数过多，请稍后再试`（details `{retryAfter}`）→ 有未过期且未提交的场次时**直接恢复**（`resumed:true`，题目和选项顺序与原来一致）。`src/api/exam/services/exam.ts:122-148`、`src/utils/account-lock.ts:4-16`
- **EXAM-022** 抽题：题池为 `enabled=true` 且选项和答案都能通过归一化的题；`weight=max(1, weight||1)`；题池为空时返回 503 `EXAM_NOT_AVAILABLE`/`题库暂未配置，请稍后再试`。用 crypto 随机源做 Fisher–Yates 洗牌，取前 `min(100, 题池大小)` 题（不按 tag 或题型分层），每题的选项再各自洗牌。`src/api/exam/services/exam.ts:150-178`、`src/utils/exam.ts:90-101`
- **EXAM-023** 创建场次时写入 `questionsSnapshot`（含 correctKeys）、`startedAt=now`、`expiresAt=now+3600s`、`requestIp`。返回 `{attemptId, resumed:false, startedAt, expiresAt, questions, config}`；questions 去掉了 `correctKeys`，只含 `{questionId,question,type,options,weight}`。`src/api/exam/services/exam.ts:180-200`、`src/utils/exam.ts:57-63`
- **EXAM-024** submit 的请求体为 `{attemptId, answers: {questionId: string[] | string}}`；缺 attemptId 返回 400 `EXAM_ATTEMPT_ID_REQUIRED`/`缺少考试场次标识`。在账号锁内执行：`requireActiveAccount` → 场次必须属于本人，否则返回 400 `EXAM_ATTEMPT_NOT_FOUND`/`考试场次不存在` → 已提交返回 400 `EXAM_ATTEMPT_ALREADY_SUBMITTED`/`本场考试已提交`。`src/api/exam/services/exam.ts:204-221`
- **EXAM-025** 超时提交（`expiresAt <= now`）：**先落库** `submittedAt=now, passed=false, answers, score=0, scorePercent=0`（totalScore 不写），提交事务后再返回 400 `EXAM_ATTEMPT_EXPIRED`/`考试已超时，请重新开始`。这一次计为一次失败。`src/api/exam/services/exam.ts:223-230,291`
- **EXAM-026** 判分：每题答案的 key 集合与正确答案**完全一致**才得该题权重分，多选题不给部分分；提交的答案为数组时转字符串、去重、排序，为非空字符串时视为单元素数组，缺失或格式不对算错；正确答案为空的题永远算错。`totalScore` 为权重之和，`scorePercent = floor(score/totalScore*100)`（totalScore 为 0 时取 0），`passed = scorePercent >= 60`（含边界）。`src/utils/exam.ts:105-168`
- **EXAM-027** 正常提交后写入 `answers, submittedAt, score, totalScore, scorePercent, passed`；返回 `{passed, score, totalScore, scorePercent, correctCount, questionCount, passScorePercent, cooldownRemaining（通过时为 0）, reward:{denny,exp}|null}`。`src/api/exam/services/exam.ts:235-289`
- **EXAM-028** review：查本人最新一次**已提交**的场次（或指定 attemptId 的那一场），没有时返回 404 `EXAM_ATTEMPT_NOT_FOUND`/`未找到已提交的考试记录`。按快照重新判分，返回 `{attemptId, passed, score, totalScore, scorePercent, correctCount, questionCount, submittedAt, config, questions:[{questionId, question, type, options, userAnswer, isCorrect, weight, score, explanation}]}`。`src/api/exam/services/exam.ts:295-354`
- **EXAM-029** review **不返回 `correctKeys`**；`explanation` 读取的是题库**当前**内容（题目被删除后为 null）。回顾次数不限，通过与否都可以查看。`src/api/exam/services/exam.ts:315-339`

### 3.4 冷却与重考

- **EXAM-030** 冷却：取最近 3 次已提交的场次，如果凑满 3 次且全部未通过，冷却结束时间为最近一次失败的 `submittedAt + 1800s`，剩余秒数向上取整。`src/api/exam/services/exam.ts:61-75`
- **EXAM-031** 只在 start 时检查冷却；submit 不检查。冷却结束后如果再失败一次，最近 3 次仍然全是失败，会立即再次进入冷却（实际效果是 3 次以后每失败一次就冷却 30 分钟）。`src/api/exam/services/exam.ts:129-134`
- **EXAM-032** 失败后次数不限，可以重考；通过后不能再开考。

### 3.5 奖励

- **EXAM-040** 首次通过（在同一账号锁事务内，并且再次确认 `!user.examPassed`）：设置 `examPassed=true`、`examPassedAt=now`、`level=max(1, calculateLevel(exp))`；然后执行 `addDenny(+50, action 'exam_passed', 描述 '通过入站考试奖励', related=user)` 和 `addExp(+50, 'exam_passed', related=user)`；任一奖励返回空值就抛错，整个事务回滚（场次判分和通过状态一起撤销）。`src/api/exam/services/exam.ts:247-275`
- **EXAM-041** `exam_passed` 的经验不受每日经验上限限制，也不占当日额度。`src/utils/level.ts:28-29,150-153`

### 3.6 隐私

- **EXAM-050** 正确答案只保存在服务端的快照里，开考和回顾都不下发；`requestIp`、`answers`、`questionsSnapshot` 都是私有字段。`src/utils/exam.ts:1-6,54-63`

### 3.7 边界与已知坑

- **EXAM-060** 开考后不提交、等它自然过期的场次（`submittedAt` 为空）**不计入**冷却，用户可以马上重新开考拿到新题，相当于可以反复刷题看题。待确认重写时是否把过期未交的场次也算作失败。`src/api/exam/services/exam.ts:61-86`
- **EXAM-061** 开考恢复场次时会返回快照里的 `weight`，前端可以看到每题权重。`src/utils/exam.ts:57-63`
- **EXAM-062** `answers` 原样存储，没有做大小或结构限制，待确认。`src/api/exam/services/exam.ts:206,235-239`
- **EXAM-063** 如果某个用户的 examPassed 被人工改回 false，由于 `addExp`/`addDenny` 对 `(user, 'exam_passed', related)` 去重，再次通过时可能返回空值导致整体回滚，用户永远无法通过。待确认 addDenny 是否也有同样的去重。`src/utils/level.ts:100-113`
- **EXAM-064** 抽题时不校验题型与选项数是否匹配（这个校验只在后台写入时做），旧数据可能存在不一致。`src/api/exam/services/exam.ts:155-169`

---

## 4. 入站题库后台 EXAMADM

- **EXAMADM-001** RBAC：`plugin::exam-question-panel.read`（查看）和 `plugin::exam-question-panel.write`（新增、编辑、删除、启用/停用）；启动时会自动给 Super Admin 角色补上这两个权限。`src/plugins/exam-question-panel/strapi-server.js:10-63`
- **EXAMADM-002** 路由：`GET /list`、`GET /stats`、`GET /:documentId`（read）；`POST /create`、`POST /bulk-delete`、`PUT /:documentId`、`POST /:documentId/toggle`（write）。`src/plugins/exam-question-panel/strapi-server.js:64-161`
- **EXAMADM-003** 列表：筛选 `type`（合法题型）、`enabled=true|false`、`q`（题干或 tag 不区分大小写包含）；按 `updatedAt` 倒序；limit 默认 20、最大 100；返回完整题目，包括 correctKeys 和 explanation。`src/api/exam-question/controllers/exam-question.ts:88-129`
- **EXAMADM-004** 写入校验（create 和 update 共用，update 是整体覆盖）：题干 trim 后非空，否则 `题干不能为空`；题型非法返回 `题型必须是 single、multiple 或 boolean`；选项非法返回 `选项格式不正确，每项需要非空的 key 和 text`；单选至少 2 个选项（`单选题至少需要 2 个选项`）；多选至少 3 个选项（`多选题至少需要 3 个选项`）；判断题必须恰好 2 个选项（`判断题需要 2 个选项`）；答案必须是选项 key 的子集（`正确答案必须是选项 key 的子集`）；多选答案至少 2 个（`多选题正确答案至少需要 2 项`）；单选和判断只能有 1 个答案（`单选/判断题只能有 1 个正确答案`）。`src/api/exam-question/controllers/exam-question.ts:31-57`
- **EXAMADM-005** 默认值：weight 不是 ≥1 的整数时**静默改为 1**；explanation 和 tag 为空串时存 null；`enabled` 只有显式传 `false` 时才是 false，否则为 true。`src/api/exam-question/controllers/exam-question.ts:59-67`
- **EXAMADM-006** findOne、update、toggle：缺 documentId 返回 `缺少 documentId`，不存在返回 404 `题目不存在`；toggle 把 enabled 取反（`enabled !== true` 的视为 false）。`src/api/exam-question/controllers/exam-question.ts:131-183`
- **EXAMADM-007** 批量删除：`documentIds` 为字符串数组，trim 后去掉空值；为空返回 `缺少 documentIds`；物理删除，返回 `{success:true, deletedCount: 传入的数量}`（不核实每个 id 是否存在）。已有场次快照不受影响。`src/api/exam-question/controllers/exam-question.ts:185-198`
- **EXAMADM-008** 统计：`{total, enabled, single, multiple, boolean, questionCount: EXAM_QUESTION_COUNT}`。`src/api/exam-question/controllers/exam-question.ts:200-210`
- **EXAMADM-009** 后台没有导入或导出功能。

---

## 5. 管理员权力 ADMIN

- **ADMIN-001** 系统中有两套「管理员」：(a) 后台管理员 `admin::user`，按 RBAC 操作举报后台和题库后台（以及 Strapi 内容管理器）；(b) 论坛用户上的 `isAdmin` 布尔字段（`configurable:false`，代码里没有任何接口可以设置，只能在后台手工修改）。两者没有关联。`src/plugins/report-panel/strapi-server.js:6-9`、`src/extensions/users-permissions/content-types/user/schema.json:177-181`
- **ADMIN-002** `isAdmin` 用户可以置顶或取消置顶任意文章（其他人返回 403 `Only admins can pin articles` / `Only admins can unpin articles`）；隐藏的文章不能置顶（`Hidden article cannot be pinned`）。`src/api/article/controllers/article.ts:3240-3290`
- **ADMIN-003** 只有 `isAdmin` 用户可以在 `adminOnly=true` 的分区发帖或把文章移入该分区，否则返回 403 `Only administrators can post in this category`。`src/api/article/controllers/article.ts:1249,1341`
- **ADMIN-004** `isAdmin` 用户和文章作者一样，可以对任意文章下的顶层评论置顶、取消置顶、采纳、取消采纳（其他人返回 403 `Not allowed to pin/unpin/accept/unaccept this comment`）。`src/api/comment/controllers/comment.ts:698-699,782-783,851-852,969-970`
- **ADMIN-005** `isAdmin` 和 `isAiAgent` 免考试门控（EXAM-011），等级不会被锁为 0（EXAM-013），权益等级直接取最高档 7。`src/utils/benefits.ts:25-31`
- **ADMIN-006** `isAiAgent` 账号不能被个人拉黑（`Cannot block system account`）；关注推送和热度统计会排除 AI 账号；私信 AI 会话要求目标是 `isAiAgent`。`src/api/user-block/controllers/user-block.ts:66`、`src/api/article/utils/follow-notification.ts:313`、`src/api/conversation/controllers/conversation.ts:903-911`
- **ADMIN-007** 论坛 `isAdmin` 用户**没有**任何举报处理权限，审核只能在后台进行。

---

## 6. 处罚效果 SANCTION

- **SANCTION-001** 封禁标志：`blocked=true`（举报 `block_user` 唯一写入的字段）；`accountStatus` 枚举 `active|blocked|deleted|merged`，在审查范围内的代码中没有任何地方把它写成 `blocked`。注销流程会同时写 `accountStatus='deleted'` 和 `blocked=true`。`src/api/report/services/report.ts:325-331`、`src/api/register-verification/services/register-verification.ts:1162-1177`
- **SANCTION-002** 判定为「非活跃」的条件：`blocked=true` 或 `accountStatus ∈ {blocked,merged,deleted}`（部分路径的写法是 `accountStatus` 存在且不等于 `active`，旧数据为 null 时视为活跃）。`src/middlewares/require-auth.ts:36`、`src/utils/account-lock.ts:12`
- **SANCTION-003** 登录：Strapi 自带的密码登录会拒绝 blocked 用户，错误信息翻译为 `账号已被管理员禁用`；米游社扫码登录返回 403 `USER_BLOCKED`/`该账号已被封禁`；邮箱验证码登录对 blocked 或已注销账号统一返回 `LOGIN_CODE_INVALID`/`验证码无效或已过期`（不暴露账号状态），发码接口对这类账号静默不发。`src/middlewares/error-format.ts:64`、`src/api/mihoyo-auth/services/mihoyo-auth.ts:368-369`、`src/api/register-verification/services/register-verification.ts:648-662,726-742`
- **SANCTION-004** 会话：新建会话（create / createForTicket）时，非活跃账号会被拒绝（403 `ACCOUNT_BLOCKED`）；刷新令牌时如果账号非活跃，**撤销整个 token family** 并返回 `ACCOUNT_BLOCKED`。`src/api/auth-session/services/auth-session.ts:38-43,83,96,118-119`
- **SANCTION-005** 已签发的 JWT：`require-auth` 和 `optional-auth` 每次请求都查库，blocked 或非活跃用户不会被当作已登录（强制登录的接口返回 401，可选登录的接口降级为匿名）；Strapi 原生鉴权路由也会拒绝 blocked 用户。因此封禁**立即**让该用户失去所有写权限和登录态读权限，不需要主动撤销会话。`src/middlewares/require-auth.ts:30-47`、`src/middlewares/optional-auth.ts:48-59`
- **SANCTION-006** 实时连接（私信 WebSocket 等）每 30 秒按库复查一次，blocked、未确认或非活跃账号会被断开（另有最长 30 分钟的绝对时限）；`block_user` 不会主动发布撤销事件，所以最长会延迟约 30 秒。私信 WS ticket 拒绝 blocked 用户。`src/utils/auth-revocation.ts:57-97`、`src/api/conversation/controllers/conversation.ts:1617`
- **SANCTION-007** 受到 requireActiveAccount 保护的写操作（考试开考/交卷，以及其他使用 `withActiveAccountLock` 的写路径）对非活跃账号返回 403 `ACCOUNT_BLOCKED`/`该账号已被封禁或已注销`。`src/utils/account-lock.ts:10-23`
- **SANCTION-008** 被封禁用户的影响面：不能被私信（`target user not available`）；不会被 @ 提及通知；不出现在作者搜索和 @ 建议里；收不到关注推送；不计入热度和浏览统计。`src/api/conversation/controllers/conversation.ts:832-836`、`src/api/comment/content-types/comment/lifecycles.ts:523-535`、`src/api/author/controllers/author.ts:930,1083`、`src/api/article/utils/follow-notification.ts:311`、`src/api/article/services/hot-activity.ts:138-142`、`src/utils/article-hot-views.ts:76`
- **SANCTION-009** 被封禁用户**已发布的内容不会被隐藏**（公开可见性过滤只看 `isHidden`、`publishedAt` 和双向拉黑）；举报系统的通知仍会发给 blocked 用户。`src/utils/content-visibility.ts:10-35`、`src/api/report/utils/report-actions.ts:34-38`
- **SANCTION-010** 内容隐藏的 `hiddenReason` 取值有三种：`auto_report_threshold`（自动隐藏）、`admin`（管理员隐藏）、`admin_delete_pending`（删除前的过渡状态）。隐藏后内容对公众不可见；文章作者本人能否看到，取决于具体调用方是否传 `allowOwnerHidden`。`src/utils/content-visibility.ts:98-118`
- **SANCTION-011** 用户之间的个人拉黑（user-block）不属于处罚，只影响双方可见性和互动。在本文范围之外，仅作区分说明。

---

## 管理后台功能清单（需重建）

举报处理（菜单名「举报处理」，需要 read 权限）：
- 页头说明文字「同一目标的多条举报会一并结案；处理结果会通过系统会话通知相关用户。」，以及刷新按钮。
- 三个 Tab：待处理、复核中、已结案。当前 Tab 的标签上显示数量（如 `待处理 (N)`）；切换 Tab 时重置为第 1 页，并重置为「只看我的」。
- 筛选：目标类型（全部、帖子、评论、用户）；举报理由（全部，加 8 项中文标签：垃圾广告 / 刷屏、辱骂攻击 / 引战、色情低俗、违法违规、泄露隐私、不实信息、抄袭搬运、其他）；关键词输入框（防抖 400ms，提示文字「目标标题 / 举报人 / 被举报人」）；起始日期和结束日期（结束日期包含当天，取到 23:59:59.999）；在复核中 Tab 额外有「范围」：只看我的或查看全部。
- 列表列：目标（标题可点击打开详情，带「已隐藏」「已封禁」「已删除」徽章）、类型、举报数、最新举报时间、状态徽章（待处理、复核中、成立、驳回）、处理人（显示名，自己领取的加「（我）」）、操作。
- 行操作（需要 resolve 权限，已结案 Tab 不显示）：查看详情；pending 时显示「领取」；pending，或 reviewing 且是自己领取时显示「处理」；reviewing 且是自己领取时显示「释放」；被他人领取时显示「已被领取」。
- 目标标题的显示规则：user 显示为 `@用户名`；article 显示标题或「(无标题)」；comment 显示「《文章标题》下的评论」或「评论」。
- 移动端（宽度 ≤768px）改为卡片列表，卡片上显示 N 人举报、处理人等信息，详情抽屉全屏显示。
- 分页：每页 20 个目标，显示「共 N 个目标 · 第 x / y 页」，有上一页和下一页按钮。
- 详情抽屉：类型和状态徽章，举报数大于 1 时显示「N 人举报」；标题；已隐藏、已封禁、已删除、匿名徽章；快照摘要；快照图片缩略图（点击打开原图）；「查看帖子」或「查看所在帖子」链接（隐藏时注明「（已隐藏）」，已删除时显示「帖子（已删除）」）；作者主页或被举报人主页链接。
- 举报明细：每条显示理由、时间、举报人（有主页时可链接，已注销时显示「(已注销)」）、补充说明。已结案的条目额外显示处理结果标签、处理人、处理时间、「举报人备注」和「被举报人备注」。
- 抽屉底部操作：显示当前处理人，提供领取、释放、处理按钮；被他人领取时提示「该举报已被他人领取，仅处理人可结案」。领取或释放之后关闭抽屉并刷新列表；出错时弹出后端返回的 message。
- 处理弹窗：标题为「处理举报 — {目标名}」；动作下拉框（user 目标不显示隐藏和删除；删除内容、封禁用户标注「（高危）」）；「处理备注（给举报人，选填）」和「被举报人备注（选填）」两个输入框，各限 500 字；选择高危动作时必须输入「确认」两个字才能提交；提交按钮文字为「确认{动作}」。
- 列表需要展示 `actionPending`（删除处理中）状态。旧 UI 没有明确展示这一状态，重写时建议加上，待确认。

入站题目管理（菜单名待确认，需要 read 权限）：
- 页头「入站题目管理」，副标题「集中管理入站考试题库，修改后自动同步到考试抽题池。」，以及刷新按钮。
- 统计卡片：总题数、已启用、单选题、多选题、判断题。已启用数量少于 `questionCount` 时标红，并把标签改为「已启用（低于考试所需题数）」。
- 筛选：题型（全部、单选、多选、判断）、状态（全部、已启用、已禁用）、搜索（题干或标签，防抖 400ms）；有 write 权限时显示「新建题目」按钮。
- 列表列：题干（超过 60 字截断）、题型、权重、标签、状态（启用或禁用）、更新时间、操作（启用/禁用切换、删除；删除前弹窗确认「确定删除「…」吗？」）。点击某行打开编辑；每页 20 条，显示「共 N 条 · 第 x / y 页」；移动端改为卡片。
- 新建或编辑弹窗：题干、题型（切换题型时重置为该题型的默认选项并清空答案；判断题默认选项为 true「正确」/ false「错误」，其他题型默认 A–D）、权重（≥1 的整数）、标签、选项编辑器（可增删，至少保留 2 项；新增选项的 key 自动按字母分配）、正确答案（多选用复选框，单选和判断用单选框；修改选项 key 时答案同步跟着改）、解析（可选）、启用复选框、「考生预览（不带答案）」。前端也做与后端一致的校验。没有 write 权限时以只读方式展示。
- 批量删除接口已存在，但 UI 只提供单条删除；要不要做批量勾选，待确认。

## 测试中体现的关键用例

- 快照：文章快照包含摘要和作者 userId；评论内容截断到 200 字；用户快照包含用户名；自动隐藏阈值默认 5；理由和动作枚举固定。（`report.test.ts`）
- 并发领取：两个管理员同时领取，只有一个成功，另一个得到 409 并看到占用者名字；已结案的举报直接返回 400，不进入事务；目标已删除且快照中没有 documentId 时只按自身判定；关系断开但快照有 documentId 时仍能领取；重复领取自己的举报幂等；同组任一行被他人领取时整体拒绝；自动隐藏产生的 reviewing 行可以领取；user 目标按 targetUser 分组；同组累积 500 条以上结案行时仍能领取，被他人领取的行仍会触发 409；释放会等待并发结案完成，且不会复活已结案的行。（`report-claim-race.test.ts`）
- 限流：分钟和天两个窗口各执行一次 incrWithTtl；超过分钟上限返回 429，`Retry-After` 取 TTL；Redis 故障时 fail-open。（`report-claim-race.test.ts`）
- 删除：删除在事务外执行，关系断开后仍能把全部举报结案；删除失败时保留任务，重试成功之前不发成功通知；收尾回滚后重试不会产生重复通知；驳回只恢复自动隐藏；同一举报不能重复结案（第二次报「已结案」，通知数不变）。（`report-resolution-database.test.ts`）
- 判分：全对通过；低于阈值失败；多选不给部分分；缺失、空或格式错误的答案算错；重复 key 会被去重；权重计入得分；空快照可以安全处理；60% 边界算通过；返回每题明细；抽题不重复，题池不足时全取；洗牌不修改输入；下发题目时去掉 correctKeys。（`exam.test.ts`）
- 题目校验：选项和答案的归一化，包括 trim、去重、非子集或为空时返回 null 等情况。（`exam-question.test.ts`）
- 修改密码和重置密码会先撤销全部会话再签发新会话，普通登录不撤销会话；带 sid 的 token 会校验会话是否仍有效，会话已撤销时抛 `SESSION_REVOKED`。（`users-permissions-extension.test.ts`）
- 注销账号后再次调用返回 `ACCOUNT_ALREADY_DELETED`；注销会撤销会话，失败时整体回滚。（`account-deletion.test.ts`）

## 未决问题

1. 重写后的封禁是否改为写 `accountStatus='blocked'`（并与 `blocked` 字段合并），以及是否需要解封接口、封禁期限、封禁原因记录。待确认。
2. 被封禁用户已发布的内容是否需要隐藏或降权。旧系统保持公开。待确认。
3. `block_user` 是否应主动撤销会话、推送实时断开事件（旧系统依赖每次请求查库，加上 30 秒复查）。待确认。
4. 过期未交的考试场次是否计为失败（EXAM-060），以及答案大小的上限。待确认。
5. 删除任务没有最大重试次数和告警，重写时是否加入死信或人工介入机制。待确认。
6. 论坛 `isAdmin` 与后台管理员账号是否打通，例如前台管理员能否在站内直接处理举报。待确认。
7. 举报 user 目标时是否应拒绝已注销或已封禁的用户（旧系统允许）。待确认。
8. 自动隐藏文章时是否需要刷新置顶缓存（RPT-072）。待确认。
9. 题库管理插件的菜单入口文案和所需权限没有逐行核对（`src/plugins/exam-question-panel/admin/src/index.jsx`）。待确认。
10. `addDenny` 对 `exam_passed` 是否去重，以及由此导致无法再次通过的风险（EXAM-063）。待确认。
