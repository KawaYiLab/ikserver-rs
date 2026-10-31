# 更新记录

版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。1.0 之前，次版本号变化也可能带来不兼容的改动，会在这里写明。
发布时把「未发布」改成版本号和日期，再推同名的 `v*` 标签（Release 工作流用这一段当发布说明）。

## [未发布]

### 新增

- 首次安装：还没有管理员时，打开 `/admin/setup` 创建第一个管理员；也可以用 `ik-server setup`。
- Docker 镜像与 `deploy/docker/` 下的 compose 部署，`init.sh` 自动生成数据库密码和密钥。
- `IK_DATABASE__AUTO_MIGRATE`：启动时自动迁移数据库（镜像里默认打开）。
- `IK_SITE_URL`：论坛前台地址，管理后台和 QQ 机器人的帖子链接按它生成，`GET /v1/site` 返回为 `siteUrl`。
- 打 `v*` 标签时发布 GitHub Release（附可执行文件）和 `ghcr.io` 镜像。
- AI 角色列表（`GET /v1/ai/characters`）新增 `popularQuestions`：最近 30 天至少 3 人用来开场的问题，敲敲「大家都在问」据此展示。
- `GET /v1/sitemap.xml`：站点地图，列出公开帖子、公开主页和至少有 2 篇公开帖子的标签页，地址按 `IK_SITE_URL` 生成（未配置时 404）。

### 变更

- 许可证：AGPL-3.0。
- 米游社扫码登录默认关闭（`IK_MIHOYO__ENABLED=true` 打开）。
- 生产环境拒绝使用 `.env.example` 里的开发密钥。
- 错误响应的 `type` 改为 `urn:ik:problem:<code>`。
- 管理后台不再在构建时写入主站地址（去掉 `NUXT_PUBLIC_SITE_URL`）。
