# 业务规则：信息流 / 热榜 / 未看 / 实时新帖流 / 相关推荐 / 搜索

## 范围

本文从旧后端（Strapi 5，TypeScript）中提取以下功能的业务规则，供 Rust 重写使用：

- `GET /articles/list`：四种 feed（`all` / `following` / `favorites` / `hot`）与四种 sort（`default` / `latest` / `hot` / `unread`），包括置顶帖与热门帖的注入和去重、游标分页与 offset 兼容分页、`ids` 增量批次。
- 热榜：计分公式、候选集、活动过滤、缓存、展示位与作者限流。
- 置顶缓存。
- 「未看」流与已读判定（`article_reads`、已读水位）。
- `GET /articles/stream`：SSE 列表级广播（`new_topic` / `topic_bumped`）。
- `GET /articles/suggested/:documentId`：相关委托。
- 搜索：`/articles/search`、`/articles/suggest`、`/comments/search`、`/authors/search`、`/authors/search-lite`，以及 Meilisearch 索引文档、设置同步、SQL 兜底和隐私规则。

旧 API 的路径和响应结构不需要保留。下文提到的旧字段名只用来说明语义；行为、公式、阈值和边界情况必须保持一致。

路径前缀：`S/` = `InterKnot-web/server/`，`A/` = `InterKnot-web/app/`。

## 来源文件

- `S/src/api/article/controllers/article.ts`（listLite / stream / searchLite / suggestLite / pin / unpin / create / update / publishMine）
- `S/src/api/article/controllers/suggested.ts`
- `S/src/api/article/routes/{article-list,article-search,article-suggested}.ts`
- `S/src/api/article/services/{hot,hot-activity,hot-ranking,hot-legacy,pinned,unread-feed}.ts`
- `S/src/api/article/utils/article-broadcast-bus.ts`
- `S/src/api/article/content-types/article/lifecycles.ts`
- `S/src/api/comment/content-types/comment/lifecycles.ts`（bump + topic_bumped）
- `S/src/api/comment/controllers/comment.ts`（searchLite）、`S/src/api/comment/routes/comment-search.ts`
- `S/src/api/author/controllers/author.ts`（search / searchLite）、`S/src/api/author/routes/{author-search,author-search-lite}.ts`
- `S/src/api/article-read/{content-types/article-read/lifecycles.ts,content-types/article-read/schema.json,controllers/article-read.ts}`
- `S/src/api/conversation/utils/dm-helpers.ts`（游标编解码）
- `S/src/utils/{meilisearch,suggested-articles,read-status,block,content-visibility,cache-query,common,anonymous,account-privacy,article-hot-views}.ts`
- `S/src/middlewares/optional-auth.ts`
- `S/src/index.ts`（bootstrap：backfillBumpedAt、索引、settings 同步、热榜刷新器）
- `S/config/plugins.ts`（meilisearch、strapi-cache）、`S/config/cron-tasks.ts`
- `S/docs/hot-ranking-rollout.md`、`S/docs/search-sections-deployment.md`、`S/scripts/inspect-hot-ranking.js`
- 测试 `S/tests/unit/`：article-feed-cursor、article-feed-updates、article-list、article-block-cap、article-suggest、suggested-articles、hot-rank、hot-ranking、hot-activity、unread-feed-database、read-status-watermark、search-fallback-limit、comment-search、meilisearch-config、article-read-batch、cache-key、content-visibility
- 前端意图：`A/composables/useArticleFeedStream.ts`、`A/composables/useArticleFeedUpdates.ts`

---

## 1 数据与字段约束

### 1.1 请求参数解析

- **FEED-001** `category` 参数：先 trim；空串或字面值 `all` 表示不按分区过滤，其他值按 category slug 精确匹配。`S/src/api/article/controllers/article.ts:56-59`
- **FEED-002** `tag` 参数：只做 trim，只有空串表示不过滤（`all` 是合法的标签 slug）。按标签过滤时只匹配未封禁的标签（`isBlocked=false` 或 NULL）。`article.ts:62-69,1566-1570`
- **FEED-003** `feed` 参数：trim 并转小写，只接受 `following|favorites|hot`，其他值一律按 `all` 处理。`article.ts:85-89`
- **FEED-004** `sort` 参数：trim 并转小写，只接受 `latest|hot|unread`，其他值按 `default` 处理。**sort 只在 feed=all 时生效**，其他 feed 强制为 `default`。`article.ts:115-119,1575-1576`
- **FEED-005** 分页参数 `parsePagination(start, limit, maxLimit=50, defaultLimit=20)`：两者都用 `parseInt` 解析；start 为 NaN 或负数时取 0；limit 为 NaN 或小于 1 时取 20，大于 50 时截为 50。`S/src/utils/common.ts:269-286`
- **FEED-006** `ids` 参数（增量批次）：必须是字符串且长度不超过 6500；按 `,` 分割并 trim 后，每一项都必须匹配 `/^[A-Za-z0-9_-]{1,128}$/`（空项也算非法），否则返回 400 `Invalid article ids`；去重后超过 50 个返回 400 `At most 50 article ids are allowed`；如果 feed≠all 或 sort≠latest，返回 400 `Article updates require the latest feed`。`article.ts:1529-1541,1577-1579`

### 1.2 信息流游标

- **FEED-010** 游标格式：`base64url(utf8("${bumpedAt ISO}|${documentId}"))`，与 DM 列表、评论回复游标使用同一种格式。`S/src/api/conversation/utils/dm-helpers.ts:132-133`
- **FEED-011** 解码时在**第一个** `|` 处切分；`|` 不能在首位，两侧都必须非空，否则视为没有传游标。`dm-helpers.ts:135-150`
- **FEED-012** 时间部分要过三道校验：(1) 匹配 `/^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}/`；(2) `Date.parse` 结果不是 NaN；(3) 规范化为 `new Date(parsed).toISOString()` 之后才参与比较。任一道不通过，都视为没有传游标（退回第一页），既不返回 400/500，也不写日志。`article.ts:335-343,1548-1556`
- **FEED-013** 游标条件是严格「早于」：`bumpedAt < iso OR (bumpedAt = iso AND documentId < docId)`；排序固定为 `bumpedAt DESC, documentId DESC`。第一页也必须使用同一个排序，才能保证同一毫秒的多条帖子既不重复也不跳过。`article.ts:285-295,2028`
- **FEED-014** 同时传了 cursor 和 start 时，cursor 优先，start 被忽略，响应中回显的 start 为 0。`article.ts:1548,1972,2129-2130`
- **FEED-015** 输出游标时，bumpedAt 统一转成 ISO 串；bumpedAt 取不到时不输出游标。`article.ts:306-310,2105-2109`

### 1.3 列表卡片字段

- **FEED-020** 列表卡片包含：documentId、title、cover（第一张封面的小 webp URL；没有封面时取 `externalVideos[0].coverUrl`）、coverWidth、coverHeight、views、isAnonymous、author、category、tags（只含未封禁的）、externalVideos、isPinned、pinnedAt、solvedAt、publishedAt、firstPublishedAt、bumpedAt（ISO 或 null）、bumpedBySelf、isRead。卡片**不含正文摘要**，也不含 liked / favorited 标记。`article.ts:1104-1174`
- **FEED-021** author 字段：如果是匿名帖，一律输出 `{documentId:null, name:"绳网用户#XXXX", avatar:null, level:null}`，其中 XXXX = `sha256(anonymousSeed ?? 'anonymous-fallback')` 的前 4 位十六进制并转大写；**作者本人看自己的匿名帖也是匿名展示**。非匿名帖输出 `{documentId, name, avatar}`，avatar 优先取装备头像（用户 accountStatus 为 deleted / merged 时忽略装备头像），没有装备头像时用 author.avatar。`article.ts:1121-1144`、`S/src/utils/anonymous.ts:23-62`、`common.ts:445-472`
- **FEED-022** `bumpedBySelf = viewerUserId != null && lastBumpedByUserId != null && lastBumpedByUserId === viewerUserId`。响应中只下发这个布尔值，**不下发 lastBumpedByUserId 原值**，避免泄露是谁顶起了帖子。`article.ts:1166-1172`
- **FEED-023** `lastBumpedByUserId` 的写入时机：首次发布（create 时 status=published；update 时第一次发布；publishMine 时由草稿转为发布）写入发帖人的 user id；新评论写入评论者的 user id。编辑后重新发布不 bump，也不改这个字段。`article.ts:1237-1243,1366-1374,2938-2950`、`S/src/api/comment/content-types/comment/lifecycles.ts:380-411`
- **FEED-024** bumpedAt 的规则：首次发布时取 now；每条新评论都把该文章所有版本行的 bumpedAt 设为 now（按 documentId 批量更新）；编辑后重新发布只写 editedAt，**不 bump**。后台直接发布、没有传 bumpedAt 时，用 publishedAt 补上；更新时库中已有值就保留。启动时对 bumpedAt 为 NULL 的行执行 `bumped_at = COALESCE(published_at, updated_at)`。`S/src/api/article/content-types/article/lifecycles.ts:16-31`、`S/src/index.ts:195-207`
- **FEED-025** 需要索引 `idx_articles_bumped (bumped_at, id)`，供主信息流的范围扫描使用。`S/src/index.ts:366`

### 1.4 阅读记录

- **READ-001** `article_reads` 的每一行表示 (user, article)，带唯一键 `dedupeKey = "u{userId}:a{articleDocumentId}"`。这个键跨 draft / published 两个版本行，所以必须用 documentId，不能用行 id。数据模型中「存在记录」就等于已读，`isRead` 字段不参与判定。`S/src/api/article-read/content-types/article-read/lifecycles.ts:9-27`、`schema.json`
- **READ-002** 批量接口：需要登录；`articleDocumentIds` 经 trim、去空、去重后截取前 100 个；数组为空时返回空数组。`markAsRead=true` 时，只要有一个 documentId 不存在，就在写入之前直接返回 404 `Article not found`；只为缺失的 id 创建记录；并发写入冲突时，回读确认记录确实存在才算成功，否则抛出原错误。查询模式下，有记录的 id 返回 `isRead:true`。`S/src/api/article-read/controllers/article-read.ts:10-108`

### 1.5 Meilisearch 索引文档

- **SRCH-001** 文章索引名为 `${MEILISEARCH_INDEX_PREFIX}article`（前缀默认为空）。只索引 published 版本，文档字段：`id, documentId, title(?? ''), text=toPlainText(text), isAnonymous, isHidden, authorName(匿名→null), authorUserId(匿名→null), categoryName, categorySlug, views, likescount, commentscount, updatedAt(毫秒 epoch，缺省 0)`。`S/config/plugins.ts:35-67`
- **SRCH-002** 评论索引名为 `${prefix}comment`，文档字段：`id, documentId, content=toPlainText(content), articleDocumentId, articleTitle(?? ''), isAnonymous, isHidden, authorName(匿名→null), authorUserId(匿名→null), likescount, createdAt(毫秒)`。`plugins.ts:89-119`
- **SRCH-003** `toPlainText` 规则：非字符串返回 `''`；然后依次执行：`/<[^>]+>/g`→空格；`/!\[[^\]]*\]\([^)]*\)/g`→空格；`/\[([^\]]*)\]\([^)]*\)/g`→`$1`；`/[#>*_~`]+/g`→空格；`/\s+/g`→单个空格；trim；最后截取前 **3000** 个字符（`ARTICLE_SEARCH_TEXT_LIMIT`，评论也用这个值）。`plugins.ts:6,18-28`
- **SRCH-004** 文章索引设置：searchable 为 `title, text, authorName, categoryName`；filterable 为 `categorySlug, isAnonymous, isHidden, authorUserId`；sortable 为 `updatedAt, views, likescount`；rankingRules 为 `words, typo, proximity, attribute, sort, exactness, updatedAt:desc`。`plugins.ts:68-81`
- **SRCH-005** 评论索引设置：searchable 为 `content, articleTitle, authorName`；filterable 为 `isAnonymous, isHidden, authorUserId, articleDocumentId`；sortable 为 `createdAt, likescount`；rankingRules 末尾为 `createdAt:desc`，其余与文章索引相同。`plugins.ts:120-138`
- **SRCH-006** 索引名如果配置成非字符串，直接抛 `Invalid Meilisearch indexName for {key}`，不允许回落到无前缀的生产索引；索引名作为 URL 中的单个路径段，要经过 encodeURIComponent。`S/src/utils/meilisearch.ts:13-19`

---

## 2 权限与前置条件

- **AUTH-001** `/articles/list`、`/articles/search`、`/articles/suggested/:id` 是公开接口，使用严格可选鉴权（`rejectInvalidBearer`）：只要请求带了 `Bearer` 头（包括空 token），token 无效、过期、session 被吊销或用户不可用，都返回 401 `Invalid authentication token`，不会静默降级为匿名。鉴权基础设施出错时抛 500。`S/src/api/article/routes/article-list.ts`、`article-search.ts`、`article-suggested.ts`、`S/src/middlewares/optional-auth.ts:21-75`
- **AUTH-002** `/articles/suggest`、`/comments/search`、`/authors/search-lite` 使用宽松可选鉴权：token 无效时按匿名处理。`article-search.ts`、`comment-search.ts`、`author-search-lite.ts`
- **AUTH-003** 可选鉴权只接受满足以下条件的用户：未被封禁（`blocked`），且 accountStatus 不是 `blocked|merged|deleted`。`optional-auth.ts:57`
- **AUTH-004** `/articles/stream` 完全匿名，不解析任何 token；登录用户可以通过 query `uid` 自报 user id（`parseInt`，必须是正整数，否则忽略），这个值只用于排除本人触发的事件。`article.ts:1451-1461`、`article-list.ts`
- **AUTH-005** `feed=following|favorites` 和 `sort=unread` 都需要登录，未登录时返回空列表（不是 401）。`article.ts:1653-1655,1731`
- **AUTH-006** `/authors/search` 必须登录（`auth: {}`），未登录返回 401；`/authors/search-lite` 公开。`S/src/api/author/routes/author-search.ts`、`author.ts:836-838`
- **AUTH-007** 置顶 / 取消置顶只允许管理员（`isAdmin === true`），否则返回 403。置顶要求文章已发布且未隐藏，否则分别返回 404 / 400 `Hidden article cannot be pinned`；取消置顶要求文章当前处于置顶状态，否则返回 400 `Article is not pinned`。`article.ts:3241-3310`
- **AUTH-008** `/articles/suggested/:id` 的 limit：可以不传；传了就必须匹配 `/^\d{1,2}$/` 且大于等于 1，否则返回 400 `Invalid suggestion limit`。种子帖不存在或对当前观众不可见时返回 404 `Article not found`。`S/src/api/article/controllers/suggested.ts:5-18`

---

## 3 排序 / 算法 / 流程

### 3.1 `/articles/list` 分支选择（按顺序判定）

- **FEED-100** 分支判定顺序：(1) 有 `ids` → 增量批次；(2) feed 为 following / favorites 且未登录 → 返回空；(3) feed=hot 或 sort=hot → 纯热门；(4) sort=unread → 未看流；(5) feed=favorites → 收藏流；(6) 其余（all / following）→ normal 流，并按条件注入特殊帖。`article.ts:1604-2137`
- **FEED-101** 增量批次（`ids`）：用 `documentId IN ids` 查询，叠加未隐藏、分区、标签、拉黑过滤，并在应用层再剔除一次被拉黑作者；排序为 `bumpedAt DESC, documentId DESC`；不注入置顶或热门，不分页，不计 count。响应 meta 只包含 `updateIds`（去重后的请求 id）；即使请求的帖子全部已删除或隐藏，也照样回执 updateIds 并返回空数据。`article.ts:1604-1629`
- **FEED-102** 前端意图：首页「有 N 条新内容」的数据来源是 SSE 事件或慢轮询对账；用户点击后，前端用 `ids` 批次拉回这些卡片并插到列表最前（排在公告之前）。轮询对账时，`bumpedBySelf=true` 或 bumpedAt 没有变新的帖子不计入。`A/composables/useArticleFeedUpdates.ts:50-63,113-150`

### 3.2 纯热门分支（feed=hot / sort=hot）

- **FEED-110** HOT_ENABLED=false 时返回空列表。feed=hot 使用**展示位榜单**（`getHotArticleIds`：最多 HOT_COUNT 条，有同作者限流）；sort=hot 使用**浏览流榜单**（`getHotFeedIds`：最多 HOT_FEED_SIZE 条，不限流）。两者都**不掺置顶帖**，榜单为空时也不回退到 normal 流。读取榜单出错时按空处理。`article.ts:1663-1675`
- **FEED-111** 流程：先用 `documentId IN 榜单` 叠加未隐藏、拉黑、分区、标签过滤，得到当前观众可见的集合，保持榜单原有顺序；`total` = 可见条数；然后按 offset 切片 `[start, start+limit)`，再取卡片并按榜单顺序重排。**必须先过滤再分页**，否则会出现幽灵页。`article.ts:1677-1724`
- **FEED-112** 纯热门分支只用 offset 分页，响应**不带** hasMore / nextCursor 字段（字段缺失本身就是提示客户端走 offset 的信号）。`article.ts:1631-1641`

### 3.3 未看流（sort=unread）

- **UNRD-001** 未看流 = 最新流（`bumpedAt DESC, documentId DESC`）去掉当前观众有阅读记录的帖子。不注入置顶或热门。响应头为 `Cache-Control: private, no-store`。`article.ts:1730-1779`、`S/src/api/article/services/unread-feed.ts:5-16`
- **UNRD-002** 可见性条件与最新流完全一致：已发布、`isHidden <> true`、分区 slug 匹配、标签 slug 匹配且该标签未封禁、拉黑过滤（匿名帖放行，非匿名帖要求作者存在且不在排除集合中）。`unread-feed.ts:94-133`
- **UNRD-003** 已读判定在 SQL 里完成：`NOT EXISTS (SELECT 1 FROM article_reads r WHERE r.dedupe_key = 'u{viewer}:a' || a.document_id)`（MySQL 用 CONCAT）。**不能**用 `documentId NOT IN 已读集合`：单个用户可能有数千条阅读记录，那样会让 bumpedAt 索引失效。`unread-feed.ts:135-146`
- **UNRD-004** 水位开关打开且用户有水位时，额外要求 `publishedAt >= mark OR firstPublishedAt >= mark`（firstPublishedAt 列存在时才加这个条件）。`unread-feed.ts:148-157`
- **UNRD-005** 取数上限 `fetchLimit = (limit + 1) × (filterBlockedInApp ? 3 : 1)`；`hasMore = visible.length > limit || rows.length >= fetchLimit`；next 锚点规则：可见行多于本页时，锚在本页最后一项；否则锚在取回的原始最后一行。没有下一页时 next=null。`unread-feed.ts:166-203`
- **UNRD-006** 控制器按服务返回的 id 顺序取回卡片（只再过滤一次未隐藏），所有卡片 `isRead:false`。分页信息固定为 `{start:0, limit, total:0, pageCount:0, hasMore, nextCursor}`，**第一页也带游标字段**，不计算 total。`article.ts:1744-1778`

### 3.4 收藏流（feed=favorites）

- **FEED-120** 按收藏的 `createdAt DESC` 做 offset 分页；收藏的文章必须满足：未隐藏、已发布、分区和标签匹配，并通过拉黑过滤（`isAnonymous=true OR author.id NOT IN excludeAuthorIds`）。这些过滤**在收藏分页和 count 之前**完成。count 与分页使用同一组条件。响应不带游标字段。`article.ts:1782-1846`

### 3.5 关注流（feed=following）

- **FEED-130** 取当前用户关注的作者，最多 `MAX_FOLLOWING_AUTHORS=1000` 条（查询不指定顺序），去重后剔除被拉黑或拉黑了我的作者；结果为空时返回空列表。过滤条件为 `author.id IN authorIds` 并且**非匿名**（`isAnonymous=false` 或 NULL），这样关注流不会暴露被关注者的匿名帖。之后走 normal 流（支持游标），不注入置顶或热门。`article.ts:121,1850-1869,1876,1893`

### 3.6 主信息流 normal + 特殊帖注入

- **FEED-140** 置顶注入条件：`feed=all` 且没有 tag（**有 category 时仍然注入**，见 10 节）。置顶 id 来自全局缓存（按 pinnedAt 倒序），在应用层按拉黑过滤：保留条件为 `isAnonymous || authorId===null || authorId ∉ excludeAuthorIdSet`。读取出错时按空处理。`article.ts:1875-1887`
- **FEED-141** 热门注入条件：`HOT_ENABLED && feed=all && 无 category && 无 tag && sort≠latest`；热门 id 取自展示位榜单 `getHotArticleIds`（最多 HOT_COUNT=4 条）。`article.ts:1892-1902`
- **FEED-142** 只在**非游标模式**下按当前观众的可见性（未隐藏、拉黑）过滤热门 id，并**去掉已经置顶的 id**（置顶优先）。被过滤掉的位置不补位。`article.ts:1915-1929`
- **FEED-143** 特殊帖顺序：`specialIds = [...pinnedIds, ...visibleHotIds]`，记 `S = specialIds.length`。游标模式下 `specialIds=[]`、S=0，但**排除集合照旧**：normal 流始终带 `documentId NOT IN unique([...pinnedIds, ...原始 hotIds])`，避免第一页已经展示过的帖子在后续页重复出现。`article.ts:1931-1942,1958`
- **FEED-144** normal 流过滤条件：`isHidden<>true` + 分区 + 标签 + 关注作者过滤 + 拉黑过滤（排除集合超过上限时不在 SQL 中拉黑，见 PRIV-004）+ 特殊帖排除；游标模式下再用 AND 叠加游标条件（与 `$or` 结构的拉黑条件并列，不能互相覆盖）。`article.ts:1944-1966`
- **FEED-145** offset 模式下的特殊帖切片：特殊帖占据整个虚拟列表的前 S 个位置。`normalStart = max(0, start − S)`；`specialOnThisPage = max(0, min(S − start, limit))`；`normalLimit = limit − specialOnThisPage`；本页的特殊帖 = `specialIds[start, min(start+limit, S))`，按 specialIds 的顺序排列。`article.ts:1972-2002`
- **FEED-146** `normalLimit = 0` 时**必须跳过 normal 查询**（本页全是特殊帖）。`normalFetchLimit = (cursorMode ? normalLimit+1 : normalLimit) × (blockOversized ? 3 : 1)`；offset 模式**不多取 1 条**。`article.ts:2014-2016,2034-2039`
- **FEED-147** count：只有 offset 模式才执行，count 的条件就是 normal 流的过滤条件（不含游标条件）；offset 模式下 `total = count + S`，`pageCount = ceil(total/limit)`；游标模式下 `pagination = {start:0, limit, total:0, pageCount:0}`。`article.ts:2040-2048,2129-2131`
- **FEED-148** hasMore：游标模式下为 `normalRows.length > normalLimit || fetchedRows.length >= normalFetchLimit`（按**取回的原始行数**判断，避免应用层过滤后误判为没有更多）；offset 模式下为 `start + 本页条数 < total + S`。`article.ts:2088-2090`
- **FEED-149** nextCursor 只取 **normal 流**中的尾项，不能用特殊帖（特殊帖的 bumpedAt 与它在流中的位置无关）。如果还有幸存行没有放进本页，锚在本页最后一项；否则锚在取回的最后一行（这样可以跳过中间那段被拉黑、本人不可见的行）。`article.ts:2092-2109`
- **FEED-150** 游标字段成对出现：有 nextCursor 时输出 `{hasMore:true, nextCursor}`；`!hasMore && normalLimit>0` 时输出 `{hasMore:false, nextCursor:null}`；其他情况（normal 查询没有执行，或尾项没有 bumpedAt）**两个字段都不输出**，客户端据此回退 offset。offset 模式的响应也会附带这两个字段，方便客户端从第一页起切换到游标模式。`article.ts:2111-2123`
- **FEED-151** 所有列表分支在补齐卡片后统一计算：回填匿名 seed、计算已读集合（isRead）、装备头像。`article.ts:2061-2075`

### 3.7 热榜计分（当前生效算法：`services/hot.ts`）

- **HOT-001** 参数与默认值，解析规则为 `integer(name, fb, max) = max(1, min(max, floor(Number(env) || fb)))`、`decimal(name, fb) = (Number(env) 有限且 > 0) ? 值 : fb`：
  - `HOT_ENABLED`：不等于字符串 `'false'` 即为开启（默认开启）；
  - `HOT_COUNT=4`（上限 100）；`HOT_FEED_SIZE=200`（上限 2000）；`HOT_REFRESH_SEC=600`（上限 3600）；`HOT_TTL_SEC = HOT_REFRESH_SEC × 3`；
  - `HOT_GRAVITY=1.2`；`HOT_RECENT_DAYS=7`（上限 90）；`HOT_CANDIDATES=1000`（上限 10000）；
  - `HOT_MAX_PER_AUTHOR = max(0, floor(Number(env ?? 1)))`，默认 1，0 表示不限。
  `S/src/api/article/services/hot.ts:16-33,187`
- **HOT-002** 精确公式（对应 Discourse TopicHotScore）。设 `now` 为毫秒时间戳，`g = HOT_GRAVITY`（默认 1.2），`D = HOT_RECENT_DAYS`（默认 7），窗口 `W = [now − D·86 400 000, now]`：

  ```
  h(t)      = max(0, now − t) / 3 600 000 + 2
  c         = firstPublishedAt ?? publishedAt                      （帖子「创建时间」）
  Lr        = 窗口 W 内点赞该帖的不同有效用户数
  Cr        = 窗口 W 内评论该帖的不同有效用户数
  o         = 1 若 since ≤ c ≤ now 否则 0                          （开帖发生在窗口内）
  Pr        = Cr + o
  likes     = (c > now − D·86 400 000) ? Lr : likescount           （新帖用窗口内点赞人数，老帖用累计点赞数）
  f         = min( {c | o=1} ∪ {窗口内最早一条有效评论时间} )，集合为空时 f = null
  score     = likes / h(c)^g  +  [f ≠ null] · (Lr + Pr − 1) / h(f)^g
  ```

  `score ≤ 0` 的帖子不上榜。`hot.ts:55-69,112-141`
- **HOT-003** 排序：`score DESC`；分数相同时比较 `f DESC`（null 视为 −∞）；再相同时比较 `bumpedAt DESC`；最后按 `documentId DESC`。结果截取前 HOT_FEED_SIZE 条。`hot.ts:143-150`
- **HOT-004** 候选集 = 按 `bumpedAt DESC, id DESC` 排序的最近活跃前 HOT_CANDIDATES 篇（已发布、`isHidden` 为 false 或 NULL），再并上窗口内有点赞或评论的所有文章（不在前者中的部分按每批 500 个用 `$in` 补查）。按 documentId 去重；c 无法解析的行丢弃；`bumpedAt` 缺失时取 c；`likescount` 取 `max(0, Number||0)`。`hot.ts:77-109`
- **HOT-005** 有效活动过滤（`queryHotActivity`，当前只读取 `like` 和 `comment` 两类）：活动的 `createdAt ∈ [since, until]`；通过 documentId 关联到**已发布**版本行，要求 `publishedAt ≤ until`、documentId 非空、文章未隐藏；**行为人 ≠ 文章作者本人**；行为人 accountStatus 为 `active` 或 NULL，非 AI 账号，未被封禁；评论本身未隐藏，且**任何一级祖先评论都未隐藏**（递归 CTE 用 UNION，遇到环也能终止）。每个 (文章, 行为人, 类型) 只算一次，时间取窗口内最早那次。`since > until` 时返回空；窗口时间无效时抛错。`S/src/api/article/services/hot-activity.ts:66-200`
- **HOT-006** 点赞活动只统计挂在文章上的赞（通过 article 关系 inner join），评论的赞不计入。`hot-activity.ts:115`
- **HOT-007** 展示位榜单（首页注入、feed=hot、Agent）：在完整榜单基础上，先按「已发布且未隐藏」再过滤一次，再对作者限流：同一作者（author 内部 id）最多 HOT_MAX_PER_AUTHOR 篇；**匿名帖和无作者的帖子不参与限流**（否则可以从「谁被挤掉」反推出匿名作者）。`getHotArticleIds` 截取前 HOT_COUNT 条；`getHotArticleIdsUpTo(n)` 截取前 `clamp(floor(n), 0, 100)` 条。浏览流（sort=hot）直接使用完整榜单，不过滤也不限流。`hot.ts:176-208`
- **HOT-008** 非生效算法（重写时**不要**实现，除非产品另行要求）：`hot-ranking.ts` 是「近期参与者」算法（每人只取最强的一次行为，权重衰减为 `w/(ageH+2)^gravity`，`score = uniqueViews·w_view + Σ`，并有 minParticipants 门槛），只被测试引用；`hot-legacy.ts` 是旧的 `LN(1+加权计数)/(活动小时+2)^1.5/(天龄+1)^0.25` 公式（变量 `HOT_W_*`、`HOT_WINDOW_DAYS`），保留它只为了回滚和对比。`article-hot-views.ts`（阅读 UV 表）目前在 src 中**没有调用方**。`S/src/api/article/services/hot-ranking.ts:30-120`、`hot-legacy.ts:10-37`、`S/docs/hot-ranking-rollout.md:36`

### 3.8 相关委托（suggested）

- **SUGG-001** 常量：默认 6 条，最多 10 条；未看档只看最近 90 天；随机档取近 365 天；随机池容量 3000，缓存 key `suggested:pool:v1`，TTL 600s。documentId 必须匹配 `/^[a-zA-Z0-9_-]{1,128}$/`。`S/src/utils/suggested-articles.ts:22-31`
- **SUGG-002** 种子帖必须对当前观众可见：未隐藏（false 或 NULL）、已发布且 `publishedAt ≤ now`，并通过拉黑过滤（匿名帖放行）；不满足时返回 null（对外 404）。`suggested-articles.ts:87-99`
- **SUGG-003** 登录用户先走未看档：如果种子帖有分区，先查同分区再查全站，否则只查全站；每轮调用未看流查询（拉黑排除传空，`limit = limit×3 + 已排除数`），再按可见性和 `bumpedAt ≥ now−90d` 取卡片，按未看流顺序填入，跳过已选中的、种子帖本身、**观众本人的帖子（包括本人的匿名帖）**。`suggested-articles.ts:102-134`
- **SUGG-004** 名额仍不足时（游客直接从这里开始）进入随机补位：随机池为近 365 天发布（按 publishedAt）、未隐藏、按 `bumpedAt DESC, id DESC` 排序的前 3000 篇，存为 `[documentId, categorySlug]`，全站共享缓存，与观众无关。同分区和其他分区各自用 Fisher–Yates 洗牌，同分区排在前面，取前 `(limit − 已选)×3` 个候选，再按观众可见性取卡片并填入。`suggested-articles.ts:63-85,136-142`
- **SUGG-005** 输出：每条含 documentId、title、cover（小 webp）、coverWidth、coverHeight、views、likesCount、commentsCount、isAnonymous、isRead、author（匿名帖用匿名占位）、category、tags、publishedAt、firstPublishedAt、bumpedAt；不计算内容相似度，也不按标签匹配。`suggested-articles.ts:144-169`

### 3.9 文章搜索 `/articles/search`

- **SRCH-010** q 只做 trim（**没有长度上限**），q 为空时直接返回空页（total 0）。单字查询在 Meilisearch 路径放行。`article.ts:2206,2220-2235`
- **SRCH-011** `sort` 参数：trim 并转小写，`latest|hot` 之外一律按 `relevance` 处理。relevance **不传** sort；latest 传 `['updatedAt:desc']`；hot 传 `['likescount:desc','views:desc']`。`article.ts:95-99,2254-2263`
- **SRCH-012** Meilisearch 请求：`{q, offset:start, limit, filter}`，filter 包括 `isHidden != true`（索引里缺这个字段的旧文档也能匹配），有分区时加 `categorySlug = "<JSON 转义>"`，有拉黑时加 `NOT authorUserId IN [ids]`（只保留正整数 user id；匿名文档的 authorUserId 为 null，所以不会被过滤）。sort 数组为空时不发送 sort 字段。超时 3000ms。total 优先取 `estimatedTotalHits`，其次 `totalHits`，最后用 hits 条数。`S/src/utils/meilisearch.ts:139-227`
- **SRCH-013** 命中后**一律回查 DB**：`documentId IN 命中`，叠加已发布、未隐藏、拉黑过滤，按命中顺序保序；DB 结果才是准的。如果索引给了命中但**这一页一条都过不了回查**（全是索引残留），判定索引不可信，转入 SQL 兜底；如果索引本来就没有命中，则返回空，不再查 SQL。Meilisearch 抛错同样转入兜底。`article.ts:2265-2307`
- **SRCH-014** SQL 兜底：q 长度小于 2 时直接返回空页；全文查询词 = `q.replace(/[+\-><()~*"@]/g,' ').replace(/\s+/g,' ').trim()`，非空时执行 MySQL `MATCH(title,text) AGAINST(? IN BOOLEAN MODE)` 或 PG `to_tsvector('simple', COALESCE(title,'')||' '||COALESCE(text,'')) @@ plainto_tsquery('simple', ?)`，只取已发布行，**最多 1000 个 document_id**（固定窗口，与 start 无关）；触顶时写 warn。全文没有命中（或查询词为空、或报错）时，退化为 `title $contains q OR text $contains q`。然后叠加分区、拉黑、未隐藏条件。`article.ts:345-371,2309-2412`
- **SRCH-015** SQL 兜底的排序：hot 为 `likescount DESC, views DESC`，relevance 和 latest 都是 `updatedAt DESC`；同时用同一组条件 count 得到 total。`article.ts:2389-2411`
- **SRCH-016** 搜索结果使用列表卡片格式，但只取 documentId、title、views、isAnonymous、publishedAt、firstPublishedAt，所以 bumpedAt=null、isPinned=false、bumpedBySelf=false；带 isRead。`article.ts:2237,2414-2432`

### 3.10 联想 `/articles/suggest`

- **SRCH-020** limit 用 `parseInt` 解析，有限值时夹到 [1,10]，否则取 8；q 为空时返回 `{data:[], meta:{total:0}}`。`article.ts:2443-2457`
- **SRCH-021** Meilisearch 请求开启高亮：`attributesToHighlight:['title']`，`highlightPreTag:'<mark>'`，`highlightPostTag:'</mark>'`，`attributesToCrop:['text']`，`cropLength:24`。回查 DB（已发布、未隐藏、拉黑），**匿名状态、作者名和分区都以 DB 为准**；每条输出 `{documentId, title, titleHighlighted(?? title), excerpt(?? ''), authorName(匿名→null), categoryName, categorySlug, isAnonymous}`。`meilisearch.ts:183-191`、`article.ts:2459-2516`
- **SRCH-022** 回查后全部被过滤（命中全是索引残留）或 Meilisearch 报错时，走 DB 兜底：q 长度小于 2 返回空；否则 `title $containsi q` + 分区 + 拉黑 + 未隐藏，按 `updatedAt DESC` 取 limit 条，titleHighlighted 等于 title，excerpt 为空串。索引本来就没有命中时返回空。`article.ts:2512-2566`

### 3.11 评论搜索 `/comments/search`

- **SRCH-030** q 依次从 `ctx.query.q`、数组首个非空值、原始 URL 中提取，trim 后**截断为 32 个字符**（不报错）；q 为空时返回空页。参数 `article` trim 后限定在某篇文章内；分页规则同 FEED-005。`S/src/api/comment/controllers/comment.ts:97,130-146,1363-1397`
- **SRCH-031** Meilisearch filter：`isHidden != true`，可选 `articleDocumentId = "<id>"`，可选 `NOT authorUserId IN [...]`；只用相关性排序。超时 3000ms。`meilisearch.ts:252-331`
- **SRCH-032** 可见性（回查和 SQL 共用）：评论未隐藏，所属文章已发布且未隐藏；评论是顶层评论，或其父评论未隐藏、父评论本身是顶层评论、且父评论所属文章公开；评论作者通过拉黑过滤（匿名评论放行）；文章作者通过拉黑过滤（匿名文章放行）；并满足可选的 articleDocumentId。之后再执行 `filterValidComments`：父评论必须和本评论属于同一篇文章。`comment.ts:1425-1433,1548`、`S/src/utils/content-visibility.ts:10-35,157-185`
- **SRCH-033** 兜底规则与文章搜索相同：命中全是残留或 Meilisearch 报错时转入 SQL；q 长度小于 2 返回空；全文查询为 MySQL `MATCH(content) AGAINST(? IN BOOLEAN MODE)` / PG `to_tsvector('simple', COALESCE(content,'')) @@ plainto_tsquery('simple', ?)`，上限 1000 个 id；没有命中时用 `content $containsi q`；排序为 `createdAt DESC`。`comment.ts:1474-1546`
- **SRCH-034** 输出：每条含 documentId、content、images、createdAt、likesCount、liked（当前观众是否赞过）、isAnonymous、author（匿名评论对所有人都匿名，包括作者本人）、article `{documentId, title, cover}`，其中 cover 是**单个对象或 null**（取第一张封面），以及 `replies:[]`；**不输出楼层号**。`comment.ts:1554-1615,157-161`

### 3.12 用户搜索

- **SRCH-040** `/authors/search`（@ 提及用）：q 依次从 query、request.query、URL 中提取并 trim；为空时返回 400 `Missing q`；超过 32 个字符返回 400 `q too long`；limit 默认 8，夹到 [1,10]。查询条件为 `name $containsi q OR user.username $containsi q`，按 `name ASC` 取 `limit×4` 行；在应用层剔除没有关联 user、被封禁（user.blocked）、`profileHidden=true`、双向拉黑的用户，再截取 limit 条。每条输出 `{documentId, name, username, level, avatar}`（装备头像优先）。`S/src/api/author/controllers/author.ts:29-33,836-965`
- **SRCH-041** `/authors/search-lite`（搜索页「用户」tab）：limit 默认 20，最大 20；start 为正整数时生效，否则为 0；q 为空、q 超过 32 个字符、或 `start > 200` 时都**返回空页而不是 400**。一次性取 `200+20=220` 行候选（条件同上，按 name ASC），在应用层过滤（同 SRCH-040）后排序：精确等于（name 或 username，忽略大小写）排第 0 档，前缀匹配排第 1 档，其余排第 2 档；同档按 name 的 `localeCompare` 排序。`total` = 过滤后的条数；切片 `[start, start+limit)`。`author.ts:42-47,975-1111`
- **SRCH-042** search-lite 输出：每条含 documentId、name、username、level、avatar、`bio`（blocks 富文本按 text 和 children 拼接，空白折叠，截取前 120 个字符，为空时给 null）、`followersCount`（逐个作者 count）、`isFollowing`（未登录恒为 false）。用户搜索**不走 Meilisearch**。`author.ts:56-70,1113-1176`

### 3.13 SSE 实时新帖流 `/articles/stream`

- **SSE-001** 频道：有 tag 时为 `tag:{slug}`，否则有 category 时为 `c:{slug}`，否则为 `latest`。每个连接只订阅**一个**频道。`S/src/api/article/utils/article-broadcast-bus.ts:65-72`、`article.ts:1452-1454`
- **SSE-002** 一条事件的投递频道 = `latest` + `c:{categorySlug}`（有分区时）+ 帖子每个**未封禁**标签对应的 `tag:{slug}`。`article-broadcast-bus.ts:75-82,183-185`
- **SSE-003** 事件类型：`new_topic`（首次发布：create published、update 时第一次发布、publishMine 草稿转发布）和 `topic_bumped`（新评论顶起旧帖）。编辑后重新发布不广播。`article.ts:128-141,1268-1270,1435-1437,2939-2940`、`comment/lifecycles.ts:76-93,418-440`
- **SSE-004** 广播前先读取 published 行；**文章隐藏或不存在时不广播**。payload 只包含 `{type, topicId, categorySlug|null, tagSlugs[], at(服务端 ISO 时间)}`，不含正文和用户信息，也不含 actor。`article-broadcast-bus.ts:152-188`
- **SSE-005** 排除触发者：new_topic 默认把文章作者的 user id 当作 actor，topic_bumped 的 actor 是评论者的 user id；自报 uid 等于 actor 的所有连接都收不到这条事件。actor 只在服务端比较，**绝不写进 payload**。`article-broadcast-bus.ts:123-143,176-178`
- **SSE-006** 协议：先尝试订阅，如果全局连接数已经 ≥ `MAX_TOTAL_SUBSCRIBERS=5000`，返回 `503` 加 `Retry-After: 30` 并结束（满了直接拒绝新连接，不踢掉旧连接）。订阅成功后返回 200，响应头为 `Content-Type: text/event-stream; charset=utf-8`、`Cache-Control: no-cache, no-transform`、`Connection: keep-alive`、`X-Accel-Buffering: no`；首先写 `retry: 5000\n\n`，然后发送 `event: hello` 事件，`data: {channel, at}`；每 25 000ms 写一行心跳注释 `:hb {Date.now()}\n\n`；事件帧格式为 `event: {type}\ndata: {json}\n\n`；连接 close 或 error 时幂等清理（停止心跳、取消订阅、结束响应）。`article.ts:47-53,1463-1525`、`article-broadcast-bus.ts:41,87-114`
- **SSE-007** 广播必须在事务**提交之后**以 fire-and-forget 方式执行，失败只记 warn，不能影响发帖或评论主流程；单个订阅者回调出错不影响其他订阅者。`article.ts:128-141`、`article-broadcast-bus.ts:131-142`
- **SSE-008** 当前是单进程内存总线；多实例部署时需要换成 pub/sub，对外接口不变。`article-broadcast-bus.ts:8-10`

---

## 4 副作用与缓存

- **CACHE-001** 热榜缓存：Redis key `hot:score:v1` 存完整榜单的 JSON id 数组，TTL 为 `HOT_REFRESH_SEC×3`（默认 1800s）。读取时缓存命中就直接用；未命中则同步计算并回写。同一进程内用 `pending` promise 做单飞，避免并发重复计算。`hot.ts:36,39,153-174`
- **CACHE-002** 热榜计算失败时写 warn 日志，并返回 Redis 中上一份榜单（没有就返回 `[]`），**不覆盖缓存**；Redis 不可用时直接计算，结果照常返回。`hot.ts:155-171`
- **CACHE-003** 置顶缓存：key `pinned:articles:v1`，值为 `[{documentId,isAnonymous,authorId}]`（按 pinnedAt 倒序，空数组也是合法缓存值，用来防穿透），TTL 为 `max(1, Number(PINNED_TTL_SEC||60))`。缓存的是全站视角的数据，拉黑过滤由调用方在应用层做；没有单飞锁；pin / unpin 后主动删除这个 key（失败静默，靠 TTL 兜底）。从 DB 加载时按每页 1000 条循环读取：`isPinned=true`、已发布、`isHidden<>true`。`S/src/api/article/services/pinned.ts:18-105`、`article.ts:3272,3307`
- **CACHE-004** pin / unpin 会更新同一 documentId 的所有版本行：pin 设置 `isPinned=true, pinnedAt=now, pinnedBy=[admin]`，unpin 设置 `false, null, []`。`article.ts:3257-3270,3293-3305`
- **CACHE-005** 拉黑集合缓存：`user-block:list:{uid}`（我拉黑了谁）和 `user-block:by:{uid}`（谁拉黑了我）都是 Redis Set，TTL 300s；空集合用 `…:empty` 占位 key 缓存；缓存出错时直接查库（fail-open）。`S/src/utils/block.ts:6-115`
- **CACHE-006** 响应缓存（Strapi 插件）：只缓存匿名请求，TTL 30s；可缓存路由前缀为 `/api/articles`、`/api/comments`、`/api/authors`、`/api/article-reads`、`/api/categories`，其中**排除 `/api/articles/list`**；实体变更时自动清理，启动时也清理。`S/config/plugins.ts:143-186`
- **CACHE-007** query 白名单归一化（同时用于缓存 key 和控制器实际读取的参数）：`/api/articles/list`→`start,limit,category,feed,sort,cursor,tag,ids`；`/api/articles/search`→`q,start,limit,category,sort`；`/api/articles/suggest`→`q,category,limit`；`/api/comments/search`→`q,start,limit,article`；`/api/authors/search-lite`→`q,start,limit`；`/api/authors/search`→`q,limit`。未知参数会被剥掉；重复参数只保留第一个；参数按白名单顺序重排；前缀匹配必须落在路径段边界上。`S/src/utils/cache-query.ts:14-101`
- **CACHE-008** 匿名用户的列表和搜索响应不依赖用户状态，可以共享缓存；isRead 只在登录时计算（匿名访客一条已读 SQL 都不发）。`S/src/utils/read-status.ts:161-170`
- **READ-010** 已读集合 `buildReadDocIdSet`：先按 documentId 去重；水位开关关闭（默认）时，只执行一次 `article_reads WHERE user=viewer AND article.documentId IN (...)`，**一次 up_users 都不读**。`read-status.ts:164-217`
- **READ-011** 水位开关 `READ_WATERMARK_ENABLED ∈ {'1','true','yes'}`，每次调用都重新读取环境变量。开关打开时，先读取用户的 `readWatermarkAt`（读取失败按 null 处理，只记 debug 日志）；帖子的 `feedTime = max(publishedAt, firstPublishedAt)`，**严格小于**水位时直接算已读，等于水位时仍查精确记录；**不看 bumpedAt 和 updatedAt**（否则老帖被新评论顶起后会变回未读）；拿不到时间的帖子退回精确查询；整页都在水位之前时不发 IN 查询。`read-status.ts:36-126,185-202`
- **READ-012** 水位只能前移：用一条带条件的 UPDATE，`WHERE id=? AND (read_watermark_at IS NULL OR read_watermark_at < at)`；返回是否真的前移了；失败时记 warn 并返回 false。目前**没有调用方**（「全部标为已读」端点尚未实现）。`read-status.ts:236-258`
- **READ-013** 已读的其他入口（文章详情、`/article-reads/batch` 查询模式）仍然只按「存在记录即已读」判定，不看水位。所以在这两处也改成「水位或记录」之前，**不能归档水位之前的阅读记录**。`read-status.ts:143-150`、`article.ts:951-996`
- **SRCH-050** 启动时幂等地把 article 和 comment 两个索引的 settings 推送到 Meilisearch（`PATCH /indexes/{name}/settings`，超时 10s），单个索引失败只记 warn。原因是插件只在全量重建时才推送 settings，config 才是唯一事实来源。`meilisearch.ts:102-134`、`S/src/index.ts:29-42`
- **SRCH-051** 账号注销后的隐私清理：在两个索引中，用 `documents/fetch filter authorUserId = {uid}` 分批（每批 500）找出文档，并把 `authorName` 改成 `'已注销用户'`（以 `_meilisearch_id` 为主键做部分更新），然后轮询 task 状态，最长 10s，每 100ms 一次，失败或超时就抛错；索引不存在时跳过。同时清理 `/api/(articles|comments|authors)` 的响应缓存。由 cron 每分钟重试，每批 20 个用户。`meilisearch.ts:42-84`、`S/src/utils/account-privacy.ts:4-24`、`S/config/cron-tasks.ts:556-559`
- **SRCH-052** 启动时建立评论全文索引：MySQL 为 `FULLTEXT ft_comments_content(content)`，PG 为 `GIN to_tsvector('simple', content)`；查询表达式必须与索引表达式逐字一致。`S/src/index.ts:427-460`、`comment.ts:1504-1511`

---

## 5 限流与防刷

- **RATE-001** `/authors/search`：按用户计数，key `authors:search:rl:{userId}`，固定窗口 1s，超过 10 次返回 429 `Search rate limit exceeded`；Redis 出错时放行。`author.ts:29-30,884-893`
- **RATE-002** `/authors/search-lite`：登录用户 key `authors:searchlite:rl:u:{uid}`，限 10 次/s；匿名用户 key `authors:searchlite:rl:ip:{clientIp}`，限 3 次/s；超限返回 429；Redis 出错时放行。`author.ts:42-43,1032-1046`
- **RATE-003** 客户端 IP 取法：从 `X-Forwarded-For` 右侧往左数第 `TRUSTED_PROXY_HOPS` 跳（默认 1；设为 0 时用 socket 对端地址）；不能取最左值（可以伪造），也不能用框架的 `request.ip`（反向代理后恒为 127.0.0.1）。`common.ts:497-518`
- **RATE-004** SSE 有全局连接上限 5000（匿名也可以连接，以此防 DoS），满了返回 503。`article-broadcast-bus.ts:40-41,93`
- **RATE-005** 防刷边界：search-lite 的 `start > 200` 直接返回空，防止一页页翻完整个作者表；list 的 limit 最大 50、ids 最多 50 个、原始串最长 6500；SQL 兜底最多 1000 个 id；坏游标退回第一页且**不写日志**（避免成为刷日志的入口）。`author.ts:39-45`、`common.ts:269-286`、`article.ts:1532-1554,353`
- **RATE-006** `/articles/list`、`/articles/search`、`/articles/suggest`、`/comments/search` **没有专门的限流**，只有匿名响应缓存 30s（list 不缓存）。`config/plugins.ts:148-158`

---

## 6 隐私与匿名过滤

- **PRIV-001** 拉黑排除集合 = 我拉黑的人 ∪ 拉黑我的人（user id），再换算成 author 内部 id 用于 DB 过滤。DB 条件为 `isAnonymous=true OR author.id NOT IN excludeAuthorIds`：**匿名内容永远不按拉黑过滤**，否则可以从「内容消失」反推出匿名作者。`article.ts:440-486`、`content-visibility.ts:44-71`
- **PRIV-002** 应用层的拉黑过滤（`rejectBlockedItems`）与 DB 条件含义相同：匿名内容保留；author.user.id 是数字且在排除集合中的剔除。`article.ts:426-434`
- **PRIV-003** 被拉黑的用户如果没有 author 记录：文章路径不在 DB 中过滤；评论搜索仍然会把这些 user id 交给 Meilisearch 的 `authorUserId` filter。`article.ts:457-460`、`comment.ts:184-189`
- **PRIV-004** `BLOCK_EXCLUDE_DB_CAP = 200`：作者排除集合**大于** 200 时（判定条件是 `>`，不是 `>=`），**只有 normal 流那条无界分页查询**不再在 SQL 中做 `NOT IN`，改为多取 `BLOCK_OVERFETCH_FACTOR=3` 倍后在应用层过滤并截回页大小；count 不变（total 会略偏大），同时写 warn。热门、置顶、收藏、搜索、联想仍然在 DB 中做 `NOT IN`。未看流在同样条件下改为 LEFT JOIN 作者，在应用层过滤。`article.ts:373-395,462-485,1949-1957,2004-2058`、`unread-feed.ts:172-187`
- **PRIV-005** Meilisearch 索引中，匿名文章和匿名评论的 `authorName`、`authorUserId` 都存为 null：无法按作者名搜到匿名内容，也无法反推作者；authorUserId 只用于拉黑过滤，**不出现在任何响应中**。`plugins.ts:57-59,112-115`、`S/docs/search-sections-deployment.md:62-64`
- **PRIV-006** 搜索、联想、评论搜索的返回内容一律以 DB 回查结果为准，包括匿名状态、作者名、是否隐藏，从而覆盖索引滞后的窗口期。`article.ts:2469-2470`、`comment.ts:1355-1357`
- **PRIV-007** 匿名 seed 是服务端私有数据，只用来派生展示名，**绝不下发**。`anonymous.ts:9-14`、`article.ts:488-525`
- **PRIV-008** 关注流排除匿名帖（FEED-130）；热榜作者限流不作用于匿名帖（HOT-007）；SSE payload 不包含 actor（SSE-005）；列表只下发 bumpedBySelf 布尔值（FEED-022）。
- **PRIV-009** 用户搜索排除 `profileHidden=true` 的用户、被封禁的用户、没有关联 user 的 author，以及双向拉黑的用户。`author.ts:925-934,1078-1087`
- **PRIV-010** 评论搜索结果不 populate 父评论，避免匿名父评论的作者名泄露（历史上发生过）。`comment.ts:1416-1418`

---

## 7 边界情况与已知坑（不得回归）

- **EDGE-001** 坏游标的两种情况都要退回第一页：解不开的 token，以及能解开但时间无效的 token（`garbage`、`2026-13-45T99:99:99Z`、`Sep 1 2026`、`1757000000000`）。否则 SQL 层会报日期格式错误，变成 500 并写日志，任何人都能在最热的接口上无限刷。`article.ts:312-343`、`tests/unit/article-feed-cursor.test.ts:443-497`
- **EDGE-002** 同一毫秒内有多条帖子时，靠 documentId 破平；排序和游标判定必须使用同一组键，否则会整批跳过或重复。`article-feed-cursor.test.ts:419-441`
- **EDGE-003** 当页面大小 ≤ 特殊帖数量时，不能执行「limit 0」的 normal 查询（Strapi 会把 limit 0 当作不限，结果拉全表）。重写时的语义是：normalLimit=0 就不查。「limit 0 等于不限」本身是 Strapi 特有问题，重写时不需要处理。`article.ts:2034-2039`
- **EDGE-004** 游标模式的 hasMore 必须按原始取回行数判断：多取的那一批里大半被拉黑时，不能误判为没有更多而截断信息流；本页为空时游标也要继续前进；幸存行多于一页时，锚点回到本页最后一项。`article-feed-cursor.test.ts:649-775`
- **EDGE-005** 带游标翻页时，仍要排除第一页注入过的置顶帖和热门帖（排除集合用原始 hotIds）。已知遗留问题：热榜在两次翻页之间刷新会导致排除集合变化，同一篇帖子可能重复出现一次，目前接受这种情况。`article.ts:1931-1937`
- **EDGE-006** 热门和收藏分支必须先按可见性过滤，再分页和计数，否则会出现短页和幽灵页。`article.ts:1684-1685,1783`
- **EDGE-007** 搜索和联想的「假空」问题：索引里堆积了已删除或已转草稿的残留文档（实测命中 370 条，而全站只有 239 篇），导致第一页回查后全被过滤。修复方式是：一页全被过滤时改走 SQL。客户端还要按 documentId 去重（offset 和返回条数不一致时会出现重复）。`S/docs/search-sections-deployment.md:129-163`
- **EDGE-008** SQL 兜底的窗口必须是固定的 1000，不能写成 `min(start+limit*3, CAP)` 这类随 start 变化的值，否则 total 和 pageCount 会随翻页变化。`article.ts:2322-2331`、`tests/unit/search-fallback-limit.test.ts:182-216`
- **EDGE-009** MySQL 全文布尔模式里，用户输入的 `C++`、`--`、半个引号会造成语法错误，进而退化成全表 LIKE 扫描，所以要先剥掉运算符；全部是运算符时直接走模糊匹配。文章和评论两处的实现必须保持一致。`article.ts:355-371`、`comment.ts:106-119`
- **EDGE-010** 评论搜索里的 `article.cover` 必须是单个对象或 null，不能是数组（否则 Android 端整条响应解析失败）。`comment.ts:157-161`、`docs/search-sections-deployment.md:165-180`
- **EDGE-011** 白名单漏掉 `cursor` 或 `tag` 时，参数会在控制器读取之前被剥掉，表现为翻页永远停在第一页、tag 页显示全站帖子（2026-09-20 实测踩坑），而且不同参数会共用同一条缓存。重写后的 API 如果有类似的参数归一化层，同样要保证这两个参数能透传。`cache-query.ts:15-24`、`tests/unit/cache-key.test.ts:122-147`
- **EDGE-012** 前缀匹配必须落在路径段边界上：documentId 以 `list` 开头的评论，其 `/replies` 请求会被误判为 `/comments/list`，导致 cursor 被剥掉。`cache-query.ts:72-86`
- **EDGE-013** 拉黑条件用的是 `$or`，游标条件必须在外层用 `$and` 包一层，否则后写的条件会覆盖拉黑条件，拉黑静默失效。「对象键冲突」是 Strapi 特有问题，重写时不需要处理，但「拉黑条件与游标条件必须同时生效」这个语义必须保留。`article.ts:1961-1966`
- **EDGE-014** 广播时读取文章要用原生查询，不走文档 API（文档 API 会触发 Meilisearch 中间件，请求没有超时且可能报错，导致广播静默丢失）。Strapi 特有，重写时不需要处理。`article-broadcast-bus.ts:159-162`
- **EDGE-015** 不要把 Meilisearch 的同步 HTTP 请求放进写库事务（会长时间持锁，打满连接池）；搜索索引同步应在事务提交后异步执行。`article.ts:1376-1398`
- **EDGE-016** 热榜计分中 c 优先取 firstPublishedAt，编辑后重新发布不会重置时间衰减；新评论只影响 bump（bumpedAt），不改变 c。`tests/unit/hot-rank.test.ts:108-116`
- **EDGE-017** `HOT_COUNT=0` 这类假值配置会回落到默认值（`Number||fallback`）；`HOT_MAX_PER_AUTHOR` 配成非数字时得到 NaN，等同于**不限流**；`PINNED_TTL_SEC` 配成非数字时 setex 失败，退化为每次请求都查 DB。重写时应在启动阶段校验这些配置（见 10 节）。`hot.ts:16-17,187`、`pinned.ts:20`
- **EDGE-018** isHidden 为 NULL 的处理在不同路径上不一致：list、search、pinned、unread 用的是 `<> true` / `$ne:true`，SQL 中 NULL 行会被**排除**；hot、suggested、content-visibility 用的是 `false OR NULL`，NULL 行被**保留**。重写时建议把 isHidden 定义为非空、默认 false，从根源上消除差异。`S/node_modules/@strapi/database/dist/query/helpers/where.js:190-196`、`hot.ts:82`、`content-visibility.ts:10-12`
- **EDGE-019** 无作者的非匿名帖在拉黑过滤下的处理不一致：DB 中的 `NOT IN` 会把它排除；置顶的应用层过滤（`authorId===null`）和 `rejectBlockedItems` 会保留它。`unread-feed.ts:123`、`article.ts:1880,432`
- **EDGE-020** 以下为 Strapi 特有，重写时不需要：同一 documentId 有 draft 和 published 两行，所以 bump 或 pin 都要批量更新所有版本行，且只按 published 行判断；文档 API 的 fields 不能包含 private 字段，因此要单独回填 anonymousSeed；表名和列名从 metadata 取；relation 查询中字段不能与 `$or` 同层混用；插件只在全量重建时推送 settings；`_meilisearch_id` 主键；Strapi 5 加载的是 `src/index.ts`，不是仓库根目录的 `index.ts`。
- **EDGE-021** 以下为 Strapi 特有，重写时不需要：Strapi 的 schema sync 不会为 `unique:true` 真正建立数据库唯一约束，需要在 bootstrap 中补建。重写时应在迁移中直接声明 `article_reads.dedupe_key` 等列的唯一约束。`S/src/index.ts:376-385`

---

## 8 定时任务

- **CRON-001** 热榜刷新：启动时立即计算一次（异步，不阻塞），之后每 `HOT_REFRESH_SEC`（默认 600s，上限 3600s）重算一次；定时器不阻止进程退出（unref），停服时清除；HOT_ENABLED=false 时不启动。这个任务**不在** cron 表中。`hot.ts:212-222`、`S/src/index.ts:63-73`、`S/config/cron-tasks.ts:25-27`
- **CRON-002** 置顶缓存（60s）和相关委托随机池（600s）都没有主动刷新任务，过期后在下一次请求时懒加载。`pinned.ts:20`、`suggested-articles.ts:29`
- **CRON-003** 账号隐私清理重试：cron `* * * * *`（Asia/Shanghai），每次最多处理 20 个 `accountStatus='deleted' AND accountCleanupPending=true` 的用户。`cron-tasks.ts:556-559`、`account-privacy.ts:17-24`
- **CRON-004** 启动时的一次性或幂等任务：backfillBumpedAt、ensureHotpathIndexes（含 `idx_articles_bumped`）、ensureCommentFulltextIndex、syncSearchIndexSettings。`S/src/index.ts:17-42`
- **CRON-005** 诊断脚本 `inspect-hot-ranking.js` 读取 Redis `hot:ranking:diagnostics:v1`，但当前生效的 `hot.ts` **不写这个 key**（它对应的是未接线的 recent / shadow 模式），所以是遗留工具。`S/scripts/inspect-hot-ranking.js:5`

---

## 9 测试中体现的关键用例

- `article-feed-cursor.test.ts`：翻页间隙有帖子被 bump 或被隐藏时，游标分页不重复也不遗漏，offset 分页会重复或漏项；只在没有游标时注入特殊帖，nextCursor 取 normal 流的尾项；带游标时不注入，但仍排除这些帖子，也不计 count；limit ≤ 特殊帖数时不输出游标字段；最后一页和空结果都输出 `hasMore:false, nextCursor:null`，游标模式的 total 为 0；同毫秒帖子靠 documentId 破平；坏游标退回第一页；游标时间规范化为 ISO；cursor 优先于 start；兼容分支的切片和 `total=count+S`；白名单放行 cursor；拉黑超阈值时 hasMore 不误判、游标不停在原地；sort=unread 的装配。
- `article-feed-updates.test.ts`：ids 批次只取请求的已发布卡片，按 bumpedAt 排序，不注入、不计 count；拉黑、匿名、tag 规则照常生效；非法或超过 50 个 id 返回 400；其他 feed 带 ids 返回 400；帖子全部删除时也回执 updateIds；ids 保留在缓存 key 中。
- `article-list.test.ts`：S=6 时 limit≤S 跳过 normal 查询，total=6006；limit=7 时 normal limit 为 1；start≥S 时 normal 从 0 开始；sort=latest 不读热榜但保留置顶；sort=hot 只读浏览流榜单、按榜单分页、不 count；feed=hot 走展示位榜单；following 上的 sort 无效。
- `article-block-cap.test.ts`：排除集合为 199 或 200 时仍走 DB `NOT IN`；201 时改为 3 倍多取并在应用层过滤，同时写 warn；count 与列表条件一致；匿名帖保留；整页被拉黑时返回短页；没有拉黑关系时过滤条件只有 `{isHidden:{$ne:true}}`。
- `unread-feed-database.test.ts`（真实 SQLite）：剔除本人读过的帖子，不受他人阅读记录影响，游标翻页不重不漏；只包含已发布、未隐藏的帖子；分区和标签过滤生效，封禁标签不算命中；DB 和应用层两种拉黑过滤都放行匿名帖；水位生效；重度用户场景下一条查询完成并按 dedupe 索引探测。
- `read-status-watermark.test.ts`：开关打开时，水位之前的帖子免查直接算已读；正好等于水位时仍查记录；bumpedAt 和 updatedAt 不参与判定；取 publishedAt 与 firstPublishedAt 中较晚的一个；开关关闭时不读 up_users；水位查询失败退回精确查询，只记 debug；水位只能前移。
- `hot-rank.test.ts`：公式与 Discourse 一致（新帖用 Lr，老帖用 likescount，没有近期活动时跳过 B 项，gravity 可覆盖）；同一人只算一次；0 分帖子不上榜；老帖可以被新回复带回榜单；firstPublishedAt 优先；浏览流不限流而展示位限流（匿名帖不参与限流）；刷新失败保留旧榜；关闭时返回空。
- `hot-activity.test.ts`（真实 SQLite）：draft 和 published 按 documentId 合并，每人每类行为只算一次且取最早时间；时间窗正确；排除未发布、未来发布、隐藏的文章；排除作者本人、AI、封禁及非 active 账号；排除隐藏评论及祖先被隐藏的评论（包括存在环的情况）；SQL 有界。
- `hot-ranking.test.ts`：只覆盖未接线的 recent 算法（见 HOT-008）。
- `suggested-articles.test.ts`：先给未看帖（同分区优先，排除本人和超过 90 天的帖子），再随机补位；游客跳过未看档，同分区优先；种子帖不存在或非法时返回 null；默认 6 条，最多 10 条。
- `article-suggest.test.ts`：命中全是残留或 Meilisearch 报错时，改用 DB 按标题联想；索引确实没有命中时返回空；单字查询不走 DB。
- `search-fallback-limit.test.ts`：文章和评论的 SQL 兜底都在 1000 处截断并写 warn；total 与 start 无关；先剥运算符再查全文；全是运算符时直接走模糊匹配。
- `comment-search.test.ts`：cover 降为单个对象或 null；命中全是残留时走 SQL；索引确实没有命中时返回空页。
- `meilisearch-config.test.ts`：前缀为空时索引名为 article / comment；有前缀时，搜索和 settings 都使用带前缀的索引名；每次调用都按运行时配置解析；索引名按单个路径段编码；配置类型非法时报错，不回落到生产索引。
- `cache-key.test.ts`：剥除垃圾参数；cursor、tag、ids 进入缓存 key；段边界对齐；Redis 使用 keyPrefix 隔离。
- `article-read-batch.test.ts`：写入并回执；不重复创建；规范化并去重；有缺失文档时在写入之前拒绝；并发重复回读确认；回读也失败时不回执成功；需要登录。
- `content-visibility.test.ts`：在真实查询引擎下，保留 NULL 标志的存量数据，排除隐藏或草稿文章及隐藏的父评论，剔除跨文章的历史回复。

---

## 10 未决问题

1. **待确认**：分区页（category，无 tag）**仍会注入全局置顶帖**，而且不按分区过滤（代码 `article.ts:1876` 只检查 `!tagSlug`，特殊帖查询也不带分区条件），但注释 `article.ts:1872` 说「置顶帖必须满足当前分类/标签」。需要确定是保留「全站公告在每个分区都显示」，还是只显示本分区的置顶。
2. **待确认**：搜索 `sort=latest` 以及 SQL 兜底的 relevance 都按 `updatedAt` 排序，而点赞数、浏览量等物化列的写入会刷新 updatedAt（见 `read-status.ts:24-25`），所以「最新」实际上是「最近有互动」。重写时是改为 publishedAt 或 bumpedAt，还是保持原样？
3. **待确认**：旧实现**没有**在作者改名时更新索引中的 authorName（全库搜不到任何相关逻辑）；只有账号注销时会改成 `已注销用户`。结果是按旧名仍能搜到，显示时则以 DB 回查的名字为准。重写时是否要加「改名后重建该作者相关的索引文档」？
4. **待确认**：isHidden 为 NULL 时的语义（EDGE-018）以及无作者非匿名帖的拉黑语义（EDGE-019）应该统一成哪一种。
5. **待确认**：已读水位（READ-011/012）默认关闭，「全部标为已读」端点也不存在。重写时是否要实现水位？如果实现，详情页和 batch 查询必须同步改为「水位或记录」。
6. **待确认**：关注作者上限 1000 是在查询中截断的，没有指定顺序，关注超过 1000 人的用户会随机丢掉一部分作者。是否需要改成分页或不设上限？
7. **待确认**：`/articles/search` 和 `/articles/suggest` 的 q 没有长度上限，而评论和用户搜索都限制为 32。是否统一？
8. **待确认**：SSE 的 uid 是客户端明文自报的（因为 EventSource 无法携带鉴权头）。新 API 是否改为短期票据或 cookie 鉴权？前端目前只按 category 订阅，没有使用 tag 频道。
9. **待确认**：`hot-ranking.ts` 的 recent 算法、`hot-legacy.ts`、`article-hot-views.ts`（阅读 UV 采集，当前无调用方）以及诊断 key `hot:ranking:diagnostics:v1` 是否全部放弃不迁移？
10. **待确认**：搜索结果卡片中 bumpedAt 恒为 null、isPinned 恒为 false（因为字段没有取）。这是有意为之，还是遗漏？
11. **待确认**：`/articles/suggested/:id` 虽然返回 `Cache-Control: private, no-store`，但它在 strapi-cache 的 `/api/articles` 前缀下，匿名请求可能被缓存 30s（随机结果会被固定一段时间），需要确认插件是否遵循响应头。
12. **待确认**：热门注入在两种情况下都不补位：被过滤掉（隐藏、拉黑、已经置顶）的位置，以及置顶缓存过期后已隐藏的帖子仍被计入 S（`article.ts:1938`）。后一种会导致当页少一条、total 多一。是否要补位或修正 S？
13. **待确认**：用户搜索没有过滤 `accountStatus=deleted|merged` 的账号（只检查了 blocked 和 profileHidden），需要确认注销账号的 author 是否已经被其他流程处理。
