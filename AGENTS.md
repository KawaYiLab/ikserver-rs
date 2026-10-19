# AGENTS.md — ikserver-rs

InterKnot 论坛的新一代后端：Rust 模块化单体，替代旧 Strapi 5 后端（`InterKnot-web/server`）。
功能对齐旧系统，但接口、表结构、实现全部重新设计。

## 技术栈（不可擅自更换）

- Rust stable（版本由 CI 的 `RUST_VERSION` 固定，本地用 `rustc --version` 对照；Windows 本机用 gnu 工具链 + MinGW-w64，
  见 `docs/dev-setup.md`），edition 2024。不放 `rust-toolchain.toml`：它会让 Windows 上的 rustup 按默认 host 去装 msvc 工具链
- Axum 0.8 + tower-http 0.7（含 `csrf` 中间件）
- PostgreSQL 17/18 + SQLx 0.9（`query!` / `query_as!` 编译期检查宏）
- 迁移：`sqlx migrate`，纯 SQL 文件，目录 `migrations/`
- 后台任务/定时任务：graphile_worker（PostgreSQL 队列）
- 缓存/限流：进程内 moka / governor（**不使用 Redis**）
- 搜索：Meilisearch（官方 SDK）
- 对象存储：aws-sdk-s3（R2/七牛兼容）
- 认证：不透明会话令牌（DB 存哈希）+ Argon2id（旧 bcrypt 哈希兼容校验）
- OpenAPI：utoipa 6 + utoipa-axum（代码即文档，禁止手写 spec）
- 大模型：async-openai（OpenAI 兼容协议）+ 进程内工具调用循环
- 管理后台：`admin/`（Nuxt 4 SPA + shadcn-vue + Tailwind v4，独立子项目；release 构建时嵌进 `ik-server`，见 deploy/README.md）

## 目录结构

```
crates/
  kernel/    配置、Problem 错误、ID、时间、分页游标、鉴权上下文、事件总线、任务入队
  auth/      账号、验证码、注册登录、会话、绑定邮箱、修改密码
  content/   分区、标签、帖子（草稿/发布/编辑/置顶/浏览量/最新信息流/投币/点赞/收藏/三连/相关推荐）、
             B 站视频信息（发帖时向 B 站查标题与封面）
  comments/  评论、楼中楼、置顶、采纳、@提及、评论点赞
  economy/   经验与等级、丁尼钱包与流水、签到、等级权益
  exam/      入站考试：题库、开考、判分、冷却、首次通过奖励
  social/    关注、拉黑（位于 content 之下，供各域做拉黑判断与过滤）
  messaging/ 私信：一对一会话、消息、编辑与撤回、输入中提示（位于 notify 之下）
  notify/    通知（敲敲）：写入、撤回、新帖扇出、列表、按发送者分组、已读；统一收件箱（私信与通知混排）
  realtime/  WebSocket 网关 /v1/realtime：连接表、按用户 / 会话 / 频道分发、心跳、吊销断开、在线快照
  profile/   个人资料与作者主页：签名、隐藏主页、改名、主页精选、用户搜索、在线人数（最上层，依赖各域 service）
  feed/      信息流：首页（置顶 + 热门位 + 最新）、热榜计算与刷新、关注流、未看流、阅读记录
  search/    搜索：帖子与评论全文搜索、联想（Meilisearch + 数据库回查与兜底）、索引同步
  media/     媒体：直传上传（签名、完成校验与发布）、媒体文件、我的图库、CDN 缩略图地址
  cosmetics/ 装扮：系统头像与论坛名片的目录、装备（装备状态存在账号上）；表情包（清单与后台维护）
  story/     绝区零剧情库：zzz-story-extractor 产出（主线、过场/漫画、设定）的语料包上传与版本管理（后台）、
             内存里的字符二元组倒排检索、五个只读工具（目录 / 检索 / 顺读 / 角色 / 指引），以 MCP 挂在
             POST /v1/story/mcp
  mail/      发信通道：国内邮箱走 SMTP、其余走 Brevo，失败改走另一个（server 装配，业务只依赖 kernel 的 Mailer）
  mihoyo/    米游社扫码登录（自动建号）、绑定与解绑、扫码注销核验、上游协议客户端
  account/   账号安全：安全概览、注销账号（编排 auth、mihoyo、profile）
  moderation/ 举报与审核：举报、多人举报自动隐藏、领取与结案、封禁与解封、审核日志
  ai/        AI 助手：模型与角色卡、每日加权额度（预占 / 结算）、OpenAI 兼容上游的流式客户端、
             AI 私聊会话（触发、流式回复、停止 / 重新生成 / 重置记忆，经 messaging 的钩子与 interop）、
             AI 评论回复（@ 触发、按帖子与评论者的记忆、沉浸守卫，经 comments 的钩子与 interop）、
             12 个只读论坛工具与工具调用循环、工作流时间线与多气泡分段
  admin/     管理后台的跨域部分：账号管理、仪表盘、搜索索引运维、审计日志查看（各域的后台接口在各自 crate）
  legacy/    一次性数据迁移：旧 Strapi 库（legacy schema）→ 新表（ADR 0003；与 migrations/ 一样是模块边界的例外）
  server/    二进制入口：组装路由、中间件、任务、CLI
docs/
  architecture.md     架构与模块边界
  api-guidelines.md   接口设计规范（含错误码登记表）
  schema.md           表结构设计说明（随迁移同 PR 更新）
  business-rules.md   业务规则索引 + 待裁决清单（D-xxx）
  glossary.md         术语表（中文产品词 ↔ 代码标识符）
  business-rules/     从旧系统提取的业务规则（改行为前先读对应文件）
  adr/                架构决策记录
migrations/           纯 SQL 迁移（时间戳命名，禁止修改已合并的迁移）
.sqlx/                编译期查询缓存（提交到 git；本地与 CI 默认都按它离线编译）
deploy/               systemd、nginx、发布脚本
```

模块清单只维护在这里；`docs/architecture.md` 讲依赖方向与设计理由，不再重复列出每个 crate 的职责。
新增、合并、改名 crate 时同一个提交里更新这张表。

## 常用命令

```powershell
# 数据库：远程开发库，连接串在 .env 的 DATABASE_URL（不入库）
cargo sqlx migrate run            # 应用迁移

# 日常开发（.cargo/config.toml 默认 SQLX_OFFLINE=true：按 .sqlx 缓存编译，不连数据库）
cargo check --workspace           # 最快反馈
cargo nextest run -p <crate> -E 'test(<测试名>)'   # 只跑相关测试（测试本身仍连 DATABASE_URL 建临时库）
cargo fmt --all -- --check
# 全量测试与 clippy 交给 CI，见下方「本地与 CI 的分工」

# 改了 SQL 之后（连开发库重新生成 .sqlx 缓存，必须提交；新查询在生成之前离线编译不过）
cargo sqlx prepare --workspace -- --all-targets   # CI 用 --check 校验

# 运行
cargo run -p ik-server -- serve
cargo run -p ik-server -- openapi > openapi.json
```

### 本地与 CI 的分工

本机磁盘小、开发库在远程（每个 `#[sqlx::test]` 都要在远程建库并跑全部迁移，单个测试约一分钟），
所以本地只做快速检查，完整的 clippy 与全量测试交给 CI（自带 PostgreSQL 与 Meilisearch，推送后自动跑）。

- 本地：`cargo check`、`cargo fmt`，以及**只跑本次改动相关的测试**：
  `cargo nextest run -p <crate> -E 'test(<测试名>)'`。改 bug 时用它确认新测试在修之前失败、修之后通过。
- 本地编译前设 `CARGO_INCREMENTAL=0`（增量缓存会占几个 GB）；`target/` 太大时 `cargo clean`。
- 不在本地跑 `cargo nextest run --workspace`；推送后看 CI 结果，失败再修。
- 改了 SQL 仍要在本地跑 `cargo sqlx prepare --workspace -- --all-targets` 并提交 `.sqlx/`
  （需要开发库已执行 `cargo run -p ik-server -- migrate` 并加载旧表结构，见 `docs/dev-setup.md`）。
- 规则只写能被检查的东西。能自动检查的（lint、测试、CI 步骤）就加检查；做不到也不打算做的，不写进本文件。

## 硬性规则（违反即 PR 不通过）

1. **SQL 只用编译期检查宏**（`query!` / `query_as!` / `query_scalar!`）。
   动态拼接只允许 `QueryBuilder` 绑定参数，严禁把用户输入拼进 SQL 文本。
   动态 SQL 必须用 `sqlx::AssertSqlSafe(...)` 显式标注并写注释说明来源可信。
2. **非测试代码禁止 `unwrap()` / `expect()` / `panic!` / `dbg!` / `todo!()` / `unimplemented!()`**
   （workspace lints 已设为 deny；测试豁免见 `clippy.toml`）。
3. **错误统一走 `ik_kernel::error::Problem`**（RFC 9457）。每个对外错误码必须登记在
   `docs/api-guidelines.md` 的错误码表里，附固定 HTTP 状态与中文 `detail`；错误码写成字符串字面量
   （`crates/server/tests/error_codes.rs` 会检查是否登记）。
   5xx 只回 `{ type, title, status, requestId }`，内部细节只进日志。
4. **迁移只增不改**：已合并进 main 的迁移文件禁止修改；变更一律新迁移。
   破坏性变更走 expand/contract（先加新列双写，后删旧列），保证新旧二进制可并行。
   唯一例外：正式上线（生产库第一次执行迁移）之前，允许把全部迁移合并成一份干净的起点，只做一次。
5. **模块边界**：每个业务域 crate 对外暴露 `service`、`routes`、`dto`、`state`，以及需要与调用方
   同一事务时的 `interop`（接收 `&mut PgConnection`）。跨域调用只走这些入口，禁止跨 crate 读写
   别人的表（编译期靠可见性约束，`repo` 模块 `pub(crate)`）。
6. **事务纪律**：业务写入 + 计数更新 + 任务入队在同一事务；提交后再发实时事件；
   事务内禁止调用外部服务（HTTP/SMTP/S3/LLM）。
   - **先查后写必须防并发**：「检查条件 → 再写入」（余额够不够、名字是否相同、冷却是否已过、
     是否已领过）要么检查与写入在同一事务里并先加锁（行锁 `FOR UPDATE` / `FOR NO KEY UPDATE`，
     或 `pg_advisory_xact_lock`），要么交给唯一约束兜底。事务外查、事务内写一律视为 bug。
   - **一个请求同一时刻只占一个数据库连接**：持有连接或事务时，不要再去连接池要第二个
     （包括调用会自己 `acquire` / `begin` 的函数）。需要时把 `&mut PgConnection` 传下去，或先释放再取。
     否则并发一高，所有请求各拿一个连接等第二个，连接池耗尽后集体超时。
7. **时间**：存储与计算用 `DateTime<Utc>`（timestamptz）；业务上的"一天"（签到、
   经验日、限额）一律用 `ik_kernel::time` 的北京时间 helper，禁止散落手写偏移。
8. **ID**：对外 ID 是字符串形式的 64 位雪花 ID（`ik_kernel::id::Id<T>`）。
   JSON 序列化为 string，比较/排序直接按数值。DTO 里的 `Id<T>` 字段要加
   `#[schema(value_type = String)]`，否则 utoipa 派生宏会要求标记类型实现 schema。
   repo 层查询结果用 `i64`，在 service 层用 `Id::new` 包装。
9. **限流**：所有匿名写接口与登录/验证码接口必须有显式限流声明（governor），
   key 推导用 `ik_kernel::http::client_ip`（按 `TRUSTED_PROXY_HOPS` 从右往左数 XFF）。
10. **隐私红线**：匿名身份（anonymousSeed 派生的假名）与真实身份在任何接口、
    搜索索引、AI 工具输出中不得关联；拉黑过滤不适用于匿名内容（防反推）。
    隐私判断按「是否曾经公开过」而不是「现在的状态」：内容一旦以某个身份公开过（发布、通知、
    被搜索收录），下线、编辑、恢复都不能让它换成另一个身份。

## 新增一个接口的检查清单

1. 在对应域 crate 的 `dto.rs` 定义请求/响应类型，`#[derive(utoipa::ToSchema)]`。
2. `repo.rs` 写 SQL（`query_as!`，`pub(crate)`）。
3. `service.rs` 写业务逻辑与权限检查（`pub`），事务按规则 6。
4. `routes.rs` 用 `utoipa_axum::router::OpenApiRouter` + `routes!` 注册，
   handler 上加 `#[utoipa::path(...)]`，错误响应引 `Problem`。
5. 错误码登记进 `docs/api-guidelines.md`。
6. 测试：service 层 `#[sqlx::test]`（真实 PG，独立临时库）；HTTP 层的鉴权、限流、状态码放在
   `crates/server/tests/`。
7. `cargo sqlx prepare --workspace -- --all-targets` 更新 `.sqlx/` 并同提交。
8. 接口有变化时重新生成 `admin/openapi.json`（`cargo run -p ik-server -- openapi > admin/openapi.json`，
   再在 `admin/` 下 `pnpm gen:api`）；CI 会比对两者。`/v1` 既有接口只做向后兼容的变更
   （见 `docs/api-guidelines.md`「版本与兼容性」），目前没有自动的破坏性变更检查，自己逐条对照。

## 新增一个迁移的检查清单

1. `cargo sqlx migrate add <snake_case_name>`。
2. 索引：读多写少，优先复合索引；外键列必须有索引。
3. 唯一性、非空、CHECK 约束尽量下沉到 DDL，代码只做友好错误映射。
4. `cargo sqlx migrate run` 通过；相关测试通过；`cargo sqlx prepare --workspace -- --all-targets`。
5. 在 `docs/schema.md` 写明新表 / 新索引的用途。

## 新增一个后台任务 / 实时事件

- 任务：在所属 crate 的 `jobs.rs` 定义 `TaskHandler`，payload 用 serde 结构体；入队必须在业务事务内
  （`graphile_worker::WorkerUtilsWithExecutor`）。失败策略（重试次数/退避/死信）写在任务定义旁。
- 实时事件：由业务域自己构造（`ik_kernel::bus::Event`），在 `docs/api-guidelines.md` 实时章节登记
  事件名、受众与载荷；放进 `Effects`，事务提交后发布；发布方不假设订阅方在线。
  网关（realtime）只按受众转发，不依赖任何业务域。

## 测试规则

- 纯函数（等级、热度公式、游标编解码、Markdown 派生）用单元测试 + proptest。
- 任何带 SQL 的逻辑用 `#[sqlx::test]`（自动建独立库，并行安全），不许 mock 数据库。
- 并发正确性必须有并发测试：凡是扣钱 / 发钱（丁尼、经验、奖励）、对外发送（邮件、短信、通知）、
  一次性的领取 / 认领（签到、举报认领、首次奖励）、会话轮换，都要测同一请求并发提交两次以上的结果。
- 改 bug 必须先写失败测试再修：本地只跑这一个测试，确认修之前失败、修之后通过。

## 提交规范

- Conventional Commits：`feat(auth): ...` / `fix(content): ...` / `refactor(kernel): ...`。
- 中文或英文 subject 均可，正文写清"为什么"。
- 一个提交只做一件事；同一类的小修复可以放进一个 PR，但每项单独一个提交。
- 迁移、`.sqlx/`、受影响的文档（`docs/schema.md`、`docs/api-guidelines.md`、本文件）与代码同提交。

## 参考文档

- 读代码前先看 `docs/glossary.md`（产品术语与代码标识符对照）。
- 业务行为以 `docs/business-rules/*.md` 为准（从旧系统提取，含来源行号）。
- 历史决策看 `docs/adr/`。
