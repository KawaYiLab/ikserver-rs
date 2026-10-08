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
