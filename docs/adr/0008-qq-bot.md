# ADR 0008：QQ 官方机器人——把站内 AI 角色搬到 QQ

- 状态：已接受（2026-10-26），第 1–3 期已实施（与方案的出入见第 18 节）
- 涉及：新 crate `ik-qqbot`（第 9 层）、`ik-ai`（新增渠道无关的「一问一答」入口、额度与记忆的小改动）、
  `ik-server`（路由、任务、配置装配）、管理后台（「QQ 机器人」页）、主站 InterKnot-Web（绑定入口，另一个仓库）
- 关联：ADR 0004（工具箱）、ADR 0005（EdgeOne 直接回源）、ADR 0007（AI 记忆）、`docs/business-rules/ai-assistant.md`

> 本文是交给执行者（人或 AI）的实施方案。**动手前先读**：`CLAUDE.md`、`docs/architecture.md`（依赖方向、事务纪律）、
> ADR 0004、ADR 0005、ADR 0007、`docs/business-rules/ai-assistant.md` 第 5 节（QUOTA）与第 13 节（MEM）、`deploy/README.md`。
> 第 2 节的 QQ 开放平台事实已于 2026-10-25 对照官方文档和腾讯官方 SDK 源码核实过（来源见第 17 节）。
> 还剩少数几项标了 **【沙箱实测】**，是文档之间有出入或文档没写的地方，第 0 期在沙箱群实测后写回本文。

## 1 需求口径

| # | 需求 | 本方案的理解 |
|---|---|---|
| R1 | QQ 官方机器人，把站内 AI agent 搬到 QQ，和群友对话 | 接 QQ 开放平台的官方机器人（不是 NapCat / go-cqhttp 之类的协议端）；群里 @机器人、单聊（C2C）都能聊；回复由站内同一套角色卡、模型、工具、记忆生成 |
| R2 | 没绑定绳网，每日只能对话 10 次 | 「游客」：按 QQ 身份每个配额日 10 次（北京时间 04:00 切换，与 QUOTA-004 一致），只计成功的回复；没有任何记忆 |
| R3 | 绑定后按用户个人额度算 | 绑定后与站内私信共用同一个每日加权 token 额度（`ai_quota_counters`，QUOTA-001～020），不再有 10 次的限制 |
| R4 | 绑定后可以使用记忆 | 读写的是站内**同一份**长期记忆（`ai_memories`，按 用户 × 角色）：QQ 里说过的事，站内私信里角色也记得，反之亦然 |
| R5 | 共享记忆 | 两层含义都做：① 个人记忆在站内与 QQ 之间共享（R4）；② **群共享记忆**——每个 QQ 群 × 角色一份「这个群的事」（群梗、群规、群里发生过的事），群里所有绑定用户的对话共同沉淀、在这个群里共用 |

如果 R5 只指 ①，第三期（群共享记忆）可以不做，其余不受影响。

## 2 QQ 开放平台的约束（决定了架构）

每行的「依据」见第 17 节的来源编号。

| 约束 | 已核实的内容 | 依据 | 对设计的影响 |
|---|---|---|---|
| 收群消息 | 默认只收 @机器人 的消息（`GROUP_AT_MESSAGE_CREATE`）。2026 年 6 月起**群主**可在机器人设置里改成「接收全部群消息」或「获取 @ 时最近 10 条消息」，开启后**连 @ 消息也改走 `GROUP_MESSAGE_CREATE`**（不新增 intent），机器人自己的发言也会回推（`author.bot = true`）；`mentions[].is_you = true` 表示 @ 的是本机器人 | [2][7][8][9] | 两种事件都要收；用 `is_you`（兜底：content 里的 `<@!{appId}>` / `<@{appId}>`）判断是否 @ 了本机器人；`author.bot` 的消息一律丢弃。全量模式下非 @ 消息只**存作群上下文**、不触发回复 |
| 收单聊消息 | `C2C_MESSAGE_CREATE`，`author.user_openid` | [7] | — |
| 事件字段 | 群：`id`、`content`、`timestamp`、`group_openid`、`author{member_openid, username, bot}`、`mentions[]`、`attachments[]`、`message_scene`、`msg_elements`；单聊：`author{user_openid}`，其余同 | [7] | **群事件现在带昵称**（`author.username`），群上下文可以用群昵称 |
| 事件投递 | Webhook：HTTPS，端口只能 80/443/8080/8443；请求头 `X-Bot-Appid`、`User-Agent: QQBot-Callback`；签名头 `X-Signature-Ed25519`（hex，64 字节）与 `X-Signature-Timestamp`；验签消息 = **`timestamp` + 原始 body**；种子 = AppSecret 重复拼接到 ≥32 字节后取前 32 字节；验签通过后回 `{"op":12,"d":0}`。平台会重推同一事件（同一 `msg_id` 可能推多次），处理慢了会超时重推 | [3][4][5][7] | 用 Webhook，不用 WebSocket：服务端多实例、无长连接；`ik.tiwat.cn` 走 EdgeOne 443 正好满足。必须先 ACK 再异步处理，并按消息 id 去重 |
| 回调地址校验 `op=13` | 请求体 `{"op":13,"d":{"plain_token","event_ts"}}`，**不带签名头**（官方 SDK 也不验签）；回应 `{"plain_token", "signature"}`，签名内容 = `event_ts + plain_token`，hex | [3][4][7] | **安全隐患，见 13 节**：照搬会变成「替任何人签任意内容」的签名机，可以拿来伪造事件推送 |
| 发消息 | `POST /v2/groups/{group_openid}/messages`、`POST /v2/users/{openid}/messages`；`msg_type` 0 文本、2 markdown、7 富媒体（单聊另有 6 = 「输入中」提示）；`msg_id` 引用被回复的用户消息（被动回复）；`msg_seq` 同一 `msg_id` 下递增，相同的 `msg_id + msg_seq` 再发会失败；单条文本约 5000 字上限；OpenAPI 域名 `https://api.sgroup.qq.com`（沙箱 `https://sandbox.api.sgroup.qq.com`）；鉴权头 `Authorization: QQBot <access_token>` | [1][2][7][11] | 只做被动回复，每条带触发消息的 `msg_id`，分段用 `msg_seq` 1、2、3… |
| 被动回复窗口 | **群：5 分钟内，每条消息最多回 5 次；单聊：60 分钟内，最多回 4 次** | [1][2] | 群里生成总预算 ≤ 90 秒，排队加发送留足余量；分段数群 ≤ 4、单聊 ≤ 3（各留一次给出错提示）；超窗就放弃，不补发 |
| 主动消息 | 2025-04-21 起取消；之后部分恢复：**群**要群主在机器人设置里打开「机器人主动在群聊内发言」（默认关）；**单聊**有「互动召回」（`is_wakeup`）：用户先发过消息后，按当天、1–3 天、3–7 天、7–30 天四个周期各可发 1 条；用户可以关掉「允许主动发送」 | [1][2][8][10] | 第一期不用主动消息；「到期待跟进」只在对方下次来聊时提起。互动召回可作第四期的「单聊主动关心」 |
| Access Token | `POST https://bots.qq.com/app/getAppAccessToken`（官方 SDK 用这个；官方文档写的是 `https://api.bot.qq.com/app/getAppAccessToken`），体 `{appId, clientSecret}`，回 `access_token`、`expires_in`（7200 秒）；**出错时 HTTP 仍是 200，要看响应体的 `code`**；有效期内重复获取返回同一个 token，到期前 60 秒内获取才换新，旧的在这 60 秒里仍有效 | [5][7] | 地址做成配置；进程内缓存，到期前 5 分钟起尝试刷新（拿到的还是同一个也没关系）；多实例各取各的不会互相挤掉 |
| IP 白名单 | 新注册的机器人在正式环境必须在开放平台登记调用方的出口 IP，否则报「接口访问源 IP 不在白名单」 | [5][8] | 部署文档补一步：登记服务器出口 IP |
| 用户身份 | **同一个机器人，同一个人在不同群里的 `member_openid` 不同**，单聊的 `user_openid` 也是另一个；`union_openid` 要向平台单独申请关联应用才有，群消息事件里不带 | [6][7] | 身份按（场景、群、openid）建，绑定按身份绑：**用户在每个群、单聊各要绑一次**（4.2）。`union_openid` 不作为第一期依赖 |
| 消息里的链接 | 应用允许 HTTP/HTTPS 外链；具体域名是否被 QQ 接受仍需群聊、单聊分别验收，拒绝时保留平台错误码 | [2][12][13] | 默认 `link_mode=allow`；保留 strip/domain 开关。明确 URL 拒绝且不存在不确定发送记录时去链接重试一次，保持当前格式 |
| Markdown / 按钮 | 官方文档明确：2026-04-23 起群聊、单聊均支持自定义 Markdown，所有机器人可用，无需申请模板 | [13] | 群聊、单聊默认使用 `msg_type=2`、`markdown.content`；指令与提示仍为纯文本。按钮、图片上传、流式回复不在本期范围 |
| 单聊增强 | 单聊有「输入中」提示（`msg_type 6`）和流式消息（`/v2/users/{openid}/stream_messages`） | [7] | 第四期可选：单聊里先发「输入中」，或流式输出 |
| 沙箱 | 有沙箱环境与沙箱群；沙箱群里主动能力默认开、不限单条 `msg_id` 的回复次数 | [8][11] | 配置项 `sandbox`。**沙箱不限回复次数，验收 5 次 / 4 次的上限要在正式环境测** |
| 群成员身份（群主 / 管理员） | 社区帖称群消息里带群内身份，但官方 SDK 的类型定义里没有这个字段【沙箱实测字段名】 | [7][8] | 第一期群里切角色只走管理后台；实测有字段后再开放给群主 |

## 3 总体架构

```mermaid
flowchart LR
  qq[QQ 开放平台] -->|Webhook POST /v1/qq/webhook| eo[EdgeOne ik.tiwat.cn]
  eo --> srv[ik-server: ik-qqbot 路由]
  srv -->|验签、去重、落库、入队（一个事务）| pg[(PostgreSQL)]
  pg -->|graphile_worker: qq.reply| job[ik-qqbot 回复任务]
  job -->|ik_ai::external::reply| ai[ik-ai 生成核心：角色卡 / 工具箱 / 记忆 / 额度]
  ai --> llm[大模型 API]
  job -->|被动回复 msg_id + msg_seq| api[api.sgroup.qq.com]
```

### 3.1 crate 划分

- **新 crate `crates/qqbot`（`ik-qqbot`，第 9 层）**，依赖 `auth`、`ai`、`notify`（绑定成功给站内发系统通知）。
  负责：Webhook 验签与事件解析、身份与绑定、游客计次、群设置、QQ 消息表、OpenAPI 客户端（access token、发消息）、
  QQ 指令、回复任务、管理后台接口、`/v1/me/qq/*` 用户接口。
- **`ik-ai` 新增 `external` 模块**（不依赖 qqbot，保持依赖方向）：渠道无关的「一问一答」生成入口，
  复用 `generate::generate`、`Toolbox`、`memory`、`quota`、`sanitize`、`immersion`。将来接 Discord / 微信也走它。
- `docs/architecture.md` 的层表加一行 `qqbot | auth, ai, notify`，并补一段说明（同其他域的写法）。
- `ik-server`：合并路由、登记任务与定时任务、装配配置。

### 3.2 一条消息的流程

1. **Webhook（`POST /v1/qq/webhook`）**：
   - 按 `X-Bot-Appid` 找到配置里的机器人，找不到 401。读原始 body（上限 64 KiB），**在解析 JSON 之前**用该机器人的公钥
     验 `X-Signature-Ed25519`（消息 = `X-Signature-Timestamp` + 原始 body）；缺头或验签失败 401、不落库。
     不做时间戳偏差检查（官方没规定时间戳格式），重放靠下面的去重挡住。
   - `op=13`（不带签名）：按 13 节的限制处理，回 `{plain_token, signature}`。
   - `op=0`：
     - `GROUP_AT_MESSAGE_CREATE` / `GROUP_MESSAGE_CREATE` / `C2C_MESSAGE_CREATE`：丢弃 `author.bot = true` 的；
       群消息判断是否 @ 了本机器人（`mentions[].is_you`，兜底看 content 里的 `<@!{appId}>` / `<@{appId}>`）。
       **一个事务**里 upsert 身份、写 `qq_messages`（入站，按 `(app_id, qq_msg_id)` 唯一，重推直接忽略）；
       单聊、或群里 @ 了本机器人的，再以 `qq:<app_id>:<消息 id>` 为执行键写 `qq_turns` 并入队 `qq.reply`；
       没 @ 的群消息（群主开了全量接收）只存作上下文。
     - `GROUP_ADD_ROBOT` / `GROUP_DEL_ROBOT` / `FRIEND_ADD` / `FRIEND_DEL` 等：只维护群和身份的状态。其他事件忽略。
   - 回 `{"op":12,"d":0}`。**不能在请求里调模型**：处理慢了平台会超时重推。
   - 不走 CSRF（服务端请求不带 `Origin` / `Sec-Fetch-Site`，tower-http 的 `CsrfLayer` 放行，**写测试确认**）；
     不受按 IP 的通用限流（来源都是 QQ 的几个出口 IP），改为按 AppID 单独限流（默认 50 次/秒）。
2. **回复任务 `qq.reply`**（graphile_worker，最多 2 次尝试；超出被动回复窗口直接放弃）：
   - 准备（短事务）：锁 turn 行、复核（群 / 身份未被封、机器人在群里仍启用）、处理指令（见第 7 节）、
     决定角色卡、判定游客 / 绑定用户、**计次或预占额度**、组装上下文与记忆。
   - 事务外：`ik_ai::external::reply(...)` 生成（不推流，同评论回复）。
   - 定稿（短事务）：写出站 `qq_messages`、结算额度、记 turn 结果、需要时排记忆提炼任务。
   - 事务外：调 QQ OpenAPI 发送（每段一个 `msg_seq`）。发送失败按错误码分类：token 过期刷新后重试一次；
     被动窗口过期 / 内容违规不重试，记 `reason`。**发送结果不回滚已结算的额度**（模型钱已经花了，与 QUOTA-019 一致）。
3. 所有外部 IO（LLM、QQ OpenAPI）都在事务之外（`architecture.md` 规则 4）。

## 4 身份与绑定

### 4.1 身份

`qq_identities`：一行一个（AppID、场景、群、openid）。场景 `group`（群成员，openid 是 `member_openid`，同时关联群）
或 `c2c`（单聊，`user_openid`）。同一个人在不同群里的 `member_openid` 不同（官方「唯一身份机制」），所以**一个人在每个群都是一个独立身份**。
群身份顺带记下最近一次看到的群昵称（`author.username`，只用于群上下文与后台展示）。

### 4.2 绑定流程（站内发码，QQ 里用码）

1. 用户在主站登录后打开「设置 → 绑定 QQ」，调 `POST /v1/me/qq/bind-codes`，拿到一个 8 位码（去掉易混字符的大写字母 + 数字），
   有效 10 分钟、只能用一次，库里只存 HMAC（`IK_SECRETS__CODE_HMAC_KEY`，与验证码同一把钥匙）。同一用户同时最多 3 个有效码。
2. 用户在 QQ 里对机器人发 `/绑定 ABCD2345`（单聊或在群里 @机器人 都行）。
3. 回复任务识别指令，在事务里消费这个码，把**发消息的那个身份**绑到发码的用户。
4. QQ 里回「绑定成功」（**群里不说绳网用户名**，避免把 QQ 号与站内账号公开关联起来）；站内给该用户发一条系统通知
   「你的绳网账号绑定了一个 QQ 身份（单聊 / 某群），不是你本人操作请到设置里解绑」。

为什么这个方向：码由已登录的站内账号生成，用户自己的消息最先把它用掉；即使在群里发、被人看到，也已经失效。
反方向（QQ 里出码、站内填）会让旁观者把**别人的** QQ 身份绑到自己账号上，看到对方以后在 QQ 里的对话被记进自己的记忆。

规则：

- 一个身份只能绑一个绳网账号；一个账号可以绑多个身份（不同群、单聊）。已绑定的身份再用码：回「这个 QQ 已经绑定了绳网账号，先发 /解绑」。
- 用户可在 QQ 发 `/解绑`（解绑当前身份），也可在站内 `GET /v1/me/qq/bindings` 查看、`DELETE /v1/me/qq/bindings/{id}` 解绑。
  站内列表只显示场景、群的展示名（管理员在后台填，没有就显示「QQ 群 #后四位」）、绑定时间、最后使用时间，不显示 openid 全文。
- 绳网账号被封禁、注销、`accountStatus` 不正常（同 TRIG-023）时，该账号的身份**按「已封禁」处理**：不回复，不降级成游客
  （否则封号后还能白嫖 10 次）。注销时级联删除绑定。
- **用户在每个群、单聊各要绑一次**（每次在站内生成一个新码）：同一个人在不同群里的 openid 本来就不同，这是平台的设计，不做绕过。
  站内绑定页写清楚「在哪个群用机器人，就在哪个群发一次 /绑定」；一个码只能用一次，所以同时最多 3 个有效码够用。
- `union_openid`（跨群统一身份）要向平台申请关联应用，消息事件里也不带，第一期不依赖它。以后申请到了，可以加一个
  「同 `union_openid` 的身份自动归到同一账号」的功能（第四期）。

## 5 额度

### 5.1 游客（未绑定）

- **每个身份每个配额日 10 次**（`qq_settings.guest_daily_turns`，后台可改，0 = 禁止游客使用）。
  计次在准备阶段原子 `+1`（`qq_guest_usage` 的行锁），生成失败、跳过、被动窗口过期都在定稿时退回这 1 次；成功发出才算。
- 再加两道保护（防一个群的人刷光全站额度）：
  - **每个群每天游客最多 N 次**（`guest_group_daily_turns`，默认 100）；
  - **全部游客每天合计的加权 token 上限**（`guest_daily_tokens`，默认 2,000,000，0 = 不限），单独计数。
- 游客的 token 同时计入**全站计数**（`user_id = 0`）：全站每日上限（`dailyTokens`）对游客同样生效。
- 游客用的模型：`qq_settings.guest_model_key`（可选，便宜的模型）；为空用角色卡默认模型。
- 用完时回一条短提示（这条不算次数）：「今天的免费次数用完了（每天 10 次，04:00 重置）。在绳网绑定 QQ 后按个人额度使用，还能让我记住你：<绑定说明>」。
  同一身份当天只提示一次，之后静默（防刷屏）。

### 5.2 绑定用户

- 与站内私信完全一致：`quota::reserve(user_id)` 预占 → 生成 → `quota::settle` 结算，同一个计数器、同一个上限。
  QQ 用量同样出现在站内的额度条里（不需要改读侧）。
- 额度不足：回「今日 AI 额度已用完，HH:MM 重置」（同一身份当天只提示一次）。站内私信是静默跳过，QQ 里静默会让人以为机器人坏了，所以这里提示。

### 5.3 对 `ik-ai` 额度代码的改动

- 游客的预占登记 `user_id = SITE(0)`。**`quota::reserve` / `settle` / `reclaim_expired` 现在都会对 `SITE` 行和 `hold.user_id` 行各加一次**，
  `user_id = 0` 时同一行会被加两次。新增 `quota::reserve_site_only`，并让 `settle`、`reclaim_expired` 在 `user_id == SITE` 时只改一次；
  补单元测试（游客预占 → 回收，全站计数回到 0）。
- 游客 token 上限的计数放在 `qq_guest_tokens(day, used)`，在同一个准备事务里、在全站行之后加锁（顺序：全站 → 游客合计 → 群 → 身份），
  写进代码注释，避免死锁。

## 6 生成

### 6.1 `ik_ai::external::reply`

```rust
// crates/ai/src/external.rs（示意）
pub struct ExternalTurn<'a> {
    pub channel: &'static str,          // "qq"，进审计与日志
    pub card_slug: &'a str,             // 角色卡
    pub quota: Payer,                   // Payer::User(user_id) | Payer::Guest { hold_id ... }（预占已在调用方事务里做完）
    pub viewer: Option<i64>,            // 论坛工具的查看者：None = 游客视角（TOOL-004）
    pub history: Vec<ExternalLine>,     // 已按时间正序、已假名化的上下文
    pub memory: Option<ExternalMemory>, // None = 不读不写记忆
    pub images: Vec<String>,            // 第二期：用户发的图片地址（识图模型才带）
    pub max_chars: usize,               // QQ 默认 4000
    pub output_mode: OutputMode,        // QQ 设置的 plain / markdown，独立于角色卡站内格式
}
pub struct ExternalReply { pub segments: Vec<String>, pub usage: Usage, pub model_key: String, pub outcome: RunOutcome }
```

- 内部复用 `dm::system_prompt`（角色人格、示例对话、日期与公共输出格式要求）、`Toolbox::for_card`、`generate::generate`（`live: None`，与评论一样不推流）、
  `sanitize` 与 `immersion` 守卫；QQ 的最终格式、分段和长度由渠道排版层处理，生成阶段不提前截断 Markdown。
- 额度：预占在调用方（qqbot）的准备事务里完成（因为要和游客计次在同一个事务），所以 `ik-ai` 对外暴露
  `quota::reserve*`、`quota::settle` 以及「这次生成的预占估算」函数（`external::estimate(...)`），结算由调用方在定稿事务里做。
- 直接使用角色卡提示词，不追加外部聊天的身份、语气、场合或链接要求；另附最终传输预算的长度提示，要求模型在自然边界收尾。群聊历史标记「当前提问者」。
  工具与记忆沿用角色卡配置及当前授权范围。QQ 的 `reply_format` 参与公共输出格式组装，最终格式、链接策略与总字数由渠道排版层处理。

### 6.2 角色卡

- 角色卡加一列 `qq_enabled BOOLEAN NOT NULL DEFAULT FALSE`（与 `dm_enabled`、`comment_enabled` 并列），后台角色卡页加开关。
- 每个机器人（AppID）有一个默认角色（`qq_bots.default_card_slug`）；每个群可以在后台覆盖（`qq_groups.card_slug`）。
- 单聊里用户可以 `/角色` 查看、`/角色 <名字>` 切换（只列 `qq_enabled` 的卡），存 `qq_identities.card_slug`。
  群里的角色只能由管理员在后台改：官方 SDK 的群消息字段里没有群主 / 管理员身份【沙箱实测字段名】，
  不开放给群成员，免得被人来回切。实测确认有这个字段后，可以开放给群主用 `/角色` 切换。

### 6.3 上下文

- **群**：这个群最近 12 条消息（`qq_messages`，2 小时内，可配），所有人的都带，按时间正序。默认只有 @机器人 的问答；
  群主开了「接收全部群消息」时，没 @ 的群聊消息也在里面（另设上限：只取最近 30 条里的，单条截 200 字），话题衔接会好很多。
  每条前缀群昵称「<昵称>：」：昵称截 16 字，去掉换行、`【】`、`<>` 等可能伪装成提示结构的字符；没有昵称时用「群友 A」按出现顺序编号。
  当前提问的人另外标出。**绝不出现绑定的绳网用户名**。机器人自己的回复按 assistant 角色。
- **单聊**：该身份与当前角色最近 20 条（同 LLM-001 的思路），12 小时内；绑定用户的单聊再加站内私信的跨会话衔接（见 8.1）。
- 用户消息里的 @ 标记（`<@!{id}>` / `<@{id}>`）去掉；@ 本机器人的判断见第 2 节。
- 重置：`/重置` 只清当前身份（单聊）或当前群（群里需要后台权限，第一期只在单聊提供）的短期上下文：
  在 `qq_identities` / `qq_groups` 上记 `context_reset_at`，之前的消息不再进上下文（不删记录），与 CTX-021 同一语义。

### 6.4 回复的 QQ 专用排版

- `qq_settings.reply_format` 为 `plain | markdown`，群聊、单聊默认 `markdown`，独立于角色卡站内输出模式。旧版后台请求省略 `replyFormat` 时保留当前值。指令、额度和敏感词提示继续使用纯文本。
- `ExternalTurn` 将输出模式带入生成快照与提示词；QQ 生成结果保留完整 Markdown，交给 QQ 层的 `pulldown-cmark` 解析、链接处理、分段与长度控制，避免提前截断链接。
- 支持一、二级标题、强调、删除线、列表、引用、分隔线和链接；更深标题归为二级。代码与表格转为可读文本，图片只保留说明，不增加图片上传或按钮。
- 链接：`qq_settings.link_mode` 默认 `allow`，允许 HTTP/HTTPS 外链；相对 `/post/<id>` 补全为 `https://interk.net/post/<id>`。Markdown 保留可点击标题，纯文本保留标题与 URL。自动链接 `<https://…>` 不按 HTML 清除，其他协议只保留说明文字。
  `domain` 仅保留规范的绳网帖子链接；`strip` 去链接，站内帖子改成「（在绳网搜《标题》）」。
- 敏感词兜底：按原文及可见正文检查 `blocked_words`，命中整条换成「这个话题我就不聊啦」，记 `reason=blocked_word`。
- 总长 `max_reply_chars` 默认 4000，计入实际发送的 Markdown 标记、URL 与换行。按结构分段、截断时优先回退到换行、句末标点等自然边界，避免把回答切在句中，并保持标记闭合；片段没有可用自然边界时按上限截断。完整链接放不下时保留标题。**群最多 4 段、单聊最多 3 段**，多余并入最后一段。群第一段不额外 @ 提问者。
- 发送类型明确区分：纯文本 `msg_type=0` + `content`；Markdown `msg_type=2` + `markdown.content`（省略顶层 `content`）；均携带原 `msg_id` 与该段 `msg_seq`。
- 分段和发送尝试持久化 format；新段摘要覆盖格式与正文，重试不按新设置重新排版，历史段默认 plain 并保留旧摘要。Markdown 明确拒绝码 `40034011`、`40034124`、`304036`、`40034127` 且无不确定发送记录时，转纯文本重试一次；URL 明确拒绝码 `304003`、`40034028`、群端点 `40054010` 同样要求无不确定记录，去链接并保持当前格式。修正先持久化并递增 payload_version，共用原序号、截止和每段 3 次 POST 上限。
- 超时、待审核、去重不触发改写；成功后仅把实际发送正文写入历史，不重复计费。后台投递详情展示每次尝试的格式与载荷版本。

上线时先暂停调度，等待 activeLeases、generating 归零并确认没有在途发送，停止旧 worker，执行新增迁移 `20261119000001_qq_markdown.sql`，启动新版本后恢复调度。迁移同时设置当前及默认 link_mode=allow，已有段和尝试按 plain 兼容；不混跑新旧发送器。沙箱群和单聊分别验证实际排版与外链；应用允许外链和平台接受链接分别验收。回退可将后台 replyFormat 改为 plain、linkMode 改为 strip，仅影响新生成消息。

### 6.5 工具

- 论坛工具：**群里一律用游客视角**（`viewer = None`），即使提问者已绑定——群是公开场合，不能按某个人的权限去读（例如他能看到而别人看不到的内容）。
  单聊里绑定用户用自己的身份（与私信一致，TOOL-004）；游客用游客视角。
- 剧情库与站外 MCP：按角色卡（ADR 0004），不变。
- 记忆工具（`remember` / `forget` / `recall_history`）：只给绑定用户（见第 8 节）。
- 生成总预算：`qq_settings.reply_timeout_secs`，默认 90 秒，**必须小于**被动回复窗口（群 5 分钟）减去排队与发送的余量；
  任务开始时如果离用户消息已超过 `窗口 − 预算`，直接放弃（`reason=window_expired`），退回计次、不预占。

## 7 指令

都以 `/` 开头，群里需要 @机器人。指令不调用模型、不计次、不计费。可以在 QQ 开放平台后台把它们登记成快捷指令，方便用户点选（可选，不影响实现）。

| 指令 | 场景 | 作用 |
|---|---|---|
| `/绑定 <码>` | 单聊、群 | 见 4.2 |
| `/解绑` | 单聊、群 | 解绑当前身份 |
| `/额度` | 单聊、群 | 游客：今天还剩几次；绑定用户：今日额度已用百分比与重置时间（同 AI-023，只给百分比）。群里只回给提问者本人看得懂的简短一句 |
| `/角色`、`/角色 <名字>` | 单聊 | 查看 / 切换角色 |
| `/重置` | 单聊 | 清当前对话上下文（不清长期记忆） |
| `/帮助` | 单聊、群 | 简介、绑定方法（主站绑定页地址按 `link_mode` 决定是否带链接） |

## 8 记忆（绑定用户才有）

### 8.1 个人记忆：与站内共用

- 读：用站内同一份 `ai_memories`（按 `user_id` × 角色的 `ai_user_id`）。复用 `memory::recall` 的拼装与 `memory::note` 的文本，
  需要把 `recall` 里依赖 `SessionRow` / 私信历史的部分（摘要、衔接、开场判定）抽成参数，QQ 传自己的历史。
- **按场合区分可见范围**（沿用 MEM-061 的思路，群是公开场合）：

| 场合 | 档案 / 关系记忆 | 事件记忆、待跟进、久别重逢 | 跨会话衔接 |
|---|---|---|---|
| 单聊（绑定） | 全部 | 有（同私信） | 带站内私信最近几轮（48 小时内，同 MEM-005） |
| 群（绑定的提问者） | `public` 的 + **来源是这个群的**（见下） | 只有来源是这个群的事件；没有待跟进、没有久别重逢 | 没有 |
| 站内评论区 | 只 `public`（不变） | 没有（不变） | 没有 |
| 站内私信 | 全部（不变，QQ 里来的记忆同样可见） | 有 | 不变 |

- `ai_memories` 加一列 `source_qq_group_id BIGINT NULL REFERENCES qq_groups(id) ON DELETE SET NULL`：
  从某个群的对话里提炼或在群里 `remember` 的记忆记上来源群。**在 A 群说的事可以在 A 群用，不会出现在 B 群**；
  私信与单聊照常可见。评论区规则不变（只 `public`）。
- 群里的记忆段说明额外加一句：「这是群聊，只在话题需要时轻描淡写地用，不要当众说出对方的私事」。

### 8.2 写入

- **`remember` / `forget`**：单聊与群里都给绑定用户；群里写入的带 `source_qq_group_id`。时间线事件不需要（QQ 没有时间线），
  `remember` 成功后回复里自然带一句即可（由模型说）。
- **后台提炼**：把 `memory_jobs::extract(state, conversation_id)` 泛化成 `extract(state, source)`：
  `Source::Conversation(id)`（现有）| `Source::Qq { user_id, ai_user_id }`。
  - 转写只取**这个绑定用户自己的发言 + 机器人回复他的话**，群里其他人的话不进他的记忆（避免把别人的事记到他头上）；
  - 进度：`qq_memory_progress(user_id, ai_user_id, until)`，只前进（同 MEM-020 的 `memory_until`）；
  - 入队：定稿时排 `ai.extract_memories_qq`，任务键 `qq:<user_id>:<ai_user_id>`，执行时间往后推 `extract_delay_secs`（默认 180 秒，同 MEM-020）；
  - 群里提炼出的新记忆带 `source_qq_group_id`；同一条记忆被单聊里再次提到时 update 成无来源群（更开放的场合覆盖更封闭的）——
    实现上：提炼时如果这批对话全部来自同一个群就带上该群，否则（有单聊）不带。
- 每晚整理、遗忘曲线、`recall_history`：整理与遗忘按 用户 × 角色 自动覆盖 QQ 来的记忆，不用改；
  `recall_history` 只在单聊提供，并扩展为也搜 `qq_messages`（该用户、该角色、单聊身份）。
- 全站记忆开关（`ai_memory_settings.enabled`）关掉时，QQ 同样不读不写。
- 计费：同 MEM 的口径，提炼不扣用户额度。

### 8.3 群共享记忆（第三期）

- 表 `qq_group_memories`：（群、角色）一份，最多 40 条，每条 1–200 字，`origin`（`extracted` / `told` / `manual`）、`pinned`、`created_by_user_id`。
- 写：
  - 绑定用户在群里说「记住：我们群周五晚上打本」→ `remember` 工具多一个参数 `scope: "me" | "group"`（只在群里提供 `group`），模型按语义选；
  - 后台提炼：每个群单独一个任务（`qq:group:<群 id>:<ai_user_id>`，延迟 10 分钟），输入是这个群里**绑定用户**的问答，
    提示词只提炼「关于这个群整体的事」（群规、约定、常聊的话题、群梗、群里的大事），**不提炼关于个人的事**，用同样的 add / update / delete 操作；
  - 管理员在后台按群查看、编辑、置顶、删除。
- 读：群里**所有**回复都带这个群的群记忆（`qq_settings.guest_reads_group_memory`，默认 `false`：游客提问时不带，
  按「绑定后才能用共享记忆」的口径；改成 `true` 就是群里人人可享）。
- 群记忆只在这个群里用，不进站内任何地方。机器人被移出群（`GROUP_DEL_ROBOT`）后保留 30 天再删。

## 9 数据模型

迁移 `migrations/2026xxxx000001_qq_bot.sql`（日期接在现有最后一个迁移之后），说明写进 `docs/schema.md` 新的「QQ 机器人」一节。
id 一律雪花（`ik_kernel::id`），时间 `TIMESTAMPTZ`，与现有表一致。

| 表 | 主要列 | 说明 |
|---|---|---|
| `qq_bots` | `app_id` PK、`display_name`、`default_card_slug`、`enabled` | 密钥不入库，在部署配置（见第 10 节）；后台只维护展示名、默认角色、开关 |
| `qq_groups` | `id`、`app_id`、`group_openid`（唯一：app_id + group_openid）、`display_name`（后台填）、`card_slug`、`enabled`、`blocked`、`context_reset_at`、`joined_at`、`left_at`、`last_active_at` | 机器人加群时建行 |
| `qq_identities` | `id`、`app_id`、`scene`（`group` / `c2c`）、`openid`、`group_id`（scene=group 时非空）、`nickname`（最近一次的群昵称，≤32）、`user_id`（绑定的绳网账号，可空，`ON DELETE SET NULL`）、`bound_at`、`card_slug`（单聊用）、`blocked`、`context_reset_at`、`last_active_at` | 唯一：(`app_id`, `scene`, `openid`, `group_id`)；索引 `user_id`。注销账号时同时清空该账号身份的 `nickname` |
| `qq_bind_codes` | `code_hash` PK、`user_id`、`expires_at`、`used_at`、`used_by_identity_id` | 定时任务清理过期码 |
| `qq_messages` | `id`、`app_id`、`identity_id`、`group_id`、`ai_user_id`（机器人当时用的角色，入站的非 @ 群消息为空）、`direction`（`in` / `out`）、`mentioned`（入站是否 @ 了本机器人）、`content`（≤4000）、`qq_msg_id`、`reply_to_id`、`created_at` | 唯一：(`app_id`, `qq_msg_id`)（入站，挡平台重推）。上下文、提炼、`recall_history` 的来源。保留：没 @ 的群消息 3 天、游客 30 天、绑定用户 180 天（定时清理）；账号注销时删该账号所有身份的消息 |
| `qq_turns` | `id`、`execution_key`（唯一，`qq:<app_id>:<消息 id>`）、`message_id`、`identity_id`、`group_id`、`ai_user_id`、`payer_user_id`（游客为空）、`status`（`queued/processing/completed/failed/skipped`）、`reason`、`hold_id`、`model_key`、`prompt_tokens`、`completion_tokens`、`segments_sent`、`attempts`、`created_at`、`finished_at` | 任务审计（等价于 `ai_jobs`，不复用它：`ai_jobs.quota_user_id NOT NULL` 且 `kind` 只有 dm / comment） |
| `qq_guest_usage` | (`day`, `identity_id`) PK、`turns`、`notified` | 游客每日计次 |
| `qq_group_usage` | (`day`, `group_id`) PK、`guest_turns`、`tokens` | 群每日计数 |
| `qq_guest_tokens` | `day` PK、`used` | 全部游客每日 token |
| `qq_memory_progress` | (`user_id`, `ai_user_id`) PK、`until` | 个人记忆提炼进度 |
| `qq_group_memories` | 见 8.3（第三期） | |
| `qq_group_memory_progress` | (`group_id`, `ai_user_id`) PK、`until` | 第三期 |
| `qq_settings` | 单行：`enabled`、`guest_daily_turns`(10)、`guest_group_daily_turns`(100)、`guest_daily_tokens`(2000000)、`guest_model_key`、`group_context_turns`(12)、`group_context_hours`(2)、`c2c_context_turns`(20)、`reply_timeout_secs`(90)、`max_reply_chars`(4000)、`reply_format`('markdown')、`link_mode`('allow')、`blocked_words TEXT[]`、`guest_reads_group_memory`(false)、`updated_at` | 后台维护、随配置快照 60 秒缓存（同 AI-002），改了记审计 |

`ai_role_cards` 加 `qq_enabled`；`ai_memories` 加 `source_qq_group_id`。

## 10 配置

部署配置（`ik_kernel::config`，figment，`IK_` 前缀），写进 `deploy/ik-server.env.example`：

```
IK_QQBOT__ENABLED=false
IK_QQBOT__SANDBOX=false                       # true 时 OpenAPI 用沙箱地址
IK_QQBOT__APPS__<名字>__APP_ID=102xxxxxx
IK_QQBOT__APPS__<名字>__SECRET=...            # AppSecret：access token 与 Webhook 验签都用它
IK_QQBOT__WEBHOOK_MAX_SKEW_SECS=300
```

- 非法配置启动即失败（`ENABLED=true` 却没有任何 app、secret 为空）。
- 支持多个机器人（多个 AppID），Webhook 按请求头 `X-Bot-Appid` 区分，找不到对应配置 401。
- `IK_QQBOT__TOKEN_URL`（默认 `https://bots.qq.com/app/getAppAccessToken`，官方 SDK 用的地址；官方文档写的 `https://api.bot.qq.com/...` 也可以），
  `IK_QQBOT__API_BASE`（默认 `https://api.sgroup.qq.com`，`SANDBOX=true` 时默认 `https://sandbox.api.sgroup.qq.com`）。
- `IK_QQBOT__ALLOW_VALIDATION`（默认 `false`）：只在后台配置回调地址的那几分钟打开，见 13 节。
- Ed25519：用 `ring`（已在依赖树里，经 rustls 引入）的 `Ed25519KeyPair::from_seed_unchecked` 签 `op=13`，
  用 `signature::UnparsedPublicKey` 验签；不要引入 aws-lc 或 OpenSSL（见 `Cargo.toml` 注释）。
- HTTP 客户端用现有的 `reqwest13` + ring 构造方式（同 `ik_ai::llm` / `mcp_client`），超时 10 秒，不跟随重定向。

## 11 接口

### 11.1 平台回调

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/v1/qq/webhook` | 见 3.2；不进 OpenAPI 文档的公开分组也行，但要在 `docs/api-guidelines.md` 登记 |

### 11.2 用户（主站调用，需登录）

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/v1/me/qq/bind-codes` | 生成绑定码，回 `{code, expiresAt, instruction}`；每人每小时最多 10 次 |
| GET | `/v1/me/qq/bindings` | 已绑定的身份列表 |
| DELETE | `/v1/me/qq/bindings/{id}` | 解绑 |

新错误码（登记进 `docs/api-guidelines.md`）：`QQ_BIND_CODE_LIMIT`（429，有效码太多或太频繁）、`QQ_BOT_DISABLED`（409，全站没开 QQ 机器人）。
绑定码错误、过期发生在 QQ 里，用文字回复，不是接口错误码。

### 11.3 管理后台（`/v1/admin/qq/`，权限同 AI 后台，写操作记审计）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET / PUT | `/settings` | `qq_settings` |
| GET / PUT | `/bots/{appId}` | 展示名、默认角色、开关（只能改配置里已有的 AppID） |
| GET | `/groups` | 群列表（活跃度、今日游客次数与 token） |
| PUT | `/groups/{id}` | 展示名、角色、启用、封禁、重置上下文 |
| GET | `/identities?user=<id 或用户名>` | 某用户绑定的身份；也能按群查 |
| PUT | `/identities/{id}` | 封禁 / 解封、强制解绑 |
| GET | `/turns?…` | 最近的回复记录（排障：状态、原因、模型、用量），不返回消息全文以外的 openid 全文 |
| GET / PUT / DELETE | `/groups/{id}/memories[...]` | 群共享记忆（第三期） |

管理后台（`admin/`）新增「QQ 机器人」页：设置、群、绑定、回复记录、群记忆；角色卡页加「QQ」开关。

## 12 后台任务与定时任务（登记在 `crates/server/src/jobs.rs`）

| 任务 | 说明 |
|---|---|
| `qq.reply` | 回复，最多 2 次尝试；幂等（执行键 + turn 状态，已 completed 直接结束，同 TRIG-037） |
| `ai.extract_memories_qq` | 个人记忆提炼（延迟、去抖） |
| `qq.extract_group_memories` | 群记忆提炼（第三期） |
| `qq.prune`（定时，每天一次） | 清过期绑定码、超期的 `qq_messages`、90 天前的计数行、退群 30 天的群记忆 |

## 13 安全与合规

- **验签必须在解析 JSON 之前**；未配置 AppID 的请求 401。
- **`op=13` 不带签名，是个签名机**：平台的回调地址校验请求没有签名头，服务端要用 AppSecret 派生的私钥对 `event_ts + plain_token` 签名后返回。
  任何人都能发 `op=13`，如果照官方 SDK 那样原样签，攻击者可以令 `event_ts` = 某个时间戳、`plain_token` = 一段伪造的事件 JSON，
  拿到的签名正好能通过 `timestamp + body` 的验签，**伪造任意事件推送**（冒充任何人在群里说话、刷别人的额度、绑定账号）。防法，两条都做：
  1. `op=13` 默认直接拒绝（`IK_QQBOT__ALLOW_VALIDATION=false`）；只在后台配置 / 修改回调地址时临时打开，配好就关；
  2. 即使打开，也只接受 `event_ts` 全是数字（≤20 位）、`plain_token` 只含字母数字（≤64 位）的请求，否则 400。
     这样签出来的内容不可能是以 `{` 开头的 JSON，不能拿去伪造事件。写单元测试：用 `plain_token = "{\"op\":0,...}"` 请求必须被拒。
- 游客与群里的生成不注入任何用户 id、openid、绳网用户名（TOOL-004）；日志里 openid 只打前 6 位。
- 群聊生成通过授权、上下文与记忆过滤隔离群友绑定的绳网账号、站内私信内容、其他群的事；测试断言记忆段的过滤。
- QQ 消息存库，隐私政策需补充「通过 QQ 与 AI 角色的对话会被保存用于上下文与记忆，可在设置中解绑并联系管理员删除」（待决问题 1）。
- 机器人在 QQ 开放平台的「内容安全」要求：上线前让运营确认角色卡提示词在 QQ 场景下的口径（不涉政、不擦边），敏感词表先填一版。
- 封禁：后台可封群、封身份；绳网账号封禁联动（4.2）。

## 14 实施分期

| 期 | 内容 | 验收 |
|---|---|---|
| 0 | 在 QQ 开放平台建机器人（Webhook 模式、登记出口 IP）、开沙箱群；实测本文标【沙箱实测】的几项（带链接的消息能否发出及错误码、群主 / 管理员字段名），结果写回本文 | 本文没有【沙箱实测】标记 |
| 1（已实施） | `ik-qqbot` 骨架：配置、Webhook 验签与 `op=13`、access token、发消息；`qq_*` 表；`ik_ai::external`；游客 10 次 + 群上限 + 游客 token 上限 + 全站计数；群与单聊短期上下文；QQ 清洗；`/帮助` `/额度`；角色卡 `qq_enabled`；后台设置与群页 | 沙箱群里 @机器人能以角色身份回话；第 11 次回提示且不调模型；平台重推同一事件只回一次；超过窗口不发送；`cargo test` 全过 |
| 2（已实施） | 绑定（站内发码、`/绑定` `/解绑`、站内通知与列表）；绑定用户走个人额度；个人记忆读写（单聊全量、群里按 8.1 过滤）、`remember` / `forget`、后台提炼、`recall_history` 搜 QQ；`/角色` `/重置`；后台绑定页与回复记录页 | 在 QQ 单聊里说的事，站内私信里同一个角色记得；在 A 群说的私事不出现在 B 群与评论区；额度条包含 QQ 用量 |
| 3（已实施） | 群共享记忆（表、`remember scope=group`、群提炼、后台管理） | 群里绑定用户 A 让机器人记住群约定，B 在同群问时机器人知道；另一个群不知道 |
| 4（可选） | 识图（用户发图）；按钮消息；单聊「输入中」提示与流式回复；单聊互动召回（`is_wakeup`）做「到期待跟进主动关心」；群主开了主动发言的群里的主动消息；申请 `union_openid` 后的跨群身份归并 | 另起说明 |

每期同步更新文档：新建 `docs/business-rules/qq-bot.md`（规则编号前缀 `QQ-`，把本文第 4～8 节的规则逐条登记）并加进
`docs/business-rules.md` 的索引表；`docs/schema.md` 加「QQ 机器人」一节；`docs/architecture.md` 加层与说明；
`docs/api-guidelines.md` 登记接口与错误码；`deploy/README.md` 补 QQ 开放平台的回调地址、IP 白名单、消息 URL 域名配置；
改了 SQL 要按 `docs/dev-setup.md` 更新 `.sqlx/` 离线缓存；本文状态改为「已接受」并在分期表里标「已实施」。

主站（InterKnot-Web 仓库）在第 2 期配合做「设置 → 绑定 QQ」：生成码、复制、倒计时、已绑定列表与解绑。

## 15 测试

沿用现有 `crates/*/tests` 的数据库集成测试写法（`crates/ai/tests/common`）。

- **验签**：把官方「签名校验」文档里的示例（secret、时间戳、body、期望签名）写成测试向量；短 secret 的种子派生（重复拼接）；
  篡改 body、缺签名头、签名不是 64 字节 → 401。
- **`op=13`**：开关关着时拒绝；打开时正常的 `plain_token` 能签；`plain_token` 含 `{`、超长或 `event_ts` 非数字一律 400（13 节的伪造攻击用例）。
- **事件**：`GROUP_MESSAGE_CREATE` 里 @ 了本机器人的触发回复、没 @ 的只入库；`author.bot = true` 的丢弃；`mentions[].is_you` 与 content 兜底两种判断。
- **CSRF / 限流**：不带 `Origin` 的 POST 能到达 webhook；通用按 IP 限流不作用于该路径。
- **幂等**：同一事件推两次只建一个 turn、只回一次。
- **游客额度**：第 10 次成功、第 11 次回提示且不调模型、提示当天只一次；生成失败退回次数；群上限、游客 token 上限、全站上限各自生效；
  跨 04:00 计数归零；`user_id = 0` 的预占回收后全站计数回到 0（5.3 的回归测试）。
- **绑定**：码只能用一次、过期无效、错码；已绑定身份再绑被拒；同一个人在两个群各绑一次、绑到同一账号；绑定后用个人额度；封禁的账号不回复也不降级为游客；
  注销级联。
- **记忆**：单聊带全部记忆；群里只带 `public` + 本群来源；群里提炼只用提问者自己的话；在 A 群 `remember` 的不出现在 B 群、评论区；
  站内私信能看到 QQ 来的记忆；记忆总开关关掉后不读不写。
- **生成**：群里论坛工具是游客视角；回复按 `reply_format` 和 `link_mode` 排版；分段数不超过上限；超出被动窗口的任务放弃并退回计次。
- **QQ OpenAPI**：用 axum 起假服务器（同 `mcp_client` 的测试），覆盖 token 获取（含 HTTP 200 但 `code` 非 0 的错误）与过期刷新、
  发送成功、`msg_seq` 递增、URL 被拒后去链接重发、内容违规与窗口过期不重试。
- 单元测试：昵称清洗与「群友 A」编号、指令解析、回复清洗、绑定码生成与 HMAC。

## 16 待决问题（落地前确认，确认后登记为 D-xxx）

1. 隐私政策补充 QQ 对话的保存与记忆（第 13 节）。
2. 游客默认值：每人 10 次、每群 100 次、游客合计 200 万加权 token/天、游客专用便宜模型——是否合适？
3. 群里的角色：一个机器人一个默认角色 + 后台按群覆盖，够不够？以后要不要让群主切换（取决于实测群主字段）？
4. 链接：默认 `link_mode=allow` 已确认；上线前分别验收群聊、单聊外链发送，按实际平台接受情况使用 domain/strip 回退。
5. 群共享记忆是否让游客提问时也带上（`guest_reads_group_memory`）。
6. 绑定用户在群里，是否允许角色用「站内的称呼」（档案里的「对方喜欢被叫 xx」）称呼他——这会在群里间接暴露站内档案；默认只在单聊用。
7. 要不要鼓励群主打开「接收全部群消息」（群上下文更好，但服务器会存更多与机器人无关的群聊，只保留 3 天）。

## 17 来源（2026-10-25 核实）

| # | 来源 | 用于 |
|---|---|---|
| 1 | 官方文档「消息收发概述」<https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/overview.html> | 被动回复窗口与次数（单聊 60 分钟 4 次、群 5 分钟 5 次）、`msg_seq` 去重、主动消息规则与频控、`msg_type` |
| 2 | 官方文档「发送消息」及「发送群聊消息」「发送单聊消息」接口页 <https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/send-receive/send.html> | 发送接口与字段、消息 URL 配置、主动消息变更记录 |
| 3 | 官方文档「Webhook 方式」<https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/event-emit/webhook.html> | 回调地址要求、端口、`op=13`、`X-Bot-Appid` |
| 4 | 官方文档「签名校验」<https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/interface-framework/sign.html> | 签名头、种子派生、`timestamp + body`、示例测试数据 |
| 5 | 官方文档「获取访问凭证」<https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/access-token.html> | token 地址、7200 秒、60 秒重叠、错误看 `code`、`QQBot` 鉴权头 |
| 6 | 官方文档「唯一身份机制」<https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/unique-id.html> 与 FAQ <https://bot.q.qq.com/wiki/bot_FAQ/> | 不同群 openid 不同、`union_openid` 需申请 |
| 7 | 腾讯官方 SDK 源码：`@tencent-connect/qqbot-nodejs` 1.0.5（npm）、<https://github.com/tencent-connect/openclaw-qqbot>、<https://github.com/tencent-connect/botgo> | 事件结构（`author.username`、`bot`、`mentions[].is_you`）、Webhook 验签与 `{"op":12,"d":0}`、`op=13` 不验签、token 地址 `bots.qq.com`、单条约 5000 字、单聊「输入中」与流式消息 |
| 8 | LINUX DO 社区帖「QQ 官 bot 近期下放的新功能」（2026-06-17）<https://linux.do/t/topic/2424596> | 群主可开全量群消息、markdown 全开放、URL 放开（与 [2] 矛盾，待实测）、群主动推送开关、单聊互动召回周期、沙箱群不限回复次数、IP 白名单 |
| 9 | 社区开源项目的问题记录：<https://github.com/xmanrui/dsh-im/issues/311>、<https://github.com/SuInk/Diana/pull/883>、<https://github.com/AmiyaBot/Amiya-Bot-core/pull/118> | 开全量后 @ 消息改走 `GROUP_MESSAGE_CREATE`、机器人自己的发言会回推 |
| 10 | IT之家「腾讯调整 QQ 机器人消息推送策略」<https://www.ithome.com/0/846/651.htm> | 2025-04 取消主动推送 |
| 11 | Qwen Code 文档 QQ 渠道 <https://qwenlm.github.io/qwen-code-docs/zh/users/features/channels/qqbot/> | 沙箱 OpenAPI 域名 |
| 12 | <https://github.com/AUTO-MAS-Project/AUTO-MAS/issues/1020> | 带链接被拒的错误码 `304003` / `40034028` |
| 13 | 官方文档「Markdown 消息」<https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/type/markdown.html> | 2026-04-23 起全量开放自定义 Markdown、群聊及单聊支持 markdown.content、支持的文字格式 |

## 18 实施记录与方案出入（2026-10-26）

- **记忆的场合不用外键列**：方案写的是 `ai_memories.source_qq_group_id → qq_groups`。实现成 `ai_memories.source_scope TEXT`
  （不透明的场合键，QQ 群是 `qq:group:<id>`）：`ai_memories` 归 ik-ai（第 8 层），不能引用 ik-qqbot（第 9 层）的表；
  ik-ai 只认「公开场合 + 键」，将来别的渠道也能用。规则不变：公开场合只用 public 的与同场合的记忆。
- **群共享记忆的表在 ik-ai**：方案写的是 `qq_group_memories`。实现成 ik-ai 的 `ai_shared_memories`（按场合键），
  因为 `remember` 工具、提炼与记忆段都在 ik-ai；qqbot 只管提炼进度（`qq_group_memory_progress`）、任务与后台接口。
- **单聊没做的**：跨会话衔接（带站内私信最近几轮）、久别重逢与会话滚动摘要没有接到 QQ 单聊；单聊的上下文是 12 小时内最近 20 条，
  长期记忆、相关往事、到期待跟进都有。单聊里的 `recall_history` 仍只翻站内私信的原文，没有扩展到 QQ 消息。
- **签名测试向量**：官方「签名校验」文档的 DEMO 给出的 sig 用它写明的输入验不过（Python cryptography 与 ring 一致），
  实现与官方 Node SDK、Go 示例逻辑一致；测试用独立实现交叉生成的向量（`webhook.rs` 测试里有说明）。
- **迁移版本号**：用 `20261117000001`～`000003`。`20261116000001` 在 main 上被一个后来撤回的迁移用过，若有环境执行过它，
  同号会被当成已执行或校验和不符。
- 第一期代码审查修过的问题：sqlx 离线缓存缺失导致编译不过；token 刷新窗口里每条消息都打 token 接口；
  回复清洗把单个 `_`、`*`（颜文字、用户名、算式）整个删掉；多出的气泡合并时没有分隔。
