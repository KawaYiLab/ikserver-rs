# ik-legacy：从旧 Strapi 库迁移数据

一次性切换用的导入工具。设计与取舍见 [ADR 0003](../../docs/adr/0003-legacy-data-import.md)。

## 切换步骤

先在一份生产数据的副本上完整演练一遍，记下耗时和预检结果，再正式切换。

```bash
# 0. 新库：建好全部表（空库）
ik-server migrate

# 1. 旧站进入维护模式（停止写入），导出旧库
pg_dump -Fc -d "$OLD_DATABASE_URL" -f interknot.dump

# 2. 在临时库里把 public 改名为 legacy，再整体导入新库的 legacy schema
createdb ik_legacy_tmp
pg_restore --no-owner --no-privileges -d ik_legacy_tmp interknot.dump
psql -d ik_legacy_tmp -c 'ALTER SCHEMA public RENAME TO legacy' -c 'CREATE SCHEMA public'
pg_dump --no-owner --no-privileges -n legacy -d ik_legacy_tmp | psql -v ON_ERROR_STOP=1 -d "$DATABASE_URL"
dropdb ik_legacy_tmp

# 3. 读取旧文件的哈希与真实类型（只读对象存储，可续跑；可以在停机前提前跑，停机后再跑一次补新文件）
#    新系统的 storage 配置必须指向旧系统同一个桶与 CDN，文件不搬家
ik-server legacy media --url-prefix https://im.tiwat.cn/ --url-prefix https://image.tiwat.cn/

# 4. 预检（只读）。[Blocking] 必须先处理；[Fixed] 会自动修正并计数；[Info] 用来判断时区
ik-server legacy check

# 5. 导入（一个事务）。时区填旧服务进程所在的时区：在旧机器上看 `date`，或对照预检里的最新时间
ik-server legacy import --legacy-timezone Asia/Shanghai

# 6. 重建搜索索引
ik-server search reindex

# 7. 验收通过后删掉旧数据
psql -d "$DATABASE_URL" -c 'DROP SCHEMA legacy CASCADE'
```

导入失败时整个事务回滚，新库保持为空，修正后直接重跑第 5 步。

## 映射规则

规则集中在 [`sql/helpers.sql`](sql/helpers.sql) 的 `legacy.ik_*` 视图里，导入语句在 [`src/steps/`](src/steps/)。

### 账号（users、user_passwords、user_profiles）

| 旧 | 新 | 规则 |
| --- | --- | --- |
| `up_users.id` | `users.id` | 原样保留 |
| `account_status` / `blocked` | `status` | 注销 → `deleted`，合并 → `merged`，`blocked` → `blocked`，其余 `active`（D-019） |
| `email` | `email` | trim + 小写；米游社占位邮箱、注销脱敏邮箱、注销 / 合并账号置空（D-069）；规范化后重复则拒绝导入 |
| `username` | `username` | `@` 换成全角 `＠`、截到 32 字；为空或撞名改为 `用户_<id>`（原名合法的一方保留） |
| `is_admin` | `role` | `admin`，否则 `user`（旧系统没有论坛内的版主，D-062） |
| `exam_passed` / `exam_passed_at` | `exam_passed_at` | 已通过的沿用时间，没有时间的用导入时刻（EXAM-014） |
| 作者 `document_id` | `legacy_id` | 旧主页链接用作者的 documentId（RPTADM-015），没有作者时用账号的 |
| `password` | `user_passwords.hash` | 只迁 bcrypt 哈希，且仅限本地注册、未注销的账号（米游社首登的随机密码不算） |
| 作者 `bio`（blocks 富文本） | `user_profiles.bio` | 取全部文字、段落换行，截到 100 字 |
| `profile_hidden` | `user_profiles.hidden` | 原样 |

### 媒体（media_files、media_uploaders、头像）

| 旧 | 新 | 规则 |
| --- | --- | --- |
| `files.url` | `object_key` | 去掉公开前缀（`--url-prefix`）后的路径；文件原地不动 |
| 文件内容 | `sha256`、`mime`、`thumbhash` | `legacy media` 读取对象算出（嗅探真实类型，只收图片、30 MB 以内）；读取失败的文件不导入，引用它的封面 / 配图 / 头像随之去掉 |
| — | `receipt` | `legacy:<id>`（收据只用于新上传的重试幂等） |
| `files_uploaders_lnk` | `media_uploaders` | 我的图库 |
| 作者 `avatar` | `users.avatar_media_id` | 自定义头像 |

### 内容（categories、tags、posts、post_drafts、post_tags、comments）

| 旧 | 新 | 规则 |
| --- | --- | --- |
| 分区 `slug` / `name` | 同名 | slug 不合规改为 `c-<id>`，名称截到 30 字 |
| 标签 | `tags` | slug 相同的合并到 id 最小的那个，帖子上的标签随之改指 |
| 文章草稿行 + 发布行 | 一条 `posts` | 帖子 id 取该文档最小的行 id；有发布行 → `published`、内容取发布行，否则 `draft` |
| 草稿行（与发布行不同时） | `post_drafts` | 已发布帖子的待发布修改 |
| `title` / `text` | `title` / `body` | 截到 50 / 16000 字；正文里的 `image.tiwat.cn` 改为 `im.tiwat.cn`（CDN-001） |
| `externalVideos` | `external_videos` | 只留第一个 |
| `cover` | `cover_ids` | 只认读取成功的文件，最多 40 张 |
| `isHidden` | `hidden_at` | `true` → 隐藏（时间取更新时间），`NULL` 视为可见（D-012） |
| `bumpedAt` | `bumped_at` | 缺失时用发布时间（POST-061） |
| `anonymousSeed` | 同名 | 匿名帖缺 seed 时由 documentId 派生（稳定） |
| `blocks`（动态区块） | — | 新系统没有这个字段，不导入（预检里报数量） |
| 评论 | `comments` | 楼层按时间编号；回复的回复挂到顶层评论下（CMT-025）；置顶、采纳只留在顶层；截到 2000 字；所在帖子或作者不存在的不导入 |
| `acceptedCommentDocumentId` | `accepted_comment_id` | 只认同一帖子下的顶层评论 |
| 作者 `profilePinnedArticles` | `user_profiles.pinned_post_ids` | documentId 换成帖子 id，只留本人仍存在的帖子，最多 6 个 |
| 作者没有账号的帖子 | — | 不导入（连同评论与互动） |

### 互动与社交

点赞（帖子 / 评论）、收藏、投币、阅读记录、关注、拉黑：关系从 `*_lnk` 链接表取，指向帖子的按文档换成帖子 id；
同一对象重复的记录只留最早的；对象或账号不存在、关注 / 拉黑自己的不导入。投币固定 1 枚、留言截到 100 字；
阅读记录有就算已读（`isRead` 不参与，READ-001）；关注对象从作者换成作者的账号。
帖子的点赞、收藏、投币、评论数和评论的点赞数按导入的行重新计算（评论数含回复与被隐藏的评论）。

## 重新生成旧表结构

旧后端的表结构有变化时（新增内容类型或字段），在空库上启动一次旧后端，让它自己建表：

```bash
createdb legacy_gen
cd ikserver   # 旧后端仓库
DATABASE_CLIENT=postgres DATABASE_URL=postgres://…/legacy_gen npx strapi develop --no-watch-admin --no-build-admin
# 看到 [discard-drafts] Migration completed successfully 后停掉（连不上 Redis / Meilisearch 不影响建表）
psql -d legacy_gen -c 'ALTER SCHEMA public RENAME TO legacy' -c 'CREATE SCHEMA public'
pg_dump -s --section=pre-data --no-owner --no-privileges --no-comments -n legacy -d legacy_gen
```

把输出（去掉 `SET` / `SELECT pg_catalog` / 注释 / `\restrict` 行）替换 `sql/strapi-schema.sql`，
重新加载到开发库，跑 `cargo nextest run -p ik-legacy` 与 `cargo sqlx prepare --workspace -- --all-targets`。
