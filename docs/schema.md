# 数据库结构

权威来源是 `migrations/*.sql`；本文档解释“为什么这样设计”，随迁移同 PR 更新。

## 全局约定

- 主键：`BIGINT` 雪花 ID（`ik_kernel::id`，纪元 2026-01-01，41 位毫秒 / 10 位节点 / 12 位序号）。
  对外以字符串传输。不用 `SERIAL`/`UUID`：雪花 ID 天然按时间有序，可直接当游标 tie-break。
- 时间：一律 `TIMESTAMPTZ`，存 UTC。业务“天”（签到、经验日）在代码里按北京时间 04:00 切日。
- 枚举：`TEXT` + `CHECK (col IN (...))`，不用 PG `ENUM` 类型（增值要锁表、不能在事务里删值）。
  增加取值 = 新迁移里 `DROP CONSTRAINT` + `ADD CONSTRAINT ... NOT VALID` + `VALIDATE`。
- 规范化下沉到 DDL：如邮箱 `CHECK (email = lower(btrim(email)))`，代码写错会直接报错而不是悄悄存脏数据。
- 外键列必须有索引（PK 本身或显式索引）。
- `legacy_id`：旧 Strapi `documentId`，只用于数据迁移与旧链接 301 跳转，业务逻辑禁止依赖。

## 账号域（`20261007000001_accounts.sql`）

### `users`

| 列 | 说明 |
|---|---|
| `username` | 1–32 字符、不含 `@`（结构上杜绝用户名与邮箱互相冒用），**大小写敏感**唯一（D-018） |
| `email` | 规范化后存储；`NULL` = 无可用邮箱（米游社首登） |
| `status` | `active` / `blocked` / `deleted` / `merged` |
| `role` | `user` / `moderator` / `admin` |
| `is_ai_agent` | AI 账号，免考试可写 |
| `exam_passed_at` | 入站考试通过时间；`NULL` = 未通过（只读） |

### `user_passwords`

一对一拆表：没设置过密码的账号没有这一行，避免 `password_hash NULL` 的三态。
哈希串自描述算法（`$argon2id$…` / 旧 `$2a$…`），旧 bcrypt 在登录成功时透明升级为 Argon2id。

### `email_codes`

- 只存 `HMAC-SHA256(key, "purpose:email:code")`，库泄漏也无法还原或跨用途复用验证码。
- 有效 10 分钟；同一验证码最多错 5 次（`CHECK attempts <= 5`）；`consumed_at` 非空即已用。
- 校验与消费用 CAS（`UPDATE … WHERE consumed_at IS NULL`），并发重放只有一个成功。
- `email_codes_latest_idx (email, purpose, sent_at DESC, id DESC)`：取最新验证码、判断冷却。
- `email_codes_ip_recent_idx (request_ip, sent_at DESC)`：每 IP 每小时 20 封上限。
- 过期行由定时任务清理（待实现）。

### `sessions`

- 只存 `SHA-256(令牌)`；令牌原文 `iks_` + 43 位 base64url（256 位随机），只在签发时返回一次。
- `expires_at` 滑动续期（活跃时每小时最多写一次），永不超过 `max_expires_at`（签发起 180 天）。
- 吊销是软删除（`revoked_at` + `revoke_reason`），便于审计“哪台设备何时被踢”。
- 每用户活跃会话上限 100，超出时吊销最旧的（签发时持用户行锁，防并发越界）。
- `sessions_user_active_idx`：部分索引，只覆盖未吊销会话，用于设备列表与上限检查。
- `sessions_expiry_idx`：定时清理过期会话（待实现）。

### `users.level`（`20261008000001_user_level.sql`）

等级 0–7，0 = 未通过入站考试（D-017 方案 A）。经验值与升级逻辑归 economy 域，这里只存结果，
供鉴权上下文（`Principal.level`）和权益矩阵读取。

## 内容域（`20261008000002_content.sql`）

### `categories` / `tags`

- 分区 slug 限 `[a-z0-9-]`，由运维命令或管理后台创建；`admin_only`、`allow_comment_accept` 是普通布尔（不再有 NULL 兼容问题）。
- 标签 slug 由归一化名称派生并唯一；并发创建同名标签靠 `ON CONFLICT (slug) DO NOTHING` + 重查收敛。

### `posts`

- **一篇帖子永远只有一行。** 旧系统的草稿行 / 发布行双版本模型导致每次发布、下线、编辑都要把点赞、评论等关系重新挂回（POST-102）；
  新设计里发布只是改 `status`，关系天然保留。
- 已发布帖子的编辑先存进 `post_drafts`（每帖最多一行），线上内容不变；发布时写回 `posts` 并删除该行。草稿状态的帖子直接改 `posts`。
- 可见性：`hidden_at IS NULL`（D-009）。公开帖子 = `status = 'published' AND hidden_at IS NULL`，信息流的两个部分索引正好覆盖这个谓词。
- `bumped_at`：信息流排序键。首次发布、下线后重新发布、新评论时刷新；编辑重发只写 `edited_at`。
- `anonymous_seed`：TEXT，与旧系统同格式（32 位 hex），迁移后匿名展示名保持不变。
- `revision`：乐观并发版本号，每次改工作副本 +1，对外以 `ETag: "rev:<n>"` 暴露。
- 计数器（浏览、点赞、评论、收藏、丁尼）只由服务端维护，`CHECK >= 0`。
- `external_videos`：JSONB 数组，最多 1 项（CHECK）。每项是 `ik_content::ExternalVideo`：编号（`bvid` / `aid` / `page` / `cid`）、
  作者原始输入 `source`，以及服务端保存时向 B 站查到的 `title`、`coverUrl`（只认 `*.hdslb.com`）、`duration`；
  查询失败时沿用之前存下的、没有就省略。读取时逐项宽松解析（`video::decode_stored`）：旧系统导入的行字段不同
  （`p`、没有 `source`、客户端提交的 `embedUrl` / `coverUrl`），播放器地址一律按编号重建，封面不合规就丢弃。
- `updated_at` 只在内容变化时刷新（计数器不动它）；搜索索引的增量扫描按 `posts_updated_idx (updated_at, id)` 取改动的帖子（`20261101000001_posts_updated_idx.sql`）。
- 删除是物理删除，`post_tags`、`post_drafts` 级联删除；后续的评论、点赞等表同样级联。

## 评论域（`20261008000003_comments.sql`）

- 楼中楼只有一层：`parent_id` 必须指向同帖的一级评论（服务端校验），删除一级评论级联删除回复。
- `floor` 只属于一级评论（`CHECK (parent_id IS NULL) = (floor IS NOT NULL)`），在锁住帖子行后按 `MAX(floor)+1` 分配，删除不回收。
- 每帖最多一条置顶、一条采纳，由部分唯一索引兜底；`posts.accepted_comment_id` 外键 `ON DELETE SET NULL`。

## 经济域（`20261014000001_economy.sql`）

### `user_economy`

- 每个用户一行，**第一次发生经济行为时才创建**（`INSERT … ON CONFLICT DO NOTHING`）；没有行时读接口按初始值（经验 0、余额 30）返回。
- 余额 `CHECK (0..9999)`：所有入账统一按 9999 封顶，超出部分丢弃（D-002 方案 A）。
- 等级不存这里：结果写回 `users.level`（鉴权上下文直接读），由 economy 通过 `ik_auth::interop::set_level` 写入。
- 串行化：所有经济写入先锁 `users` 行（auth interop），再锁本行。多人时按用户 id 升序加锁；与帖子相关的写入先锁帖子行。

### `exp_logs`

- 只记实际发放量 > 0 的行；被每日上限截断为 0 时不写，按日去重的动作因此不会被占掉「今日首次」名额（EXP-010）。
- `dedup_key`：按日去重为 `day:<业务日>`，按实体去重为 `ref:<…>`；`UNIQUE (user_id, action, dedup_key)` 兜底并发。
- `bucket` 与 `biz_day` 冗余存储，按日统计额度只走 `exp_logs_daily_idx`。

### `denny_logs`

- `amount` 有符号，`balance` 为变动后余额。`ref_key` 非空时 `(user_id, action, ref_key)` 唯一，用于幂等入账（签到 `day:<业务日>`、考试 `user:<id>`、投币 `post:<id>` 等）。

### `check_ins` / `check_in_days`

- `check_ins` 主键 `(user_id, day)`：每人每个签到日一行。
- 当日排名由 `check_in_days` 计数器 `INSERT … ON CONFLICT DO UPDATE … RETURNING` 原子分配，唯一且连续。

### `post_denny_gifts`

- 主键 `(post_id, giver_id)`：每人每帖终身一次；金额固定 1。帖子删除时级联删除。
- 帖子上的 `denny_count` 由 content 在同一事务里 +1。

## 考试域（`20261014000002_exam.sql`）

### `exam_questions`

- `options` 为 `[{key,text}]`（2–26 项，key 不重复）；`correct_keys` 已归一化（去重、排序、都是选项 key）。
- 题型与选项数、答案数的匹配（单选 ≥2 选项 1 答案、多选 ≥3 选项 ≥2 答案、判断恰好 2 选项 1 答案）在写入时由 service 校验。
- 抽题时再归一化一次，不合格的题不进题池（兼容迁移来的旧数据）。

### `exam_attempts`

- `questions` 是开考时的完整快照（含正确答案与权重），判分、恢复、回顾只看快照；题库后来的改动不影响已开考的场次。解析例外：回顾时读题库当前内容（EXAM-029）。
- 结果列（`submitted_at`、`passed`、`score`、`score_percent`）要么全空（进行中），要么全有（已交卷），由 CHECK 保证。
- 每人最多一场未交卷的场次（部分唯一索引）。过期未交卷的场次在下次开考时按失败收尾，交卷时间记为过期时间，计入冷却（D-060 方案 A）。
- 通过状态写在 `users.exam_passed_at`（auth interop），丁尼、经验奖励经 economy 发放，与交卷在同一事务。奖励按 `user:<id>` 幂等，已发过视为成功（D-064 方案 A）。
- 旧系统存量用户的考试豁免（EXAM-014）在数据迁移脚本里处理：导入时把存量用户的 `exam_passed_at` 设为导入时间。
## 社交关系域（`20261015000001_social.sql`）

### `follows`

- 主键 `(follower_id, followee_id)`，`CHECK` 不能关注自己。旧系统关注的是「作者」，新系统作者就是用户。
- 粉丝数实时 `count(*)`（走 `follows_followee_idx`），不做物化列：写入频率低，且计数列应归 auth 的 `users` 表所有。

### `user_blocks`

- 主键 `(blocker_id, blocked_id)`，单向记录、不可修改，只能删除。
- 「任一方向」判断与列表过滤的排除集合由 `ik_social::interop` 提供，其他域不直接读这张表。
  列表 SQL 里写成 `(is_anonymous OR NOT (author_id = ANY($excluded)))`，匿名内容永远不过滤。
- 同一对用户之间的关注 / 拉黑写入用事务级 advisory lock（按两人 id 的有序对哈希）串行化，
  避免「关注」与「拉黑」并发提交后留下拉黑双方之间的关注。
## 点赞与收藏（`20261015000002_reactions.sql`）

- `post_likes`、`post_favorites` 主键 `(post_id, user_id)`，`comment_likes` 主键 `(comment_id, user_id)`：
  每人每个目标一条，并发重复点赞由主键收敛（`INSERT … ON CONFLICT DO NOTHING`，按是否真的插入决定计数是否 +1）。
- 计数是 `posts.like_count`、`posts.favorite_count`、`comments.like_count`，与关系行在同一事务里用
  单条 `UPDATE … GREATEST(x + delta, 0)` 增减（LIKE-003）。
- 帖子、评论、用户删除时级联删除。删除一级评论会级联删除回复，回复的点赞也随之删除（D-029）。
- 表归属：帖子的赞、藏归 content，评论的赞归 comments；拉黑判断经 `ik_social::interop`。
- 「我的点赞 / 收藏」按 `(user_id, created_at DESC, post_id DESC)` 索引分页，可见性与拉黑过滤都在 SQL 里完成。
## 通知域（`20261016000001_notifications.sql`）

- `kind` 八种；`system` 没有发送者（CHECK 保证），其余都有；`CHECK` 禁止给自己发。
- `bucket_key` 是敲敲会话键：`user-<发送者 id>` 或 `system`。会话列表在读取时按它聚合
  （递归 CTE 松散索引扫描 + 每组取最后一条 + 未读计数），**不维护分组表**：删除通知后会话自然一致，
  没有旧系统「分组表回退」那套乐观锁与幽灵会话问题（NOTIF-022～025）。代价与「不同发送者数」成正比；
  需要时可以加物化表，接口不变。
- `post_title`、`excerpt` 是写入时的快照，notify 不读 posts、comments 表（模块边界）。
  帖子、评论被删除时通知经外键级联删除（取代旧系统的手动级联与分组回退）。
- `dedup_key` + 部分唯一索引用于幂等：新帖扇出是 `follow:<帖子 id>`，同一帖子对同一人只通知一次。
- 新写入不产生匿名通知；旧系统匿名评论引起的通知在导入时转为系统通知，去掉发送者并保留摘要与帖子链接（D-032、ADR 0003）。
## 个人资料域（`20261017000001_profiles.sql`）

- `user_profiles` 每个用户一行，第一次修改资料时 upsert 创建；没有行时按默认值（空签名、不隐藏、未配置精选）。
- 用户名不在这里：仍是 `users.username`（auth 域），改名经 `ik_auth::interop::rename` 写入，与扣费同一事务。
  先改名（`UPDATE` 立即拿到用户行的排他锁，名字冲突立刻失败）再扣费；顺序反过来时，扣费拿的
  `FOR NO KEY UPDATE` 要在同一事务里升级成改唯一键所需的 `FOR UPDATE`，会与并发发帖的外键 `KEY SHARE` 互等。
- `pinned_post_ids BIGINT[]`：`NULL` = 未配置（主页按时间展示全部），`{}` = 主动清空，有序数组 = 精选（最多 6，CHECK）。
  只存 id，不加外键：帖子删除、下线、转匿名后，读取时自然跳过。
- 头像、名片（COSMETIC）随 media 模块加入。
## 阅读记录与热榜索引（`20261018000001_post_reads.sql`）

- `post_reads` 主键 `(user_id, post_id)`：有记录即已读。未看流用 `NOT EXISTS` 按主键探测，不把已读集合拉到应用层（UNRD-003）。
- 已读水位按 D-024 建议默认首版不实现。
- `post_likes (created_at)`、`comments (created_at)`：热榜按时间窗口扫描近期活动（HOT-005）。
- 热榜本身不落库：每个进程定时计算（默认 10 分钟）后放在内存里，计算失败保留上一份；多实例时各自计算，结果一致。
## 私信域（`20261019000001_messaging.sql`）

### `conversations`

- 只有 `direct`（D-073 群聊不实现）。`(user_low, user_high)` 唯一，较小的 id 在前，保证同一对用户至多一个会话（CONV-002）；
  并发发起靠 `ON CONFLICT DO NOTHING` 收敛。
- 没有 `last_message_id` / `last_message_at`：最后一条消息用 `messages_conversation_idx` 现查（一行 LIMIT 1），不维护指针。

### `conversation_members`

- 每个会话两行，`peer_id` 反范式记录对方，`(user_id, peer_id)` 唯一：通知到达时按对方找会话推进排序、收件箱按对方合并通知分组，
  都不必关联 `conversations`。
- `activity_at` 是收件箱排序键，按成员各自维护、只前进（`GREATEST`）：新消息推进双方，对方的通知只推进收件人。
- `left_at` 非空 = 从自己的列表移除；新消息会清空它（对方重新看到会话），通知不会。新建会话时对方那一行先带 `left_at`（隐藏），
  第一条消息到达时出现。
- `pinned_at` 非空 = 置顶，每人最多 50 个（写入时锁用户行再计数；历史超限照样全部返回）。
- 未读 = 晚于 `last_read_at`、别人发的、未撤回的消息，读取时现算，不维护计数器（旧系统的经验：入口多，计数器必漂）。
- `conversation_members_inbox_idx` 是部分索引（在场且未置顶），对应收件箱的分页流；置顶的单独取。

### `messages`

- `kind` 只有 `text`（D-021 图片消息不实现）与预留给 AI 会话分界的 `system`。正文 1～4000 字由 CHECK 兜底。
- 撤回只写 `withdrawn_at`，行与正文保留（举报复核需要），接口层对所有人隐藏正文。
- `(sender_id, client_id)` 部分唯一索引实现发送去重。
- 收件箱时间线（`/v1/inbox/dm-<id>/items`）把这里的消息与 `notifications` 里对方发来的通知按 `(created_at, id)` 归并，
  同一时刻通知在前；两边各自走自己的索引，不跨表 JOIN。

## 媒体域（`20261020000001_media.sql`）

### `media_files`

- 只存校验通过、已发布的图片：`object_key` 是服务端生成的最终 key（客户端拿到的预签名地址只能写暂存 key，UPL-052）；
  `mime` 来自文件头嗅探；`size_bytes` 统一用字节（旧系统是 KB，UPL-063）；`width`/`height` 是客户端上报的。
- `sha256` 是服务端对读回的字节计算的哈希，去重只信它（UPL-051）。签名阶段只按「自己上传过的」去重，
  跨用户去重只在完成阶段字节已校验之后（D-077 方案 A）。
- `receipt` = 完成凭证字符串的 SHA-256，唯一：同一凭证的重试与并发完成收敛到同一行（UPL-014/035）。
- 文件从不物理删除（D-078 方案 A）；对象存储里的暂存对象完成后删除，没完成的靠存储的生命周期规则清理（UPL-060）。

### `media_uploaders`

- 文件 ↔ 上传者，多对多。永不对外暴露（防匿名内容溯源，UPL-056）。「我的图库」就是这张表；从图库移除只删这一行。
- `(user_id, created_at DESC, media_id DESC)` 支撑图库分页。

## 媒体引用（`20261020000002_profile_avatar.sql`、`20261021000001_media_references.sql`）

- 头像：`users.avatar_media_id`（auth 域的列）。最初放在 `user_profiles`（20261020000002），因为帖子卡片、评论、通知、
  私信里的作者展示都要带头像，而这些域只能经 auth 的作者资料读用户，所以挪到 `users`；旧列没有上线过，迁移里直接删除。
  设置由 profile 经 `ik_auth::interop::set_avatar` 写入，事先经 `ik_media::interop::owned_media` 校验归属，与扣费同一事务。
- 帖子封面：`posts.cover_ids` / `post_drafts.cover_ids`（`BIGINT[]`，有序，≤ 40）；评论图片：`comments.image_ids`（≤ 18）。
  文件从不物理删除，所以用数组而不是关系表，也不需要外键；数量上限随等级在服务端校验，DDL 只兜底最高档。
  卡片查询直接取 `cover_ids[1]`。
- `media_files.thumbhash`：完成上传时在阻塞线程里解码（限制 16384 像素、256 MiB）并缩到 100×100 计算。

## 后台任务队列（graphile_worker）

- schema `graphile_worker` 由 graphile_worker 自己迁移（执行器启动、`ik-server migrate`），不进 `migrations/`，
  也不归任何业务域；业务代码只经 `ik_kernel::jobs::Jobs::enqueue` 在自己的事务里写入。

## 幂等键（`20261022000001_idempotency_keys.sql`）

- `idempotency_keys` 属于 ik-server 的横切中间件（`crates/server/src/idempotency.rs`），不属于任何业务域。
  主键 `(user_id, key)`；`fingerprint` 是请求的 SHA-256；处理中 `locked_until` 非空，完成后存状态码、
  需要重放的响应头与响应体（CHECK 约束保证两种状态互斥，只存 2xx）。
- 占用的身份是 `created_at`（截断到微秒），完成与释放都按它比对，避免失效后被别的请求接手时误改。
- 每日定时任务删除超过 24 小时的键（`created_at` 索引）。

## 集群事件转运（`20261022000002_bus_outbox.sql`）

- `bus_outbox` 属于 `ik_kernel::cluster`：放不进 NOTIFY 载荷（约 8000 字节）的事件写在这里，通知只带 id。
  只是转运，发布方每 5 分钟删除 10 分钟前的行；没有外键，不归任何业务域。

## 装扮（`20261023000001_cosmetics.sql`）

- `system_avatars`、`business_cards` 归 ik-cosmetics；`kind ∈ {character, city, news}`，名字唯一（CLI 导入按名字新增或更新）。
  列表按 `(kind, id)` 排序，雪花 id 即上架先后。
- 装备是账号上的列（auth 的表）：`equipped_avatar_id`、`equipped_card_id`，以及冗余的 `equipped_avatar_media_id`
  （作者展示经 auth 一次取到 `COALESCE(equipped_avatar_media_id, avatar_media_id)`）。CHECK 保证系统头像 id 与图片 id 同时有或同时无；
  外键不级联，删除目录项前须先卸下。

## 米游社（`20261024000001_mihoyo.sql`）

- `mihoyo_bindings` 即米游社身份（替代旧的 `user_identities` 里 provider = mihoyo 的行与 `mihoyo_bindings` 两处）：
  主键 `user_id`（一个账号最多一个米游社账号），`aid` 唯一、`mid` 部分唯一（一个米游社账号最多绑一个账号）。
  解析时 aid、mid 指向不同账号 → `IDENTITY_CONFLICT`，由运维处理，不自动合并。绝区零角色字段拉取失败时为空。
- `mihoyo_qr_logins` 替代旧系统的 Redis 键（会话、状态微缓存、处理互斥、结果）。主键是客户端 qrId 的 SHA-256，
  米游社 ticket 只在服务端。`claimed_until` 是「已确认」的处理权，`result` 是可重放的处理结果
  （成功 5 分钟、失败 1 分钟）。每小时的定时任务清理过期行。
- 米游社首登的账号 `users.email` 为 NULL（D-069 方案 A），没有 `user_passwords` 行。

## 账号安全（`20261025000001_account_security.sql`）

- `sessions.login_method` 加 `password_change`（修改密码后给当前设备签的会话）。
- `mihoyo_qr_logins.mode` 加 `delete`（扫码注销核验），与 `bind` 一样必须有发起的 `user_id`。
- 注销不删行：`users.status = 'deleted'`、用户名改为「已注销用户_<id>」、`email` 置 NULL、删 `user_passwords` 与
  `mihoyo_bindings` 行、清 `user_profiles` 的签名与精选。

## 举报与审核（`20261026000001_moderation.sql`）

- `reports` 归 ik-moderation。目标用 `(target_type, target_id)` 表示（帖子、评论、用户的雪花 id，内容删除后不变，
  替代旧的 `targetKey` 与关系回退，RPT-004～006）。`owner_user_id` 是作者或被举报的用户，匿名内容也记录（处置要用），
  但不出现在任何接口里。`snapshot` 存举报时的标题、摘要、图片 id、是否匿名。
- 部分唯一索引 `reports_open_uq`：同一人对同一目标只能有一条未结案的举报（RPT-011 下沉到 DDL）。
- CHECK 保证结案与 `resolution`、`handled_at` 同时出现；领取人随账号删除置空。
- 没有旧系统的 `report_actions` 删除任务表：删除内容与结案在同一个事务里完成（见业务规则差异）。
- `moderation_log`：每次自动隐藏、结案、解封一行（操作人、动作、目标、涉及的举报条数、备注）。
- 隐藏原因（`posts.hidden_reason`、`comments.hidden_reason`）：`auto_report`（多人举报自动隐藏，驳回时恢复）、
  `moderator`（版主隐藏）。

## 管理后台的内容管理（`20261103000001_admin_content_management.sql`）

- `posts_hidden_idx`、`comments_hidden_idx`：`(hidden_at DESC, id DESC) WHERE hidden_at IS NOT NULL`，后台「只看已隐藏」用。
- `moderation_log.action` 增加 `unhide_content`（后台直接恢复可见）。版主不经举报直接隐藏、删除同样记 `hide_content`、
  `delete_content`，`report_count = 0`。
- `moderation_log.subject_user_id` / `subject_anonymous`：被处置的人（内容作者或被处置的用户）以及处置的是不是匿名内容。
  匿名内容同样记录真实作者（与 `reports.owner_user_id` 一样只存不出），按人查询处罚记录时必须排除
  `subject_anonymous`（AGENTS.md 规则 10）。旧记录在迁移里回填：用户类取 `target_id`，内容类取同一目标最近一条举报的
  `owner_user_id` 与快照里的匿名标记，查不到的保持 NULL。索引 `moderation_log_subject_idx (subject_user_id, created_at DESC)`。

## 管理后台的账号详情（`20261104000001_admin_user_detail.sql`）

- `exp_logs_user_created_idx (user_id, created_at DESC, id DESC)`：账号详情的经验流水按时间翻页
  （原有的 `(user_id, biz_day, bucket)` 只服务每日上限统计）。
- `reports_owner_idx (owner_user_id, created_at DESC) WHERE owner_user_id IS NOT NULL`：某人收到的举报。
  查询时按 `snapshot ->> 'anonymous'` 排除匿名内容的举报（AGENTS.md 规则 10）。

## 管理后台工作台的趋势（`20261105000001_admin_dashboard_trends.sql`）

- `users_created_idx`、`posts_first_published_idx`（部分索引）、`reports_created_idx`：按业务日统计新增账号、首次发布的帖子、
  举报（评论复用 `comments_recent_idx`）。分桶用 `width_bucket(ts, $bounds)`，边界由 `ik_kernel::time::biz_day_bounds`
  算好传进去，SQL 里不写时区偏移。

## 会员号（`20261106000001_user_member_no.sql`）

- `users.member_no`：对外展示的「第 N 位用户」编号，唯一、非空，默认取序列 `users_member_no_seq`。主键仍是雪花 `id`
  （规则 8），会员号只用于展示和后台按号查找，不当外键用。
- 从旧系统迁移来的账号沿用旧 id（用户以前看到的 UID）；新注册的接在最大旧 id 之后递增。迁移时按 `id < 2^22`
  判断旧账号（雪花 ID 的时间戳部分大于 0，一定不小于 2^22），其余已有账号按注册时间补号。
  `ik-legacy` 导入时写入旧 id 并把序列推到最大值之后（导入只对全新的库执行）。
- 列带默认值，旧版本二进制插入账号时同样拿到编号，新旧版本可以并行。

## AI 助手（`20261027000001_ai.sql`）

- 表都归 ik-ai。`ai_settings` 单行（`id = 1` 的 CHECK），两个上限的单位是加权 token，0 表示不限制（AI-001）。
- `ai_models`：`key` 对外；`provider`（提供方名字，见下面的 `ai_providers`）与 `provider_model` 是上游路由，只在服务端流转。
  部分唯一索引保证最多一条默认模型，CHECK 保证默认模型启用（D-005 方案 A：默认模型是一条有自己倍率的记录）。
- `ai_role_cards`：角色卡直接持有 system prompt 与示例对话（`examples` JSONB，few-shot，GEN-071）；
  `allowed_model_ids` 为空表示不限制；「某用户的角色卡」= 启用的、按 `sort_order, id` 取第一张（部分索引）。
- 额度账本（替代旧 Redis + Lua）：`ai_quota_counters(day, user_id)`，`user_id = 0` 是全站，`day` 是北京时间
  04:00 切换的配额日；计数含在飞预占，并发请求在行锁上串行。`ai_quota_holds` 一份预占一行，删除即认领；
  过期的由每分钟的定时任务回收。`ai_quota_settlements` 是结算去重标记（键 `hold:<id>` 或 `exec:<执行 id>`，
  保留 48 小时）。计数器保留 90 天。

## AI 上游提供方（`20261108000001_ai_providers.sql`）

- `ai_providers` 归 ik-ai：管理后台维护的 OpenAI 兼容上游（`name`、`base_url`、`api_key`、启停）。模型按 `name` 引用，
  不建外键：部署配置（`IK_AI__PROVIDERS__<名字>__…`）里的提供方不在表里，名字也可能暂时指向不存在的提供方
  （调用时退回默认模型，LLM-011）。同名时以部署配置为准，后台不允许新建同名的。
- `api_key` 明文存储、只在服务端流转：后台接口只回末四位，审计只记「是否更换」。与配置快照一起缓存 60 秒。

## AI 调用 MCP 工具（`20261109000001_ai_mcp.sql`，ADR 0004）

- `ai_mcp_servers` 归 ik-ai：管理后台维护的站外 MCP 服务（Streamable HTTP）。`name` 是暴露给模型的工具名前缀
  （`^[a-z0-9][a-z0-9_-]{0,31}$`）；`auth_header` 是整行请求头，明文存储、只在服务端流转，后台接口只回名字加值的末四位，
  审计只记「是否更换」。`last_listed_at` / `last_error` 记最近一次拉取工具清单的结果。
- `ai_mcp_tools`：每个服务的工具。`remote` 是最近一次拉到的定义（`{title, description, inputSchema, readOnly}`），
  `approved` 是管理员确认过的定义，`*_digest` 是键排序后的 JSON 的 SHA-256。运行时只用 `approved`，且只在
  `enabled AND NOT missing AND approved_digest = remote_digest` 时提供：远端改了定义就自动停用，等管理员重新确认。
  CHECK 约束保证只有确认过的只读工具能启用。`exposed_name`（`<服务名>__<工具名>`）全表唯一；写入清单时先拿
  `pg_advisory_xact_lock(hashtextextended('ik:ai_mcp_tools', 0))` 再查冲突。外键 `server_id` 由唯一约束
  `(server_id, remote_name)` 的索引覆盖，删服务级联删工具。
- `ai_role_cards.story_tools`：是否提供剧情库工具；`mcp_server_ids`：启用的站外服务（与 `allowed_model_ids` 一样用数组，
  不建外键；删服务时用 `array_remove` 从各角色卡里去掉）。

## AI 私聊（`20261028000001_ai_sessions.sql`）

- messaging 的表：`conversations.kind` 增加 `ai`（AI 会话，同一对用户可以有多条，CONV-012），`title` 只有 AI 会话有。
  「同一对用户至多一个会话」改为只约束 direct 的部分唯一索引 `conversations_direct_pair_uq`；成员的
  `(user_id, peer_id)` 唯一索引改为普通索引（direct 的唯一性由会话保证）。通知只合并进 direct 会话（AI-016）。
- `messages.ai_state`（`streaming` / `done` / `failed` / `stopped`）与 `ai_model_key` 是 AI 回复的展示字段；
  `streaming` 的占位正文可以为空（替换了原来的长度 CHECK）。
- `ai_sessions`（ik-ai）：每条 AI 会话一行，`context_epoch` 与 `context_from`（最近一次重置的时刻，AI 只读之后的消息）、
  会话级模型 `model_key`（存 key，停用或移出白名单时静默跳过）。
- `ai_jobs`（ik-ai）：任务与审计合一（AI-008）。`execution_key` 唯一去重（`dm:<触发消息>`、`regenerate:<旧回复>`），
  `reply_message_id` 唯一（同一任务重试复用同一条占位），`stop_requested_at` 是停止信号（用户停止、重置、撤回触发消息），
  用量逐次累加。旧系统每次尝试一行审计（TRIG-035），这里只保留最后一次尝试的原因与累计用量。

## AI 评论回复（`20261029000001_ai_comment_jobs.sql`）

- 评论任务与私信任务共用 `ai_jobs`：`kind = 'comment'`，去重键 `comment:<触发评论 id>`（同一条评论只回复一次）。
  新增 `post_id`、`trigger_comment_id`、`reply_comment_id`，都是 `ON DELETE SET NULL`：评论或帖子删除后审计行保留，
  执行时复核触发评论仍可见（TRIG-016）。`reply_comment_id` 唯一（一个任务至多一条回复，重放直接结束）。
- 评论记忆（CTX-003）按（帖子、评论者、AI）取之前已回复的问答：部分索引
  `ai_jobs_comment_memory_idx (post_id, quota_user_id, ai_user_id, created_at) WHERE post_id IS NOT NULL`，
  也兼作 `post_id` 外键索引。
- AI 的回复就是一条普通的实名评论（`author_id` 是 AI 账号，`parent_id` 是触发评论所在的一级评论，D-004 方案 A）；
  `comments` 表本身没有改动。

## AI 工作流时间线（`20261030000001_ai_workflow.sql`）

- `messages.ai_workflow JSONB`：AI 回复定稿时写入整条时间线（WF-003），只存**已脱敏**的客户端形态
  （思考文本、工具参数与结果从不进入事件），最多 1000 个事件（超限先丢最早的 `reasoning.delta`，WF-005）。
  同一占位重试时清空为 `[]`，seq 与 stepId 按尝试次数换起点继续递增（WF-006）。CHECK：只有 AI 回复有，且是数组。

## 管理后台审计（`20261031000001_admin_audit.sql`）

- `admin_audit_log`（ik-auth）：每个 `/v1/admin/*` 写操作与业务写入同一事务记一行：`actor_id`（账号删除后置空）、
  `action`（`<域>.<对象>.<动作>`）、`target_type` / `target_id`（id、key 或 slug，批量操作为 NULL）、
  `details`（参数摘要，不含密钥类字段）、`ip`（INET）、`user_agent`。举报结案另有业务上的 `moderation_log`，两者并存。

## 站点设置（`20261110000001_site_settings.sql`）

- `site_settings`（ik-auth）：后台「系统设置」里随时切换、立即生效的开关，只有一行（`id = 1`，CHECK 约束）。
  目前只有 `exam_required`（入站考试开关，默认开启）；`updated_at` 为空表示从没在后台改过。
- 读取方（会话解析、`lock_active_user`、`/v1/me`）在各自的查询里用标量子查询顺带读出，不多一次往返，多实例之间也不用失效缓存；
  缺行时按默认值（开启）处理，误删这一行不会放开门控。后台修改先补行再 `FOR UPDATE`，与审计同一事务。

## 管理后台调整丁尼与经验（`20261031000002_admin_economy_actions.sql`）

- `denny_logs.action` 增加 `admin_deduct`（管理员扣除；发放沿用 `admin_grant`，DENNY-020 的枚举第一次有写入方）；
  `exp_logs.action` 增加 `admin_grant`（豁免每日上限，去重键 `ref:admin:<雪花 id>`，每次发放一行）。

## 在线人数（`20261102000001_realtime_presence.sql`）

- `realtime_presence`（ik-realtime）：每个实例一行（`node_id` = `IK_NODE_ID`），每 15 秒写入本实例实时连接的快照：
  登录用户 id 数组（最近建立连接的在前）与游客连接数。读时只合并 45 秒内更新过的行（PRES-003 的在线窗口），
  登录用户跨实例去重；实例宕机后它的行自然过期，正常停机时删掉自己的行。
- `UNLOGGED`：数据只有几十秒的意义，不写 WAL，崩溃后清空也无妨。没有外键（快照不需要引用完整性）。

## 表情包（`20261102000002_emotes.sql`）

- `emote_groups`、`emotes` 归 ik-cosmetics，只由管理员维护。评论正文里写作 `:ik-<code>:`，`code` 的 CHECK 与评论里的
  匹配正则一致（CMT-008），唯一且建好后不能改（旧评论靠它渲染）。
- `active = false`：下架，不出现在表情面板，但仍在公开清单里（`selectable = false`），旧评论照常显示成图片。
- 删除分组时组里的表情变成未分组（`ON DELETE SET NULL`）；表情与分组图标引用 `media_files`（不级联，图片不会被单独删除）。

## 剧情库（`20261107000001_story_corpora.sql`）

- `story_corpora` 归 ik-story：每行是一次上传的语料包（zzz-story-extractor 的产出打成的 gzip JSON），原样保存在
  `bundle`；索引不入库，各实例启动或切换版本时从包构建，所以索引结构改了也不用迁移数据。
- `active`：至多一行（部分唯一索引 `story_corpora_active_uq`，也是启动时查生效版本用的索引）。「换生效版本」在事务里先拿
  `pg_advisory_xact_lock(hashtextextended('ik:story_corpora', 0))` 再改。
- `bundle_sha256`：同一个包重复上传时复用已有行，只把它设为生效。只保留最近 5 个版本（生效版本总是保留）。
- `uploaded_by → users`（不级联；注销账号是脱敏不是删除）。
