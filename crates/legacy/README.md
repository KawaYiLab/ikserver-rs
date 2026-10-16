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

# 3. 预检（只读）。[Blocking] 必须先在旧库处理；[Fixed] 会自动修正并计数；[Info] 用来判断时区
ik-server legacy check

# 4. 导入（一个事务）。时区填旧服务进程所在的时区：在旧机器上看 `date`，或对照预检里的最新时间
ik-server legacy import --legacy-timezone Asia/Shanghai

# 5. 重建搜索索引
ik-server search reindex

# 6. 验收通过后删掉旧数据
psql -d "$DATABASE_URL" -c 'DROP SCHEMA legacy CASCADE'
```

导入失败时整个事务回滚，新库保持为空，修正后直接重跑第 4 步。

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
