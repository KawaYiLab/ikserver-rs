# 业务规则：评论与回复（comments）

> 新实现已改为扁平楼层（[ADR 0006](../adr/0006-flat-comments.md)）：取消楼中楼，每条评论都占一个楼层、从旧到新排，
> 回复可以指向任意一条评论并带上它的引用，删除不再级联，置顶 / 采纳对所有评论开放。本文记录的是旧系统的行为，
> 与 ADR 0006 冲突的规则（`CMT-025`、`CMT-026`、`CMT-048`、`CMT-052`、`CMT-057`、`CMT-060`、`CMT-064`～`CMT-070`、
> `CMT-074`、`CMT-091`、`CMT-136` 等）以 ADR 为准。

## 范围

评论创建（正文、图片、表情、@提及、匿名）、楼中楼回复结构、评论列表 / 回复分页 / 评论搜索、删除、置顶 / 取消置顶、采纳 / 取消采纳最佳答案、评论点赞、评论产生的通知、对文章的副作用（commentscount、bump）、经验、AI 触发条件、限流、入站考试门控、隐藏（举报风控）与评论的交互。

不在范围：文章本身的发布 / 隐藏流程、通知会话聚合桶的内部实现、AI 回复的执行流程（只记录触发条件）、举报流程本身（只记录对评论的影响）。

旧 API 路径与响应形状不需要保留；下文提到旧路径只为说明语义。行号引用以旧后端根目录 `InterKnot-web/server/` 为基准（前端以 `InterKnot-web/app/` 为基准）。

## 来源文件

- `src/api/comment/controllers/comment.ts`（create / pin / unpin / accept / unaccept / listLite / repliesLite / searchLite）
- `src/api/comment/content-types/comment/lifecycles.ts`、`schema.json`
- `src/api/comment/routes/{comment,comment-accept,comment-list,comment-pin,comment-replies,comment-search}.ts`
- `src/api/comment/utils/{emote,mention}.ts`
- `src/api/emote/**`、`src/api/emote-group/**`
- `src/api/like/controllers/like.ts`、`src/api/like/content-types/like/{lifecycles.ts,schema.json}`、`src/api/like/routes/like-custom.ts`
- `src/api/notification/content-types/notification/lifecycles.ts`、`src/api/notification/utils/{serialize,conversation,bucket}.ts`、`src/api/notification/controllers/notification.ts`
- `src/api/conversation/utils/notification-merge.ts`
- `src/api/report/services/report.ts`、`src/api/report/utils/{report-core,report-actions}.ts`
- `src/api/author/controllers/author.ts`（profileComments）、`src/api/article/controllers/article.ts`（计数重算）
- `src/utils/{content-visibility,anonymous,block,level,benefits,categories,common,meilisearch}.ts`、`src/utils/agent/triggers.ts`
- `src/policies/is-comment-author.ts`、`src/middlewares/{require-exam-passed,rate-limit}.ts`
- `config/{plugins,cron-tasks,middlewares}.ts`、`src/index.ts`、`database/migrations/2026.06.30T00.00.00.backfill-article-commentscount.js`
- 测试：`tests/unit/{comment-accept,comment-pin,comment-replies-paging,comment-search,emote,interaction-count-increment,interaction-dedupe,content-visibility,content-write-boundaries,anonymous,denny-notification-after-commit}.test.ts`
- 前端：`app/components/PostOverlay.vue`、`app/composables/useApi.ts`、`app/utils/benefits.ts`

---

## 1 数据与字段约束

### 1.1 评论实体

- **CMT-001** 评论字段：`content`（text，可空，maxLength 2000）、`author`（作者档案 manyToOne）、`article`（manyToOne）、`parent`（自引用 manyToOne）、`images`（多值媒体，allowedTypes `images`）、`likescount`（int，默认 0，min 0）、`isAnonymous`（默认 false）、`isHidden`（默认 false）、`hiddenReason`（private）、`anonymousSeed`（private）、`pinnedAt`/`pinnedBy`（pinnedBy private）、`acceptedAt`/`acceptedBy`（acceptedBy private）、`agentJobId`（unique，private）、`quotaReceipt`（json，private）。`src/api/comment/content-types/comment/schema.json`
- **CMT-002** 评论没有草稿 / 发布两态，一条评论只对应一行；不存在评论编辑接口（只开放 create / delete 两个核心路由）。`schema.json`（`draftAndPublish: false`）、`src/api/comment/routes/comment.ts:8`
- **CMT-003** `anonymousSeed`、`hiddenReason`、`pinnedBy`、`acceptedBy`、`agentJobId`、`quotaReceipt` 永远不能下发给客户端。`schema.json`
- **CMT-004** 客户端只能写 `content`、`author`、`article`、`parent`、`images`、`isAnonymous`；其余字段（seed、计数、置顶、采纳、隐藏）全部由服务端决定。`src/api/comment/controllers/comment.ts:639-649`

### 1.2 正文

- **CMT-005** `content` 先做 `trim()`，非字符串视为空串；trim 后长度 `> 2000` 返回 400 `content exceeds 2000 characters`。长度按 JS `String.length`（UTF-16 码元）计算，重写时要保持同一口径。`src/api/comment/controllers/comment.ts:550-553`
- **CMT-006** 允许纯图片评论：`content` 为空但 `images` 至少一张时可以创建；两者都为空返回 400 `content or images is required`。这个检查在图片解析之后执行。`src/api/comment/controllers/comment.ts:606-608`；测试 `content-write-boundaries.test.ts:104,113`
- **CMT-007** 正文以纯文本存储，内嵌两种 token：表情 `:ik-<code>:` 与提及 `@[显示名](authorDocumentId)`。服务端不做 HTML 处理。`src/api/comment/utils/emote.ts:4-6`、`src/api/comment/utils/mention.ts:5-6`

### 1.3 表情（emote）

- **CMT-008** 表情 token 正则为 `/:(ik-[a-z0-9-]{1,32}):/g`，捕获组是含 `ik-` 前缀的完整 code；前端使用同一个正则，两端必须一字不差。`src/api/comment/utils/emote.ts:25`；测试 `emote.test.ts:11`
- **CMT-009** 单条评论表情 token 总数（不去重，按出现次数计）`> 20`（`MAX_COMMENT_EMOTES`）返回 400 `content exceeds 20 emoticons`。`src/api/comment/controllers/comment.ts:199,558-561`
- **CMT-010** 不存在于清单中的 code 不会被拒绝，按原文保留；渲染时查不到就原样显示 `:ik-xxx:`。`src/api/comment/controllers/comment.ts:555-557`、`src/api/comment/utils/emote.ts:14-15`
- **CMT-011** 通知预览 / AI prompt / 搜索使用的纯文本转换：表情 token 替换为 `[表情:name]`，查不到 name 时替换为 `[表情]`；要先剥离提及再剥离表情。`src/api/comment/utils/emote.ts:78-88`
- **CMT-012** 表情实体：`code` 唯一且必须匹配 `^ik-[a-z0-9-]{1,32}$`；`name` 必填；`image` 单图必填；`group` 关联分组；`order` int ≥0 默认 0；`isActive` 默认 true。分组实体：`name` 唯一，`order`，`isActive`，`icon` 可选。两者只能由管理员在后台维护，不对外提供 CRUD。`src/api/emote/content-types/emote/schema.json`、`src/api/emote-group/content-types/emote-group/schema.json`、`src/api/emote-group/routes/emote-group.ts`
- **CMT-013** 表情清单（公开，无需登录）：返回 `isActive=true` 且有图片的表情，按 `order asc, id asc` 排序，最多 500 条；字段 `code,name,group,url,width,height`；`group` 取自已启用分组的名称，分组未启用或没有分组时为 `通用`。同时返回 `isActive=true` 的分组，按 `order asc, id asc`，最多 100 条，字段 `name,order,iconUrl`（没有图标时为 null）。响应头 `Cache-Control: public, max-age=60`。`src/api/emote/controllers/emote.ts:22-64`

### 1.4 @提及（mention）

- **CMT-014** 提及 token 正则为 `/@\[([^\[\]\n]{1,40})\]\(([A-Za-z0-9]{6,32})\)/g`：显示名 1–40 个字符（不含 `[`、`]`、换行），id 是作者档案的 documentId（6–32 位字母数字）。显示名是快照，被提及者改名后旧评论仍显示旧名。`src/api/comment/utils/mention.ts:8-17`
- **CMT-015** 「裸 @」自动展开：写库前把不在已有 token 区间内的 `@<word>`（正则 `/@([\p{L}\p{N}_\-]+)/gu`）展开成 `@[name](docId)`。`src/api/comment/content-types/comment/lifecycles.ts:17-29`、`src/api/comment/utils/mention.ts:88,134-223`
- **CMT-016** 裸 @ 解析口径：候选词同时与 `user.username` 和 `author.name` 做精确相等匹配（OR），只考虑 `blocked != true` 的用户；同一候选词命中 ≥2 个不同作者（包括 username 与 name 分别指向不同人）视为歧义，原文保留；没有命中也原文保留；生成 token 的显示名取 `author.name || username`。所有候选词用一次批量查询解析。`src/api/comment/utils/mention.ts:117-199`
- **CMT-017** 裸 @ 展开失败（例如数据库异常）不阻断评论创建，按原文写入。`src/api/comment/content-types/comment/lifecycles.ts:23-28`
- **CMT-018** 提及通知最多处理正文中前 10 个**不同的** authorDocumentId（按首次出现顺序，在解析用户之前截断，无效 id 也占名额）。正文里的 token 数量本身没有上限。`src/api/comment/utils/mention.ts:66-80`、`src/api/comment/content-types/comment/lifecycles.ts:511`
- **CMT-019** 会话预览中提及 token 显示为 `@显示名`。`src/api/comment/utils/mention.ts:54-60`

### 1.5 图片

- **CMT-020** 单条评论图片上限由权益等级决定：`commentMaxImages = [0, 3, 6, 9, 9, 12, 12, 18]`（下标 = 等级 0–7）；管理员与 AI Agent 按最高档（7 → 18 张）；等级取整并夹在 0..7。前端持有同一张表，仅用于展示。`src/utils/benefits.ts:12,24-34`、`app/utils/benefits.ts:12`
- **CMT-021** 超出上限返回 400 `images cannot exceed ${max} items`。这个检查按**原始数组长度**在逐项解析（查库）之前进行，防止超大数组压垮连接池。`src/api/comment/controllers/comment.ts:518-522`；测试 `content-write-boundaries.test.ts:120`
- **CMT-022** 图片引用支持的形态：正整数 id、纯数字字符串、上传文件 documentId 字符串、`{id}`、`{documentId}`，以及外层的数组 / `{set:[...]}` / `{connect:[...]}` / 单值。`null` 与空串会被跳过。`src/api/comment/controllers/comment.ts:456-531`
- **CMT-023** documentId 查不到时返回 400 `images[i] references upload file "<id>" but it does not exist`；其他形态返回 400 `images[i] has an unsupported media reference format`。数字 id 不做存在性校验。`src/api/comment/controllers/comment.ts:464-498`
- **CMT-024** 服务端**不校验**图片文件归属上传者，控制器也不校验 MIME（只有 schema 的 `allowedTypes: images`）。见 10 未决问题。`src/api/comment/controllers/comment.ts:434-499`

### 1.6 回复结构

- **CMT-025** 楼中楼只有一层：`parent` 必须是**同一文章下、未隐藏的顶层评论**（`parent.parent == null`）。不允许回复的回复。`src/api/comment/controllers/comment.ts:610-631`
- **CMT-026** 「回复某条回复」在前端实现为：`parent` 仍指向所属顶层评论，并在输入框开头自动预填被回复者的提及 chip（自己回复自己时不预填）；被回复者通过 mention 通知得知。`app/components/PostOverlay.vue:1007-1030`、`src/api/comment/controllers/comment.ts:303-306`

---

## 2 权限与前置条件

### 2.1 创建

- **CMT-027** 创建评论要求已登录，并且通过入站考试（`examPassed === true`，或 `isAdmin`、`isAiAgent`）；未通过返回 403 `{ error: { code: 'EXAM_REQUIRED', message: '需要先通过入站考试才能进行此操作' } }`。每次请求都按 user id 重新查询考试状态。`src/api/comment/routes/comment.ts:10-12`、`src/middlewares/require-exam-passed.ts:16-39`
- **CMT-028** 请求体缺少 `data` 对象返回 400 `Missing "data" payload`。`src/api/comment/controllers/comment.ts:546-548`
- **CMT-029** `author` 必填（缺失返回 400 `author is required`），可以是 documentId 字符串、`{documentId}` 或 `{connect:[...]}`；解析不出返回 400 `Invalid author`；不存在返回 400 `Author not found`；作者档案不属于当前用户返回 403 `Author does not belong to you`。`src/api/comment/controllers/comment.ts:564-584`
- **CMT-030** `article` 必填（400 `article is required`）；解析不出返回 400 `Invalid article`；文章不可见返回 400 `Article not found`。「可见」的定义：已发布（`publishedAt` 非空）、`isHidden !== true`（作者本人也不能评论自己被隐藏的帖子），且当文章非匿名时评论者与文章作者之间没有任何方向的拉黑。`src/api/comment/controllers/comment.ts:587-594`、`src/utils/content-visibility.ts:98-118`；测试 `content-write-boundaries.test.ts:72`
- **CMT-031** `parent` 存在时：解析不出返回 400 `Invalid parent`；不存在返回 400 `Parent comment not found`；被隐藏、本身是回复、或属于其他文章，返回 400 `Parent must be a visible top-level comment in this article`。`src/api/comment/controllers/comment.ts:612-631`；测试 `content-write-boundaries.test.ts:85`
- **CMT-032** 写库前再做一次兜底校验：文章不可见抛出 `Article is not available for comments`；parent 不合法抛出同 CMT-031 的文案；文章非匿名且评论者与文章作者有拉黑关系抛出 `Cannot comment on blocked user`；parent 非匿名且评论者与 parent 作者有拉黑关系抛出 `Cannot reply to blocked user`。`src/api/comment/content-types/comment/lifecycles.ts:118-172`
- **CMT-033** 拉黑判断是双向的（任一方拉黑另一方都算）；目标内容是匿名的则跳过拉黑判断（防止通过「被拒」反推匿名作者身份）。`src/api/comment/content-types/comment/lifecycles.ts:159-171`
- **CMT-034** 创建前的校验顺序（决定先报哪个错）：data → 正文长度 → 表情数 → author → article → 图片数量与解析 → 空内容 → parent。`src/api/comment/controllers/comment.ts:540-631`

### 2.2 删除

- **CMT-035** 只有评论作者本人（评论的 `author.user.id === 当前用户`）可以删除；路由参数先按 documentId 查，查不到作者时再把参数当数字 id 查。帖主和管理员都**不能**通过这个接口删除他人评论（管理员只能通过举报处理删除，见 CMT-118）。`src/policies/is-comment-author.ts:1-35`、`src/api/comment/routes/comment.ts:14-16`
- **CMT-036** 删除接口**没有**入站考试门控（只要求登录并通过作者策略）。`src/api/comment/routes/comment.ts:14-16`

### 2.3 置顶 / 采纳

- **CMT-037** 置顶、取消置顶、采纳、取消采纳都要求登录且通过入站考试。`src/api/comment/routes/comment-pin.ts:4-20`、`src/api/comment/routes/comment-accept.ts:4-20`
- **CMT-038** 上述四个操作只允许文章作者（`article.author.user.id === 当前用户`）或 `user.isAdmin === true` 执行，否则返回 403：`Not allowed to pin this comment` / `Not allowed to unpin this comment` / `Not allowed to accept this comment` / `Not allowed to unaccept this comment`。`src/api/comment/controllers/comment.ts:697-699,781-783,850-852,968-970`
- **CMT-039** 采纳只在分区开启时可用：文章所属分区 `allowCommentAccept === true`（严格等于 true；NULL 或文章没有分区都视为关闭），否则返回 403 `该分区未开启评论采纳`，管理员也不例外。这个检查在帖主 / 管理员身份检查**之前**。`src/api/comment/controllers/comment.ts:837-842`、`src/utils/categories.ts:95-109`
- **CMT-040** 取消采纳**不**检查分区开关：分区关闭或帖子被移出分区后，存量最佳答案仍要能被撤销。`src/api/comment/controllers/comment.ts:936-939`；测试 `comment-accept.test.ts:197`
- **CMT-041** 置顶、采纳、取消置顶、取消采纳都不检查文章是否已隐藏或已下架（只检查评论本身）。`src/api/comment/controllers/comment.ts:687-695,844-848`

### 2.4 读取

- **CMT-042** 评论列表、回复分页、评论搜索都是公开接口；有登录态时才做拉黑过滤和点赞态标记。`src/api/comment/routes/comment-list.ts:7-10`、`comment-replies.ts:30-36`、`comment-search.ts:12-17`

---

## 3 流程与状态机

### 3.1 创建流程

- **CMT-043** 匿名标记只认 `data.isAnonymous === true`，其他值一律视为非匿名；`anonymousSeed` 只在匿名时由服务端生成（16 字节随机数的 hex，32 字符），客户端传入的 seed 一律忽略。`src/api/comment/controllers/comment.ts:633-636`、`src/utils/anonymous.ts:13-14`
- **CMT-044** 创建成功返回 HTTP 201，内容包含 `documentId`、`content`（裸 @ 展开后的规范化正文）、`createdAt`、`isAnonymous`。`src/api/comment/controllers/comment.ts:651-662`
- **CMT-045** 创建后的副作用顺序（同一事务内）：通知被评论 / 被回复者 → 提及通知 → 文章 `commentscount +1` → 文章 bump → 发评论经验 → AI 任务写入 outbox；提交后再异步广播 `topic_bumped`。`src/api/comment/content-types/comment/lifecycles.ts:70-94`
- **CMT-046** 「通知被评论 / 被回复者」这一步没有 try/catch，失败会让整条评论回滚；提及、bump、经验各自吞掉错误；AI 入队在事务中失败会重新抛出，导致评论创建失败。`src/api/comment/content-types/comment/lifecycles.ts:198-240,495-561,380-411,442-477`、`src/utils/agent/triggers.ts:158-162`

### 3.2 删除流程（硬删除）

- **CMT-047** 删除是硬删除，没有软删除和墓碑。删除前先记下被删评论的 id、所属文章、全部后代评论 id（BFS，最多 32 层，用于防止成环），以及各后代按文章分组的数量。`src/api/comment/content-types/comment/lifecycles.ts:47-69,660-678`
- **CMT-048** 删除顶层评论会级联删除它的全部回复，以及这些回复关联的通知和点赞；不能让回复的 parent 变成 NULL 后漂回顶层列表。`src/api/comment/content-types/comment/lifecycles.ts:54-56,691-806`
- **CMT-049** 级联顺序：①删除后代评论关联的通知并重算通知桶；②删除后代评论的点赞（单次最多 500 条），逐条补偿点赞计数与点赞通知；③批量删除后代评论；④失效收件人的会话缓存；然后文章 `commentscount` 先 −1（被删评论本身），再按后代所属文章分别减去后代数量；最后删除被删评论本身关联的全部通知（comment / reply / mention / like / accepted）并重算通知桶、失效缓存。`src/api/comment/content-types/comment/lifecycles.ts:103-115,575-639,691-806`
- **CMT-050** 级联删除后代失败时会抛出错误，让整个删除回滚（注释写的是「失败不抛错」，但代码实际会 rethrow，以代码为准）；删除被删评论自身通知的步骤失败只记 warn。`src/api/comment/content-types/comment/lifecycles.ts:802-805,636-638`
- **CMT-051** 删除评论**不**撤销已经发放的经验，也**不**清理文章上的采纳投影（`solvedAt` / `acceptedCommentDocumentId`），见 CMT-138。`src/api/comment/content-types/comment/lifecycles.ts:103-115`

### 3.3 置顶状态机

- **CMT-052** 只有顶层评论可以置顶 / 取消置顶（回复返回 400 `Only top-level comments can be pinned` / `Only top-level comments can be unpinned`）；评论不存在返回 404 `Comment not found`；被隐藏返回 400 `Comment is hidden`；没有关联文章返回 400 `Comment has no article`；文章不存在返回 404 `Article not found`；缺少参数返回 400 `Missing documentId`。`src/api/comment/controllers/comment.ts:665-695,748-779`
- **CMT-053** 每篇文章最多一条置顶评论：置顶时在事务里先对文章行（按 document_id，全部版本行）和评论行加 `FOR UPDATE` 锁，在锁内重新检查是否隐藏（锁内发现已被隐藏返回 400 `Comment is hidden`，且不做任何修改），清掉本文其他所有置顶顶层评论的 `pinnedAt`/`pinnedBy`，再置顶当前评论（`pinnedAt = now`，`pinnedBy = 操作者`）。`src/api/comment/controllers/comment.ts:701-745`；测试 `comment-pin.test.ts:89,131`
- **CMT-054** 对已经置顶的评论再次置顶是幂等的：保留原 `pinnedAt`，但仍会清掉同文其他置顶（修复历史上的多置顶脏数据）。`src/api/comment/controllers/comment.ts:725-738`
- **CMT-055** 取消置顶在锁外先检查 `pinnedAt` 为空就返回 400 `Comment is not pinned`；锁内若发现已被并发取消，视为成功的空操作。取消置顶不检查隐藏状态以外的条件，并且锁外也会拒绝被隐藏的评论（`Comment is hidden`）。`src/api/comment/controllers/comment.ts:764-808`；测试 `comment-pin.test.ts:156`
- **CMT-056** 置顶 / 取消置顶成功都返回 `{ ok: true }`，不发通知，不给经验。`src/api/comment/controllers/comment.ts:745,808`

### 3.4 采纳状态机

- **CMT-057** 只有顶层评论可以采纳（400 `Only top-level comments can be accepted`）；被隐藏的不能采纳（400 `Comment is hidden`，锁内复查）；其余错误文案同置顶。`src/api/comment/controllers/comment.ts:811-852,857-866`
- **CMT-058** 每篇文章最多一条采纳评论：事务内对文章和评论加锁，清掉同文其他已采纳顶层评论的 `acceptedAt`/`acceptedBy`；当前评论未采纳时置 `acceptedAt = now`、`acceptedBy = 操作者`（已采纳的保留原 `acceptedAt`）；然后把文章所有版本行写成 `solvedAt = now`、`acceptedCommentDocumentId = 评论 documentId`（重复采纳会刷新 `solvedAt`）。`src/api/comment/controllers/comment.ts:857-899`；测试 `comment-accept.test.ts:126`
- **CMT-059** 采纳与置顶互不清除：采纳不会清 `pinnedAt`，置顶也不检查是否已采纳。同时处于两种状态的评论只在「采纳」位置展示（见 CMT-073）。`src/api/comment/controllers/comment.ts:868-890,1052-1065`
- **CMT-060** 取消采纳：回复返回 400 `Only top-level comments can be unaccepted`；未采纳返回 400 `Comment is not accepted`；不检查隐藏状态。事务内清掉评论的 `acceptedAt`/`acceptedBy`，只有当文章当前的 `acceptedCommentDocumentId` 正是这条评论时才清空文章的 `solvedAt` 和 `acceptedCommentDocumentId`（防止误清并发采纳的另一条）。`src/api/comment/controllers/comment.ts:940-1001`；测试 `comment-accept.test.ts:197`
- **CMT-061** 取消采纳不撤回已发的 `accepted` 通知，也不扣回经验。`src/api/comment/controllers/comment.ts:989-990`

### 3.5 评论列表（某文章的顶层评论）

- **CMT-062** 必须提供文章 documentId（缺失返回 400 `Missing article`）；分页参数 `start`（offset，负数或非法按 0）与 `limit`（默认 20，最大 50，小于 1 按 20）。`src/api/comment/controllers/comment.ts:1003-1012`、`src/utils/common.ts:269-286`
- **CMT-063** 文章不可见（未发布、已隐藏、或观看者与非匿名文章作者有拉黑关系）时返回空列表，而不是 404。`src/api/comment/controllers/comment.ts:1015-1020`
- **CMT-064** 普通列表只包含：本文、顶层、未隐藏、**未置顶且未采纳**、通过拉黑过滤的评论，按 `createdAt desc` 排序（Strapi 分页会隐式追加 `id` 作为稳定排序，重写时要显式加 tie-breaker）。`total` 与 `pageCount = ceil(total/limit)` 按同一条件计算。`src/api/comment/controllers/comment.ts:1019-1050,1207-1216`
- **CMT-065** 楼层号 `floor = total - (start + index)`：最新的评论楼层号最大；只有普通列表项有楼层号，置顶 / 采纳项没有。楼层号是动态的，删除或隐藏会让楼层号变化。`src/api/comment/controllers/comment.ts:1200`
- **CMT-066** 只有 `start === 0` 时才额外返回 `accepted`（最多 1 条，按 `acceptedAt desc`）和 `pinned`（最多 1 条，按 `pinnedAt desc`，且要求未采纳）；两者同样要求顶层、未隐藏、通过拉黑过滤。`src/api/comment/controllers/comment.ts:1052-1092`
- **CMT-067** 每条评论项（普通 / 置顶 / 采纳）的字段：`documentId, content, images[], createdAt, likesCount, liked, isAnonymous, author, replies[], repliesCount, repliesHasMore`；采纳项另加 `isAccepted: true, acceptedAt`，置顶项另加 `isPinned: true, pinnedAt`，普通项另加 `floor`。`src/api/comment/controllers/comment.ts:1177-1201`
- **CMT-068** 每条顶层评论内联前 3 条（`INLINE_REPLIES_LIMIT`）可见回复，按 `createdAt asc, documentId asc` 排序；`repliesCount` 是**可见**回复总数（已扣除隐藏和拉黑），`repliesHasMore = repliesCount > replies.length`。`src/api/comment/controllers/comment.ts:204,346-427`；测试 `comment-replies-paging.test.ts:465,503,523`
- **CMT-069** 每个父评论各自取前 3 条（不能是整页共享 3 条）；隐藏 / 拉黑条件要放进查询条件里，不能先取出来再过滤。置顶 / 采纳项同样带回复。清点回复的查询最多读取 5000 行（`REPLY_TALLY_ROW_CAP`），触顶时记 warn，尾部的 `repliesCount` 会偏小。`src/api/comment/controllers/comment.ts:215,367-384`；测试 `comment-replies-paging.test.ts:546,565,587`
- **CMT-070** 回复项字段：`documentId, content, images[], createdAt, likesCount, liked, isAnonymous, author`；不下发 `parent`（单层结构下它是冗余的）。列表内联回复与回复分页接口必须使用**同一份**映射。`src/api/comment/controllers/comment.ts:285-322`；测试 `comment-replies-paging.test.ts:611,871`

### 3.6 回复分页（某顶层评论的回复）

- **CMT-071** 只按游标翻页，不接受 offset；`limit` 默认 20，最大 50。`src/api/comment/controllers/comment.ts:1244-1246`
- **CMT-072** 游标 = base64url(`${createdAt}|${documentId}`)，在第一个 `|` 处切分；时间部分要能被解析，并统一成规范 ISO 字符串后再使用。排序为 `createdAt asc, documentId asc`，翻页条件是严格晚于游标：`createdAt > c` 或（`createdAt = c` 且 `documentId > d`）。第一页之后续上的正好是内联 3 条之后的第 4 条。`src/api/comment/controllers/comment.ts:217-278`；测试 `comment-replies-paging.test.ts:677,693,801,824`
- **CMT-073** 坏游标（无法解码、缺少 `|`、时间解析失败）按「没传」处理，回到第一页，不能返回 400 或 500。`src/api/comment/controllers/comment.ts:1247-1251`；测试 `comment-replies-paging.test.ts:761`
- **CMT-074** 目标评论不存在、被隐藏、所属文章不可见、或作者被观看者拉黑（非匿名），返回 404 `Comment not found`；目标本身是回复，返回 404 `Only top-level comments have replies`；缺少 id 返回 400 `Missing documentId`。`src/api/comment/controllers/comment.ts:1240-1267`；测试 `comment-replies-paging.test.ts:863`
- **CMT-075** 多取 1 条判断 `hasMore`，不另外 count；返回 `{ data, meta: { hasMore, nextCursor } }`，`hasMore` 为 true 时必须带 `nextCursor`（两者同时有或同时没有）。拉黑条件与游标条件都要生效，不能互相覆盖。`src/api/comment/controllers/comment.ts:1269-1291,1335-1340`；测试 `comment-replies-paging.test.ts:707,730`
- **CMT-076** 未登录也能读，此时 `liked` 一律为 false，不查询点赞表。`src/api/comment/controllers/comment.ts:1312-1324`；测试 `comment-replies-paging.test.ts:887`

### 3.7 评论搜索

- **CMT-077** 搜索词 `q` trim 后截到 32 个字符；为空时直接返回空页，不查索引。可选 `article` 参数把范围限定到某篇文章；分页同 CMT-062。`src/api/comment/controllers/comment.ts:97,145-146,1363-1397`
- **CMT-078** 主路径用全文搜索引擎（按相关性排序，过滤 `isHidden != true`、可选文章、拉黑用户），命中后**一律回查数据库**并按命中顺序输出；回查时再次应用可见性与拉黑过滤。`src/api/comment/controllers/comment.ts:1438-1464`、`src/utils/meilisearch.ts:274-279`
- **CMT-079** 搜索引擎有命中但一条都通不过回查（索引残留）时，退到 SQL 兜底而不是返回空页；搜索引擎确实没有命中时返回空页，不查 SQL。`src/api/comment/controllers/comment.ts:1458-1464`；测试 `comment-search.test.ts:135,156`
- **CMT-080** SQL 兜底：`q.length < 2` 时直接返回空页；先去掉全文布尔运算符 `[+\-><()~*"@]`、合并空白，再用全文索引匹配，最多取 1000 个 id（固定窗口，不随翻页变化；触顶记 warn）；没有全文命中时退到不区分大小写的子串匹配；按 `createdAt desc` 排序，`total` 用同一条件 count。`src/api/comment/controllers/comment.ts:104,118-119,1474-1546`
- **CMT-081** 搜索结果包含匿名评论（作者脱敏）；结果项字段同列表项，另加 `article: { documentId, title, cover }`（`cover` 取第一张图的缩略图单对象，没有时为 null），`replies: []`，**没有** `floor`。不下发 parent 信息。`src/api/comment/controllers/comment.ts:1359-1361,1416-1423,1588-1613`；测试 `comment-search.test.ts:103,120`
- **CMT-082** 搜索结果最终还要经过「同文 parent」校验：回复的 parent 必须与回复属于同一篇文章，否则剔除。`src/api/comment/controllers/comment.ts:1548`、`src/utils/content-visibility.ts:157-185`

### 3.8 用户主页的评论列表

- **CMT-083** 用户主页评论列表对所有人（包括本人）都不包含匿名评论；观看者与主人有拉黑关系，或主人设置 `profileHidden` 且观看者不是本人时，返回空列表；只返回公开可见的评论，按 `createdAt desc` 排序；不下发 parent 摘要（历史上曾通过它泄露匿名 parent 作者）。`src/api/author/controllers/author.ts:620-727`

---

## 4 副作用（计数、通知、经验、丁尼、搜索、实时、AI 触发）

### 4.1 文章计数与 bump

- **CMT-084** 每创建一条评论（包括回复、匿名、AI 评论），文章 `commentscount` 原子 +1，写到该文章 documentId 的全部版本行，结果下限为 0：`CASE WHEN COALESCE(c,0)+delta < 0 THEN 0 ELSE COALESCE(c,0)+delta END`。`src/api/comment/content-types/comment/lifecycles.ts:73,251-265`
- **CMT-085** `commentscount` 统计的是全部评论行（顶层 + 回复，**包括被隐藏的**）；隐藏 / 取消隐藏不改变计数。`src/api/comment/content-types/comment/lifecycles.ts:333-373`、`src/api/report/services/report.ts:123-166`
- **CMT-086** 删除时 −1（被删评论）并减去级联删除的后代数量（按各自所属文章）。`src/api/comment/content-types/comment/lifecycles.ts:110-113`
- **CMT-087** 评论的 `article` 关系被修改时（目前没有对外入口），旧文章 −1、新文章 +1。`src/api/comment/content-types/comment/lifecycles.ts:95-102`
- **CMT-088** 每条新评论都会把文章全部版本行的 `bumpedAt` 设为当前时间，`lastBumpedByUserId` 设为评论者 user id（列表接口用它计算 `bumpedBySelf`，让评论者自己的「有 N 条新内容」提示跳过自己顶起的帖子）。匿名评论同样会 bump。失败不阻断评论。`src/api/comment/content-types/comment/lifecycles.ts:380-411`

### 4.2 通知

- **CMT-089** 新评论的主通知：有 parent 时类型为 `reply`，收件人是 parent 作者；没有 parent 时类型为 `comment`，收件人是文章作者。字段 `sender = 评论者`、`Article = 文章`、`Comment = 新评论`、`isRead = false`。`src/api/comment/content-types/comment/lifecycles.ts:198-240`
- **CMT-090** 以下情况不发主通知：评论是匿名的；文章是匿名的；parent 是匿名的；评论者没有 user；收件人为空；收件人就是评论者本人。`src/api/comment/content-types/comment/lifecycles.ts:203-228`
- **CMT-091** 回复只通知顶层 parent 的作者；文章作者不会因为别人的回复收到 `comment` 通知（除非文章作者被提及）。`src/api/comment/content-types/comment/lifecycles.ts:214-223`
- **CMT-092** 提及通知（类型 `mention`）：评论匿名、文章匿名或 parent 匿名时**全部跳过**；解析 token 中的 authorDocumentId → user，跳过：没有 user 的、`user.blocked === true`（被封禁）的、评论者本人、本条评论的主通知收件人（parent 作者或文章作者，避免一次行为发两条）、重复的收件人。字段同主通知。`src/api/comment/content-types/comment/lifecycles.ts:479-561`
- **CMT-093** 提及通知失败不阻断评论创建。`src/api/comment/content-types/comment/lifecycles.ts:558-560`
- **CMT-094** 所有通知在创建前都会检查收件人与发送者之间是否有拉黑关系，有则拒绝创建（`Blocked user cannot receive notification`）；因此被拉黑的提及对象收不到 mention 通知。`src/api/notification/content-types/notification/lifecycles.ts:253-266`
- **CMT-095** 通知的实时推送（SSE、DM WS）、通知桶写入与缓存失效都必须在事务**提交之后**执行；事务回滚时不推送。`src/api/notification/content-types/notification/lifecycles.ts:40-73`；测试 `denny-notification-after-commit.test.ts`
- **CMT-096** 采纳通知（类型 `accepted`）：收件人是答主，发送者是操作者（帖主或管理员），带 `Article` 与 `Comment`；以下情况不发：答主没有 user、答主就是操作者（自采纳）、评论匿名、文章匿名。在事务提交后发送，失败只记 warn。每次（非自采纳）采纳调用都会发一条新通知，不去重。`src/api/comment/controllers/comment.ts:903-924`；测试 `comment-accept.test.ts:126,160,179`
- **CMT-097** 通知预览文案（会话列表 / 通知桶）：`like` 带评论时为 `赞了你的评论`，否则 `赞了你的帖子`；`favorite` → `收藏了你的帖子`；`denny` → `给你的帖子投了币`；`accepted` → `采纳了你的评论为最佳答案`；其他有评论正文的取正文，先把提及转成 `@名`，再把表情转成 `[表情:name]`/`[表情]`；`system` → `message || '系统消息'`；都不满足时为空串。`src/api/notification/utils/bucket.ts:232-242`、`src/api/notification/controllers/notification.ts:111-129`
- **CMT-098** 通知转成 DM 消息时的纯文本：`mention` 固定为 `在帖子中提到了你`；`comment`/`reply` 取剥离后的正文，**纯图片评论（正文为空）**时分别为 `评论了你的帖子 [图片]` / `回复了你的评论 [图片]`；其余同 CMT-097。注意通知桶预览（CMT-097）对纯图片评论给的是空串，两处口径不同。`src/api/conversation/utils/notification-merge.ts:533-550`
- **CMT-099** 通知序列化时附带评论的 `documentId, content(原文，含 token), images[](缩略图), isAnonymous`；评论类通知（comment/reply/mention）若关联评论是匿名的，发送者显示为 `绳网用户#XXXX`（由评论 seed 派生），id、documentId、level、头像全部置空。`src/api/notification/utils/serialize.ts:101-189`
- **CMT-100** 会话分组：`comment/reply/mention` 关联匿名评论时归入 `anonymous` 会话（peerKey = 评论 seed），否则按发送者归入 `contacts`；`like/favorite/denny/accepted` 总是按发送者归入 `contacts`；联系人会话查询必须排除匿名评论类通知。`src/api/notification/utils/conversation.ts:30-33,95-118,154-189`

### 4.3 经验与丁尼

- **CMT-101** 评论经验：评论者每天（以北京时间 04:00 为日界）**第一条**发在他人文章下的评论得 3 点经验，action `create_comment`，按「用户 + action + 当天」去重（与评论的是哪篇文章无关）；在自己文章下评论不得经验，也不占当天名额；匿名评论同样得经验。失败不阻断。`src/api/comment/content-types/comment/lifecycles.ts:442-477`、`src/utils/level.ts:31-53,84-98`
- **CMT-102** 采纳经验：答主（非自采纳）得 10 点，action `comment_accepted`，按实体 `api::comment.comment` + `comment:${comment.id}` 终身去重（反复采纳 / 取消不会重复发）；匿名评论或匿名文章也照常发放。`src/api/comment/controllers/comment.ts:925-930`；测试 `comment-accept.test.ts:155,179`
- **CMT-103** 每日经验上限：`like_received/comment_received/favorite_received/denny_received` 共享每日 1000；其余（包括 `create_comment`、`comment_accepted`、`like_give`）共享每日 50（`exam_passed` 豁免）；超出部分截断，发放 0 时不写日志。未通过考试的普通用户等级锁定为 0。`src/utils/level.ts:20-29,124-183`
- **CMT-104** 评论相关流程中没有发放 `comment_received` 经验的代码路径；评论也没有任何丁尼（投币）逻辑。`src/utils/level.ts:20-25`（只在列表中声明）

### 4.4 评论点赞

- **CMT-105** 点赞 / 取消点赞是同一个切换操作，要求登录并通过入站考试；参数 `targetType`（`article`/`comment`，忽略大小写和首尾空格）与 `targetId`（documentId）；非法类型返回 400 `Invalid targetType`，缺少 id 返回 400 `Missing targetId`。`src/api/like/controllers/like.ts:52-61`、`src/api/like/routes/like-custom.ts:4-11`
- **CMT-106** 已点赞时直接删除点赞（不检查评论当前是否可见），返回 `{ liked: false, likesCount }`；未点赞时目标评论必须公开可见（未隐藏、文章公开、parent 合法），否则返回 404 `Target not found`。`src/api/like/controllers/like.ts:113-136`
- **CMT-107** 点赞去重键 `u{userId}:c{commentDocumentId}`，带数据库唯一约束；并发重复点赞视为「已点赞」，返回 `{ liked: true, likesCount }`，不能返回 500。`src/utils/common.ts:403-432`、`src/api/like/content-types/like/lifecycles.ts:88-99,122-135`、`src/api/like/controllers/like.ts:145-152`；测试 `interaction-dedupe.test.ts:75,237`
- **CMT-108** 点赞非匿名评论时，若点赞者与评论作者有拉黑关系，返回 400 `Cannot like blocked user`；匿名评论不做拉黑判断。`src/api/like/content-types/like/lifecycles.ts:101-120`
- **CMT-109** 点赞计数：评论 `likescount` 按 id 原子 ±1，`GREATEST(COALESCE(likescount,0)+delta, 0)`；返回的 `likesCount` 从计数列回读（列为空或行不存在时为 0）。`src/api/like/content-types/like/lifecycles.ts:227-246`、`src/api/like/controllers/like.ts:94-111`；测试 `interaction-count-increment.test.ts:211,393,407,416`
- **CMT-110** 点赞通知（类型 `like`，带 `Comment`）：评论匿名时不发；点赞者就是评论作者时不发。取消点赞时删除该发送者对该评论的**全部** `like` 通知（不限于未读，最多 500 条），并重算通知桶、失效会话缓存。`src/api/like/content-types/like/lifecycles.ts:291-394`
- **CMT-111** 点赞经验：点赞者给他人评论点赞时得 1 点，action `like_give`，每天仅首次；给自己点赞不得经验；匿名评论照常发放。被点赞者在这条路径上不获得经验。`src/api/like/content-types/like/lifecycles.ts:396-424`
- **CMT-112** 点赞的副作用（计数、通知、经验）互相独立，任一失败只记 error，不影响点赞本身和其他副作用。`src/api/like/content-types/like/lifecycles.ts:158-194`
- **CMT-113** 批量查询点赞状态：每次最多 100 个 id（去重、trim）；我的点赞列表只返回目标仍然可见的记录。`src/api/like/controllers/like.ts:15,26-47,184-317`

### 4.5 搜索索引

- **CMT-114** 评论搜索索引字段：`id, documentId, content(纯文本), articleDocumentId, articleTitle, isAnonymous, isHidden, authorName, authorUserId, likescount, createdAt(毫秒)`；匿名评论的 `authorName` 与 `authorUserId` 必须为 null。可搜索字段 `content, articleTitle, authorName`；可过滤字段 `isAnonymous, isHidden, authorUserId, articleDocumentId`；排序规则最后一项是 `createdAt:desc`。`config/plugins.ts:89-139`
- **CMT-115** SQL 兜底依赖评论正文全文索引 `ft_comments_content`（MySQL FULLTEXT；PG 为 `GIN(to_tsvector('simple', COALESCE(content,'')))`，查询表达式必须与索引一致），启动时自动创建，创建失败不阻断启动。`src/index.ts:425-458`、`src/api/comment/controllers/comment.ts:1503-1511`

### 4.6 实时

- **CMT-116** 评论创建提交后，异步向文章列表广播 `topic_bumped`（携带评论者 user id 作为触发者，触发者自己的列表不弹提示）；只广播已发布且未隐藏的文章；失败只记 warn。`src/api/comment/content-types/comment/lifecycles.ts:81-92,413-440`、`src/api/article/utils/article-broadcast-bus.ts:152-175`
- **CMT-117** 没有「新评论出现在帖子详情」的实时推送；评论区的实时性只依赖通知推送和客户端重新请求。`src/api/comment/content-types/comment/lifecycles.ts:70-94`（待确认，见 10）

### 4.7 AI 触发（只记录条件）

- **CMT-118** AI 回复评论的触发条件（每条评论最多入队一个任务 `{ type: 'comment', commentId }`）：全局 AI 开关未关闭（`globalEnabled !== false`）；评论、文章、parent 都不是匿名的；评论者本身不是 AI 用户（`isAiAgent !== true`）。目标 AI 的选择优先级：①正文前 10 个不同提及中，第一个属于 AI 作者且绑定了 user 的；②parent 作者是 AI；③文章作者是 AI（回复也会检查这一条）。目标 AI 的角色卡存在且 `commentEnabled !== false` 才入队。`src/utils/agent/triggers.ts:100-163`
- **CMT-119** 触发判定读取评论时，查不到会按 `[0, 50, 100, 200, 400, 800]` ms 退避重试（覆盖提交可见性延迟），仍查不到则记 warn 并放弃。任务写入 outbox 与评论处于同一事务。`src/utils/agent/triggers.ts:72-113`、`src/api/comment/content-types/comment/lifecycles.ts:78-80`

---

## 5 限流与防刷

- **CMT-120** 评论接口没有专门的频率限制，只有全站固定窗口限流（Redis，60 秒窗口）：匿名读 300 / 匿名写 60 / 登录读 600 / 登录写 120；Redis 不可用时放行。`config/middlewares.ts:57-70`、`src/middlewares/rate-limit.ts:1-35`
- **CMT-121** 防刷手段：表情数 ≤ 20（CMT-009）、图片数按等级封顶且在查库前拦截（CMT-021）、提及通知最多 10 个对象（CMT-018）、评论经验每天仅首次且不能在自己帖子下刷（CMT-101）、采纳经验按评论终身去重（CMT-102）、点赞去重唯一约束（CMT-107）。
- **CMT-122** 写操作（创建、点赞、置顶、采纳）都要求通过入站考试（CMT-027、CMT-037、CMT-105）。

---

## 6 隐私与匿名

- **CMT-123** 匿名评论的展示名为 `绳网用户#XXXX`，`XXXX` = `sha256(seed)` 的前 4 位 hex 转大写；seed 缺失时用 `anonymous-fallback` 派生。匿名作者对象只有 `{ documentId: null, name, avatar: null, level: null }`。`src/utils/anonymous.ts:6,16-48`；测试 `anonymous.test.ts`
- **CMT-124** 匿名评论对**所有人**都匿名，包括作者本人和管理员（读接口不存在「本人可见真名」的分支）。`src/utils/anonymous.ts:50-60`、`src/api/comment/controllers/comment.ts:72-91`
- **CMT-125** 每条匿名评论都有独立的随机 seed，与文章的 `anonymousSeed` 无关；同一用户在同一帖子下的多条匿名评论显示为不同的匿名名，匿名帖作者在自己帖子下的匿名评论也不会显示为「楼主」。`src/api/comment/controllers/comment.ts:635-636`
- **CMT-126** 文章是否匿名不会影响评论是否匿名：评论只有在自己带 `isAnonymous: true` 时才匿名；匿名帖作者以非匿名方式评论会暴露真实身份（前端是否有提示待确认）。`src/api/comment/controllers/comment.ts:635`
- **CMT-127** 非匿名作者对象字段：`id, documentId, name, login(username), avatar, level, isAiAgent`；头像优先使用装备头像（账号 `accountStatus` 为 `deleted`/`merged` 时不使用装备头像），否则使用作者头像，都转为缩略图。匿名评论不查询装备头像。`src/api/comment/controllers/comment.ts:47-91,1108-1128`、`src/utils/common.ts:445-472`
- **CMT-128** 拉黑过滤：观看者已登录时，排除「观看者拉黑的人」与「拉黑了观看者的人」写的**非匿名**评论；匿名评论不过滤（避免通过内容消失反推匿名作者）；文章作者被拉黑（非匿名文章）时整篇的评论都不可见。`src/api/comment/controllers/comment.ts:163-198,1014-1017`
- **CMT-129** 匿名评论、匿名帖、匿名 parent 都会抑制主通知和提及通知（CMT-090、CMT-092），匿名评论被点赞不发通知（CMT-110），采纳匿名评论或匿名帖中的评论不发通知（CMT-096）；目的都是不让通知暴露匿名身份。
- **CMT-130** 任何会下发评论的接口都不能 populate 并输出 parent 的作者信息（历史上曾经泄露匿名 parent 作者）。`src/api/comment/controllers/comment.ts:1416-1418`、`src/api/author/controllers/author.ts:715-718`
- **CMT-131** 搜索索引中匿名评论不存作者名与 user id（CMT-114）。

### 6.1 隐藏与审核

- **CMT-132** `isHidden` 只能由举报风控或管理员设置。同一评论的待处理举报中，不同举报人数达到阈值（环境变量 `REPORT_AUTO_HIDE_THRESHOLD`，默认 5）时自动隐藏，`hiddenReason = 'auto_report_threshold'`，并通知作者 `你的评论因收到多人举报已被暂时隐藏，等待管理员复核。`。`src/api/report/services/report.ts:173-227`、`src/api/report/utils/report-core.ts:78-82`
- **CMT-133** 隐藏评论时同时清掉 `pinnedAt` 和 `pinnedBy`，但**不**清 `acceptedAt`。`hiddenReason` 取值：`auto_report_threshold`、`admin`（管理员隐藏）、`admin_delete_pending`（管理员删除排队中）。`src/api/report/services/report.ts:123-166,284-295`
- **CMT-134** 管理员驳回举报时，只有 `hiddenReason === 'auto_report_threshold'` 的评论会恢复为可见（`isHidden=false, hiddenReason=null`）；管理员主动隐藏的不会被驳回恢复。`src/api/report/services/report.ts:337-341`
- **CMT-135** 管理员选择删除时先隐藏（`admin_delete_pending`），再由异步任务执行真正的删除，走与作者删除相同的级联逻辑。`src/api/report/services/report.ts:292-316`、`src/api/report/utils/report-actions.ts:113-114`
- **CMT-136** 被隐藏评论的影响：不出现在列表 / 搜索 / 回复中；它的回复也一并不可见（公开可见要求 parent 未隐藏）；不能被回复、置顶、采纳、点赞；回复分页返回 404。`src/utils/content-visibility.ts:16-35`、`src/api/comment/controllers/comment.ts:626-629,681,832,1266`

---

## 7 边界情况与已知坑

- **CMT-137** 公开可见判断必须把 `isHidden IS NULL` 视为未隐藏（历史数据）。但旧代码的列表、内联回复、回复分页使用 `isHidden: { $ne: true }`，在 SQL 中会被翻译成 `is_hidden <> true`，从而**排除** NULL 行；只有 `PUBLIC_*_FILTER` 是 NULL 安全的。重写时统一按 NULL = 未隐藏处理（如果历史数据确实存在 NULL，这是对旧行为的修正，见 10）。`src/utils/content-visibility.ts:9-12`、`src/api/comment/controllers/comment.ts:1022,371,1278`、`node_modules/@strapi/database/dist/query/helpers/where.js`（`$ne` 分支）；测试 `content-visibility.test.ts:41`
- **CMT-138** 删除一条已采纳的评论后，文章的 `solvedAt` 与 `acceptedCommentDocumentId` 不会被清理，文章仍显示「已解决」并指向一条不存在的评论。重写时建议删除时同步清理（待确认，见 10）。`src/api/comment/content-types/comment/lifecycles.ts:103-115`
- **CMT-139** 被隐藏的已采纳评论仍保留 `acceptedAt`，文章 `solvedAt` 也不变，但列表的采纳位不显示它（查询要求未隐藏）。`src/api/report/services/report.ts:150-154`、`src/api/comment/controllers/comment.ts:1067-1079`
- **CMT-140** 删除评论时，被删评论**自身**的点赞记录只是失去了与评论的关联，点赞行本身没有被删除（只有后代评论的点赞被显式删除）。重写时应一并删除（待确认）。`src/api/comment/content-types/comment/lifecycles.ts:726-771`
- **CMT-141** 历史数据中可能存在跨文章回复（回复与 parent 不在同一文章）、多层嵌套和环；读路径必须过滤掉它们（parent 必须是同一文章的顶层评论），删除时按 BFS 清理多层孤儿链（深度上限 32）。`src/utils/content-visibility.ts:120-185`、`src/api/comment/content-types/comment/lifecycles.ts:654-678`；测试 `content-visibility.test.ts:58`
- **CMT-142** 正文长度与表情数量是在裸 @ 展开**之前**检查的，展开后正文可能超过 2000 字符且不会再次校验。`src/api/comment/controllers/comment.ts:550-561`、`src/api/comment/content-types/comment/lifecycles.ts:17-29`
- **CMT-143** 裸 @ 正则会把 `@` 后面连续的字母数字（包括中文）整体当作候选词，所以 `@张三你好` 不会匹配到 `张三`；邮箱 `a@b.com` 中的 `b` 也会成为候选词，如果恰好有用户名为 `b` 的用户就会被改写。显示名含 `[`、`]`、换行或超过 40 个字符时，生成的 token 不符合提及正则，不会触发通知。`src/api/comment/utils/mention.ts:88,218`
- **CMT-144** 回复游标的时间部分必须先归一化成规范 ISO 再用于查询，否则 `Sep 1 2026` 之类的写法会让公开接口稳定返回 500。`src/api/comment/controllers/comment.ts:241-257`；测试 `comment-replies-paging.test.ts:801`
- **CMT-145** 拉黑过滤和游标条件各带一个 OR 组，必须用 AND 组合，不能合并到同一个对象里互相覆盖。`src/api/comment/controllers/comment.ts:1269-1280`；测试 `comment-replies-paging.test.ts:730`
- **CMT-146** 内联回复不能用「整页共享的 limit」实现，否则第一条评论会吃掉全部名额。`src/api/comment/controllers/comment.ts:328-345`；测试 `comment-replies-paging.test.ts:565`
- **CMT-147** 搜索 SQL 兜底的匹配窗口必须固定（1000），不能随 `start` 增长，否则 `total` 会随翻页变大。`src/api/comment/controllers/comment.ts:1487-1491`
- **CMT-148** 搜索结果中文章 `cover` 必须是单个对象或 null，不能是数组（客户端会整条解析失败）。`src/api/comment/controllers/comment.ts:148-161`；测试 `comment-search.test.ts:103`
- **CMT-149** 采纳 / 置顶的通知与经验必须在事务提交后执行，否则跨连接查询看不到新行，导致实时推送静默丢失。`src/api/comment/controllers/comment.ts:903-904`
- **CMT-150** 拉黑校验失败在旧代码中抛出普通 `Error`（`Cannot comment on blocked user`、`Cannot reply to blocked user`），会变成 HTTP 500；重写时应返回 4xx。而在控制器层，文章作者拉黑的情况已经先被 `findVisibleArticle` 拦成 400 `Article not found`。`src/api/comment/content-types/comment/lifecycles.ts:160-171`
- **CMT-151** 自采纳的判定是「答主没有 user，或答主就是操作者」；管理员采纳帖主自己的评论不算自采纳，会通知帖主并给经验。`src/api/comment/controllers/comment.ts:905-906`
- **CMT-152** 同一 documentId 的文章有 draft / published 两行；评论计数、bump、采纳投影都要写到全部版本行。文章重新发布时会重新挂回评论关系并全量重算 `commentscount`。「Strapi 特有，重写时不需要」：如果新设计文章只有一行，这些多行写入可以去掉。`src/api/comment/content-types/comment/lifecycles.ts:338-341,397-407`、`src/api/article/controllers/article.ts:185-210`
- **CMT-153** 「Strapi 特有，重写时不需要」：读取 query 参数时从 `ctx.request.url` 兜底（Koa 解析和缓存中间件改写 URL 导致参数丢失）；`/api/comments` 前缀的匿名响应会被缓存 30 秒，缓存规范化层对 `/comments/:id/replies` 的前缀匹配曾经剥掉 `cursor`。重写时只需保证：公开读接口如有缓存，缓存键必须包含 `cursor`/`limit` 等全部有效参数，且最长陈旧 30 秒是可以接受的。`src/api/comment/controllers/comment.ts:121-142`、`src/api/comment/routes/comment-replies.ts:1-27`、`config/plugins.ts:143-158`；测试 `comment-replies-paging.test.ts:898`
- **CMT-154** 「Strapi 特有，重写时不需要」：自定义 create 绕过核心 `validateInput`（`Invalid key article`）；201 状态码需要通过 `send` 的第二个参数设置；`db.query.deleteMany` 不触发 lifecycle，所以级联删除时要手动补偿点赞计数和通知；`deleteMany` 不支持嵌套关联条件，要先查 id 再删。`src/api/comment/controllers/comment.ts:536-539,651`、`src/api/comment/content-types/comment/lifecycles.ts:683-689`
- **CMT-155** 级联删除和取消点赞时要失效收件人的会话缓存（`knock:conv:<userId>` 以及 DM 会话列表两条读路径的缓存键），并在失效**之前**重算通知桶；否则未读数不下降或出现幽灵会话。重写时如果没有这些缓存，只需保证通知删除后未读数与会话列表立即正确。`src/api/comment/content-types/comment/lifecycles.ts:601-635`、`src/api/like/content-types/like/lifecycles.ts:352-393`

---

## 8 定时任务 / 对账

- **CMT-156** 每天北京时间 04:23 对账 `comments.likescount`：按评论 id 统计 `count(distinct like)`，只修正不一致的行（`COALESCE` 处理 NULL），记录修正行数；上一轮未结束时跳过本轮；单条口径失败不影响其他口径。`config/cron-tasks.ts:57-78,164-217,564-568`；测试 `interaction-count-increment.test.ts:633-766`
- **CMT-157** `commentscount` **没有**定时对账，只依靠增量 ±1、文章重新发布时的全量重算（`syncArticleCommentCounts`：按 documentId 统计全部评论并写回全部版本行），以及一次性回填迁移（按 documentId 统计 `COUNT(DISTINCT comment_id)`）。`src/api/comment/content-types/comment/lifecycles.ts:333-373`、`database/migrations/2026.06.30T00.00.00.backfill-article-commentscount.js`
- **CMT-158** 通知桶有每日对账（04:47）与每分钟补漏任务；评论删除导致的通知删除依赖即时重算（CMT-049），不依赖对账。`config/cron-tasks.ts:578-589`

---

## 9 测试中体现的关键用例

- **CMT-159** 采纳：非帖主非管理员被拒；孤儿评论（没有文章）直接 400，不查文章；采纳成功会撤销同帖旧采纳、设置当前、同步文章、通知答主并给 10 经验；帖主采纳自己的评论不通知、不给经验；匿名帖中采纳不通知但仍给经验；取消采纳不受分区开关限制，并清理文章投影；取消采纳同样要求帖主或管理员。`tests/unit/comment-accept.test.ts:76-229`
- **CMT-160** 置顶：孤儿评论被拒；置顶时按「文章行 → 评论行」顺序加锁，并清理同文所有其他置顶；等锁期间被隐藏的评论不会被置顶；并发取消置顶视为成功的空操作。`tests/unit/comment-pin.test.ts:71-175`
- **CMT-161** 内联回复与回复分页：5 条可见回复只内联前 3 条、`repliesCount = 5`；混入隐藏项时仍返回 3 条可见回复；`repliesCount` 扣除隐藏和拉黑但不扣匿名；多条顶层评论各自拿满 3 条；5 条回复按 limit 2 分三页，拼起来不重不漏；同一时间戳靠 documentId 破平；坏游标回到第一页；未登录 `liked` 全为 false。`tests/unit/comment-replies-paging.test.ts:464-898`
- **CMT-162** 搜索：cover 降成单对象、缺失时为 null；索引命中但回查全部失败时走 SQL 兜底；索引确实没有命中时返回空页。`tests/unit/comment-search.test.ts:103-156`
- **CMT-163** 创建边界：拒绝隐藏 / 不存在 / 被拉黑的文章，且文章查询必须带 `publishedAt` 非空条件；拒绝跨文章、已隐藏、嵌套的 parent；合法顶层回复写入 parent 并返回 201；有图无字可以创建；无字无图被拒。`tests/unit/content-write-boundaries.test.ts:72-118`
- **CMT-164** 可见性：NULL 隐藏标记视为可见；隐藏 / 草稿文章下的评论和隐藏 parent 的回复不可见；历史跨文章回复被剔除。`tests/unit/content-visibility.test.ts:41-67`
- **CMT-165** 表情工具：正则与前端一致；非 `ik-` 前缀或非法字符不匹配；去重保序；`[表情]` / `[表情:name]` 降级；不影响提及 token。`tests/unit/emote.test.ts`
- **CMT-166** 点赞：去重键使用 documentId 而不是数字 id，并 trim；`DuplicateInteractionError` 映射为 400；并发插入失败返回「已点赞」；评论计数按 id 原子增减，带下限和 NULL 兜底。`tests/unit/interaction-dedupe.test.ts:70-300`、`tests/unit/interaction-count-increment.test.ts:165-416`

---

## 10 未决问题

- **Q-01** 图片引用不校验上传者归属，也不校验 MIME，数字 id 不做存在性校验；重写时是否要求图片必须是当前用户上传的、且类型为图片？「待确认」（CMT-024）
- **Q-02** 删除已采纳评论时是否要清理文章 `solvedAt` / `acceptedCommentDocumentId`？旧代码不清理。「待确认」（CMT-138）
- **Q-03** 被隐藏的已采纳评论是否应清除采纳状态或文章 `solvedAt`？旧代码两者都保留。「待确认」（CMT-139）
- **Q-04** 删除评论时自身的点赞行是否应一并删除？旧代码只解除关联。「待确认」（CMT-140）
- **Q-05** 生产数据中是否存在 `isHidden IS NULL` 的评论？如果存在，旧列表实际上把它们隐藏了，重写按「NULL = 可见」会让它们重新出现。「待确认」（CMT-137）
- **Q-06** `comment_accepted` 经验计入「自发行为」每日 50 上限，而不是「被动互动」1000 上限，是否为有意设计？「待确认」（CMT-103）
- **Q-07** 重复采纳同一条评论会重复发送 `accepted` 通知并刷新 `solvedAt`，是否需要去重？「待确认」（CMT-058、CMT-096）
- **Q-08** 纯图片评论的通知预览：通知桶 / 会话列表给空串，DM 消息给 `评论了你的帖子 [图片]` / `回复了你的评论 [图片]`，新设计统一用哪一种？「待确认」（CMT-097、CMT-098）
- **Q-09** 新的评论 / 点赞都不会为匿名评论产生通知，`anonymous` 会话分组是否只服务于历史数据？「待确认」（CMT-100）
- **Q-10** 裸 @ 展开后正文超过 2000 字符时是否需要拒绝或截断？「待确认」（CMT-142）
- **Q-11** username / author.name 的精确匹配是否区分大小写取决于数据库排序规则，旧行为在 MySQL 下可能不区分大小写。「待确认」（CMT-016）
- **Q-12** 是否需要「新评论实时出现在帖子详情页」的推送？旧代码没有。「待确认」（CMT-117）
- **Q-13** 管理员是否应能直接删除任意评论（不通过举报流程）、帖主是否应能删除自己帖子下的评论？旧代码都不允许。「待确认」（CMT-035）
- **Q-14** 删除接口不要求通过入站考试，是否有意为之？「待确认」（CMT-036）
- **Q-15** 匿名帖作者以非匿名身份评论自己的帖子会暴露身份，前端是否有提示或默认匿名？后端没有相关规则。「待确认」（CMT-126）
- **Q-16** 评论是否需要专门的发帖频率限制（例如每分钟 N 条）？旧代码只有全站限流。「待确认」（CMT-120）
