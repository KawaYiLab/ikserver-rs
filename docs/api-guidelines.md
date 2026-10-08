# API 设计规范（/v1）

本文件是 `/v1` 接口的唯一权威规范。OpenAPI 文档由 utoipa 从代码生成（`ik-server openapi`），
CI 用 oasdiff 阻止破坏性变更。

## 基础约定

- 基础地址：`https://api.<站点域名>/v1`（每个网页域名配同站 API 子域，Cookie 同站）。
- 全部 JSON；请求 `Content-Type: application/json`；响应 UTF-8。
- ID：字符串形式的 64 位雪花 ID（按时间有序），JSON 里永远是 string。
- 时间：RFC 3339（UTC，`Z` 结尾）。
- 字段命名：camelCase。空值省略字段（`skip_serializing_if`），不返回 null 占位。
- 语言：错误 `detail` 为中文；`code` 为稳定英文大写蛇形。

## 版本与兼容性

- `/v1` 内只做向后兼容的变更：新增字段、新增可选参数、新增接口、新增错误码。
- 禁止：删字段、改类型、改语义、收窄枚举、必填化参数、改路径。
- 破坏性变更必须走 `/v2`；旧版本至少维护到全部客户端升级。

## 认证

- 网页：`HttpOnly; Secure; SameSite=Lax` 会话 Cookie（`ik_session`）。
- App/脚本：`Authorization: Bearer <token>`。
- 不透明令牌（256 位随机，DB 存 SHA-256 哈希），30 天有效、活跃自动续期、最长 180 天。
- 会话可单独吊销；吊销即时生效并断开对应 WebSocket。
- CSRF：tower-http `CsrfLayer`（Sec-Fetch-Site + Origin 白名单 + Origin/Host 回退）。
  仅 Cookie 鉴权的非 GET 请求受约束；Bearer 请求天然免疫。

## 分页

一律游标分页：

```
GET /v1/posts?cursor=eyJ...&limit=20

200 {
  "items": [...],
  "nextCursor": "eyJ..."   // 没有更多时省略
}
```

- `limit` 默认 20，上限 50（管理后台表格上限 100，且允许 `page` 参数做页码分页）。
- 游标不透明（base64url 编码的服务端结构，含排序键与 tie-break id）；客户端禁止解析。
- 游标非法 → 400 `CURSOR_INVALID`（不得 500、不得静默回第一页）。
- 排序必须有唯一 tie-break（通常是 id），保证翻页稳定。

## 幂等与并发控制

- 创建类接口（发帖、评论、私信、举报等）接受 `Idempotency-Key: <uuid>` 头：
  同一用户 24h 内重放返回首次结果（含状态码），不重复创建。
- 互动类接口用幂等动词，不用 toggle：
  `PUT /v1/posts/{id}/like`（重复调用无副作用）、`DELETE /v1/posts/{id}/like`。
- 编辑类接口返回 `ETag: "rev:<n>"`；更新需带 `If-Match`，不匹配返回 412 `REVISION_CONFLICT`。

## 错误格式（RFC 9457 Problem Details）

```json
{
  "type": "https://api.interk.net/problems/exam-required",
  "title": "Forbidden",
  "status": 403,
  "detail": "需要先通过入站考试才能执行此操作",
  "code": "EXAM_REQUIRED",
  "requestId": "01925f..."
}
```

- 表单校验失败额外带 `errors: [{ "field": "title", "code": "TOO_LONG", "detail": "..." }]`。
- 5xx：`detail` 固定为通用文案，不含内部信息，仅 `requestId` 可追日志。

### 错误码登记表（新增错误码必须在此追加）

| code | status | 中文 detail（默认文案） | 说明 |
|---|---|---|---|
| VALIDATION_FAILED | 400 | 请求参数不合法 | 带 errors[]；也用于 JSON 字段缺失/类型错/多余字段 |
| INVALID_JSON | 400 | 请求体不是合法的 JSON | |
| CURSOR_INVALID | 400 | 分页游标无效 | |
| IDEMPOTENCY_KEY_INVALID | 400 | 幂等键格式不正确 | 需为 UUID |
| CODE_INVALID | 400 | 验证码错误或已过期 | 已使用的验证码同样返回此码 |
| UNAUTHORIZED | 401 | 请先登录 | 没带凭据 |
| SESSION_EXPIRED | 401 | 登录已过期，请重新登录 | 凭据无效/过期/已吊销；触发前端跳登录 |
| INVALID_CREDENTIALS | 401 | 账号或密码错误 | 账号不存在与密码错误不区分 |
| CSRF_REJECTED | 403 | 跨站请求被拒绝 | CsrfLayer 拒绝 |
| FORBIDDEN | 403 | 没有权限执行该操作 | |
| EXAM_REQUIRED | 403 | 需要先通过入站考试才能执行此操作 | |
| ANONYMOUS_POST_LOCKED | 403 | 匿名委托发布后不能再修改、下线或删除 | POST-043 |
| CATEGORY_ADMIN_ONLY | 403 | 只有管理员可以在该分区发帖 | |
| ACCOUNT_BLOCKED | 403 | 该账号已被封禁或已注销 | |
| REGISTRATION_CLOSED | 403 | 当前不允许注册 | `IK_AUTH__REGISTRATION_OPEN=false` |
| NOT_FOUND | 404 | 内容不存在或不可见 | 存在性探测统一 404；路径里的 id 格式不对也是 404 |
| ALREADY_EXISTS | 409 | 已存在相同记录 | 通用唯一冲突 |
| EMAIL_TAKEN | 409 | 该邮箱已注册 | 仅注册用途；登录/重置用途防枚举，不暴露 |
| NOT_PUBLISHED | 409 | 这篇帖子还没有发布 | 对草稿执行下线 / 丢弃修改 |
| REVISION_CONFLICT | 412 | 内容已在别处被修改，请刷新后重试 | If-Match 失败或格式不对 |
| PAYLOAD_TOO_LARGE | 413 | 请求体过大 | |
| UNSUPPORTED_MEDIA_TYPE | 415 | 请求体必须是 application/json | |
| RATE_LIMITED | 429 | 操作过于频繁，请稍后再试 | 带 `Retry-After`（秒） |
| CODE_COOLDOWN | 429 | 验证码发送过于频繁，请 N 秒后再试 | 仅注册用途；带 `Retry-After` |
| CODE_TOO_MANY_ATTEMPTS | 429 | 验证码错误次数过多，请重新获取 | 同一验证码错 5 次作废 |
| INTERNAL | 500 | 服务器开小差了，请稍后再试 | 仅 requestId |
| SERVICE_UNAVAILABLE | 503 | 服务暂时不可用，请稍后再试 | 外部依赖超时 |
| MAIL_SEND_FAILED | 503 | 验证码邮件发送失败，请稍后再试 | 5xx 统一文案，客户端按 code 区分 |

## 限流

- 被限流的 429 响应带 `Retry-After: <秒>`（已实现）。
- 计划：成功响应附带 `RateLimit-Limit`、`RateLimit-Remaining`、`RateLimit-Reset`（尚未实现）。
- 维度：IP（XFF 从右往左数 `IK_HTTP__TRUSTED_PROXY_HOPS` 跳）+ 用户 + 接口类别。
- 默认值：匿名读 300/min、匿名写 60/min、登录读 600/min、登录写 120/min；
  登录/验证码类另按 IP 20/min。具体接口在代码里用 helper 声明。

## 实时（WebSocket `/v1/realtime`）

握手：网页同站 Cookie；App 用 `Authorization` 头（WS 握手允许带 Authorization）。

```
客户端 → { "op": "subscribe",   "seq": 1, "t": "dm",        "d": { "conversationId": "..." } }
客户端 → { "op": "unsubscribe", "seq": 2, "t": "dm",        "d": { "conversationId": "..." } }
客户端 → { "op": "ping",        "seq": 3 }
客户端 → { "op": "typing",      "t": "dm", "d": { "conversationId": "..." } }

服务端 → { "op": "hello", "d": { "heartbeatSec": 30, "resumeHint": "..." } }
服务端 → { "op": "ack",   "seq": 1 }
服务端 → { "op": "event", "t": "dm.message.created", "d": { ... } }
服务端 → { "op": "error", "seq": 1, "d": { "code": "...", "detail": "..." } }
```

- 事件类型注册进 OpenAPI components（SDK 一并生成类型）。
- 限制：每用户最多 5 连接（挤掉最旧）；单帧 ≤ 64KB；订阅数上限 100；慢消费者断开。
- 心跳：服务端 30s 不活跃则发 ping 帧，客户端 2 个周期无响应视为断线。
- 断线补齐：`GET /v1/...?since=<cursor>`，WS 不保证不丢事件，REST 为准。

## 上传

两步直传（S3 预签名）：

1. `POST /v1/uploads/sign` → 返回 `{ uploadUrl, method, headers, uploadId, expiresAt }`；
   若内容哈希已存在（且归本人或可复用）直接返回已有媒体（`existing: true`）。
2. 客户端 PUT 到对象存储。
3. `POST /v1/uploads/{uploadId}/complete` → 服务端 HeadObject 比对大小 →
   带 If-Match(ETag) 读取 → SHA-256 + 魔数嗅探 → 发布到服务端随机新 key → 返回媒体对象。

- 限制：≤ 30 MiB；MIME 白名单 image/jpeg|png|gif|webp|avif；像素上限防爆。
- 媒体对象统一含 `{ id, url, width, height, thumbhash, mime, size }`。

## 缓存

- 公开 GET：`Cache-Control: public, max-age=<n>`（可交给 CDN）；带凭据的请求一律
  `Cache-Control: private, no-store`。
- 列表/详情支持 `ETag` + `If-None-Match`（304）。

## 管理后台

- 全部在 `/v1/admin/*` 下，要求 moderator/admin 角色。
- 写操作一律写审计日志（操作者、动作、目标、参数、IP、UA）。
- 列表接口允许 `page` 分页与排序参数（管理场景可用性优先）。
