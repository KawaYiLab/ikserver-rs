# 业务规则：媒体上传与运维横切面（media-ops）

## 范围

旧后端（Strapi 5 / TypeScript）中以下行为的精确规格，用于 Rust 重写与新管理后台：

- 直传上传（签名 → 客户端 PUT → 完成校验/发布）、内容去重、收据（receipt）、并发与超时。
- 文件表（Strapi `files`）字段、provider delete 补丁、旧域名迁移、CDN 缩略图 URL 规则、各实体对文件的引用与数量上限。
- 用户图库 `me-uploads`、自定义头像 `me-avatar upload-custom`、`upload-validate` 中间件。
- 运维横切：全局限流、客户端 IP、错误格式化、CORS/CSP/body 限制、strapi-cache 与 query 归一化、Redis 工具。
- 定时任务（cron）与 bootstrap 任务。
- 邮件双通道调度。
- 运营在 Strapi Admin 中手工维护的内容类型（新管理后台 CRUD 需求）。
- `server/docs/*.md` 中的运维规则与教训。

路径约定：`server/` 指 `C:\Users\Administrator\Documents\GitHub\InterKnot-web\server`，`app/` 指 `C:\Users\Administrator\Documents\GitHub\InterKnot-web\app`。

## 来源文件

- `server/src/api/direct-upload/{routes,controllers,services}/direct-upload.ts`
- `server/src/utils/direct-upload.ts`、`server/src/utils/upload-content.ts`、`server/src/utils/upload-constraints.ts`
- `server/src/extensions/upload/strapi-server.ts`
- `server/database/migrations/2026.09.23T00.00.00.add-verified-upload-fields.js`
- `server/src/api/me-upload/{routes,controllers}/me-upload.ts`
- `server/src/api/me-avatar/{routes,controllers}/me-avatar.ts`
- `server/src/middlewares/{upload-validate,rate-limit,error-format,sanitize-cache-query,require-exam-passed}.ts`
- `server/src/utils/common.ts`、`server/src/utils/cache-query.ts`、`server/src/utils/redis.ts`、`server/src/utils/redis-config.ts`、`server/src/utils/benefits.ts`
- `server/src/api/article/controllers/article.ts`、`server/src/api/comment/controllers/comment.ts`（媒体引用解析部分）
- `server/config/{middlewares,plugins,server,cron-tasks}.ts`、`server/.env.example`
- `server/src/index.ts`、`server/src/api/tag/services/{indexes,cache}.ts`、`server/src/utils/account-privacy.ts`
- `server/src/utils/agent/{queue,quota-receipt}.ts`、`server/src/api/report/utils/report-actions.ts`、`server/src/api/notification/utils/bucket.ts`、`server/src/api/auth-session/services/auth-session.ts`、`server/src/api/article/utils/follow-notification.ts`、`server/src/api/article/services/hot.ts`
- `server/src/utils/email/{index,domains,config,types}.ts`、`server/src/utils/email/senders/{ali-smtp,brevo-api}.ts`
- `server/src/api/*/content-types/*/schema.json`、`server/src/extensions/users-permissions/content-types/user/schema.json`、`server/src/plugins/{report-panel,exam-question-panel}/strapi-server.js`
- `server/docs/{backend-remediation,admin-virtual-author,search-sections-deployment,hot-ranking-rollout,mcp-forum-tools-plan,exam-question-management-plan}.md`
- `app/utils/upload.ts`、`app/utils/image.ts`、`app/composables/useApi.ts:2026-2252`
- 测试：`server/tests/unit/{direct-upload-security,upload-content,upload-receipt-database,upload-replacement,me-upload,error-format,cache-key,incr-with-ttl,redis-config,email-dispatcher,email-domains,migration-read-only}.test.ts`

---

## 1. 直传上传（UPL）

### 1.1 数据与约束

- `UPL-001` 允许的 MIME 固定为 `image/jpeg`、`image/png`、`image/gif`、`image/webp`、`image/avif`；扩展名映射 `jpeg→[jpg,jpeg]`、`png→[png]`、`gif→[gif]`、`webp→[webp]`、`avif→[avif]`，规范扩展名取列表第一个（`.jpg`）。来源 `server/src/utils/upload-constraints.ts:1-44`。
- `UPL-002` 请求 MIME 先 `trim().toLowerCase()`，再把非标准 `image/jpg` 归一为 `image/jpeg`。来源 `server/src/utils/upload-constraints.ts:29-34`、`server/src/api/direct-upload/services/direct-upload.ts:201-203`。
- `UPL-003` 单文件上限 `MAX_VERIFIED_UPLOAD_BYTES = 30 * 1024 * 1024`（30 MiB）；另受 `plugin::upload.sizeLimit`（env `UPLOAD_SIZE_LIMIT`，默认 30 MiB）约束，超出报 `File exceeds upload size limit`。来源 `server/src/utils/upload-content.ts:5`、`server/src/api/direct-upload/services/direct-upload.ts:236-239`、`server/config/plugins.ts:192`。
- `UPL-004` 签名请求体字段：`filename`（必填，取 `path.basename(trim)`）、`mimeType`（必填）、`size`（number 或可 `parseInt` 的字符串，必须为 `1..30MiB` 的安全整数）、`contentHash`（可选，小写后必须匹配 `^[a-f0-9]{64}$`，否则视为未提供）、`path`（可选）、`fileInfo.{name,alternativeText,caption}`（可选，trim 后为空则丢弃）。来源 `server/src/api/direct-upload/services/direct-upload.ts:35-45,74-82,199-246`、`server/src/utils/direct-upload.ts:169-175`。
- `UPL-005` 文件名扩展名只需属于允许扩展名全集（不要求与 MIME 一一对应），真实类型由完成阶段魔数嗅探把关；无扩展名或以 `.` 结尾即拒绝 `filename extension is not an allowed image type`。来源 `server/src/api/direct-upload/services/direct-upload.ts:228-234`、`server/src/utils/upload-constraints.ts:46-52`。
- `UPL-006` `path` 归一化：trim、`\`→`/`、去首尾 `/`；包含 `..` 则拒绝 `Upload path cannot contain ".."`；为空时使用服务器本地时间的 `YYYY/MM` 作为路径。来源 `server/src/utils/direct-upload.ts:148-167`、`server/src/api/direct-upload/services/direct-upload.ts:330-357`。
- `UPL-007` 对象 key 格式：`{rootPath去尾斜杠}/` + `{path去首尾斜杠}/` + `{hash}{ext}`（各段为空则省略）。签名阶段写入的 staging key 为 `getObjectKey({...file, path: "staging/" + (file.path||"")})`，即 `[rootPath/]staging/YYYY/MM/{hash}{ext}`。来源 `server/src/utils/direct-upload.ts:119-129`、`server/src/api/direct-upload/services/direct-upload.ts:434-435`。
- `UPL-008` 最终（发布）key 由服务端生成：`hash = randomBytes(16).hex`（32 位十六进制），`ext = 嗅探 MIME 的规范扩展名`，路径同 token 中的 `file.path`（不含 `staging/`）。来源 `server/src/api/direct-upload/services/direct-upload.ts:544-545`。
- `UPL-009` 公开 URL：若配置 `baseUrl`（env `CDN_URL`，默认 `https://im.tiwat.cn`）则为 `baseUrl/objectKey`；否则为 `{endpoint}/{bucket}/{objectKey}`，两者都缺报 `Unable to determine public object URL`。来源 `server/src/utils/direct-upload.ts:131-146`、`server/config/plugins.ts:207`。
- `UPL-010` 仅支持 provider `@strapi/provider-upload-aws-s3`（S3 兼容，env `QINIU_S3_ENDPOINT/REGION/BUCKET`、`QINIU_ACCESS_KEY/SECRET_KEY`，`forcePathStyle: true`），ACL 默认 `public-read`；缺 bucket 报 `Missing object storage bucket configuration`。来源 `server/src/utils/direct-upload.ts:92-117`、`server/config/plugins.ts:190-210`。
- `UPL-011` presign 有效期 `DIRECT_UPLOAD_SIGN_EXPIRES` 秒（正整数），默认 600 秒；完成 token 的 `exp` 与 presign 同为 `now + expiresIn`。来源 `server/src/utils/direct-upload.ts:38-44`、`server/src/api/direct-upload/services/direct-upload.ts:436,450-453`。
- `UPL-012` 完成 token 格式：`base64url(JSON payload) + "." + base64url(HMAC-SHA256(secret, body))`。payload：`version: 2`、`exp`（秒）、`userId`、`objectKey`（staging key）、`contentHash`（可选）、`file:{name,alternativeText,caption,folder,folderPath,hash,ext,mime,size(KB),sizeInBytes,path}`。来源 `server/src/utils/direct-upload.ts:46-54`、`server/src/api/direct-upload/services/direct-upload.ts:451-470`。
- `UPL-013` token 密钥优先级：`DIRECT_UPLOAD_TOKEN_SECRET` → `JWT_SECRET` → `APP_KEYS` 第一个；全部缺失时生产环境（`NODE_ENV=production`）抛错拒绝签发，非生产回退常量 `direct-upload-secret`。来源 `server/src/utils/direct-upload.ts:17-36`。
- `UPL-014` 收据（receipt）存于 `files` 表：`direct_upload_id = sha256_hex(完整 token 字符串)`，唯一约束 `files_direct_upload_id_uq`；`verified_content_hash`（varchar 255，普通索引 `idx_files_verified_content_hash`）只由服务端写入。两列均 private、不可写、不可见。来源 `server/src/api/direct-upload/services/direct-upload.ts:502`、`server/database/migrations/2026.09.23T00.00.00.add-verified-upload-fields.js:8-17`、`server/src/extensions/upload/strapi-server.ts:64-76`。
- `UPL-015` 持久化的文件行字段：`name/alternativeText/caption/folder/folderPath`（来自 token）、`hash`（最终随机 hash）、`ext`/`mime`（来自嗅探，不信任客户端）、`size`（KB，来自 token）、`url`（最终 key 的公开 URL）、`provider`、`provider_metadata = {objectKey: 最终key, directUpload: true, contentHash?}`、`verifiedContentHash`（服务端计算的 SHA-256）、`directUploadId`、`uploaders=[userId]`、可选 `width/height`。来源 `server/src/api/direct-upload/services/direct-upload.ts:359-415,555`。
- `UPL-016` `width/height` 来自完成请求体（客户端在浏览器测得），正数 number 四舍五入、或可 `parseInt` 的正数字符串；服务端不校验真实尺寸。来源 `server/src/api/direct-upload/services/direct-upload.ts:137-150,367-368`、`app/composables/useApi.ts:2156-2161`。
- `UPL-017` 文件夹：媒体库根目录名固定 `API Uploads`，其下按 `YYYY`/`MM` 子目录归档；根目录查找顺序为 store 键 `plugin::upload/api-folder` 指向且名称匹配 → 按名称+`parent=null` 最小 id → 新建并回写 store。「Strapi 特有，重写时不需要」（但「按年月归档」可保留为对象 key 规则）。来源 `server/src/api/direct-upload/services/direct-upload.ts:248-338`。

### 1.2 权限

- `UPL-020` `POST /direct-upload/sign` 与 `POST /direct-upload/complete` 都挂 `global::require-exam-passed`：未登录 401（`authentication required`）；`examPassed`/`isAdmin`/`isAiAgent` 皆不为真时 403 `{error:{code:'EXAM_REQUIRED',message:'需要先通过入站考试才能进行此操作'}}`；每次请求按 user id 重查库。来源 `server/src/api/direct-upload/routes/direct-upload.ts:1-22`、`server/src/middlewares/require-exam-passed.ts:16-39`。
- `UPL-021` 完成时要求当前用户 id 与 token 中 `userId` 严格相等，否则 `Direct upload token does not belong to the current user`。来源 `server/src/api/direct-upload/services/direct-upload.ts:497-500`。
- `UPL-022` 去重可跨用户：任何用户提交的 `contentHash` 命中任意文件的 `verifiedContentHash`，都会直接复用该文件并把自己加入其 `uploaders`（M:N）。来源 `server/src/api/direct-upload/services/direct-upload.ts:423-430,511-516`。

### 1.3 流程

- `UPL-030` 签名：校验输入 → 若有合法 `contentHash` 且命中 `verifiedContentHash`，返回 `{existing: 文件}`（不签发 PUT）→ 否则计算文件信息、生成 staging key、预签名 `PutObject(Bucket, Key, ContentType=file.mime, ACL)`，返回 `{uploadUrl, uploadToken, method:'PUT', objectKey, publicUrl, headers:{'Content-Type': mime}, expiresAt(ISO)}`，HTTP 200 包 `{data}`。来源 `server/src/api/direct-upload/services/direct-upload.ts:418-483`、`server/src/api/direct-upload/controllers/direct-upload.ts:3-9`。
- `UPL-031` 客户端流程：先在浏览器算 SHA-256 → sign → 若 `existing` 直接使用 → 否则按返回的 method/headers PUT 原文件 → 读取宽高 → complete。客户端 MIME 缺省填 `image/jpeg`。来源 `app/composables/useApi.ts:2123-2165`。
- `UPL-032` 完成步骤顺序：(1) `uploadToken` 必填并 trim；(2) 验签；(3) `now >= exp` 拒绝；(4) 校验归属；(5) 按 `directUploadId` 查已有收据，命中则幂等返回（并补 uploader）；(6) token 带 `contentHash` 时按 `verifiedContentHash` 查同内容文件，命中则复用；(7) 进入校验并发槽位执行后续 S3 操作。来源 `server/src/api/direct-upload/services/direct-upload.ts:485-520`。
- `UPL-033` 槽位内：`HeadObject(staging key)`，`ContentLength` 必须等于 `token.file.sizeInBytes`；`GetObject` 带 `IfMatch: HEAD 的 ETag`，流式读入预分配 `sizeInBytes` 的缓冲，超出即中止；读完长度不等即拒绝；计算 SHA-256，若 token 带 hash 则必须一致；`file-type` 魔数嗅探 MIME 必须在允许集合内。来源 `server/src/api/direct-upload/services/direct-upload.ts:521-539`、`server/src/utils/upload-content.ts:8-38`。
- `UPL-034` 发布：把**已校验的原始字节**以 `PutObject(Key=最终key, Body, ContentLength, ContentType=嗅探MIME, ACL)` 写到新 key（不复制 staging 对象），再在事务中 `files.create`（文件行 + folder + uploader 关系同一事务）。来源 `server/src/api/direct-upload/services/direct-upload.ts:541-563`。
- `UPL-035` 收据唯一冲突：仅当确认为唯一约束冲突且能查到赢家时，删除自己刚发布的最终对象并返回赢家（补 uploader）；其它错误原样抛出且不删除对象。来源 `server/src/api/direct-upload/services/direct-upload.ts:564-572`。
- `UPL-036` 成功后仅当 `token.version === 2` 才删除 staging 对象（删除失败只 `console.warn`）；旧版 token 的 objectKey 可能指向已公开的历史文件，绝不删除。来源 `server/src/api/direct-upload/services/direct-upload.ts:117-126,573-574`。
- `UPL-037` 完成返回 HTTP 201 `{data: 文件}`，文件经 Content API 输出净化（去除 private 字段），`url` 被替换为 360 宽缩略图 URL（按 `size` 选择 ESA/R2，见 CDN 节）。签名阶段的 `existing` 同样处理。来源 `server/src/api/direct-upload/controllers/direct-upload.ts:11-18`、`server/src/api/direct-upload/services/direct-upload.ts:168-182`。

### 1.4 并发、队列与超时

- `UPL-040` 进程内校验槽位数 `DIRECT_UPLOAD_MAX_CONCURRENT_VERIFICATIONS`，默认 6、上限 32；排队上限固定 64，满则立即抛 busy。来源 `server/src/utils/upload-content.ts:50-51,84`。
- `UPL-041` 排队等待超时 `DIRECT_UPLOAD_QUEUE_TIMEOUT_MS`，默认 60000；单任务总超时 `DIRECT_UPLOAD_TASK_TIMEOUT_MS`，默认 90000；两者上限 10 分钟（`setTimeout` 超过 2^31-1 会变 1ms）。来源 `server/src/utils/upload-content.ts:40-57`。
- `UPL-042` S3 单请求超时 `DIRECT_UPLOAD_S3_REQUEST_TIMEOUT_MS` 默认 15000（Head/Put/Delete），GetObject 读取超时 `DIRECT_UPLOAD_VERIFY_TIMEOUT_MS` 默认 30000；任务超时会 abort 所有子请求。来源 `server/src/api/direct-upload/services/direct-upload.ts:92-115`。
- `UPL-043` 任务超时时调用方立即被拒绝，但槽位要等任务真正结束才释放（任务可能仍持有缓冲）；任务失败也会释放槽位。来源 `server/src/utils/upload-content.ts:97-122`。
- `UPL-044` busy/超时错误是 `RateLimitError`（HTTP 429），`details.code` 分别为 `UPLOAD_VERIFICATION_BUSY`（`Upload verification is busy; retry shortly`）与 `UPLOAD_VERIFICATION_TIMEOUT`（`Upload verification timed out; retry shortly`）。来源 `server/src/utils/upload-content.ts:62-69`。
- `UPL-045` S3/网络层的 `TimeoutError`、`AbortError`、`SniffTimeoutError` 由错误中间件映射为 503 `SERVICE_UNAVAILABLE`。来源 `server/src/middlewares/error-format.ts:22-25`。

### 1.5 错误信息（全部为英文 `ApplicationError`，HTTP 400）

| 场景 | message |
|---|---|
| 缺文件名 | `filename is required` |
| 缺 MIME | `mimeType is required` |
| size 非法/超 30MiB | `size must be a positive integer` |
| MIME 不允许 | `mimeType is not allowed` |
| 扩展名不允许 | `filename extension is not an allowed image type` |
| 超 sizeLimit | `File exceeds upload size limit` |
| path 含 `..` | `Upload path cannot contain ".."` |
| 缺 token | `uploadToken is required` |
| token 格式错 | `Invalid direct upload token` |
| 签名不符 | `Direct upload token signature mismatch` |
| 过期 | `Direct upload token has expired` |
| 非本人 | `Direct upload token does not belong to the current user` |
| 大小不符 | `Uploaded object size does not match the signed request` / `Uploaded object exceeds the signed size` |
| 签名 size 非法 | `Invalid uploaded image size` |
| 哈希不符 | `Uploaded object checksum does not match the signed request` |
| 非图片 | `Uploaded object is not a supported image` |
| 无可读内容 | `Uploaded object has no readable content` |

来源 `server/src/api/direct-upload/services/direct-upload.ts:211-238,486-500,526-536`、`server/src/utils/direct-upload.ts:56-77,162-164`、`server/src/utils/upload-content.ts:13-36`。

### 1.6 安全校验

- `UPL-050` token 验证必须：恰好 2 段、两段都只含 `[A-Za-z0-9_-]`、签名 base64url 重新编码后与原文逐字相同（拒绝 padding/别名编码）、长度相等后 `timingSafeEqual`；原因是收据以 token 字符串为键，别名编码会产生新收据。来源 `server/src/utils/direct-upload.ts:56-77`。
- `UPL-051` 去重只信任服务端写入的 `verifiedContentHash`；历史 `provider_metadata.contentHash`（客户端上报）不参与复用、也不回填。来源 `server/src/api/direct-upload/services/direct-upload.ts:509-516`、`server/src/extensions/upload/strapi-server.ts:64-65`。
- `UPL-052` 发布到客户端不可写的新 key：presigned PUT 在过期前仍有效，若直接发布 staging key，校验后仍可被覆盖。来源 `server/src/api/direct-upload/services/direct-upload.ts:541-545`。
- `UPL-053` GetObject 使用 `IfMatch: ETag` 防止 HEAD 与读取之间对象被替换。来源 `server/src/api/direct-upload/services/direct-upload.ts:532-534`。
- `UPL-054` 收据行在外层事务内创建，关系写入成功前对其它连接不可见，避免并发重试拿到半成品。来源 `server/src/api/direct-upload/services/direct-upload.ts:557-563`、`server/tests/unit/upload-receipt-database.test.ts:12`。
- `UPL-055` 已认证文件（`verifiedContentHash` 或 `directUploadId` 非空）禁止原生 `upload.replace` 原地替换，报 `Verified uploads cannot be replaced in place; upload a new file instead`；未认证历史文件仍可替换。来源 `server/src/extensions/upload/strapi-server.ts:21-43`。
- `UPL-056` `uploaders` 关系为 M:N、private、不可写、不可见，禁止通过 Content API populate 反查上传者（防匿名发帖溯源）。来源 `server/src/extensions/upload/strapi-server.ts:49-62`。

### 1.7 边界与已知坑

- `UPL-060` 签名阶段不做任何 S3 存在性检查；staging 对象若客户端从未 complete 会永久残留，文档建议对 staging 前缀配置存储生命周期。来源 `server/docs/backend-remediation.md`「部署与数据兼容」第 4 条。
- `UPL-061` `.env.example` 中的 `DIRECT_UPLOAD_SNIFF_TIMEOUT_MS` 在代码中无引用（实际读取的是 `DIRECT_UPLOAD_VERIFY_TIMEOUT_MS`）。来源 `server/.env.example:51`、`server/src/api/direct-upload/services/direct-upload.ts:97-100`。
- `UPL-062` 并发槽位与队列是进程内计数，多实例部署时总并发 = 实例数 × 槽位数。来源 `server/src/utils/upload-content.ts:59-60`。
- `UPL-063` Strapi `size` 字段单位为十进制 KB（`sizeInBytes` 才是字节），重写时建议统一存字节。「Strapi 特有」。来源 `server/src/utils/common.ts:19-38`。
- `UPL-064` 文件夹 `dateFolderCache` 进程内并发去重 + 唯一冲突回查。「Strapi 特有，重写时不需要」。来源 `server/src/api/direct-upload/services/direct-upload.ts:287-328`。

---

## 2. 文件与引用（FILE）

### 2.1 数据与约束

- `FILE-001` `files` 表重写需要的字段：`id`、`document_id`、`name`、`alternative_text`、`caption`、`hash`、`ext`、`mime`、`size`（KB）、`width`、`height`、`url`、`provider`、`provider_metadata`（JSON，含 `objectKey`、`directUpload`、可选 `contentHash`）、`folder`/`folder_path`、`verified_content_hash`、`direct_upload_id`（唯一）、`created_at`；以及 M:N `uploaders`（文件 ↔ 用户）。来源 `server/src/api/direct-upload/services/direct-upload.ts:384-404`、`server/src/api/me-upload/controllers/me-upload.ts:60-70`。
- `FILE-002` 引用文件的实体与数量上限：

| 实体.字段 | 多值 | 上限/规则 |
|---|---|---|
| `article.cover` | 是 | 等级权益 `articleMaxImages = [0,9,12,15,18,24,30,40]`（下标=等级，0=未通过考试；管理员/AI 取 7 级），超出 `当前等级图片最多只能上传 ${n} 张` |
| `comment.images` | 是 | `commentMaxImages = [0,3,6,9,9,12,12,18]`，在逐项查库之前按原始数组长度拦截，超出 `images cannot exceed ${n} items` |
| `author.avatar` | 否 | 自定义头像（见 AVA 节） |
| `avatar.image` / `business-card.image` / `emote.image`(必填) / `emote-group.icon` / `ai-role-card.avatar` / `conversation.avatar` | 否 | 仅后台维护或未开放写入 |
| `message.attachments` | 是 | schema 存在，但 API 未写入（见未决问题） |

来源 `server/src/utils/benefits.ts:8-34`、`server/src/api/article/controllers/article.ts:537-542,863`、`server/src/api/comment/controllers/comment.ts:501-531,596-604`、各 `schema.json`。

- `FILE-003` 媒体引用解析（文章/评论共用逻辑）：接受正整数 id、纯数字字符串 id、文件 `documentId` 字符串、`{id}` 或 `{documentId}` 对象；多值字段还接受 `{set:[...]}`/`{connect:[...]}`；`documentId` 查不到报 `${fieldPath} references upload file "${x}" but it does not exist`，其它格式报 `${fieldPath} has an unsupported media reference format`。来源 `server/src/api/comment/controllers/comment.ts:434-531`、`server/src/api/article/controllers/article.ts:709-769`。
- `FILE-004` 评论正文为空时必须至少带一张图片：`content or images is required`。来源 `server/src/api/comment/controllers/comment.ts:606-608`。

### 2.2 provider delete 补丁

- `FILE-010` bootstrap 时对 upload provider 的 `delete` 打补丁（幂等标记 `__directUploadDeletePatched`）：若 `provider_metadata.directUpload === true` 且有 `objectKey`，则去掉 `rootPath/` 前缀；当相对 key 以 `{hash}{ext}` 结尾时，把剩余前缀（去尾斜杠）写入 `file.path` 再交给原 delete；否则原样删除。用途是让直传文件（key 带年月/staging 前缀）能被 Strapi 媒体库删除到正确对象。「Strapi 特有」，重写时直接按 `provider_metadata.objectKey` 删除即可。来源 `server/src/index.ts:93-165`。

### 2.3 安全校验 / 边界

- `FILE-020` 文章与评论引用媒体时**不校验文件归属**（任意存在的文件 id/documentId 都可引用）。来源 `server/src/api/comment/controllers/comment.ts:456-499`。（是否在重写中收紧见未决问题。）
- `FILE-021` 正文 `text`/`editorState` 中内嵌的图片 URL 不形成媒体关系，因此「无关系引用」不能证明文件未被使用；物理删除只能由同时索引内嵌 URL 的独立 GC 流程完成（尚未实现）。来源 `server/src/api/me-upload/controllers/me-upload.ts:140-144`。

---

## 3. CDN URL 规则（CDN）

- `CDN-001` 当前图片主机 `IMAGE_HOST = https://im.tiwat.cn`；旧主机 `image.tiwat.cn`（含 `//image.tiwat.cn` 协议相对写法）重写为 `im.tiwat.cn`。来源 `server/src/utils/common.ts:7-8,70-86`、`app/utils/image.ts:1-2,50-66`。
- `CDN-002` 旧七牛缩略图后缀 `LEGACY_THUMB_SUFFIX = '-small.webp'`：仅当 `?` 之前的路径以该后缀结尾时剥离，保留 query。来源 `server/src/utils/common.ts:9,40-47`。
- `CDN-003` `blob:`/`data:` URL 原样返回；`//` 开头补 `https:`；非 `http(s)://` 的相对路径去前导 `/` 后拼到 `https://im.tiwat.cn/`。来源 `server/src/utils/common.ts:15-17,77-83`。
- `CDN-004` 缩略图服务选择：`size` 换算为字节（字符串按 KB×1000；整数 ≥1,000,000 视为已是字节，否则 ×1000），`> 10,000,000` 字节（`ESA_SIZE_THRESHOLD`）用 R2，否则（含未知 size）用阿里云 ESA。来源 `server/src/utils/common.ts:13,23-38,112-118`。
- `CDN-005` R2 变换：路径前插 `/cdn-cgi/image/{options}`，缩略图 options `width={w},format=webp,quality=80`，不缩放版 `format=webp,quality=80`。ESA 变换：追加 query `image_process=resize,w_{w}/format,webp/quality,q_80`，不缩放版 `image_process=format,webp/quality,q_80`（已有 `?` 则用 `&`）。来源 `server/src/utils/common.ts:88-97,120-142`。
- `CDN-006` 已是变换 URL 时保持同一服务只换宽度：R2 替换/前插 `width=`，ESA 先剥掉 `image_process=` 参数再重建。`toCanonicalUrl` 同时剥离 `/cdn-cgi/image/{opts}/` 前缀与 `image_process` 参数得到原图。来源 `server/src/utils/common.ts:144-243`。
- `CDN-007` 后端统一缩略图宽度 360（`withSmallWebp`、`withCanonicalUrl` 的 `thumbUrl`、直传返回、举报详情）；名片图用 `toNoResizeWebpUrl`（只转 WebP 不缩放）。前端卡片封面用 720（有 GPU）/480（无 GPU）。来源 `server/src/utils/common.ts:292-322`、`server/src/api/me-business-card/controllers/me-business-card.ts:30`、`server/src/api/author/controllers/author.ts:277,359`、`app/utils/image.ts:193-195`。
- `CDN-008` 前端对非本站主机（非 `im.tiwat.cn`/`image.tiwat.cn`，如 B 站封面）不加变换参数，后端无此判断。来源 `app/utils/image.ts:41-48,181-184`。
- `CDN-009` CSP 允许的图片/媒体源：`'self' data: blob: *.qiniucs.com im.tiwat.cn image.tiwat.cn`（`*.qiniucs.com` 为七牛默认域，兼容旧外链）。来源 `server/config/middlewares.ts:16-31`。

---

## 4. 用户图库 me-uploads（MEUP）

- `MEUP-001` `GET /me/uploads?page&pageSize`：需登录（否则 401 `You must be logged in.`）；`page` 默认 1，`pageSize` 默认 24、上限 100，非正数回退默认；条件 `uploaders 包含当前用户`，`createdAt` 倒序，offset 分页。来源 `server/src/api/me-upload/controllers/me-upload.ts:6-86`。
- `MEUP-002` 列表字段白名单 `id, documentId, name, url, mime, size, width, height, createdAt`，`url` 转 360 缩略图；返回 `meta.pagination{page,pageSize,total,pageCount=ceil(total/pageSize)}`；不暴露 uploaders。来源 `server/src/api/me-upload/controllers/me-upload.ts:55-85`。
- `MEUP-003` `DELETE /me/uploads/:documentId`：documentId trim 后为空 400 `documentId is required`；查询条件同时限定 `documentId` 与 `uploaders 包含当前用户`，查不到（包括非本人）统一 404 `File not found`。来源 `server/src/api/me-upload/controllers/me-upload.ts:88-108`。
- `MEUP-004` 删除语义：事务内（MySQL/PG 对文件行 `FOR UPDATE`，SQLite 不加锁）断开当前用户的 uploader 关系，**从不物理删除**文件或对象；返回 `{data:{deleted:false, inUse: 是否存在 related 媒体关系}}`；不依赖对象存储可用。来源 `server/src/api/me-upload/controllers/me-upload.ts:110-147`、`server/tests/unit/me-upload.test.ts:129-208`。
- `MEUP-005` 前端只展示 `documentId`、`url` 存在且 `mime` 以 `image/` 开头（或无 mime）的条目。来源 `app/composables/useApi.ts:2056-2058`。
- `MEUP-006` 路由无 `require-exam-passed`，鉴权依赖 users-permissions 角色权限。「Strapi 特有」，重写需显式要求登录。来源 `server/src/api/me-upload/routes/me-upload.ts`。

---

## 5. 自定义头像（AVA）

- `AVA-001` `PUT /me/avatars/upload-custom` 请求体 `{fileId}`（number 或可转数字），非正有限数 400 `fileId is required`；文件不存在 404 `Uploaded file not found.`；当前用户没有 author 400 `Author profile not found.`。来源 `server/src/api/me-avatar/controllers/me-avatar.ts:78-101`。
- `AVA-002` 在账号锁（`withActiveAccountLock`）内：扣 10 丁尼（类型 `spend_avatar`，备注 `设置自定义头像`）→ 余额不足返回 400 `丁尼余额不足`；成功则 `author.avatar = fileId` 并清空 `user.equippedAvatar`；返回 `{success:true, avatar:{url(360缩略图), width, height}}`。来源 `server/src/api/me-avatar/controllers/me-avatar.ts:103-117`。
- `AVA-003` 扣款与资料更新同属一个锁内操作，异常时返回 500 `设置头像失败，未扣除丁尼`（`ACCOUNT_BLOCKED` 错误原样抛出）。注意：5xx 消息会被错误中间件替换为 `Internal server error`，用户实际看不到该中文。来源 `server/src/api/me-avatar/controllers/me-avatar.ts:118-122`、`server/src/middlewares/error-format.ts:125-128`。
- `AVA-004` 前端流程：先走直传 `uploadImage`（进度占 80%），再以返回的 `id` 调用 upload-custom。来源 `app/composables/useApi.ts:2243-2252`。
- `AVA-005` 不校验 `fileId` 是否为当前用户上传（任意文件 id 均可设为头像），路由也未挂 `require-exam-passed`。来源 `server/src/api/me-avatar/controllers/me-avatar.ts:88-93`、`server/src/api/me-avatar/routes/me-avatar.ts`。
- `AVA-006` 同模块：`GET /me/avatars` 列出全部系统头像（按 `type asc, createdAt asc`）与当前 `equippedAvatarDocumentId`；`PUT /me/avatars/equip {documentId}` 装备（不存在 404 `Avatar not found.`），空值表示卸下；均在账号锁内写。来源 `server/src/api/me-avatar/controllers/me-avatar.ts:20-76`。

---

## 6. upload-validate 中间件（UPV）

- `UPV-001` 只作用于路径以 `/api/upload` 或 `/upload` 开头且方法为 POST/PUT 的请求（Strapi 原生 multipart 上传，包括后台媒体库）；递归收集 `ctx.request.files` 中带 `filepath`/`path` 的文件。来源 `server/src/middlewares/upload-validate.ts:53-70`。
- `UPV-002` 每个文件读取前 `maxBytesToCheck=4100` 字节做魔数嗅探：缺临时路径 400 `UPLOAD_FILEPATH_MISSING`；无法识别 400 `UPLOAD_FILETYPE_UNKNOWN`；不在允许 MIME 400 `UPLOAD_MIME_NOT_ALLOWED`（details 含 `filename`、`detectedMime`、`allowedMimeTypes`）。来源 `server/src/middlewares/upload-validate.ts:72-102`、`server/config/middlewares.ts:87-99`。
- `UPV-003` 以嗅探结果为准改写 `mimetype`，并把文件名扩展名改成规范扩展名。来源 `server/src/middlewares/upload-validate.ts:104-120`。
- `UPV-004` 该中间件服务于 Strapi 原生上传接口。「Strapi 特有」；重写时若新后台支持直接上传，需复用同一套嗅探规则。

---

## 7. 限流与客户端 IP（RL）

### 7.1 全局限流

- `RL-001` Redis 固定窗口；键 `rl:{r|w}:{u:<userId> | ip:<clientIp>}`；`incrWithTtl(key, windowSeconds)` 原子自增并仅在无 TTL 时设 TTL。来源 `server/src/middlewares/rate-limit.ts:109-124`、`server/src/utils/redis.ts:29-76`。
- `RL-002` 生产阈值（每 60 秒）：匿名读 300、匿名写 60、登录读 600、登录写 120；代码默认值为 60/20/120/40。读 = `GET/HEAD/OPTIONS`，其它为写。来源 `server/config/middlewares.ts:61-70`、`server/src/middlewares/rate-limit.ts:30-36,109`。
- `RL-003` 只限 `/api/` 开头的路径；`/api/agent/*` 跳过（`/api/agent/characters*` 除外，仍受限），因为 agent worker 内部接口另有鉴权且高频。来源 `server/src/middlewares/rate-limit.ts:95-107`。
- `RL-004` 身份推导在路由鉴权之前进行：优先 `ctx.state.user.id`；否则解析 `Authorization: Bearer <token>`，`jwtManagement=refresh` 模式用纯内存 `validateAccessToken` 且要求 `payload.type==='access'`，否则用 jwt 验签取 `payload.id`；任何失败按匿名处理，不查库。来源 `server/src/middlewares/rate-limit.ts:51-90`。
- `RL-005` 响应头 `X-RateLimit-Limit`、`X-RateLimit-Remaining = max(0, limit - current)`；超限 429 `{error:{code:'TOO_MANY_REQUESTS',message:'Rate limit exceeded. Please slow down.'}}` 并设 `Retry-After = 剩余 TTL（≤0 时为窗口秒数）`，不再进入下游。来源 `server/src/middlewares/rate-limit.ts:126-140`。
- `RL-006` Redis 出错时 fail-open（只 warn，放行）。来源 `server/src/middlewares/rate-limit.ts:141-144`。
- `RL-007` 已知取舍：固定窗口边界最坏可放行 2×limit，可接受；严格配额的入口另有计数器。来源 `server/src/middlewares/rate-limit.ts:6-11`。

### 7.2 认证接口限流（users-permissions）

- `RL-010` 认证路由每 IP 每路由每分钟 20 次；键 `up-auth:{小写去尾斜杠的 path}:{ip}`，IP 规则同 `getClientIp`；键中**不得**包含请求体中的 identifier/email（否则变换字段即可无限配额）。来源 `server/config/plugins.ts:268-304`。

### 7.3 客户端 IP

- `RL-020` `getClientIp`：`TRUSTED_PROXY_HOPS`（≥0 整数，默认 1）；为 0 时直接用 socket 对端地址；否则取 `X-Forwarded-For` 拆分后**从右往左第 hops 个**（`parts[len - hops]`，越界取 `parts[0]`），无 XFF 时用 socket 地址，再无则 `0.0.0.0`。不使用最左值（可伪造）。来源 `server/src/utils/common.ts:486-518`。
- `RL-021` 部署规则：按代理层数填（现网只有 EdgeOne 一层，填 1，见 `deploy/README.md`）；层数变化不同步会导致匿名桶塌成全站一个。来源 `server/docs/search-sections-deployment.md`「四、安全红线」。
- `RL-022` Koa 代理需配置为 `server.proxy.koa = true`（写成 `proxy: true` 无效，导致 `ctx.secure` 恒 false、安全 cookie 抛错、`ctx.ip` 恒 127.0.0.1）。「Strapi 特有」，但重写时同样要正确处理 TLS 终止于反代的情况。来源 `server/config/server.ts:8-16`。

### 7.4 Redis 工具

- `RL-030` `incrWithTtl`：Lua `INCRBY` 后若 `PTTL < 0` 则 `PEXPIRE`；TTL 毫秒 = `max(1, ceil(ttlSeconds*1000))`（绝不下发 0）；不延长已有 TTL；能自愈历史无 TTL 键；不依赖 Redis 7 的 `PEXPIRE NX`。来源 `server/src/utils/redis.ts:11-76`、`server/tests/unit/incr-with-ttl.test.ts:127-215`。
- `RL-031` Redis 客户端：`maxRetriesPerRequest: 3`，重连退避 `min(times*200, 5000)` ms。来源 `server/src/utils/redis.ts:37-55`。
- `RL-032` `REDIS_URL` 解析：仅 `redis:`/`rediss:`，不允许 query/hash，db 必须为数字；`rediss:` 启用 TLS（主机名作 servername，IP 不设）；IPv6 去方括号；凭据 URL 解码；非法时报 `Invalid REDIS_URL configuration` 且不得泄露 URL/凭据；默认 `redis://127.0.0.1:6379/0`。来源 `server/src/utils/redis-config.ts:5-24`、`server/tests/unit/redis-config.test.ts`。

---

## 8. 错误格式与 HTTP 横切（ERR）

- `ERR-001` 每个请求生成 `requestId = randomUUID()` 并写入响应头 `X-Request-Id`。来源 `server/src/middlewares/error-format.ts:30-31`。
- `ERR-002` 统一错误体 `{error:{code, message, details?}}`。状态取 `err.status ?? err.statusCode ?? 按 name 映射 ?? 500`，不在 400–599 时按 500。来源 `server/src/middlewares/error-format.ts:32-47,71-77`。
- `ERR-003` name→状态映射：`ValidationError/ApplicationError/YupValidationError/PaginationError 400`、`UnauthorizedError 401`、`ForbiddenError/PolicyError 403`、`NotFoundError 404`、`PayloadTooLargeError 413`、`RateLimitError 429`、`NotImplementedError 501`、`TimeoutError/AbortError/SniffTimeoutError 503`。来源 `server/src/middlewares/error-format.ts:7-26`。
- `ERR-004` `code` 取 `err.code`（字符串）→ `err.name` → 5xx 为 `INTERNAL_SERVER_ERROR`、否则 `REQUEST_ERROR`；`details` 取 `err.details` 或 `err.data`（对象）。来源 `server/src/middlewares/error-format.ts:79-100`。
- `ERR-005` 5xx 脱敏：503 返回 `{code:'SERVICE_UNAVAILABLE',message:'Service temporarily unavailable',details:{requestId}}`，其它 5xx 返回 `{code:'INTERNAL_SERVER_ERROR',message:'Internal server error',details:{requestId}}`；控制器直接设置 5xx（未抛错）同样被改写。来源 `server/src/middlewares/error-format.ts:38-46,125-128`。
- `ERR-006` 5xx 写服务端日志一行：`[500] requestId=<id> <stack|message|JSON>`。来源 `server/src/middlewares/error-format.ts:102-120`。
- `ERR-007` 4xx 若 body 为非空字符串，包装为 `{code:'REQUEST_ERROR', message: body}`；已有 `error` 对象则不动。来源 `server/src/middlewares/error-format.ts:129-135`。
- `ERR-008` 消息中文翻译表：`Invalid identifier or password→账号或密码错误`、`Your account email is not confirmed→账号邮箱未验证`、`Your account has been blocked by an administrator→账号已被管理员禁用`、`Missing or invalid credentials→缺少或无效的登录凭证`、`Policy Failed→没有权限执行该操作`。来源 `server/src/middlewares/error-format.ts:61-69`。
- `ERR-009` 唯一约束冲突判定需逐层下钻 `cause/originalError/details/error/errors[]`（深度 ≤5）：MySQL `ER_DUP_ENTRY`/errno 1062、PG `23505`、SQLite `SQLITE_CONSTRAINT(_UNIQUE)`/errno 19/`UNIQUE constraint failed`；漏判会让业务冲突变 500。来源 `server/src/utils/common.ts:337-384`。
- `ERR-010` CORS 白名单：`https://tiwat.cn`、`https://www.tiwat.cn`、`https://interk.net`、`https://www.interk.net`、`https://dev.interk.net`、`http://localhost:3000`、`http://localhost:5173`；方法 `GET,POST,PUT,PATCH,DELETE,HEAD,OPTIONS`；`credentials: true`；允许头 `Content-Type, Authorization, Origin, Accept, Cache-Control`；出错时保留 CORS 头。来源 `server/config/middlewares.ts:37-54`。
- `ERR-011` CSP：默认指令 + `script-src 'self' 'unsafe-inline' cdn.jsdelivr.net`、`connect-src 'self' https:`、img/media 源见 `CDN-009`、`upgradeInsecureRequests: null`。主要服务 Strapi admin 页面。「Strapi 特有」，新后台按需重定。来源 `server/config/middlewares.ts:8-36`。
- `ERR-012` 请求体上限：JSON/form/text 均 `30mb`，multipart 单文件 30 MiB。来源 `server/config/middlewares.ts:76-86`。
- `ERR-013` 中间件顺序：`strapi::errors` → `error-format` → security → cors → poweredBy → logger → rate-limit → query → sanitize-cache-query → body → upload-validate → session → favicon → public。来源 `server/config/middlewares.ts:1-103`。

---

## 9. 响应缓存（CACHE）—「Strapi 特有」

- `CACHE-001` strapi-cache 插件（Redis provider，键前缀 `strapi-cache:`）缓存匿名 GET 响应 30 秒；可缓存前缀 `/api/articles`、`/api/comments`、`/api/authors`、`/api/article-reads`、`/api/categories`，排除 `/api/articles/list`；实体变更自动清理、启动时清空。目的：降低匿名浏览对 DB 的压力。来源 `server/config/plugins.ts:143-187`。
- `CACHE-002` `cacheAuthorizedRequests: false`：带鉴权的请求不读缓存，必须走 JWT/session 校验以便撤销立即生效。来源 `server/config/plugins.ts:170-173`。
- `CACHE-003` 缓存键：匿名 `{method}:{归一化url}`；带非空 Bearer 时 `{method}:u:{sha256(authorization)前32位}:{url}`。来源 `server/config/plugins.ts:174-183`、`server/tests/unit/cache-key.test.ts:25-48`。
- `CACHE-004` 必须设置 keyPrefix：插件 reset 在无前缀时会 `FLUSHDB`，冲掉同库的 `dm:*`、`presence:*`、`hot:*`、限流等应用键。重写若用 Redis 缓存，同样必须命名空间隔离。来源 `server/config/plugins.ts:160-168`。
- `CACHE-005` query 白名单归一化（缓存键与控制器看到的 URL 共用）：仅保留白名单参数并按白名单顺序重排，重复参数取首个，值重新编码；前缀需落在路径段边界（不以 `/` 结尾的前缀要求下一字符为 `/` 或结束）；更具体的前缀排在前面。白名单：`/api/articles/list[start,limit,category,feed,sort,cursor,tag,ids]`、`/api/articles/search[q,start,limit,category,sort]`、`/api/articles/suggest[q,category,limit]`、`/api/articles/bilibili-info[bvid,aid]`、`/api/articles/detail/[]`、`/api/articles/my/detail/[]`、`/api/articles/my/drafts[start,limit]`、`/api/articles/my/published[start,limit]`、`/api/articles/my[start,limit]`、`/api/comments/list[article,start,limit]`、`/api/comments/search[q,start,limit,article]`、`/api/comments/count[article]`、`/api/comments/[cursor,limit]`、`/api/authors/search-lite[q,start,limit]`、`/api/authors/search[q,limit]`、`/api/authors/[start,limit]`、`/api/article-reads[start,limit]`、`/api/categories/list[]`。来源 `server/src/utils/cache-query.ts:14-101`、`server/src/middlewares/sanitize-cache-query.ts:15-22`。
- `CACHE-006` 教训：漏掉 `cursor`/`tag` 会导致翻页永远返回第一页、tag 页显示全站帖子；未归一的可缓存路由可被 `?junk=<随机>` 打出无限键撑爆 Redis。重写若保留任何 URL 级缓存，必须同样做参数白名单。来源 `server/src/utils/cache-query.ts:15-45`。
- `CACHE-007` 额外清理：文章/标签相关写入在提交后（合并到一次 `setImmediate`）按正则 `/api/articles(?:[/?]|$)` 清缓存；跳过新建标签、只改草稿的写、与公开列表无关字段的更新（仅 `tags/publishedAt/isHidden/documentId` 变化触发）。账号注销清理按 `/api/(articles|comments|authors)` 清缓存。来源 `server/src/api/tag/services/cache.ts:21-57`、`server/src/utils/account-privacy.ts:4-14`。

---

## 10. 定时任务（CRON）

所有 cron 由 Strapi cron（node-schedule）执行，`{rule, tz}` 原样传入；每个任务都有进程内防重入标志（上一轮未完成则跳过）。来源 `server/config/cron-tasks.ts:17-27,542-620`、`server/config/server.ts:18-21`。

| 任务名 | rule | tz | 语义摘要 | 来源 |
|---|---|---|---|---|
| `drain-report-actions` | `* * * * *` | Asia/Shanghai | 执行举报删除持久任务（每轮最多 10 条） | `server/config/cron-tasks.ts:543-549` |
| `authSessionCleanup` | `*/5 * * * *` | Asia/Shanghai | 删除 `expiresAt < now-7天` 的会话，每轮最多 1000 行 | `server/config/cron-tasks.ts:550-555` |
| `accountPrivacyCleanup` | `* * * * *` | Asia/Shanghai | 重试已注销账号的派生数据清理，每轮最多 20 个 | `server/config/cron-tasks.ts:556-559` |
| `reconcile-interaction-counts` | `23 4 * * *` | Asia/Shanghai | 点赞/投币物化计数全量对账 | `server/config/cron-tasks.ts:564-569` |
| `reconcile-notification-buckets` | `47 4 * * *` | Asia/Shanghai | 回填 `notifications.bucket_key IS NULL` 并重算桶 | `server/config/cron-tasks.ts:578-583` |
| `repair-notification-bucket-gaps` | `* * * * *` | Asia/Shanghai | 环形扫描补缺失的桶行，每轮 500 | `server/config/cron-tasks.ts:584-590` |
| `dispatch-agent-outbox` | `* * * * *` | Asia/Shanghai | 把 `queued` AI 任务投递到 Redis 队列，每轮 50 | `server/config/cron-tasks.ts:591-597` |
| `settle-agent-replies` | `* * * * *` | Asia/Shanghai | 结算消息/评论上残留的 AI 配额收据，每表每轮 50 | `server/config/cron-tasks.ts:598-604` |
| `drain-follow-fanout` | `* * * * *` | 无（进程时区） | 消费关注者扇出通知队列 | `server/config/cron-tasks.ts:614-619` |
| （非 cron）热榜刷新 | `setInterval(HOT_REFRESH_SEC)` | — | 启动即刷新一次，之后每 600 秒（上限 3600） | `server/src/api/article/services/hot.ts:212-218` |

- `CRON-001` reconcile-interaction-counts 口径：`articles.likescount ↔ likes_article_lnk(article_id, like_id)`（身份列 `document_id`）、`comments.likescount ↔ likes_comment_lnk(comment_id, like_id)`（身份列 `id`）、`articles.denny_count ↔ denny_givens_article_lnk(article_id, denny_given_id)`（身份列 `document_id`）；收藏无物化列，不对账。来源 `server/config/cron-tasks.ts:55-88`。
- `CRON-002` 对账 SQL：`LEFT JOIN` 联表、`WHERE identity IS NOT NULL`、`GROUP BY identity`、`HAVING MIN(COALESCE(cnt,0)) <> COUNT(DISTINCT link) OR MAX(COALESCE(cnt,0)) <> COUNT(DISTINCT link)`；必须 `COUNT DISTINCT`（同一点赞可能同时挂在 draft 与 published 两行）。来源 `server/config/cron-tasks.ts:110-133`。
- `CRON-003` 只更新不一致的行：`WHERE identity=? AND (cnt IS NULL OR cnt <> total)`，因此可重入、多实例并跑安全、无需锁；日志记录漂移目标数、修正行数（样例最多 10 个 `identity=total`），即使 0 也要打一条完成日志；单口径失败不影响其它口径。已知窗口：快照与写回间的新点赞会差 1，下一轮自愈。来源 `server/config/cron-tasks.ts:135-219`。
- `CRON-004` reconcile-notification-buckets：按 `id DESC` 游标扫 `bucket_key IS NULL` 的通知（LEFT JOIN recipient/sender/comment 联表），每批 2000、每轮上限 20000；游标无条件前进到本批最小 id；推不出 senderKey 的行计 `unclassified` 跳过；无 recipient 的行仍回填键但计 `orphaned`；回填 `UPDATE ... WHERE id IN (...) AND bucket_key IS NULL`（写裸 senderKey，例如 `contacts:2`）；之后按 `BUCKET_RECOMPUTE_CAP` 切片重算 `(recipient, senderKey)` 桶（只拉正、不新建）。不受 `NOTIF_BUCKETS_ENABLED` 开关影响。整个任务不向调度器抛错。来源 `server/config/cron-tasks.ts:221-510`。
- `CRON-005` repair-notification-bucket-gaps：从模块级 `repairCursor` 起按 `id ASC` 取 `limit`（1..2000，默认 500）条有 `bucket_key` 的通知，LEFT JOIN `notification_buckets(recipient_user_id, sender_key)`；缺桶的目标取该 (收件人, senderKey) 最新一条通知 upsert 桶行（含 preview、lastAt、lastNotificationId）并失效该用户 DM/通知缓存；不足一批时游标归零。来源 `server/src/api/notification/utils/bucket.ts:586-623`。
- `CRON-006` drain-report-actions：选 `state='pending' 且 (next_attempt_at 为空或 ≤ now)` 或 `state='processing' 且 lease_until < now` 的行，按 id 升序最多 10 条；CAS 认领（随机 claim_token，租约 5 分钟）；先在事务外 `documents.delete` 目标文章/评论，再在举报组锁 + 事务内校验 token，写系统通知（给内容作者及该组 pending/reviewing 举报的举报人，跳过 deleted/merged 账号，消息截断 500），把这些举报改为 `accepted / content_deleted`，任务置 `complete` 并清空 payload；失败则回到 `pending`，`attempts+1`，退避 `min(3600, 30 * 2^min(attempts,7))` 秒，`last_error` 截断 2000。来源 `server/src/api/report/utils/report-actions.ts:78-199`。
- `CRON-007` authSessionCleanup：删除早于 7 天前过期的会话（保留过期前驱会话以检测刷新令牌重放）。来源 `server/src/api/auth-session/services/auth-session.ts:192-198`。
- `CRON-008` accountPrivacyCleanup：对 `accountStatus='deleted' AND accountCleanupPending=true` 的用户（每轮 20），并行执行搜索索引作者匿名化与响应缓存清理，成功后置 `accountCleanupPending=false`；单个失败只 warn。来源 `server/src/utils/account-privacy.ts:4-24`。
- `CRON-009` dispatch-agent-outbox：取 `ai_agent_jobs` 中 `status='queued' AND payload 非空`，按 id 升序 50 条；逐条 `enqueue` 到 Redis `ik:agent:jobs`（去重键 `ik:agent:outbox:{jobId}`，TTL 86400 秒），再 CAS 更新为 `processing`。数据库行是任务的接受点，Redis 故障由该任务重试。来源 `server/src/utils/agent/queue.ts:42-71`。
- `CRON-010` settle-agent-replies：对 `message`、`comment` 两表中 `quotaReceipt` 非空的行（各 50 条，id 升序）执行幂等 `settleQuota` 后清空 `quotaReceipt`。来源 `server/src/utils/agent/quota-receipt.ts:16-32`。
- `CRON-011` drain-follow-fanout：Redis 锁 `ik:fanout:lock`（TTL 120 秒）互斥，抢不到即返回；持锁后先回收 inflight 中的孤儿任务；每轮上限：时间 15 秒、通知 500 条、任务 50 个；关注者按 1000 一页游标处理；同一 ref 一轮只处理一次（再取到就放回队首并结束本轮）；成功耗尽则 ack 并清除入队去重键，未完成则先入队续跑任务再从 inflight 摘除，失败走重试；永不向调度器抛错。来源 `server/src/api/article/utils/follow-notification.ts:364-453`、`server/src/utils/fanout-queue.ts:32,62`。

---

## 11. 启动任务（BOOT）

执行顺序（`register` 后 `bootstrap` 内串行）。来源 `server/src/index.ts:10-70`。

- `BOOT-001` register 阶段：bcryptjs 替换为走线程池的原生实现（纯 JS 单次约 100ms 会阻塞事件循环）；必须在 HTTP listen 前完成。来源 `server/src/index.ts:10-15`。
- `BOOT-002` `patchUploadProviderDelete`（见 `FILE-010`）。来源 `server/src/index.ts:18`。
- `BOOT-003` `backfillBumpedAt`：`articles.bumped_at IS NULL` 的行设为 `COALESCE(published_at, updated_at)`；幂等，失败只 warn。来源 `server/src/index.ts:189-207`。
- `BOOT-004` `backfillConversationLastMessageAt`：`conversations.last_message_at IS NULL` 设为 `COALESCE(updated_at, created_at)`。来源 `server/src/index.ts:209-229`。
- `BOOT-005` `exemptLegacyUsersFromExam`：仅执行一次（store `plugin:exam` 键 `legacyExemptionDone`）；把 `up_users` 中 `exam_passed = false 或 NULL` 的所有用户设为 `exam_passed=true, exam_passed_at=now`，然后写标记；之后注册的新用户不受影响。来源 `server/src/index.ts:167-187`。
- `BOOT-006` `ensureHotpathIndexes`（仅 MySQL/PG，先查元数据存在性，单项失败不阻断）：`idx_articles_bumped(bumped_at,id)`、`idx_messages_created(created_at,id)`、`idx_comments_created(created_at,id)`、`idx_likes_created(created_at,id)`、`idx_favorites_created(created_at,id)`、`idx_denny_givens_created(created_at,id)`、`idx_hot_views_recent(last_viewed_at,article_document_id)`、唯一 `article_hot_views_dedupe_key_unique(dedupe_key)`、`idx_conversations_last_message(last_message_at,id)`。来源 `server/src/index.ts:350-422`。
- `BOOT-007` `ensureCommentFulltextIndex`：MySQL `ALTER TABLE comments ADD FULLTEXT ft_comments_content(content)`；PG `CREATE INDEX ft_comments_content ON comments USING GIN (to_tsvector('simple', COALESCE(content,'')))`；失败只 warn（只影响评论 SQL 兜底搜索速度）。来源 `server/src/index.ts:424-461`。
- `BOOT-008` `ensureNotificationBuckets`（仅 MySQL/PG，表与列存在时）：建 `idx_notification_buckets_window(recipient_user_id, last_at DESC, sender_key DESC)`；把有 `bucket_key` 的通知按 `(recipient, bucket_key)` 聚合（`MAX(created_at)`、`MAX(id)`），对不存在的桶以 500 一批直插：`bucket_key = u{uid}:{senderKey}`、`category = senderKey 冒号前部分`、`preview` 留空、`document_id = bkt{uid}x{hex(senderKey)前40}`；已存在的桶不动。来源 `server/src/index.ts:231-348`。
- `BOOT-009` `ensureAccountV2Indexes`（表存在才建）：`auth_sessions` 上唯一 `uq_auth_sessions_refresh_hash(refresh_token_hash)`、`idx_auth_sessions_user_active(user_id,revoked_at,expires_at)`、`idx_auth_sessions_token_family(token_family)`、`idx_auth_sessions_expiry(expires_at)`；`user_identities` 上唯一 `(provider,provider_subject)`、唯一 `(provider,provider_account_id)`、`idx_user_identities_provider_alias(provider,provider_alias)`；`auth_login_attempts` 上唯一 `(opaque_state_hash)`、`idx_auth_login_attempts_expiry(status,expires_at)`。来源 `server/src/index.ts:481-521`。
- `BOOT-010` `ensureTagIndexes`（失败**阻断启动**）：MySQL 把 `tags.slug` 排序规则改为 `utf8mb4_bin`；确保 `slug` 单列唯一约束（缺则建 `uq_tags_slug`，并发安装由另一实例完成也算成功）；PG 建 `idx_tags_slug_prefix (slug text_pattern_ops)`，SQLite 建 `COLLATE NOCASE` 前缀索引。来源 `server/src/api/tag/services/indexes.ts:57-84`。
- `BOOT-011` `subscribeTagCacheInvalidation`（见 `CACHE-007`）。来源 `server/src/index.ts:27`。
- `BOOT-012` `syncSearchIndexSettings`：幂等同步 Meilisearch `article`/`comment` 索引的 searchable/filterable/sortable/ranking 设置；失败只 warn。索引名可带 `MEILISEARCH_INDEX_PREFIX`。来源 `server/src/index.ts:29-42`、`server/config/plugins.ts:31-141`。
- `BOOT-013` `ensureAuthorForUser`：订阅用户 `afterCreate`，若该用户无 author，则创建 `author{name: username, user: id}`；失败只记 error，不影响注册。来源 `server/src/index.ts:44-49,76-91`。
- `BOOT-014` DM WebSocket 服务挂到 HTTP server 的 upgrade 事件；失败只 warn（REST/SSE 照常）。来源 `server/src/index.ts:51-61`。
- `BOOT-015` 热榜刷新器（`HOT_ENABLED !== 'false'` 时）；`destroy` 时停止。来源 `server/src/index.ts:63-73`。
- `BOOT-016` 教训：迁移在 schema sync 之前执行，新表上的索引/回填在迁移里会被跳过且不再重跑，所以放在 bootstrap 幂等执行。重写时用常规迁移即可。「Strapi 特有」。来源 `server/src/index.ts:231-241,350-353`。

---

## 12. 邮件调度（MAIL）

- `MAIL-001` 收件人域名（最后一个 `@` 之后、小写）属于国内列表时为 `domestic`，否则（含非法邮箱）为 `international`。国内列表：`qq.com, vip.qq.com, foxmail.com, 163.com, vip.163.com, 126.com, yeah.net, sina.com, sina.cn, vip.sina.com, sohu.com, aliyun.com, 139.com, 189.cn, wo.cn, tom.com, 21cn.com, sina.com.cn, 263.net, x263.net, etang.com, eyou.com, 56.com, chinaren.com, 188.com, vip.188.com`，另可用 `EMAIL_DOMESTIC_DOMAINS`（逗号分隔，进程内缓存）追加。来源 `server/src/utils/email/domains.ts:9-80`。
- `MAIL-002` 路由：domestic 主通道阿里企业邮箱 SMTP、备用 Brevo；international 主通道 Brevo、备用阿里。来源 `server/src/utils/email/index.ts:39-42`。
- `MAIL-003` 降级：主通道失败且 `EMAIL_FALLBACK_ENABLED`（默认 true；`true/1/false/0`）时切到备用；关闭时直接抛主通道错误；两路都失败抛备用通道错误（调用方处理 `*_EMAIL_FAILED`）。返回 `{provider, region, fellBack, messageId?}`。来源 `server/src/utils/email/index.ts:46-77`。
- `MAIL-004` 降级计数：Redis `ik:email:fallback:{失败的provider}:{UTC YYYY-MM-DD}`，`incrWithTtl` TTL 7 天（自当天首次起算，不续期）；fire-and-forget，Redis 失败只 warn。来源 `server/src/utils/email/index.ts:61-97`。
- `MAIL-005` 日志中邮箱脱敏：保留前 2 字符 + `***` + `@domain`；`@` 位置 ≤2 时只保留首字符；无合法 `@` 为 `***`。来源 `server/src/utils/email/index.ts:99-105`。
- `MAIL-006` 阿里 SMTP：`SMTP_HOST`、`SMTP_PORT`（默认 465）、`SMTP_SECURE`（默认 true）、`SMTP_USER/PASS`、`SMTP_DEFAULT_FROM`、`SMTP_DEFAULT_REPLYTO`；连接池 `maxConnections 3`、`maxMessages 100`、连接/握手超时 10 秒、socket 超时 15 秒。来源 `server/src/utils/email/config.ts:55-67`、`server/src/utils/email/senders/ali-smtp.ts:12-31`。
- `MAIL-007` Brevo：`POST {BREVO_API_URL 默认 https://api.brevo.com/v3}/smtp/email`，头 `api-key: BREVO_API_KEY`，超时 `BREVO_TIMEOUT_MS` 默认 10000；`BREVO_FROM` 解析 `"Name" <email>` 或纯邮箱为 `sender`；可选 `BREVO_REPLYTO`；正文同时发 html 与 text。来源 `server/src/utils/email/config.ts:68-74`、`server/src/utils/email/senders/brevo-api.ts:10-55`。
- `MAIL-008` 邮件配置独立于 Strapi email 插件，懒加载单例。来源 `server/src/utils/email/config.ts:1-4,46-79`。

新系统实现（`crates/mail`，配置 `mail.*`，见 `.env.example`）：

- MAIL-001～003、005～007 语义不变。旧环境变量对应：`SMTP_*` → `IK_MAIL__SMTP__*`（`SMTP_SECURE=true` → `tls = implicit`，
  `false` → `starttls`，且必须升级成功才发送），`BREVO_*` → `IK_MAIL__BREVO__*`，`EMAIL_FALLBACK_ENABLED` → `IK_MAIL__FALLBACK`，
  `EMAIL_DOMESTIC_DOMAINS` → `IK_MAIL__DOMESTIC_DOMAINS`（JSON 数组）。
- 只配置一个通道时所有邮件都走它（旧系统两个通道都必须配置）。
- MAIL-004 的 Redis 计数器不再保留（没有 Redis）：降级写一条 `mail_fallback = true` 的警告日志，按日志计数告警。
- SMTP 邮件带 `Message-ID: <随机串@发件域名>`；连接池上限 3（`pool_size`），超时 15 秒（`timeout_secs`）。

---

## 13. 后台手工维护内容（ADMINCRUD）

以下类型没有公开写接口（或只有只读接口），运营在 Strapi Admin Content Manager 或自定义插件中维护。新管理后台需提供对应 CRUD。来源各 `server/src/api/*/routes/*.ts` 与 `schema.json`。

| 内容类型 | 公开 API | 字段（类型/约束/默认） | 后台需求 |
|---|---|---|---|
| `category`（分区） | `GET /categories/list`（公开）、core `find` | `name` string；`slug` uid；`order` int=0；`isActive` bool=true；`adminOnly` bool=false；`allowCommentAccept` bool=false；`description` text；`articles` 1:N | 增删改、排序、上下架、是否仅管理员可发帖、是否允许采纳评论 |
| `tag`（标签） | `GET /tags/suggest`（公开） | `name` string 必填 ≤30；`slug` string 必填唯一 ≤60（二进制排序）；`isBlocked` bool=false private；`articles` M:N | 改名、封禁（改名/封禁要清文章缓存） |
| `emote`（表情） | `GET /emotes/manifest`（公开） | `code` 唯一必填；`name` 必填；`image` 单图必填；`group` N:1；`order` int≥0=0；`isActive` bool=true | 增删改、排序、上下架、分组 |
| `emote-group` | 无 | `name` 唯一必填；`order` int≥0=0；`isActive` bool=true；`icon` 单图；`emotes` 1:N | 增删改、排序、上下架 |
| `avatar`（系统头像） | core `find/findOne` | `name` 必填；`type` enum `character/city/news` 必填=character；`image` 单图 | 增删改、上传图片 |
| `business-card`（名片） | core `find/findOne` | `name` 必填；`description` text；`story` blocks（富文本）；`type` enum `character/city/news`=character；`image` 单图 | 增删改、富文本编辑 |
| `ai-model` | 无（对外仅 `GET /api/agent/models`） | `key` 必填唯一 ≤64；`displayName` 必填 ≤64；`description` text；`badge` ≤16；`astrbotProviderId`；`enabled` bool=true；`sortOrder` int=0；`costWeight` decimal=1；`inputPricePerMTokens`、`outputPricePerMTokens` decimal | 增删改、启停、排序、计费权重 |
| `ai-role-card` | 无 | `displayName` 必填；`slug` uid 必填；`bio` text；`avatar` 单图；`boundUser` N:1 用户；`enabled` bool=false；`knockVisible` bool=true；`dmEnabled` bool=true；`commentEnabled` bool=true；`sortOrder` int=0；`outputMode` enum `plain/markdown`=plain；`astrbotRef`；`defaultModel` N:1 ai-model；`allowedModels` M:N ai-model；`suggestedQuestions` json | 增删改、绑定 AI 用户账号、模型白名单 |
| `ai-agent-config`（单例） | 无 | `globalEnabled` bool=true；`dailyTokensPerUser` int≥0=1000000；`dailyTokens` int≥0=0 | 编辑全局开关与额度 |
| `exam-question` | 无公开 CRUD；有自定义后台插件 | `question` text 必填；`type` enum `single/multiple/boolean`=single；`options` json 必填（`[{key,text}]`）；`correctKeys` json 必填 private；`explanation` text private；`weight` int≥1=1；`enabled` bool=true；`tag` string | 列表筛选（题型/状态/关键词）、增删改、启停、批量删除、统计 |
| `author`（含虚拟作者） | 有公开读接口 | `name`；`avatar` 单图；`bio` blocks；`user` 1:1（可空）；`profilePinnedArticles` json | 创建不关联用户的虚拟作者并用于后台发帖 |
| 用户（users-permissions） | 自有资料接口 | 运营标志：`accountStatus` enum `active/blocked/deleted/merged`；`blocked`；`isAdmin`；`isAiAgent`；`examPassed`/`examPassedAt`；`profileHidden`；`denny`=30；`exp`/`level`；`confirmed`；`role` | 封禁/解封、设管理员/AI 账号、手动通过考试、调整丁尼/经验 |
| `report`（举报） | 自定义插件 `report-panel` | — | 列表、认领（reviewing）/取消认领（unclaim）、结案（隐藏/删除/封禁），权限 `read`/`resolve` |

- `ADMINCRUD-001` 考试题后台校验规则（与抽题一致）：每个选项 `key`、`text` 非空且 `key` 不重复；`correctKeys` 必须是选项 key 的子集；多选至少 2 个正确答案，单选/判断恰好 1 个；单选至少 2 个选项、多选至少 3 个、判断固定 2 个；`weight` 为 ≥1 的整数。启用题数小于 `EXAM_QUESTION_COUNT` 时考试返回 `EXAM_NOT_AVAILABLE`，后台需预警。来源 `server/docs/exam-question-management-plan.md` 3.2、3.3、6 节。
- `ADMINCRUD-002` 考试题删除不影响历史考试（`exam_attempt.questionsSnapshot` 已快照）；`correctKeys`/`explanation` 只对后台可见，公开 `/api/exam/*` 不返回。来源 `server/docs/exam-question-management-plan.md` 6 节。
- `ADMINCRUD-003` 考试题后台插件路由：`GET /list`、`GET /stats`、`POST /create`、`POST /bulk-delete`、`GET /:documentId`、`PUT /:documentId`、`POST /:documentId/toggle`；权限 `read`（查看入站题目管理）与 `write`（新增/编辑/删除入站题目）。来源 `server/src/plugins/exam-question-panel/strapi-server.js:20-149`。
- `ADMINCRUD-004` 举报后台插件路由：`GET /list`、`POST /:documentId/resolve`、`POST /:documentId/reviewing`、`POST /:documentId/unclaim`；权限 `read`（查看举报列表）与 `resolve`（处理举报：结案/隐藏/删除/封禁）。来源 `server/src/plugins/report-panel/strapi-server.js:15-72`。
- `ADMINCRUD-005` 虚拟作者：`author.user` 为空即虚拟作者，可复用于多篇帖子；前端此时头像回退到 `author.avatar`，不显示等级与装备头像，不能进入真实用户主页。来源 `server/docs/admin-virtual-author.md`。
- `ADMINCRUD-006` 后台直接发布文章不经过 API 控制器，因此由 lifecycle 兜底：发布时若 `bumpedAt` 为空则设为 `publishedAt`；草稿不顶帖；手动填写的 `bumpedAt` 不被覆盖；编辑已发布帖保留原 `bumpedAt`。新后台发帖必须实现同样规则。来源 `server/docs/admin-virtual-author.md`「三」。
- `ADMINCRUD-007` 后台需提供 Meilisearch 索引运维能力：添加 `comment` 内容类型索引、对 `article` 重建索引（残留旧文档曾导致搜索返回空页），并比对已索引数量与表行数。来源 `server/docs/search-sections-deployment.md` 二、6.1 节。

---

## 14. 运维文档中的其它规则与教训（OPS）

- `OPS-001` 热榜：`score = likes/(发布小时数+2)^gravity + (近期点赞人数+近期发言人数-1)/(窗口内首次发言距今小时数+2)^gravity`；结果写 Redis `hot:score:v1`，TTL 为刷新间隔的 3 倍，刷新失败继续用旧缓存；env `HOT_ENABLED=true`、`HOT_GRAVITY=1.2`、`HOT_RECENT_DAYS=7`、`HOT_REFRESH_SEC=600`、`HOT_CANDIDATES=1000`、`HOT_FEED_SIZE=200`、`HOT_COUNT=4`、`HOT_MAX_PER_AUTHOR=1`。完整规则属热榜领域。来源 `server/docs/hot-ranking-rollout.md`。
- `OPS-002` 搜索安全红线：匿名内容不得通过任何搜索路径泄露作者（索引中 `authorName/authorUserId` 置 null）；Meilisearch 命中后必须回查 DB 重新过滤隐藏与拉黑；「索引有命中但本页全部过不了回查」视为索引不可信，回退 SQL。来源 `server/docs/search-sections-deployment.md` 四、6.1 节。
- `OPS-003` `/authors/search-lite` 公开化的三道闸：Redis 限流 `authors:searchlite:rl:{userId|ip}`（登录 10 次/秒，匿名 3 次/秒）、`limit ≤ 20` 且 `start > 200` 返回空页、可见性过滤并截断 `bio` 至 120 字。来源 `server/docs/search-sections-deployment.md` 四。
- `OPS-004` 多值 media（如 `article.cover`）在作为单个引用对象返回时要先取第一张再生成缩略图，不能把数组原样输出（曾导致 Android 解析失败）。来源 `server/docs/search-sections-deployment.md` 6.2。
- `OPS-005` 部署：唯一性迁移遇到存量冲突时停止并报告，不自动合并或删除；已认证文件不可原地替换；图库移除不物理删除；实时撤销先广播关闭连接，并每 30 秒复查数据库状态。来源 `server/docs/backend-remediation.md`「部署与数据兼容」。
- `OPS-006` 只读审计/dry-run 脚本必须使用显式 `AUDIT_DATABASE_URL`（不读应用的 `DATABASE_URL`），连接设为只读会话，不启动 Strapi，不创建不存在的数据库。来源 `server/tests/unit/migration-read-only.test.ts:122-206`。
- `OPS-007` MCP 工具设计约束：工具不返回 email、password、真实 id 或隐私设置；写类工具默认关闭，需要用户确认、配额与考试校验、内容审核，并记录审计日志。来源 `server/docs/mcp-forum-tools-plan.md`「安全与隐私约束」。

---

## 测试中体现的关键用例

- 带后缀段 `token.another-receipt` 或带 `=` 填充的 token 必须被拒绝。来源 `server/tests/unit/direct-upload-security.test.ts:76-80`。
- 仅存在历史 `provider_metadata.contentHash` 的文件不能被复用，查询条件必须是 `verifiedContentHash`；签发的 token `version=2`，objectKey 以 `staging/` 开头。来源 `server/tests/unit/direct-upload-security.test.ts:81-90`。
- MIME 与 HEAD 都合法但字节不是图片时拒绝，且不创建文件行。来源 `server/tests/unit/direct-upload-security.test.ts:92-99`。
- 伪造 hash 时拒绝，不创建文件行，也不发起任何 PutObject。来源 `server/tests/unit/direct-upload-security.test.ts:101-107`。
- 只 PUT 一次，Body 与校验字节完全相同，key 不同于 staging key；同一 token 重试时即使 S3 已不可用也返回同一文件（收据幂等）。来源 `server/tests/unit/direct-upload-security.test.ts:109-123`。
- 两个并发 complete 返回同一 id，只有一行，赢家的对象不会被删除。来源 `server/tests/unit/direct-upload-security.test.ts:125-137`。
- 无 version 的旧 token 完成时不删除源对象。来源 `server/tests/unit/direct-upload-security.test.ts:139-144`。
- 跨 chunk 计算 hash；长度 ±1 都拒绝；签名 size 超过 30MiB 时不读取任何字节。来源 `server/tests/unit/upload-content.test.ts:10-28`。
- 收据在关系写入成功前对其它连接不可见；迁移可重复执行，历史行 `verified_content_hash` 保持 NULL，`direct_upload_id` 唯一；Content API 输入/输出都剥离这两列。来源 `server/tests/unit/upload-receipt-database.test.ts:12-92`。
- 已认证文件阻止 replace，且在触碰存储之前就拒绝；未认证文件保留原生替换。来源 `server/tests/unit/upload-replacement.test.ts:6-18`。
- 图库：未登录 401；缺 documentId 400；不存在或非本人 404；最后一个上传者移除后仍保留文件；有内容引用或其它上传者时只断开关联；重复删除幂等；不依赖对象存储。来源 `server/tests/unit/me-upload.test.ts:53-208`。
- 5xx 不泄露 SQL/凭据，details 只含 requestId；显式返回的 503 也被脱敏；4xx 保留 code/message/details；非法状态码（999）按 500 处理。来源 `server/tests/unit/error-format.test.ts:12-39`。
- 缓存键：匿名为 `method:url`，带 token 为 `method:u:<hash32>:url`，空 Bearer 回退匿名；垃圾参数被剥离；`/api/articles/my` 不吞掉更具体的子路由；documentId 以 `list` 开头的 `/replies` 请求保留 cursor；不同 cursor、tag 生成不同键；Redis 配置带 keyPrefix。来源 `server/tests/unit/cache-key.test.ts:24-175`。
- 限流：第 3 次（限额 2）返回 429，`Retry-After: 60`、`X-RateLimit-Remaining: 0`，窗口内只设一次 TTL；Redis 出错时放行。来源 `server/tests/unit/incr-with-ttl.test.ts:217-262`。
- 邮件：国内走阿里、国际走 Brevo；互为降级；降级计数为按天分桶、TTL 7 天；两路都失败时抛错；关闭降级时不切换；日志脱敏。来源 `server/tests/unit/email-dispatcher.test.ts:41-150`、`server/tests/unit/email-domains.test.ts`。

---

## 未决问题

1. 待确认：`server/tests/unit/upload-content.test.ts:29-59` 假设只有 2 个并发槽位，并期望排队超时返回 `status: 503`；但代码默认槽位为 6（`upload-content.ts:51`），超时错误为 `RateLimitError`（429）。`backend-remediation.md` 也写「最多同时处理两份30MiB缓冲」。重写应采用哪个默认值（2 还是 6），排队超时返回 429 还是 503？
2. 待确认：`server/tests/unit/error-format.test.ts:21` 期望 `log.error` 收到对象 `{requestId, error}`，但代码记录的是单行字符串（`error-format.ts:119`）。该测试目前是否通过需要确认，但不影响重写语义。
3. 待确认：`message.attachments`（多图 media）存在于 schema，但服务端 API 没有任何写入或读取它的代码。重写时是否需要支持私信图片附件？如需支持，上限是多少？
4. 待确认：文章封面、评论图片和自定义头像引用文件时都不校验上传者归属（`FILE-020`、`AVA-005`）。重写时是否要求只能引用自己 `uploaders` 中的文件？
5. 待确认：`me-avatar` 和 `me-uploads` 路由没有 `require-exam-passed`。自定义头像（扣 10 丁尼）是否应要求通过考试？
6. 待确认：内容去重会把任意用户加入已有文件的 `uploaders`。知道某张公开图片的 SHA-256 即可把它放进自己的图库。这一行为是否保留？
7. 待确认：staging 前缀的对象存储生命周期规则（保留天数）在仓库中没有定义，需要运维给出取值。
8. 待确认：引用 GC（同时扫描正文内嵌 URL 的物理删除流程）尚未实现。重写是否需要提供，还是继续只断开关联？
9. 待确认：`DIRECT_UPLOAD_SNIFF_TIMEOUT_MS` 只出现在 `.env.example` 中，代码不读取。重写时应删除该变量，还是把它与 `DIRECT_UPLOAD_VERIFY_TIMEOUT_MS` 合并？
10. 待确认：`drain-follow-fanout` 没有设置 `tz`，其余任务都设置为 `Asia/Shanghai`。因为它每分钟执行，时区不影响触发，重写时可以统一设置。
