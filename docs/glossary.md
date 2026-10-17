# 术语表

产品用《绝区零》"绳网"世界观词汇；代码用英文标识符。本表是两者唯一权威对照，
命名前先查表，新术语先加行再使用。

## 领域术语

| 中文（产品/UI） | 代码标识符 | 说明 |
|---|---|---|
| 绳网 | InterKnot | 站点名 |
| 委托 / 帖子 | post | 旧代码叫 article；新系统统一 `posts` 表 / `Post` 类型 |
| 回复 / 评论 | comment | |
| 楼层回复 | reply | comment 的 parent 指向的二级结构（只允许一层） |
| 丁尼 | denny | 虚拟货币，钱包余额 `denny_balance` |
| 经验 / 等级 | exp / level | |
| 签到 | check_in | |
| 入站考试 | exam | 通过（`exam_passed`）后才能发帖等写操作 |
| 敲敲 | knock | 通知的产品名；代码域为 `notify`，收件箱 `inbox` |
| 私信 | dm / conversation / message | 会话（direct/group/ai_session）与消息 |
| 三连 | triple | 点赞+收藏+投币一键完成 |
| 投币 | denny give | 给帖子投 1 丁尼 |
| 匿名 | anonymous | 匿名身份由 `anonymous_seed` 派生假名 |
| 顶帖 | bump | `bumped_at` 驱动默认信息流排序 |
| 热榜 | hot | Discourse 热度公式 |
| 未看 | unread | 基于阅读水位 |
| 名片 | business_card | 装扮 |
| 头像（装扮） | avatar | 商城头像，区别于用户上传头像 |
| 表情 | emote | 自定义表情组 |
| 相关推荐 | related posts | 帖子详情页底部，按相关度排序 |
| 拉黑 | user_block | 双向生效 |
| 举报 | report | 目标 article/comment/user |
| 处罚 | sanction | 禁言（mute）/封禁（ban），带到期时间 |
| 审计日志 | audit_log | 管理操作留痕 |
| AI 助手 / 角色卡 | ai / role_card | 人格、模型、额度 |
| 米游社 | mihoyo | 扫码登录/绑定 |
| 权益 | benefit | 等级权益 |
| 上传回执 | upload receipt | 直传完成凭证 |

## 通用代码词汇

| 标识符 | 含义 |
|---|---|
| `Id<T>` | 雪花 ID 新类型（kernel），JSON 序列化为字符串 |
| `legacy_id` | 迁移自旧 Strapi 的 documentId，供旧链接跳转 |
| `Problem` | RFC 9457 错误类型 |
| `AuthCtx` | 已认证请求上下文（用户、会话、角色、权限位） |
| `Cursor` | 不透明分页游标 |
| `EventBus` | 进程内事件总线 trait（realtime 推送源） |
| `Effect` | 事务提交后要执行的副作用（推送/索引/通知） |
| 业务日 | 北京时间 04:00 为界的"一天"（`kernel::time::biz_day`） |

## 旧系统对照（阅读 business-rules 时用）

| 旧（Strapi） | 新（Rust） |
|---|---|
| `articles` / `documentId` | `posts` / 雪花 ID（`legacy_id` 保留跳转） |
| `*_lnk` 关系表 | 外键列（如 `posts.author_id`） |
| draftAndPublish 双行 | `posts` 单行 + `post_drafts` |
| lifecycles 副作用 | service 显式调用 + 事务内任务入队 |
| Redis 队列/cron | graphile_worker |
| users-permissions JWT | 不透明会话令牌 |
| Strapi admin + 插件 | `/v1/admin/*` + Nuxt shadcn-vue 后台 |
| agent-worker + AstrBot | `ai` 域直连大模型（OpenAI 兼容） |
