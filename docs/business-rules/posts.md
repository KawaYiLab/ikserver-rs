# 帖子/委托（Article）内容生命周期 — 业务规则规格

## 范围

帖子（article，前端称「委托」）的创建 / 更新 / 草稿 / 发布 / 取消发布 / 丢弃草稿 / 删除；字段与限额；分区（category）与标签（tag）；匿名；隐藏与可见性；置顶；顶帖（bump）；采纳（solved）；浏览计数；我的文章列表；主页精选（profile pinned）；考试门控；所有权策略；文章上的计数器与相关丁尼/经验副作用；B 站视频信息代理。

搜索 / 信息流排序 / 热榜算法的完整规则属于相邻 spec，本文只收录与帖子生命周期直接相关的可见性与注入规则。

## 来源文件

- `server/src/api/article/controllers/article.ts`（3371 行，主控制器）
- `server/src/api/article/controllers/suggested.ts`
- `server/src/api/article/routes/*.ts`（全部路由声明）
- `server/src/api/article/content-types/article/{schema.json,lifecycles.ts}`
- `server/src/api/article/services/pinned.ts`、`services/unread-feed.ts`（部分）
- `server/src/api/article/utils/{follow-notification.ts,article-broadcast-bus.ts}`
- `server/src/api/category/{controllers/category.ts,content-types/category/schema.json,routes/category-list.ts}`
- `server/src/api/tag/{controllers/tag.ts,content-types/tag/{schema.json,lifecycles.ts},routes/tag-suggest.ts,services/{cache.ts,indexes.ts}}`
- `server/src/api/article-read/**`
- `server/src/api/article-hot-view/content-types/article-hot-view/schema.json`
- `server/src/policies/{is-owner.ts,is-author-self.ts}`
- `server/src/middlewares/{require-exam-passed.ts,optional-auth.ts,require-auth.ts,error-format.ts,rate-limit.ts}`
- `server/src/utils/{tags.ts,categories.ts,anonymous.ts,benefits.ts,denny.ts,level.ts,common.ts,content-visibility.ts,read-status.ts,article-hot-views.ts,owned-interaction.ts,suggested-articles.ts,cache-query.ts}`
- `server/src/api/me-profile/controllers/me-profile.ts`、`src/api/author/controllers/author.ts`（精选部分）
- `server/src/api/comment/controllers/comment.ts`（accept/unaccept）、`src/api/comment/content-types/comment/lifecycles.ts`（bump）
- `server/src/api/report/services/report.ts`（隐藏写入方）
- `server/config/{plugins.ts,middlewares.ts,features.ts}`、`src/index.ts`（索引/回填 bootstrap）
- 测试：`tests/unit/{anonymous,article-block-cap,article-detail,article-hot-views,article-pin,article-read-batch,article-tags,article-update-transaction,article-view-cooldown,content-visibility,content-write-boundaries,is-owner-policy,profile-pinned-articles,tags,tag-cache,tag-database,upload-content,suggested-articles,article-suggest}.test.ts`

---

## 1 数据与字段约束

### 1.1 Article schema（`src/api/article/content-types/article/schema.json`）

- `POST-001` `draftAndPublish: true`：同一 documentId 存在 draft 与 published 两个版本行（物理两行）。所有「按 documentId 更新所有版本行」的规则都源于此。`schema.json:10-12`
- `POST-002` `title`：string，required，minLength 1，maxLength 50（Strapi 按 JS 字符串长度，即 UTF-16 code unit 计）。`schema.json:15-20`
- `POST-003` `text`（正文，markdown/richtext）：非必填（2026-10-01 提交 `9b021a3` 放宽，允许纯图片帖），schema maxLength 8000；另有按等级的服务端限额（见 `POST-012`）。`schema.json:21-25`
- `POST-004` `editorState`：json，编辑器状态，仅服务于作者编辑流，公共详情不下发（`article.ts:2149-2150` 的 fields 不含它，注释见 `1049-1051`），只经 `myDetailLite` 返回。
- `POST-005` `externalVideos`：json，外部视频数组，最多 1 个（`MAX_EXTERNAL_VIDEOS = 1`），超限抛 400 `最多只能嵌入 1 个外部视频`。`article.ts:550,686-691`
- `POST-006` `cover`：media multiple，allowedTypes `["images"]`；实际数量上限按等级（见 `POST-012`）。`schema.json:32-39`
- `POST-007` `blocks`：dynamiczone，允许组件 `shared.media`（单 file）、`shared.quote`、`shared.rich-text`、`shared.slider`（多 files）。`schema.json:58-66`
- `POST-008` 计数器（全部 integer、default 0、min 0，均为服务端维护的反范式字段，客户端不可写）：`commentscount`、`likescount`、`views`、`denny_count`、`tripleCount`。`schema.json:85-99,138-147`
- `POST-009` 状态/审计字段：`isAnonymous` bool 默认 false；`isHidden` bool 默认 false；`hiddenReason` string private；`anonymousSeed` string private；`bumpedAt` datetime；`lastBumpedByUserId` integer private；`editedAt` datetime；`isPinned` bool 默认 false；`pinnedAt` datetime；`pinnedBy` manyToOne→user，private；`solvedAt` datetime；`acceptedCommentDocumentId` string private。`schema.json:106-167`
- `POST-010` mappedBy 反向关系：`likes` / `comments` / `favorites` / `article_reads` / `denny_received`（oneToMany）。发布/取消发布会删除并重建 published 行，这些关系会被级联删除，必须显式挂回（见 `POST-054`）。`schema.json:67-83,100-105,132-137`；`article.ts:148`
- `POST-011` `firstPublishedAt` 由 Strapi 特性开关 `experimental_firstPublishedAt` 注入：首次 publish 时写入，编辑重发保留原值；客户端传入一律剥离（伪造会被 Strapi 置空并连累草稿行原值）。`config/features.ts:2-7`；`article.ts:1207-1208,1307-1308`

### 1.2 等级权益矩阵（`src/utils/benefits.ts`）

- `POST-012` 权益等级 = `user.level` 夹到 [0,7]；`isAdmin === true` 或 `isAiAgent === true` 恒为 7。`articleMaxImages = [0,9,12,15,18,24,30,40]`（cover 图片上限，等级 0 = 未通过考试 = 0 张）；`articleMaxBody = [0,3000,4000,6000,8000,10000,13000,16000]`（正文字符上限）。`benefits.ts:6-15,19-34`
- `POST-013` 正文超限抛 400 `当前等级正文最多只能输入 ${maxBody} 字`；封面超限抛 400 `当前等级图片最多只能上传 ${maxCovers} 张`。两处校验都在 `create` / `update` / `publishMine` 里执行（发布时按草稿行再校验一次，防草稿期绕过）。`article.ts:537-548,1213-1221,2897-2899`
- `POST-014` 正文图片总扇出限额：遍历 `blocks`，`shared.media` 的 file 计 `min(1, 数量)`、`shared.slider` 的 files 全计，累计超过等级上限即抛 400 `当前等级正文图片最多只能上传 ${maxCovers} 张`（在做任何上传文件查库之前拒绝）。`article.ts:844-850`

### 1.3 外部视频（B 站）（`article.ts:552-707`）

- `POST-015` 识别正则：`BVID_RE = /^BV[0-9A-Za-z]{10}$/`；`AVID_RE = /^(?:av)?(\d+)$/i`；`BILIBILI_URL_RE = /(?:bilibili\.com\/video\/(BV[0-9A-Za-z]{10})|bilibili\.com\/video\/(?:av)?(\d+))/i`；分集用 `/[?&]p=(\d+)/`（p ≤ 0 丢弃）。先按 URL 解析，失败再按裸 BV/av 号解析。`article.ts:552-554,588-639`
- `POST-016` 存储形态：`{ provider:'bilibili', bvid, aid, cid, p, page(=p), embedUrl, coverUrl, title, duration }`。embedUrl = `https://player.bilibili.com/player.html?bvid=..&aid=..&cid=..&p=..&autoplay=0&danmaku=0&poster=1`（仅有值参数才带上，p>0 才带）。`article.ts:569-586`
- `POST-017` 输入归一化（`normalizeExternalVideos`）：数组元素可为字符串（URL/BV/av）、`{url}` 对象、或含 `bvid`/`aid` 的对象（走 `buildExternalVideo`，bvid 须过 BVID_RE、aid>0、p>0、cid>0、coverUrl 须 `^https?://`、duration ≥ 0）。非法元素静默丢弃；全部非法时结果为 `[]`（清空）。非数组输入返回 null（不动该字段）。`article.ts:641-707`

### 1.4 媒体引用解析（`article.ts:709-906`）

- `POST-018` cover 与 blocks 内的媒体引用接受：正整数 id、纯数字字符串、`{id}`、`{documentId}`、`{set:[...]}` / `{connect:[...]}`（取第一个元素）。documentId 反查 `plugin::upload.file`，查不到抛 400 `${fieldPath} references upload file "..." but it does not exist`；不支持形态抛 400 `${fieldPath} has an unsupported media reference format`。`article.ts:731-774`
- `POST-019` cover 在归一化前按「原始条目数」（数组长度 / set/connect 长度）先做一次上限拦截，超限在任何 DB 查询前抛错；归一化后再 assert 一次。`article.ts:832-843,855-864`

### 1.5 标签（Tag）

- `POST-020` Tag schema：`name` string required maxLength 30；`slug` string required maxLength 60 unique（DB 层必须有唯一约束，启动时自检/补建，补不上则拒绝启动）；`isBlocked` bool 默认 false private。`tag/schema.json`；`tag/services/indexes.ts:58-84`
- `POST-021` 名称归一化（`normalizeTagName`）：NFKC → 去除 Default_Ignorable 与控制字符 → 剥掉 `[^\p{L}\p{M}\p{N}\s_+#.-]` 之外的字符 → 折叠空白 → 截 30 个 UTF-16 单元且不切断代理对 → trim。结果必须含字母或数字（`HAS_ALNUM = /[\p{L}\p{N}]/u`），否则视为无效。`utils/tags.ts:8-23`
- `POST-022` slug 派生（`toTagSlug`）：归一化名称 → 小写 → NFKC → `[\s_.]+` 折成 `-` → 剥掉 `[^\p{L}\p{M}\p{N}+#-]` 之外字符 → 折叠/修剪 `-`。刻意保留 `+` 与 `#`：C / C++ / C# 是三个不同 slug。`utils/tags.ts:26-33`
- `POST-023` 发帖打标（`resolveOrCreateTags`）：`MAX_TAGS_PER_ARTICLE = 5`。输入接受名字数组 / `{name}` 对象数组 / set/connect 包装；非这些形态抛 400 `tags 必须是标签名称数组；使用空数组清空标签`；数组长度 >100 抛 `标签输入过多`；单项名称非字符串或 >256 抛 `标签必须包含长度不超过 256 的名称`。按 slug 去重保序，取前 5 个。`utils/tags.ts:5,36-58,90-106`
- `POST-024` 全部名称归一化后为空且原数组非空 → 抛 400 `标签名称必须包含字母或数字；使用空数组清空标签`；显式空数组 = 清空标签（合法）。`utils/tags.ts:107-110`
- `POST-025` 命中已存在且 `isBlocked` 的标签 → 抛 400 `标签「${slug}」已被停用`；且在任何新建之前先检查全部已存在标签（一次请求不部分创建）。`utils/tags.ts:63-69,113-122`
- `POST-026` 并发创建同一 slug：create 撞唯一约束后重查已提交的胜利者行并复用；重查不到则把原始错误上抛（绝不把故障吞成「标签静默丢失」）。`utils/tags.ts:71-87`
- `POST-027` Admin 后台写 tag 走 lifecycle 强制同一归一化：data 带 name 时 name/slug 同步重写；只改 slug 不带 name 直接 ValidationError `标签 slug 必须随名称派生，不能单独修改`；name 归一后为空抛 `标签名称必须包含字母或数字`。`tag/lifecycles.ts:6-19`
- `POST-028` 标签联想 `GET /tags/suggest`（匿名可读）：q 长度 ≤256，经 toTagSlug 归一后按 `slug $contains` 子串匹配，排除 isBlocked，按 slug 升序，limit 默认 8、上限 10（`parseLimit`，非数字回落默认）。返回 `[{name, slug}]`。`tag/controllers/tag.ts:4-29`
- `POST-029` 被封禁标签不出现在任何文章响应：详情/列表/搜索/我的列表 populate tags 一律带过滤器 `$or:[{isBlocked:false},{isBlocked:{$null:true}}]`（NULL 兼容旧数据），序列化层 `serializeTag` 再兜一次（isBlocked 或 name/slug 为空 → 丢弃）。`article.ts:67-73`；`utils/tags.ts:131-141`

### 1.6 分区（Category）

- `POST-030` Category schema：`name` string；`slug` uid（由 name 派生）；`order` int 默认 0；`isActive` bool 默认 true；`adminOnly` bool 默认 false；`allowCommentAccept` bool 默认 false；`description` text。无 draft/publish。`category/schema.json`
- `POST-031` `GET /categories/list`（匿名可读）：只回 `isActive === true`，按 `order asc, id asc`，字段 `{documentId,name,slug,order,adminOnly}`。`category/controllers/category.ts:13-29`
- `POST-032` 发帖/编辑时客户端传的 category 可为：裸字符串（documentId 或 slug，两者都试）、`{documentId}`、`{connect:[...]}`、`{set:[...]}`（取首元素）。解析不到、不存在或 `isActive === false` 一律视为「没传」——**不报错也不兜底**，不写/不改分区。`utils/categories.ts:36-81`
- `POST-033` `adminOnly === true` 的分区：非 `user.isAdmin` 用户在 create/update 时传入该分区 → 403 `Only administrators can post in this category`。`article.ts:1249-1251,1341-1343`
- `POST-034` `allowCommentAccept` 判定必须 `=== true`（Strapi 给存量表加列不会落 schema default，老分区读出 NULL）；帖子无分区视为未开启。详情下发的 `allowCommentAccept` 与 comment.accept 共用 `isCommentAcceptAllowed`。`utils/categories.ts:95-109`

### 1.7 阅读记录（article-read）

- `POST-035` `article_reads` 表：`user` + `article` + `isRead`(bool 默认 false) + `dedupeKey`（private、unique）。dedupeKey = `u${userId}:a${articleDocumentId}`，由 beforeCreate/beforeUpdate 写入；beforeCreate 还做一次应用层重复检查，重复抛 `DuplicateInteractionError('Article already marked as read')`。「存在记录即已读」，读取侧不看 isRead 字段。`article-read/schema.json`；`article-read/lifecycles.ts:8-43`；`utils/read-status.ts:130-135`
- `POST-036` `POST /article-reads/batch`（需登录）：body `articleDocumentIds`（或 `data.articleDocumentIds`）去空去重后截断到 100 个；空数组返回 `{data:[]}`。`markAsRead:true` 模式：任一 documentId 没有对应 article 行 → 整体 404 `Article not found`（先校验后写入）；只对缺失的并行创建；create 失败时回查确认目标状态已存在才算成功（并发重复不报错，无关 DB 错误必须上抛）。查询模式：返回存在记录的 `{articleDocumentId, isRead:true}` 列表。**不做已发布/未隐藏校验**（与单条 create 经 `findVisibleArticle` 不同）。`article-read/controllers/article-read.ts:10-112`
- `POST-037` 单条 create/update 走 `writeOwnedInteraction`：article 引用必须恰好一种形态（id / documentId / 单元素 set/connect，嵌套深度 ≤3），多形态混传抛 400 `Article must contain exactly one relation reference`；目标文章必须 `findVisibleArticle` 可见（已发布、未隐藏、非拉黑），否则 404；user 强制为当前登录者。`utils/owned-interaction.ts:8-41,59-120`

---

## 2 权限与前置条件

- `POST-040` 考试门控（`require-exam-passed`）：挂在 create / update / publish / discard-draft / unpublish / pin / unpin 路由上（注意：**DELETE 删除不挂**）。每次请求按 user id 重查 `examPassed`，通过后立即生效无需重新登录；`examPassed || isAdmin || isAiAgent` 放行，否则 403 `{code:'EXAM_REQUIRED', message:'需要先通过入站考试才能进行此操作'}`。未登录 401。`middlewares/require-exam-passed.ts:16-40`；`article/routes/article.ts:10-16` 等
- `POST-041` 创建（POST /articles，core create）：policy `is-author-self` —— 客户端 body.data.author 必须能解析出属于当前用户的 author（支持 id / documentId / connect[0] 形态），否则 403。控制器随后**不信任**该值，按当前用户反查 canonical author 注入（`findAuthorByUser`，查不到 400 `Author not found`），消除 id/documentId/connect 混传的歧义。`policies/is-author-self.ts:1-39`；`article.ts:1223-1227`
- `POST-042` 所有权（policy `is-owner`，挂在 update / my-detail / publish / discard-draft / unpublish / delete 上）：先查 published 行、查不到再查 draft（status 不受客户端 query 影响）。两种情形必须区分：文档不存在 → 抛 NotFoundError → **404** `草稿不存在或已被删除`；存在但无权（含「已发布的匿名帖」）→ return false → PolicyError → **403**（error-format 把 `Policy Failed` 翻成 `没有权限执行该操作`）。`policies/is-owner.ts:8-45`；`middlewares/error-format.ts:17-18,61-69`
- `POST-043` **已发布的匿名帖不可编辑/删除/取消发布**：is-owner 对 `isAnonymous && publishedAt` 非空的文档一律拒绝（403）；匿名草稿可编辑。这意味着匿名帖发布后作者自己也无法修改/下线/删除（deleteMine 控制器内还有一层自己的 ownership 校验，但 is-owner 先行拦截）。`policies/is-owner.ts:40-41`
- `POST-044` `ctx.state.user` 来源：`auth:false` 的公开读路由挂 `optional-auth`（有 Bearer 就解析，无效 token 按匿名放行；`rejectInvalidBearer:true` 的路由 —— /articles/list、/articles/search、/articles/suggested —— 无效 token 直接 401）。用户被 blocked 或 accountStatus ∈ {blocked, merged, deleted} 时视为未登录。`middlewares/optional-auth.ts:21-76`
- `POST-045` 写白名单：create/update 只接受 `{title, text, editorState, externalVideos, cover, blocks, isAnonymous}`（`ARTICLE_WRITABLE_FIELDS`），其余字段（views、各计数器、isPinned、pinnedAt、pinnedBy、bumpedAt、editedAt、isHidden、hiddenReason、anonymousSeed、firstPublishedAt、author、category、tags 关系键）一律剥离后再 validateInput。`article.ts:531-535,1194-1210,1292-1310`
- `POST-046` 置顶/取消置顶：`POST /articles/:documentId/pin|unpin`，`require-auth` + `require-exam-passed` + 控制器内 `user.isAdmin === true`，否则 403 `Only admins can pin articles` / `Only admins can unpin articles`。`article/routes/article-pin.ts`；`article.ts:3241-3245,3277-3281`
- `POST-047` 采纳（solved）：`comment.accept` 要求帖主或管理员，且分区 `allowCommentAccept === true`（管理员也不例外，403 `该分区未开启评论采纳`）；`unaccept` 刻意不校验分区开关（关区/挪区后存量答案仍须可撤）。`comment/controllers/comment.ts:837-852,936-940`

---

## 3 流程与状态机

### 3.1 创建与更新

- `POST-050` create：body 必须含对象形态 `data`，否则 400 `Missing "data" payload in the request body`。query `status` 仅认 `draft` / `published`，其他一律按 `published`。成功返回 201。`article.ts:1184-1192,1273`
- `POST-051` create 时：`isAnonymous === true` → 服务端生成 `anonymousSeed`（16 字节随机 hex，32 字符）；显式 `false` → 置 null 清残留；不传不动。客户端传的 anonymousSeed 永远剥离。`article.ts:1203-1204,1229-1235`；`utils/anonymous.ts:13-14`
- `POST-052` create 且直接发布：写 `bumpedAt = now()` 与 `lastBumpedByUserId = 发帖人 user id`；status=draft 不写。发布后触发发布事件（见 `POST-070`）。`article.ts:1237-1243,1268-1270`
- `POST-053` update（PUT /articles/:id）：category/tags 只在请求**显式携带**对应 key 时处理；tags 显式空数组 = 清空全部标签。匿名切换可逆：改回 true 时复用已有 seed，没有才新生成；改为 false 置 null。`article.ts:1293-1298,1324-1354`
- `POST-054` update 语义按目标 status 分支：目标 published 且已存在 published 行（编辑重发）→ 只写 `editedAt = now()`，**不顶帖**（不动 bumpedAt）；目标 published 且无 published 行（草稿首次发布）→ 写 bumpedAt + lastBumpedByUserId 并触发发布事件。`article.ts:1356-1374,1435-1437`
- `POST-055` `documents().update()` 对不存在的 documentId 返回 null 而不抛错：控制器必须兜 404 `Article not found`（历史上此处对 null 取 `.documentId` 的 TypeError 跑在请求域外、无 catch，导致整个 Node 进程退出 —— 已认证用户可触发的进程级 DoS）。重写时：更新不存在的帖子必须返回 404。`article.ts:1413-1424`
- `POST-056` 编辑重发前**先**读旧 published 行的 mappedBy 关系 id（likes/comments/favorites/article_reads/denny_received），update 后用独立事务 `connect` 回新行并重算 likescount / commentscount；该事务失败重试一次，两次都失败只记 error 日志放行（帖子已更新，不对外 500）。刻意不把 `documents().update()` 包进同一事务（Meilisearch 索引推送是无 timeout 的 HTTP，入事务会攥住行锁拖垮连接池 —— Strapi 特有机制，但「换版本行后必须挂回关系+重算计数」这条业务语义必须保留）。`article.ts:168-252,1400-1433`

### 3.2 发布 / 取消发布 / 丢弃草稿 / 删除

- `POST-057` publish（POST /articles/:id/publish）：按草稿行发布。发布前按当前等级重新校验 cover 数量与正文长度（草稿期可能绕过或等级下降）。已有 published 行 = 编辑重发：写 editedAt、不顶帖、不发关注通知；否则首次发布：写 bumpedAt + lastBumpedByUserId、触发发布事件。编辑重发同样做关系挂回（`POST-056` 语义，此处无重试，失败记 error）。返回 `{documentId, publishedAt, firstPublishedAt, hasPublishedVersion:true, isEdit}`。`article.ts:2882-2981`
- `POST-058` discard-draft（POST /articles/:id/discard-draft）：无 published 版本 → 400 `No published version to restore from`；否则丢弃草稿回滚到线上版。返回 `{documentId, hasPublishedVersion:true}`。`article.ts:2983-3009`
- `POST-059` unpublish（POST /articles/:id/unpublish）：body `discardDraft` 为真 → 先 discardDraft 再 unpublish；为假且当前无草稿行 → 先 discardDraft 生成一份草稿（否则 unpublish 后无行可挂关系，点赞/评论/收藏全部丢失）。unpublish 后把旧 published 行关系挂到剩余草稿行并重算计数（失败记 error 放行）。返回 `{documentId, unpublishedAt, hasPublishedVersion:false, draftPreserved, locale}`。`article.ts:3011-3075`
- `POST-060` delete（DELETE /articles/:id）：控制器内再验 ownership（非本人 403 `You can only delete your own articles`；无 author 400→notFound `Author not found`）。仅当存在已发布版本时扣 **10 丁尼**（action `spend_delete_post`，描述 `删除帖子: ${documentId}`，relatedType `api::article.article`）；纯草稿删除免费。扣款系统异常 → 500（请求原文 `扣除丁尼失败，请稍后重试`，经 error-format 5xx 统一遮蔽为 `Internal server error`）；余额不足 → 400 `丁尼余额不足`。删除失败且已扣款 → 退款 10 丁尼（action `refund`，relatedId `refund:${documentId}`，靠 denny-log 的 (user, action, related_type, related_id) 幂等检查保证只退一次）→ 500。成功返回 `{success:true, message, newBalance?}`。`article.ts:3077-3159`；`utils/denny.ts:151-251`
- `POST-061` bumpedAt 兜底（lifecycle beforeCreate/beforeUpdate）：写库时若行已发布（publishedAt 非空）且无 bumpedAt，且库中旧行也没有 bumpedAt，则 `bumpedAt = publishedAt`。保证 Strapi 后台直接建帖/发布也能进主信息流排序。bootstrap 另有一次性回填：`bumped_at = COALESCE(published_at, updated_at)` where null。`article/lifecycles.ts:12-32`；`src/index.ts:190-206`
- `POST-062` 新评论顶帖（comment afterCreate）：把该文章**所有版本行**的 `bumpedAt = now()`、`lastBumpedByUserId = 评论者 user id`（按 documentId updateMany），失败只 warn 不阻断评论。`comment/lifecycles.ts:375-411`
- `POST-063` 列表项下发 `bumpedAt`（ISO）与 `bumpedBySelf`（布尔：`lastBumpedByUserId === 当前登录用户 id`）；`lastBumpedByUserId` 原值绝不下发（避免泄露「谁顶的帖」）。`article.ts:1165-1173`
- `POST-064` 采纳状态机（comment.accept/unaccept，写 article 冗余投影）：accept 在事务内先 `SELECT ... FOR UPDATE` 锁 articles（按 document_id 全部行）与目标 comment 行；同帖其它已采纳顶层评论全部撤销（acceptedAt=null, acceptedBy 清空）；写 comment.acceptedAt/acceptedBy；再 updateMany 文章所有版本行 `solvedAt=now, acceptedCommentDocumentId=评论documentId`。unaccept 反向：清 comment 字段，且**仅当**文章当前 acceptedCommentDocumentId 正是这条评论才清 solvedAt/acceptedCommentDocumentId（防误清并发采纳的另一条）。`comment/controllers/comment.ts:854-1001`

### 3.3 我的文章 / 详情

- `POST-065` `GET /articles/my`（需登录）：当前用户 author 的 **draft 状态**文档（即含已发布帖的最新草稿行），`updatedAt:desc`，字段不含 text/editorState；附加 `hasPublishedVersion`。**过滤掉已发布的匿名帖**（不可编辑，见 `POST-043`），未发布的匿名草稿保留。`article.ts:2569-2645`
- `POST-066` `GET /articles/my/drafts`（需登录）：纯草稿 = 排除有任何 published 版本的 documentId；每项 `text` 截断到 80 字符（`PREVIEW_LIMIT`）。`article.ts:2647-2725`
- `POST-067` `GET /articles/my/published`（需登录）：published 状态、`updatedAt:desc`，且**排除匿名帖**（`$or:[{isAnonymous:{$null}},{isAnonymous:false}]`，与主页口径一致），`hasPublishedVersion:true`。`article.ts:2727-2808`
- `POST-068` `GET /articles/my/detail/:id`（需登录 + is-owner）：返回 draft 行的编辑回填数据，含 `editorState`、`externalVideos` 原始数组、cover 带 documentId 的完整列表、`hasPublishedVersion`（published 行 count>0）。这是唯一暴露 editorState 的端点。`article.ts:2810-2880`
- `POST-069` 公共详情 `GET /articles/detail/:documentId`（匿名可读）：只查 published 行；`isHidden === true` 且非作者本人 → 404；非匿名帖作者与观看者互相拉黑 → 403 `{code:'USER_BLOCKED', message:'无法查看该委托', details:{authorDocumentId, authorName}}`。返回字段见 `buildArticleDetail`：documentId/title/text/cover(全部,带小图)/views/likesCount/commentsCount/liked/favorited/favoritesCount(实时 count)/dennyCount/hasGivenDenny/isRead/isAnonymous/isHidden/isPinned/pinnedAt/solvedAt/isOwner/createdAt/updatedAt/editedAt/publishedAt/firstPublishedAt/author/category/allowCommentAccept/tags/externalVideos。登录时并发查「我是否已点赞/投丁尼/收藏/读过/关注作者」5 项；匿名帖不查关注态。`article.ts:908-1052,2140-2199`

### 3.4 发布事件（首次发布）

- `POST-070` 首次发布（create 直接发布、update 草稿转发布、publishMine 首次）在事务提交后（runAfterCommit + setImmediate）触发两件尽力而为的副作用：①关注者扇出入队 `notifyFollowersOnPublish`；②SSE 广播 `publishTopicEventByDocumentId(documentId, 'new_topic')`。失败仅 warn。`article.ts:128-141`

---

## 4 副作用

- `POST-071` 关注者通知（首次发布）：入队 Redis 列表 `ik:fanout:jobs`，cron 每分钟 drain（见 §8）。通知为 system 类型，文案 `你关注的 ${authorName} 发布了新委托《${title}》`（authorName/title 各截 100 字符，message 截 500）。过滤：文章已删/下线/隐藏/匿名/无作者 → 任务作废；接收者排除 blocked、未 confirmed、isAiAgent、作者本人、与作者互相拉黑者。幂等键 `ik:fanout:sent:article:${documentId}:follower:${userId}` TTL 7 天 —— 按 documentId 而非行 id，「下线→重新上线」不会给同一人重复发。落库失败必须释放幂等键并抛错重试。`utils/follow-notification.ts:56-231`
- `POST-072` SSE 广播：事件类型 `new_topic` / `topic_bumped`（评论顶帖时由 comment 侧发后者）。频道：`latest` 全站 + `c:${categorySlug}` + 每个可见 tag 的 `tag:${slug}`；载荷只含公开字段 `{type, topicId, categorySlug, tagSlugs, at}`。只广播未隐藏的已发布帖；tagSlugs 排除 isBlocked。actorUserId 用于「不把事件推回触发者本人的连接」；`new_topic` 的 actor 缺省取文章作者 user id（**匿名帖也一样按真实作者排除**）。`utils/article-broadcast-bus.ts:20-188`
- `POST-073` 经验：首次发布 +4 exp，action `create_article`，relatedType `api::article.article`，relatedId = documentId，`dedupMode:'daily'`（每日仅首次发帖得经验）。afterCreate 只处理「新建即发布」；afterUpdate 兜底草稿转发布，且先做实体级检查（该 documentId 已有 create_article 日志则跳过，区分编辑 vs 首次发布）。另受每日上限约束（self_initiated 桶）。`article/lifecycles.ts:43-114`；`utils/level.ts:62-114`
- `POST-074` 计数器维护方：`likescount` / `commentscount` 由 like / comment 生命周期全量重算（reattach 时也重算这两项）；`views` 由 view 端点按 document_id 对所有版本行 `INCREMENT`；`denny_count` 由投丁尼流程 `COALESCE(denny_count,0)+1`；`tripleCount` 由三连流程维护。favoritesCount 无反范式字段，详情页实时 count。`article.ts:204-209,939-948,3225-3238`；`utils/denny.ts:399-427`
- `POST-075` 删除文章的清理（afterDelete）：仅当该 documentId 已无任何版本行时，把该 documentId 从作者的 `profilePinnedArticles` 数组中剔除（发布/丢弃草稿替换单行不触发）。失败仅 error 日志。`article/lifecycles.ts:116-172`
- `POST-076` 隐藏联动（举报风控写入，非本域端点但须保持一致）：隐藏文章时同事务写 `isHidden:true, hiddenReason, isPinned:false, pinnedAt:null` 并逐行清 `pinnedBy`；举报被驳回时仅对 `hiddenReason='auto_report_threshold'` 的行解除隐藏。**注意不主动失效置顶 Redis 缓存**（靠列表 NOT_HIDDEN 过滤剔除，最长 60s 延迟）。`report/services/report.ts:123-166,337-341`
- `POST-077` 搜索索引（Meilisearch，strapi-plugin-meilisearch documents 中间件）：只索引 published 行；索引字段 title + text（去 HTML/Markdown 标记、截 3000 字符）+ isAnonymous + isHidden + authorName/authorUserId（匿名帖两者均为 null，防索引泄露身份）+ categoryName/categorySlug + views/likescount/commentscount + updatedAt(ms)。settings：searchable `[title,text,authorName,categoryName]`，filterable `[categorySlug,isAnonymous,isHidden,authorUserId]`，sortable `[updatedAt,views,likescount]`，rankingRules 尾部带 `updatedAt:desc`。搜索/联想在 Meilisearch 命中但 DB 回查全灭（全是索引残留）时降级到 SQL 兜底，不返回假空。`config/plugins.ts:6-82`；`article.ts:2289-2307,2512-2522`
- `POST-078` 响应缓存失效：tag 或 article 的写（含 updateMany 审核）提交后清 `strapi-cache` 中 `/api/articles` 前缀的缓存（setImmediate 合并同一 tick 多次失效；草稿自动保存、浏览量自增不清 —— 守卫条件：update 且 data 不含 `tags|publishedAt|isHidden|documentId` 任一键则跳过，且 result.publishedAt 为 null 的非发布写跳过）。新建游离 tag（afterCreate）不清。`tag/services/cache.ts:22-57`
- `POST-079` 响应缓存前提（重写时可重设计）：`/api/articles*`、`/api/categories/list` 等 GET 走 30s Redis 响应缓存，**仅匿名请求**（`cacheAuthorizedRequests:false`）；缓存 key 经 query 白名单归一化防垃圾参数撑爆 Redis。`config/plugins.ts:143-176`；`utils/cache-query.ts:14-55`

---

## 5 限流与防刷

- `POST-080` 浏览量端点 `POST /articles/:documentId/view`（匿名可调）：
  - 身份键：登录 `u:${userId}`，匿名 `ip:${getClientIp(ctx) || 'unknown'}`。
  - 全局限流：`view_gl:${clientKey}` 固定窗口 60s 内 30 次（`incrWithTtl` 原子自增+TTL），超限返回 `{ok:true}`（不计数但 200）；该层 Redis 出错 fail-open。
  - 冷却窗口：`view_cd:${clientKey}:${documentId}`，`SET key 1 EX 300 NX` 原子抢占（`VIEW_COOLDOWN_SEC = 5*60`），未抢到返回 `{ok:true}`；抢到后文章不存在 → 404 且**冷却键不删**（挡「反复上报不存在的 id」）。这层 Redis 抛错直接上抛 500（fail-open 会导致全站重复计数，更糟）。
  - 计数：`UPDATE articles SET views=views+1 WHERE document_id=?`（draft + published 两行一起加，防编辑重发清零）。返回 `{ok:true, counted:true, views: 校验时读到的值+1}`（并发下可能瞬时偏小，不回读）。
  - 文章条件：`publishedAt` 非空且 ≤ now、`isHidden != true`。`article.ts:255,3161-3239`
- `POST-081` 全局限流中间件（对所有 /api/* 生效）：Redis 固定窗口 60s，键 `rl:r|w:u:${userId}` 或 `rl:r|w:ip:${ip}`；配置值：匿名读 300/min、匿名写 60/min、登录读 600/min、登录写 120/min。超限 429 带 `Retry-After` 与 `X-RateLimit-*` 头；Redis 不可用 fail-open。固定窗口边界可放行 2× 是已知取舍。`config/middlewares.ts:57-67`；`middlewares/rate-limit.ts:30-36,121-131`
- `POST-082` 拉黑排除集合规模防御：排除作者数 > `BLOCK_EXCLUDE_DB_CAP = 200` 时，主信息流的无界分页查询不再带 `$notIn`，改为多取 `BLOCK_OVERFETCH_FACTOR = 3` 倍后应用层过滤（代价：可能短页、total 略偏大，记 warn 留痕）；其余查询（热榜/置顶/收藏/搜索）仍走 DB `$notIn`。`article.ts:389-395,440-486`
- `POST-083` SSE 总线全局连接上限 `MAX_TOTAL_SUBSCRIBERS = 5000`，满则端点回 503 + `Retry-After: 30`；心跳 25s；初始发 `retry: 5000` 与 `hello` 事件。`utils/article-broadcast-bus.ts:41`；`article.ts:1443-1526`
- `POST-084` 列表 `ids` 增量参数：必须是不超 6500 字符的逗号串，每项匹配 `/^[A-Za-z0-9_-]{1,128}$/`，去重后 ≤50 个，否则 400 `Invalid article ids` / `At most 50 article ids are allowed`；且只允许 feed=all + sort=latest，否则 400 `Article updates require the latest feed`。`article.ts:1529-1541,1577-1579`
- `POST-085` 搜索 SQL 兜底窗口固定 `SEARCH_FALLBACK_MATCH_CAP = 1000` 个 documentId（不随 start 变化，防 total 随翻页漂移）；单字查询（q.length < 2）不走 SQL 兜底直接返回空（`LIKE '%x%'` 全表扫描）；MySQL 布尔全文运算符 `+ - > < ( ) ~ * " @` 一律剥成空格（`sanitizeFulltextQuery`）。`article.ts:353,370-371,2309-2314,2331-2355`
- `POST-086` 联想 `GET /articles/suggest`：limit 默认 8、夹到 [1,10]；空 q 返回空；DB 兜底只按标题 `$containsi`，单字不接。`article.ts:2439-2457,2524-2548`
- `POST-087` 通用分页：`parsePagination(start, limit)`，start<0 → 0，limit<1 → 默认 20，上限 50。`utils/common.ts:269-286`

---

## 6 隐私与匿名

- `POST-090` 匿名展示：`anonymousSeed` 为服务端持有的 16 字节随机 hex，绝不回传客户端（schema private；写入路径一律剥离客户端传入值；读取靠 db.query 单独反查回填，因为 documents API 的 fields 不允许 private 字段）。`utils/anonymous.ts:13-14`；`article.ts:496-525,1203-1204`
- `POST-091` 匿名名派生：`sha256(seed).hex` 取前 4 位大写，展示名 `绳网用户#XXXX`；seed 缺失回退固定占位 `anonymous-fallback`（保证确定性不崩溃）。`buildAnonymousAuthor` 输出 `{documentId:null, name, avatar:null, level:null}`，防止跳转真实主页或头像反查。`utils/anonymous.ts:6,16-48`
- `POST-092` `shouldAnonymize`：**匿名帖对所有人（包括作者本人）一律匿名化**，无例外。`utils/anonymous.ts:51-60`
- `POST-093` 匿名帖的作者关注态不解析（详情 `authorFollowed` 恒 false，匿名作者的关注关系能反推身份）；匿名帖不参与作者装备头像查询。`article.ts:937-938,983-991,922-925`
- `POST-094` 拉黑过滤对匿名帖一律放行：DB 侧 `$or:[{isAnonymous:true},{author:{$notIn:...}}]`，应用层 `rejectBlockedItems` 同义。否则「内容消失」能反推匿名作者身份。`article.ts:426-434,481-483`
- `POST-095` 匿名帖不进搜索索引的作者名/作者 userId（`POST-077`）；联想接口以 DB 为准取匿名态（索引滞后窗口期不泄露）。`article.ts:2469-2509`
- `POST-096` 主页文章列表（`GET /profiles/:documentId/articles`）对**所有人包括作者本人**隐藏匿名文章；被隐藏（isHidden）的帖子仅作者本人可见；作者与用户互相拉黑 → 返回空列表；`user.profileHidden` 且非本人 → 空列表。`author/controllers/author.ts:439-469`
- `POST-097` 可见性谓词（跨域共用）：公开文章 = `isHidden ∈ {false, null}`（NULL 兼容旧数据）且 publishedAt 非空；`findVisibleArticle` 额外要求非互相拉黑，作者本人可选豁免 isHidden（`allowOwnerHidden`）。`utils/content-visibility.ts:9-15,98-118`
- `POST-098` 详情/列表/搜索/联想/相关委托/置顶池全部排除 `isHidden` 内容（`NOT_HIDDEN_FILTER = {isHidden:{$ne:true}}`）；置顶池与热榜在缓存外再按当前观众过滤一次（缓存是全局视角）。`article.ts:258`；`services/pinned.ts:40-51`
- `POST-099` 相关委托（`GET /articles/suggested/:documentId`，Discourse suggested topics）：种子帖必须当前可见（含 publishedAt ≤ now）否则 404；登录用户先取「未读」帖（同分区优先、按 bumpedAt 倒序、近 90 天），剩余名额从近一年随机池（3000 条、bumpedAt 倒序、Redis 缓存 600s）补齐，同分区优先；两步都排除种子本身与查看者自己的帖子（含匿名自己帖，按 author.user.id 比对）；limit 默认 6、上限 10；响应带 `Cache-Control: private, no-store` + `Vary: Authorization`。`utils/suggested-articles.ts:11-169`；`controllers/suggested.ts`

---

## 7 边界情况与已知坑

- `POST-100` 编辑重发**不顶帖、不重复通知、不改 firstPublishedAt**，只写 editedAt；首次发布才 bumpedAt + 通知 + 广播。三处入口（create 直接发布 / update status=published / publishMine）判定口径一致：「已存在 published 行数 > 0」。`article.ts:1356-1374,1435-1437,2901-2950`
- `POST-101` 更新不存在的 documentId 必须 404，不能把「目标已消失」伪装成 200 + data:null（前端自动保存会持续往死 id 上写），更不能在空指针上崩进程（历史事故，见 `POST-055`）。
- `POST-102` 关系挂回是发布后补偿：Strapi 的 publish/unpublish/编辑重发会删旧 published 行并新建行，mappedBy 关系被级联删除；挂回失败只能记 error + 日志带 id 列表供手工修复（不再回滚主操作）。「Strapi 特有，重写时不需要」——但「发布/下线/编辑不得丢点赞、评论、收藏、阅读、投丁尼关系与计数」这条业务语义必须保留。`article.ts:177-252`
- `POST-103` 浏览量只累加在 published 行会被编辑重发清零 —— 必须按 documentId 打所有版本行。like/comment/denny 计数同构。`article.ts:3225-3230`
- `POST-104` 正文 schema maxLength 8000 与等级上限（最高 16000）冲突：等级 ≥4 的用户服务端校验放行但 Strapi 实体校验可能仍按 8000 拦截。「待确认」是否历史遗留未生效上限；重写时应以等级矩阵为准并明确单一上限。`schema.json:24`；`benefits.ts:14`
- `POST-105` 丁尼扣款无幂等键：deductDenny 不做 (related_type, related_id) 幂等检查，并发双击「删除」可能扣两次 10 丁尼（第二次删除因文档已删而失败、触发一次 refund，但窗口期内两笔扣款都已发生）。重写建议给 `spend_delete_post` 加幂等。「待确认」是否接受该现状。`article.ts:3109-3158`；`utils/denny.ts:177-238`
- `POST-106` 经验实体级检查漏洞：用户当天第 2 篇及以后的帖子因 daily 去重不落日志，其 afterUpdate 的「该 documentId 已有日志」检查为假 —— 次日编辑该帖重发会触发一次 `create_article` 经验（占用次日额度）。属现存怪异行为，重写时可修正但需产品确认。`article/lifecycles.ts:73-114`
- `POST-107` SSE `new_topic` 对匿名帖也按真实作者 actor 排除（`uid` 是客户端明文自报的订阅参数）：遍历 uid 观察「收没收到推送」理论上可探测匿名帖作者。「待确认」是否接受（设计注释只论证了伪造无收益，未讨论该探测面）。`article.ts:1456-1461`；`article-broadcast-bus.ts:169-178`
- `POST-108` 置顶帖注入按分类页不做分类过滤：listLite 中 pinnedIds 只按拉黑过滤后直接注入任意 category 页顶部（代码注释自称「必须满足当前分类和可见性条件」但实现未按分类过滤；tag 页与 cursor 后续页确实不注入）。「待确认」是有意（全站公告）还是 bug。`article.ts:1871-1937`
- `POST-109` 游标分页：cursor = base64url(`${bumpedAtISO}|${documentId}`)，排序恒为 `bumpedAt desc, documentId desc`；非法/伪造 cursor 一律当作没传（退化成第一页，绝不 400/500）；时间半截必须过形态正则 `/^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}/` + `Date.parse` + 规范化 toISOString 三道（防畸形 token 在全站最热匿名接口上刷 500）。置顶/热门只在第一页注入，后续页照旧排除。`article.ts:260-343,1964-2136`
- `POST-110` 已读水位（read_watermark_at）默认关闭（`READ_WATERMARK_ENABLED` 未开启时 `buildReadDocIdSet` 逐字走旧的单次 IN 查询）；水位比较只认 publishedAt/firstPublishedAt 的最大值，刻意不认 bumpedAt/updatedAt；归档水位之前的阅读记录**目前不安全**（详情页与 batch 查询仍是纯「存在记录即已读」）。「全部标为已读」端点未接线（`advanceReadWatermark` 无调用方）。`utils/read-status.ts:14-258`
- `POST-111` article_hot_views（热门 UV 表）当前**无写入方**：`recordRecentArticleView` 在 Gorse 移除（提交 `6c77aa9`）时从 view 端点摘除，只剩 `hot-ranking.ts` 的死读路径（computeRecentHotRanking 无调用方）。表、索引自愈（`article_hot_views_dedupe_key_unique`）、prune（保留 ≥7 天、每小时最多清一次）仍在。重写时该子系统可整体删除或按原设计恢复。「待确认」。`utils/article-hot-views.ts`；`services/hot-ranking.ts:83-120`；`src/index.ts:372-385`
- `POST-112` 热榜/置顶缓存里可能有已隐藏/已拉黑内容：取卡片时一律再过一次 NOT_HIDDEN + 当前观众拉黑过滤，缓存本身不做按用户视图。「隐藏/删除后最长一个缓存 TTL（置顶 60s，key `pinned:articles:v1`）才消失」是已接受语义。`services/pinned.ts:18-22,74-105`；`article.ts:1909-1929`
- `POST-113` 404 语义区分：is-owner 对「不存在」必须 404（NotFoundError，中文文案 `草稿不存在或已被删除`）而非 403 —— 前端要对已删除的 id 停止自动保存重试；策略拒绝（无权 / 已发布匿名帖）才是 403。`policies/is-owner.ts:32-41`；`tests/unit/is-owner-policy.test.ts:42-96`
- `POST-114` 主页精选（profile pinned articles）：存储在 author.profilePinnedArticles（documentId 数组，null=未配置=默认展示最新 6 篇，[]=主动清空一篇不展示）。上限 `PROFILE_PINNED_MAX = 6`（PUT 超限 400 `At most 6 articles can be pinned`）；输入归一化：只收字符串数组或 null（其他 400 `pinned must be null or string[]`）、trim、去空、去重保序；保存时剔除失效项（已删/撤回/转匿名）而不是整单失败；候选与校验都只认**本人、已发布、非匿名**帖子（不过滤 isHidden）。GET 候选分页 limit 默认 50 上限 100，q 截 50 字符按标题 `$containsi`，已选卡片只在 start=0 时附带。`me-profile/controllers/me-profile.ts:12-48,264-427`
- `POST-115` B 站信息代理 `GET /articles/bilibili-info?bvid=|aid=`（匿名）：转发 `https://api.bilibili.com/x/web-interface/view`，timeout 8s，固定 UA + `Referer: https://www.bilibili.com`；上游 code ≠ 0 → 404 `无法获取该 B 站视频信息`；网络异常 → 500；`pic` 的 `http://` 强制改 `https://`；返回 `{bvid, aid, title, pic, duration, cid, videos, pages:[{cid,page,part,duration}], owner:{name,mid}}`。缺参 400 `缺少 bvid 或 aid 参数`。`article.ts:3312-3367`
- `POST-116` 「Strapi 特有，重写时不需要」清单：category/tags/author 关系键在 validateInput 前剥离再注入（`article.ts:1194-1210`）；documents API 的 `status: 'draft'|'published'` 双行模型及 publish 删旧建新语义；`fields` 不允许 private 字段导致的 anonymousSeed 反查（`article.ts:488-525`）；Meilisearch 中间件无 timeout 导致的事务拆分（`article.ts:1376-1399`）；Strapi 查询层 `limit:0` 退化为全量查询的防御（`article.ts:2034-2039`）；uid 字段 slug 由 name 派生（category）；`updateMany` where 带关系过滤的查询层 bug 绕行。

---

## 8 定时任务 / 对账

- `POST-120` 关注者扇出 drain：cron 每分钟一次 `drainFollowFanoutQueue`。单把 Redis 锁互斥（TTL 120s）；单轮上限：时间 15s（`FOLLOW_FANOUT_TIME_BUDGET_MS`）、通知 500 条、任务 50 个；批 100 条（游标按 follow.id 推进，整批完成才推进）；失败重试 LPUSH 回队首（天然 1 分钟退避）；永不向 cron 调用方抛错。`utils/follow-notification.ts:56-71,376-452`；`config/cron-tasks.ts`
- `POST-121` bootstrap 兜底（启动时幂等执行）：`bumpedAt` 空值回填（`POST-061`）；热路径索引补建（`idx_articles_bumped (bumped_at, id)`、`idx_hot_views_recent`、`article_hot_views_dedupe_key_unique` 等）；tag slug 唯一约束自检（失败拒绝启动）。`src/index.ts:190-206,354-418`；`tag/services/indexes.ts`

---

## 9 测试中体现的关键用例

- `tests/unit/is-owner-policy.test.ts`：不存在 → NotFoundError → 404 且中文文案；未登录/无 id/已发布匿名帖/他人帖 → 403；?status=draft 不能绕过 published 所有权检查；PolicyError 不记 error 日志。
- `tests/unit/content-write-boundaries.test.ts`：create/update 剥离全部服务端字段后才 validate；歧义 author 输入归一为当前用户；cover 41 张在任何 DB 查询前被拒；两个 slider 共 42 图累计超限被拒。
- `tests/unit/article-update-transaction.test.ts`：先读旧关系再换行；挂回+两次计数重算同一事务；换 published 行不进事务；挂回失败整段回滚重试一次、两次失败记 error 不 500；首次发布不读旧关系、通知留在事务外；目标不存在三路径均 404。
- `tests/unit/article-view-cooldown.test.ts`：SET NX 抢占成功才计数且只发一次 Redis 命令；冷却中短路不碰 DB；并发只计一次；自增按 document_id 覆盖两行；游客按 IP；冷却 Redis 抛错不计数；404 不删冷却键；全站限流先行拦截；限流计数器抛错 fail-open。
- `tests/unit/article-pin.test.ts`：非管理员拒绝；pin 更新该 documentId 全部行（isPinned/pinnedAt/pinnedBy）；隐藏帖不可 pin（400）；不存在 404；unpin 清三字段；未 pin 的 unpin 400。
- `tests/unit/article-read-batch.test.ts`：批量写入幂等、先验存在再写、并发重复经回查确认、写失败且回查失败不谎报已读、兼容旧 body 信封、需登录。
- `tests/unit/article-tags.test.ts`：tag=all 是真实标签过滤且不注入置顶/热门；tag 过滤兼容游标分页；五个端点 populate tags 均带可见性过滤；tags 字段缺省不动、显式空数组清空；非对象 data 400 不 500。
- `tests/unit/tags.test.ts` / `tag-database.test.ts`：NFKC、全角转半角、剥 emoji/零宽字符、纯符号拒绝、30 字截断不劈代理对；slug 小写化、空白/下划线/点折 `-`、C/C++/C# 互不相同；并发创建收敛到同一行；blocked 标签先验拒；suggest 子串匹配排除 blocked 且有界。
- `tests/unit/anonymous.test.ts`：seed 32 位 hex 且近似唯一；同名派生确定性；`绳网用户#` + 4 位大写 hex；seed 缺失回退稳定占位；buildAnonymousAuthor 只暴露匿名名；匿名对所有人含作者生效。
- `tests/unit/article-block-cap.test.ts`：超 200 阈值改应用层过滤、多取 3 倍截回页大小、total 偏大口径不变、匿名帖不过滤、整页被拉黑返回短页。
- `tests/unit/article-detail.test.ts`：详情返回全部 cover 的小图 URL；详情下发 allowCommentAccept。
- `tests/unit/profile-pinned-articles.test.ts`：候选分页报 total；q 过滤；已选卡片按保存顺序返回（即使在首页之外）；失效 id 保存时剔除而非整单失败；6 篇上限仍强制；主页精选模式报 articleTotal；scope=all 忽略精选并回传 pinnedIds。
- `tests/unit/article-hot-views.test.ts`（死代码参考）：UV 去重键 `sha256([articleDocumentId, viewerHash])`、viewerHash = `HMAC-SHA256(secret, ['article-hot-view:v1', documentId, identity])`；自看/AI/blocked/非 active/未知身份不计；乱序请求不回退 lastViewedAt；保留 ≥7 天、每小时最多 prune 一次。
- `tests/unit/tag-cache.test.ts`：tag/article 写提交后合并清 `/api/articles` 缓存；草稿自动保存与浏览量自增不清；显式 unpublish 清；批量审核隐藏清。
- `tests/unit/content-visibility.test.ts`：可见性谓词保留 NULL 旧标记；跨文章回复被剔除。
- `tests/unit/suggested-articles.test.ts` / `article-suggest.test.ts`：未读优先同分区、游客跳过未读层；种子非法 404；limit 夹 [1,10] 默认 6；索引残留全灭时落 DB 兜底而非假空；单字不落兜底。

---

## 10 未决问题

1. `POST-104`：text 的 schema maxLength 8000 vs 等级矩阵最高 16000，实际上限以哪个为准？「待确认」。
2. `POST-105`：删帖扣 10 丁尼无幂等，并发可双扣；重写是否加幂等键？「待确认」。
3. `POST-106`：次日编辑「当天第 N 篇（未落日志）」的帖子会补发 create_article 经验，是否修正？「待确认」。
4. `POST-107`：SSE 匿名帖 actor 排除可被遍历 uid 探测作者，是否接受？「待确认」。
5. `POST-108`：置顶帖注入分类页不按分类过滤，代码与注释矛盾，是有意还是 bug？「待确认」。
6. `POST-111`：article_hot_views 记录链路已断开（只有读没有写），重写是整体删除还是恢复记录？「待确认」。
7. `POST-114`：主页精选 PUT 校验不剔除 isHidden 帖子（只剔已删/撤回/转匿名），被举报隐藏的帖子是否应允许留在精选？「待确认」。
8. `POST-060`：DELETE /articles/:id 不挂 `require-exam-passed`（其余写操作都挂），是有意放行还是遗漏？「待确认」。
