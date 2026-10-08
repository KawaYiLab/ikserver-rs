# 账号与认证 — 业务规则规格（InterKnot Strapi 5 后端提取）

> 本文是从旧 Strapi 5（TypeScript）后端提取的**业务规则规格**，驱动 Rust 重写。
> 重写采用新 API + 新认证设计：不透明服务端会话 token 替代 JWT+refresh、Argon2id 哈希（存量 bcrypt 哈希迁移）。
> 旧路径与响应形状不必保留，但每条规则、校验、限额、流程、反滥用措施与边界行为必须等价或更安全。
> 文中标注 「新设计下不需要」 的规则表示该机制是 Strapi/JWT 特有的实现细节，可丢弃；但其**安全意图**以规则正文形式保留。
> 所有行号引用以提取时的代码版本为准。

## 范围

账号模型、注册、邮箱验证码（5 种用途）、密码登录 / 验证码登录 / 重置密码 / 修改密码、会话生命周期（旋转、重放检测、设备管理、实时撤销广播）、登录尝试状态机、米游社（国服）扫码登录 / 绑定 / 解绑 / 注销核验、身份模型（user_identity + 迁移）、账号注销与脱敏、认证中间件与限流、客户端 IP 解析、邮件双通道发送。

## 来源文件

后端 `server/`（均为只读来源）：

- `src/api/register-verification/services/register-verification.ts`（验证码五流程 + 注册 + 注销，~1207 行）
- `src/api/register-verification/controllers/register-verification.ts`、`routes/register-verification.ts`、`content-types/register-verification/schema.json`
- `src/api/auth-session/services/auth-session.ts`、`controllers/auth-session.ts`、`routes/auth-session.ts`、`content-types/auth-session/schema.json`
- `src/api/auth-login-attempt/**`（service/controller/routes/schema）
- `src/api/mihoyo-auth/services/mihoyo-auth.ts`、`controllers/mihoyo-auth.ts`、`routes/mihoyo-auth.ts`
- `src/api/mihoyo-binding/content-types/mihoyo-binding/schema.json`
- `src/api/user-identity/services/user-identity.ts`、`content-types/user-identity/schema.json`
- `src/api/me-security/controllers/me-security.ts`、`routes/me-security.ts`
- `src/utils/identity-service.ts`、`account-lock.ts`、`verification-attempt.ts`、`registration-policy.ts`、`auth-session-http.ts`、`auth-revocation.ts`、`session-location.ts`、`account-privacy.ts`、`bcrypt-native.ts`、`mihoyo.ts`、`common.ts`（getClientIp）、`self-profile.ts`、`after-commit.ts`
- `src/utils/email/{index,domains,config,senders/ali-smtp,senders/brevo-api}.ts`
- `src/middlewares/{optional-auth,require-auth,rate-limit,require-exam-passed,error-format}.ts`
- `src/extensions/users-permissions/strapi-server.ts`、`src/extensions/users-permissions/content-types/user/schema.json`
- `src/index.ts`（afterCreate 建 author、存量用户考试豁免、bootstrap 建索引）
- `config/plugins.ts`（users-permissions ratelimit 修正）、`config/middlewares.ts`、`config/server.ts`、`config/cron-tasks.ts`
- `database/migrations/2026.09.12T00.00.00.add-account-identity-model.js`、`2026.09.23T01.00.00.unique-account-email.js`
- `scripts/migrate-account-identities.js`
- `node_modules/@strapi/plugin-users-permissions/dist/server/{controllers/auth.js,controllers/validation/auth.js,services/user.js,config.js,routes/content-api/auth.js}`
- 前端（UX 意图）：`app/stores/auth.ts`、`app/utils/auth-session.ts`、`app/composables/useApi.ts`、`app/composables/useMihoyoQr.ts`、`app/pages/account.vue`
- 测试：`tests/unit/{auth-session-lifecycle,login-with-code,verification-atomicity,identity-service,account-identity-migrate,account-deletion,account-privacy,users-permissions-extension,mihoyo-auth-service,mihoyo,email-dispatcher,email-domains,optional-auth,realtime-revocation,session-location}.test.ts`

---

## 1. 账号数据模型与凭证规则（ACC）

### 数据与约束

- ACC-001 用户表核心字段：`username`（string，minLength 3，unique，required）、`email`（email 类型，unique，minLength 6，required）、`password`（password 类型，minLength 6，private，不 searchable）、`provider`（string）、`confirmed`（bool 默认 false）、`blocked`（bool 默认 false）、`role`（manyToOne 角色）。来源：`src/extensions/users-permissions/content-types/user/schema.json:16-75`。
- ACC-002 `accountStatus` 枚举：`active | blocked | deleted | merged`，默认 `active`，required。`active` 是唯一可登录状态；迁移会为旧行补默认值 `active`。来源：`schema.json:34-39`、`database/migrations/2026.09.12T00.00.00.add-account-identity-model.js:82-91`。
- ACC-003 `accountCleanupPending`（bool 默认 false，private）：注销后公开数据清理未完成标记，由 cron 重试直至完成。来源：`schema.json:41-45`、`src/utils/account-privacy.ts:11-13`。
- ACC-004 账号附属初始值：`denny` 默认 30（min 0，max 9999）、`level` 默认 1（注册时显式写 0，见 REG-006）、`exp` 默认 0、`examPassed` 默认 false、`profileHidden` 默认 false、`isAdmin` / `isAiAgent` 默认 false。来源：`schema.json:107-190`。
- ACC-005 邮箱规范化：所有认证流程入口处 `trim().toLowerCase()`；唯一性按规范化值判断。来源：`register-verification.ts:44-46`、`scripts/migrate-account-identities.js:27`。
- ACC-006 邮箱格式校验：长度 ≥ 6 且匹配 `/^[^\s@]+@[^\s@]+\.[^\s@]+$/`，否则 400 `INVALID_EMAIL`「邮箱格式不正确」。来源：`register-verification.ts:16,48-54`。
- ACC-007 邮箱唯一性是**大小写不敏感**的：迁移先将全表 `LOWER(TRIM(email))` 归一，若存在重复归一化邮箱则**中止部署**并只输出用户 id 列表（绝不输出邮箱/凭据），要求人工调和后才允许建唯一索引。来源：`database/migrations/2026.09.23T01.00.00.unique-account-email.js:7-26`。
- ACC-008 用户名唯一性是**大小写敏感**的（DB 唯一约束 + 精确匹配查询）；改名接口另有限制：trim 后长度 3–20 字符、与他人 `username` 精确冲突时报「该用户名已被占用」、每次改名扣 10 丁尼（余额恰好 10 可成功归零，9 拒绝）。来源：`src/api/me-profile/controllers/me-profile.ts:9-10,93-118,157-174`（改名细节属个人资料域，此处仅记账号侧约束）。
- ACC-009 注册与邮箱绑定时的冲突检测是**交叉四元**的：`email = X OR username = X OR (username = Y OR email = Y)`——邮箱值不能占用任何账号的 username 列，反之亦然（防止「用户名撞他人邮箱」绕过登录路由）。来源：`register-verification.ts:97-117`。
- ACC-010 无保留用户名清单；但自动生成的用户名前缀 `绳网用户`、注销后的 `已注销用户_<id>`、作者脱敏名 `已注销用户` 是系统占用形态，新系统应将这类前缀列入保留/模式黑名单。来源：`register-verification.ts:131,1130,1189`。**「待确认」**：旧系统未阻止用户手动改名为这些前缀。
- ACC-011 密码规则：最少 6 位（所有自定义流程统一，错误 400 `INVALID_PASSWORD`「密码长度不能少于 6 位」）；插件校验另有限制「UTF-8 字节数 ≤ 72」（bcrypt 输入上限），修改密码要求 `passwordConfirmation` 一致、`currentPassword` 正确、新旧密码不同。来源：`register-verification.ts:56-61`、`plugin-users-permissions/controllers/validation/auth.js:24,85-120`、`controllers/auth.js:201-215`。
- ACC-012 密码哈希：bcrypt cost=10（users-permissions 默认值），生产用 @node-rs/bcrypt 原生实现以避免阻塞事件循环；哈希格式 `$2[abxy]$10$...`，bcryptjs 与原生实现双向兼容（正则 `/^\$2[abxy]?\$\d{2}\$[./A-Za-z0-9]{53}$/`）。**新设计下**：换 Argon2id，存量 bcrypt 哈希需在首次成功校验时透明重哈希。来源：`src/utils/bcrypt-native.ts:28-29,57-107`、`services/user.js:61`。
- ACC-013 米游社占位邮箱：`mihoyo_<aid>@mihoyo-login.inter-knot.invalid`（后缀常量 `MIHOYO_PLACEHOLDER_EMAIL_SUFFIX`，唯一一处定义，大小写不敏感 endsWith 判定）。带此后缀 = 「没有可用邮箱」：不发验证码、不可邮箱登录。新系统应以独立标记位替代占位邮箱，但语义必须保留。来源：`src/utils/mihoyo.ts:38-49`。
- ACC-014 登录凭据可读字段最小化：optional-auth 只 select `id, level, isAdmin, isAiAgent, blocked, accountStatus`；会话/身份查询也只取必要标量；password 永不出接口（schema private + sanitize）。来源：`src/middlewares/optional-auth.ts:44-59`。

### 流程与状态机

- ACC-015 账号状态对认证的影响：`requireActiveAccount` 在 `!user || blocked || accountStatus !== 'active'` 时抛 403 `ACCOUNT_BLOCKED`「该账号已被封禁或已注销」；所有凭证变更（重置密码、绑邮箱、绑/解绑米游社、注销）都在其保护下执行。来源：`src/utils/account-lock.ts:10-16`。
- ACC-016 `merged` 状态（账号合并后的弃号）在 optional-auth / require-auth / 实时连接复检中一律视为不可认证（与 blocked/deleted 同列）；旧代码中无主动合并入口，仅为数据模型与防御。来源：`optional-auth.ts:57`、`require-auth.ts:36`、`auth-revocation.ts:62-63`。
- ACC-017 每个用户创建后由 DB lifecycle afterCreate 自动创建一个 `author`（人设作者）记录，`name = username`，幂等（已存在则跳过），失败只记日志不阻断。来源：`src/index.ts:44-49,76-91`。

### 副作用

- ACC-018 用户创建 ⇒ 自动建 author（ACC-017）。用户删除（注销）⇒ author 脱敏（DEL-007）。

---

## 2. 邮箱验证码（CODE）

适用 5 种用途，枚举 `purpose`: `register | reset-password | bind-email | delete-account | login`。来源：`register-verification/schema.json:30-45`。

### 数据与约束

- CODE-001 验证码记录字段：`email`（规范化后）、`codeHash`（private）、`purpose`、`sentAt`、`expiresAt`、`consumedAt`（null = 未消费）、`attempts`（默认 0）、`requestIp`（private）。来源：`register-verification/schema.json:21-60`。
- CODE-002 验证码 = 6 位数字，`crypto.randomInt(0, 1000000)` 生成后 `padStart(6,'0')`（允许前导零）；输入校验 `/^\d{6}$/`，错误 400 `INVALID_VERIFICATION_CODE`「验证码必须是 6 位数字」；数字类型输入会被 String 化。来源：`register-verification.ts:63-73`。
- CODE-003 TTL：`CODE_TTL_SECONDS` 默认 600s（10 分钟，env `REGISTER_CODE_TTL_SECONDS`）；重发冷却 `SEND_COOLDOWN_SECONDS` 默认 60s（env `REGISTER_CODE_COOLDOWN_SECONDS`）；错误尝试上限 `MAX_VERIFY_ATTEMPTS` = 5（env `REGISTER_CODE_MAX_ATTEMPTS`）。三个 env 都要求正整数，否则回落默认值。来源：`register-verification.ts:13-15,25-28`。
- CODE-004 验证码只存 HMAC-SHA256 哈希：`HMAC(secret, "<email>:<code>")`，secret 依次取 `REGISTER_CODE_SECRET` → `JWT_SECRET` → `server.app.keys`（数组 join(':')），都没有则 500 `REGISTER_CODE_SECRET_MISSING`「注册验证码密钥未配置」。哈希绑定邮箱，换邮箱即失效。来源：`register-verification.ts:75-91`。
- CODE-005 每个 (email, purpose) 同时只有一条有效验证码：发新码前把该 (email, purpose) 所有 `consumedAt: null` 的记录批量置为已消费（废弃旧码）。来源：`register-verification.ts:397-406` 及四种用途的相同段。
- CODE-006 邮件发送失败时立即把刚创建的记录置为已消费并抛 500 `<PURPOSE>_EMAIL_FAILED`「验证码邮件发送失败」（如 `REGISTER_CODE_EMAIL_FAILED`）；不会让无效码占用冷却窗口。来源：`register-verification.ts:420-429`。

### 流程与状态机（所有用途共用）

- CODE-007 冷却判定：查该 (email, purpose) 最近一条未消费且 `sentAt >= now-60s` 的记录；命中则 429 `<PURPOSE>_CODE_COOLDOWN`「验证码发送过于频繁，请稍后再试」，`details.retryAfter = max(1, 60 - 已过秒数)`。**注意**：reset-password 与 login 用途对冷却命中是**静默返回成功**（不抛错），见 CODE-010 反枚举。来源：`register-verification.ts:370-392,534-537,678-681`。
- CODE-008 验证流程固定顺序：取最新未消费记录（`sentAt desc, id desc`，同秒双发取新者）→ 不存在报 `*_CODE_NOT_FOUND` → 过期则顺手消费并报 `*_CODE_EXPIRED` → `attempts >= 5` 则顺手消费并报 429 `*_CODE_TOO_MANY_ATTEMPTS` → 哈希不等则 CAS 递增 attempts 报 `*_CODE_INVALID`（`details.attemptsRemaining`）→ 正确则**原子消费**（见 CODE-009）。来源：`register-verification.ts:447-481` 等。
- CODE-009 消费是单条 SQL 的 CAS：`updateMany where { id, consumedAt: null, attempts < 5（或 null）, expiresAt > now } set consumedAt=now`，`count !== 1` 即视为已被并发消费/过期，报 `*_CODE_NOT_FOUND`。这保证一码只用一次、并发注册只成一个账号。来源：`register-verification.ts:486-492,632-635,782-788,968-971,1144-1152`。
- CODE-010 错误尝试计数 `recordFailedVerification`：CAS 递增（`updateMany` 带全部谓词，不用 read-modify-write），达到上限的当次顺便置 `consumedAt`；只推进活跃挑战、绝不重开已消费记录；CAS 竞争重读重试，重试上限 `min(maxAttempts+1, 32)`；遇非法 attempts（非安全整数/负数/超限）或已过期直接停。来源：`src/utils/verification-attempt.ts:6-43`。

### 权限与反枚举

- CODE-011 5 种用途中，register / bind-email / delete-account 的发码接口对失败是**显式报错**的（邮箱已占用 409 `EMAIL_ALREADY_TAKEN`、冷却 429 等）；reset-password 与 login 的发码与登录提交是**反枚举**的，见 CODE-012/CODE-013/LOGIN-004。
- CODE-012 发送重置码：无论邮箱是否注册都返回相同结构 `{ email, sent: true, expiresIn, cooldown }`；未注册静默返回；冷却期内也静默返回（不发信）。发码对象固定为账号当前 email（即使请求按 username 命中）。来源：`register-verification.ts:506-572`、`controllers/register-verification.ts:45-57`。
- CODE-013 发送登录码：仅对「已注册 + 未 blocked + accountStatus≠deleted + 非占位邮箱」的账号真正发码，其余一律静默返回相同成功响应。来源：`register-verification.ts:643-716`。
- CODE-014 发送注册码：先查注册开关（403 `REGISTER_DISABLED`「当前不允许注册」），再查邮箱占用（409 `EMAIL_ALREADY_TAKEN`「该邮箱已注册」），再冷却。注册接口的占用报错是刻意的（注册场景不防枚举）。来源：`register-verification.ts:364-367`。

### 邮件模板（Chinese 文本，逐字保留）

- CODE-015 邮件主题格式：`【<code>】<用途>验证码 - 绳网`，用途依次为 `注册 / 重置密码 / 绑定邮箱 / 注销账号 / 登录`（登录主题为 `【<code>】登录验证码 - 绳网`）。纯文本正文统一格式：`您的<用途>验证码是 <code>，<N> 分钟内有效。…`（N = TTL/60）。来源：`register-verification.ts:313-361`。
- CODE-016 HTML 模板共用深色卡片版式（品牌「绳网」，logo `https://im.tiwat.cn/zzzicon_200x200.png`），preheader 文案 `您的<heading>是 <code>（<N> 分钟内有效）。如非本人操作请忽略。`；五种用途的 heading/description/footer：
  - 注册：「注册验证码 / 请使用以下验证码完成账号注册 / 如果你没有请求此验证码，请忽略此邮件。」
  - 重置密码：「重置密码 / 请使用以下验证码完成密码重置 / 如果你没有请求重置密码，请忽略此邮件，你的密码不会被更改。」
  - 绑定邮箱：「绑定邮箱 / 请使用以下验证码完成邮箱绑定 / 如果你没有请求绑定邮箱，请忽略此邮件。」
  - 注销账号：「注销账号 / 请使用以下验证码确认注销账号，此操作将不可撤回 / 如果你没有请求注销账号，请立即修改密码并忽略此邮件，你的账号不会被注销。」
  - 登录：「登录验证码 / 请使用以下验证码登录你的账号 / 如果你没有尝试登录，请忽略此邮件，并考虑修改账号密码。」
  来源：`register-verification.ts:154-311`。

### 副作用

- CODE-017 发码/验码不写账号本体；只有消费成功的流程（注册、重置、绑定、登录、注销）才改账号，见各自小节。
- CODE-018 验证码记录**无定期清理任务**（register_verifications 表只增不删；cron 里没有对应任务）。**「待确认」**：新系统应加过期行清理（如每日删除 expiresAt < now-7d）。来源：`config/cron-tasks.ts:542-569`（仅 authSessionCleanup 与 accountPrivacyCleanup 等）。

---

## 3. 注册（REG）

### 流程

- REG-001 唯一注册入口是验证码注册 `POST /api/auth/send-register-code` + `POST /api/auth/register-with-code`；Strapi 原生的 `POST /api/auth/local/register` 被整体替换为 400 `REGISTER_CODE_REQUIRED`「请使用邮箱验证码注册接口」，`details` 给出两个新端点路径。来源：`src/extensions/users-permissions/strapi-server.ts:4-13,64-75`。
- REG-002 注册开关与默认角色取自 users-permissions 插件 store 的 `advanced` 设置（`allow_register`、`default_role`，缺省 `{allow_register: true, default_role: 'authenticated'}`）；开关关闭时 403 `REGISTER_DISABLED`；角色不存在时 500 `DEFAULT_ROLE_NOT_FOUND`「默认用户角色不存在」。来源：`src/utils/registration-policy.ts:3-27`。
- REG-003 `register-with-code` 顺序：校验 email/code/password → 取注册角色（顺带校验开关）→ 邮箱占用检查 → 验证码状态机（CODE-008）→ 生成用户名 → 事务内：再次占用检查（锁内复查）→ CAS 消费验证码 → 创建用户 → 链接 email 身份（同一事务，失败整体回滚，不留无主账号或已消费码）。来源：`register-verification.ts:438-503`。
- REG-004 自动用户名：`绳网用户` + 4 位随机后缀（字母表 `0-9A-Za-z`，`crypto.randomInt`），最多尝试 20 次，每次检查与任何 `username`/`email` 冲突；20 次全撞则用 8 位后缀兜底（不再检查）。来源：`register-verification.ts:119-145`。
- REG-005 注册成功即签发会话并 201 返回（`loginMethod: 'email_code'`，见 SESS）。来源：`controllers/register-verification.ts:33-43`。
- REG-006 新用户初始值：`provider: 'local'`、`confirmed: true`（验证码即邮箱验证）、`level: 0`、`examPassed: false`、角色为注册默认角色、denny 走 schema 默认 30、email 身份 `verifiedAt = now`。来源：`register-verification.ts:493-501`。

### 边界与已知坑

- REG-007 入站考试：新用户 `examPassed=false`，写操作（发帖/评论/点赞/收藏/私聊/投币/签到）被 `require-exam-passed` 中间件拦截，403 `EXAM_REQUIRED`「需要先通过入站考试才能进行此操作」；`isAdmin`/`isAiAgent` 豁免；只读浏览不受限。存量用户在考试上线时一次性豁免（`exam_passed=true`，core_store 标记 `legacyExemptionDone` 保证只跑一次）。来源：`src/middlewares/require-exam-passed.ts:1-40`、`src/index.ts:167-187`。
- REG-008 米游社扫码首登也算「注册」，同样受注册开关限制：开关关闭时新米游社身份不能建号，但**已有账号不受影响仍可登录**。来源：`mihoyo-auth/services/mihoyo-auth.ts:421-430`、测试 `mihoyo-auth-service.test.ts:457-467`。

---

## 4. 密码登录 / 验证码登录 / 重置与修改密码（LOGIN）

### 密码登录（users-permissions callback 覆写）

- LOGIN-001 `POST /api/auth/local`：body `{identifier, password}`；查 `provider='local'` 且 `email = identifier.toLowerCase()` 或 `username = identifier`（注意 email 侧小写、username 侧原样）；用户不存在 / 无密码哈希 / 密码不符统一报 `ValidationError 'Invalid identifier or password'`（不区分哪种失败）。来源：`plugin-users-permissions/controllers/auth.js:64-90`。
- LOGIN-002 仅 `provider='local'` 可走密码登录；米游社占位邮箱账号（provider='mihoyo'）在通过重置密码自设密码（provider 被归回 'local'，见 LOGIN-009）之前无法密码登录。来源：`plugin-users-permissions/controllers/auth.js:68-79`、`register-verification.ts:627-636`。
- LOGIN-003 登录时检查 `email_confirmation` 开关（开启且 `confirmed !== true` 则拒绝「Your account email is not confirmed」）与 `blocked === true`（拒绝「Your account has been blocked by an administrator」）；插件**不检查 `accountStatus`**，但包装层在会话创建时会再查 `activeUser`（blocked 或 accountStatus≠active 则 403 `ACCOUNT_BLOCKED`）。来源：`plugin-users-permissions/controllers/auth.js:91-100`、`auth-session/services/auth-session.ts:38-43,81-87`。
- LOGIN-004 包装层（对 `callback` / `changePassword` / `resetPassword` 三个 action）：先 `assertAuthOrigin`；原 action 未产出 `jwt`+`user.id`（凭据错误）则原样返回、**不建会话、不回填身份**；产出成功响应后：若 `provider==='local'` 且有 email，回填 email 身份（`linkIdentity`，无 verifiedAt）；`changePassword`/`resetPassword` 先 `revokeAll` 再签新会话（顺序不可反，否则新会话被一起撤）；登录（callback）**不踢其他会话**；`loginMethod` 取 `password_change | password_reset | password | oauth`（按 action 与 ctx.params.provider，**不按账号的 provider 列**）。来源：`src/extensions/users-permissions/strapi-server.ts:33-62`。
- LOGIN-005 记录登录方式按**本次认证手段**而非账号 provider：绑过邮箱的米游社账号用密码登录，session.loginMethod 仍是 `password`。来源：测试 `users-permissions-extension.test.ts:248-272`、`auth-session-lifecycle.test.ts:424-441`。

### 邮箱验证码登录

- LOGIN-006 `POST /api/auth/send-login-code` + `POST /api/auth/login-with-code`。发码反枚举（CODE-013）。登录提交：账号不存在 / blocked / accountStatus='deleted' / 占位邮箱，统一抛 400 `LOGIN_CODE_INVALID`「验证码无效或已过期」（与验证码错误同形，不泄露账号状态）；其余走 CODE-008 状态机 + CAS 原子消费。来源：`register-verification.ts:723-798`。
- LOGIN-007 `login-with-code` 只认邮箱（请求先过 `validateEmail`，用户名形态的标识直接 400 `INVALID_EMAIL`）。成功后返回完整用户（含 SELF_PROFILE_POPULATE）并由 controller 签会话（`loginMethod: 'email_code'`）。来源：`register-verification.ts:723-740`、`controllers/register-verification.ts:82-90`；测试 `login-with-code.test.ts:279-287`。

### 重置密码

- LOGIN-008 `POST /api/auth/send-reset-code`（CODE-012）+ `POST /api/auth/reset-password`。提交时先按 `email 或 username` 找账号；找不到报 400 `RESET_USER_NOT_FOUND`「验证码无效或已过期」（**通用化文案**，不暴露邮箱未注册）；然后按账号真实 email 走验证码状态机。来源：`register-verification.ts:574-625`。
- LOGIN-009 重置成功在 `withAccountLock` 内执行：`requireActiveAccount` → 重校验 `current.email === targetEmail`（不一致报 409 `ACCOUNT_CHANGED`「账号信息已变更，请重新验证」）→ CAS 消费验证码 → `user.edit({password, provider:'local'})`（provider 归位使 hasPassword 为真）→ `revokeAll(userId)`（踢掉全部会话）。返回 `{success: true}`，**不**自动登录。来源：`register-verification.ts:627-641`。
- LOGIN-010 路径冲突：插件原生 `POST /auth/reset-password`（resetPasswordToken 流程）与本站自定义 `POST /api/auth/reset-password`（邮箱验证码流程）同路径同方法，实际生效者依赖 Strapi 路由注册顺序。前端（`app/composables/useApi.ts:852`）发送 `{email, code, password}` 指向自定义流程。**「待确认」**路由优先级；新 API 应只保留验证码重置一条路径。

### 修改密码

- LOGIN-011 `POST /api/auth/change-password`（需登录）：校验 currentPassword 正确、新旧不同、两次输入一致、新密码 ≤72 字节；成功后包装层 `revokeAll` 再签新会话（`loginMethod: 'password_change'`）——即旧设备全部下线，仅当前会话存活。来源：`plugin-users-permissions/controllers/auth.js:196-241`、`strapi-server.ts:52-60`。

### 遗留 renew 端点

- LOGIN-012 `POST /api/auth/renew`（auth 开启）：优先按 refresh cookie 旋转（等价 /auth/session/refresh）；无 cookie 时仅当 env `AUTH_LEGACY_RENEW_ENABLED === 'true'` 才允许用**无 sid 的**旧 JWT 换取新会话（带 sid 的一律 401——防止已撤销家族借尸还魂）。这是显式的临时迁移门。**新设计下不需要**（无不透明 token 之前的遗留 JWT），但「旧凭证不得绕过会话撤销」的意图必须保留。来源：`controllers/register-verification.ts:92-106`。

---

## 5. 会话（SESS）

> 旧模型：DB 持久化的 refresh token 家族 + 15 分钟短期 JWT 访问令牌。新设计（不透明 token）下，JWT 验签/sid 机制「新设计下不需要」，但旋转、重放检测、家族撤销、设备列表等规则全部保留。

### 数据与约束

- SESS-001 `auth_sessions` 字段：`userId`(int, required)、`deviceId`(string ≤128, required, 缺省 randomUUID)、`refreshTokenHash`(string, private, required)、`tokenFamily`(string, private, required)、`userAgent`(string ≤255, private)、`ip`(string ≤45)、`location`(string)、`loginMethod`(enum: `password | mihoyo | email_code | password_change | password_reset | oauth`)、`loginAt`(datetime)、`ipHash`(string, private)、`expiresAt`(required)、`lastSeenAt`、`revokedAt`。来源：`auth-session/schema.json`。
- SESS-002 refresh token = `crypto.randomBytes(48).toString('base64url')`（384 bit），DB 只存 `sha256(token)` hex；`tokenFamily` = `crypto.randomUUID()`。唯一索引 `uq_auth_sessions_refresh_hash(refresh_token_hash)`；索引 `(user_id, revoked_at, expires_at)`、`(token_family)`、`(expires_at)`。来源：`auth-session/services/auth-session.ts:13-15,44-48`、`database/migrations/2026.09.12...:112-116`。
- SESS-003 会话绝对有效期 **30 天**（`30 * 86400000` ms）；**旋转不续期**——新行继承旧的 `expiresAt`，30 天到期必须重新登录。来源：`auth-session.ts:44,141`。
- SESS-004 每用户最多 100 个活跃会话（`MAX_SESSIONS = 100`）：新建前按 `lastSeenAt desc, id desc` 取第 100 名之后的溢出者批量撤销并逐个广播实时撤销。测试口径：建 101 个后第一个家族失活、list 恰为 100。来源：`auth-session.ts:11,68-79,84`、测试 `auth-session-lifecycle.test.ts:462-467`。
- SESS-005 所有家族级变更（建、旋转、撤销）都在 `locked(userId)` 内执行：事务 + `SELECT ... FOR UPDATE` 锁 `up_users` 行，消灭「消费旧 token 与插入后继之间」的并发窗口。来源：`auth-session.ts:32-37`。
- SESS-006 建会话前置检查 `activeUser`：`!blocked && (!accountStatus || accountStatus==='active')`，否则抛 403 `ACCOUNT_BLOCKED`。旋转时同样重查（命中旋转缓存前也重查）；账号失活时**顺手撤销整个家族**再返回 `{error:'ACCOUNT_BLOCKED'}`。来源：`auth-session.ts:38-43,81-87,114-119`。
- SESS-007 会话公开投影（设备列表/登录响应里的 `session`）：`id, deviceId, userAgent, ip, location, loginMethod, loginAt, lastSeenAt, expiresAt, createdAt, isCurrent`；`refreshTokenHash/tokenFamily/ipHash` 永不外发。`location` 缺失时按 `ip` 现场解析。来源：`auth-session.ts:12,16-22`。
- SESS-008 旋转产生的新行继承旧行的 `deviceId/userAgent/ip/location/loginMethod/loginAt`（`insert(userId, row, family, oldExpiresAt)`）：旋转不是新登录事件，`loginAt` 保留首登时刻。来源：`auth-session.ts:53-55,140-141`；测试 `auth-session-lifecycle.test.ts:398-413`。

### 旋转与重放检测

- SESS-009 `rotate(refreshToken)`：空 token 或哈希查不到 → `{error:'INVALID_REFRESH_TOKEN'}`；过期 → `'REFRESH_TOKEN_EXPIRED'`；然后在家族锁内处理（SESS-010~013）。错误均以 `{error}` 形式返回（不是 throw），以便撤销等副作用能随事务提交。来源：`auth-session.ts:108-117,137-138`。
- SESS-010 **30 秒旋转窗口（并发/重试容错）**：旋转成功后把 `{token: 新refreshToken}` 写 Redis `auth:session:rotation:<sha256(旧token)>`，TTL 30s。30s 内再次拿同一旧 token 来旋转：缓存命中且后继会话仍活跃 → 直接返回同一新 token（同一 session 结果）；缓存未命中但行已被撤销且撤销时间 < 30s → 返回 `{error:'REFRESH_TOKEN_ROTATED'}`（controller 对此**不清 cookie**，区别于其他错误）。缓存里只存新 token 一个字段，绝不缓存用户数据/密钥。来源：`auth-session.ts:111,120-135,143`、`controllers/auth-session.ts:5-13`；测试 `auth-session-lifecycle.test.ts:443-461,564-604`。
- SESS-011 **重放检测**：行已撤销且撤销时间 ≥ 30s → 视为凭证泄露，撤销**整个家族**（cascade）并返回 `{error:'REFRESH_TOKEN_REPLAY'}`（return 而非 throw，保证撤销提交）。来源：`auth-session.ts:131-139`；测试 `auth-session-lifecycle.test.ts:737-799`。
- SESS-012 正常旋转：旧行置 `revokedAt=now`，同家族插入新行（继承 expiresAt，见 SESS-003/008），写 30s 旋转缓存。来源：`auth-session.ts:140-144`。
- SESS-013 `isActive(userId, family)` 以 DB 为权威：`tokenFamily` 匹配且 `revokedAt is null` 且 `expiresAt > now`；Redis 里的阳性缓存一概不采信（DEL 失败/竞态无法保证）。测试：Redis 残留活跃标记但 DB 已过期/撤销时 isActive=false。来源：`auth-session.ts:172-181`；测试 `auth-session-lifecycle.test.ts:454-461,722-735`。

### 撤销与设备管理

- SESS-014 `revoke(refreshToken)`（登出）：按哈希找行，锁内撤销整个家族；查不到返回 false（controller 仍回 `{ok:true}` 并清 cookie——幂等登出）。来源：`auth-session.ts:147-153`、`controllers/auth-session.ts:14-19`。
- SESS-015 `revokeAll(userId, exceptFamily?)`：锁内批量 `revokedAt=now`（可排除当前家族），提交后广播实时撤销。来源：`auth-session.ts:162-171`。
- SESS-016 `revokeSession(id, userId)`：锁内按 id+userId 找行（防止撤他人会话），撤销其家族，返回 `{ok, tokenFamily}`；行不存在返回 false → controller 404。来源：`auth-session.ts:154-161`、`controllers/auth-session.ts:34-42`。
- SESS-017 设备列表 `GET /api/auth/sessions`：活跃会话按 `lastSeenAt desc, id desc` 最多 100 条；若当前家族不在其中（被截断）则单独查出并 unshift 到首位（满 100 先 pop 末尾）。来源：`auth-session.ts:182-191`。
- SESS-018 设备撤销端点 `DELETE /api/auth/sessions/:id`：`id='all'` 撤全部并清本端 cookie；`id='others'|'other'` 保留当前家族撤其余；数字 id 撤单个（若撤的恰是自己的家族则清本端 cookie）。`list`/`revoke` 要求 `ctx.state.tokenFamily`（即 V2 会话令牌），否则 401 `SESSION_UPGRADE_REQUIRED`——**旧式无 sid JWT 不能查看/管理设备**。来源：`controllers/auth-session.ts:20-44`；测试 `auth-session-lifecycle.test.ts:486-494`。

### QR 票据的确定性会话

- SESS-019 `createForTicket(userId, ticket)`（米游社扫码登录专用）：`family = sha256("mihoyo:<userId>:<ticket>")`，`refreshToken = HMAC-SHA384(jwtSecret, "qr-session:<family>")`（base64url）——**同一 ticket 永远兑换出同一个 token**。已有同 (userId, family, hash) 行：活跃则原样返回（并发轮询/断线重连兑换到同一会话）；已撤销或已过期则抛 409 `QR_SESSION_CONSUMED`（已消费票据永远不能另起新会话）。jwtSecret 或 ticket 缺失抛 `QR_SESSION_CONFIGURATION_INVALID`。来源：`auth-session.ts:88-107`；测试 `auth-session-lifecycle.test.ts:478-484`。**「待确认」**：新设计的密钥来源（旧用 JWT_SECRET 作 HMAC key）。

### Cookie 与访问令牌（「新设计下不需要」项的安全意图）

- SESS-020 refresh token 只进 HttpOnly Cookie `refresh_token`，**不进响应体**：`httpOnly: true, overwrite: true, path: '/'`，`secure` 与 `sameSite` 生产环境为 `true`/`'none'`、开发为 `false`/`'lax'`，`expires` 对齐会话 expiresAt；清除时写空串 + `expires: new Date(0)`。测试断言响应体无 refresh token。来源：`src/utils/auth-session-http.ts:15-21`；测试 `users-permissions-extension.test.ts:345-362`。
- SESS-021 访问令牌：15 分钟 JWT，payload `{id, sid: tokenFamily}`；verify 被包装——带 `sid` 的令牌每次校验都查 `isActive(id, sid)`，失活抛 `SESSION_REVOKED`；不带 sid 的旧令牌不查库直接放行（但有 SESS-018 等能力阉割）。「新设计下不需要」JWT 本身，但「每次请求都能以 O(1) 判定会话已撤销」是硬需求。来源：`src/utils/auth-session-http.ts:24-26`、`src/extensions/users-permissions/strapi-server.ts:16-28`；测试 `users-permissions-extension.test.ts:325-344`。
- SESS-022 旧插件配置 `jwt.expiresIn: '30d'`（插件默认值）只对无 sid 遗留令牌有意义；新签发的访问令牌一律 15m。来源：`plugin-users-permissions/config.js:15`、`auth-session-http.ts:25`。

### 清理 cron

- SESS-023 会话清理 cron 每 5 分钟（`*/5 * * * *`，tz Asia/Shanghai）：删除 `expiresAt < now - 7天` 的行，每批最多 1000 条。7 天保留期是为了让已过期家族的前代 token 仍可触发重放检测（SESS-011）。来源：`config/cron-tasks.ts:550-555`、`auth-session.ts:192-198`；测试 `auth-session-lifecycle.test.ts:469-476`。

### 地理位置

- SESS-024 IP 地理位置完全离线解析（本地 ip2region 库，登录 IP 永不外发给地理服务）：先过滤非公网地址（v4：0/10/127/≥224、100.64-127、169.254、172.16-31、192.168、192.0.0/192.0.2、198.18/19、198.51.100、203.0.113；v4-mapped `::ffff:` 前缀剥离；v6 只收 `2xxx:`/`3xxx:` 全球单播且排除 `2001:db8:`），非公网返回 null。结果取 `国家(中国则略)/省/市`，过滤 `'0','-','未知'` 并去重，以 `，` 连接；查不到或为中国的空结果时回落 `'中国'`/null。进程内 LRU 缓存 1000 条。解析失败绝不阻断登录。来源：`src/utils/session-location.ts:1-54`；测试 `session-location.test.ts`。

### 实时撤销广播

- SESS-025 实时连接（SSE 通知、DM WebSocket）注册 `registerRealtimeConnection(identity, onClose)`：Redis pub/sub 频道 `ik:auth:realtime-revoke`，事件 `{userId, sid?} / {userId, exceptSid?}`——按用户撤单个会话、撤全部、撤其余。发布先本进程 dispatch 再 publish；Redis 故障不致命，因为**每 30s 还有一次权威 DB 复检**（`isRealtimeIdentityActive`）。来源：`src/utils/auth-revocation.ts:11-47,57-66`。
- SESS-026 连接硬上限：绝对寿命 ≤ min(令牌 exp, 30 分钟)；每次复检设 10s 超时，复检挂起/失败一律**fail-closed 关连接**；心跳不能续期。`isRealtimeIdentityActive` 拒绝：`blocked`、`confirmed === false`、`accountStatus ∈ {blocked, merged, deleted}`、sid 家族失活。来源：`auth-revocation.ts:12-13,50-97`；测试 `realtime-revocation.test.ts:35-55`。
- SESS-027 撤销广播时机：`revokeFamily`/`revokeAll`/`makeRoom` 用 `runAfterCommit` 注册，只在**最外层事务真正提交后**执行（回滚不广播）；回调异常自吞，不影响主事务。来源：`auth-session.ts:65,76-78,168`、`src/utils/after-commit.ts:29-65`。

### 前端配套行为（UX 意图，供新前端参考）

- SESS-028 前端用 `navigator.locks('ik:auth-cookie')` + 进程内 promise 队列串行化所有改写共享 refresh cookie 的请求；access token 存 localStorage（`ik:auth:has-session` 仅作「可能有会话」提示，服务端只认 HttpOnly cookie）；登出先写 `ik:auth:logged-out=pending` 保证刷新页面后仍完成登出；token 临近过期才调 refresh。来源：`app/utils/auth-session.ts:1-22`、`app/stores/auth.ts:91-221`。

---

## 6. 登录尝试状态机（LOCK）

> 这是「扫码登录尝试」的通用状态机（当前仅米游社用，schema 预留 email provider）。与「登录失败锁定」无关——**旧系统没有密码错误次数锁定账号的机制**，防爆破靠 RL-001 的 per-IP 限流。

### 数据与约束

- LOCK-001 `auth_login_attempts` 字段：`provider`(enum `email|mihoyo`)、`mode`(enum `login|bind` 默认 login)、`opaqueStateHash`(string ≤128, private)、`pollSecretHash`(string ≤128, private)、`status`（10 态枚举，见 LOCK-003）、`user`(manyToOne)、`expiresAt`、`errorCode`(≤128, private)、`completedAt`。唯一索引 `uq_auth_login_attempts_state(opaque_state_hash)`，索引 `(status, expires_at)`。来源：`auth-login-attempt/schema.json`、`2026.09.12...:117-118`。
- LOCK-002 创建：`attemptId = randomUUID()`，`pollSecret = randomBytes(32).base64url`；DB 只存 `sha256(attemptId)` 与 `sha256(pollSecret)`；初始 `status='created'`，TTL 180s。返回值含明文 attemptId/pollSecret/expiresAt（仅此一次可见）。来源：`auth-login-attempt/services/auth-login-attempt.ts:8-18`。
- LOCK-003 状态机（10 态：`created, scanned, confirmed, identity_resolved, account_created, account_linked, session_issued, failed, expired, cancelled`），合法迁移：
  - `created → scanned | failed | cancelled | expired`
  - `scanned → confirmed | failed | cancelled`
  - `confirmed → identity_resolved | account_created | account_linked | failed`
  - `identity_resolved → account_created | account_linked | session_issued | failed`
  - `account_created → session_issued | failed`；`account_linked → session_issued | failed`
  - `session_issued / failed / expired / cancelled` 为终态；同态重复迁移幂等放行；非法迁移静默忽略（返回原行）。`session_issued` 与 `failed` 迁移顺手写 `completedAt`。来源：`auth-login-attempt/services/auth-login-attempt.ts:36-44`。
- LOCK-004 读取/迁移需同时提供 attemptId + pollSecret：先按哈希找行，`expiresAt` 已过且非终态则顺手置 `expired` 并视为不存在；pollSecret 用 `crypto.timingSafeEqual` 比对（长度不等直接拒）。找不到一律 401「Invalid or expired login attempt」，对外只暴露 `{status, expiresAt, errorCode}`。来源：`auth-login-attempt/services/auth-login-attempt.ts:20-31`、`controllers/auth-login-attempt.ts:12-17`。
- LOCK-005 `POST /api/auth/login-attempt` 创建（`global::optional-auth`，`mode='bind'` 要求已登录）；`POST /api/auth/login-attempt/status` 查询。当前服务端没有任何代码消费/推进此状态机（米游社流程使用自己的 Redis 键，见 MHY），属于预置能力。attempts 表同样**无清理 cron**。来源：`auth-login-attempt/routes/auth-login-attempt.ts`、`controllers/auth-login-attempt.ts:4-11`。**「待确认」**：新系统是把米游社流程迁到此表，还是放弃该表。

---

## 7. 米游社扫码登录 / 绑定 / 解绑（MHY）

### 外部 API 协议（完整步骤）

所有上游调用由后端代理，前端永不接触米游社 Cookie；上游请求超时 10s；统一 `MihoyoUpstreamError`（status 502, code `MIHOYO_UPSTREAM_ERROR`，由 error-format 中间件格式化）；https agent keepAlive（maxSockets 100 / maxFreeSockets 10）。来源：`src/utils/mihoyo.ts:1-24,51-62,179-186`。

常量：`QR_APP_ID='bll8iq97cem8'`；base `https://passport-api.miyoushe.com` 与 `https://api-takumi.mihoyo.com`；`ZZZ_GAME_BIZ='nap_cn'`（绝区零国服）；`QR_CLIENT_TYPE='4'`；`QR_DEVICE_NAME='绳网论坛'`；兜底 UA `Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36`。来源：`mihoyo.ts:13,20-36`。

公共请求头（passportHeaders；deviceName/deviceModel/deviceOs 需 encodeURIComponent）：`x-rpc-app_id, x-rpc-device_id, x-rpc-client_type, x-rpc-device_name, x-rpc-device_model, x-rpc-device_os, User-Agent(= deviceInfo.userAgent), Referer: https://user.mihoyo.com/, Origin: https://user.mihoyo.com, Content-Type: application/json`。来源：`mihoyo.ts:164-177`。

DeviceInfo 构造：`deviceName` 固定 `绳网论坛`；`deviceModel` = 浏览器名+版本（从发起请求的客户端 UA 解析，依次匹配 Edge/Opera/Chrome/Chromium/Firefox/Safari，未知则 `Unknown Browser`）；`deviceOs` 从 UA 括号内容识别 iOS/Android/Windows 10/8.1/8/7/macOS/Linux，兜底 `Web`；`userAgent` = 客户端 UA 或兜底 UA。来源：`mihoyo.ts:115-162`。

**协议步骤：**

1. MHY-P1 创建二维码：`POST {PASSPORT}/account/ma-cn-passport/web/createQRLogin`，body `{}`，公共头。要求 `retcode===0` 且 `data.url` 与 `data.ticket` 均存在，否则抛上游错误。返回 `{url, ticket, expiresAt}`，`expiresAt` 从 url 的 `expire=<unix秒>` 查询参数解析（解析失败为 null）。来源：`mihoyo.ts:189-211,78-83`。
2. MHY-P2 前端把 `url` 渲染为二维码；后端用 `ticket` 轮询。
3. MHY-P3 查询状态：`POST {PASSPORT}/account/ma-cn-passport/web/queryQRLoginStatus`，body `{ticket}`，公共头（**device_id 必须与创建时一致**）。`retcode=-3501 → Expired`；`retcode=-3505 → Cancelled`；其他非 0 或缺 `data.status` 抛上游错误。`data.status ∈ Created|Scanned|Confirmed|Expired|Cancelled`；`Confirmed` 时必须带 `data.user_info.aid`（缺失抛错），`mid` 可空；同时从响应 `Set-Cookie` 解析登录 Cookie（ltoken_v2 等）。来源：`mihoyo.ts:226-267`。
4. MHY-P4 拉游戏角色（仅 login/bind 模式）：`GET {TAKUMI}/binding/api/getUserGameRolesByCookie?game_biz=nap_cn`，头 `Cookie: k=v; ...`（Confirmed 响应的 Set-Cookie 拼成）、固定 UA、`Referer: https://user.mihoyo.com/`。`retcode!==0` 抛上游错误；`data.list` 非数组时按空列表。来源：`mihoyo.ts:273-299`。
5. MHY-P5 主角色选择：优先 `is_chosen=true`，否则按 `level` 降序取第一。落库字段：`game_uid→zzzUid, nickname→zzzNickname, level→zzzLevel, region→zzzRegion, region_name→zzzRegionName`，并写 `lastSyncedAt=now`。来源：`mihoyo.ts:301-307`、`mihoyo-auth.ts:395-404`。
6. MHY-P6 米游社 Cookie 只在 Confirmed 后的单次请求链路内使用，**不落库、不写日志**。来源：`mihoyo.ts:7-8`。

### 数据与约束

- MHY-001 `mihoyo_bindings`：`user`(oneToOne)、`aid`(string, required, **unique**)、`mid`(string, private)、`zzzUid/zzzNickname/zzzLevel(int)/zzzRegion/zzzRegionName/lastSyncedAt`。公开视图只含 `aid, zzzUid, zzzNickname, zzzLevel, zzzRegion, zzzRegionName, lastSyncedAt`（mid 不外发）。来源：`mihoyo-binding/schema.json`、`mihoyo-auth.ts:120-131`。
- MHY-002 Redis 键（TTL 单位秒）：会话 `mihoyo:qr:<ticket>`（TTL = `max(180, 上游真实expiresIn + 30)`）；状态微缓存 `mihoyo:qr:status:<ticket>`（TTL 2s，不缓存 confirmed）；确认互斥 `mihoyo:qr:claim:<ticket>`（TTL 60s，`SET NX`）；结果 `mihoyo:qr:result:<ticket>`（成功 TTL 300s，失败 TTL 60s）。来源：`mihoyo-auth.ts:31-46,88-91,146-161,237-258`。
- MHY-003 二维码有效期提示：米游社二维码实测约 120s，比本站会话短；返回给前端的 `expiresIn` 取 URL 上的真实 expire 值，解析不出才用 180。来源：`mihoyo-auth.ts:144-150`；测试 `mihoyo-auth-service.test.ts:98-126`。

### 端点与流程

- MHY-004 `POST /api/auth/mihoyo/qr`（optional-auth）：`body.mode` 显式 `login|bind|delete`；`bind`/`delete` 要求已登录否则 401 `MIHOYO_BIND_REQUIRES_AUTH`「该操作需要先登录」；`mode='login'` 时**即使带有效 token 也忽略**（避免前端拿到 bind 结果无法完成登录）；不传 mode 时退回旧行为（有 userId 即 bind）。来源：`controllers/mihoyo-auth.ts:43-83`。
- MHY-005 `POST /api/auth/mihoyo/qr/status`（optional-auth）：`{ticket}`；`bind`/`delete` 会话要求 `currentUserId === session.userId`，否则 403 `MIHOYO_BIND_USER_MISMATCH`「扫码会话与当前登录用户不一致」。轮询返回 `status: waiting|scanned|expired|cancelled` 或 confirmed 结果。来源：`mihoyo-auth.ts:170-203,219-234`。
- MHY-006 轮询顺序（防退化设计）：**先查结果缓存**（重放，见 MHY-008）→ 会话不存在但 claim 标记在 → 返回 `scanned`（另一路正在处理，让前端继续等）→ 会话与结果都没有 → `expired` → 状态微缓存（2s，confirmed 不缓存）→ 查上游。Expired/Cancelled 删除会话并写微缓存。来源：`mihoyo-auth.ts:175-234`。
- MHY-007 Confirmed 处理互斥：`SET mihoyo:qr:claim:<ticket> 1 EX 60 NX`，抢不到返回 `scanned`（等结果）；抢到后删除会话键再处理。保证并发轮询只有一路建号/绑定。来源：`mihoyo-auth.ts:236-240`；测试 `mihoyo-auth-service.test.ts:191-211`。
- MHY-008 结果可重放：Confirmed 的处理结果（outcome，仅标量：mode/userId/isNewUser/binding 视图/matched/失败三元组）写 `mihoyo:qr:result:<ticket>`；后续轮询重放同一结果——login 模式重放时重新读用户行并**重新检查封禁**（blocked 或 accountStatus∈{blocked,merged,deleted} 则 403 `USER_BLOCKED`「该账号已被封禁」；用户行消失则 500 `MIHOYO_LOGIN_USER_MISSING`）；bind/delete 重放仍校验归属用户。处理失败也写失败结果（60s），让前端看到真实错误而不是「二维码已过期」。来源：`mihoyo-auth.ts:241-260,329-378`。
- MHY-009 login 模式确认后由 controller 调 `createForTicket`（SESS-019）签发确定性会话并返回 `{status:'confirmed', mode:'login', isNewUser, binding, ...session响应}`。来源：`controllers/mihoyo-auth.ts:86-115`。

### 登录（建号/解析）

- MHY-010 `loginWithMihoyo(aid, mid, zzzRole)`：aid/mid 先 trim；事务内 `resolveIdentity('mihoyo', {aid, mid})` 命中用户 → 锁内 `requireActiveAccount` + 刷新绑定（同步 ZZZ 资料）；未命中 → `getRegistrationRole`（**注册开关关闭时拒绝建新号**，403 REGISTER_DISABLED）→ 创建用户：`username = generateUsername(zzzNickname)`、`email = mihoyo_<aid||mid>@mihoyo-login.inter-knot.invalid`、`password = randomBytes(24).base64url`（随机，无人能登）、`provider='mihoyo'`、`confirmed=true`、`accountStatus='active'`、`level=0`、`examPassed=false`、注册默认角色 → `bindToUser` → `isNewUser=true`。全部在同一事务。来源：`mihoyo-auth.ts:407-431`。
- MHY-011 建号并发兜底：唯一键冲突（`23505 | ER_DUP_ENTRY | SQLITE_CONSTRAINT | IDENTITY_ALREADY_BOUND` 或消息含 unique constraint/duplicate entry）时回滚后重新 resolveIdentity，命中赢家则以赢家身份重走登录一次（不递归第二次）——并发首登绝不产生第二个账号。来源：`mihoyo-auth.ts:432-443`。
- MHY-012 用户名生成：优先 ZZZ 昵称（trim，截 20 字符，长度 ≥2 才用），冲突时依次尝试 5 个 `昵称#<4位数字>` 变体（总长仍 ≤20），再 20 个 `绳网用户<4位数字>`，全部冲突兜底 `绳网用户<8位数字>`（这里的随机后缀是**纯数字**，区别于邮箱注册的字母数字混合）。来源：`mihoyo-auth.ts:47,112-118,546-568`。

### 绑定 / 解绑

- MHY-013 `bindToUser`（锁内 + requireActiveAccount）：resolveIdentity 命中**其他**用户 → 409 `IDENTITY_ALREADY_BOUND`「该米游社账号已绑定其他绳网账号，请先在原账号解绑或注销」（**无任何「接管空壳号」逻辑**，占用方是空壳/已过考试/已绑真邮箱/被封禁一律拒绝）；当前账号已绑**另一个**米游社账号（binding 的 aid/mid 都不同，或存在多余 identity 行）→ 409 `USER_ALREADY_BOUND`「当前账号已绑定其他米游社账号，请先解绑」。通过则 `ensureMihoyoIdentity`（幂等）+ upsert `mihoyo_bindings`（`aid: cleanAid || cleanMid`）并同步 ZZZ 资料。来源：`mihoyo-auth.ts:445-471`；测试 `mihoyo-auth-service.test.ts:513-594`。
- MHY-014 解绑 `DELETE /api/auth/mihoyo/binding`（require-auth）：锁内；**无真实邮箱（占位邮箱）的账号禁止解绑**——403 `MIHOYO_UNBIND_FORBIDDEN`「当前账号只能通过米游社扫码登录，请先绑定邮箱」（防止锁死账号）；未绑定 → 404 `MIHOYO_NOT_BOUND`「当前账号未绑定米游社」；通过则删 binding 行 + `unlinkIdentity(userId,'mihoyo')`。注意：只要求有真实邮箱，**不要求先设密码**（可用重置密码找回）。来源：`mihoyo-auth.ts:473-486`；测试 `mihoyo-auth-service.test.ts:596-625`。
- MHY-015 `GET /api/auth/mihoyo/binding`（require-auth）返回公开视图（无 binding 时 null）。来源：`mihoyo-auth.ts:488-493`。

### 扫码注销核验（delete 模式）

- MHY-016 delete 模式 Confirmed 只解析身份归属：`matched = (resolveIdentity(aid,mid).userId === session.userId)`，**绝不建号、不改数据、不拉角色**。来源：`mihoyo-auth.ts:273-289`。
- MHY-017 `verifyDeleteTicket(userId, ticket)`（注销前置，只读不消费）：结果键不存在 → 400 `MIHOYO_DELETE_TICKET_EXPIRED`「扫码验证已过期，请重新扫码」；非 delete 模式 → 400 `MIHOYO_DELETE_TICKET_INVALID`；归属不符 → 403 `MIHOYO_BIND_USER_MISMATCH`；`matched=false` → 403 `MIHOYO_DELETE_ACCOUNT_MISMATCH`「扫码的米游社账号与当前账号不一致，请使用当前账号绑定的米游社扫码」。300s 结果 TTL 即「扫码后完成注销」的有效窗口。来源：`mihoyo-auth.ts:495-531`。
- MHY-018 `discardDeleteTicket(ticket)`：注销**事务成功后**才 best-effort 删除结果键；删键失败由 300s TTL 兜底，且 accountStatus='deleted' 会阻断任何重放。来源：`mihoyo-auth.ts:533-544`。

### 限流

- MHY-019 QR 生成：每 IP 每 60s 最多 30 次（`mihoyo:qr:create:rl:<ip>`）；轮询：每 IP 每 60s 最多 600 次（`mihoyo:qr:poll:rl:<ip>`）；超限 429「QR 生成过于频繁，请稍后再试」/「轮询过于频繁，请稍后再试」。计数用 `incrWithTtl`（自增与补 TTL 原子完成）；**Redis 故障时 fail-open 放行**（避免 Redis 挂掉导致登录不可用）。来源：`controllers/mihoyo-auth.ts:9-29,50-53,92-95`。

### 前端轮询节奏（UX 意图）

- MHY-020 前端轮询：waiting 最小间隔 1500ms、scanned 后 3000ms、页面不可见 5000ms、错误退避上限 5000ms、连续失败 120s 判定登录失败；前端校验返回 `mode` 与请求一致，不匹配即报错。来源：`app/composables/useMihoyoQr.ts:35-44,80-82,184-227`。

---

## 8. 身份模型（IDENT）

### 数据与约束

- IDENT-001 `user_identities` 字段：`user`(manyToOne)、`provider`(enum `email|mihoyo`)、`providerSubject`(≤255, required)、`providerAccountId`(≤255)、`providerAlias`(≤255, private)、`verifiedAt`、`lastLoginAt`、`metadata`(json, private)。唯一索引 `(provider, provider_subject)` 与 `(provider, provider_account_id)`；普通索引 `(provider, provider_alias)`。**刻意没有** `(user, provider)` 唯一约束：「一个账号最多一个米游社身份」是 `bindToUser` 的业务规则而非 schema 约束（旧实现里 user 关系走联表也建不了；且做成唯一键会导致「改邮箱后无法登录」类问题）。来源：`user-identity/schema.json`、`database/migrations/2026.09.12...:95-111`。
- IDENT-002 规范化：通用 `String(v).trim()`；email 身份再 `.toLowerCase()`。email 身份的 subject = 规范化邮箱；mihoyo 身份 `providerSubject = aid || mid`、`providerAccountId = aid`、`providerAlias = mid`（aid 是米游社 canonical subject，mid 是历史 alias）。来源：`src/utils/identity-service.ts:37-42,183-189`。
- IDENT-003 `identity_conflicts` 运维表（自建，非 content-type）：`provider, provider_subject, provider_account_id, provider_alias, reason(≤64), details(text), resolved_by, resolved_at, created_at, updated_at`，索引 `(provider, reason)`；`down` 迁移**不回滚**它与 `account_status`（审计数据优先）。来源：`2026.09.12...:56-74,125-129`。

### 解析规则

- IDENT-004 `resolveIdentity('email', ...)`：subject 查不到时回退按 `up_users.email`（**$eqi 大小写不敏感**）找账号；identity 行存在但没有所属用户 → 500 `IDENTITY_DATA_CORRUPTION`「登录身份缺少所属账号」；email 回退命中多个账号 → 409 `IDENTITY_CONFLICT`「邮箱地址关联了多个账号」。来源：`identity-service.ts:93-106,117-133`。
- IDENT-005 `resolveIdentity('mihoyo', ...)`：aid 查 subject、mid 查 alias，**并总是同时检查遗留 `mihoyo_bindings`**（aid 与 mid 各查一次，按 user 去重）；aid、mid、遗留绑定三者指向的 user 集合 >1 → 409 `IDENTITY_CONFLICT`「米游社身份标识指向多个账号」（details 带 aid/mid/userIds）；遗留绑定缺所属用户 → 500 `IDENTITY_DATA_CORRUPTION`「米游社绑定缺少所属账号」。aid/mid 均空 → 502 `IDENTITY_INPUT_INVALID`「米游社身份标识为空」。**绝不把「查不到」当成「该建号」之外的任何中间态**。来源：`identity-service.ts:66-91,112-193`。
- IDENT-006 `ensureIdentityForUser`：先 resolve，命中他人 → 409 `IDENTITY_ALREADY_BOUND`「该登录身份已绑定其他账号」；命中本人 → 幂等返回。未命中则在事务内**再查一次**后创建（`verifiedAt` 缺省 now）；唯一键竞态冲突时回滚后重读——赢家是本人则返回赢家行，是他人则 409。来源：`identity-service.ts:208-254`。
- IDENT-007 `linkIdentity`（service 层）包 `withAccountLock` + `requireActiveAccount`；`unlinkIdentity(userId, provider)` 按 provider+user 删全部身份行（「先查 id 再按 id 删」避免关系过滤歧义）。`resolveIdentity` service 包装可在命中时更新 `lastLoginAt`。来源：`user-identity/services/user-identity.ts:16-48`。
- IDENT-008 身份写入时机：邮箱注册（verifiedAt=now，REG-003）、密码/重置/改密登录成功后的回填（无 verifiedAt，LOGIN-004）、绑定邮箱（先 unlink 再 link，CODE 流程）、米游社绑定/登录（ensureMihoyoIdentity）、注销（unlink email + mihoyo）。来源：见各章。

### 迁移脚本（scripts/migrate-account-identities.js）

- IDENT-009 审计维度：重复 aid（指向多 user）、重复 mid、重复邮箱（规范化后）、孤儿绑定（无 user）、空壳号清单（占位邮箱）；audit 默认只读（独立只读连接，需显式 `AUDIT_DATABASE_URL`），`--apply` 幂等。来源：`scripts/migrate-account-identities.js:60-99,226-255`。
- IDENT-010 backfill 规则：占位邮箱用户**不**生成 email 身份（占位邮箱表示米游社登录而非独立账号）；`accountStatus ∈ {deleted, merged}` 的用户不生成 email 身份；重复邮箱/aid 一律**记入 identity_conflicts 并跳过**（reason: `ORPHAN_BINDING | DUPLICATE_SUBJECT | IDENTITY_CONFLICT`），绝不把登录任意指派给某个账号；已存在身份归属不符也记冲突。来源：`scripts/migrate-account-identities.js:139-224`。
- IDENT-011 迁移的幂等 documentId：`identity_<sha256(provider:subject).slice(0,40)>`；并发建身份时读回已存在行并校验归属。来源：`scripts/migrate-account-identities.js:30-33,127-137`。

---

## 9. 账号安全端点与注销（DEL）

### 端点（全部 `global::require-auth` + `assertAuthOrigin`）

- DEL-001 `GET /api/me/security` 返回 `{email, provider, hasBoundEmail, hasPassword}`：占位邮箱显示为空串且 `hasBoundEmail=false`；`provider` 只区分 `'mihoyo'|'local'`；`hasPassword = (provider === 'local')`（米游社注册号是随机密码，自设密码后 provider 归 local，见 LOGIN-009）。来源：`register-verification.ts:800-822`、`me-security/controllers/me-security.ts:8-14`。
- DEL-002 改邮箱（绑定新邮箱）：`POST /api/me/email/send-code` + `PUT /api/me/email`。发码前：新邮箱与当前相同 → 400 `EMAIL_UNCHANGED`「该邮箱已绑定当前账号」；被他人占用（email 或 username 列，排除自己）→ 409 `EMAIL_ALREADY_TAKEN`「该邮箱已被其他账号使用」。确认绑定在锁内：requireActiveAccount → CAS 消费验证码 → unlink 旧 email 身份 → link 新 email 身份（verifiedAt=now）→ `user.edit({email, confirmed:true})`。返回最新 security payload。来源：`register-verification.ts:824-979`。
- DEL-003 发注销码 `POST /api/me/delete-account/send-code`：已注销 → 400 `ACCOUNT_ALREADY_DELETED`「账号已注销」；无真实邮箱（占位邮箱）→ 400 `DELETE_ACCOUNT_NO_EMAIL`「当前账号未绑定邮箱，请通过重新扫码米游社账号注销」。只发到**自己已绑定**的邮箱。来源：`register-verification.ts:985-1057`。

### 注销流程

- DEL-004 `POST /api/me/delete-account`：核验方式按「是否有真实邮箱」分流（不看 provider）：有 → body.code 走 delete-account 验证码（`assertDeleteAccountCode` **锁外只校验不消费**）；无 → body.ticket 走扫码核验（verifyDeleteTicket，MHY-017）。已注销重复调用 → 400 `ACCOUNT_ALREADY_DELETED`。来源：`register-verification.ts:1096-1126`。
- DEL-005 锁内重校验（TOCTOU 防护）：`requireActiveAccount` 后比对 `password`、`provider`、`email` 与锁外一致，任一变化 → 409 `ACCOUNT_CHANGED`「账号信息已变更，请重新验证」（email 纳入是因为验证码绑定到该邮箱）。邮箱路径在锁内 CAS 消费验证码（失败回滚整笔注销，凭证复原可重试）。来源：`register-verification.ts:1132-1152`。
- DEL-006 注销写入（同一事务）：unlink mihoyo + email 身份 → 删 `mihoyo_bindings` 行 → 用户脱敏：`email = deleted_<userId>_<epochMs>@anonymized.local`、`username = 已注销用户_<userId>`、`password = randomBytes(32).hex`（**绕过哈希生命周期直接写库**，任何 bcrypt 比对都会失败，等效不可用）、`accountStatus='deleted'`、`accountCleanupPending=true`、`blocked=true`、`confirmed=false`、`resetPasswordToken=null`、`confirmationToken=null`、`equippedAvatar=null`、`equippedCard=null`、`profileHidden=true` → 关联 author 脱敏：`name='已注销用户'`、`avatar=null`、`bio=null`、`profilePinnedArticles=null` → `revokeAll(userId)`（撤销失败会回滚整笔注销与凭证释放）。来源：`register-verification.ts:1153-1194`。
- DEL-007 扫码凭证在事务提交后才丢弃（Redis 不在 DB 事务内，提前删键会让「注销回滚」的用户被迫重新扫码）；controller 随后清 refresh cookie。`cleanupAccountPrivacy` 在锁外 best-effort 调用，失败仅 warn（由 pending 标记 + cron 兜底）。来源：`register-verification.ts:1196-1204`、`me-security/controllers/me-security.ts:59-67`。
- DEL-008 公开数据清理 `cleanupAccountPrivacy`：Meilisearch 文章/评论索引中该用户的 `authorName` 分批（500/批，fetch+部分更新+轮询 task 至 succeeded，10s 超时）改为「已注销用户」（**只改作者名，历史正文保留**；索引不存在跳过）；同时清缓存中间件里 `/api/articles|/api/comments|/api/authors` 的缓存（正则清除）；两者都成功才把 `accountCleanupPending` 置 false。搜索故障**不阻断**缓存清理（Promise.all 内各自独立，但两者都完成才清标记）。来源：`src/utils/account-privacy.ts:4-14`、`src/utils/meilisearch.ts:42-84`。
- DEL-009 重试 cron 每分钟（`* * * * *`，Asia/Shanghai）：取 `accountStatus='deleted' AND accountCleanupPending=true` 最多 20 个账号逐个重跑清理；单个失败不影响其余。来源：`config/cron-tasks.ts:556-559`、`src/utils/account-privacy.ts:16-24`。
- DEL-010 注销**不删除**用户产生的内容（文章/评论/点赞/收藏/私信/关注等均保留），只做作者身份脱敏与账号凭证销毁；无「冷静期/宽限期」，一经确认立即生效且不可撤回（邮件文案亦明示「此操作将不可撤回」）。**「待确认」**：新系统是否引入宽限期属产品决策，旧系统没有。

---

## 10. 限流与防刷（RL）

- RL-001 认证路由限流（users-permissions 插件 ratelimit，已修正分桶）：**每 IP 每条认证路由 20 次/分钟**（interval 1min, max 20）；key = `up-auth:<routePath小写去尾斜杠>:<ip>`，IP 用 TRUSTED_PROXY_HOPS 算法（见 RL-004）；覆盖插件自带认证路由（/auth/local、/auth/local/register、/auth/forgot-password、/auth/reset-password、/auth/change-password、/auth/email-confirmation、/auth/send-email-confirmation、/connect/*）。key 刻意**不含** body 里的 identifier/email（否则攻击者变换该字段即得无限配额，GHSA-7mqx-wwh4-f9fw）。自定义 `/api/auth/*` 端点（验证码系列）不在插件中间件覆盖内，由全局中间件（RL-002）+ 验证码自身冷却（CODE-007）兜底。**新设计下**：应对所有认证端点统一施加等价的 per-IP per-route 20/min。来源：`config/plugins.ts:244-305`。
- RL-002 全局限流中间件（固定窗口 Redis，`rl:<r|w>:<identity>`，60s 窗口）：默认档 `匿名读 60 / 匿名写 20 / 登录读 120 / 登录写 40`，生产配置放宽为 `300/60/600/120`；身份 = 登录用户 `u:<id>` 否则 `ip:<clientIp>`（登录判定 best-effort 解析 Bearer，解析失败按匿名 fail-open）；只作用于 `/api/`（`/api/agent/*` 除 `/api/agent/characters` 外豁免）；响应头 `X-RateLimit-Limit/-Remaining`，超限 429 `{error:{code:'TOO_MANY_REQUESTS', message:'Rate limit exceeded. Please slow down.'}}` + `Retry-After`。**已知取舍**：固定窗口边界最坏放行 2×limit，可接受；Redis 故障 fail-open。来源：`src/middlewares/rate-limit.ts:1-149`、`config/middlewares.ts`（rate-limit 段）。
- RL-003 米游社 QR 专项限流见 MHY-019。验证码发送冷却与尝试上限见 CODE-003/CODE-007/CODE-010。错误响应统一由 `global::error-format` 包成 `{error:{status, code, message, details?}}`，内部错误只给 `INTERNAL_SERVER_ERROR`/`SERVICE_UNAVAILABLE` + requestId，不泄露细节。来源：`src/middlewares/error-format.ts:34-122`。
- RL-004 客户端 IP 解析：取 `X-Forwarded-For` **从右往左第 `TRUSTED_PROXY_HOPS` 跳**（默认 1，nginx=1，CDN+nginx=2），hops≤0 或无 XFF 时用 socket 对端地址；parts 少于 hops 时取最左。绝不信任最左值（客户端可伪造），也绝不用 `ctx.request.ip`（反代后恒为 127.0.0.1，曾导致全站塌成一个限流桶）。限流、会话 IP、验证码 requestIp 全部使用此函数。来源：`src/utils/common.ts:486-518`、`config/server.ts:8-16`。
- RL-005 Origin 校验 `assertAuthOrigin`：所有签发/撤销凭证的端点（登录、注册、验证码、QR、会话、安全端点）——`Origin` 存在且不等于请求自身 origin 且不在 CORS 白名单，或无 Origin 但 `Sec-Fetch-Site: cross-site` → 403 `AUTH_ORIGIN_REJECTED`。白名单即 CORS origin 列表（tiwat.cn、interk.net 等 + localhost 开发端口）。来源：`src/utils/auth-session-http.ts:7-14`、`config/middlewares.ts`（cors 段）。

---

## 11. 认证中间件（AUTH-MW）

- MW-001 `global::optional-auth`：用于 `auth:false` 的公共路由——有 Bearer 就验签并填充 `ctx.state.user`（最小字段，ACC-014），无则匿名放行；`blocked` 或 `accountStatus ∈ {blocked, merged, deleted}` 的用户即使 token 有效也按匿名处理（**不检查 `confirmed`**）。`rejectInvalidBearer: true` 严格模式下：形如 Bearer 的头但验签失败（`Invalid token.`/`SESSION_REVOKED`）→ 401「Invalid authentication token」，不得静默降级成匿名数据；DB 等基础设施错误在严格模式下抛出（500）而非伪装匿名。来源：`src/middlewares/optional-auth.ts:1-75`。
- MW-002 `global::require-auth`：用于 `auth:false` 但需要登录的路由——验签失败/无头 → 401「authentication required」；`payload.sid` 存入 `ctx.state.tokenFamily`（供 SESS-018）；blocked/merged/deleted 用户不放行。来源：`src/middlewares/require-auth.ts:17-50`。
- MW-003 封禁判定三处一致（optional-auth、require-auth、实时复检）：`blocked === true` 或 `accountStatus ∈ {blocked, merged, deleted}`；注意 Strapi 内置鉴权只查 `blocked`，这些中间件是补齐 `accountStatus` 的防线，新系统必须在**每一个**认证入口做同等检查。来源：MW-001/MW-002、`auth-revocation.ts:57-66`。
- MW-004 插件会话模式保持 `jwtManagement: 'legacy-support'`（默认）；限流中间件里对 refresh 模式的 access token 有内存校验分支（validateAccessToken），当前不生效。 「新设计下不需要」。来源：`plugin-users-permissions/config.js`、`rate-limit.ts:67-90`。
- MW-005 HTTP 层细节：注册成功必须 201（controller 注释：双参 send 才会写状态码）；body 限制 30mb；CORS `credentials: true`。来源：`controllers/register-verification.ts:41-42`、`config/middlewares.ts`。

---

## 12. 邮件发送基础设施（EMAIL）

- EMAIL-001 双通道调度：按收件人域名分类——国内（domestic）主通道阿里企业邮箱 SMTP、备用 Brevo HTTP API；国际（含未知域名）主通道 Brevo、备用阿里。主通道失败且 `EMAIL_FALLBACK_ENABLED`（默认 true）时自动切备用；双通道都失败抛错（调用方的 `*_EMAIL_FAILED` 处理，CODE-006）。来源：`src/utils/email/index.ts:29-78`。
- EMAIL-002 国内域名名单（26 个，小写精确匹配，含 qq.com/vip.qq.com/foxmail.com/163.com/vip.163.com/126.com/yeah.net/sina.com/sina.cn/vip.sina.com/sohu.com/aliyun.com/139.com/189.cn/wo.cn/tom.com/21cn.com/sina.com.cn/263.net/x263.net/etang.com/eyou.com/56.com/chinaren.com/188.com/vip.188.com）；env `EMAIL_DOMESTIC_DOMAINS`（逗号分隔）可追加；域名取最后一个 `@` 之后并小写；格式非法/未知一律 international（Brevo 全球可达，误投风险更低）。来源：`src/utils/email/domains.ts:9-80`。
- EMAIL-003 降级事件计数：Redis `ik:email:fallback:<provider>:<YYYY-MM-DD>`，TTL 7 天（只在 key 无 TTL 时补），fire-and-forget 不阻塞发送。来源：`src/utils/email/index.ts:80-97`。
- EMAIL-004 日志脱敏：邮箱在日志中掩码（`ab***@domain`，短地址 `a***@domain`）；绝不记录验证码本身。来源：`src/utils/email/index.ts:99-105`。
- EMAIL-005 阿里通道：nodemailer 连接池（pool, maxConnections 3, maxMessages 100, connectionTimeout 10s, greetingTimeout 10s, socketTimeout 15s），env `SMTP_HOST/PORT(465)/SECURE(true)/USER/PASS/DEFAULT_FROM/DEFAULT_REPLYTO`。Brevo 通道：`POST {BREVO_API_URL=https://api.brevo.com/v3}/smtp/email`，头 `api-key`，body `{sender:{name?,email}, to:[{email}], subject, htmlContent, textContent, replyTo?}`，超时 `BREVO_TIMEOUT_MS`(10s)，from 支持 `"Name" <email>` 解析。来源：`src/utils/email/{config.ts,senders/ali-smtp.ts,senders/brevo-api.ts}`。
- EMAIL-006 Strapi 自带 email 插件配置（nodemailer SMTP）与双通道调度器**并存**，但验证码邮件只走调度器；新系统只需保留调度器语义。来源：`config/plugins.ts:212-230`。

---

## 测试中体现的关键用例

1. **验证码原子性**（`verification-atomicity.test.ts`）：同秒双发取最新一条；并发错误猜测总数不超上限；legacy `attempts=NULL` 原子归一化且不死循环；已消费码不会被迟到的错误猜测重开；并发用同一注册码只成一个账号一条身份；身份创建失败时账号创建与验证码消费整体回滚；归一化后重复的遗留邮箱**拒绝迁移且不改任何账号**。
2. **会话生命周期**（`auth-session-lifecycle.test.ts`）：旋转继承 loginMethod/loginAt/location；旋转缓存命中仍重查账号状态且缓存只含 token；Redis 残留标记不能复活已过期会话；第 101 个会话挤掉最旧者；前代 token 保留至过期+7 天；同一 ticket 并发兑换只得一个会话、撤销后兑换 409；旧 JWT 无 sid 不能列/撤设备；30s 内并发旋转返回同一新 token、缓存未命中回 `REFRESH_TOKEN_ROTATED` 且不清 cookie；>30s 重放触发家族级联撤销；过期 refresh 报 `REFRESH_TOKEN_EXPIRED`；blocked 用户持有效 token 被 require-auth 拒绝；revoked sid 验签抛 SESSION_REVOKED；撤单个/others/all/登出的 cookie 行为矩阵。
3. **验证码登录**（`login-with-code.test.ts`）：未注册/占位邮箱/封禁/注销账号在发码与登录两侧都与「验证码错误」不可区分；attempts 递增与上限；消费后不可重放；冷却期重复请求与未知邮箱同响应。
4. **身份服务**（`identity-service.test.ts`、`mihoyo-auth-service.test.ts`）：aid/mid 双标识解析到同一身份；指向不同账号即 IDENTITY_CONFLICT 拒绝登录（防串绑）；aid 未命中但 mid 命中遗留绑定时不误建临时账号；孤儿绑定报 IDENTITY_DATA_CORRUPTION 不自动接管；占用米游社账号的任何形态（空壳/已绑邮箱/已封禁/已过考试）都拒绝接管。
5. **users-permissions 扩展**（`users-permissions-extension.test.ts`）：控制器必须「工厂包工厂」；凭据失败不建会话不回填身份；改密/重置先 revokeAll 再签新会话；登录不踢会话；跨站 Origin 拦截；refresh token 只在 HttpOnly cookie；登录方式按本次手段记录。
6. **米游社 QR**（`mihoyo-auth-service.test.ts`、`mihoyo.test.ts`）：confirmed 结果可重放（中断不退化为 expired）；重放时复查封禁；失败结果短缓存重放真实错误；并发轮询只一路建号；会话已消费但结果未写完回 scanned；retcode -3501/-3505 映射 Expired/Cancelled；Confirmed 缺 user_info 抛错；expiresIn 取 URL 真实 expire。
7. **注销**（`account-deletion.test.ts`、`account-privacy.test.ts`）：锁外校验锁内消费的完整矩阵（无码/错码/过期/超限/并发抢占回滚）；扫码核验非本人拒绝；注销回滚不丢扫码凭证可重试；锁内邮箱被改则 ACCOUNT_CHANGED；重复注销 400；占位邮箱拒发注销码；搜索脱敏分批、索引缺失跳过、失败保留重试标记、cron 单账号失败不影响批次。
8. **实时撤销**（`realtime-revocation.test.ts`）：跨传输撤单个会话保留其他家族；Redis pubsub 跨进程广播；pubsub 漏事件时 30s DB 复检兜底关连接；复检挂起 fail-closed；心跳不能续期；无 exp 的 claims 拒绝。
9. **中间件与限流**（`optional-auth.test.ts`、`common.test.ts`）：严格模式 Bearer 无效即 401；非严格保留匿名回退；getClientIp 右数第 N 跳、hops=0 回退 socket、无 XFF 回退 socket。
10. **邮件**（`email-dispatcher.test.ts`、`email-domains.test.ts`）：域名路由、主备降级、降级禁用开关、双失败抛错、日志掩码、多 `@` 取最后、未知域名归国际。

---

## 未决问题（「待确认」）

1. `POST /api/auth/reset-password` 与插件原生同路径路由的运行时优先级（前端只发自定义流程的 `{email,code,payload}`，见 LOGIN-010）。新 API 只保留一条验证码重置路径即可规避。
2. `register_verifications` 与 `auth_login_attempts` 两表在旧系统无任何清理任务，长期增长；新系统应定义保留期（建议对齐会话的过期+7 天）。见 CODE-018 / LOCK-005。
3. `auth_login_attempts` 状态机当前无任何服务端代码推进（米游社流程走自己的 Redis 键）。新系统需决定：迁移米游社流程到该表，或删除该模型。见 LOCK-005。
4. 会话 `lastSeenAt` 仅在创建/旋转时更新，「活跃会话上限挤掉最旧者」实际挤的是最久未旋转的家族而非最久未活跃的设备；新设计若改成按请求刷新 lastSeenAt 需重新评估写放大。见 SESS-004。
5. 用户名与邮箱互撞的交叉检查（ACC-009）在**改名**接口不存在（改名只查 username 列）；是历史疏漏还是有意为之待确认，新系统建议统一交叉检查。
6. 旧系统无密码复杂度规则（仅长度 6–72 字节）、无密码错误账号锁定、无登录历史/异地提醒；新系统如需增强属新增需求而非对齐项。见 ACC-011 / LOCK 章首注。
7. `confirmed` 语义混用：注册/绑邮箱后置 true，注销置 false；optional-auth/require-auth 不查 confirmed，实时复检查。新系统应以 `accountStatus` 为唯一权威，待确认是否保留 confirmed 字段。
8. 占位邮箱方案（ACC-013）依赖「email 列非空」的 schema 约束（email required）；新系统若允许 email 为 NULL，需要重新设计「无邮箱账号」的表示并同步所有 `isMihoyoPlaceholderEmail` 判定点的语义。
9. QR 确定性会话的 HMAC 密钥旧用 `JWT_SECRET`（SESS-019）；新系统的等价密钥来源待确认。
10. 注销时未清理的数据面（文章/评论正文、私信、关注、收藏、点赞、经验/丁尼流水等）是否满足目标合规要求，待产品/法务确认。见 DEL-010。
