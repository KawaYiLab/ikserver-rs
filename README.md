# ikserver-rs

绳网（InterKnot）论坛的后端。绳网是一个《绝区零》同人社区，线上地址 <https://interk.net>。

你可以用它搭一个自己的论坛：发帖、评论、私信、通知、审核、管理后台都有，部署只需要一个程序加一个 PostgreSQL 数据库。

> 这个仓库只是后端和管理后台。用户看到的论坛页面在 [InterKnot-Web](https://github.com/KawaYiLab/InterKnot-Web)，
> 安卓客户端在 [InterKnot-Android](https://github.com/KawaYiLab/InterKnot-Android)。

## 有什么

- **发帖和讨论**：分区、标签、草稿、置顶、楼中楼、@ 别人、点赞收藏，支持 Markdown
- **找内容**：首页、热榜、关注的人、全文搜索
- **社交**：关注、拉黑、私信、通知，消息实时推送，不用刷新
- **玩法**：等级、签到、虚拟货币、头像框和名片装扮、表情包、新人入站考试（可关）
- **管理**：举报、封禁、屏蔽词、只读维护、操作记录；网页后台里就能改大部分设置
- **可选**：AI 助手（接任意 OpenAI 兼容模型）、图片存对象存储、QQ 机器人、米游社扫码登录

## 快速开始

装好 Docker 就能在自己电脑上试：

```bash
git clone https://github.com/KawaYiLab/ikserver-rs.git
cd ikserver-rs/deploy/docker
./init.sh try         # 生成配置，密码和密钥都是随机的
docker compose up -d
```

打开 <http://127.0.0.1:8080/admin/>，创建第一个管理员，就装好了。数据库一起装在 Docker 里，表会在启动时自动建好。
试用模式不发邮件，注册验证码看日志：`docker compose logs -f ik-server`。

正式上线用 `./init.sh`（不带 `try`），填好网站地址和发信配置（注册要发验证码邮件），再在前面放一个反向代理，
步骤见 [Docker 部署](deploy/docker/README.md)。

## 部署

| 方式 | 适合 |
| --- | --- |
| [Docker](deploy/docker/README.md) | 大多数人。数据库一起装好，升级就是换镜像 |
| [直接运行程序](deploy/README.md) | 不想用 Docker，用 systemd 管理 |

需要的东西：

- **必须**：PostgreSQL 17 或 18
- **可选**：Meilisearch（搜索更好用，不装就用数据库搜索）、SMTP 发信、S3 兼容存储（上传图片）

所有配置都写在环境变量里，模板见 [`deploy/ik-server.env.example`](deploy/ik-server.env.example)。
装好以后，注册方式、公告、分区这些在管理后台里改。

## 参与开发

需要 Rust 1.99+ 和 Docker（用来跑数据库）。

```bash
cp .env.example .env
docker run -d --name ik-pg -e POSTGRES_USER=ik -e POSTGRES_PASSWORD=ik -e POSTGRES_DB=ik -p 5432:5432 postgres:18
cargo run -p ik-server -- migrate
cargo run -p ik-server -- serve       # http://127.0.0.1:8080，接口文档在 /docs
```

开发时不会真的发邮件，验证码直接打印在日志里。管理后台的开发方法见 [`admin/README.md`](admin/README.md)，
测试和其他细节见 [开发说明](docs/dev-setup.md)。想了解代码怎么组织，先看 [架构说明](docs/architecture.md)。

用到的主要技术：Rust、Axum、SQLx、PostgreSQL；管理后台是 Nuxt 4。

## 许可证

[AGPL-3.0](LICENSE)。可以免费使用和修改；如果你改了代码并拿来对外提供服务，需要把改过的代码也公开出来。
