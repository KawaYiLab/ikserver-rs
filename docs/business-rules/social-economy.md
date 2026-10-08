# 业务规则：社交互动与经济系统、个人资料

## 范围

点赞、收藏、关注（含新帖关注者扇出通知）、拉黑（及其在全站的生效点）、一键三连、丁尼（Denny）货币、经验（绳网信用）与等级、签到、等级权益、装扮（系统头像 / 自定义头像 / 论坛名片）、个人资料（改名 / 签名 / 主页隐藏 / 主页精选 / 个人助手占位）、作者主页与用户搜索、注销账号的隐私清理。

不在本文范围：文章 / 评论本身的发布与编辑规则、通知中心与私信的完整规则、入站考试题目与评分规则（这里只记录它们和本领域的交界处）。

约定：
- 路径都相对于旧后端根目录 `InterKnot-web/server/`。
- 文中的 HTTP 路径只用来说明旧接口，重写时不需要保留；但业务行为、数值、错误码和中文文案要保留。
- `documentId` 是文章 / 评论 / 作者的稳定字符串 ID。`user.id` 是用户的数字主键。作者（author）和用户（user）是一对一关系；**关注和拉黑的接口参数都是作者的 documentId**，但拉黑关系存的是 user 之间的关系。
- 「CST」指 UTC+8。
- 「Strapi 特有，重写时不需要」表示这是 Strapi 框架或其 draft/published 双行模型带来的实现细节，新系统只需要保证对应的业务结果。

## 来源文件

- `src/api/like/**`（`controllers/like.ts`、`content-types/like/lifecycles.ts`、`schema.json`、`routes/like-custom.ts`）
- `src/api/favorite/**`（`controllers/favorite.ts`、`content-types/favorite/lifecycles.ts`、`routes/*.ts`）
- `src/api/follow/**`（`controllers/follow.ts`、`content-types/follow/lifecycles.ts`、`schema.json`、`routes/follow-custom.ts`）
- `src/api/user-block/**`（`controllers/user-block.ts`、`content-types/user-block/lifecycles.ts`、`schema.json`、`routes/user-block-custom.ts`）
- `src/api/user-denny/**`、`src/api/denny-given/**`、`src/api/denny-log/**`
- `src/api/check-in/**`、`src/api/check-in-log/**`
- `src/api/user-exp/**`、`src/api/experience-log/**`
- `src/api/benefit/**`、`src/utils/benefits.ts`
- `src/api/avatar/**`、`src/api/business-card/**`、`src/api/me-avatar/**`、`src/api/me-business-card/**`
- `src/api/me-profile/**`、`src/api/author/**`
- `src/utils/denny.ts`、`level.ts`、`triple.ts`、`block.ts`、`fanout-queue.ts`、`self-profile.ts`、`owned-interaction.ts`、`account-privacy.ts`、`account-lock.ts`、`content-visibility.ts`、`common.ts`（`parsePagination`、`buildInteractionDedupeKey`、`isUniqueConstraintViolation`、`batchResolveEquippedAvatars`）
- `src/api/article/utils/follow-notification.ts`
- `src/policies/*.ts`、`src/middlewares/require-exam-passed.ts`
- 交界处：`src/api/article/controllers/article.ts`（详情页拉黑、删帖扣费、发布触发扇出）、`src/api/article/content-types/article/lifecycles.ts`（发帖经验）、`src/api/comment/content-types/comment/lifecycles.ts`（评论经验、评论拉黑）、`src/api/comment/controllers/comment.ts`（采纳经验）、`src/api/exam/services/exam.ts`、`src/utils/exam.ts`（考试奖励）、`src/api/notification/content-types/notification/lifecycles.ts`（通知拉黑）、`src/api/message/controllers/message.ts`、`src/api/conversation/controllers/conversation.ts`（私信拉黑）、`src/utils/meilisearch.ts`、`config/cron-tasks.ts`
- 用户模型：`src/extensions/users-permissions/content-types/user/schema.json`；作者模型：`src/api/author/content-types/author/schema.json`

---

## 0. 通用规则（COMMON）

- `COMMON-001` 通用分页 `parsePagination(start, limit, max=50, default=20)`：`start` 不是整数或小于 0 时取 0；`limit` 不是整数或小于 1 时取 20，否则取 `min(limit, 50)`。返回的 `pageCount = ceil(total/limit)`。来源 `src/utils/common.ts:269-286`。
- `COMMON-002` 批量 check 接口的 ID 列表可以是数组，也可以是逗号分隔的字符串；每项先 trim，再丢弃空串、去重，然后截取前 `MAX_TARGET_IDS = 100` 个。ID 列表为空时直接返回 `{ data: {} }`，不查库。返回值是 `{ [id]: boolean }`，请求里的每个 ID 都必须出现在结果中。来源 `src/api/like/controllers/like.ts:15-47`、`src/api/favorite/controllers/favorite.ts:12-28`、`src/api/follow/controllers/follow.ts:11-27`。
- `COMMON-003` 互动去重键 `buildInteractionDedupeKey`：文章目标为 `u{userId}:a{articleDocumentId}`，评论目标为 `u{userId}:c{commentDocumentId}`。必须恰好有一个目标，否则返回 null。键用 documentId 而不是数字 id，原因是文章重新发布后数字 id 会变，用数字 id 会让同一用户能重复点赞。来源 `src/utils/common.ts:403-432`。
- `COMMON-004` 点赞、收藏、投币、关注、拉黑的唯一性**最终由数据库唯一约束（dedupe_key）保证**；先查一次只是快路径。并发插入撞上唯一约束时，一律当作「已存在」处理，不能返回 500。MySQL `ER_DUP_ENTRY`/errno 1062、PG `23505`、SQLite `SQLITE_CONSTRAINT(_UNIQUE)`/errno 19/`UNIQUE constraint failed` 都算唯一冲突，判断时要逐层向下检查嵌套的 cause（最多 5 层）。来源 `src/utils/common.ts:337-401`。
- `COMMON-005` 写操作门控 `require-exam-passed`：只有 `examPassed || isAdmin || isAiAgent` 的用户才能放行；其他用户返回 403 `EXAM_REQUIRED`「需要先通过入站考试才能进行此操作」。每次请求都按 user id 重新读取，所以用户通过考试后不需要重新登录。未登录返回 401。来源 `src/middlewares/require-exam-passed.ts:16-40`。
- `COMMON-006` 本领域中**受考试门控**的写操作有：点赞 toggle、一键三连、收藏 toggle/create/update/delete、关注 toggle、投币、签到。**不受门控**的有：拉黑、改名、签名、主页隐藏、精选、装备头像 / 名片、上传自定义头像，以及所有只读的 check/list 接口。来源 `src/api/like/routes/like-custom.ts`、`src/api/favorite/routes/*.ts`、`src/api/follow/routes/follow-custom.ts`、`src/api/user-denny/routes/user-denny.ts:18`、`src/api/check-in/routes/check-in.ts:10`、`src/api/me-profile/routes/me-profile.ts`、`src/api/me-avatar/routes/me-avatar.ts`。
- `COMMON-007` 可见文章 `PUBLIC_ARTICLE_FILTER` 的条件是：已发布，且 `isHidden` 为 false 或 NULL。可见评论 `PUBLIC_COMMENT_FILTER` 的条件是：评论未隐藏、所属文章可见，并且评论是顶层评论，或者它的父评论是未隐藏、属于同一篇可见文章的顶层评论（只允许两层）。来源 `src/utils/content-visibility.ts:10-35`、`:157-185`。
- `COMMON-008` 拉黑可见性过滤 `contentAuthorFilter`：排除作者属于「我拉黑的人 ∪ 拉黑我的人」的内容，但**匿名内容（`isAnonymous=true`）永远不按拉黑过滤**，避免通过「内容消失」反推出匿名作者。来源 `src/utils/content-visibility.ts:44-86`、`src/api/article/controllers/article.ts:436-439`。
- `COMMON-009` 「每次写操作都在同一事务中先对 `up_users` 行加 `FOR UPDATE`」是本领域统一的串行化手段，用于加经验、加丁尼、改名、签到、拉黑和资料写入。`withActiveAccountLock` 会在加锁后再检查一次账号状态：`blocked`、`accountStatus≠active` 时抛 403 `ACCOUNT_BLOCKED`「该账号已被封禁或已注销」。来源 `src/utils/account-lock.ts:4-23`。

---

## 1. 点赞（LIKE）

### 数据与约束
- `LIKE-001` 点赞记录包含 `user`、`article?`、`comment?` 和 `dedupeKey`（唯一、私有）。一条记录**必须恰好指向文章或评论之一**，否则报 400「Like target must be either article or comment」（控制器对外文案为「Invalid like target」）。来源 `src/api/like/content-types/like/schema.json`、`lifecycles.ts:63-79`、`controllers/like.ts:153-155`。
- `LIKE-002` 同一用户对同一目标只能有一条点赞，去重键规则见 `COMMON-003`。重复点赞抛 `DuplicateInteractionError('Already liked')`，状态码 400。来源 `lifecycles.ts:88-99`、`:122-135`。
- `LIKE-003` 文章和评论都有物化计数列 `likescount`（非负）。点赞时 +1，取消时 -1，写法是单条原子 UPDATE：`GREATEST(COALESCE(likescount,0)+delta, 0)`，所以计数有下限 0，NULL 按 0 处理。来源 `lifecycles.ts:227-246`。
- `LIKE-004` 文章计数按 `document_id` 更新**所有版本行**；评论按 id 更新。「Strapi 特有，重写时不需要」：draft/published 双行；重写后只需要每篇文章有一个计数。来源 `lifecycles.ts:211-214`、`:234-245`。
- `LIKE-005` 计数增量写失败只记日志，不回滚点赞。每天 04:23（Asia/Shanghai）由对账任务用全量 COUNT 校正 `articles.likescount`、`comments.likescount`、`articles.denny_count`。来源 `config/cron-tasks.ts:166-199`、`:564-569`。

### 权限
- `LIKE-006` toggle 和一键三连要求登录并通过考试（`COMMON-005`）。check 和 my-list 只要求登录。来源 `src/api/like/routes/like-custom.ts`。
- `LIKE-007` 点赞非匿名内容时，如果点赞者和内容作者之间存在任一方向的拉黑，报 400「Cannot like blocked user」。**匿名内容不做拉黑检查**。来源 `lifecycles.ts:101-120`。

### 流程
- `LIKE-008` toggle 的入参是 `targetType ∈ {article, comment}`（trim 后转小写）和 `targetId`（documentId）。非法 targetType 返回 400「Invalid targetType」，缺 targetId 返回 400「Missing targetId」。来源 `controllers/like.ts:52-61`。
- `LIKE-009` toggle 时如果已经点过赞，就**直接删除**，删除前不检查目标是否可见，返回 `{liked:false, likesCount}`。没点过赞时，要求目标满足 `PUBLIC_ARTICLE_FILTER` / `PUBLIC_COMMENT_FILTER`，否则返回 404「Target not found」。创建时撞到唯一冲突，返回 `{liked:true, likesCount}`。来源 `controllers/like.ts:113-159`。
- `LIKE-010` toggle 返回的 `likesCount` 读回物化列：先取该 document 的 published 行；没有 published 行时退回 draft 行；有多行时取最大值；没有行时返回 0。重写时只需返回该文章 / 评论当前的计数。来源 `controllers/like.ts:94-111`。
- `LIKE-011` check 的入参是 `targetType`（必填，非法时返回 400）和 `targetIds`，返回 `{data:{[documentId]:bool}}`。来源 `controllers/like.ts:184-222`。
- `LIKE-012` my-list 的 `targetType` 可选，传入非法值时返回 400。结果按 createdAt 倒序分页（`COMMON-001`），只返回当前用户可见的文章或评论（`buildArticleVisibilityFilter` / `buildCommentVisibilityFilter`，包含拉黑过滤），评论还要再经过 `filterValidComments`。每项包含 `documentId, createdAt, targetType, article{documentId,title,updatedAt,likesCount}|null, comment{documentId,content,createdAt,likesCount,article{documentId,title}}|null`。来源 `controllers/like.ts:224-317`。

### 副作用
- `LIKE-013` 点赞成功后发通知：`type='like'`，`recipient` 为内容作者，`sender` 为点赞者，并携带 Article/Comment。以下情况不发：**匿名文章或匿名评论**、自己点自己、作者没有绑定 user。来源 `lifecycles.ts:291-311`。
- `LIKE-014` 取消点赞时，删除该 sender→recipient、该目标、`type='like'` 的通知（单次最多 500 条）。**删除不按是否已读筛选**，已读的也会删（函数名带 Unread，但实现如此）。删除后要回退通知桶，并让收件人的会话列表 / 未读缓存失效。来源 `lifecycles.ts:313-394`。
- `LIKE-015` 点赞者获得经验：`like_give`，1 点，`dedupMode='daily'`（每个奖励日只有第一次点赞有经验），relatedId 为 `give:{article|comment}:{documentId}`。自己点自己不给；**点赞匿名内容仍然给**（只检查 sender≠作者 user）。**被点赞者没有经验**。来源 `lifecycles.ts:396-424`。
- `LIKE-016` 计数、通知、经验三个副作用互相独立，都是尽力而为：任一失败只记 error，不影响点赞本身，也不影响其他副作用。来源 `lifecycles.ts:158-194`。

### 边界与已知坑
- `LIKE-017` my-list 的 `total` 用的是过滤前的 where 计数，而列表还会被 `filterValidComments` 再过滤一遍，所以 total 可能比实际能翻到的条数大。待确认重写时是否修正。来源 `controllers/like.ts:271-276`。
- `LIKE-018` 对已经不可见（隐藏、拉黑、下线）的目标，仍然可以取消点赞，但不能再点赞。来源 `controllers/like.ts:118-136`。

---

## 2. 收藏（FAV）

### 数据与约束
- `FAV-001` 收藏**只针对文章**，记录包含 `user`、`article`、`dedupeKey`（唯一，`u{userId}:a{articleDocumentId}`）。重复收藏抛 `DuplicateInteractionError('Already favorited')`。来源 `src/api/favorite/content-types/favorite/schema.json`、`lifecycles.ts:92-105`、`:126-139`。
- `FAV-002` 收藏数**没有物化列**，每次实时 COUNT（按文章 documentId 统计）。来源 `controllers/favorite.ts:89-96`、`src/utils/triple.ts:82-117`。
- `FAV-003` 缺少 user 时报「Favorite user is required」，缺少 article 时报「Favorite article is required」；文章对该用户不可见（`findVisibleArticle`：未发布、隐藏、非匿名且双方有拉黑）时报「Favorite article is not available」。都是 400。来源 `lifecycles.ts:84-117`。

### 权限
- `FAV-004` 收藏非匿名文章时，如果双方存在任一方向的拉黑，报 400「Cannot favorite blocked user」；匿名文章不检查拉黑。来源 `lifecycles.ts:119-124`。
- `FAV-005` 收藏的写接口都要求登录并通过考试；check 和 list 只要求登录。通用的 create/update/delete 接口使用 `is-favorite-owner` 策略：没有 id 时，要求 body 中的 `data.user` 等于当前用户；有 id 时，要求该收藏属于当前用户，并且如果 body 带了 `data.user`，它也必须等于当前用户。来源 `src/api/favorite/routes/favorite.ts`、`src/policies/is-favorite-owner.ts:47-66`。
- `FAV-006` 通用 create/update 接口（`writeOwnedInteraction`）**忽略客户端传来的 owner**，强制使用当前用户；`data.article` 必须恰好是一个引用（正整数 id、数字串、documentId 字符串（≤255）、`{id}`、`{documentId}`、只含 1 项的数组，或 `set`/`connect` 包装），否则报 400「Article must contain exactly one relation reference」。文章不可见时返回 404「Article not found」。update 只能修改自己的记录，否则返回 404「Interaction not found」。缺少 `data` 时返回 400「Missing data payload」。来源 `src/utils/owned-interaction.ts:8-120`。

### 流程
- `FAV-007` toggle 的入参是 `targetId`（或 `articleDocumentId`）。已经收藏时直接删除，返回 `{favorited:false, favoritesCount}`。未收藏时要求文章满足 `PUBLIC_ARTICLE_FILTER`，否则返回 404「Target not found」。创建时撞到唯一冲突，返回 `{favorited:true, favoritesCount}`。来源 `controllers/favorite.ts:81-138`。
- `FAV-008` check 按文章 documentId 批量查询（`COMMON-002`）。list 按 createdAt 倒序分页，只包含可见文章（含拉黑过滤），每项只带 `article{documentId, updatedAt}`。来源 `controllers/favorite.ts:35-74`、`:145-178`。

### 副作用
- `FAV-009` 收藏成功后发通知：`type='favorite'`，发给文章作者。匿名文章、自己收藏自己、作者没有 user 时不发；发送失败只记 warn。来源 `lifecycles.ts:142-170`。
- `FAV-010` **取消收藏不会删除已发出的收藏通知**（这一点和点赞不同）。**收藏没有任何经验**（`favorite_received` 只出现在经验分类表中，没有写入方）。来源 `src/api/favorite/content-types/favorite/lifecycles.ts`（没有 afterDelete）、`src/utils/level.ts:20-25`。

---

## 3. 关注（FOLLOW）

### 数据与约束
- `FOLLOW-001` 关注记录包含 `follower`（user）、`following`（**author**）和 `dedupeKey`（唯一，`u{followerUserId}:a{authorDocumentId}`）。重复关注报 `DuplicateInteractionError('Already following')`。来源 `src/api/follow/content-types/follow/schema.json`、`lifecycles.ts:26-67`。
- `FOLLOW-002` 不能关注自己（作者的 user 等于关注者），报 400「Cannot follow yourself」。作者不存在时，toggle 返回 404「Author not found」，在数据层则报「Follow author not found」。来源 `controllers/follow.ts:47-55`、`lifecycles.ts:44-54`。

### 权限
- `FOLLOW-003` toggle 要求登录并通过考试；check 和 following 列表只要求登录。来源 `routes/follow-custom.ts`。
- `FOLLOW-004` **新建关注时**，如果双方存在任一方向的拉黑，返回 400「Cannot follow blocked user」。**取消关注不检查拉黑**。来源 `controllers/follow.ts:70-79`。

### 流程
- `FOLLOW-005` toggle 的入参是 `authorDocumentId`（或 `targetId`），缺失时返回 400「Missing authorDocumentId」。已关注时删除，返回 `{following:false, followersCount}`；否则创建，返回 `{following:true, followersCount}`。并发重复时也返回 `following:true`。`followersCount` 实时统计该作者的被关注数。来源 `controllers/follow.ts:37-97`。
- `FOLLOW-006` check 的入参是 `authorIds`（作者 documentId 列表，规则见 `COMMON-002`）。following 列表按关注时间倒序分页，每项返回 `{documentId, name, avatar}`，其中 avatar **只取 author.avatar 的缩略图**，不看用户装备的系统头像（和其他列表不一致）。来源 `controllers/follow.ts:104-194`。

### 副作用
- `FOLLOW-007` **关注动作本身不发通知，也不加经验**。来源 `src/api/follow/**`（没有通知或经验写入）。
- `FOLLOW-008` 拉黑时会双向删除关注关系，见 `BLOCK-008`。

### 新帖关注者扇出通知（FOLLOW-FANOUT）
- `FOLLOW-009` 触发时机：文章**首次发布**时，在事务提交之后异步入队，覆盖三种情况：新建时直接发布、更新时 `existingPublishedCount===0`、草稿转发布且不是编辑。编辑已发布的文章不触发。来源 `src/api/article/controllers/article.ts:128-132`、`:1268-1269`、`:1435-1436`、`:2938-2940`。
- `FOLLOW-010` 入队只执行 `SET NX ik:fanout:dedupe:follow:{ref} EX 120` 和 `LPUSH ik:fanout:jobs`，不查库。如果去重键已存在，就跳过入队（记 debug 日志）。如果 LPUSH 失败，必须清除去重键。**Redis 不可用时只记 warn，不让发帖失败**，这段时间的通知直接丢失，没有补偿。来源 `src/utils/fanout-queue.ts:26-50`、`:133-166`。
- `FOLLOW-011` 任务结构为 `{type:'follow-fanout', ref(文章 id 或 documentId), cursor(follow.id，不含该 id), attempt, enqueuedAt}`。解析不了的消息进入死信 `ik:fanout:dead-letter`。来源 `src/utils/fanout-queue.ts:64-105`、`:169-188`。
- `FOLLOW-012` 消费方每分钟由 cron 执行一次，**永不抛错**。持有互斥锁 `ik:fanout:lock`（TTL 120s，token 带随机值，释放时用 Lua 比对 token 后再删）才能运行。抢锁成功后，先把 `ik:fanout:inflight` 中的孤儿任务捞回队列（attempt+1，达到上限进死信）。来源 `src/api/article/utils/follow-notification.ts:376-452`、`src/utils/fanout-queue.ts:277-330`。
- `FOLLOW-013` 单轮上限：最多 50 个任务、500 条通知、15 秒，三者撞上任何一个就停。同一 ref 一轮只处理一次，再次取到时放回队列并结束本轮，相当于 1 分钟退避。来源 `follow-notification.ts:56-71`、`:394-437`。
- `FOLLOW-014` 取任务用 RPOPLPUSH 移到 inflight。处理完是终态时，LREM inflight 并**清除入队去重键**，保证下线后再发布时能重新入队。没处理完时带新游标重新入队，attempt 清零。重新入队的顺序是先 LPUSH，再 LREM。来源 `fanout-queue.ts:190-217`、`follow-notification.ts:428-436`。
- `FOLLOW-015` 失败处理：如果本次游标有推进，attempt 清零；没有推进则 attempt+1。`attempt >= FANOUT_MAX_ATTEMPTS = 3` 时进死信（同时 LREM 并清除去重键），否则重新入队。来源 `fanout-queue.ts:52-53`、`:219-268`。
- `FOLLOW-016` 终态丢弃（不重试、不报错）的情况：文章没有 published 行（已删除或已下线）、`isHidden`、**`isAnonymous`**、没有作者。来源 `follow-notification.ts:258-268`。
- `FOLLOW-017` 收件人过滤：follower 不存在、`blocked=true`、`confirmed=false`、`isAiAgent=true`、和作者之间存在任一方向的拉黑、是作者本人时，都不发。来源 `follow-notification.ts:155-166`、`:307-317`。
- `FOLLOW-018` 分页按 `follow.id > cursor`、id 升序，每页 1000 条；每 100 条一批并发发送。一整批发完才推进游标（被过滤掉的人也计入推进），批内任何一条失败，整批抛错。如果整页处理完游标没有前进，就中止分页，防止死循环。来源 `follow-notification.ts:49-56`、`:283-347`。
- `FOLLOW-019` 每个收件人有幂等键 `ik:fanout:sent:article:{documentId}:follower:{userId}`，TTL 7 天，用 SET NX 抢占。抢到后立即落库；**落库失败必须释放这个键再抛错**。键按 documentId 而不是行 id 计算，所以「发布→下线→再发布」7 天内不会重复通知。来源 `follow-notification.ts:58-84`、`:198-230`、`fanout-queue.ts:332-364`。
- `FOLLOW-020` 通知内容：`type='system'`，`recipient` 为关注者，`message = 你关注的 {作者名} 发布了新委托《{标题}》`，作者名依次取 author.name、user.username、`未知用户`，截取 100 字；标题截取 100 字；message 截取 500 字。`Article` 为 published 行 id，`isRead=false`，没有 sender。写库失败必须抛出，不能吞掉（不能复用会吞异常的 `sendSystemNotification`）。来源 `follow-notification.ts:168-196`、`:272-279`。

---

## 4. 拉黑（BLOCK）

### 数据与约束
- `BLOCK-001` 拉黑记录包含 `blocker`（user）、`blockedUser`（user）和 `dedupeKey`（唯一，`u{blockerId}:u{blockedId}`）。**记录不可修改**：update 一律报错「User block entries are immutable」。重复拉黑报 `DuplicateInteractionError('Already blocked')`。来源 `src/api/user-block/content-types/user-block/schema.json`、`lifecycles.ts:4-41`。
- `BLOCK-002` 拉黑是单向记录；但几乎所有生效点都用 `hasAnyBlock(a,b)`，也就是**任一方向的拉黑就生效**。来源 `src/utils/block.ts:130-145`。
- `BLOCK-003` 没有总量上限，只有新增速率限制（`BLOCK-006`）。来源 `src/api/article/controllers/article.ts:374-389`（注释说明 `BLOCK_RATE_PER_DAY` 只限制每天新增）。

### 权限
- `BLOCK-004` toggle、check、my-list 都使用 `require-auth`（会拒绝被封禁的账号），**不要求通过考试**。来源 `routes/user-block-custom.ts`。
- `BLOCK-005` 入参是 `authorDocumentId`（或 `targetId`），缺失时返回 400「Missing authorDocumentId」。作者不存在或没有 user 时返回 404「Author not found」。拉黑自己返回 400「Cannot block yourself」。**不能拉黑 AI 账号**（`isAiAgent=true`），返回 400「Cannot block system account」。来源 `controllers/user-block.ts:53-66`。

### 限流
- `BLOCK-006` toggle 按用户做固定窗口限流：每分钟 `BLOCK_RATE_PER_MIN`（环境变量，默认 10）次，每天 `BLOCK_RATE_PER_DAY`（默认 100）次。日窗口是从第一次计数开始的 86400 秒，不是自然日。自增和设置 TTL 必须原子完成。超限返回 429，`Retry-After` 设为剩余 TTL（没有 TTL 时分钟窗口取 60、日窗口取 86400），body 为 `{error:{code:'TOO_MANY_REQUESTS', message:'操作太频繁，请稍后再试'}}`。Redis 故障时放行。**限流在参数校验之前计数**，所以取消拉黑和非法请求也会消耗额度。来源 `src/utils/block.ts:10-14`、`:176-204`、`controllers/user-block.ts:40-51`。

### 流程
- `BLOCK-007` toggle 在事务中先对拉黑者的 `up_users` 行加锁（MySQL/PG）。已拉黑时删除记录，返回 `{blocked:false, authorDocumentId}`；否则创建，返回 `{blocked:true, authorDocumentId}`。并发撞上唯一冲突时视为 `blocked:true`。来源 `controllers/user-block.ts:68-148`。
- `BLOCK-008` 拉黑时**在同一事务中双向删除关注**：删除「我关注 TA 的作者」和「TA 关注我的作者」两条。**取消拉黑不会恢复关注**。来源 `controllers/user-block.ts:94-131`。
- `BLOCK-009` toggle 完成后，让双方的拉黑缓存都失效。缓存有两类：`user-block:list:{uid}`（我拉黑谁）和 `user-block:by:{uid}`（谁拉黑我），都是 Redis SET，TTL 300 秒；空列表另存一个 `:empty` 占位键，同样 300 秒。读缓存出错时直接查库。来源 `src/utils/block.ts:6-115`、`:147-157`、`controllers/user-block.ts:143-146`。
- `BLOCK-010` check 的入参是 `authorIds`（作者 documentId）。**只返回「我拉黑了 TA」这一方向**，不反映「TA 拉黑了我」。来源 `controllers/user-block.ts:156-189`。
- `BLOCK-011` my-list 按拉黑时间倒序分页，每项返回 `{documentId(作者), name, username, level, avatar, createdAt}`。avatar 优先取装备的系统头像，其次取 author.avatar。没有作者 documentId 的项会被丢弃。来源 `controllers/user-block.ts:195-258`。

### 生效点（凡有拉黑关系即生效；除特别注明外，匿名内容都不受影响）
- `BLOCK-012` 文章详情：非匿名文章，访客和作者之间存在拉黑时，返回 403 `{error:{code:'USER_BLOCKED', message:'无法查看该委托', details:{authorDocumentId, authorName}}}`。来源 `src/api/article/controllers/article.ts:2175-2189`。
- `BLOCK-013` 文章列表、搜索、联想、未读流、置顶、热门、推荐都排除相关作者的非匿名文章。排除集合超过 `BLOCK_EXCLUDE_DB_CAP = 200` 个作者时，列表分页改为「多取 3 倍、在应用层过滤后截回页大小」，此时 total 会偏大，也可能返回不满的一页；搜索和联想仍然在数据库层用 `$notIn`。Meilisearch 侧使用过滤条件 `NOT authorUserId IN [...]`（匿名文章的 authorUserId 为 null，不受影响）。来源 `src/api/article/controllers/article.ts:374-474`、`:1873-2098`、`src/utils/meilisearch.ts:168-173`、`:279`、`src/api/article/services/unread-feed.ts:14`、`src/api/article/services/pinned.ts:12`、`src/utils/suggested-articles.ts:126`。
- `BLOCK-014` 评论列表、回复、评论搜索都排除相关用户的非匿名评论（数据库层加 Meilisearch 层）。来源 `src/api/comment/controllers/comment.ts:164-186`、`src/utils/content-visibility.ts:73-86`。
- `BLOCK-015` 发表评论：非匿名文章的作者与评论者之间存在拉黑时，报「Cannot comment on blocked user」；回复非匿名的父评论且与其作者存在拉黑时，报「Cannot reply to blocked user」。来源 `src/api/comment/content-types/comment/lifecycles.ts:159-171`。
- `BLOCK-016` 点赞、收藏、关注、投币都会被拉黑拦截，分别见 `LIKE-007`、`FAV-004`、`FOLLOW-004`、`DENNY-012`。
- `BLOCK-017` 私信：在私聊会话中向存在拉黑关系的对方发消息，返回 403「cannot send message to blocked user」；和存在拉黑关系的人新建或恢复私聊，返回 403「cannot start conversation with blocked user」。来源 `src/api/message/controllers/message.ts:213-222`、`src/api/conversation/controllers/conversation.ts:839-841`、`:912-913`。
- `BLOCK-018` 通知：任何同时带 sender 和 recipient 的通知（like、favorite、denny、accepted、评论回复、@ 提及等），只要双方存在拉黑，创建时就会抛错「Blocked user cannot receive notification」。没有 sender 的 system 通知不受这条约束，但关注扇出会单独过滤（`FOLLOW-017`）。来源 `src/api/notification/content-types/notification/lifecycles.ts:253-266`。
- `BLOCK-019` 作者主页：双方存在拉黑时，`/profiles/:id` 返回精简版（见 `PROFILE-021`），主页文章和评论列表都返回空页。来源 `src/api/author/controllers/author.ts:241-304`、`:441-450`、`:662-671`。
- `BLOCK-020` 用户搜索：@ 提及搜索和用户搜索页都会排除我拉黑的人和拉黑我的人。来源 `src/api/author/controllers/author.ts:895-933`、`:1048-1087`。
- `BLOCK-021` 我的点赞列表和我的收藏列表按拉黑可见性过滤，被拉黑作者的内容不显示（`COMMON-008`）。来源 `src/api/like/controllers/like.ts:234-248`、`src/api/favorite/controllers/favorite.ts:44-47`。
- `BLOCK-022` @ 提及本身没有单独的拉黑校验，只在 @ 候选搜索（`BLOCK-020`）和提及通知（`BLOCK-018`）两处生效。重写时是否要在提及解析处拦截：待确认。

---

## 5. 一键三连（TRIPLE）

- `TRIPLE-001` 含义：**点赞 + 收藏 + 投 1 丁尼**，作用对象只能是文章。入参是 `articleId`（或 `targetId`），缺失时返回 400「Missing articleId」。文章不满足 `PUBLIC_ARTICLE_FILTER` 时返回 404「Article not found」。要求登录并通过考试。来源 `src/api/like/controllers/like.ts:162-182`、`routes/like-custom.ts:13-22`。
- `TRIPLE-002` 点赞和收藏是「**确保存在**」，而不是 toggle：已经存在时保持原状，不会取消。这一步幂等，并发撞上唯一冲突时按已存在处理。点赞和收藏并行执行，各自的副作用（通知、经验、计数）照常触发。来源 `src/utils/triple.ts:31-117`、`:168-176`。
- `TRIPLE-003` 投币在点赞和收藏都完成后**串行执行**（走 `giveDennyToArticle`，`DENNY-008`）。投币失败是软失败，只通过 `coinReason` 返回原因，**不回滚点赞和收藏**。来源 `src/utils/triple.ts:158-192`。
- `TRIPLE-004` 点赞或收藏任一方抛出真实错误（例如被拉黑时的 400「Cannot like blocked user」）时，整个三连失败，投币不会执行；另一方可能已经写入，不做补偿。来源 `src/utils/triple.ts:163-166`。
- `TRIPLE-005` 返回结构为 `{liked:true, likesCount, favorited:true, favoritesCount, coinGiven, coinReason, dennyCount, newBalance}`。`coinReason ∈ GIVEN|ALREADY_GIVEN|SELF_GIVE|ANONYMOUS_ARTICLE|INSUFFICIENT_BALANCE|ARTICLE_NOT_FOUND|USER_NOT_FOUND|INTERNAL_ERROR|FAILED`。投币成功时 `dennyCount` 为投币后的计数、`newBalance` 为投币后余额；失败时 `dennyCount` 为原计数、`newBalance=null`。`likesCount` 读 published 行的物化列，`favoritesCount` 实时统计。来源 `src/utils/triple.ts:9-29`、`:178-203`。
- `TRIPLE-006` 投币返回的 `USER_BLOCKED` **没有映射**，会落到 `FAILED`。不过一般情况下点赞那一步就已经因为拉黑报错了，走不到这里。来源 `src/utils/triple.ts:123-136`。
- `TRIPLE-007` 三连的成本是 1 丁尼（只有投币成功时才扣），没有额外费用。重复三连：点赞和收藏不变，投币返回 `ALREADY_GIVEN`，不再扣费。来源 `src/utils/triple.ts`、`src/utils/denny.ts:292`。

---

## 6. 丁尼货币（DENNY）

### 数据与约束
- `DENNY-001` 用户字段 `denny` 的默认值是 **30**（新注册用户的初始余额），约束为 `min 0, max 9999`；`denny_given` 表示累计投出的数量，默认 0。来源 `src/extensions/users-permissions/content-types/user/schema.json:147-159`。
- `DENNY-002` 丁尼日志 `denny_logs` 包含 `user, action(枚举), amount(有符号整数), balance(变动后余额), related_type, related_id, description`。`related_id` 和 `description` 截取 255 字符。来源 `src/api/denny-log/content-types/denny-log/schema.json`、`src/utils/denny.ts:51-52`。
- `DENNY-003` action 枚举：`daily_login, level_up, give_post, receive_post_give, spend_rename, spend_avatar, spend_delete_post, refund, admin_grant, exam_passed`。来源 `src/api/denny-log/content-types/denny-log/schema.json:21-35`。
- `DENNY-004` 投币记录 `denny_givens` 包含 `user, article, amount(固定 1，min=max=1), message(≤100), dedupeKey(唯一，u{uid}:a{articleDocumentId})`。通过其他途径（例如后台）创建的记录也必须补上 dedupeKey。来源 `src/api/denny-given/content-types/denny-given/schema.json`、`lifecycles.ts:10-47`。

### 加币 addDenny
- `DENNY-005` amount 必须大于 0，否则返回 null。在事务中先锁定 `up_users` 行。**幂等**：同时提供 `relatedType` 和 `relatedId` 时，如果已经存在相同 `(user, action, related_type, related_id)` 的日志，就直接返回 `{existing:true, added:0}`，不写日志、不改余额。来源 `src/utils/denny.ts:29-91`。
- `DENNY-006` 封顶：`newDenny = min(min(current,9999) + amount, 9999)`，实际增加量为差值。**实际增加量为 0 时不写日志**，余额也不更新。返回 `{newBalance, added, requested, capped: added<amount}`。默认描述为 `Denny +{added} (capped from {amount})`。异常时返回 null。来源 `src/utils/denny.ts:93-136`。

### 扣币 deductDenny
- `DENNY-007` amount 必须大于 0。扣减用一条条件 UPDATE 原子完成：`WHERE id=? AND denny >= amount`，`denny -= amount`。影响 0 行时返回 `{success:false, error:'INSUFFICIENT_BALANCE', currentBalance}`（用户不存在时返回 null）。成功后写一条 `amount=-amount` 的日志，日志写入失败时整个事务回滚。**扣币没有幂等检查**。默认描述为 `Denny -{amount}`。来源 `src/utils/denny.ts:151-251`。

### 投币 giveDennyToArticle
- `DENNY-008` 每次投币固定为 **1** 丁尼；每个用户对每篇文章**终身只能投一次**。处理顺序：文章必须已发布（`ARTICLE_NOT_FOUND`）→ 不能是匿名文章（`ANONYMOUS_ARTICLE`）→ 不能是自己的文章（`SELF_GIVE`）→ 双方不能存在拉黑（`USER_BLOCKED`）→ 按 user id 升序锁定投币者和作者两行 → 检查是否投过（`ALREADY_GIVEN`）→ 检查余额（`INSUFFICIENT_BALANCE`，`currentBalance`）。来源 `src/utils/denny.ts:286-374`。
- `DENNY-009` 投币成功时，在同一事务中完成：投币者 `denny-1`、`denny_given+1`；写投币记录（message 截取 100 字）；文章 `denny_count = COALESCE(denny_count,0)+1`（原子自增，按 document_id 更新所有版本行），然后读回 published 行的计数作为 `articleDennyCount`；作者 `denny+1`，封顶为 **99999999**（不是 9999）；投币者写日志 `give_post, -1, related_type='api::article.article', related_id=文章 documentId, description='投币给帖子 {id}'`；作者余额大于 0 时写日志 `receive_post_give, +1, description='收到帖子投币: {id}'`。来源 `src/utils/denny.ts:376-472`。
- `DENNY-010` 并发下第二次投币撞上 dedupeKey 唯一约束时，**必须整体回滚**（包括已经扣掉的币），对外返回 `ALREADY_GIVEN`。其他异常返回 `INTERNAL_ERROR`「Internal server error」。来源 `src/utils/denny.ts:539-550`。
- `DENNY-011` 投币通知**必须在事务提交后**才创建（`type='denny'`，recipient 为作者，sender 为投币者，Article 为文章数字 id，isRead=false）。通知创建失败只记 warn，不回滚，也不影响返回值。作者没有 user 或者是自己的文章时，不发通知。来源 `src/utils/denny.ts:475-512`。
- `DENNY-012` 提交后再给作者加经验：`denny_received`，1 点，relatedType `api::denny-given.denny-given`，relatedId `receive:{giverUserId}:article:{articleDocumentId}`（按实体去重），计入「被动互动」每日 1000 的上限。来源 `src/utils/denny.ts:514-536`、`src/utils/level.ts:20-27`。
- `DENNY-013` 投币接口：`articleId` 必须是长度不超过 100 的字符串，否则返回 400 `INVALID_PARAMS`「articleId is required and must be a valid string.」。`message` 截取 100 字并删除 `<>"'&` 字符。错误码到 HTTP 状态的映射：`ARTICLE_NOT_FOUND 404, ANONYMOUS_ARTICLE 403, SELF_GIVE 403, ALREADY_GIVEN 409, INSUFFICIENT_BALANCE 402, INTERNAL_ERROR 500`，其余（包括 `USER_BLOCKED`、`USER_NOT_FOUND`）返回 400。错误 body 只包含 `{error:{code,message}}`。成功返回 `{success:true, message:'Successfully given 1 Denny to the article.', newBalance, articleDennyCount}`。来源 `src/api/user-denny/controllers/user-denny.ts:53-108`。
- `DENNY-014` 余额接口返回 `{denny, dennyGiven, recentLogs}`，`recentLogs` 是最近 10 条日志（`action, amount, balance, description, createdAt`，按时间倒序）。读取失败时返回 500 `FETCH_FAILED`。**没有对外的扣币接口**。来源 `src/api/user-denny/controllers/user-denny.ts:12-45`、`routes/user-denny.ts:21`。

### 消费与退款
- `DENNY-015` 改名 `spend_rename`：扣 10，描述为 `改名: {旧名} -> {新名}`，没有 related，和改名在同一事务中（`PROFILE-004`）。来源 `src/api/me-profile/controllers/me-profile.ts:157-175`。
- `DENNY-016` 自定义头像 `spend_avatar`：扣 10，描述为「设置自定义头像」，没有 related，和换头像在同一事务中（`COSMETIC-006`）。来源 `src/api/me-avatar/controllers/me-avatar.ts:103-112`。
- `DENNY-017` 删帖 `spend_delete_post`：**只有存在已发布版本时**才扣 10（只删草稿免费），related 为 `('api::article.article', documentId)`。扣费失败（返回 null）时返回 500「扣除丁尼失败，请稍后重试」；余额不足时返回 400「丁尼余额不足」。扣费和删除**不在同一事务中**：删除失败时调用 `addDenny(10, 'refund', '删除帖子失败退款: {id}', 'api::article.article', 'refund:{documentId}')` 退款（按 relatedId 幂等），并返回 500「删除帖子失败，已退还丁尼」。只删草稿失败时返回 500「删除草稿失败」。成功时返回中带 `newBalance`。来源 `src/api/article/controllers/article.ts:3109-3158`。
- `DENNY-018` 签到 `daily_login`：+10（受 9999 封顶），related 为 `('plugin::users-permissions.user', '{userId}_{签到日}')`，用于幂等。描述为 `每日签到奖励 - 第{N}天`。来源 `src/api/check-in/controllers/check-in.ts:206-214`。
- `DENNY-019` 考试 `exam_passed`：**首次**通过时加 `EXAM_REWARD_DENNY`（环境变量，默认 50），related 为 `('plugin::users-permissions.user', userId)`，用于幂等，描述为「通过入站考试奖励」。加币失败时让考试提交的事务失败（抛出 `EXAM_DENNY_REWARD_FAILED`）。来源 `src/utils/exam.ts:24`、`src/api/exam/services/exam.ts:247-275`。
- `DENNY-020` `level_up` 和 `admin_grant` **只存在于枚举中，代码里没有任何写入方**：升级时不发丁尼，也没有管理员发币接口（可能只能通过 Strapi 后台手工写日志）。见未决问题。来源 `grep` 结果，只出现在 `src/api/denny-log/content-types/denny-log/schema.json:25`、`:32`。

### 丁尼动作总表

| action | 金额 | 方向 | 触发 | 幂等键 / 去重 | 备注 |
|---|---|---|---|---|---|
| `daily_login` | +10 | 入 | 签到成功 | `(plugin::users-permissions.user, {uid}_{YYYY-MM-DD})` | 9999 封顶，封顶时不写日志 |
| `give_post` | -1 | 出 | 投币 | 投币记录 dedupeKey `u{uid}:a{docId}`（终身一次） | 同时 `denny_given+1` |
| `receive_post_give` | +1 | 入 | 被投币 | 随 `give_post` 一起发生，没有独立幂等 | 封顶 99999999（和 9999 不一致） |
| `spend_rename` | -10 | 出 | 改名 | 无（同一事务加行锁） | 余额不足时报「丁尼余额不足」 |
| `spend_avatar` | -10 | 出 | 上传自定义头像 | 无 | 同上 |
| `spend_delete_post` | -10 | 出 | 删除已发布文章 | 无（related 为文章 documentId，但不检查） | 只删草稿免费 |
| `refund` | +10 | 入 | 删帖失败退款 | `(api::article.article, refund:{docId})` | 9999 封顶 |
| `exam_passed` | +`EXAM_REWARD_DENNY`（默认 50） | 入 | 首次通过考试 | `(plugin::users-permissions.user, {uid})` | 9999 封顶 |
| `level_up` | — | — | 无写入方 | — | 待确认 |
| `admin_grant` | — | — | 无写入方 | — | 待确认 |

---

## 7. 经验与等级（EXP）

### 等级
- `EXP-001` 等级阈值 `[0, 500, 2000, 6000, 15000, 35000, 80000]` 分别对应 Lv.1 到 Lv.7，最高 7 级。经验为负数时按 Lv.1 计算。来源 `src/utils/level.ts:3-18`。
- `EXP-002` **没有考试豁免的用户，等级固定为 0**：豁免条件为 `examPassed || isAiAgent || isAdmin`，否则每次加经验后 `level=0`。经验照常累积，不会丢失。来源 `src/utils/level.ts:156-158`。
- `EXP-003` 首次通过考试时，立即把等级设为 `max(1, calculateLevel(exp))`，然后发放考试经验和丁尼奖励（同一事务，`DENNY-019`）。来源 `src/api/exam/services/exam.ts:253-273`。
- `EXP-004` 用户字段 `exp` 默认 0；`level` 在 schema 中默认 **1**，但新用户还没有通过考试，第一次获得经验后就会被写成 0。重写时新用户应直接是 0：待确认。来源 `src/extensions/users-permissions/content-types/user/schema.json:107-116`。
- `EXP-005` **升级没有任何奖励**（不发丁尼，也不发通知）。来源 `src/utils/level.ts`（没有相关逻辑）。

### 加经验 addExp
- `EXP-006` 在事务中先锁定 `up_users` 行，串行化同一用户的并发加经验，防止每日上限被击穿。来源 `src/utils/level.ts:72-76`。
- `EXP-007` 奖励日的边界是 **CST 04:00**，即 `[当日 04:00 CST, 次日 04:00 CST)`；CST 04:00 之前算前一天。这个计算不受服务器时区影响。来源 `src/utils/level.ts:31-53`。
- `EXP-008` 去重模式：`entity`（默认）在提供了 relatedType 和 relatedId 时，如果已经存在相同 `(user, action, related_type, related_id)` 的日志就跳过；`daily` 则按 `(user, action, 当前奖励日)`，当天已经有该 action 的日志就跳过。跳过时返回 null。来源 `src/utils/level.ts:83-114`。
- `EXP-009` 每日上限分两个桶：「被动互动」桶包含 `like_received, comment_received, favorite_received, denny_received`，上限为 **1000**；其余 action 都属于「主动」桶，上限为 **50**（`SELF_INITIATED_DAILY_EXP_CAP`）。`exam_passed` **豁免上限**，全额发放，并且不占用当天额度。当天已获得的数量只统计同一桶内的日志。来源 `src/utils/level.ts:20-29`、`:124-153`。
- `EXP-010` 实际发放量为 `min(max(0, amount), remaining)`。**发放量为 0 时不写日志**（因此 `daily` 模式下，被上限挡住的那次不会占用「今日首次」的名额）。只有经验或等级发生变化时才更新用户。返回 `{newExp, newLevel, added, requested, capped, dailyGained, dailyCap, dailyScope}`。来源 `src/utils/level.ts:150-194`。
- `EXP-011` 经验日志 `experience_logs` 包含 `user, action(字符串), exp(实际发放量), related_type, related_id, description`。description 格式为 `Experience change: {granted}/{amount} (Total: {newExp}, Daily {scope}: {x}/{cap})`。来源 `src/api/experience-log/content-types/experience-log/schema.json`、`src/utils/level.ts:161-172`。

### 经验动作总表

| action | 经验 | 上限桶 | 去重模式 / relatedId | 触发与条件 | 来源 |
|---|---|---|---|---|---|
| `check_in` | 6 + min(max(连签天数-1,0),4)，即 6~10 | 主动 50 | entity，`(plugin::users-permissions.user, {uid}_{签到日})` | 签到成功 | `src/api/check-in/controllers/check-in.ts:9-17`、`:201` |
| `create_article` | 4 | 主动 50 | daily；relatedId 为文章 documentId | 首次发布（新建时直接发布，或草稿首次发布）；编辑重发时先按实体检查，已发过就跳过 | `src/api/article/content-types/article/lifecycles.ts:43-114` |
| `create_comment` | 3 | 主动 50 | daily；relatedId `user:{uid}:article:{articleDocId}` | 在**别人的**文章下评论（在自己文章下评论不给） | `src/api/comment/content-types/comment/lifecycles.ts:442-469` |
| `like_give` | 1 | 主动 50 | daily；relatedId `give:{article|comment}:{docId}` | 点赞别人的内容（包括匿名内容） | `src/api/like/content-types/like/lifecycles.ts:396-424` |
| `comment_accepted` | 10 | 主动 50 | entity，`(api::comment.comment, comment:{commentId})` | 自己的评论被别人采纳；自己采纳自己不给；匿名也给经验（只是不发通知） | `src/api/comment/controllers/comment.ts:905-931` |
| `denny_received` | 1 | 被动 1000 | entity，`(api::denny-given.denny-given, receive:{giverUid}:article:{docId})` | 自己的文章收到投币 | `src/utils/denny.ts:514-536` |
| `exam_passed` | `EXAM_REWARD_EXP`（默认 50） | 豁免 | entity，`(plugin::users-permissions.user, {uid})` | 首次通过考试 | `src/utils/exam.ts:25`、`src/api/exam/services/exam.ts:270-273` |
| `like_received` / `comment_received` / `favorite_received` | — | 被动 1000 | — | **没有写入方**，只用于统计历史日志的额度 | `src/utils/level.ts:20-25` |

- `EXP-012` 主动桶一天最多 50 点，而签到最多 10、发帖 4、评论 3、点赞 1 都是「每日首次」，所以实际上限主要由 `comment_accepted`（每条 10）决定。签到经验也计入 50；如果当天额度已经被占满，签到经验会是 0，但签到本身仍然成功，丁尼照发。来源 `src/utils/level.ts:128-153`、`src/api/check-in/controllers/check-in.ts:201-204`。
- `EXP-013` 经验查询接口返回 `{exp, level(缺失时为 1), lastCheckInDate, consecutiveCheckInDays, canCheckIn, nextEligibleAt(仅在不能签到时返回)}`。每日状态接口返回 `{todaySelfGained, todaySelfCap:50, sources:{checkIn, createArticle, createComment, likeGive}}`，每项为 `{done, exp}`；`todaySelfGained` **只把这 4 项相加，不包含 `comment_accepted`**，所以和真实的已用额度可能不一致。来源 `src/api/user-exp/controllers/user-exp.ts:14-166`。
- `EXP-014` 经验日志的 relatedId 用 `String(relatedId)` 写入；调用方没有传时会存成字面量 `"undefined"`。「Strapi 特有，重写时不需要」：重写时没有 relatedId 就存 NULL。来源 `src/utils/level.ts:168`。

---

## 8. 签到（CHECKIN）

- `CHECKIN-001` 签到日的边界是 **04:00**，04:00 之前算前一天，日期格式为 `YYYY-MM-DD`。**注意**：签到日用的是服务器本地时区（`Date#getHours`、`formatLocalDate`），而经验奖励日固定用 CST。生产服务器的时区是否为 CST：待确认；重写时建议统一为 CST 04:00。来源 `src/api/check-in/controllers/check-in.ts:19-35`、`src/utils/common.ts:434-439`。
- `CHECKIN-002` 每个用户每个签到日只能签到一次，用一条 CAS 实现：在锁定 `up_users` 的事务中执行 `UPDATE ... SET lastCheckInDate=today, consecutiveCheckInDays=N WHERE id=? AND (lastCheckInDate IS NULL OR lastCheckInDate<>today)`，影响行数不是 1 时视为已经签到。来源 `src/api/check-in/controllers/check-in.ts:171-189`。
- `CHECKIN-003` 连签计算：上次签到日正好是前一个签到日时，`N = 已存连签天数 + 1`，否则 `N = 1`。`consecutiveCheckInDays` 在断签时**不会清零**，要到下次签到时才重新计算。来源 `src/api/check-in/controllers/check-in.ts:44-61`、`:151-157`。
- `CHECKIN-004` 奖励：经验为 `6 + min(max(N-1,0), 4)`，也就是连签第 5 天起固定为 10；丁尼固定 +10（`daily_login`）。CAS 闸门、签到日志、经验、丁尼在**同一事务**中完成，任何一步失败（包括 addExp 或 addDenny 返回 null）都会整体回滚，并返回 500 `CHECK_IN_FAILED`「Check-in failed, please try again.」`{details:{checkInDay}}`。来源 `src/api/check-in/controllers/check-in.ts:9-17`、`:159-239`。
- `CHECKIN-005` 事务内部还要再检查一次账号必须为 active（`requireActiveAccount`）。被封禁的账号会在这里抛错，最终被统一映射为 500 `CHECK_IN_FAILED`，而不是 403。来源 `src/api/check-in/controllers/check-in.ts:174`、`:233-238`。
- `CHECKIN-006` 已经签到时返回 409 `CHECK_IN_ALREADY_TODAY`「Already checked in for the current check-in day.」`{details:{checkInDay, nextEligibleAt(ISO，下一次 04:00), rank}}`。CAS 前的快速检查和 CAS 失败都返回同样的结果。来源 `src/api/check-in/controllers/check-in.ts:141-149`、`:220-228`。
- `CHECKIN-007` 签到日志 `check_in_logs` 包含 `user, checkInDay(date), rank(≥1)`。**当日排名**在事务提交后计算：统计同一签到日中 `createdAt < 我的` 或 `(createdAt 相等且 id <= 我的)` 的记录数，算出后写回日志。之后查询时优先使用已存的 rank，没有就补算一次。来源 `src/api/check-in-log/content-types/check-in-log/schema.json`、`src/api/check-in/controllers/check-in.ts:63-98`、`:241-242`。
- `CHECKIN-008` 签到成功返回 `{message:'Check-in successful', reward(经验), consecutiveDays, totalDays(该用户签到日志总数), rank, currentExp, currentLevel, currentDenny, dennyAdded, dennyCapped, nextEligibleAt}`。来源 `src/api/check-in/controllers/check-in.ts:243-259`。
- `CHECKIN-009` 状态接口返回 `{canCheckIn, totalDays, consecutiveDays(数据库原值，未清零), rank(能签到时为 0), checkInDay, nextEligibleAt(能签到时为 null), currentDenny, nextConsecutiveDays, nextReward}`。能签到时，`nextConsecutiveDays` 是按断签规则预算出的「现在签到会得到的天数」；已签到时就是已存的天数。来源 `src/api/check-in/controllers/check-in.ts:266-314`。
- `CHECKIN-010` 签到要求登录并通过考试；状态查询只要求登录。来源 `src/api/check-in/routes/check-in.ts`。

---

## 9. 等级权益（BENEFIT）

- `BENEFIT-001` 权益矩阵（下标即等级，0 表示未通过考试）：
  - `articleMaxImages`（发帖封面图数量上限）：`[0, 9, 12, 15, 18, 24, 30, 40]`
  - `commentMaxImages`（评论图片数量上限）：`[0, 3, 6, 9, 9, 12, 12, 18]`
  - `articleMaxBody`（文章正文字数上限）：`[0, 3000, 4000, 6000, 8000, 10000, 13000, 16000]`
  来源 `src/utils/benefits.ts:6-15`。
- `BENEFIT-002` 权益等级：未登录为 0；`isAdmin` 或 `isAiAgent` 直接取最高档 7；其他用户取存储的 `level`，向下取整并夹到 `[0,7]`。来源 `src/utils/benefits.ts:19-31`。
- `BENEFIT-003` 服务端是配额的唯一裁决方。生效位置：发帖、编辑、草稿转发布时校验正文字数和封面图数量；发评论时校验图片数量。前端保留一份同样的矩阵，只用于展示。来源 `src/utils/benefits.ts:2-5`、`src/api/article/controllers/article.ts:1212-1221`、`:1313-1322`、`:2897-2899`、`src/api/comment/controllers/comment.ts:597`。
- `BENEFIT-004` 权益查询接口公开（登录可选），返回 `{level, maxLevel:7, benefits, nextLevel?, nextBenefits?}`；已经是 7 级时不返回 next 字段。来源 `src/api/benefit/controllers/benefit.ts:12-25`、`routes/benefit.ts`。

---

## 10. 装扮：系统头像、自定义头像、论坛名片（COSMETIC）

- `COSMETIC-001` 系统头像 `avatars` 包含 `name(必填), type ∈ {character, city, news}(默认 character), image(只能是图片)`；论坛名片 `business_cards` 包含 `name, description(text), story(富文本 blocks), type(同一枚举), image`。两者只由管理员维护，公开接口只读（find/findOne）。来源 `src/api/avatar/content-types/avatar/schema.json`、`src/api/business-card/content-types/business-card/schema.json`、`src/api/avatar/routes/avatar.ts`、`src/api/business-card/routes/business-card.ts`。
- `COSMETIC-002` 用户的装备字段为 `equippedAvatar` 和 `equippedCard`（各自一对一，可以为 null）。**所有头像、名片都免费、对所有人开放**，没有等级或解锁限制。来源 `src/extensions/users-permissions/content-types/user/schema.json:132-141`、`src/api/me-avatar/controllers/me-avatar.ts:48-76`、`src/api/me-business-card/controllers/me-business-card.ts:98-126`。
- `COSMETIC-003` 装备操作的入参是 `documentId`：非空时要求对应的头像或名片存在，否则返回 404「Avatar not found.」/「Card not found.」；为空或缺失时表示卸下。写入时加账号锁，并检查账号状态（`COMMON-009`）。返回 `{success:true, equippedAvatarDocumentId|equippedCardDocumentId}`。来源 同上。
- `COSMETIC-004` 头像列表一次返回全部，按 `type asc, createdAt asc` 排序，同时返回 `equippedAvatarDocumentId`。名片列表分页：`page` 默认 1，`pageSize` 默认 30、最多 60，非法值回退到默认值；可按 `type` 过滤，type 非法时返回 400「Invalid card type.」且不查库；排序为 `type asc, createdAt asc, id asc`；同时返回 `equippedCardDocumentId` 和 `equippedCard`。来源 `src/api/me-avatar/controllers/me-avatar.ts:20-46`、`src/api/me-business-card/controllers/me-business-card.ts:6-96`。
- `COSMETIC-005` 上传自定义头像的入参是 `fileId`（正数，必填，否则返回 400「fileId is required」）。文件不存在时返回 404「Uploaded file not found.」；用户没有作者档案时返回 400「Author profile not found.」。来源 `src/api/me-avatar/controllers/me-avatar.ts:78-101`。
- `COSMETIC-006` 自定义头像**收费 10 丁尼**（`spend_avatar`）。在账号锁事务中依次：扣费 → 设置 `author.avatar = 该文件` → **清空 `equippedAvatar`**。余额不足时返回 400「丁尼余额不足」；其他失败回滚并返回 500「设置头像失败，未扣除丁尼」；账号被封禁时继续向上抛 `ACCOUNT_BLOCKED`。成功返回 `{success:true, avatar:{url, width, height}}`。来源 `src/api/me-avatar/controllers/me-avatar.ts:103-122`。
- `COSMETIC-007` 上传自定义头像**不校验 fileId 是否为当前用户上传**，也不限制大小和类型（只要求是已存在的上传文件）。重写时应改为只能使用自己上传的图片：待确认。来源 `src/api/me-avatar/controllers/me-avatar.ts:88-93`。
- `COSMETIC-008` 头像显示优先级：大多数场景（自己的资料、作者主页文章列表、评论、拉黑列表、用户搜索）都是**装备的系统头像优先，其次 author.avatar**；已注销（`deleted`）或已合并（`merged`）账号的装备头像不显示。例外：作者主页 `/profiles/:id` 的完整视图中，`avatar` 字段只给 author.avatar，`equippedAvatar` 单独给；拉黑视图是 `author.avatar ?? equippedAvatar.image`；关注列表只给 author.avatar。来源 `src/utils/common.ts:441-472`、`src/utils/self-profile.ts:23-27`、`src/api/author/controllers/author.ts:258-260`、`:336-371`、`:478-484`、`src/api/follow/controllers/follow.ts:174-180`。
- `COSMETIC-009` 名片图片输出使用原尺寸 webp 地址；头像输出使用 360 缩略图。「Strapi 特有，重写时不需要」：具体的 CDN 地址改写规则。来源 `src/api/me-business-card/controllers/me-business-card.ts:27-40`、`src/utils/common.ts:288-303`。

---

## 11. 个人资料与作者主页（PROFILE）

### 自己的资料
- `PROFILE-001` 自己的资料（`/me/profile` 和登录响应共用同一个投影）只按白名单输出：`id, documentId, username, email, exp, level(缺失时为 1), denny(缺失时为 0), profileHidden, isAdmin, isAiAgent, examPassed, examPassedAt, author{documentId, name, avatar}|null`。avatar 优先取装备的系统头像，其次 author.avatar。**不能把原始的 user 对象直接展开输出**。刷新 token 时不加载资料。来源 `src/utils/self-profile.ts:5-70`。

### 改名
- `PROFILE-002` 新名称先 trim，长度必须在 **3 到 20** 个字符之间（按 JS 字符串长度，即 UTF-16 码元计数），否则返回 400「用户名长度需在 3-20 个字符之间」。**没有字符集限制，也没有冷却时间**。来源 `src/api/me-profile/controllers/me-profile.ts:9-10`、`:92-99`。
- `PROFILE-003` 新名称和当前用户名相同时，直接返回成功，不扣费、不开事务。来源 `src/api/me-profile/controllers/me-profile.ts:101-103`、`:125`。
- `PROFILE-004` 唯一性检查对象是 `up_users.username`（排除自己）：事务外先查一次，事务内再查一次，数据库唯一约束兜底。三处冲突都返回 400「该用户名已被占用」，且不扣费（事务回滚）。大小写是否敏感取决于数据库排序规则：待确认。来源 `src/api/me-profile/controllers/me-profile.ts:105-118`、`:127-140`、`:209-211`。
- `PROFILE-005` 改名的事务流程：锁定 `up_users` 行 → 检查账号为 active → 再次检查唯一性 → 必须有作者档案（否则返回 404「Author profile not found.」，不扣费）→ 扣 10 丁尼（`spend_rename`；余额不足时返回 400「丁尼余额不足」）→ **同时**更新 `author.name` 和 `user.username`。任何异常都整体回滚；扣费返回 null 时返回 500「扣除丁尼失败，请稍后重试」；其他异常返回 500「改名失败，请稍后重试」；`ACCOUNT_BLOCKED` 继续向上抛。成功返回 `{success:true, name}`。**每次改名都收费，包括第一次**。来源 `src/api/me-profile/controllers/me-profile.ts:120-214`。
- `PROFILE-006` 余额刚好 10 时可以改名，改完余额为 0；余额为 9 时拒绝且余额不变。同一用户只有 15 丁尼时并发改名两次，只有一次成功。两个用户并发抢同一个名字时，失败的一方不扣费。来源 `tests/unit/adversarial-rename-transaction.test.ts:401-443`、`:445-498`、`:574-606`。

### 签名、主页隐藏
- `PROFILE-007` 签名先 trim，最多 **100** 个字符，超出时返回 400「签名长度不能超过 100 个字符」。存储为富文本 blocks：非空时为一个段落，空字符串时为 `[]`。没有作者档案时返回 404。写入时加账号锁。返回 `{success:true, bio}`。不收费。来源 `src/api/me-profile/controllers/me-profile.ts:11`、`:217-262`。
- `PROFILE-008` `profileHidden` 必须是布尔值，否则返回 400「profileHidden must be a boolean」。写入时加账号锁。返回 `{success:true, profileHidden}`。来源 `src/api/me-profile/controllers/me-profile.ts:67-84`。
- `PROFILE-009` `profileHidden=true` 的效果：非本人查看主页时，得到隐藏视图（`PROFILE-022`），主页文章和评论列表返回空页，也不会出现在 @ 搜索和用户搜索中。**不影响**文章和评论本身在信息流、详情页中的可见性。来源 `src/api/author/controllers/author.ts:217`、`:306-334`、`:452-458`、`:673-679`、`:925-933`、`:1078-1087`。

### 主页精选（置顶）
- `PROFILE-010` `author.profilePinnedArticles` 是 JSON 字段：`null` 表示未配置（默认按更新时间分页展示），`[]` 表示主动清空（主页不显示任何文章），字符串数组表示精选文章的 documentId，**有序**。最多 **6** 篇。来源 `src/api/author/content-types/author/schema.json`、`src/api/me-profile/controllers/me-profile.ts:12`、`src/api/author/controllers/author.ts:26`、`:72-91`。
- `PROFILE-011` 保存时入参 `pinned` 必须是 `null` 或字符串数组，否则返回 400「pinned must be null or string[]」。处理方式：trim，丢弃空串，去重并保持顺序；超过 6 篇返回 400「At most 6 articles can be pinned」（在去掉失效项之前就判断）。来源 `src/api/me-profile/controllers/me-profile.ts:34-48`、`:378-385`。
- `PROFILE-012` 保存时只保留「自己的、已发布的、**非匿名的**」文章，失效的 ID **静默剔除**，不会让整个保存失败。隐藏的文章不会被剔除。返回 `{pinned: 实际保存的值}`。来源 `src/api/me-profile/controllers/me-profile.ts:401-426`。
- `PROFILE-013` 读取精选候选：候选为自己已发布的非匿名文章，按 updatedAt 倒序；`limit` 默认 50、最多 100；`start` 不小于 0；`q` 先 trim，截取 50 字，按标题做不区分大小写的模糊匹配。返回 `{pinned, pinnedItems?(只在 start=0 时返回，按保存的顺序给出完整卡片), candidates, max:6, total, start, limit}`。卡片格式为 `{documentId, title, cover(第一张封面的缩略图), updatedAt}`。来源 `src/api/me-profile/controllers/me-profile.ts:264-370`。

### 作者主页
- `PROFILE-014` `/profiles/:documentId` 公开访问（登录可选），作者不存在时返回 404。来源 `src/api/author/routes/author-profile.ts`、`controllers/author.ts:96-121`。
- `PROFILE-015` 主页统计**只计非匿名内容**，对作者本人也一样：`articleCount` 为已发布的非匿名文章数；`commentCount` 为挂在文章下的非匿名评论数；`totalViews` 为这些文章的浏览量之和；`totalComments` 和 `totalLikes` 分别为这些文章下的评论总数和点赞总数，**这两项包含匿名评论，也不过滤隐藏评论**。来源 `src/api/author/controllers/author.ts:151-210`。
- `PROFILE-016` 主页关注信息：`followersCount` 为该作者的被关注数，`followingCount` 为该用户关注的作者数，`isFollowing` 为访客是否已关注（访客是本人或未登录时为 false）。来源 `src/api/author/controllers/author.ts:219-239`。
- `PROFILE-017` 完整视图包含：`userId, documentId, name, bio(原始 blocks), avatar, user{username, level, exp}, equippedCard{documentId, name, description, type, image}, equippedAvatar{documentId, name, type, image}, zzz{uid, nickname, level, regionName}|null(米游社绑定), isSelf, isHidden:false, profileHidden, isAiAgent, isBlockedByMe:false, hasBlockedMe:false, isFollowing, followersCount, followingCount, stats`。来源 `src/api/author/controllers/author.ts:336-397`。
- `PROFILE-018` 主页文章列表：`limit` 默认 **10**、最多 50。**匿名文章对所有人（包括本人）都不显示**；隐藏文章只有本人能看到；只返回已发布的文章。来源 `src/api/author/controllers/author.ts:400-493`。
- `PROFILE-019` 主页文章列表的精选模式：`profilePinnedArticles` 不为 null 且没有传 `scope=all` 时，按精选顺序返回，最多 6 篇，跳过已失效的文章；分页信息固定为 `{start:0, limit:6, total, pageCount: total>0?1:0}`，`meta.pinned=true`，`meta.articleTotal` 为全部可见文章数。传 `scope=all` 时忽略精选，普通分页，并附带 `meta.pinnedIds`（为 null 时表示使用默认展示）。每项为 `{documentId, title, cover, views, commentsCount, likesCount, createdAt, updatedAt, publishedAt, firstPublishedAt, isRead, author}`。来源 `src/api/author/controllers/author.ts:486-617`。
- `PROFILE-020` 主页评论列表：`limit` 默认 20、最多 50，按 createdAt 倒序；条件为 `PUBLIC_COMMENT_FILTER` 并且非匿名；**不输出 parent 信息**（防止泄露匿名父评论作者的身份）；带当前访客的 `liked` 状态。每项为 `{documentId, content, images, createdAt, likesCount, liked, author{documentId, name, avatar, level}, article{documentId, title, cover}}`。来源 `src/api/author/controllers/author.ts:620-827`。
- `PROFILE-021` 存在拉黑关系时的主页（任一方向，访客不是本人）：`bio:[]`、`user.exp:0`、`stats:null`、`zzz:null`、关注数为 0、`isFollowing:false`，**保留头像和名片**，并返回 `isBlockedByMe` / `hasBlockedMe` 两个方向的状态。拉黑判断**优先于**隐藏判断。来源 `src/api/author/controllers/author.ts:241-304`。
- `PROFILE-022` 隐藏主页（`profileHidden` 且不是本人）：`bio:[]`、`exp:0`、`equippedCard:null`、`equippedAvatar:null`、`stats:null`、`isHidden:true`、`profileHidden:true`，**仍然返回**关注数和 `isFollowing`。来源 `src/api/author/controllers/author.ts:306-334`。

### 用户搜索
- `PROFILE-023` @ 提及搜索（必须登录）：`q` 先 trim，长度 1 到 32（空时返回 400「Missing q」，超长返回 400「q too long」）；`limit` 默认 8、最多 10；按用户每秒最多 10 次（Redis 故障时放行），超限返回 429「Search rate limit exceeded」。按 `author.name` 或 `user.username` 做不区分大小写的模糊匹配，name 升序，先取 limit×4 条，再排除没有 user、`blocked`、`profileHidden`、存在拉黑关系的用户。每项为 `{documentId, name, username, level, avatar}`。来源 `src/api/author/controllers/author.ts:28-33`、`:836-965`、`routes/author-search.ts`。
- `PROFILE-024` 用户搜索页（公开）：q 为空、超过 32 字或 `start > 200` 时直接返回空页，不报 400；`limit` 默认且最多 20；限流为登录用户每秒 10 次、匿名用户按客户端 IP 每秒 3 次。候选池最多 220 条，过滤规则同上。排序：完全匹配排第 0 档，前缀匹配排第 1 档，其余排第 2 档，同档内按 name 排序。每项为 `{documentId, name, username, level, avatar, bio(纯文本，最多 120 字), followersCount, isFollowing(未登录时恒为 false)}`。来源 `src/api/author/controllers/author.ts:35-70`、`:975-1176`、`routes/author-search-lite.ts`。

### 个人助手、注销后的隐私清理
- `PROFILE-025` `/me/ai-assistant` 只是占位接口：要求登录，固定返回 `{data:{enabled:false, message:'个人助手功能即将上线'}}`。来源 `src/api/me-profile/controllers/me-profile.ts:429-439`。
- `PROFILE-026` 注销账号后的隐私清理：对 `accountStatus='deleted' 且 accountCleanupPending=true` 的用户，在搜索索引中匿名化其作者信息，同时清空 `/api/(articles|comments|authors)` 的响应缓存；**两者都成功后**才把 `accountCleanupPending` 设为 false。cron 每分钟运行一次，每批最多 20 个用户，单个用户失败不影响其他用户。搜索服务不可用时，仍然要清缓存，并保留重试标记。来源 `src/utils/account-privacy.ts:3-24`、`config/cron-tasks.ts:555-558`、`tests/unit/account-privacy.test.ts`。

### 相关策略
- `PROFILE-027` 文章所有权策略 `is-owner`：先查 published 版本，查不到再查 draft 版本；都查不到时返回 **404**「草稿不存在或已被删除」，而不是 403。已发布的匿名文章不能被编辑或删除（返回 403）；未发布的匿名草稿可以编辑；不能通过 `?status=draft` 绕过已发布版本的所有权检查。来源 `src/policies/is-owner.ts:8-45`、`tests/unit/is-owner-policy.test.ts`。
- `PROFILE-028` 其他所有权策略：`is-author-self` 要求 body 中的 `data.author` 对应作者的 user 是当前用户；`is-comment-author` 要求评论作者是当前用户；`is-user-self` 用于阅读记录（article-read），要求属于当前用户。来源 `src/policies/is-author-self.ts`、`is-comment-author.ts`、`is-user-self.ts`。

---

## 12. 测试中体现的关键用例

- `tests/unit/interaction-dedupe.test.ts`：点赞、收藏、投币的 dedupeKey 基于 documentId；撞上唯一约束时视为已存在，不返回 500；迁移只清理真重复，`document_id IS NULL` 的行不算重复。
- `tests/unit/interaction-count-increment.test.ts`：点赞计数 +1/-1 用原子 SQL，下限 0，NULL 按 0 处理，按 document 更新所有版本行；toggle 读回 published 行的计数（与行顺序无关），没有 published 行时退回 draft 行，目标不存在时返回 0。
- `tests/unit/community-dedupe-database.test.ts`：在真实的 SQLite schema 上，并发写入同一互动只允许一行；评论计数的并发增量不会丢失；外层事务回滚时计数也一起回滚。
- `tests/unit/triple-parallel.test.ts`：点赞和收藏并行，投币在两者之后执行；返回字段名不变；撞上唯一约束算成功；一方真实报错时整体报错且不投币；投币失败只体现在 coinReason 中；无法识别的错误码返回 FAILED。
- `tests/unit/denny-notification-after-commit.test.ts`：投币通知在事务提交后才创建，Article 用数字 id；通知失败不回滚；财务日志写入失败时整笔投币失败且不发通知；唯一约束导致回滚时不发通知；作者没有 user 时不发通知。
- `tests/unit/level.test.ts`：等级阈值的边界值和 7 级封顶；一次投币给作者 1 点经验；被动桶上限为 1000。
- `tests/unit/follow-fanout-queue.test.ts`：入队只入队、不查库；有去重；Redis 不可用时不抛错；LPUSH 失败要清除去重键；所有收件人过滤规则；匿名文章或已删除文章的任务直接作废；按 (文章, 关注者) 做幂等；持锁互斥，只释放自己的锁；孤儿任务能被捞回；下线后再发布能重新通知但不会重复通知；条数和时间上限下按游标续传；中途有人取关时不遗漏、也不多发；失败重试，3 次后进死信；落库失败时释放幂等键。
- `tests/unit/article-block-cap.test.ts`：拉黑排除超过 200 个作者时，列表多取 3 倍再在应用层过滤，total 不变，匿名文章不受影响，被过滤满时返回不满的一页；没有拉黑关系时完全不加排除条件。
- `tests/unit/me-profile-rename-transaction.test.ts`：改名扣 10 丁尼并同时更新 username 和 author.name，写一条 `spend_rename` 日志；任何失败都整体回滚；三种数据库的唯一冲突都返回 400；余额不足（包括 0）返回 400；长度 3 到 20；同名直接成功；没有作者档案返回 404。
- `tests/unit/adversarial-rename-transaction.test.ts`：只写一条扣费日志；同一用户 15 丁尼并发改名只有一次成功；两人抢同一个名字时失败方退还丁尼；余额 10 可以改名、9 不行；区分唯一冲突和外键或语法错误。
- `tests/unit/self-profile.test.ts`：登录和 `/me/profile` 返回同一个白名单投影（头像、余额一致）；新注册用户补加载一次关联；刷新 token 时不加载资料。
- `tests/unit/me-business-card.test.ts`：名片分页默认 1/30，最多 60；类型过滤；排序带 id 作为最后一级；非法参数回退默认值；非法类型返回 400 且不查库。
- `tests/unit/profile-pinned-articles.test.ts`：精选候选可分页、可搜索标题；pinnedItems 按保存的顺序返回；保存时剔除失效项；最多 6 篇；主页精选模式返回全部文章数；`scope=all` 时普通分页，并返回 pinnedIds（可以为 null）。
- `tests/unit/owned-interaction-input.test.ts`：收藏和阅读记录忽略伪造的 owner，只写一个规范化的文章引用；update 只能修改自己的记录，不能转移给别人；支持数字 id 和单元素的 set/connect 写法。
- `tests/unit/is-owner-policy.test.ts`：文档不存在返回 404（中文文案）；未登录、没有 id、已发布的匿名文章、别人的文章返回 403；本人草稿和未发布的匿名草稿放行；PolicyError 返回 403 且不记 error 日志。
- `tests/unit/account-privacy.test.ts`：分批匿名化搜索数据；缓存和搜索都成功后才清除重试标记；搜索不可用时仍清缓存；可选的索引不存在时跳过；单个用户失败时继续处理同批其他用户。

---

## 13. 未决问题

1. `level_up` 和 `admin_grant` 只存在于丁尼日志枚举中，没有任何写入方。升级是否要发丁尼？发多少？管理员发币是否需要接口？待确认。
2. 签到日的 04:00 用的是服务器本地时区，经验奖励日固定用 CST 04:00。生产服务器的时区是什么？重写时是否统一为 CST？待确认。
3. 作者收到投币后，余额的封顶是 99999999，与全局的 9999 封顶（schema 也是 `max 9999`）不一致。重写时是否统一为 9999？超出部分是丢弃还是允许超额？待确认。
4. `/user-denny/give` 只要求文章已发布，**不检查 `isHidden`**；一键三连则要求文章可见。被隐藏的文章是否允许投币？待确认。
5. 上传自定义头像不校验 fileId 的归属，也不限制文件类型和大小。重写时是否改为「只能使用自己刚上传的图片」？类型和大小的限制是什么？待确认。
6. 改名没有字符集限制、没有冷却时间，每次都收费，唯一性的大小写敏感程度取决于数据库排序规则。重写时是否维持？待确认。
7. 用户 schema 中 `level` 默认为 1，但未通过考试的用户在第一次获得经验后会变成 0。新注册用户在重写后的初始等级应该是 0 还是 1？待确认。
8. `like_received`、`comment_received`、`favorite_received` 属于被动桶，但没有写入方。被点赞、被评论、被收藏是否应该给作者加经验？待确认（目前只有被投币给经验）。
9. 取消点赞会删除对应的点赞通知（包括已读的）；取消收藏则不删除收藏通知。重写时是否统一？待确认。
10. 每日经验状态接口的 `todaySelfGained` 不包含 `comment_accepted`，和真实的主动桶额度不一致。是否修正？待确认。
11. 签到事务中遇到账号被封禁时返回 500 `CHECK_IN_FAILED`，而不是 403。重写时是否改为 403 `ACCOUNT_BLOCKED`？待确认。
12. 拉黑的 check 接口只返回「我拉黑了 TA」，不返回「TA 拉黑了我」；而主页会返回两个方向。重写时 check 是否也返回两个方向？待确认。
13. @ 提及在解析正文时不做拉黑拦截，只依赖通知层的拦截和候选搜索的排除。被拉黑的人是否仍然可以被 @ 到并显示在正文中？待确认。
14. 我的点赞列表的 total 可能偏大（`LIKE-017`），拉黑排除超过 200 个作者时文章列表的 total 也会偏大。重写时是否要求 total 精确？待确认。
15. 主页的 `totalComments` 和 `totalLikes` 统计包含匿名评论和隐藏评论。这是否符合产品意图？待确认。
16. 关注列表的头像只取 author.avatar，不看装备的系统头像，和其他列表不一致。是否统一？待确认。
