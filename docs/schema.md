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