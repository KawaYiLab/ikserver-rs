# 业务规则索引与待裁决清单

本文是 `docs/business-rules/` 下 9 份业务规则规格的总索引，并汇总了其中所有「待确认 / 未决问题 / 矛盾」项，供 Rust 重写时逐项裁决。

- 这 9 份规格从旧 Strapi 5 论坛后端（`InterKnot-web/server`）提取，**以旧系统的实际行为为准**，每条规则都附有旧代码的来源文件与行号。
- 修改任何业务行为之前，先读对应的规格文件；本文只做索引和汇总，不新增规则。
- 第 3 节的「建议默认」只是建议，未经产品确认不能当作已定规则。裁决后应回写到对应规格文件（或 ADR），并在本文标注结论。
- 不同文件之间有同名前缀（`RL`、`CACHE`、`CRON`、`AUTH`），引用时必须带文件名，例如 `media-ops.md` 的 `RL-020`。

---

## 1. 索引

规则数的统计口径：只统计以列表项定义的唯一规则 ID。`comments.md` 中的 `Q-01`～`Q-16`（未决问题编号）和 `auth-account.md` 中的 `MHY-P1`～`MHY-P6`（米游社协议步骤）不计入。9 份文件合计 1340 条规则。

| 文件 | 范围 | 规则数 | 规则 ID 前缀 | 最关键的 3 条规则 |
|---|---|---|---|---|
| [posts.md](business-rules/posts.md) | 帖子（委托）的创建、编辑、草稿、发布、下线、删除，字段与等级限额，分区、标签、匿名、置顶、采纳投影、浏览量、主页精选 | 112 | `POST` | `POST-043` 已发布的匿名帖连作者本人也不能编辑、下线或删除；`POST-100` 编辑后重新发布不顶帖、不通知、不改 firstPublishedAt；`POST-012` 等级权益矩阵是正文字数和图片数量的唯一依据 |
| [feed-search.md](business-rules/feed-search.md) | 信息流（4 种 feed × 4 种 sort）、游标分页、置顶与热门注入、热榜算法、未看流、相关推荐、SSE 新帖流、文章/评论/用户搜索 | 156 | `FEED` `HOT` `UNRD` `READ` `SUGG` `SRCH` `SSE` `AUTH` `CACHE` `RATE` `PRIV` `EDGE` `CRON` | `FEED-013` 游标严格「早于」，排序固定为 `bumpedAt DESC, documentId DESC`；`HOT-002` 热度公式（Discourse TopicHotScore）；`SRCH-013` 搜索命中后一律回查 DB，命中全是残留时转 SQL 兜底 |
| [comments.md](business-rules/comments.md) | 评论创建（正文、图片、表情、@提及、匿名）、单层楼结构、列表与回复分页、搜索、硬删除级联、置顶、采纳、评论点赞、评论产生的通知和经验 | 166 | `CMT` | `CMT-025` 楼中楼只有一层，parent 必须是同帖未隐藏的顶层评论；`CMT-129` 匿名评论、匿名帖、匿名 parent 一律抑制通知；`CMT-048` 删除顶层评论级联删除全部回复及其通知和点赞 |
| [social-economy.md](business-rules/social-economy.md) | 点赞、收藏、关注（含扇出通知）、拉黑及其全站生效点、一键三连、丁尼、经验与等级、签到、权益、装扮、个人资料与作者主页 | 171 | `COMMON` `LIKE` `FAV` `FOLLOW` `BLOCK` `TRIPLE` `DENNY` `EXP` `CHECKIN` `BENEFIT` `COSMETIC` `PROFILE` | `COMMON-004` 互动唯一性由 DB 唯一约束兜底，撞约束视为「已存在」而不是 500；`DENNY-008` 投币固定 1 丁尼、每人每帖终身一次，匿名帖和自己的帖不能投；`EXP-009` 每日经验分主动桶 50、被动桶 1000 |
| [moderation-exam.md](business-rules/moderation-exam.md) | 举报（用户侧）、审核（按目标聚合、领取、结案、删除 outbox）、多人举报自动隐藏、入站考试、题库后台、管理员权力、处罚效果 | 147 | `RPT` `RPTADM` `EXAM` `EXAMADM` `ADMIN` `SANCTION` | `RPTADM-055` 一次结案作用于同目标的整组未结案举报；`RPTADM-073` 删除内容分两阶段（先隐藏并入队，再在审核事务外删除、收尾时发通知）；`EXAM-011` 写操作门控 `EXAM_REQUIRED`，每次请求重查 |
| [auth-account.md](business-rules/auth-account.md) | 账号模型、邮箱验证码（5 种用途）、注册、密码/验证码登录、重置与修改密码、会话旋转与重放检测、米游社扫码、身份模型、注销、认证限流与中间件、邮件双通道 | 146 | `ACC` `CODE` `REG` `LOGIN` `SESS` `LOCK` `MHY` `IDENT` `DEL` `RL` `MW` `EMAIL` | `SESS-011` 已撤销超过 30 秒的 refresh token 再次出现即判定泄露，撤销整个家族；`CODE-009` 验证码消费是单条 CAS，一码只用一次；`CODE-013` 登录与重置发码反枚举，账号状态与「验证码错误」不可区分 |
| [messaging-notifications.md](business-rules/messaging-notifications.md) | 私信会话与消息、DM WebSocket、通知（8 种类型）与分桶、敲敲与统一收件箱融合、在线状态、相关 cron | 120 | `CONV` `MSG` `WS` `NOTIF` `KNOCK` `PRES` `CRON` `X` | `NOTIF-040` 通知投递必须发生在事务提交之后；`NOTIF-013` contacts 桶绝不能读到匿名评论类通知；`KNOCK-010` 统一收件箱的双源归并规则（同 peer 合并、严格早于游标） |
| [ai-assistant.md](business-rules/ai-assistant.md) | AI 配置、模型、角色卡、触发与去重、上下文与代际、配额预占与结算、流式生成、输出清洗、沉浸守卫、工作流事件、12 个只读论坛工具、直连大模型要求 | 161 | `AI` `TRIG` `CTX` `QUOTA` `GEN` `WF` `TOOL` `LLM` | `QUOTA-005` 调用模型前先预占，用量达到上限才拒；`QUOTA-020` 拿不到额度判定时不得调用模型（fail-closed）；`TOOL-004` 工具的查看者身份只来自可信执行上下文，绝不读工具参数 |
| [media-ops.md](business-rules/media-ops.md) | 直传上传（签名、PUT、完成校验与发布）、文件与引用、CDN URL、用户图库、自定义头像、全局限流与客户端 IP、错误格式、响应缓存、cron 与启动任务、邮件调度、后台手工维护内容 | 161 | `UPL` `FILE` `CDN` `MEUP` `AVA` `UPV` `RL` `ERR` `CACHE` `CRON` `BOOT` `MAIL` `ADMINCRUD` `OPS` | `UPL-033` 完成阶段校验大小、SHA-256 与魔数嗅探；`UPL-052` 校验后的字节发布到客户端不可写的新 key；`UPL-014` 收据以 token 哈希为唯一键，重试幂等 |

---

## 2. 跨文档的全局约束

以下约束在多份文件中反复出现。实现任意一个域时都必须遵守，并以各文件的具体规则为准。

### 2.1 匿名隐私红线

匿名身份（由服务端私有的 `anonymousSeed` 派生的 `绳网用户#XXXX`）在任何接口、通知、搜索索引、实时事件、AI 工具输出中都不能与真实身份关联。匿名内容对所有人匿名，包括作者本人和管理员（后台审核除外）。

- posts.md：`POST-090`～`POST-096`、`POST-063`、`POST-072`、`POST-077`
- feed-search.md：`FEED-021`、`FEED-022`、`FEED-130`、`HOT-007`、`PRIV-001`、`PRIV-005`～`PRIV-008`、`PRIV-010`
- comments.md：`CMT-003`、`CMT-090`、`CMT-092`、`CMT-099`、`CMT-114`、`CMT-123`～`CMT-131`
- social-economy.md：`COMMON-008`、`LIKE-013`、`FAV-009`、`FOLLOW-016`、`DENNY-008`、`PROFILE-018`、`PROFILE-020`
- moderation-exam.md：`RPT-061`、`RPTADM-016`（后台可见匿名作者主页链接）
- messaging-notifications.md：`NOTIF-010`、`NOTIF-013`、`NOTIF-020`、`X-002`
- ai-assistant.md：`TRIG-012`、`TOOL-006`、`TOOL-009`
- media-ops.md：`UPL-056`（禁止通过 uploaders 反查上传者）、`OPS-002`、`OPS-007`

### 2.2 拉黑语义

拉黑是单向记录，但几乎所有生效点都按「任一方向」判断；**匿名内容一律不按拉黑过滤**，避免通过「内容消失 / 操作被拒」反推出匿名作者。

- social-economy.md：`BLOCK-002`、`BLOCK-008`、`BLOCK-012`～`BLOCK-022`、`COMMON-008`、`LIKE-007`、`FAV-004`、`FOLLOW-004`、`FOLLOW-017`
- posts.md：`POST-037`、`POST-069`、`POST-082`、`POST-094`
- feed-search.md：`PRIV-001`～`PRIV-004`、`EDGE-013`、`EDGE-019`、`SUGG-002`
- comments.md：`CMT-030`、`CMT-032`、`CMT-033`、`CMT-094`、`CMT-108`、`CMT-128`、`CMT-145`
- messaging-notifications.md：`CONV-011`、`MSG-013`、`NOTIF-002`、`X-001`
- ai-assistant.md：`AI-013`（AI 账号不能被拉黑）、`TOOL-008`
- moderation-exam.md：`SANCTION-011`（拉黑不是处罚）

### 2.3 提交后副作用

通知投递、实时推送、SSE 广播、会话撤销广播、AI 任务投递都只能在最外层事务提交后执行；事务回滚时一律不执行。事务内禁止调用外部服务（搜索引擎、S3、SMTP、LLM）。

- posts.md：`POST-056`、`POST-070`
- comments.md：`CMT-045`、`CMT-046`、`CMT-095`、`CMT-116`、`CMT-149`
- social-economy.md：`DENNY-011`、`DENNY-012`、`FOLLOW-009`
- feed-search.md：`SSE-007`、`EDGE-015`
- messaging-notifications.md：`NOTIF-040`、`NOTIF-041`、`MSG-014`、`X-004`
- auth-account.md：`SESS-027`、`DEL-007`
- moderation-exam.md：`RPT-050`、`RPTADM-073`、`RPTADM-074`、`RPTADM-090`
- ai-assistant.md：`TRIG-010`、`LLM-009`
- media-ops.md：`UPL-054`

### 2.4 北京时间日界

业务上的「一天」统一用北京时间（`ik_kernel::time`）。旧系统存在一处例外，见 D-001。

- social-economy.md：`EXP-007`（经验日为北京时间 04:00）、`CHECKIN-001`（签到日用服务器本地时区 04:00，与前者矛盾）、`LIKE-005`（04:23 对账）
- comments.md：`CMT-101`、`CMT-156`
- ai-assistant.md：`QUOTA-004`（配额日为北京时间 04:00）
- messaging-notifications.md：`CRON-001`（04:47 对账）、`CONV-028`（按天降级计数器，文档未写明时区）
- media-ops.md：`CRON` 表（全部任务 `Asia/Shanghai`，`drain-follow-fanout` 例外）、`MAIL-004`（邮件降级计数按 **UTC** 日期分桶）
- moderation-exam.md：`RPTADM-078`

### 2.5 入站考试门控

`examPassed || isAdmin || isAiAgent` 才能执行受门控的写操作，否则返回 403 `EXAM_REQUIRED`；每次请求按用户 id 重查，通过后无需重新登录。未通过考试的普通用户等级固定为 0，权益全部为 0。门控覆盖范围不完全一致，见 D-015。

- moderation-exam.md：`EXAM-011`～`EXAM-014`、`ADMIN-005`、`RPT-020`（举报不门控）
- social-economy.md：`COMMON-005`、`COMMON-006`、`EXP-002`、`BENEFIT-002`
- posts.md：`POST-040`（删帖不门控）
- comments.md：`CMT-027`、`CMT-036`（删评论不门控）、`CMT-037`、`CMT-122`
- messaging-notifications.md：`CONV-010`
- auth-account.md：`REG-007`
- media-ops.md：`UPL-020`、`MEUP-006`、`AVA-005`
- ai-assistant.md：`AI-014`

### 2.6 账号状态判定

「不可认证 / 非活跃」= `blocked = true` 或 `accountStatus ∈ {blocked, merged, deleted}`。每一个认证入口（可选鉴权、强制鉴权、实时连接复检、会话创建与旋转）都必须做同样的检查。字段统一问题见 D-019。

- auth-account.md：`ACC-002`、`ACC-015`、`ACC-016`、`SESS-006`、`SESS-026`、`MW-001`～`MW-003`
- moderation-exam.md：`SANCTION-002`～`SANCTION-007`
- feed-search.md：`AUTH-003`
- posts.md：`POST-044`
- social-economy.md：`COMMON-009`
- messaging-notifications.md：`WS-003`

### 2.7 客户端 IP 推导

从 `X-Forwarded-For` **右侧往左数第 `TRUSTED_PROXY_HOPS` 跳**（默认 1；0 表示用 socket 对端地址）；绝不取最左值，也不用框架自带的 `request.ip`。限流、会话 IP、验证码 `requestIp`、浏览量键全部使用同一个函数。

- media-ops.md：`RL-020`、`RL-021`、`RL-022`
- auth-account.md：`RL-001`、`RL-004`
- feed-search.md：`RATE-002`、`RATE-003`
- posts.md：`POST-080`
- social-economy.md：`PROFILE-024`

### 2.8 幂等键与唯一约束

唯一性最终由 DB 唯一约束保证；并发撞约束时按「已存在 / 回读赢家」处理，不能返回 500。

- social-economy.md：`COMMON-003`、`COMMON-004`、`LIKE-002`、`FAV-001`、`FOLLOW-001`、`FOLLOW-019`、`BLOCK-001`、`DENNY-004`、`DENNY-005`、`DENNY-010`、`DENNY-018`、`CHECKIN-002`、`EXP-008`
- posts.md：`POST-026`、`POST-035`、`POST-060`（退款按 relatedId 幂等）、`POST-071`
- comments.md：`CMT-102`、`CMT-107`
- feed-search.md：`READ-001`、`EDGE-021`
- moderation-exam.md：`RPTADM-071`、`RPTADM-072`、`RPTADM-080`
- auth-account.md：`CODE-009`、`SESS-002`、`SESS-019`、`IDENT-001`、`IDENT-006`、`MHY-011`
- messaging-notifications.md：`CONV-002`、`CONV-020`、`NOTIF-023`
- ai-assistant.md：`TRIG-030`、`TRIG-035`、`QUOTA-015`、`GEN-002`、`LLM-010`
- media-ops.md：`UPL-014`、`UPL-035`、`ERR-009`

### 2.9 公开可见性谓词

公开文章 = 已发布且 `isHidden` 为假；公开评论 = 自身、parent（若有）、所在文章都公开，且 parent 是同帖顶层评论。`isHidden IS NULL` 的处理旧系统不一致，见 D-009。

- posts.md：`POST-097`、`POST-098`
- social-economy.md：`COMMON-007`
- comments.md：`CMT-030`、`CMT-136`、`CMT-137`、`CMT-141`
- feed-search.md：`SRCH-032`、`EDGE-018`
- ai-assistant.md：`TOOL-008`
- moderation-exam.md：`RPT-009`、`SANCTION-009`

### 2.10 游标格式与坏游标降级

游标统一为 `base64url("<时间 ISO>|<documentId>")`，在第一个 `|` 处切分，时间部分必须规范化为 ISO；坏游标一律按「未传」处理，退回第一页，不返回 400/500，也不写日志。

- feed-search.md：`FEED-010`～`FEED-015`、`EDGE-001`、`EDGE-002`、`EDGE-004`
- posts.md：`POST-109`
- comments.md：`CMT-072`、`CMT-073`、`CMT-144`
- messaging-notifications.md：`CONV-024`、`CONV-025`、`MSG-010`、`KNOCK-013`、`X-005`

### 2.11 fail-open 与 fail-closed

缓存、限流、存在性检查在存储故障时放行（fail-open），但降级值不写缓存；涉及花钱、会话有效性、重复计数的闸门必须 fail-closed。

- messaging-notifications.md：`X-003`、`CONV-028`、`WS-008`（实时复检 fail-closed）
- media-ops.md：`RL-006`
- posts.md：`POST-080`（浏览冷却存储出错返回 500，避免全站重复计数）
- ai-assistant.md：`QUOTA-020`、`QUOTA-030`
- auth-account.md：`SESS-013`、`SESS-026`、`MHY-019`

### 2.12 5xx 脱敏与统一错误体

5xx 只返回固定文案和 `requestId`，内部细节只进日志；控制器自行设置的 5xx 同样被改写（因此 5xx 的中文提示用户看不到）。

- media-ops.md：`ERR-001`～`ERR-008`、`AVA-003`
- auth-account.md：`RL-003`
- posts.md：`POST-060`

### 2.13 计数器原子更新与对账

物化计数用单条原子 UPDATE（下限 0、NULL 视为 0），失败不回滚主操作，由每日对账修正；没有对账的计数（`commentscount`）依赖增量与全量重算。

- social-economy.md：`LIKE-003`、`LIKE-005`、`FAV-002`
- posts.md：`POST-074`、`POST-103`
- comments.md：`CMT-084`～`CMT-086`、`CMT-109`、`CMT-156`、`CMT-157`
- media-ops.md：`CRON-001`～`CRON-003`

---

## 3. 待裁决清单

共 90 项（D-001～D-090）。D-001～D-024 是跨文档或跨模块的问题，其中 D-001～D-008 是已确认的代码、注释、测试、配置之间的矛盾；D-025 起按模块分组。「影响模块」使用 `AGENTS.md` 中的 crate 名称。

**已裁决**（条目内以「裁决」行标注）：D-001、D-007、D-009、D-018、D-019、D-069、D-070。未裁决条目在实现时按「建议默认」处理，并在代码注释中引用编号，便于日后改判。

已按「建议默认」实现、尚待产品确认的：D-010（置顶帖在所有分区页显示）、D-014（正文上限以等级矩阵为准，最高 16000）、D-015（删除自己的帖子不需要通过考试）、D-017（新用户等级 0）、D-002（所有入账统一 9999 封顶）、D-025（首次发布看帖子自己的 `first_published_at`）、D-048（投币要求帖子公开）、D-049（被点赞/评论/收藏不给经验）、D-050（被采纳经验计入主动桶）、D-051（今日主动经验含被采纳）、D-052（签到时账号被封禁返回 403）、D-060（过期未交卷计一次失败；作答只接受快照里的题目与选项）、D-064（考试奖励已发过时视为成功，不回滚）、D-053（关系查询只返回「我拉黑了对方」）、D-054（@ 提及解析时不做拉黑拦截）、D-020（我的点赞 / 收藏列表在 SQL 里完成全部过滤，游标分页不返回 total）、D-029（删除评论时级联删除它的点赞）、D-011（取消点赞、取消收藏都撤回对应通知，已读的也删）、D-075（通知接口未登录返回 401）、D-008（实时连接 60s 无任何帧断开；服务端 ping 间隔按接口规范为 30s）、D-026（主页精选保存时保留被隐藏的帖子，向访客展示时过滤）、D-023（只实现生效的热榜算法 HOT-002）、D-024（首版不实现已读水位）、D-041（关注流不设关注人数上限）、D-045（热门位被过滤后不补位）、D-072（没有「谁可以私信我」设置）、D-073（不实现群聊）、D-021（不支持图片消息）、D-076（免打扰只是客户端偏好，服务端不抑制推送与未读）、D-038（搜索「最新」按 `bumpedAt`）、D-039（改名后更新索引里的作者名）、D-042（帖子搜索与联想的 q 最多 64 字，超出截断）、D-043（搜索结果用完整的列表卡片）、D-003（上传校验默认 2 个槽位，排队满与超时返回 429）、D-077（签名阶段只对自己的文件去重，跨用户去重在字节校验之后）、D-078（图库移除只断开关联，不做 GC）、D-012（头像只能用自己图库里的图；帖子封面、评论图片接入时沿用）、D-028（隐藏的已采纳评论保留采纳）、D-057（提供解封，不做期限）、D-058（封禁不隐藏已发布内容）、D-059（封禁立即吊销会话）、D-063（不能举报已注销的用户）、D-005（必须有一条默认模型，结算按实际模型倍率）、D-082（额度用尽或全局关闭时 AI 回一条不计费的系统提示）、D-083（调用已开始但没有用量时按已收到的文本估算计费）、D-086（私信历史最多 40 条、工具最多 3 轮，可配置，预占估算按工具轮数上限）、D-088（编辑消息不重新触发 AI）、D-004（AI 评论回复挂到触发评论所在的一级评论下，开头提及触发者）、D-089（评论回复仍一次性发出，没有时间线、停止与重新生成）、D-084（AI 工具统一以触发者为查看者做拉黑过滤，匿名内容豁免）、D-085（AI 工具按用户名只做精确匹配）。D-062 按 ADR 0001 以论坛账号的角色审核（与建议默认 A 不同，待确认）。

**实现中新增的行为决定（待确认）：**

- **取消关注、取消点赞、取消收藏都不要求通过入站考试**，与「删除自己的内容不需要考试」（D-015）一致；新建仍然门控。旧系统的 toggle 两个方向都门控（FOLLOW-003、LIKE-006、FAV-005）。
- **一键三连的点赞、收藏在同一个事务里完成**，任一步被拒（不可见、拉黑）就都不写入。旧系统并行执行，可能只成功一半（TRIPLE-004）。投币仍然是随后单独执行的软失败，返回的原因直接用投币接口的错误码（`DENNY_ALREADY_GIVEN` 等），不再另起一套 `coinReason`（TRIPLE-005）。
- **收藏数用 `posts.favorite_count` 物化列**，与点赞数一样原子增减；旧系统每次实时 COUNT（FAV-002）。
- **通知被拉黑抑制时静默跳过**，不像旧系统那样抛错（NOTIF-002）：触发动作本身已经被拉黑拦截，走到通知这一步的只剩边界情况，不该让业务写入失败。**被封禁、已注销的账号不收任何通知**（旧系统只在提及和举报两处跳过）。
- **敲敲会话不维护分组表，读取时聚合**（见 `docs/schema.md` 通知域）；会话键改为 `user-<id>` / `system`，不再暴露匿名种子。
- **实时通道不再用一次性 ticket**（WS-001/002）：握手直接带 Cookie 或 Bearer，Cookie 连接校验 `Origin` 白名单
  防跨站劫持。**游客可以连接**，只收广播与公开频道（旧文章流 SSE 的能力，KNOCK-040），全站游客连接上限 5000。
  已登录连接不设 30 分钟绝对寿命（旧系统受 JWT 有效期约束，WS-008）；改为吊销事件即时断开 + 每 30s 回库复查，
  复查失败或超时按失效断开（fail-closed）。
- **改名长度按 Unicode 字符计 3～20**（旧系统按 UTF-16 码元，emoji 算 2 个）；除 D-018 已定的 `@` 外，**额外拒绝控制字符**
  （换行等会破坏 @ 提及与各处展示）。用户名被占用返回 409 `USERNAME_TAKEN`（旧系统 400）。
- **作者主页的 `commentCount` 不按所属帖子是否公开过滤**（统计非匿名、未隐藏的评论；评论域不读帖子表）；
  `totalComments` 取帖子的评论数，含他人匿名评论。D-055 的「排除隐藏评论」等 moderation 模块有隐藏操作后再接入。
- **作者主页评论列表先取再按帖子可见性过滤**，一页可能不满，`nextCursor` 为空才是到底。
- **用户搜索只返回一页（最多 20 条），不再提供 `start` 翻页**（旧系统最多翻到 200）；@ 候选只用于输入时联想。
- 自定义头像、装备的系统头像与名片、绑定的绝区零主角色（`zzz`）都在主页展示：隐藏主页时省略装扮与 `zzz`，
  拉黑时保留装扮、省略 `zzz`（PROFILE-021/022）。
- **信息流只用游标分页**：首页把置顶、热门、普通流分成三个数组返回（置顶与热门只在第一页），
  不再有旧 `/articles/list` 的 offset 虚拟列表、`total`、`hasMore` 字段对（FEED-145～150）；热门榜的游标内部是偏移量。
  `feed=favorites` 由 `GET /v1/me/favorites` 提供；`ids` 增量批次（FEED-101）暂不提供，「有 N 条新内容」点击后重新拉第一页即可。
- **帖子卡片新增 `excerpt`（正文摘要）与 `isRead`**；旧卡片没有摘要（FEED-020）。
- **热榜放在进程内存**，不用 Redis；每个实例各自定时计算。
- **新帖扇出由后台任务分批完成**，取代旧系统的 Redis 队列：首次发布且作者有粉丝时，在发布事务里入队 graphile_worker 任务，
  每批 1000 条关注记录、一批一个事务，按 `follow:<帖子 id>` 幂等，重试不会重复通知。旧系统 Redis 不可用时通知直接丢失，
  新实现与发布同一事务入队，不会丢。任务执行时帖子已经下线、隐藏或删除，剩下的粉丝不再通知；通知里的标题取执行时的标题。
- **拉黑后帖子详情返回 403 `USER_BLOCKED`，但不带作者信息**（旧系统带 `authorDocumentId`、`authorName`，BLOCK-012）。客户端已经知道自己点开的是谁的帖子。
- **拉黑的日额度改为令牌桶**：容量 100、每 864 秒回 1 个，近似「每天 100 次」。旧系统是从第一次计数起的 86400 秒固定窗口（BLOCK-006）。
- **与读者有拉黑关系的人的非匿名评论，其回复页同样 404**；楼中楼的回复数也不计入这些人的回复，保证「共 N 条回复」与能翻到的条数一致。
- **私信「离开会话」改为「从自己的列表移除」**：只影响自己，对方的列表不变、也不收到 `member.removed`；
  对方再发来消息、或自己重新发起私聊时恢复（历史保留）。旧系统任一方离开后双方都看不到会话，复活时只恢复发起方（CONV-020/021），
  对方会一直看不到新消息。
- **新建的私聊在对方列表里先隐藏**，收到第一条消息才出现；旧系统建会话时双方立即可见（空会话打扰对方）。
- **私聊排序键按成员各自维护**（`conversation_members.activity_at`）：新消息推进双方，对方发来的通知只推进收件人自己，
  不再把通知时间写进会话共享的 `lastMessageAt`（旧系统会让对方的列表也因为「我收到通知」而重排，CONV-005）。
- **收件箱改为双源归并**：私聊会话与通知分组各自游标分页后归并，与某人有在场私聊时他的通知分组在 SQL 里直接排除、
  合并进会话；不再有旧系统的读时/写时两套聚合、`pseudo:*` 伪会话 id 与填页窗口（KNOCK-010～015）。条目键为
  `dm-<会话 id>` / `user-<id>` / `system`。
- **会话列表与已读统一在 `/v1/inbox`**：点开私聊（`POST /v1/inbox/dm-<id>/read`）推进水位，并把对方发来的通知一并标已读（CONV-034）；
  只标调用时刻及之前的通知。私信模块本身不提供单独的已读接口。
- **发私信的去重改用可选的 `clientId`**（同一发送者唯一），重试返回第一次的消息（200）；旧系统没有幂等。
- **编辑、撤回的时间窗超出返回 409 `MESSAGE_TOO_OLD`**（旧系统 400）；编辑已撤回的消息返回 409 `MESSAGE_WITHDRAWN`；
  编辑、撤回别人的消息：会话成员 403，非成员 404（旧系统一律 403，会暴露消息存在）。
- **搜索的索引同步不走任务队列**：事件（发布、下线、顶帖、删评论、改名）触发即时同步，每 30 秒按 `updated_at` / `created_at`
  增量扫描补漏，启动时与每 24 小时全量对账（新代次重写全部文档，再删除旧代次的残留）。数据库是事实来源、查询结果一律回查，
  所以索引最终一致即可。旧系统在写库时同步调用 Meilisearch（EDGE-015），删除残留只靠人工重建。
- **搜索结果的分页用不透明游标**（内部是偏移量，最多翻到第 1000 条），响应带估计的 `total`；
  旧系统是 `start`/`limit`。游标非法或超过 1000 返回 400 `CURSOR_INVALID`。
- **数据库兜底不用全文索引**：中文在 PG `simple` 分词下按空格切词，基本匹配不到（旧系统实际退化成 LIKE）；
  直接用不区分大小写的子串匹配，窗口 1000 条。帖子按信息流时间（`hot` 按点赞、浏览）排序，评论按时间倒序。
- **全站默认限流改为令牌桶**（media-ops.md `RL-001`～`RL-007`）：阈值沿用生产值，读写与游客/登录分桶不变；
  不再有固定窗口边界放行两倍的问题；响应头去掉 `X-` 前缀（`RateLimit-Limit` 等），`RateLimit-Reset` 是全部恢复的秒数；
  登录身份查会话（缓存 30 秒），无效令牌按游客的 IP 计，编造令牌拿不到新配额。进程内计数，没有 Redis 故障放行的问题（RL-006）。
- **搜索接口有限流**：登录用户每秒 10 次，游客每 IP 每秒 5 次（旧系统没有，RATE-006）。
- **联想的 `titleHighlighted` 由服务端转义**后再加 `<mark>`（旧系统直接把 Meilisearch 的高亮串交给前端，标题里的 HTML 不转义）；
  `excerpt` 是纯文本。作者与分区直接用卡片里的 `author`、`category`。
- **评论搜索结果的帖子封面是 `postCover`（单个对象或省略，EDGE-010）**，其余字段同 SRCH-034。
- **上传凭证不再携带文件夹、标题等 Strapi 字段**，只有过期时间、用户、暂存 key、大小、可选哈希与文件名；
  路径不再由客户端指定（UPL-006 的 `path` 参数取消），统一为 `[root/]YYYY/MM/<随机>.<扩展名>`（年月按北京时间）。
- **上传错误改为中文与稳定错误码**（旧系统是英文 message 的 400）；类型、大小、文件名不合法走 `VALIDATION_FAILED` 字段错误。
- **签名接口限流每人每分钟 60 次**（旧系统没有）。
- **完成时声明了哈希、而自己已有相同内容的文件**：直接返回它并删除暂存对象，不再读回校验（旧系统对任何人的文件都这样做，D-077）。
- **我的图库改为游标分页**（默认 24、最多 100），移除返回 204，不再返回 `inUse`（引用关系分散在各业务域，图库不反查）。
- **自定义头像**：`PUT /v1/me/avatar { mediaId }` 每次 10 丁尼，换成同一张不扣费，`DELETE /v1/me/avatar` 免费清除；
  不要求通过考试（AVA-005）。设置自定义头像同时卸下系统头像（AVA-002），换成同一张图也会卸下但不扣费。
  头像存在账号上，帖子卡片、详情、评论、通知发送者、私信对方、用户搜索、@ 候选都带 `avatarUrl`；匿名内容永远不带。
- **米游社扫码登录与绑定（MHY）**：`POST /v1/auth/mihoyo/qr { mode }`、`POST /v1/auth/mihoyo/qr/poll { qrId, client }`、
  `GET /v1/me/mihoyo`、`DELETE /v1/me/mihoyo`。与旧系统的差异：
  - 轮询凭证是服务端生成的随机 `qrId`，不是米游社的 `ticket`（ticket 印在二维码里，旧系统里看到二维码的人能凭它
    轮询拿走登录会话）。`mode` 必填（旧系统不传时按是否登录推断）。
  - 首登账号没有邮箱（`email` 为 NULL，D-069 方案 A），也没有密码行；不能解绑米游社（MHY-014）。
  - 确定性会话的密钥是专用的 `IK_SECRETS__QR_SESSION_KEY`（D-070 方案 A）；同一二维码重放拿到同一个会话，
    登出后再轮询 409 `QR_SESSION_CONSUMED`（SESS-019）。
  - 拉取绝区零角色失败不影响登录与绑定，只是没有角色资料（旧系统整次失败）。
  - 账号被封禁时返回 403 `ACCOUNT_BLOCKED`（旧系统 `USER_BLOCKED`，新系统 `USER_BLOCKED` 专指拉黑）。
  - Redis 键改为 `mihoyo_qr_logins` 表；限流（每 IP 每分钟生成 30 次、轮询 600 次）在进程内。
  - 扫码注销核验：`mode = delete`，确认后只返回 `matched`（扫的是不是当前账号绑定的米游社账号），不拉角色、不改数据；
    注销时以 `qrId` 交回，在注销事务里核对并消费（旧系统是 Redis 键，提交后才删，DEL-007）。
- **账号安全（DEL-001～010、LOGIN-011，D-071 方案 A）**：
  - `GET /v1/me/security`：邮箱、是否设置过密码、是否绑定米游社、注销用哪种核验（旧系统的 `provider` 字段取消）。
  - 绑定 / 更换邮箱：`POST /v1/me/email/code` + `PUT /v1/me/email`；与当前相同 400、已被占用 409 `EMAIL_TAKEN`。
    这两种用途（以及注销）不能经公开的 `/v1/auth/email-codes` 申请。不踢会话（与旧系统一致）。
  - 修改密码：`PUT /v1/me/password`，所有会话下线后给当前设备签新会话（`password_change`）；没有密码的账号 400
    `PASSWORD_NOT_SET`，用重置密码设置。
  - 注销：`POST /v1/me/delete-account/code` + `POST /v1/me/delete-account { code | qrId }`。立即生效、不可撤回；
    用户名改为「已注销用户_<id>」，邮箱置空（可以重新注册，旧系统改成 `deleted_<id>_<时间>@anonymized.local`）、删密码、
    清头像与装扮、清签名与精选并隐藏主页、删米游社绑定、吊销全部会话；帖子、评论、私信、互动与流水保留（DEL-010）。
  - 旧系统的 `accountCleanupPending` + 每分钟重试（DEL-008/009）不再需要：搜索索引经 `user.renamed` 事件与定期对账更新，
    没有需要清的响应缓存。
- **举报与审核（RPT、RPTADM、SANCTION）**：用户侧 `POST /v1/reports`、`GET /v1/reports/check`、`GET /v1/me/reports`；
  审核侧 `/v1/admin/reports`（按目标聚合）、`/v1/admin/reports/{targetType}/{targetId}`（详情、`claim`、`unclaim`、
  `resolve`）、`POST /v1/admin/users/{id}/unblock`。与旧系统的差异：
  - 审核人是论坛账号里的版主、管理员（`role`），不再有独立的后台账号（D-062，ADR 0001 已定 `/v1/admin` + 角色）；
    版主与管理员权限相同。
  - 目标类型叫 `post` / `comment` / `user`（旧系统 `article`），目标 id 稳定，不需要 `targetKey` 与关系回退。
  - 列表在数据库里 GROUP BY，不再取 5000 条到内存聚合（RPTADM-102）；分页是 `page` + `limit`（默认 20，最多 100）。
  - 删除内容与结案、通知在同一个事务里完成，没有「删除处理中」（`actionPending`）与删除任务表，D-061 不再适用
    （RPTADM-070～080 作废）。
  - 多人举报自动隐藏在举报的同一个事务里完成（旧系统是 `setImmediate` 异步），阈值 `IK_MODERATION__AUTO_HIDE_THRESHOLD`
    （默认 5）。
  - 封禁写 `users.status = 'blocked'` 并立即吊销全部会话、断开实时连接（D-059 方案 A）；不能封禁版主与管理员；
    提供解封（D-057 方案 B，期限与原因属于新需求，未做）；被封禁用户已发布的内容保持公开（D-058 方案 A）。
  - 匿名内容的作者不在审核接口里出现（AGENTS.md 规则 10；旧系统后台可以看到），处置照常作用于真实作者。
  - 已注销、已合并的用户不能被举报（D-063 方案 A）；举报重复返回 409 `REPORT_DUPLICATE`（旧系统 400）。
  - 举报限流（每分钟 3、每天 20）在进程内，校验失败不消耗额度（RPT-043）；没有 Redis 故障放行。
  - 被隐藏的已采纳评论保留采纳状态（D-028 方案 A）；每次结案、自动隐藏、解封记审核日志（旧系统没有）。
- **装扮（COSMETIC-001～009）**：`GET /v1/me/avatars`、`PUT /v1/me/avatars/equipped { avatarId|null }`、
  `GET /v1/me/cards?kind=&cursor=&limit=`、`PUT /v1/me/cards/equipped { cardId|null }`；免费，不要求通过考试。
  - 所有展示作者头像的地方统一「装备的系统头像优先，其次自定义头像」，关注列表也一样（D-056 方案 A）。
  - 名片列表改为游标分页（默认 20、最多 50），分类参数叫 `kind`，不合法时 400 `VALIDATION_FAILED`（旧系统 page/pageSize 默认 30、最多 60）。
  - 名片的背景故事是 Markdown（旧系统是富文本 blocks）。名片图用原尺寸 WebP（`imageUrl`），头像用 360 缩略图（COSMETIC-009）。
  - 目录在管理后台上线前用 CLI 维护：`ik-server cosmetics import <file.json>`，按名字新增或更新，图片引用已上传的媒体 id。
- **帖子封面、评论图片只能用自己图库里的图**（D-012 方案 A），引用形态只有媒体 id 数组（旧系统的 documentId、`{set}`/`{connect}`
  等写法取消，FILE-003、POST-018、CMT-022）；先按原始条数拦截再去重。正文是 Markdown，插图数（`![...](...)`）按同一等级上限
  校验（对应旧系统按 blocks 计数，POST-014）。
- **纯图片评论的通知预览统一为「[图片]」**（Q-08：旧系统通知桶给空串、私信给「评论了你的帖子 [图片]」）。
- **没有完成的上传由后台任务清理**：graphile_worker 定时任务每小时删除暂存前缀下超过 24 小时（可配置）的对象，
  多实例时只有一个实例执行，不再依赖存储的生命周期规则（UPL-060）。
- **媒体对象带 ThumbHash**（新增；AVIF 没有纯 Rust 解码器，不算）。
- **`typing` 走实时通道**，网关按连接每秒最多处理 1 帧，私信模块校验成员身份与拉黑关系后只推给对方；被拉黑时不再推 typing。

- **帖子发布后不能切换匿名。** 旧系统允许已发布的实名帖改成匿名（POST-053），但大家已经看到过作者，改匿名反而把「这个匿名身份 = 这个人」公开了，违反隐私红线。从未发布过的草稿可以随意切换；发布过一次后即使下线回到草稿也不能再切换（按 `first_published_at` 判断），否则「实名发布 → 下线 → 改匿名 → 重新发布」照样能对上号。
- **一篇帖子只有一行，编辑已发布帖子时修改单独存放**（见 `docs/schema.md`）。旧系统的双版本行与关系挂回机制不复刻。
- **请求参数不合法时明确报错，而不是静默忽略。** 旧系统对不存在的分区、无效的标签、超过 5 个的标签都是静默丢弃（POST-023/032）；新接口返回 400 和字段级错误。
- **坏的分页游标返回 400**（API 规范统一规则），旧系统是静默回到第一页（POST-109）。
- **标签名里的制表符、换行按空格处理**，旧系统是直接删除（「多\t标签」旧系统得到「多标签」，新系统得到「多 标签」）。

### 3.1 跨文档矛盾与跨模块问题

#### D-001 签到日用服务器本地时区，经验日和配额日固定北京时间 04:00
- 涉及：social-economy.md `CHECKIN-001`、`EXP-007`、未决问题 2；ai-assistant.md `QUOTA-004`；comments.md `CMT-101`
- 旧系统现状：签到日边界为 04:00，但按服务器本地时区计算（`Date#getHours`）；经验奖励日、AI 配额日固定为北京时间 04:00；生产服务器的时区未知。
- 可选方案：A 三者统一为北京时间 04:00；B 签到改为北京时间零点，经验与配额保持 04:00；C 维持服务器本地时区。
- 建议默认（建议）：A。`AGENTS.md` 规则 7 要求业务上的「一天」统一走北京时间 helper，统一后签到的经验也不会落到另一个奖励日。迁移前需确认生产时区，否则切换当天可能出现重复签到或误判断签。
- 影响模块：economy、ai、kernel
- **裁决（2026-10-08）：A。** 签到、经验日、AI 配额日统一为东八区凌晨 04:00 切日（`ik_kernel::time::biz_day`）。

#### D-002 作者的投币收入封顶 99999999，全局余额封顶 9999
- 涉及：social-economy.md `DENNY-001`、`DENNY-006`、`DENNY-009`、丁尼动作总表、未决问题 3
- 旧系统现状：用户 `denny` 字段的 schema 为 `max 9999`，`addDenny` 入账封顶 9999；但投币时作者 `denny+1` 的封顶是 99999999。文档没有记录超过 9999 时 schema 校验是否生效。
- 可选方案：A 全局统一 9999，超出部分丢弃（与 `addDenny` 一致，封顶时不写日志）；B 全局统一 99999999；C 维持两套口径。
- 建议默认（建议）：A，与 schema 和其余所有入账路径一致。迁移前先统计生产中余额大于 9999 的行；如果存在，交产品决定截断还是放宽 CHECK 约束。
- 影响模块：economy

#### D-003 上传校验并发槽位：代码默认 6，测试和运维文档按 2；超时状态码 429 与 503 不一致
- 涉及：media-ops.md `UPL-040`、`UPL-041`、`UPL-044`、`UPL-045`、未决问题 1
- 旧系统现状：代码默认 6 个校验槽位（上限 32）、排队 64，排队或任务超时抛 `RateLimitError`（429）；测试 `upload-content.test.ts` 假设 2 个槽位并期望 503；`backend-remediation.md` 写的是「最多同时处理两份 30MiB 缓冲」。
- 可选方案：A 默认 2，排队满和超时返回 429；B 默认 6，返回 429；C 默认 2，返回 503。
- 建议默认（建议）：A。按运维文档的 2×30MiB 内存上限更保守，且可配置；429 加 `Retry-After` 符合「稍后重试」语义，并与现有 `details.code`（`UPLOAD_VERIFICATION_BUSY` / `UPLOAD_VERIFICATION_TIMEOUT`）一致。S3 层超时仍按 `UPL-045` 返回 503。
- 影响模块：media

#### D-004 AI 评论回复的 parent 设为触发评论本身，可能违反单层楼规则
- 涉及：ai-assistant.md `TRIG-013`、`TRIG-017`、未决问题 1；comments.md `CMT-025`、`CMT-031`
- 旧系统现状：AI 回复的 `parent = 触发评论`；当触发评论本身是楼中楼时，评论创建会被「parent 必须是顶层评论」拒绝，重试后进入死信，`TRIG-013` 的「父评论作者是 AI」分支实际走不通。文档标注这是代码推断，需要线上日志确认。
- 可选方案：A parent 取触发评论所在的顶层评论，正文前保留对触发者的提及 token；B 放开为两层以上的回复；C 楼中楼中的评论不触发 AI。
- 建议默认（建议）：A，与单层楼规则以及前端「回复某条回复」的做法（`CMT-026`）一致，不需要改动楼层模型。
- 影响模块：ai、content

#### D-005 未指定模型时按最贵模型的倍率计费
- 涉及：ai-assistant.md `QUOTA-008`、`QUOTA-018`、`CTX-030`、未决问题 3
- 旧系统现状：预占时不知道模型就取启用模型中最大的 `costWeight`；结算时如果走的是服务端默认模型（没有实际模型 key），沿用预占的最贵倍率。旧 worker 只传 userId，所以生产上一律按最贵档预占。
- 可选方案：A 新设计要求存在一条「默认模型」记录（有自己的 `costWeight`），模型决议链不再有「无模型」一级，结算按实际模型倍率；B 维持最贵档；C 默认模型倍率固定为 1。
- 建议默认（建议）：A，结算按实际倍率；预占仍可按最贵档估高，因为估高只降低并发度，不会让闸门失效（`QUOTA-007`）。
- 影响模块：ai

#### D-006 `POST /api/auth/reset-password` 存在两条同路径路由
- 涉及：auth-account.md `LOGIN-008`、`LOGIN-010`、未决问题 1
- 旧系统现状：插件原生的 resetPasswordToken 流程与本站的邮箱验证码流程同路径同方法，实际生效者取决于 Strapi 路由注册顺序；前端只发送验证码流程的 `{email, code, password}`。
- 可选方案：A 新 API 只保留验证码重置一条路径；B 两条都保留但路径分开。
- 建议默认（建议）：A，前端只使用验证码流程，新 API 不需要兼容旧插件路径。
- 影响模块：auth

#### D-007 删帖扣 10 丁尼没有幂等键，扣费与删除不在同一事务
- 涉及：posts.md `POST-060`、`POST-105`、未决问题 2；social-economy.md `DENNY-007`、`DENNY-017`
- 旧系统现状：`deductDenny` 不做幂等检查；扣费和删除分属不同事务，删除失败时退款（退款按 relatedId 幂等）。并发双击删除可能扣两次、只退一次。
- 可选方案：A 扣费、删除、计数在同一事务内完成，并以 `(user, spend_delete_post, article, documentId)` 唯一约束防重复；B 只加幂等键，保留分事务加退款；C 维持现状。
- 建议默认（建议）：A。`AGENTS.md` 规则 6 要求业务写入同事务，新设计删除时没有外部调用阻碍；单事务后退款路径也不再需要。
- 影响模块：economy、content
- **裁决（2026-10-08）：取消删帖扣费。** 删帖不扣丁尼，`spend_delete_post` 动作与退款路径整体删除；POST-060、POST-105、DENNY-007、DENNY-017 中的扣费描述作废。

#### D-008 DM WebSocket 心跳：代码为 25s ping / 60s 空闲断开，注释写 30s
- 涉及：messaging-notifications.md `WS-006`、`WS-020`、`X-008`
- 旧系统现状：服务端每 25s 发 ping，60s 内没有任何活跃就 terminate；文件头注释写「30s 没收到 pong 则关闭」。文档要求以代码为准。前端每 20s 发应用层 ping。
- 可选方案：A 新网关按代码行为（25s / 60s）做成配置项；B 按注释改为 30s。
- 建议默认（建议）：A，现有前端已经按这个行为运行，60s 能容忍连续丢失两次 25s ping。
- 影响模块：realtime

#### D-009 `isHidden IS NULL` 在不同查询路径上的语义不一致
- 涉及：comments.md `CMT-137`、`Q-05`；feed-search.md `EDGE-018`、未决问题 4；social-economy.md `COMMON-007`；posts.md `POST-097`
- 旧系统现状：文章列表、搜索、置顶、未看流、评论列表、内联回复、回复分页使用 `<> true`，SQL 中会**排除** NULL 行；热榜、相关推荐、`content-visibility` 把 NULL 视为可见。生产中是否存在 NULL 行未知。
- 可选方案：A 新表 `is_hidden NOT NULL DEFAULT false`，迁移时 NULL 回填 false；B 回填 true（维持旧列表的实际表现）；C 先统计生产中的 NULL 行再决定。
- 建议默认（建议）：先 C 后 A：先统计，数量为 0 时直接按 A；不为 0 时交产品逐条确认，因为回填 false 会让旧列表中不可见的内容重新出现。
- 影响模块：content、feed、search、moderation
- **裁决（2026-10-08，授权由实现方决定）：** 新表不用布尔，改为 `hidden_at TIMESTAMPTZ NULL` + `hidden_by` + `hidden_reason`；`hidden_at IS NULL` 即可见，从结构上消除三态，并保留审计信息。所有读路径共用同一个可见性谓词（content crate 的 `visible` SQL 片段）。数据迁移时旧 `isHidden IS NULL` 视为**可见**（旧系统的隐藏操作总是显式写 true，NULL 只来自从未被隐藏的历史行）；迁移脚本输出 NULL 行数量供复核。

#### D-010 置顶帖注入分区页且不按分区过滤（代码与注释矛盾）
- 涉及：posts.md `POST-108`、未决问题 5；feed-search.md `FEED-140`、未决问题 1
- 旧系统现状：`feed=all` 且无 tag 时注入全站置顶，即使带了 category 也注入，且不按分区过滤；代码注释却说「置顶帖必须满足当前分类/标签」。tag 页和游标后续页不注入。
- 可选方案：A 保留「全站公告在每个分区都显示」；B 只注入本分区的置顶；C 区分「全站置顶」和「分区置顶」两级。
- 建议默认（建议）：A，保持现有用户可见行为；以后需要时可以按 C 扩展，不会破坏 A。
- 影响模块：feed、content

#### D-011 取消点赞删除全部点赞通知（含已读），取消收藏不删除通知
- 涉及：social-economy.md `LIKE-014`、`FAV-010`、未决问题 9；messaging-notifications.md `NOTIF-042`、`X-008`、未决问题 3
- 旧系统现状：取消点赞删除该 sender 对该目标的全部 like 通知（函数名带 Unread，实际不按已读过滤），并回退通知桶；取消收藏不删除收藏通知；重新点赞会产生一条新通知。
- 可选方案：A 点赞和收藏都在取消时删除全部对应通知；B 都只删未读；C 维持不一致。
- 建议默认（建议）：A，避免反复切换刷出重复通知，且点赞路径已经是这个行为；删除后必须回退通知桶并失效缓存。
- 影响模块：social、notify

#### D-012 引用媒体不校验归属与类型（文章封面、评论图片、自定义头像）
- 涉及：comments.md `CMT-022`～`CMT-024`、`Q-01`；posts.md `POST-018`；media-ops.md `FILE-003`、`FILE-020`、`AVA-005`、未决问题 4；social-economy.md `COSMETIC-007`、未决问题 5
- 旧系统现状：任意已存在的文件 id 或 documentId 都可以被引用；评论的数字 id 不校验存在性；自定义头像不校验类型和大小。上传本身只允许图片 MIME（`UPL-001`）。
- 可选方案：A 只允许引用当前用户 `uploaders` 中的文件，同时校验存在性和图片 MIME（管理员与 AI 是否豁免另定）；B 只校验存在性和图片 MIME；C 维持现状。
- 建议默认（建议）：A，防止盗用他人图片和引用非图片文件。需要与 D-077（去重会把用户加入他人文件的 uploaders）一起裁决，否则 A 可以被绕过。
- 影响模块：media、content、social

#### D-013 隐藏文章时不失效置顶缓存
- 涉及：posts.md `POST-076`、`POST-112`；moderation-exam.md `RPT-072`、未决问题 8；feed-search.md `CACHE-003`
- 旧系统现状：隐藏文章时同时清掉 `isPinned`，但不删除置顶缓存，依赖 60s TTL 和读侧的未隐藏过滤；管理员 pin/unpin 时会主动删除缓存。
- 可选方案：A 隐藏提交后主动失效缓存；B 继续依赖 TTL。
- 建议默认（建议）：A，进程内缓存失效成本很低，也能避免 D-045 中「已隐藏置顶仍计入 S」的问题；多实例部署时仍需 TTL 兜底。
- 影响模块：feed、moderation

#### D-014 正文 schema 上限 8000 与等级矩阵最高 16000 冲突
- 涉及：posts.md `POST-003`、`POST-104`、未决问题 1；social-economy.md `BENEFIT-001`、`BENEFIT-003`
- 旧系统现状：schema `maxLength 8000`；等级矩阵 `articleMaxBody` 最高 16000；等级 ≥4 的用户通过服务端校验后，Strapi 实体校验是否仍按 8000 拦截没有确认。
- 可选方案：A 以等级矩阵为唯一上限（DB CHECK ≤16000）；B 全局封顶 8000。
- 建议默认（建议）：A，等级矩阵是服务端唯一裁决依据（`BENEFIT-003`）；迁移前确认生产中是否已有超过 8000 字的正文。
- 影响模块：content

#### D-015 入站考试门控的覆盖范围不一致
- 涉及：posts.md `POST-040`、未决问题 8；comments.md `CMT-036`、`Q-14`；social-economy.md `COMMON-006`；media-ops.md `MEUP-006`、`AVA-005`、未决问题 5；moderation-exam.md `EXAM-012`
- 旧系统现状：删帖、删评论、自定义头像（扣 10 丁尼）、图库移除、拉黑、改名、签名不门控；直传上传门控。由于自定义头像的前端流程先走直传（`AVA-004`），新上传的头像实际上已被间接门控（推断）。
- 可选方案：A 维持：删除自己的内容和个人资料类操作不门控，创建与互动类门控；B 所有写操作都门控；C 只给扣费操作额外加门控。
- 建议默认（建议）：A，用户应始终能删除自己的内容、管理自己的资料；在 `docs/api-guidelines.md` 中显式列出门控清单。
- 影响模块：content、social、media、kernel

#### D-016 文章流 SSE 用客户端自报的 uid 排除触发者，可探测匿名帖作者
- 涉及：posts.md `POST-072`、`POST-107`、未决问题 4；feed-search.md `AUTH-004`、`SSE-005`、未决问题 8；messaging-notifications.md `KNOCK-040`
- 旧系统现状：文章流 SSE 完全匿名，客户端用明文 `uid` 自报身份，只用于排除本人触发的事件；`new_topic` 对匿名帖也按真实作者排除，遍历 uid 观察「是否收到推送」理论上可以探测匿名帖作者。
- 可选方案：A 新的统一 WS 网关用已鉴权身份做排除，且匿名帖的 `new_topic` 不排除任何人；B 用已鉴权身份排除，匿名帖也排除作者；C 维持自报 uid。
- 建议默认（建议）：A。自报身份可伪造，而匿名帖排除作者本身就是探测面；按隐私红线，宁可让作者也收到自己匿名帖的提示。未登录访客仍可订阅。
- 影响模块：realtime、feed

#### D-017 新用户初始等级：schema 默认 1，注册路径实际写 0
- 涉及：social-economy.md `EXP-002`、`EXP-004`、`EXP-013`、`PROFILE-001`、未决问题 7；auth-account.md `ACC-004`、`REG-006`、`MHY-010`；moderation-exam.md `EXAM-040`
- 旧系统现状：schema 中 `level` 默认 1；邮箱注册和米游社建号都显式写 0；首次通过考试设为 `max(1, calculateLevel(exp))`；部分读接口在字段缺失时回落为 1。
- 可选方案：A 列默认 0 且 NOT NULL，读侧不再回落 1；B 默认 1。
- 建议默认（建议）：A，与注册路径的实际写入和「未通过考试等级为 0」的规则一致。
- 影响模块：auth、economy

#### D-018 用户名规则：大小写、交叉检查、保留前缀、字符集、冷却
- 涉及：auth-account.md `ACC-008`、`ACC-009`、`ACC-010`、未决问题 5；social-economy.md `PROFILE-002`、`PROFILE-004`、未决问题 6；comments.md `CMT-016`、`Q-11`
- 旧系统现状：改名 3～20 个 UTF-16 码元、无字符集限制、无冷却、每次扣 10 丁尼；用户名唯一性被描述为大小写敏感，但实际取决于 DB 排序规则；注册和绑定邮箱有 email/username 交叉检查，改名没有；`绳网用户`、`已注销用户` 等系统前缀没有保留；裸 @ 解析的精确匹配在 MySQL 下可能不区分大小写。
- 可选方案：A 唯一性按小写归一（lower 唯一索引），改名补上交叉检查和保留前缀黑名单，字符集与冷却维持现状；B 大小写敏感，但补交叉检查和保留前缀；C 维持现状。
- 建议默认（建议）：A，大小写不敏感的唯一性可以防冒名，并与邮箱规则（`ACC-007`）一致。迁移前审计大小写重复的用户名，冲突时按 `ACC-007` 的方式中止并报告。
- 影响模块：auth、social、content
- **裁决（2026-10-08）：用户名区分大小写**（`Alice` 与 `alice` 是两个不同用户名）。@ 提及解析按精确匹配。用户名不能含 `@`（已由 DDL 保证，天然不会与邮箱混淆）。保留前缀、字符集、改名冷却未单独裁决，暂按旧系统现状。

#### D-019 封禁状态的表示：`blocked`、`accountStatus`、`confirmed` 三处混用
- 涉及：moderation-exam.md `SANCTION-001`、`SANCTION-002`、`RPTADM-063`、未决问题 1；auth-account.md `ACC-002`、`ACC-015`、`ACC-016`、`MW-003`、未决问题 7
- 旧系统现状：举报封禁只写 `blocked = true`，`accountStatus` 从未被写成 `blocked`；注销同时写 `deleted` 和 `blocked = true`；`confirmed` 只在实时复检中检查；各入口的判定写法略有不同。
- 可选方案：A 以 `accountStatus` 为唯一权威（`active / blocked / deleted / merged`），去掉 `blocked` 和 `confirmed` 两列，迁移时做映射；B 保留 `blocked` 布尔加 `accountStatus`；C 维持现状。
- 建议默认（建议）：A，单一枚举消除各入口判定不一致，kernel 的鉴权上下文只需检查一处。迁移映射中 `confirmed = false` 且 `active` 的存量行如何处理需要确认。
- 影响模块：auth、moderation、kernel
- **裁决（2026-10-08）：** 产品上只有两个独立维度——**是否被封号**、**是否通过入站答题**。实现：封号只由 `users.status = 'blocked'` 表示（`deleted` / `merged` 是注销与账号合并的生命周期状态，不是封号）；答题由 `users.exam_passed_at` 表示。`confirmed` 不迁移。旧数据映射：`blocked = true` 且未注销 → `blocked`；已注销 → `deleted`。

#### D-020 列表 total 是否必须精确
- 涉及：social-economy.md `LIKE-017`、`BLOCK-013`、未决问题 14；feed-search.md `PRIV-004`、`FEED-147`
- 旧系统现状：我的点赞列表的 total 用过滤前的计数；拉黑排除集合超过 200 个作者时，主信息流改为应用层过滤，total 偏大，也可能返回不满的一页。
- 可选方案：A 接受近似 total，并在 API 文档中注明；B 在 SQL 中完成全部过滤，total 精确；C 游标分页的列表不返回 total。
- 建议默认（建议）：我的点赞列表按 B，主信息流按 C（游标模式本来就返回 `total: 0`）。在 PostgreSQL 中用反连接表达拉黑过滤，应当不再需要 200 上限的变通，需要压测确认。
- 影响模块：social、feed

#### D-021 私信图片消息没有附件写入路径
- 涉及：messaging-notifications.md `MSG-001`、`CONV-032`、未决问题 2；media-ops.md `FILE-002`、未决问题 3
- 旧系统现状：`kind = image` 只是一个标记，`attachments` 字段存在于 schema 但从不写入也不读取；前端似乎没有上线发图。
- 可选方案：A 首版不支持图片消息（拒绝 `kind = image`）；B 支持，复用直传与归属校验，并确定附件数量和类型上限。
- 建议默认（建议）：A，旧系统没有附件写入路径，等产品给出上限后再加。
- 影响模块：messaging、media

#### D-022 多张表和存储前缀没有保留期与清理任务
- 涉及：auth-account.md `CODE-018`、`LOCK-005`、未决问题 2；moderation-exam.md `RPT-073`；media-ops.md `UPL-060`、未决问题 7
- 旧系统现状：`register_verifications`、`auth_login_attempts`、已结案的举报行都没有清理任务；对象存储的 staging 前缀没有生命周期规则，未完成的直传对象会永久残留。
- 可选方案：A 统一定义保留期并用 graphile_worker 定时清理（验证码与登录尝试按过期后 7 天，staging 对象由对象存储生命周期规则处理，举报保留期另定）；B 不清理。
- 建议默认（建议）：A。验证码与登录尝试可沿用会话「过期后 7 天」的口径；举报属于审计数据，保留期由运营决定；staging 保留天数由运维给出。
- 影响模块：auth、moderation、media

#### D-023 未接线的热榜算法与阅读 UV 表是否迁移
- 涉及：posts.md `POST-111`、未决问题 6；feed-search.md `HOT-008`、`CRON-005`、未决问题 9
- 旧系统现状：生效算法只有 `hot.ts`（`HOT-002`）；`hot-ranking.ts` 的 recent 算法、`hot-legacy.ts` 公式没有调用方；`article_hot_views` 没有写入方；诊断 key `hot:ranking:diagnostics:v1` 没有写入方。
- 可选方案：A 全部不迁移，只实现 `HOT-002`；B 迁移 UV 表并恢复浏览记录写入。
- 建议默认（建议）：A，生效算法不依赖它们。
- 影响模块：feed

#### D-024 已读水位是否实现
- 涉及：posts.md `POST-110`；feed-search.md `READ-010`～`READ-013`、`UNRD-004`、未决问题 5
- 旧系统现状：水位开关默认关闭；「全部标为已读」端点不存在；文章详情和 batch 查询仍只按「存在记录即已读」判断。
- 可选方案：A 首版不实现；B 实现水位，并把未看流、详情、batch 查询统一改为「水位或记录」。
- 建议默认（建议）：A，旧系统生产未启用。以后实现时必须先改完所有已读入口，才能归档水位之前的阅读记录。
- 影响模块：feed

### 3.2 内容（posts.md）

#### D-025 次日编辑重发会补发一次发帖经验
- 涉及：posts.md `POST-073`、`POST-106`、未决问题 3
- 旧系统现状：`create_article` 按每日去重，当天第 2 篇以后不写经验日志；草稿转发布时用「该 documentId 是否已有经验日志」区分首次发布，因此次日编辑这类帖子会触发一次经验（占用次日额度）。
- 可选方案：A 首次发布的判定改为文章自身的事实（如首次发布时间是否已写入），与经验去重分开；B 维持现状。
- 建议默认（建议）：A，「是否首次发布」是文章本身的状态，不应依赖经验日志是否落库。
- 影响模块：content、economy

#### D-026 主页精选不剔除被隐藏的帖子
- 涉及：posts.md `POST-114`、未决问题 7；social-economy.md `PROFILE-012`、`PROFILE-019`
- 旧系统现状：保存精选时只剔除已删除、已撤回、已转匿名的帖子，不剔除 `isHidden`；文档没有说明精选模式向访客展示时是否过滤隐藏帖。
- 可选方案：A 保存时允许保留，向访客展示时过滤；B 保存时剔除。
- 建议默认（建议）：A，自动隐藏可能被驳回恢复（`RPTADM-060`），保存时剔除会丢失用户的配置。
- 影响模块：content、social

### 3.3 评论（comments.md）

#### D-027 删除已采纳的评论后，文章仍显示「已解决」
- 涉及：comments.md `CMT-051`、`CMT-138`、`Q-02`
- 旧系统现状：删除评论不清理文章的 `solvedAt` 和 `acceptedCommentDocumentId`，文章指向一条不存在的评论。
- 可选方案：A 删除时在同一事务中清理（仅当文章指向的正是被删评论）；B 维持现状。
- 建议默认（建议）：A，否则会留下悬空引用。
- 影响模块：content

#### D-028 被隐藏的已采纳评论是否清除采纳状态
- 涉及：comments.md `CMT-133`、`CMT-139`、`Q-03`
- 旧系统现状：隐藏时保留评论的 `acceptedAt` 和文章的 `solvedAt`，但列表的采纳位不显示它。
- 可选方案：A 维持（驳回后自然恢复）；B 隐藏时清除。
- 建议默认（建议）：A，隐藏可以被驳回撤销，而清除不可逆。
- 影响模块：content、moderation

#### D-029 删除评论时，评论自身的点赞行只解除关联
- 涉及：comments.md `CMT-140`、`Q-04`
- 旧系统现状：只有后代评论的点赞被显式删除，被删评论自身的点赞行变成孤儿。
- 可选方案：A 一并删除（外键级联）；B 维持现状。
- 建议默认（建议）：A，避免孤儿行，新表用外键 `ON DELETE CASCADE` 即可。
- 影响模块：content、social

#### D-030 重复采纳同一条评论会重复发通知并刷新 `solvedAt`
- 涉及：comments.md `CMT-058`、`CMT-096`、`Q-07`
- 旧系统现状：每次非自采纳的采纳调用都发一条新的 `accepted` 通知并刷新 `solvedAt`；经验按评论终身去重。
- 可选方案：A 对已采纳的同一条评论再次采纳视为空操作；B 维持现状。
- 建议默认（建议）：A，状态没有变化就不应产生通知。
- 影响模块：content、notify

#### D-031 纯图片评论的通知预览两处口径不同
- 涉及：comments.md `CMT-097`、`CMT-098`、`Q-08`
- 旧系统现状：通知桶和会话列表预览为空串；转成 DM 消息时为 `评论了你的帖子 [图片]` / `回复了你的评论 [图片]`。
- 可选方案：A 统一用带 `[图片]` 的文案；B 统一用空串。
- 建议默认（建议）：A，空预览在统一收件箱中无法阅读。
- 影响模块：notify

#### D-032 `anonymous` 通知分组是否只服务于历史数据
- 涉及：comments.md `CMT-100`、`Q-09`；messaging-notifications.md `NOTIF-013`、`NOTIF-020`
- 旧系统现状：新的评论和点赞不会为匿名评论产生通知，但分组逻辑和 anonymous 桶仍然存在。
- 可选方案：A 保留 anonymous 类别，仅用于迁移后的历史通知，新写入不再产生；B 迁移时丢弃匿名历史通知。
- 建议默认（建议）：A，不丢用户数据，同时必须守住 `NOTIF-013` 的身份边界。
- **实现（2026-10-16，数据迁移，待产品确认）：** 新表不建匿名分组；迁移时把匿名评论引起的历史通知改为**系统通知**
  （去掉发送者，正文注明「有人匿名评论了你的帖子」并保留评论摘要与帖子链接）。不丢数据，也不越过身份边界；
  若产品要求在会话列表里保留独立的匿名分组，再按方案 A 加列（expand 迁移）。见 ADR 0003。
- 影响模块：notify

#### D-033 裸 @ 展开后正文可能超过 2000 字
- 涉及：comments.md `CMT-005`、`CMT-142`、`Q-10`
- 旧系统现状：长度和表情数在展开之前检查，展开后不再校验。
- 可选方案：A 展开后再校验一次，超限返回 400；B 截断；C 维持现状。
- 建议默认（建议）：A，约束下沉到 DDL 之后，写入值不能超过 CHECK。
- 影响模块：content

#### D-034 新评论是否实时出现在帖子详情页
- 涉及：comments.md `CMT-117`、`Q-12`
- 旧系统现状：没有这种推送，只靠通知和客户端重新请求。
- 可选方案：A 首版不做；B 在新 WS 网关中增加帖子级订阅。
- 建议默认（建议）：A，旧系统没有，前端也不依赖。
- 影响模块：realtime、content

#### D-035 管理员和帖主是否能直接删除他人的评论
- 涉及：comments.md `CMT-035`、`Q-13`
- 旧系统现状：只有评论作者本人能删除；管理员只能通过举报处理删除。
- 可选方案：A 维持现状；B 管理员可直接删除；C 帖主也可删除自己帖子下的评论。
- 建议默认（建议）：A，保持审核留痕只走举报流程。
- 影响模块：content、moderation

#### D-036 匿名帖作者以实名评论自己的帖子会暴露身份
- 涉及：comments.md `CMT-126`、`Q-15`
- 旧系统现状：评论是否匿名只看评论自身的 `isAnonymous`，后端没有任何提示或默认值。
- 可选方案：A 后端不变，由前端提示；B 匿名帖作者在自己帖子下评论时默认匿名。
- 建议默认（建议）：A，后端强制会改变评论的匿名语义，这是前端交互问题。
- 影响模块：content

#### D-037 评论是否需要专门的频率限制
- 涉及：comments.md `CMT-120`、`CMT-121`、`Q-16`
- 旧系统现状：只有全站固定窗口限流（登录写 120 次/分钟）。
- 可选方案：A 增加按用户的评论限流（数值待定）；B 只保留全站限流。
- 建议默认（建议）：A，按用户限流成本低，数值由产品给出。
- 影响模块：content

### 3.4 信息流与搜索（feed-search.md）

#### D-038 搜索的「最新」实际按 `updatedAt` 排序
- 涉及：feed-search.md `SRCH-011`、`SRCH-015`、未决问题 2
- 旧系统现状：`sort=latest` 和 SQL 兜底的 relevance 都按 `updatedAt`，而计数列写入会刷新 `updatedAt`，所以「最新」其实是「最近有互动」。
- 可选方案：A 改按首次发布时间；B 改按 `bumpedAt`；C 维持现状。
- 建议默认（建议）：B，与主信息流的 latest 口径一致；需要在索引文档中增加 `bumpedAt`。
- 影响模块：search

#### D-039 改名后搜索索引中的作者名不更新
- 涉及：feed-search.md `SRCH-001`、未决问题 3
- 旧系统现状：只有注销时把 `authorName` 改成 `已注销用户`；改名后按旧名仍能搜到。
- 可选方案：A 改名提交后入队，重建该作者的非匿名文档；B 维持现状。
- 建议默认（建议）：A，匿名文档的 `authorName` 为 null，不受影响。
- 影响模块：search、social

#### D-040 无作者的非匿名帖在拉黑过滤下的处理不一致
- 涉及：feed-search.md `EDGE-019`、未决问题 4
- 旧系统现状：DB 的 `NOT IN` 会排除它；置顶的应用层过滤和 `rejectBlockedItems` 会保留它。
- 可选方案：A 保留（不过滤）；B 排除。
- 建议默认（建议）：A，拉黑是用户之间的关系，没有作者的帖子不存在被拉黑的一方。SQL 中需要写成「作者为空或不在排除集合中」。
- 影响模块：feed

#### D-041 关注流只取前 1000 个关注作者，且查询无序
- 涉及：feed-search.md `FEED-130`、未决问题 6
- 旧系统现状：关注超过 1000 人的用户会随机丢掉一部分作者。
- 可选方案：A 改用 SQL 子查询，不设上限；B 保留上限，但按关注时间排序截断。
- 建议默认（建议）：A，PostgreSQL 子查询不需要把作者 id 拉到应用层。
- 影响模块：feed

#### D-042 文章搜索与联想的 q 没有长度上限
- 涉及：feed-search.md `SRCH-010`、`SRCH-020`、未决问题 7
- 旧系统现状：文章搜索和联想无上限；评论搜索截断为 32 字，用户搜索超过 32 字返回 400 或空页。
- 可选方案：A 统一设上限（数值待定）；B 维持现状。
- 建议默认（建议）：A，防止超长查询压垮搜索引擎和 SQL 兜底。
- 影响模块：search

#### D-043 搜索结果卡片的 `bumpedAt` 恒为 null、`isPinned` 恒为 false
- 涉及：feed-search.md `SRCH-016`、未决问题 10
- 旧系统现状：因为只取了部分字段，这两个字段在搜索结果中总是缺省值。
- 可选方案：A 补齐，与列表卡片同形；B 维持现状。
- 建议默认（建议）：A，卡片同形可以减少客户端分支，成本只是多取几个字段。
- 影响模块：search

#### D-044 相关推荐接口可能被匿名响应缓存固定 30 秒
- 涉及：feed-search.md `SUGG-001`、`CACHE-006`、未决问题 11
- 旧系统现状：接口返回 `Cache-Control: private, no-store`，但位于 strapi-cache 的 `/api/articles` 前缀下，插件是否遵循响应头未确认。
- 可选方案：A 个性化和随机结果的接口不做响应缓存，只缓存随机池（600s）；B 允许短时缓存。
- 建议默认（建议）：A，新设计没有 strapi-cache，按接口声明的语义处理即可。
- 影响模块：feed

#### D-045 热门注入不补位，置顶 S 计数可能包含已隐藏的帖子
- 涉及：feed-search.md `FEED-142`、`FEED-145`、未决问题 12
- 旧系统现状：被过滤掉的热门位置不补位；置顶缓存过期前已隐藏的帖子仍计入 S，导致当页少一条、total 多一。
- 可选方案：A 修正 S，只计入通过可见性过滤后的置顶，热门仍不补位；B 补位；C 维持现状。
- 建议默认（建议）：A，修正 S 是纯修复，补位会改变展示位的语义。
- 影响模块：feed

#### D-046 用户搜索不按 `accountStatus` 过滤注销或合并的账号
- 涉及：feed-search.md `SRCH-040`、`SRCH-041`、未决问题 13；auth-account.md `DEL-006`
- 旧系统现状：只过滤 `blocked` 和 `profileHidden`。注销流程会写 `blocked = true` 和 `profileHidden = true`（`DEL-006`），所以注销账号实际上已被过滤（推断）；`merged` 没有主动入口。
- 可选方案：A 显式要求 `accountStatus = active`；B 维持现状。
- 建议默认（建议）：A，配合 D-019 统一状态字段后，显式过滤更不容易出错。
- 影响模块：search、social

### 3.5 社交与经济（social-economy.md）

#### D-047 丁尼动作 `level_up`、`admin_grant` 没有写入方
- 涉及：social-economy.md `DENNY-003`、`DENNY-020`、`EXP-005`、未决问题 1；media-ops.md 第 13 节（后台需要调整丁尼）
- 旧系统现状：两者只存在于日志枚举；升级没有任何奖励；没有管理员发币接口。
- 可选方案：A 删除 `level_up`，把 `admin_grant` 做成带审计的后台调账接口；B 两者都实现；C 两者都删除。
- 建议默认（建议）：A，升级无奖励是现状，而后台确实需要调整丁尼。
- 影响模块：economy、admin

#### D-048 投币接口不检查文章是否被隐藏
- 涉及：social-economy.md `DENNY-008`、`TRIPLE-001`、未决问题 4
- 旧系统现状：投币只要求文章已发布；一键三连要求文章公开可见。
- 可选方案：A 投币也要求公开可见；B 维持现状。
- 建议默认（建议）：A，与三连和其他互动的可见性规则一致。
- 影响模块：economy

#### D-049 被动经验 `like_received`、`comment_received`、`favorite_received` 没有写入方
- 涉及：social-economy.md `EXP-009`、经验动作总表、未决问题 8；comments.md `CMT-104`
- 旧系统现状：只出现在被动桶分类中；目前只有被投币会给作者经验。
- 可选方案：A 维持不发放；B 实现。
- 建议默认（建议）：A，改变经济数值属于新需求。
- 影响模块：economy

#### D-050 采纳经验计入主动桶（每日 50）而不是被动桶
- 涉及：comments.md `CMT-103`、`Q-06`；social-economy.md `EXP-012`
- 旧系统现状：`comment_accepted`（每条 10 点）计入主动桶，实际上是主动桶上限的主要来源。
- 可选方案：A 维持主动桶；B 改为被动桶。
- 建议默认（建议）：A，保持经济数值与旧系统一致。
- 影响模块：economy

#### D-051 每日经验状态接口的 `todaySelfGained` 不包含采纳经验
- 涉及：social-economy.md `EXP-013`、未决问题 10
- 旧系统现状：只把签到、发帖、评论、点赞 4 项相加，与真实的主动桶用量不一致。
- 可选方案：A 按主动桶真实统计；B 维持现状。
- 建议默认（建议）：A，这是纯展示修正。
- 影响模块：economy

#### D-052 签到时账号被封禁返回 500
- 涉及：social-economy.md `CHECKIN-005`、未决问题 11
- 旧系统现状：事务内的账号检查失败被统一映射为 500 `CHECK_IN_FAILED`。
- 可选方案：A 返回 403 `ACCOUNT_BLOCKED`；B 维持现状。
- 建议默认（建议）：A，与其他带账号锁的写操作一致。
- 影响模块：economy

#### D-053 拉黑 check 接口只返回「我拉黑了 TA」
- 涉及：social-economy.md `BLOCK-010`、`PROFILE-021`、未决问题 12
- 旧系统现状：check 接口单向；作者主页会返回两个方向。
- 可选方案：A 维持单向；B 返回双向。
- 建议默认（建议）：A，不扩大「谁拉黑了我」的暴露面。
- 影响模块：social

#### D-054 @ 提及在解析正文时不做拉黑拦截
- 涉及：social-economy.md `BLOCK-022`、未决问题 13；comments.md `CMT-094`
- 旧系统现状：被拉黑的人仍可出现在正文的提及中，但不会收到通知，也不会出现在 @ 候选中。
- 可选方案：A 维持现状；B 解析时对有拉黑关系的对象不展开或拒绝发布。
- 建议默认（建议）：A，拒绝发布会向发布者暴露拉黑关系。
- 影响模块：content、social

#### D-055 主页统计 `totalComments`、`totalLikes` 包含匿名评论和隐藏评论
- 涉及：social-economy.md `PROFILE-015`、未决问题 15
- 旧系统现状：这两项统计的是作者非匿名文章下的全部评论和点赞，不过滤隐藏评论，包含他人的匿名评论。
- 可选方案：A 排除隐藏评论，保留匿名评论；B 维持现状。
- 建议默认（建议）：A，隐藏内容不应计入公开统计；匿名评论只计数，不暴露身份。
- 影响模块：social

#### D-056 关注列表的头像不看装备头像
- 涉及：social-economy.md `FOLLOW-006`、`COSMETIC-008`、未决问题 16
- 旧系统现状：关注列表只取 `author.avatar`，其他大多数列表是装备头像优先。
- 可选方案：A 统一为装备头像优先；B 维持现状。
- 建议默认（建议）：A，与其他列表一致。
- 影响模块：social

### 3.6 审核与考试（moderation-exam.md）

#### D-057 封禁没有解封入口、期限与原因记录
- 涉及：moderation-exam.md `RPTADM-063`、`RPTADM-100`、未决问题 1
- 旧系统现状：只能在 Strapi 内容管理器中手改 `blocked`。
- 可选方案：A 后台提供解封、可选期限和原因，期满由定时任务解封；B 只提供解封。
- 建议默认（建议）：至少 B，因为新后台没有内容管理器可以手改；期限与原因属于新需求，由产品决定。
- 影响模块：moderation、admin

#### D-058 被封禁用户已发布的内容是否隐藏
- 涉及：moderation-exam.md `SANCTION-009`、未决问题 2
- 旧系统现状：保持公开。
- 可选方案：A 维持公开；B 隐藏或降权。
- 建议默认（建议）：A，内容处理已有独立的隐藏与删除动作。
- 影响模块：moderation、content

#### D-059 `block_user` 是否主动撤销会话和断开实时连接
- 涉及：moderation-exam.md `SANCTION-005`、`SANCTION-006`、未决问题 3；auth-account.md `SESS-015`、`SESS-025`
- 旧系统现状：依赖每次请求查库，以及实时连接每 30 秒复检，最长约 30 秒才断开。
- 可选方案：A 封禁提交后执行 `revokeAll` 并广播实时撤销；B 维持被动检查。
- 建议默认（建议）：A，主动撤销成本低，并能立即关闭实时连接。
- 影响模块：moderation、auth、realtime

#### D-060 过期未提交的考试不计入失败，答案没有大小限制
- 涉及：moderation-exam.md `EXAM-030`、`EXAM-060`、`EXAM-062`、未决问题 4
- 旧系统现状：开考后不提交、等它过期，不计入冷却，可以反复开考看题；`answers` 原样存储。
- 可选方案：A 过期未交计一次失败，`answers` 只接受快照中存在的题目 id 和选项 key；B 维持现状。
- 建议默认（建议）：A，防止刷题看题，并限制写入大小。
- 影响模块：exam

#### D-061 删除任务没有最大重试次数和死信
- 涉及：moderation-exam.md `RPTADM-076`、未决问题 5
- 旧系统现状：失败后指数退避（上限 1 小时）无限重试，没有告警。
- 可选方案：A 设定最大次数，超过后转死信并在后台告警，举报保持 `reviewing + actionPending`；B 维持无限重试。
- 建议默认（建议）：A，graphile_worker 任务本身支持最大尝试次数。
- 影响模块：moderation

#### D-062 论坛 `isAdmin` 与后台管理员是否打通
- 涉及：moderation-exam.md `ADMIN-001`、`ADMIN-007`、未决问题 6
- 旧系统现状：两套账号互不关联，论坛管理员没有任何举报处理权限。
- 可选方案：A 维持分离；B 统一为论坛账号加后台角色。
- 建议默认（建议）：A，权限边界清晰，变更属于新的权限设计。
- 影响模块：admin、moderation、auth

#### D-063 是否允许举报已注销或已封禁的用户
- 涉及：moderation-exam.md `RPT-009`、未决问题 7
- 旧系统现状：user 目标只要求用户存在。
- 可选方案：A 拒绝 `deleted / merged`，允许 `blocked`；B 维持现状。
- 建议默认（建议）：A，注销账号已经无法被处置。
- 影响模块：moderation

#### D-064 考试奖励去重可能导致用户永远无法再次通过
- 涉及：moderation-exam.md `EXAM-040`、`EXAM-063`、未决问题 10；social-economy.md `DENNY-005`、`EXP-008`
- 旧系统现状：首次通过时任一奖励返回空值就回滚整次提交。如果 `examPassed` 被人工改回 false，再次通过时：`addExp` 去重跳过会返回 null（`EXP-008`）；`addDenny` 去重返回 `{existing: true}` 而非 null（`DENNY-005`）。所以至少 `addExp` 会触发回滚。
- 可选方案：A 奖励因已存在而跳过时视为成功，不回滚；B 维持现状。
- 建议默认（建议）：A，奖励幂等不应阻断通过状态的写入。
- 影响模块：exam、economy

#### D-065 管理后台的三个细节
- 涉及：moderation-exam.md「管理后台功能清单」、未决问题 9
- 旧系统现状：举报列表没有明确展示 `actionPending`（删除处理中）；题库批量删除接口存在但 UI 只有单条删除；题库插件的菜单文案和权限没有逐行核对。
- 可选方案：A 展示 `actionPending`，批量删除按需提供，菜单文案核对旧插件 `index.jsx` 后再定；B 全部维持旧 UI。
- 建议默认（建议）：A，`actionPending` 期间只允许再次提交删除，不展示会让管理员困惑。
- 影响模块：admin

### 3.7 账号与认证（auth-account.md）

#### D-066 `auth_login_attempts` 状态机没有任何代码推进
- 涉及：auth-account.md `LOCK-001`～`LOCK-005`、`MHY-002`、未决问题 3
- 旧系统现状：表和 10 态状态机已就绪，但米游社扫码流程使用自己的 Redis 键（会话、状态微缓存、claim 互斥、结果）。
- 可选方案：A 删除该模型，为米游社流程另建 PG 表；B 把米游社扫码迁移到该状态机。
- 建议默认（建议）：B，新设计没有 Redis，扫码会话、claim 和结果都需要 PG 存储，现成的状态机和哈希化的 poll secret 正好承接。
- 影响模块：auth

#### D-067 会话 `lastSeenAt` 只在创建和旋转时更新
- 涉及：auth-account.md `SESS-004`、`SESS-017`、未决问题 4
- 旧系统现状：100 个会话上限挤掉的是「最久未旋转」的家族，而不是「最久未活跃」的设备。
- 可选方案：A 维持现状；B 按请求节流刷新（例如每 N 分钟最多写一次）。
- 建议默认（建议）：B，设备列表的「最近活跃」和上限挤出才符合直觉；节流控制写放大。
- 影响模块：auth

#### D-068 没有密码复杂度、错误锁定、异地登录提醒
- 涉及：auth-account.md `ACC-011`、第 6 节章首注、未决问题 6
- 旧系统现状：密码只限制 6 位到 72 字节；防爆破只靠每 IP 每路由 20 次/分钟的限流。
- 可选方案：A 对齐旧行为；B 增强。
- 建议默认（建议）：A，增强功能另立需求。
- 影响模块：auth

#### D-069 米游社账号的占位邮箱
- 涉及：auth-account.md `ACC-013`、`DEL-001`、`MHY-014`、未决问题 8
- 旧系统现状：用 `mihoyo_<aid>@mihoyo-login.inter-knot.invalid` 表示「没有可用邮箱」，因为 email 列是必填的。
- 可选方案：A email 允许 NULL，迁移时把占位邮箱置为 NULL，并同步所有判定点；B 保留占位邮箱。
- 建议默认（建议）：A，`ACC-013` 本身就建议用独立表示替代占位邮箱，语义（不发码、不能邮箱登录、不能解绑米游社）必须保留。
- **裁决（2026-10-15）：A。** 米游社首登账号 `users.email` 为 NULL；旧数据迁移时把占位邮箱置为 NULL。
- 影响模块：auth

#### D-070 扫码确定性会话的 HMAC 密钥来源
- 涉及：auth-account.md `SESS-019`、未决问题 9
- 旧系统现状：`refreshToken = HMAC-SHA384(JWT_SECRET, ...)`，同一 ticket 永远兑换出同一个 token。
- 可选方案：A 使用独立配置的专用密钥；B 复用其他应用密钥。
- 建议默认（建议）：A，新设计不再有 JWT 密钥，专用密钥便于独立轮换。
- **裁决（2026-10-15）：A。** 专用密钥 `IK_SECRETS__QR_SESSION_KEY`（生产与多实例必填）。
- 影响模块：auth

#### D-071 注销没有宽限期，数据面是否合规
- 涉及：auth-account.md `DEL-006`、`DEL-010`、未决问题 10
- 旧系统现状：一经确认立即生效且不可撤回；只做作者身份脱敏和凭证销毁，不删除文章、评论、私信、互动记录、流水。
- 可选方案：A 对齐旧行为；B 引入宽限期或扩大删除范围。
- 建议默认（建议）：A 对齐，同时把数据面清单交产品和法务确认。
- 影响模块：auth、social

### 3.8 私信、通知与实时（messaging-notifications.md）

#### D-072 没有「谁可以私信我」的设置
- 涉及：messaging-notifications.md `CONV-013`、未决问题 1
- 旧系统现状：除拉黑和考试外没有任何 DM 准入策略。
- 可选方案：A 维持现状；B 引入设置（例如仅互关）。
- 建议默认（建议）：A，属于新需求。
- 影响模块：messaging

#### D-073 群聊从未上线
- 涉及：messaging-notifications.md `CONV-016`、`CONV-021`、事件表中的 `conversation.member.added`、未决问题 4
- 旧系统现状：schema 和角色模型齐备，但创建接口直接返回 501；`member.added` 事件没有发送点。
- 可选方案：A 首版不实现，保留 `kind` 枚举；B 按新设计实现。
- 建议默认（建议）：A，没有历史行为需要兼容。
- 影响模块：messaging、realtime

#### D-074 在线状态与头像堆叠没有可见性过滤
- 涉及：messaging-notifications.md `PRES-005`、未决问题 5
- 旧系统现状：在线人数和最多 8 个头像全站公开，不过滤 `profileHidden` 和拉黑关系。
- 可选方案：A 维持现状；B 头像堆叠排除 `profileHidden` 的用户，在线人数不变。
- 建议默认（建议）：B，设置了隐藏主页的用户不应在公共位置露出头像。
- 影响模块：realtime

#### D-075 未登录访问通知和敲敲列表返回空列表而不是 401
- 涉及：messaging-notifications.md `NOTIF-030`、`NOTIF-031`、`KNOCK-020`、未决问题 6
- 旧系统现状：未登录返回空列表或 `{count: 0}`。
- 可选方案：A 新 API 返回 401；B 维持空列表。
- 建议默认（建议）：A，新 API 不需要兼容旧形状，401 语义更明确。
- 影响模块：notify

#### D-076 `muted` 只是客户端偏好
- 涉及：messaging-notifications.md `CONV-015`、未决问题 7
- 旧系统现状：服务端不做任何推送或未读抑制。
- 可选方案：A 维持现状；B 服务端从红点总数中排除 muted 会话。
- 建议默认（建议）：A，属于新需求。
- 影响模块：messaging

### 3.9 媒体与运维（media-ops.md）

#### D-077 内容去重会把任意用户加入他人文件的 `uploaders`
- 涉及：media-ops.md `UPL-022`、`UPL-030`、`UPL-032`、未决问题 6
- 旧系统现状：签名阶段只要提交的 `contentHash` 命中任意文件，就直接复用并把当前用户加入 uploaders；知道一张公开图片的 SHA-256 即可把它放进自己的图库。
- 可选方案：A 签名阶段只对当前用户自己的文件去重；跨用户去重只在完成阶段（已上传并校验了字节）进行；B 维持现状；C 取消跨用户去重。
- 建议默认（建议）：A，完成阶段已经校验过字节，持有证明成立；签名阶段只凭哈希不足以证明持有。与 D-012 一起裁决。
- 影响模块：media

#### D-078 引用 GC 没有实现
- 涉及：media-ops.md `FILE-021`、`MEUP-004`、未决问题 8
- 旧系统现状：图库移除只断开关联，从不物理删除；正文内嵌的图片 URL 不形成关系，无法证明文件未被使用。
- 可选方案：A 首版继续只断开关联；B 实现同时扫描正文 URL 的 GC。
- 建议默认（建议）：A，误删的代价远大于存储成本。
- 影响模块：media

#### D-079 `DIRECT_UPLOAD_SNIFF_TIMEOUT_MS` 只出现在 `.env.example`
- 涉及：media-ops.md `UPL-042`、`UPL-061`、未决问题 9
- 旧系统现状：代码实际读取的是 `DIRECT_UPLOAD_VERIFY_TIMEOUT_MS`。
- 可选方案：A 删除该变量；B 与 verify 超时合并。
- 建议默认（建议）：A，它从未生效。
- 影响模块：media

#### D-080 `drain-follow-fanout` 没有设置时区
- 涉及：media-ops.md `CRON` 表、未决问题 10
- 旧系统现状：其余 cron 都设置 `Asia/Shanghai`；该任务每分钟执行，时区不影响触发。
- 可选方案：A 新的定时任务统一显式声明时区；B 不处理。
- 建议默认（建议）：A，统一声明可以避免以后改成按日执行时出错。
- 影响模块：ops

#### D-081 旧测试 `error-format.test.ts` 的日志期望与代码不一致
- 涉及：media-ops.md `ERR-006`、未决问题 2
- 旧系统现状：测试期望日志收到对象 `{requestId, error}`，代码写的是单行字符串；文档说明不影响重写语义。
- 可选方案：A 新实现按 `ERR-006` 的语义输出带 requestId 的结构化日志，不需要裁决；B 无。
- 建议默认（建议）：A，只记录，不阻塞。
- 影响模块：kernel

### 3.10 AI 助手（ai-assistant.md）

#### D-082 额度用尽或全局关闭时用户收不到任何提示
- 涉及：ai-assistant.md `TRIG-036`、`QUOTA-023`、未决问题 2
- 旧系统现状：任务被记为 skipped，私信中没有任何回应；输入框也不会因额度用尽而禁用。
- 可选方案：A 在私信中回一条不计费的系统提示（带重置时间）；B 前端根据额度禁用输入；C 维持现状。
- 建议默认（建议）：A，用户能明确知道原因，可复用 `QUOTA-023` 的 `resetAt`。
- 影响模块：ai、messaging

#### D-083 模型调用已开始但没拿到用量的失败按整份预占计费
- 涉及：ai-assistant.md `QUOTA-019`、未决问题 4
- 旧系统现状：这类失败按整份预占扣费，每次重试各扣一次；`quota.ts` 中「失败整份退回」的注释已过时。
- 可选方案：A 维持整份计费；B 失败一律整份退回；C 直连流式时用已收到的文本按 `GEN-082` 估算计费，拿不到才按整份。
- 建议默认（建议）：C，更接近真实消耗，同时仍然不会让花钱的闸门 fail-open。
- 影响模块：ai

#### D-084 AI 工具的匿名与拉黑口径不统一
- 涉及：ai-assistant.md `TOOL-004`、`TOOL-008`、工具明细表、未决问题 5
- 旧系统现状：`get_post_content`、`get_post_engagement` 可以读匿名帖（不输出作者）；旧 MCP 调用不传查看者，`get_related_posts` 实际按游客处理，拉黑过滤不生效。
- 可选方案：A 所有工具统一以触发者为查看者做拉黑过滤（匿名内容豁免），匿名帖可读但不输出作者；B 维持现状。
- 建议默认（建议）：A，与站内可见性一致，并守住匿名内容不做拉黑过滤的红线。
- 影响模块：ai

#### D-085 AI 工具按作者名模糊找人
- 涉及：ai-assistant.md `TOOL-005`、未决问题 6
- 旧系统现状：username 精确查不到时，按作者名「包含」匹配取第一条，可能返回其他用户的公开数据。
- 可选方案：A 只做精确匹配；B 维持现状。
- 建议默认（建议）：A，避免把别人的数据当成目标用户。
- 影响模块：ai

#### D-086 私信历史条数与工具循环轮数没有上限
- 涉及：ai-assistant.md `LLM-001`、`LLM-005`、`QUOTA-007`、未决问题 7
- 旧系统现状：由 AstrBot 托管，没有现成数值。
- 可选方案：A 做成配置项，初值按预占估算口径反推（`QUOTA-007` 假设 4000 上下文 token、2 次工具调用）；B 不设上限。
- 建议默认（建议）：A，上限必须与预占估算一致，否则并发闸门会失效。具体数值需新定。
- 影响模块：ai

#### D-087 群聊 @AI 用子串匹配
- 涉及：ai-assistant.md `TRIG-022`、未决问题 8；messaging-notifications.md `CONV-016`
- 旧系统现状：按小写子串匹配 `@<username>` 或 `@<作者名>`，可能误伤；群聊本身从未上线。
- 可选方案：A 改为提及 token 精确匹配；B 维持现状。
- 建议默认（建议）：A，群聊上线前没有实际影响。
- 影响模块：ai

#### D-088 编辑消息不重新触发 AI
- 涉及：ai-assistant.md `TRIG-020`、未决问题 9
- 旧系统现状：只有发送成功的文本消息触发。
- 可选方案：A 维持现状；B 编辑后重新触发。
- 建议默认（建议）：A，避免重复计费；用户可以用「重新生成」。
- 影响模块：ai

#### D-089 评论回复没有工作流时间线、停止和重新生成
- 涉及：ai-assistant.md `GEN-030`、未决问题 10
- 旧系统现状：评论回复非流式，一次性发出。
- 可选方案：A 维持现状；B 直连后补齐。
- 建议默认（建议）：A，属于新需求。
- 影响模块：ai

#### D-090 全站额度用尽只在审计和日志里可见
- 涉及：ai-assistant.md `QUOTA-009`、未决问题 11
- 旧系统现状：没有运营告警。
- 可选方案：A 增加告警指标；B 维持现状。
- 建议默认（建议）：A，成本低，可以避免全站 AI 静默失效。
- 影响模块：ai、ops

---

## 4. Strapi / AstrBot / Redis 特有、重写时不复刻的机制

下列机制只是旧框架或旧组件带来的实现细节，重写时不需要照搬，但右侧的用户可见不变量必须保持。

- **draft / published 双行模型与关系挂回**（posts.md `POST-001`、`POST-010`、`POST-056`、`POST-102`、`POST-116`；social-economy.md `LIKE-004`；comments.md `CMT-152`；feed-search.md `EDGE-020`）：发布、下线、编辑重发不得丢失点赞、评论、收藏、阅读、投币关系与计数；每篇文章只有一份计数。
- **documents API 不能读 private 字段导致的 seed 反查**（posts.md `POST-090`、`POST-116`）：`anonymousSeed` 永不下发，只用于派生展示名。
- **Meilisearch 中间件同步调用导致的事务拆分**（posts.md `POST-056`；feed-search.md `EDGE-014`、`EDGE-015`）：搜索索引在提交后异步同步，事务内不调用外部服务。
- **查询层怪癖**：`limit: 0` 等于不限（feed-search.md `EDGE-003`）、`$or` 对象键冲突（`EDGE-013`、comments.md `CMT-145`）、`updateMany` 关系过滤丢数据（posts.md `POST-116`、moderation-exam.md `RPTADM-104`）、`deleteMany` 不触发 lifecycle（comments.md `CMT-154`）、关系存于 `*_lnk` 联表只能逐条写（`RPTADM-095`）。不变量：本页全是特殊帖时不查 normal 流；拉黑条件与游标条件同时生效；级联删除必须补偿计数、通知和通知桶；审核组锁的语义保留。
- **schema sync 不建唯一约束、迁移先于 schema sync，所以索引和回填放在 bootstrap**（feed-search.md `EDGE-021`；media-ops.md `BOOT-003`～`BOOT-010`、`BOOT-016`；messaging-notifications.md `CRON-003`、`CRON-004`）：唯一约束、索引、回填全部用常规迁移声明。
- **core_store 标记的存量考试豁免**（moderation-exam.md `EXAM-014`；media-ops.md `BOOT-005`）：用一次性迁移把存量用户设为已通过，此后注册的用户不受影响。
- **strapi-cache 匿名响应缓存与 query 白名单**（media-ops.md `CACHE-001`～`CACHE-007`；feed-search.md `CACHE-006`～`CACHE-008`、`EDGE-011`、`EDGE-012`；comments.md `CMT-153`；posts.md `POST-078`、`POST-079`）：如果保留任何公开读缓存，缓存键必须包含全部有效参数（尤其 `cursor`、`tag`、`ids`），带鉴权的请求不走共享缓存，陈旧不超过 30 秒，缓存必须有命名空间隔离。
- **Koa 与 Strapi 的 HTTP 细节**：从原始 URL 兜底读 query（comments.md `CMT-153`）、`send` 第二参数才能写 201（`CMT-154`、auth-account.md `MW-005`）、409 被默认参数覆盖（moderation-exam.md `RPTADM-103`）、`proxy.koa` 配置（media-ops.md `RL-022`）、admin 页面 CSP（`ERR-011`）。不变量：状态码准确；反代终止 TLS 时安全 cookie 与客户端 IP 正确。
- **Strapi 上传插件细节**：`API Uploads` 年月文件夹（media-ops.md `UPL-017`、`UPL-064`）、provider delete 补丁（`FILE-010`）、`size` 以 KB 存储（`UPL-063`）、原生 multipart 上传的 `upload-validate`（`UPV-004`）。不变量：按 `provider_metadata.objectKey` 删除对象；所有上传入口复用同一套魔数嗅探；大小统一存字节。
- **users-permissions 的 JWT、refresh 与遗留 renew**（auth-account.md `LOGIN-012`、`SESS-021`、`SESS-022`、`MW-004`）：每次请求都能 O(1) 判定会话已撤销；任何旧凭证都不能绕过会话撤销。
- **零散 Strapi 产物**：经验日志 relatedId 存成字面量 `"undefined"`（social-economy.md `EXP-014`，不变量：缺省存 NULL）、CDN 地址改写规则（`COSMETIC-009`）、历史 like 升级显示为 favorite（messaging-notifications.md `NOTIF-011`，迁移后可能不需要）、MySQL 与 SQLite 方言分支（唯一冲突错误码、FULLTEXT；新系统只需 PostgreSQL）。
- **读时聚合与缓存分道等迁移期产物**（messaging-notifications.md `CONV-029`、`KNOCK-014`、`KNOCK-023`）：直接采用写时聚合的通知桶模型；降级值不写缓存；pseudo 会话 id 统一一种编码。
- **三条实时通道与进程内订阅表**（messaging-notifications.md `KNOCK-033`、`X-006`、`WS-004`；feed-search.md `SSE-008`）：合并为单一 WS 网关；单节点内存态要么分布式替代，要么显式接受单节点限制。
- **legacy Redis 上下文代际收养**（messaging-notifications.md `CONV-038`；ai-assistant.md `CTX-023`）：代际以数据库为准且只能前进，任何情况下都不能回退到 0 重新打开已重置之前的历史。
- **Redis 关注扇出队列、锁、inflight 与死信**（social-economy.md `FOLLOW-010`～`FOLLOW-019`；posts.md `POST-071`、`POST-120`；media-ops.md `CRON-011`）：首次发布才扇出；每个 (文章 documentId, 关注者) 7 天内只通知一次；落库失败可重试；收件人过滤规则不变。旧系统 Redis 不可用时通知直接丢失，新设计在发布事务内用 graphile_worker 入队即可消除这个缺口。
- **Redis 限流与计数**（media-ops.md `RL-001`～`RL-007`、`RL-030`；auth-account.md `RL-001`、`RL-002`；social-economy.md `BLOCK-006`；moderation-exam.md `RPT-040`～`RPT-043`；messaging-notifications.md `MSG-020`）：改用进程内 governor，阈值、窗口、429 响应体和 `Retry-After` 语义保留；计数与 TTL 不能出现「永不过期」的键。
- **Redis 缓存**：拉黑集合（social-economy.md `BLOCK-009`）、热榜与置顶（feed-search.md `CACHE-001`～`CACHE-003`）、DM 列表与桶快照（messaging-notifications.md `CONV-029`、`CONV-030`、`CONV-040`）。不变量：拉黑变更对双方立即生效；热榜刷新失败保留旧榜；通知变化后未读数与会话列表立即正确。
- **Redis 短期凭证与互斥键**：refresh 旋转 30 秒缓存（auth-account.md `SESS-010`）、WS ticket（messaging-notifications.md `WS-002`）、米游社扫码会话、claim 与结果（auth-account.md `MHY-002`、`MHY-006`～`MHY-008`）、停止生成标志（messaging-notifications.md `CONV-039`；ai-assistant.md `GEN-010`）、浏览量冷却（posts.md `POST-080`）。不变量：30 秒内重复旋转返回同一后继；ticket 30 秒内一次性；扫码确认只有一路处理且结果可重放；停止要真正停止；浏览量 5 分钟冷却且并发只计一次。
- **Redis pub/sub 实时撤销**（auth-account.md `SESS-025`；messaging-notifications.md `WS-008`）：撤销按 sid 精确关闭连接，并保留每 30 秒的 DB 权威复检（fail-closed）。
- **Redis 在线状态 ZSET**（messaging-notifications.md `PRES-003`、`PRES-004`）：45 秒在线窗口；统计失败时不跳成 0。
- **AI 配额的 Redis 计数、Lua 结算与进程内降级账本**（ai-assistant.md `QUOTA-010`～`QUOTA-016`、`QUOTA-030`）：计数放到 PostgreSQL；预占、结算、回收的原子性和幂等语义不变；花钱的闸门不得 fail-open。
- **AI 内部 HTTP 端点、`X-Agent-Secret`、Redis 可靠队列与租约头、outbox 补投**（ai-assistant.md `AI-025`、`AI-026`、`TRIG-030`～`TRIG-032`）：任务持久、至少执行一次、同一触发源只执行一次；失去任务所有权的执行者不能再写入。
- **AstrBot 会话 id、creator 自愈、固定身份**（ai-assistant.md 第 9 节）：私信记忆按（会话, 代际）隔离，评论记忆按（帖子, 评论者, AI）隔离；会话身份用稳定 id，改名不影响记忆；开发与生产数据不能串。
- **AstrBot 的协议细节**：HTTP 200 加业务错误信封、用量只在统计帧、停止需另调接口、`run_snapshot` 结果回收、SSE 累计帧与增量帧（ai-assistant.md 第 9 节）。不变量：上游错误不能当正文回给用户；必须拿到或估算用量；停止后截断的回答不得以全文进入记忆；首帧 90 秒、总预算 300 秒、退回默认模型一次。
- **AstrBot 的模型路由与人格**：`selected_provider`、provider 校验、`astrbotRef` 人格注入、`begin_dialogs` 改写（ai-assistant.md 第 9 节、`GEN-070`、`GEN-071`）。不变量：模型 key 到上游的映射只在服务端，非法时退回默认模型并告警；角色卡直接持有 system prompt；示例对话只能作为 few-shot 块，不得伪装成历史。
- **MCP JSON-RPC 服务**（ai-assistant.md `TOOL-001`、第 9 节）：工具改为进程内函数，仍然只有 12 个只读工具，结果格式（帖子 markdown 链接）不变。
