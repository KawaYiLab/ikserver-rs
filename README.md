# ikserver-rs

绳网（InterKnot）论坛的后端：一个用 Rust 写的模块化单体。一个可执行文件提供全部接口、WebSocket 实时通道、后台任务和管理后台，
必须的外部服务只有 PostgreSQL。

配套的论坛前台是 [InterKnot-Web](https://github.com/KawaYiLab/InterKnot-Web)，安卓客户端是
[InterKnot-Android](https://github.com/KawaYiLab/InterKnot-Android)。线上实例：<https://interk.net>。

## 功能

- **帖子与评论**：分区、标签、草稿、置顶、采纳、楼中楼、@提及、点赞收藏、Markdown
- **信息流与搜索**：首页、热榜、关注流、未看流；全文搜索（Meilisearch，可不配，自动退回数据库搜索）
- **账号**：邮箱验证码注册登录、会话管理、登录设备、注销；可选的米游社扫码登录
- **社区**：关注、拉黑、私信、通知、在线状态，全部经 WebSocket 实时推送
- **成长体系**：经验与等级、货币（丁尼）、签到、头像与名片装扮、表情包、入站考试（可关）
- **审核**：举报、多人举报自动隐藏、封禁、屏蔽词、IP 封禁、注册限制、只读维护模式、审计日志
- **AI 助手**：接任意 OpenAI 兼容模型，角色卡、额度、私聊和评论区 @ 回复、长期记忆、可调用站内工具和外部 MCP 服务
- **管理后台**：嵌在可执行文件里，`/admin` 直接打开，站点参数可在线修改
- **可选集成**：S3 兼容对象存储（图片直传）、QQ 官方机器人、游戏剧情库 MCP 服务

## 技术栈

Rust 2024 · Axum · SQLx + PostgreSQL · graphile_worker（任务队列）· Meilisearch · utoipa（OpenAPI）·
管理后台用 Nuxt 4 + shadcn-vue。没有 Redis，缓存与限流都在进程内。

## 用 Docker 试一下

```bash
git clone https://github.com/KawaYiLab/ikserver-rs.git
cd ikserver-rs/deploy/docker
./init.sh                     # 生成 .env 和随机密钥
# 本机试用：.env 里 IK_ENV 改成 dev、IK_SESSION__COOKIE_SECURE 改成 false
docker compose up -d
docker compose logs ik-server | grep setup_token
```

打开 <http://127.0.0.1:8080/admin/>，用日志里的安装码创建管理员。正式上线要用 `IK_ENV=prod` 并填好站点地址和发信配置，
见 [`deploy/docker/README.md`](deploy/docker/README.md)。

## 本地开发

需要 Rust 1.99+ 和 PostgreSQL 17/18（下面用 Docker 起一个）。

```bash
cp .env.example .env
docker run -d --name ik-pg -e POSTGRES_USER=ik -e POSTGRES_PASSWORD=ik -e POSTGRES_DB=ik -p 5432:5432 postgres:18

cargo run -p ik-server -- migrate     # 建表
cargo run -p ik-server -- serve       # http://127.0.0.1:8080
```

- 接口文档：<http://127.0.0.1:8080/docs>
- 开发环境不真的发信，注册验证码会打印在日志里。
- 第一次启动时日志里有安装码，打开 `/admin/setup` 创建管理员；或者 `cargo run -p ik-server -- setup --email 你的邮箱 --username 站长`
- 管理后台：需要 Node 22 和 pnpm 10，见 [`admin/README.md`](admin/README.md)。

更多开发说明（测试、改 SQL 后更新 `.sqlx` 缓存、运维命令）见 [`docs/dev-setup.md`](docs/dev-setup.md)。

## 部署

- Docker：[`deploy/docker/README.md`](deploy/docker/README.md)，镜像在 `ghcr.io/kawayilab/ikserver-rs`。
- 直接跑可执行文件：[`deploy/README.md`](deploy/README.md)，一台服务器、一个反向代理、systemd 托管，发布脚本负责编译、迁移和重启；
  每个版本的可执行文件附在 [GitHub Releases](https://github.com/KawaYiLab/ikserver-rs/releases) 上。

配置全部走 `IK_` 前缀的环境变量，模板是 [`deploy/ik-server.env.example`](deploy/ik-server.env.example)。

## 文档

| 文档 | 内容 |
| --- | --- |
| [`docs/architecture.md`](docs/architecture.md) | 模块划分与依赖方向 |
| [`docs/api-guidelines.md`](docs/api-guidelines.md) | 接口约定与错误码 |
| [`docs/schema.md`](docs/schema.md) | 表结构说明 |
| [`docs/business-rules/`](docs/business-rules/) | 各模块的业务规则 |
| [`docs/adr/`](docs/adr/) | 架构决策记录 |
| [`docs/glossary.md`](docs/glossary.md) | 术语表 |

## 许可证

[GNU Affero General Public License v3.0](LICENSE)。修改后对外提供服务时，需要按许可证公开你的修改。
