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
| `username` | 1–32 字符、不含 `@`（结构上杜绝用户名与邮箱互相冒用），**大小写敏感**唯一（待裁决 D-018） |
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
- 匿名历史通知（D-032）暂未建模：新写入永远不产生匿名通知；迁移旧数据时再加列（expand 迁移，不破坏现有接口）。
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
