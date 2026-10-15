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

- 已登录用户的 `POST` 请求可以带 `Idempotency-Key: <uuid>` 头（创建类接口：发帖、评论、私信、举报等应当带）：
  同一用户同一个键 24h 内只处理一次，重放返回第一次的状态码、响应体和 `Content-Type` / `ETag` / `Location`，
  并带 `Idempotent-Replayed: true`。
  - 只保存成功（2xx）的结果；失败的请求没有副作用，不占用这个键，改正后可以用同一个键重试。
  - 同一个键用在不同的请求上（方法、路径、查询、请求体任一不同）→ 422 `IDEMPOTENCY_KEY_REUSED`；
    第一次还在处理 → 409 `IDEMPOTENCY_IN_PROGRESS`（`Retry-After: 1`）；键不是 UUID → 400 `IDEMPOTENCY_KEY_INVALID`。
  - 未登录的请求忽略这个头。客户端断开或请求超时不会中断处理，结果照样记下。
  - 已知限制：实例在业务提交之后、记下结果之前崩溃时，占用在请求超时 + 30 秒后失效，之后的重试会再处理一次。
- 发私信另有请求体里的 `clientId` 去重：同一发送者重复提交返回第一次的消息（200 而非 201），不限 24h。
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
| COMMENT_NOT_TOP_LEVEL | 400 | 只有一级评论可以置顶或采纳 | |
| FOLLOW_SELF | 400 | 不能关注自己 | |
| UPLOAD_TOKEN_INVALID | 400 | 上传凭证无效 | 格式或签名不对，或不是自己的凭证 |
| UPLOAD_TOKEN_EXPIRED | 400 | 上传凭证已过期，请重新上传 | 签名后 10 分钟内完成 |
| UPLOAD_NOT_FOUND | 400 | 没有找到已上传的文件，请重新上传 | 还没 PUT，或暂存对象已被清理 |
| UPLOAD_SIZE_MISMATCH | 400 | 上传的文件大小与声明不符 | 也包括读取过程中对象被替换 |
| UPLOAD_CHECKSUM_MISMATCH | 400 | 上传的文件内容与声明的哈希不符 | |
| UPLOAD_NOT_IMAGE | 400 | 上传的文件不是支持的图片格式 | 按文件头嗅探，只认 jpeg/png/gif/webp/avif |
| DM_SELF | 400 | 不能给自己发私信 | |
| REPLY_TARGET_INVALID | 400 | 被回复的消息不存在或已撤回 | 也包括别的会话里的消息 |
| BLOCK_SELF | 400 | 不能拉黑自己 | |
| BLOCK_SYSTEM_ACCOUNT | 400 | 不能拉黑系统账号 | AI 账号不能被拉黑 |
| UNAUTHORIZED | 401 | 请先登录 | 没带凭据 |
| SESSION_EXPIRED | 401 | 登录已过期，请重新登录 | 凭据无效/过期/已吊销；触发前端跳登录 |
| INVALID_CREDENTIALS | 401 | 账号或密码错误 | 账号不存在与密码错误不区分 |
| INSUFFICIENT_DENNY | 402 | 丁尼余额不足 | 投币、改名、自定义头像等扣费操作 |
| CSRF_REJECTED | 403 | 跨站请求被拒绝 | CsrfLayer 拒绝 |
| FORBIDDEN | 403 | 没有权限执行该操作 | |
| EXAM_REQUIRED | 403 | 需要先通过入站考试才能执行此操作 | |
| ANONYMOUS_POST_LOCKED | 403 | 匿名委托发布后不能再修改、下线或删除 | POST-043 |
| CATEGORY_ADMIN_ONLY | 403 | 只有管理员可以在该分区发帖 | |
| ACCOUNT_BLOCKED | 403 | 该账号已被封禁或已注销 | |
| ACCEPT_DISABLED | 403 | 该分区未开启评论采纳 | CMT-039 |
| DENNY_SELF_GIVE | 403 | 不能给自己的委托投丁尼 | DENNY-008 |
| DENNY_ANONYMOUS_POST | 403 | 不能给匿名委托投丁尼 | DENNY-008 |
| USER_BLOCKED | 403 | 你与对方存在拉黑关系，无法进行此操作 | 任一方向的拉黑；不说明谁拉黑了谁。查看帖子详情时文案为「无法查看该委托」。匿名内容永不返回此码 |
| REGISTRATION_CLOSED | 403 | 当前不允许注册 | `IK_AUTH__REGISTRATION_OPEN=false` |
| MIHOYO_BIND_REQUIRES_AUTH | 401 | 该操作需要先登录 | 生成绑定用的二维码时没有登录 |
| MIHOYO_BIND_USER_MISMATCH | 403 | 扫码会话与当前登录用户不一致 | 绑定模式只有发起的账号能轮询 |
| MIHOYO_UNBIND_FORBIDDEN | 403 | 当前账号只能通过米游社扫码登录，请先绑定邮箱 | 没有邮箱的账号解绑会把自己锁在外面（MHY-014） |
| MIHOYO_NOT_BOUND | 404 | 当前账号未绑定米游社 | |
| IDENTITY_ALREADY_BOUND | 409 | 该米游社账号已绑定其他绳网账号，请先在原账号解绑或注销 | 不接管任何账号（MHY-013） |
| USER_ALREADY_BOUND | 409 | 当前账号已绑定其他米游社账号，请先解绑 | |
| IDENTITY_CONFLICT | 409 | 这个米游社账号关联了多个绳网账号，请联系管理员 | aid 与 mid 指向不同账号（历史数据），运维处理 |
| QR_SESSION_CONSUMED | 409 | 这次扫码登录已经失效，请重新扫码 | 同一个二维码登录的会话已登出或过期，不会另起新会话 |
| NOT_FOUND | 404 | 内容不存在或不可见 | 存在性探测统一 404；路径里的 id 格式不对也是 404 |
| ALREADY_EXISTS | 409 | 已存在相同记录 | 通用唯一冲突 |
| USERNAME_TAKEN | 409 | 该用户名已被占用 | 改名时；区分大小写（D-018），冲突时不扣费 |
| EMAIL_TAKEN | 409 | 该邮箱已注册 | 仅注册用途；登录/重置用途防枚举，不暴露 |
| IDEMPOTENCY_IN_PROGRESS | 409 | 相同的请求正在处理，请稍后再试 | 带 `Retry-After: 1` |
| NOT_PUBLISHED | 409 | 这篇帖子还没有发布 | 对草稿执行下线 / 丢弃修改 |
| DM_PIN_LIMIT | 409 | 最多置顶 50 个会话 | CONV-023 |
| MESSAGE_TOO_OLD | 409 | 消息发出超过 5 分钟，不能再修改或撤回 | MSG-015、MSG-016 |
| MESSAGE_WITHDRAWN | 409 | 消息已撤回 | 编辑已撤回的消息 |
| CHECK_IN_ALREADY_TODAY | 409 | 今天已经签到过了，明天 4 点后再来吧 | 签到日按北京时间 04:00 切日 |
| DENNY_ALREADY_GIVEN | 409 | 你已经给这篇委托投过丁尼了 | 每人每帖终身一次 |
| EXAM_ALREADY_PASSED | 409 | 你已通过入站考试 | 开考时 |
| EXAM_ATTEMPT_ALREADY_SUBMITTED | 409 | 本场考试已提交 | |
| EXAM_ATTEMPT_EXPIRED | 409 | 考试已超时，请重新开始 | 超时交卷照样记一次失败 |
| REVISION_CONFLICT | 412 | 内容已在别处被修改，请刷新后重试 | If-Match 失败或格式不对 |
| PAYLOAD_TOO_LARGE | 413 | 请求体过大 | |
| UNSUPPORTED_MEDIA_TYPE | 415 | 请求体必须是 application/json | |
| IDEMPOTENCY_KEY_REUSED | 422 | 这个幂等键已经用在了另一个请求上 | 方法、路径、查询或请求体与第一次不同 |
| WEBSOCKET_REQUIRED | 426 | 请使用 WebSocket 连接 | `/v1/realtime` 收到普通 HTTP 请求 |
| RATE_LIMITED | 429 | 操作过于频繁，请稍后再试 | 带 `Retry-After`（秒） |
| UPLOAD_VERIFICATION_BUSY | 429 | 上传校验繁忙，请稍后重试 | 排队已满；带 `Retry-After` |
| UPLOAD_VERIFICATION_TIMEOUT | 429 | 上传校验超时，请稍后重试 | 排队或校验超时；带 `Retry-After` |
| CODE_COOLDOWN | 429 | 验证码发送过于频繁，请 N 秒后再试 | 仅注册用途；带 `Retry-After` |
| CODE_TOO_MANY_ATTEMPTS | 429 | 验证码错误次数过多，请重新获取 | 同一验证码错 5 次作废 |
| EXAM_COOLDOWN | 429 | 考试失败次数过多，请 N 分钟后再试 | 最近 3 次都未通过；带 `Retry-After` |
| INTERNAL | 500 | 服务器开小差了，请稍后再试 | 仅 requestId |
| SERVICE_UNAVAILABLE | 503 | 服务暂时不可用，请稍后再试 | 外部依赖超时 |
| MAIL_SEND_FAILED | 503 | 验证码邮件发送失败，请稍后再试 | 5xx 统一文案，客户端按 code 区分 |
| EXAM_NOT_AVAILABLE | 503 | 题库暂未配置，请稍后再试 | 5xx 统一文案，客户端按 code 区分 |
| MIHOYO_UPSTREAM_ERROR | 502 | 米游社服务暂时不可用，请稍后再试 | 5xx 统一文案，客户端按 code 区分 |
| MIHOYO_DISABLED | 503 | 米游社登录暂不可用 | `IK_MIHOYO__ENABLED=false`；5xx 统一文案 |

## 限流

- 被限流的 429 `RATE_LIMITED` 响应带 `Retry-After: <秒>`。
- 全站默认限流（`/v1/` 下所有请求，`crates/server/src/rate_limit.rs`）：游客按 IP（XFF 从右往左数
  `IK_HTTP__TRUSTED_PROXY_HOPS` 跳）、登录用户按用户计，读（GET/HEAD）写分开；默认匿名读 300/min、匿名写 60/min、
  登录读 600/min、登录写 120/min（`IK_RATE_LIMIT__*` 可调，`IK_RATE_LIMIT__ENABLED=false` 关闭）。
  令牌无效、账号封禁按游客处理。令牌桶（GCRA），没有固定窗口边界的突发。
- `/v1/` 下的每个响应（包括 429）带 `RateLimit-Limit`（每分钟次数）、`RateLimit-Remaining`、
  `RateLimit-Reset`（全部额度恢复还要几秒）。这几个头只描述全站默认限流；具体接口的限流只在超限时用 429 体现。
- 具体接口另有更严的限流（两者都生效）：登录/验证码类按 IP 每路由 20/min，评论、私信、拉黑、上传签名等见各域代码。
- 计数在进程内（不使用 Redis），多实例时每个实例各自计。
- 搜索（`/v1/search/*`）：登录用户每秒 10 次，游客每 IP 每秒 5 次。

## 实时（WebSocket `/v1/realtime`）

握手：网页同站 Cookie；App 用 `Authorization` 头（WS 握手允许带 Authorization）。
不带凭据时以**游客**连接，只收广播与公开频道（全站游客连接上限 5000，满了返回 503 + `Retry-After: 30`）。

- 带了凭据但已失效：401 `SESSION_EXPIRED`，不会静默降级成游客；账号封禁：403 `ACCOUNT_BLOCKED`。
- 非 Bearer 的连接（Cookie 或游客）带了 `Origin` 时必须在白名单里，否则 403 `CSRF_REJECTED`
  （握手是 GET，CSRF 中间件不拦，这里单独防跨站 WebSocket 劫持）。
- 不是 WebSocket 握手：426 `WEBSOCKET_REQUIRED`。

```
客户端 → { "op": "subscribe",   "seq": 1, "t": "post", "d": { "postId": "..." } }
客户端 → { "op": "unsubscribe", "seq": 2, "t": "post", "d": { "postId": "..." } }
客户端 → { "op": "ping",        "seq": 3 }
客户端 → { "op": "typing",      "seq": 4, "t": "dm", "d": { "conversationId": "..." } }

服务端 → { "op": "hello", "d": { "heartbeatSec": 30, "userId": "...", "serverTime": "..." } }
服务端 → { "op": "ack",   "seq": 1 }
服务端 → { "op": "event", "t": "notification.created", "d": { ... } }
服务端 → { "op": "error", "seq": 1, "d": { "code": "...", "detail": "..." } }
```

- 带 `seq` 的 `ping` / `subscribe` / `unsubscribe` 成功时回 `ack`，不带 `seq` 时成功不回帧。
- 频道：目前只有公开帖子 `t = "post"`。私信事件按用户投递给会话成员，不需要订阅。
- `typing`：只有登录连接能发（游客回 `UNSUPPORTED`）；每条连接每秒最多处理 1 帧，多出来的直接 `ack` 不处理；
  不是会话成员或与对方有拉黑关系时回 `NOT_ALLOWED`。服务端只推给会话里在场的对方。
- 帧格式与事件信封的类型登记在 OpenAPI components（`ClientFrame`、`ServerFrame`）。
- 已定义的事件：`post.published`、`post.removed`、`post.bumped`（广播）；
  `notification.created { id?, bucket, conversationId? }`（与发送者有在场私聊时带会话 id，收件箱据此把它并进会话）、
  `notification.removed { bucket }`、`notification.read { bucket?, count }`（只发给收件人本人）。
- 私信事件（发给会话里在场的成员，含发送者本人的其他端）：`dm.message.created { conversationId, message }`、
  `dm.message.edited { conversationId, message }`、`dm.message.withdrawn { conversationId, message }`（`message` 不含正文）；
  `dm.typing { conversationId, userId }`（只发给对方）；`dm.conversation.updated { conversationId, muted, pinned }`、
  `dm.conversation.left { conversationId }`（只发给本人，多端同步）。
- 收件箱：`inbox.read { key, lastReadAt?, notificationsUpdated }`、`inbox.read.all { at }`（只发给本人）。
- 内部事件（`Audience::Internal`，网关不转发，只给进程内订阅者，例如搜索索引同步）：
  `comment.removed { commentId, postId }`、`user.renamed { userId }`（载荷里是数字 id）。
  帖子的 `post.published` / `post.removed` / `post.bumped` 同时被搜索索引同步消费；
  以后的隐藏 / 恢复帖子（moderation）必须发 `post.removed` / `post.published`，隐藏评论必须发 `comment.removed`。
- 限制：每用户最多 5 连接（挤掉最旧）；客户端单帧 ≤ 64KB；订阅数上限 100；待发队列（256 帧）写满即断开。
- 心跳：连接 30s 没有收到客户端任何帧时服务端发 ping 帧，60s 没有任何帧则断开（D-008：60s 容忍度与旧系统一致）。
- 吊销：登出、踢设备、重置密码后立即断开对应连接；另外已登录连接每 30s 回库复查一次，复查失败按失效处理。
- 断线补齐：`GET /v1/...?since=<cursor>`，WS 不保证不丢事件，REST 为准。

| WS 错误码（`error` 帧） | 含义 |
|---|---|
| INVALID_FRAME | 不是合法 JSON，或超过 64KB |
| UNKNOWN_OP | 未知的 `op` |
| CHANNEL_INVALID | 频道不存在或无权订阅 |
| TOO_MANY_SUBSCRIPTIONS | 订阅超过 100 个频道 |
| UNSUPPORTED | 协议里有、但当前连接不能用的操作（如游客发 `typing`） |
| NOT_ALLOWED | 会话不存在或无权操作（`typing` 的目标会话不是自己的，或与对方有拉黑关系） |

| 关闭码 | 原因 | 客户端应当 |
|---|---|---|
| 1001 | 服务端停机 | 退避后重连 |
| 4000 | 同一用户连接超过 5 条，被挤掉 | 不要自动重连 |
| 4001 | 会话已吊销、过期或账号被封禁 | 回到登录，不要自动重连 |
| 4002 | 空闲超时 | 重连 |
| 4003 | 读得太慢，待发队列满 | 重连后用 REST 补齐 |

## 上传

两步直传（S3 预签名）：

1. `POST /v1/uploads/sign`（需要通过考试）→ 自己上传过相同内容（声明了 `contentHash`）时返回
   `{ existing: true, media }`；否则返回 `{ existing: false, uploadId, uploadUrl, method: "PUT", headers, expiresAt }`。
2. 客户端按 `method`、`headers` 把原文件 PUT 到 `uploadUrl`（`Content-Length` 已签入，必须与声明的 `size` 一致）。
3. `POST /v1/uploads/{uploadId}/complete`（可带 `{ width, height }`）→ 服务端 HEAD 比对大小 → 带 If-Match(ETag) 读回 →
   SHA-256（与声明的一致）+ 文件头嗅探 → 把校验过的字节发布到服务端生成的新 key → 返回媒体对象（201）。
   同一 `uploadId` 重试返回同一个文件（200）；内容与已有文件相同时复用已有文件并加入自己的图库（200）。

- 限制：≤ 30 MiB；类型只认 image/jpeg|png|gif|webp|avif（以嗅探为准，不信任客户端声明）；签名每人每分钟 60 次；
  同时校验 2 个（可配置），排队最多 64 个，满了或超时返回 429。
- 媒体对象统一含 `{ id, url, thumbUrl, mime, size, width?, height?, name, thumbhash?, createdAt }`；`thumbUrl` 为 360 宽 WebP
  （配置了 CDN 时按大小选阿里云 ESA 或 R2 图片变换，没有 CDN 时与原图相同）；`thumbhash` 是 base64 的 ThumbHash 模糊占位
  （JPEG/PNG/GIF/WebP 在完成上传时计算，AVIF 与解码失败的图片没有）。
- 我的图库：`GET /v1/me/uploads`、`DELETE /v1/me/uploads/{id}`（只从图库移除，文件不删）。
- 引用媒体只能用自己图库里的文件（D-012 方案 A）：头像 `PUT /v1/me/avatar { mediaId }`、帖子封面 `PostInput.coverIds`
  （卡片 `cover` 只带第一张，详情 `covers` 全部）、评论图片 `CreateCommentRequest.imageIds`（`CommentView.images`）。
  编辑帖子时，原本就在帖子里的封面不再校验归属（作者之后从图库移除也不影响）。
- 作者展示（`AuthorView`、通知发送者、私信对方、用户搜索、@ 候选）带 `avatarUrl`（360 宽缩略图）；匿名、已注销、
  没有设置头像时省略。
- 没有完成的上传留在暂存前缀 `[root/]staging/` 下，后台每小时删除超过 `IK_STORAGE__STAGING_TTL_HOURS`（默认 24）的对象。

## 缓存

- 公开 GET：`Cache-Control: public, max-age=<n>`（可交给 CDN）；带凭据的请求一律
  `Cache-Control: private, no-store`。
- 列表/详情支持 `ETag` + `If-None-Match`（304）。

## 管理后台

- 全部在 `/v1/admin/*` 下，要求 moderator/admin 角色。
- 写操作一律写审计日志（操作者、动作、目标、参数、IP、UA）。
- 列表接口允许 `page` 分页与排序参数（管理场景可用性优先）。
