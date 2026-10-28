# ikserver-rs

[绳网（InterKnot）](https://interk.net)论坛的后端服务，一个用 Rust 编写的模块化单体（modular monolith）。
单个可执行文件提供 REST API、WebSocket 实时通道、后台任务队列和内嵌的管理后台，唯一必需的外部依赖是 PostgreSQL。

> 本仓库只包含后端与管理后台。论坛前端（SPA）见 [InterKnot-Web](https://github.com/KawaYiLab/InterKnot-Web)，
> 安卓客户端见 [InterKnot-Android](https://github.com/KawaYiLab/InterKnot-Android)。

## 功能

- **内容**：分区、标签、草稿、置顶、采纳、楼中楼、@提及、点赞收藏、Markdown
- **信息流与搜索**：首页、热榜、关注流、未看流；全文检索（Meilisearch，未配置时回退到 PostgreSQL）
- **账号**：邮箱验证码注册登录、会话与设备管理、账号注销；可选米游社扫码登录（OAuth）
- **社交**：关注、拉黑、私信、通知、在线状态，经 WebSocket 实时推送
- **成长体系**：经验与等级、虚拟货币、签到、头像框与名片装扮、表情包、入站考试（可关闭）
- **审核**：举报与阈值自动隐藏、封禁、敏感词过滤、IP 封禁、注册限制、只读维护模式、审计日志
- **AI 助手**：接入任意 OpenAI 兼容 API，支持角色卡、配额、私聊与评论区 @ 回复、长期记忆、站内工具调用和外部 MCP 服务
- **管理后台**：编译进可执行文件，挂载在 `/admin`，站点参数支持运行时修改
- **可选集成**：S3 兼容对象存储（预签名直传）、QQ 官方机器人、游戏剧情库 MCP 服务

## 技术栈

| 层 | 选型 |
| --- | --- |
| Web 框架 | Axum 0.8（Tokio） |
| 数据库 | PostgreSQL 17/18，SQLx（编译期校验查询，离线缓存在 `.sqlx/`） |
| 任务队列 | graphile_worker（基于 PostgreSQL） |
| 搜索 | Meilisearch（可选） |
| API 文档 | utoipa 生成 OpenAPI 3.1，Scalar 渲染于 `/docs` |
| 管理后台 | Nuxt 4（SPA）+ shadcn-vue + Tailwind CSS 4，rust-embed 嵌入 |

不依赖 Redis：缓存与限流均在进程内实现。

## 快速开始

使用 Docker Compose 在本地启动（ik-server + PostgreSQL）：

```bash
git clone https://github.com/KawaYiLab/ikserver-rs.git
cd ikserver-rs/deploy/docker
./init.sh try         # 生成 .env（开发模式、随机密钥、邮件输出到日志）
docker compose up -d
```

容器启动时自动执行数据库迁移。访问 <http://127.0.0.1:8080/admin/> 创建首个管理员账号。
开发模式下邮件验证码输出到日志：`docker compose logs -f ik-server`。

生产部署使用 `./init.sh`（不带参数），配置 `IK_SITE_URL` 与 SMTP，并在前面部署反向代理终止 TLS，
详见 [`deploy/docker/README.md`](deploy/docker/README.md)。

## 部署

| 方式 | 说明 |
| --- | --- |
| [Docker Compose](deploy/docker/README.md) | 镜像 `ghcr.io/kawayilab/ikserver-rs`，含 PostgreSQL，可选 Meilisearch |
| [二进制 + systemd](deploy/README.md) | [GitHub Releases](https://github.com/KawaYiLab/ikserver-rs/releases) 提供预编译 Linux 可执行文件 |

依赖：

- **必需**：PostgreSQL 17+
- **可选**：Meilisearch、SMTP、S3 兼容对象存储、OpenAI 兼容 LLM API

配置全部通过 `IK_` 前缀的环境变量注入（嵌套字段用 `__` 分隔），模板见
[`deploy/ik-server.env.example`](deploy/ik-server.env.example)。注册策略、公告、分区等运行时参数在管理后台修改。

## 开发

环境要求：Rust 1.99+、Docker（用于 PostgreSQL）；管理后台另需 Node 22 与 pnpm 10。

```bash
cp .env.example .env
docker run -d --name ik-pg -e POSTGRES_USER=ik -e POSTGRES_PASSWORD=ik -e POSTGRES_DB=ik -p 5432:5432 postgres:18
cargo run -p ik-server -- migrate
cargo run -p ik-server -- serve       # http://127.0.0.1:8080，OpenAPI 文档在 /docs
```

测试使用 `cargo nextest`，集成测试通过 `#[sqlx::test]` 连接真实数据库。修改 SQL 后需执行
`cargo sqlx prepare --workspace -- --all-targets` 更新离线缓存。

| 文档 | 内容 |
| --- | --- |
| [`docs/dev-setup.md`](docs/dev-setup.md) | 开发环境、测试、运维命令 |
| [`docs/architecture.md`](docs/architecture.md) | 模块划分与依赖方向 |
| [`docs/api-guidelines.md`](docs/api-guidelines.md) | API 约定与错误码 |
| [`docs/schema.md`](docs/schema.md) | 数据库表结构 |
| [`docs/business-rules/`](docs/business-rules/) | 各模块业务规则 |
| [`docs/adr/`](docs/adr/) | 架构决策记录（ADR） |
| [`admin/README.md`](admin/README.md) | 管理后台开发 |

## 许可证

[GNU Affero General Public License v3.0](LICENSE)。修改后以网络服务形式对外提供时，须按 AGPL 向用户公开对应源代码。
