# 私信、通知（敲敲）、在线状态与实时通信 —— 业务规则规格

> 面向 Rust 重写的业务规则提取。旧实现是 Strapi 5（TypeScript，MySQL/PG 双方言），
> 新实现设计目标：**单一 WebSocket 网关**取代旧的「DM WebSocket + 敲敲 SSE + 文章流 SSE」三条通道，
> 通知采用**分组（桶）模型**并与 DM 会话合并成统一收件箱（按最后活动时间混排）。
> 旧的路径与响应形状不需要保留，但所有行为、上限、隐私规则与边界情形必须逐条落实。

## 范围

- 会话（direct / group / AI 会话）、成员、消息（text / image / system）、已读水位与未读聚合。
- DM WebSocket（ticket 鉴权、连接管理、心跳、事件分发）、敲敲 SSE、文章流 SSE、实时吊销。
- 通知（8 种类型）的触发、抑制、匿名化、聚合桶（读时聚合 + 写时聚合双路径）、已读。
- 在线状态（presence）、相关 cron 与 bootstrap 自愈任务。

## 来源文件

| 路径 | 内容 |
|---|---|
| `server/src/api/conversation/controllers/conversation.ts` | 会话 list/direct/aiSession/createGroup/detail/update/markRead/markAllRead/leave/resetContext/aiStop/aiRegenerate/wsTicket |
| `server/src/api/conversation/utils/dm-helpers.ts` | 缓存键、游标、成员查询、findOrCreateDirect、发送限流 |
| `server/src/api/conversation/utils/notification-merge.ts` | 通知→伪会话融合（读时/写时两条路径） |
| `server/src/api/conversation/utils/{activity,dm-typing,ws-server,ws-bus,ws-auth}.ts` | 活跃度推进、typing、WS 服务器/总线/ticket |
| `server/src/api/conversation/routes/dm-custom.ts` | 全部 `/api/dm/*` 路由与鉴权约定 |
| `server/src/api/message/controllers/message.ts`、`utils/serialize.ts` | 消息 list/send/edit/withdraw 与序列化 |
| `server/src/api/notification/**`（lifecycles.ts、controllers/notification.ts、utils/{bucket,conversation,serialize,sse-bus,system}.ts） | 通知生命周期、敲敲接口、桶写侧、SSE |
| `server/src/api/{comment,like,favorite,article,report}/**` | 各类通知的触发点 |
| `server/src/utils/{denny,block,after-commit,auth-revocation,anonymous,notif-buckets-flag}.ts` | 投币、拉黑、事务提交后执行、吊销、匿名名 |
| `server/src/api/presence/**` | 在线状态 |
| `server/config/cron-tasks.ts`、`server/src/index.ts` | 对账 cron 与 bootstrap 自愈 |
| `server/tests/unit/`（dm-conversation-paging、dm-unread-aggregate、messaging-privacy、messaging-state、notification-bucket-*、notification-merge-ai、presence-store、realtime-revocation、read-status-watermark、denny-notification-after-commit） | 钉住行为的测试 |
| `app/composables/{useDmStream,useDmConversations,useKnockKnockConversations,usePresence,useArticleFeedUpdates}.ts` | 前端 UX 意图 |

---

## 1. 会话（CONV）

### 数据与约束

- CONV-001 会话 `kind` 枚举只有 `direct` / `group`（schema 无 ai-session）；**AI 会话就是 kind=direct 且对端 `isAiAgent=true` 的会话**，区别在创建路径（`aiSession` 不写 `directPairKey`、不复用，每次新建）。`server/src/api/conversation/content-types/conversation/schema.json:14`、`controllers/conversation.ts:887`
- CONV-002 direct 会话由 `directPairKey`（`[a,b].sort().join(':')`，唯一索引）保证同一对用户至多一条；AI 会话无 pairKey，可对同一 AI 开多条。`utils/dm-helpers.ts:495`
- CONV-003 会话字段：`title`（≤64 字符）、`avatar`（单图，仅 images）、`owner`、`lastMessageAt`（datetime）、`lastMessage`（一对一指向 message）、`contextEpoch`（AI 上下文分代，int，私有）、`aiModel`（多对一）。`schema.json`
- CONV-004 成员表 `conversation-member`：`role` ∈ `owner|admin|member`（默认 member，direct 双方均为 member）、`lastReadAt`、`muted`、`pinned`（默认 false）、`leftAt`（null = 在场）。`conversation-member/schema.json:25`
- CONV-005 `lastMessageAt` 为**单调递增**的水位：仅当新值更晚才写（`WHERE` 级比较），且与 `lastMessage` 指针分离——通知到达会 bump `lastMessageAt` 但**不动** `lastMessage`（列表预览仍指向真实 DM 消息）。`utils/activity.ts:3`、`notification/content-types/notification/lifecycles.ts:202`
- CONV-006 `lastMessage` 指针推进规则：仅在 `lastMessage.createdAt < 新消息.createdAt` 或同时刻 `id` 更大时更新；整个推进在会话行行锁事务内完成。`utils/activity.ts:18`
- CONV-007 列表查询前提：`lastMessageAt` 非空；建会话路径必写（`= now()`），存量由 bootstrap `backfillConversationLastMessageAt` 用 `COALESCE(updated_at, created_at)` 幂等回填。`src/index.ts:217`

### 权限

- CONV-010 所有 DM 端点：`auth:false` + `global::require-auth`（JWT 解析 + 拒绝 blocked/accountStatus∈{blocked,merged,deleted} 的用户）；**写操作**（direct/aiSession/group/send/edit）额外挂 `global::require-exam-passed`：`examPassed || isAdmin || isAiAgent` 否则 403 `{code:'EXAM_REQUIRED', message:'需要先通过入站考试才能进行此操作'}`。读操作（list/detail/messages/markRead/leave/withdraw/wsTicket）不要求考试。`routes/dm-custom.ts:24`、`middlewares/require-exam-passed.ts:28`、`middlewares/require-auth.ts:40`
- CONV-011 创建私聊：target 必须存在、未 `blocked`、非 `confirmed===false`，否则 404/403；**双向任一方向拉黑**（`hasAnyBlock`）→ 403 `cannot start conversation with blocked user`；不允许自己对自己（400）。`controllers/conversation.ts:815`
- CONV-012 创建 AI 会话：除 CONV-011 外要求 `target.isAiAgent===true`（400 `target user is not an AI agent`）；可选 `title`（trim + 截 64）；每次新建不复用。`controllers/conversation.ts:887`
- CONV-013 **没有「谁可以私信我」的隐私设置**：除拉黑与考试外无任何 DM 准入策略（账号 `profileHidden` 被查询但从未用于 DM 判定）。「待确认」是否需要在新设计中引入。
- CONV-014 所有会话级操作（detail/update/markRead/leave/send/messages）要求调用者是在场成员（`leftAt IS NULL`），否则 403 `not a member`。**例外：`message.edit` 与 `message.withdraw` 只校验「是 sender」，不校验成员关系**。`controllers/message.ts:327`
- CONV-015 `muted` 仅作为客户端展示偏好存储，**服务端不做任何推送/未读抑制**（WS 事件照常推给 muted 成员）。
- CONV-016 群聊（kind=group）：schema 与 role 模型齐备，但 `POST /dm/conversations/group` 直接 501「Group conversation will be available in a later phase.」——**群聊从未上线**，重写时可按新设计实现，无历史行为要兼容。`controllers/conversation.ts:956`

### 流程

- CONV-020 `direct` 幂等：事务内先锁 `up_users` 较小 id 行（防并发重复创建），两步查询找「双方都在（含已 leftAt）的 direct 会话」；找到则优先复用双方都未 left 的、否则复用历史会话并**把 self 的 `leftAt` 重置为 null**（重新激活、保留历史）；找不到才建新会话 + 双方 member（同一事务，任一失败整体回滚）。`utils/dm-helpers.ts:406`
- CONV-021 `leave`：写 `leftAt=now()`；对 direct，另一端列表缓存也失效；向其它在场成员推 `conversation.member.removed {userId}`。私聊任一方离开后双方列表都不再出现该会话（历史保留，可由 CONV-020 复活）。群 owner 转让未实现（注释声明留待下阶段）。`controllers/conversation.ts:1377`
- CONV-022 列表 `GET /dm/conversations`：排序 = **pinned 在前（仅第一页、全量返回，不进游标流、不参与截断），其后按 `(lastMessageAt desc, documentIdSortKey desc)` 游标序**；页大小默认 20（`DM_CONV_PAGE_DEFAULT`）、上限 50（`DM_CONV_PAGE_MAX`）。`controllers/conversation.ts:530`、`utils/dm-helpers.ts:64`
- CONV-023 置顶上限 `DM_CONV_PINNED_CAP = 50`：写路径（pinned false→true）在用户行锁内计数超限即 400 `最多置顶 50 个会话`；**读路径不设限**——存量超 50 的置顶必须全部可见（legacy over-limit 容忍）。`controllers/conversation.ts:1154`、注释 `controllers/conversation.ts:602`
- CONV-024 游标：`base64url("${lastMessageAtIso}|${documentId}")`；`convCursorWhere` 用 `$or[ lastMessageAt < iso, lastMessageAt = iso AND sortKey < docId ]` 保证同毫秒不丢不重；解码失败视为「未传游标」走第一页。`utils/dm-helpers.ts:132`、`utils/dm-helpers.ts:207`
- CONV-025 游标破平键 `convIdSortKey`：伪会话 id（`pseudo:*`）归一为 `￿(U+FFFF) + senderKey`，即 pseudo 排在同毫秒所有真会话之前；pseudo 之间比的是换算回的 senderKey 而非 pseudo id 本身（跨 category 字典序不一致：`pseudo:anonymous < pseudo:system < pseudo:user` vs `anonymous < contacts < other:system`）。`utils/dm-helpers.ts:183`
- CONV-026 第一页 `meta.totalUnread` = 全量 DM 未读合计 + 全量桶未读合计（跨全部会话，不止本页）；后续页不下发 totalUnread，前端靠 WS 事件增量维护红点。`controllers/conversation.ts:786`
- CONV-027 未读口径（`computeUnreadByConv`）：`COUNT(DISTINCT message)` where 会话∈本人参与且在场、`sender != 本人`、`deletedAt IS NULL`、且（本人 `lastReadAt IS NULL` 或 `createdAt > lastReadAt`）；**水位从 DB 的 `conversation_members.last_read_at` join 出来，不信入参**；只返回未读>0 的会话；rowLimit（列表 5000 / 序列化 2000）只是分组数兜底，触顶按未读倒序保大头 + warn。`controllers/conversation.ts:436`
- CONV-028 未读聚合失败时 **fail-open 按全 0 返回**（不 500），但：① 打按天计数器 `ik:dm:unread-degraded:YYYY-MM-DD`（TTL 7 天，供告警轮询）；② 通过 `onDegraded` 阻止本次 payload 写入 30s 列表缓存。`controllers/conversation.ts:384`、`controllers/conversation.ts:798`
- CONV-029 第一页且 `limit==20` 的响应写 Redis 缓存 `dm:conv-list:{b0|b1}:{userId}`（TTL 30s，`DM_CONV_LIST_TTL_SEC`）；键带读路径车道（b0=读时聚合/b1=写时聚合），失效时**三条键一起删**（含分道前的历史键 `dm:conv-list:{userId}`）。非默认页大小不读不写缓存。「缓存分道」属迁移期产物，重写时不需要。`utils/dm-helpers.ts:28-60`
- CONV-030 通知桶快照缓存 `dm:notif-buckets:{userId}`（TTL 60s）与列表缓存分离：发消息只清列表，桶快照只在「新通知/标已读/一键已读/建会话/leave」时失效（`invalidateDmNotifCache` = 列表三键 + 桶快照）。`utils/dm-helpers.ts:85-114`
- CONV-031 会话摘要序列化：direct 的 `peer` 取「除自己外的参与者」（**含已 left 的历史成员**——对端退出后仍显示其身份与头像）；`memberCount` 只数在场成员；`title` direct 为 `conv.title ?? peer.name`（AI 会话有 title）、group 为 conv.title；头像 direct 取 peer（装备头像优先于 author.avatar）、group 取 conv.avatar。`controllers/conversation.ts:180-253`
- CONV-032 最后一条消息预览：`kind=system` → 截断正文；`deletedAt` 非空 → 固定文案 `[消息已撤回]`；`kind=image` → `[图片]`；其余截 80 字符加 `…`。`controllers/conversation.ts:67-73`、`215-231`
- CONV-033 `update`：muted/pinned 本人可改；title 仅「群聊 owner|admin」或「direct 且对端是 AI」可改（否则 400 `only ai sessions can have title`）；`aiModelKey` 仅 AI 私聊可设，须为启用模型且在该角色卡白名单内，`null`/`''` 清除回落角色卡默认。会话级改动推 `conversation.updated` 给全部在场成员。`controllers/conversation.ts:1056`
- CONV-034 `markRead`（真会话）：`lastReadAt=now()`；**direct 会话额外把对端发给本人的全部 contacts 桶通知标已读**（`notifMarked`，「点开会话=既看了 DM 也看了通知」）；推 `conversation.read {lastReadAt, notifMarked}` 给本人所有端。`controllers/conversation.ts:1192`
- CONV-035 `markRead`（伪会话）：只对桶内未读通知批量置 `isRead=true`（500 条/批、id 游标、`createdAt <= 调用时刻` 防并发新通知被误标），无成员行可写；推 `conversation.read {lastReadAt, updatedCount}`。`controllers/conversation.ts:1199`、`notification-merge.ts:597`
- CONV-036 `markAllRead`：推进本人全部在场 member 的 `lastReadAt`（findMany 上限 5000 条 member 行）+ 全部未读通知置已读（候选上限 5000）；推 `conversation.read.all {at, dmUpdated, notifUpdated}`。`controllers/conversation.ts:1300`
- CONV-037 `resetContext`（仅 AI 会话）：事务内行锁 + `contextEpoch = COALESCE(contextEpoch,0)+1` + 写一条 `kind=system`、`content='[对话已重置]'` 的分界消息（广播 `message.created`），并停止所有旧 epoch 的未完成生成。不删任何历史；AI 侧只读分界之后的消息。`controllers/conversation.ts:1428`、`utils/agent/dm-reset.ts:7`
- CONV-038 `contextEpoch` 初始化：缺列值时**一次性**从 legacy Redis 键 `ik:agent:session-epoch:dm:{convDocId}` 收养（Redis 读失败绝不回落 0——那会重开重置前的私密历史）。「Strapi/迁移特有，重写时不需要 legacy 收养，但语义是 epoch 只能前进。」`utils/agent/dm-reset.ts:14`
- CONV-039 `aiStop`：校验消息是「AI 发的 text 且未删除」（400 `not an AI streaming message`）且调用者是在场成员，写 Redis 停止标志 `ik:agent:stop:{messageDocId}`（EX 300）；`aiRegenerate`：只允许重生成「会话最后一条 AI 回复」（按最近 30 条 text 判定），先成功入队（带 `regenerate: 旧消息docId`）才软删旧回复并广播 `message.deleted`——入队失败则旧回复保留。无因果链的 legacy 回复只在「前一条恰好是唯一连续用户消息」时才推断触发消息，否则 400。`controllers/conversation.ts:1486`、`1531`

### 缓存/副作用一览

- CONV-040 缓存键：`dm:conv-list:{lane}:{uid}`（30s，仅第一页默认页大小）、`dm:notif-buckets:{uid}`（60s）、`dm:conv:{conversationId}`（60s，单会话元信息）、`knock:conv:{uid}`（30s，敲敲会话列表）。所有 Redis 操作 fail-open（异常静默继续）。`utils/dm-helpers.ts:18-62`

---

## 2. 消息（MSG）

### 数据与约束

- MSG-001 `kind` ∈ `text | image | system`（默认 text）；`content` ≤ 4000 字符（`DM_CONTENT_MAX`，schema maxLength 4000 同步）；`attachments` 多图（仅 images）——**当前发送路径完全不写 attachments**：`kind=image` 只靠 kind 标记 + 空 content 也允许（text 必须有非空 trim 后内容）。「待确认」image 消息的完整契约（前端当前似未上线发图）。`message/schema.json:35`、`controllers/message.ts:240`
- MSG-002 私有字段（不下发）：`agentJobId`（唯一，幂等）、`generationEpoch`、`streamFinishedAt`、`quotaReceipt`、`triggerMessage`；下发字段含 `workflow`（脱敏后的 AI 工作流事件）与 `aiModelKey`。`message/utils/serialize.ts:51`
- MSG-003 `editedAt` / `deletedAt` 均为 datetime；`deletedAt` 非空即「已撤回」的软删语义：内容对所有人不可见（序列化 `content=null`、预览显示 `[消息已撤回]`），但行保留且仍出现在消息流里。`message/utils/serialize.ts:54`

### 流程与权限

- MSG-010 历史 `GET /dm/conversations/:id/messages`：倒序（createdAt desc, documentId desc）返回，`before=base64url("${createdAtIso}|${documentId}")` 严格早于游标；limit 默认 50、上限 100；游标落在 `notif:` 开头的伪消息上时同毫秒真消息全部保留（同 CONV-025 的思路）。`controllers/message.ts:47`
- MSG-011 **真 direct 会话的消息流会混入对端发给本人的通知**（序列化为 `kind:'notification'`、`documentId:'notif:<docId>'` 的伪消息），按 createdAt desc 归并、同毫秒 `notif:` 排在真消息之前；**AI 会话不混入**（否则新 AI 会话被历史评论通知污染）、group 不混入；翻页时通知也按同一游标分页。混入失败会向上抛（message.list 与 CONV 列表的 fail-open 口径不同）。`controllers/message.ts:151-190`
- MSG-012 伪会话的消息流：一次性拉对应桶的通知（`limit+1`），支持 `before` 游标分页（`documentId` 破平），无真实消息。`controllers/message.ts:77`
- MSG-013 `send`：要求在场成员；direct 会话先查双向拉黑（403 `cannot send message to blocked user`）；replyTo 必须存在、属同一会话、未撤回（否则 400 `replyTo not found / not in this conversation / has been withdrawn`）。`controllers/message.ts:197`
- MSG-014 发送写路径（事务内）：建消息 → `advanceConversationActivity`（推进 lastMessageAt + lastMessage）→ text 消息尝试入队 AI DM 任务（`maybeEnqueueDmAgentJob`：仅 text 非空、发送者非 AI、对端是绑定角色卡的 AI 时才真正入队）；事务后序列化并向**全部在场成员（含发送者本人，多端同步）**推 `message.created {message}`。`controllers/message.ts:271`
- MSG-015 `edit`：仅 sender 本人（403 `not your message`）、未撤回、仅 text、`Date.now() - createdAt <= DM_EDIT_WINDOW_MS`（5 分钟，`DM_EDIT_WINDOW_MS = 300000`）；新内容 trim 后非空且 ≤4000；无变化直接 ok（不产生 editedAt）。推 `message.edited {content, editedAt}`。**刻意不失效列表缓存**：lastMessage 预览最多滞后 30s。`controllers/message.ts:327`、`utils/dm-helpers.ts:75`
- MSG-016 `withdraw`：仅 sender、5 分钟窗（`DM_WITHDRAW_WINDOW_MS = 300000`）、**幂等**（已撤回再撤回返回 ok）；行锁 + 条件更新（`deletedAt IS NULL`）防并发；撤回后停止以该消息为 trigger 的未完成 AI 生成；失效成员列表缓存并推 `message.deleted {deletedAt}`。`controllers/message.ts:385`
- MSG-017 replyTo 序列化：`{documentId, content: 已撤回?null:截80, senderUserId}`。`message/utils/serialize.ts:72`

### 限流

- MSG-020 发送双桶固定窗口限流：`dm:send-rate:{uid}:s` 每秒 5 条（`DM_SEND_PER_SEC`）、`dm:send-rate:{uid}:m` 每分钟 60 条（`DM_SEND_PER_MIN`）；超限 429 `{name:'TooManyRequests', message:'发送过于频繁，请稍后再试', details:{retryAfterSec}}`（取桶剩余 TTL，兜底 1/60）；**Redis 故障 fail-open 不限流**；`incrWithTtl` 原子自增补 TTL（两段式会永久卡死用户）。`utils/dm-helpers.ts:502`、`70-72`

---

## 3. WebSocket 实时通道（WS）

### 连接与鉴权

- WS-001 路径仅 `/dm/socket`（挂在主 HTTP server 的 upgrade 上，其它路径放行）；**不接受 URL 传 JWT**，只接受 `?ticket=`。`utils/ws-server.ts:22`
- WS-002 ticket 两段式：`POST /api/dm/socket/ticket` 要求 Bearer JWT 且与登录用户一致、未被封禁/未确认，签发 24 字节随机 base64url ticket（约 32 字符）存 Redis `dm:ws:ticket:{ticket}`，**TTL 30s（`TICKET_TTL_SEC`）、一次性**（GET+DEL 原子 Lua），响应 `{ticket, ttlSec:30}`；Redis 故障返回 503。ticket 只存 `{userId, sid, expiresAt}`（identity），校验时要求未过期。`utils/ws-auth.ts:16`、`controllers/conversation.ts:1614`
- WS-003 握手时二次校验 `isRealtimeIdentityActive`：JWT 未过期 + 用户存在且未 `blocked`、`confirmed!==false`、`accountStatus ∉ {blocked,merged,deleted}` + 有 sid 时会话仍在 `auth-session` 里活跃。失败回 HTTP 401 `invalid ticket`。`utils/auth-revocation.ts:57`、`utils/ws-server.ts:71`
- WS-004 单用户连接上限 `CONNECTIONS_PER_USER = 5`，超出按 FIFO 淘汰最老连接并 `close(1000, 'evicted')`。**进程内 Map，多实例不共享**「单节点限制，重写时若要多实例需 Redis pub/sub」。`utils/ws-bus.ts:43-85`
- WS-005 发送背压：`ws.bufferedAmount > 256KB`（`MAX_BUFFERED_BYTES`）直接 `terminate()`。`utils/ws-bus.ts:46`
- WS-006 心跳：服务端每 25s（`HEARTBEAT_MS`）发 WS ping；任何 pong / 客户端消息刷新活跃时间；**60s（`IDLE_TIMEOUT_MS`）无活跃 terminate**（注意：文件头注释写「30s 没收到 pong 则关闭」，与实现 60s 不一致，以代码为准）。`utils/ws-server.ts:25-28`、`108`
- WS-007 单条客户端消息上限 8KB（`MAX_MESSAGE_BYTES`，ws `maxPayload` + 应用层 double-check，超限回 `error {reason:'message too large'}`）；二进制帧忽略；非法 JSON 回 `error {reason:'invalid json'}`；未知 type 回 `error {reason:'unknown type: X'}`。`utils/ws-server.ts:31`、`126-181`
- WS-008 连接即注册实时吊销：`registerRealtimeConnection` 给每条连接 ①绝对寿命 = min(JWT exp, 30 分钟 `MAX_CONNECTION_MS`)；②每 30s（`RECHECK_MS`）DB 权威复查（10s 超时未决视为失效，fail-closed 关连接）；③订阅 Redis 频道 `ik:auth:realtime-revoke`，按 `{userId, sid?, exceptSid?}` 精确关闭。登出/踢设备/改密等由 auth-session 服务在事务提交后 publish。SSE 敲敲流同样注册。`utils/auth-revocation.ts:12-97`、`api/auth-session/services/auth-session.ts:65`

### 客户端→服务端操作

- WS-010 `ping` → 回 `pong`；`typing {conversationId}` → 服务端 1s/连接节流 + 校验发送者是该会话在场成员后，向**其它在场成员**推 `typing {userId}`；`subscribe`/`unsubscribe` 是保留协议位，当前**完全不生效**（所有事件按 user 维度推）。`utils/ws-server.ts:150-174`、`utils/dm-typing.ts:15`
- WS-011 握手成功立即发 `hello {userId}`。`utils/ws-server.ts:194`

### 服务端→客户端事件（DM WS 通道）

| 事件 type | 触发 | 接收者 | data 载荷 |
|---|---|---|---|
| `hello` | 连接建立 | 该连接 | `{userId}` |
| `pong` | 收到 `ping` | 该连接 | — |
| `error` | 消息过大/非法 JSON/未知 type | 该连接 | `{reason}` |
| `message.created` | 发消息（MSG-014）；AI 流式开始（`data.streaming:true` + 空 content 占位）；AI 上下文重置分界消息；**新通知**（conversationId 为真会话或 pseudo id，message 为 `kind:'notification'` 伪消息） | 会话全部在场成员（含本人）/ 通知收件人 | `{message}`（完整序列化消息） |
| `message.delta` | AI 流式增量（不落库） | 会话在场成员 | `{content}`（**累计**全文，非增量片段） |
| `message.workflow` | AI 工作流事件（不落库，脱敏后） | 会话在场成员 | `{event}` |
| `message.edited` | 消息编辑（MSG-015）；AI 流式定稿（额外带 `message` 与 `streamingDone:true`） | 会话在场成员 | `{content, editedAt, message?, streamingDone?}` |
| `message.deleted` | 撤回（MSG-016）；AI 重生成软删旧回复 | 会话在场成员 | `{deletedAt}` |
| `conversation.read` | markRead（真/pseudo） | 本人所有端 | `{lastReadAt, notifMarked?}` 或 `{lastReadAt, updatedCount?}` |
| `conversation.read.all` | 一键已读 | 本人所有端 | `{at, dmUpdated, notifUpdated}` |
| `conversation.updated` | 会话 title/aiModelKey 变更 | 会话全部在场成员 | `{title?}` / `{aiModelKey?}` |
| `conversation.member.removed` | 成员 leave | 其它在场成员 | `{userId}` |
| `conversation.member.added` | **已声明在事件类型枚举里但无任何发送点**「死协议位」 | — | — |
| `typing` | 对端 typing 上送 | 会话其它在场成员 | `{userId}` |

事件信封统一为 `{type, conversationId?, messageId?, data?, at}`（at = ISO 时间戳）。`utils/ws-bus.ts:15-41`

### 前端 UX 意图（供新网关契约参考）

- WS-020 前端单例连接、断线指数退避 1s→32s、连失败 6 次放弃；应用层每 20s 发 `ping`；收到 close code 非 1000/1001 时记日志（1006=网络，1008=被拒）。typing 展示 3s TTL；AI 定稿后 1.5s 内忽略残余 typing。`app/composables/useDmStream.ts:24-29`、`useDmConversations.ts:180-182`
- WS-021 前端在 `message.created` 且本地只有对应 `pseudo:user:N` 项时做就地「实质化迁移」（通知历史并入真会话、active 切换、删 pseudo 项）——新网关若保持 pseudo id 设计，需保留足够字段让前端能做这个迁移。`app/composables/useDmConversations.ts:955-980`

---

## 4. 通知（NOTIF）

### 数据与类型

- NOTIF-001 字段：`type` 枚举 `comment|reply|like|favorite|mention|system|denny|accepted`；`message`（≤500 字符，仅 system 类有正文，其余为 null）；`isRead`（默认 false）；`recipient`/`sender`（user 多对一）；`Article`/`Comment`（多对一，可空）；`bucketKey`（私有，写时聚合的反范式**裸 senderKey**）。`notification/schema.json:14`
- NOTIF-002 `beforeCreate` 校验：recipient 与 sender 均存在且不等时，若双向任一方向拉黑则**抛错** `'Blocked user cannot receive notification'`（会中止创建；调用方多数已提前挡拉黑或 catch）。`notification/lifecycles.ts:253`
- NOTIF-003 **自我通知抑制在各触发点**（recipient===sender 直接不发）；lifecycle 不自查。「新实现应在通知服务层统一强制。」

### 触发点与文案（精确模板）

| type | 触发点 | recipient | 触发条件与抑制 | 列表预览 / 消息文案 |
|---|---|---|---|---|
| `comment` | 对帖子发顶层评论（comment afterCreate） | 帖子作者 | 评论/帖子任一匿名则不发；自评不发；对帖主有拉黑关系时评论本身被拒绝 | 评论正文（剥离 `@[name](id)`→`@name`、表情 token→`[表情:name]`） |
| `reply` | 回复评论 | 父评论作者 | 父评论必须可见、顶级、同帖；链上任一匿名则不发；自回复不发；对父作者拉黑时评论被拒绝 | 同上 |
| `like` | 点赞帖子或评论 | 帖子作者 / 评论作者 | 目标匿名不发；自赞不发；点赞对拉黑目标的动作在更早被拒 | `赞了你的帖子` / `赞了你的评论`（Comment 关联存在即后者，即使那评论是收件人自己写的） |
| `favorite` | 收藏帖子 | 帖子作者 | 匿名帖不发；自收藏不发 | `收藏了你的帖子` |
| `mention` | 评论中 `@[name](id)`（裸 `@username` 会先展开为 token） | 被提及用户 | 匿名评论/匿名帖/匿名父评论不发；单条评论最多解析 10 个；跳过自己、跳过已会收到 comment/reply 通知的主收件人、跳过被封禁用户、同评论同收件人去重 | `在帖子中提到了你`（notificationToDmMessage）/ 评论正文（previewOf 路径） |
| `denny` | 给帖子投币（1 币） | 帖子作者 | 匿名帖不能投币、自己帖子不能投、双向拉黑不能投（均报错而非静默）；**事务提交后**才建通知（扣款成功但通知失败只 warn 不回滚） | `给你的帖子投了币` |
| `accepted` | 评论被采纳为最佳答案 | 被采纳评论作者 | 采纳自己的评论不发；评论或帖匿名不发；创建失败只 warn | `采纳了你的评论为最佳答案` |
| `system` | ① 举报处理（自动隐藏/管理员结案/举报人回执）② 关注扇出（发帖通知关注者）③ `sendSystemNotification` 工具（当前无生产调用方） | 内容作者/举报人/关注者 | 举报通知跳过 accountStatus∈{deleted,merged} 的用户；关注扇出跳过 blocked/未 confirmed/AI 账号/双向拉黑/作者本人，幂等键 7 天 | 见下方模板 |

system 类精确文案（截 500 字符；`处理备注：` 后缀可拼接）：
- 自动隐藏：`你的{委托《title》|评论}因收到多人举报已被暂时隐藏，等待管理员复核。` `api/report/services/report.ts:222`
- 管理员隐藏：`你的{label}因违反绳网社区规范已被隐藏。如有异议请联系管理员。` `report.ts:290`
- 管理员删除：`你的{label}因违反绳网社区规范已被删除。如有异议请联系管理员。` `report.ts:306`
- 警告：`请注意：你的{label}收到举报且经核实存在不当之处，请遵守绳网社区规范。` `report.ts:322`
- 封号：`你的账号因违反绳网社区规范已被封禁。如有异议请联系管理员。` `report.ts:335`
- 举报人回执：`你举报的{label}已处理完毕，感谢你维护绳网环境！` 或驳回时 `你举报的{label}经核实暂未发现违规，感谢你维护绳网环境。`；`{label}` = `委托《title》` / `评论` / `账号`。`report.ts:296`、`361-366`
- 关注扇出：`你关注的 {authorName} 发布了新委托《{title}》`（name/title 各截 100）。`api/article/utils/follow-notification.ts:279`

### 序列化与隐私

- NOTIF-010 匿名掩码：匿名桶内 sender 一律替换为 `{id:null, username:'绳网用户#XXXX', level:null, author:{documentId:null, name:'绳网用户#XXXX', avatar:null}}`；`#XXXX` = `sha256(anonymousSeed)` 前 4 位 hex 大写；seed 缺失时回退固定名 `绳网用户#` + sha256('anonymous-fallback') 前 4 位。**装备头像也必须置空**（头像能反查身份）。`utils/anonymous.ts:6-28`、`notification/utils/serialize.ts:104-124`
- NOTIF-011 like→favorite 升级显示：历史 `type=like` 且关联 Article（非 Comment）的通知，若 sender 当前对该文章存在 favorite 记录，则下发 `type:'favorite'`、`rawType:'like'`。「历史数据纠偏，重写迁移后可能不需要。」`notification/utils/serialize.ts:58-99`
- NOTIF-012 通知下发字段：`documentId, type, rawType, isRead, createdAt, message, sender, article{documentId,title,coverAspectRatio}, comment{documentId,content,images,isAnonymous}`；`coverAspectRatio` 取封面首图宽/高。`notification/utils/serialize.ts:23-33`
- NOTIF-013 身份边界（测试钉死）：contacts 桶的 SQL 过滤**必须排除匿名评论**（`Comment.isAnonymous` 为 true 的永远不许经真实 sender 的桶被读到，即使 DTO 已脱敏）；anonymous 桶按 `anonymousSeed` 过滤且标已读时不得跨 seed 误标；匿名目标的 like（如点赞匿名评论）**仍归 contacts 桶**（`Comment.isAnonymous` 指的是「被赞的评论」，与点赞者身份无关）。`notification/utils/conversation.ts:95-131`、`tests/unit/messaging-privacy.test.ts`
- NOTIF-014 AI sender 不单独成桶：sender 为 `isAiAgent` 的 contacts 桶在列表侧整条剔除（也不计入 totalUnread）；通知照样存库，只是融进 AI 会话本身或不可见。WS 推送侧同样：无真会话时 AI 通知不推 pseudo 桶。`notification-merge.ts:742`、`notification/lifecycles.ts:197`
- NOTIF-015 四种「无桶」情形（不进任何桶、不计未读、不出现在列表）：缺 type / 未知 type / 匿名缺 seed / 缺 sender——与 `classifyNotification` 返回 null 同口径。`notification/utils/bucket.ts:100`

### 分桶规则（category / senderKey / bucketKey）

- NOTIF-020 分类（`classifyNotification`）：评论类 `comment|reply|mention` —— Comment 匿名且有 seed → `anonymous:{seed}`，否则按 sender → `contacts:{senderId}`；交互类 `like|favorite|denny|accepted` —— 永远 `contacts:{senderId}`（不看 Comment.isAnonymous）；`system` → `other:system`。`notification/utils/conversation.ts:154-189`
- NOTIF-021 `senderKey = "${category}:${peerKey}"`（只切第一个冒号，seed 可含冒号）；`bucketKey = "u{recipientUserId}:{senderKey}"`（带收件人前缀保证全局唯一）；**notifications.bucket_key 列存裸 senderKey，notification_buckets.bucket_key 存带前缀键——两者同名不同格式**，曾写反导致每个伪会话 unreadCount 恒 0。`bucket.ts:86-104`、`172-179`
- NOTIF-022 桶行（`notification_buckets`）全标量无关系：`recipientUserId, bucketKey(唯一), senderKey, category, lastAt, preview, lastNotificationId`；**刻意没有 unreadCount**——未读由 `WHERE recipient=? AND is_read=false AND bucket_key IS NOT NULL GROUP BY bucket_key` 现算（已读/删除入口有 8 处，维护型计数器必漂）。`notification-bucket/schema.json`、`bucket.ts:14-21`
- NOTIF-023 桶 upsert **只前进不后退**：`UPDATE ... WHERE bucket_key=? AND last_at < ?`，等时不写（同毫秒先到先得）；并发撞唯一约束收敛为重试 advance。`bucket.ts:297-337`
- NOTIF-024 桶 preview 物化上限 `BUCKET_PREVIEW_MAX=200`（列是 varchar(255)，评论正文可到 2000），截断时不得把 UTF-16 代理对切成半个；读时聚合路径用**不截断**的同函数保持逐字节一致。`bucket.ts:57`、`250-260`
- NOTIF-025 桶回退（通知被删）：按 `lastNotificationId` 做乐观锁，把桶拨回「桶内剩余最新一条」；桶空则删桶行（防幽灵伪会话）；被删的不是最新一条则不动；时间无法解析则不动；并发新通知推走了桶则放弃（conflict）。单批上限 `BUCKET_RECOMPUTE_CAP=200`，逐桶兜错不抛。调用点：取消点赞、删评论、级联删楼中楼。`bucket.ts:436-584`
- NOTIF-026 桶未读聚合分组上限 `NOTIF_BUCKET_UNREAD_GROUP_CAP=2000`，触顶偏小并 warn；失败 fail-open 全 0 + onDegraded（不写缓存）。`notification-merge.ts:1138-1208`

### 通知接口（旧契约，语义需保留）

- NOTIF-030 `GET /notifications/list`：offset 分页（start 默认 0，limit 默认 20、上限 50），`isRead=true|false` 过滤；**未登录返回空列表**（不 401）。`controllers/notification.ts:185`
- NOTIF-031 `GET /notifications/unread-count`：未登录返回 `{count:0}`。`controllers/notification.ts:259`
- NOTIF-032 `PUT /notifications/:id/mark-read`：`:id` 接受数字 id 或 documentId；仅限本人通知；推 SSE `notification.read`。`controllers/notification.ts:273`
- NOTIF-033 `PUT /notifications/mark-all-read`：候选上限 5000 条；推 SSE `notification.read.bulk {count}`。`controllers/notification.ts:311`

### 生命周期投递（关键时序）

- NOTIF-040 通知 `afterCreate`：经 `runAfterCommit` 保证在最外层事务 COMMIT 后才执行（回滚则不执行）——旧实现曾因 `setImmediate` 落在「事务已置空、COMMIT 未落」窗口导致 denny 通知的桶键与推送静默丢失。**新实现必须保留「投递发生在提交后」这一语义**。`notification/lifecycles.ts:40-74`、`utils/after-commit.ts`
- NOTIF-041 投递内容（同一趟 populate 完成四件事，顺序固定）：① 桶双写（桶行 upsert + `notifications.bucket_key`，**先于缓存失效**，否则并发列表请求会把「没这个桶」缓存住）；② 失效 `knock:conv:{recipient}` + DM 列表/桶快照缓存；③ 老 SSE `notification.created {conversationId, notificationId}`；④ DM WS `message.created`（conversationId 取真 direct 会话 docId，无则 pseudo id；**有真会话时 bump 该会话 lastMessageAt**，bump 后再失效一次缓存修序）。`notification/lifecycles.ts:76-251`
- NOTIF-042 通知的删除路径：取消点赞删除（sender,recipient,Article/Comment）匹配的**全部** like 通知（函数名叫 removeUnreadLikeNotification 但实际无 isRead 过滤——已读的也删，重写时保留或修正需「待确认」）；删评论级联删该评论衍生的 comment/reply/mention/like 通知及楼中楼衍生通知。删除后必须 recompute 桶 + 失效缓存。**重新点赞会产生一条全新通知**。`like/lifecycles.ts:313`、`comment/lifecycles.ts:575`

---

## 5. 敲敲与统一收件箱融合（KNOCK）

### 伪会话

- KNOCK-001 伪会话 id：`pseudo:user:{senderId}` / `pseudo:anonymous:{seed}` / `pseudo:system`；`other:{senderId}` 等历史脏形态一律不映射（防两个 senderKey 合并成一个 pseudo:system）；contacts 的 peerKey 必须纯数字。`notification-merge.ts:78-141`
- KNOCK-002 伪会话项形状与真会话项逐字段一致：`{documentId, kind:'direct', title:null, avatar:null, peer{userId,authorDocumentId,name,avatar,level}, memberCount:2, lastMessageAt, lastMessage{documentId:'notif:<docId>', content, createdAt, kind:'notification', senderUserId}, unreadCount, self:{role:'member',muted:false,pinned:false,lastReadAt:null}, pseudoKind}`。`notification-merge.ts:403-478`
- KNOCK-003 系统消息对端展示名固定 `Fairy`（`SYSTEM_PEER_NAME`），头像取环境变量 `SYSTEM_NOTIFICATION_AVATAR`（可空，前端回退默认头像）；匿名对端名为 `绳网用户#XXXX`。`notification/utils/conversation.ts:20-28`

### 列表归并（统一收件箱语义）

- KNOCK-010 归并规则（读时聚合 `mergePagedDmWithBuckets`，写时聚合 controller 内同口径）：① contacts 桶且**本页（含置顶）存在同 peer 的真 DM 项** → 合并进那一行（unread 累加；通知 lastMessage.createdAt 比真消息新时换预览为通知伪消息）；② 有真 DM 但不在本页 → 跳过（等它那页合并，不允许 pseudo 与真 DM 同时出现）；③ 其余桶 → 独立 pseudo 项按同一游标序参与归并截断；有游标时只保留严格早于游标的。`notification-merge.ts:953-1011`、`controllers/conversation.ts:706-730`
- KNOCK-011 合并进真 DM 项时**比较预览新旧用 `lastMessage.createdAt` 而非 `lastMessageAt`**（后者可能已被通知 bump）。`notification-merge.ts:946`
- KNOCK-012 写时聚合（`NOTIF_BUCKETS_ENABLED=1|true|yes` 才开读侧；**写侧双写永远开启**）的读路径：桶表按 `(lastAt desc, senderKey desc)` SQL 游标分页，填页循环最多 `NOTIF_BUCKET_FILL_WINDOW_CAP=5` 窗（滤掉的 AI 桶/有真 DM 的桶会造成稀疏页）；本页真 DM 对端的桶按 senderKey 精确补取（extras，**不进游标流**）；游标只推进到「已处理完」的行（`windowTail`）。`notification-merge.ts:1545-1717`、`utils/notif-buckets-flag.ts:16`
- KNOCK-013 `nextCursor` 由 `pickNextConvCursor` 决定：hasMore 为假 → null；只许取游标流（不含置顶）中最后一个**有 lastMessageAt** 的项；写时聚合下与 `windowTail` 比较取**较新者**（windowTail 是下界不是兜底，取错会让中间的桶永久不可达）；hasMore 为真却取不到游标时必须 warn 并把 hasMore 压 false。「这套规则全是线上踩过的坑，新实现的双源归并必须等价。」`notification-merge.ts:843-899`
- KNOCK-014 读时聚合路径：扫通知按桶聚合（单批 `NOTIF_MERGE_AGGREGATE_CAP=500`，循环翻页直到取尽并防卡死检测），产出带 `hasRealDm` 标记的快照并缓存 60s。已知缺陷：**>500 条通知的用户老桶在旧版曾静默丢失**（现已循环翻页修复）；「读时聚合整体是迁移期产物，重写直接采用写时聚合（桶表）模型即可。」`notification-merge.ts:74`、`713-779`
- KNOCK-015 桶行 lastNotificationId 指向的通知已被删时：伪会话项 `lastMessage=null` 但 `lastMessageAt` 必须保住桶行 lastAt（否则该项排最末被截断且游标越过后永不再现，未读却照算）。`notification-merge.ts:441-469`、`1441-1490`

### 敲敲旧接口（语义保留，路径可弃）

- KNOCK-020 `GET /knock/conversations`：扫最近 5000 条（`CONVERSATION_AGGREGATE_CAP`）通知内存分桶，返回 `{id: "category:base64url(peerKey)", category, peerKey, peerName, peerAvatar, unread, lastAt, lastPreview, lastType}` 按 lastAt desc；超限 `meta.truncated=true`；结果缓存 `knock:conv:{uid}` 30s。**未登录返回空列表**。`controllers/notification.ts:345-450`
- KNOCK-021 `GET /knock/conversations/:id/messages`：游标分页（limit 默认 50 上限 100；拉 `limit*3+1` 行再经 `matchesConversation` JS 二次过滤防 SQL/内存分类漂移）；返回 **createdAt 升序**（与 DM 消息流的 desc 相反）；`hasMore` 还要算上「取满缓冲但过滤后不足」的情形。`controllers/notification.ts:452-557`
- KNOCK-022 `POST /knock/conversations/:id/mark-read`：同分类口径标已读（扫描上限 5000），实际更新 >0 才推 SSE `notification.read.bulk {conversationId, count}` 并失效缓存。`controllers/notification.ts:563-631`
- KNOCK-023 敲敲会话 id 编解码：`"${category}:${base64url(peerKey)}"`（旧 URL-safe 形态，与 pseudo id 是两套编码，新设计统一为 pseudo 形态即可）。`notification/utils/conversation.ts:47-74`

### 敲敲 SSE（旧通道，语义并入新网关）

- KNOCK-030 `GET /api/knock/stream`：`auth:false`，controller 手动验证 `Authorization: Bearer` **或 `?token=<jwt>`**（EventSource 不能带 header，Android 用 query）；identity 校验同 WS-003。响应 `retry: 5000`、hello 事件、25s 注释行心跳（`:hb <ms>`）、关闭前发 `bye` 事件；`X-Accel-Buffering: no`。`controllers/notification.ts:639-714`
- KNOCK-031 单用户 SSE 订阅上限 5（`SUBSCRIBER_LIMIT_PER_USER`），FIFO 淘汰时经 evict 回调主动关旧连接（客户端收到 bye 后不再重连）；`res.writableLength > 256KB` 主动断连（背压）。`sse-bus.ts:37`、`controllers/notification.ts:700`
- KNOCK-032 SSE 事件：`notification.created {conversationId?, notificationId?, at}` / `notification.read {notificationId, at}` / `notification.read.bulk {conversationId?, count, at}`；hello/bye 为连接级事件。前端对连续 3 次连接失败主动放弃重连。`sse-bus.ts:10-25`、`app/composables/useKnockKnockConversations.ts:95-103`
- KNOCK-033 进程内订阅表，多实例需换 Redis pub/sub「单节点限制，重写时不需要沿用」。

### 文章流 SSE（第三条被合并的通道，简录）

- KNOCK-040 `GET /api/articles/stream?category&tag&uid`：**故意匿名可连**（不挂 auth）；频道 = `tag:{slug}` > `c:{slug}` > `latest`；全局订阅上限 5000，满了回 503 + `Retry-After: 30`（不淘汰旧连接）；事件 `new_topic` / `topic_bumped {topicId, categorySlug, tagSlugs[], at}`；触发者本人（凭客户端自报的 `uid`，可伪造但无攻击收益）的连接不推自己触发的事件；只广播已发布且未隐藏的帖子，载荷仅公开字段。心跳 25s、`retry: 5000`、hello `{channel, at}`。`api/article/utils/article-broadcast-bus.ts`、`api/article/controllers/article.ts:1443-1526`
- KNOCK-041 `topic_bumped` 由评论落库 post-commit 触发；`new_topic` 由发布触发。前端据此累计「查看 N 条新帖」（高亮 3s；`bumpedBySelf` 与轮询对账配合）。「属文章区业务，但新网关需要承接这条通道。」

---

## 6. 在线状态（PRES）

- PRES-001 端点：`POST /api/presence/ping`（optional-auth）与 `GET /api/presence/online`（完全匿名，只读不写）。`api/presence/routes/presence.ts`
- PRES-002 心跳身份：登录用户 `user:{userId}`；匿名访客由客户端生成并持久化 `presenceId`（localStorage `presence:id`，UUID），服务端校验 `^[\w-]{1,128}$`，存为 `anon:{presenceId}`。`presence.ts:67-86`
- PRES-003 存储：Redis ZSET `presence:online`，score=毫秒时间戳；**在线窗口 45s（`TTL_MS`）**，判定用开区间下界 `(now-45000` 当场计算（ZCOUNT），与清理时机解耦；清理（ZREMRANGEBYSCORE）按 45s 节流搭在 count 的同一 pipeline 里；key 自身 EXPIRE = 105s。`presence-store.ts:3-31`
- PRES-004 `ping` 一次 pipeline（ZADD+EXPIRE）；`count` 失败时**回退进程内上一次成功值**（在线人数宁可不刷新也不跳成 0）「`lastCount` 是单进程内存，重写时按需保留语义即可」；`recentUserIds` 失败返回空数组。`presence-store.ts:43-122`
- PRES-005 头像堆叠：取最近活跃的**登录用户**最多 64 个扫出 8 个（`AVATAR_SCAN=64`/`AVATAR_LIMIT=8`）有头像者（装备头像优先），缓存在 `presence:avatars` TTL 20s；头像与人数共用同一个 45s 新鲜窗口（避免「人数 3、头像 5 个」）。**无 profileHidden/拉黑过滤**——在线状态是全站公开的。「待确认」新设计是否引入可见性过滤。`presence.ts:10-64`
- PRES-006 前端每 20s ping 一次、页面隐藏时暂停、可见时立即补一次；匿名也可参与。`app/composables/usePresence.ts:12`

---

## 7. Cron 与 bootstrap 自愈

- CRON-001 `reconcile-notification-buckets`（每天 04:47 Asia/Shanghai，与 04:23 的计数对账错开）：扫 `bucket_key IS NULL` 的通知（批 2000、单轮上限 `NOTIF_BUCKET_SCAN_CAP=20000`、游标按 id **降序**前进——脏行沉在低 id 端，降序保证新漏的行先修），LEFT JOIN 四表推导 senderKey 回填裸键（`UPDATE ... WHERE id IN (...) AND bucket_key IS NULL` 防与 afterCreate 双写竞态），再对受影响桶 recompute（按 200/批切片）；三类计数（filled/unclassified/orphaned）都落日志；重入保护；不向调度器抛错。`config/cron-tasks.ts:252-520`
- CRON-002 `repair-notification-bucket-gaps`（每分钟）：环形主键游标扫「有 bucket_key 但缺桶行」的通知（默认 500、上限 2000），用该桶最新一条通知 upsert 补建行并失效该用户缓存；模块级重入保护，无需重启即可自愈。`bucket.ts:586-623`
- CRON-003 bootstrap `ensureNotificationBuckets`（每次启动幂等）：建窗口索引 `(recipient_user_id, last_at DESC, sender_key DESC)`；把有 bucket_key 的通知按 `(user, bucket_key)` 聚合成缺失的桶行（**preview 留空**，读侧按 lastNotificationId 回查现算）；已存在的桶一律不动。迁移只回填 bucket_key 列、桶行聚合刻意留给 bootstrap（Strapi 迁移跑在 schema sync 之前，且迁移登记后不重跑）「Strapi 部署顺序特有，重写时合并为一次迁移即可」。`src/index.ts:252-348`
- CRON-004 bootstrap `backfillConversationLastMessageAt`：`last_message_at IS NULL` 的会话用 `COALESCE(updated_at, created_at)` 回填，幂等。`src/index.ts:217`
- CRON-005 关注扇出队列（每分钟 drain）：发帖产生的 system 通知走 Redis 队列异步扇出，每关注者 7 天幂等键（`claimOnce`，落库失败必须放键重试），drain 侧做全部过滤（匿名/隐藏帖、作者本人、blocked/未 confirmed/AI、双向拉黑），有每轮条数/时限预算。「细节属文章区，但通知模型要支持这条生产路径。」`api/article/utils/follow-notification.ts`

---

## 8. 横切约束与已知坑

- X-001 **拉黑（block）是全站双向闸门**：DM 建立/发送、评论、投币、通知创建（beforeCreate 兜底抛错）、关注扇出，任一方向拉黑即拒。`utils/block.ts:134`
- X-002 **匿名三律**：匿名内容不触发 comment/reply/mention/like/favorite 通知；匿名通知只经 seed 维度的 anonymous 桶可见，绝不许经真实 sender id 被关联出来（SQL 与内存两层过滤）；匿名展示名由 seed 派生且稳定（`绳网用户#XXXX`），头像/id/authorDocumentId 一律置空。
- X-003 全部 Redis 缓存/限流/存在性检查 fail-open（Redis 挂 = 直查 DB + 不限流）；但「降级值不写缓存」是硬规则（CONV-028、KNOCK 分支同理）。
- X-004 所有「读自己刚写的行再做副作用」必须等事务提交（runAfterCommit 语义），否则跨连接读不到未提交行，静默丢推送与桶键。
- X-005 列表/消息的游标分页破平规则（pseudo 排同毫秒最前、senderKey 与 pseudo id 的换算、windowTail 下界）是线上事故换来的，重写换存储模型后仍需等价的「双源归并 + 严格早于游标」语义，并有对应测试。
- X-006 单进程内存态清单（重写时都需要分布式替代或显式接受单节点）：ws-bus 连接表、sse-bus 订阅表、article-broadcast-bus、presence 的 lastCount/lastCleanupMs、bucket.ts 的 repairCursor/repairing、cron 的重入 flag。「Strapi 特有，重写时不需要」照搬。
- X-007 `conversation.lastMessageAt` 会被通知 bump 而 `lastMessage` 不会——**列表排序键与预览内容刻意分离**；另外 bootstrap 历史回填过全部通知的桶，因此桶 lastAt 可能早于会话 lastMessageAt（extras 必须按 senderKey 精确取，不能依赖时间窗）。
- X-008 文档注释声明的限制与实现不符处：ws-server 注释「30s 无 pong 关闭」实为 60s 空闲；`removeUnreadLikeNotification` 名为 unread 实际删全部。「以代码为准。」

---

## 9. 测试中体现的关键用例

- `dm-conversation-paging`：游标 round-trip（documentId 含 `|` 不坏）、畸形游标不抛；同毫秒 pseudo 排最前、pseudo 间按 senderKey 换算破平；`mergePagedDmWithBuckets` 的合并/跳页/置顶不截断/两路挤出标记；`pickNextConvCursor` 三规则（不回落置顶、跳过无时间尾项、windowTail 作下界）。
- `dm-unread-aggregate`：混合水位一次算准；自己发的/已撤回的不计；**只认在场成员行的水位（退群重进不串）**；非成员会话不算；PG bigint 字符串转 number；降级不写缓存 + 按天降级计数器可告警；缓存键三车道失效。
- `messaging-privacy`：contacts 桶绝不泄露匿名通知（真实 sender id 也读不到）；contacts 标已读不动匿名回复、但会标到「匿名目标上的 like」；anonymous 标已读不跨 seed；hidden profile 不泄露动态。
- `messaging-state`：AI 重生成用已存 trigger 而非猜测、入队失败保留旧回复、legacy 歧义回复拒绝猜；建会话事务第二条 member 失败整体回滚；lastMessageAt 与 lastMessage 两个水位独立单调。
- `notification-bucket-write`：三类桶 + 四种无桶情形；桶 upsert 只前进（乱序 5 条收敛到最晚）、同毫秒不写、唯一约束并发收敛；双写两路各自兜错互不拖累。
- `notification-bucket-read`：600 个桶翻页不重不漏；preview 为空按 lastNotificationId 回查现算；本页空也给出跨全部桶的 totalUnread；脏 senderKey 丢行不炸页。
- `notification-bucket-recompute`：删最新一条拨回剩余最新、删较旧不动、删空删桶行、乐观锁按 lastNotificationId（含 NULL 指针的历史行）。
- `notification-bucket-reconcile`：脏行跳过且游标照样前进；孤儿通知（无 recipient 联表行）键照回填桶跳过；`whereNull` 双保险幂等；裸键/前缀键不写反。
- `notification-merge-ai`：AI sender 的 contacts 桶整条剔除、人类 sender 保留、AI 的 system 通知仍入 system 桶。
- `presence-store`：ping 单 pipeline；过期边界（score 恰等于 now-45s 算过期）；清理节流；失败回退上次人数不跳 0。
- `realtime-revocation`：按 sid 精确吊销、exceptSid 保留当前会话、跨进程 Redis pubsub、pubsub 漏消息由 30s DB 复查兜底、复查超时 fail-closed、JWT 到期绝对关闭。
- `denny-notification-after-commit`：投币通知只在事务提交后创建、失败只 warn 不回滚、回滚/作者无 user 不建通知。
- `read-status-watermark`：属文章「已读水位」区（非 DM 已读），此处仅注明边界——它用的是 `up_users` 上的阅读水位列，与 DM `lastReadAt` 是两套独立机制。

## 10. 未决问题（待确认）

1. 「谁可以私信我」的隐私设置旧后端完全不存在（CONV-013）；新设计是否引入（如互关才可 DM）？
2. `kind=image` 消息当前无 attachments 写入路径，仅有 kind 标记与 `[图片]` 预览；发图的完整契约（附件数量/类型上限、content 是否可选）未定。
3. 取消点赞删除**全部**匹配 like 通知（含已读）是函数名与行为的偏差（NOTIF-042）；重写时保留还是改为只删未读？
4. `conversation.member.added` 事件类型已声明但无发送点；群聊整体（创建/角色/转让）从未实现，重写时按新设计补齐即可，无兼容负担。
5. presence 对 profileHidden / 拉黑关系无过滤（全站公开在线状态与头像），是否引入可见性规则？
6. 未登录访问 `/notifications/list` 返回空列表、`/knock/conversations` 返回空列表（而非 401）的兼容语义，新 API 是否保留？
7. muted 当前纯客户端语义；新设计是否让 muted 参与服务端推送抑制（如不进统一收件箱红点）？
