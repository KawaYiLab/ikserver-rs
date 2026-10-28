# 用 Docker 部署

一台装了 Docker（含 compose 插件）的服务器就够了。这里起的是 ik-server 和 PostgreSQL，
Meilisearch 可选；论坛前台 [InterKnot-Web](https://github.com/KawaYiLab/InterKnot-Web) 是静态文件，
和反向代理放在一起（见 [`../README.md`](../README.md)「推荐拓扑」）。

## 第一次启动

```bash
git clone https://github.com/KawaYiLab/ikserver-rs.git
cd ikserver-rs/deploy/docker
./init.sh            # 生成 .env：数据库密码、Meilisearch 主密钥、两把服务密钥都自动填好
$EDITOR .env         # 填站点地址和发信配置
docker compose up -d
```

然后打开 `http://127.0.0.1:8080/admin/`，填管理员的邮箱、用户名、密码，
创建完会自动登录并进入「系统设置」。

只想在本机试一下：用 `./init.sh try` 代替 `./init.sh`，不用改 `.env`，也不用配发信，
注册验证码会打印在日志里（`docker compose logs -f ik-server`）。

不想开网页时，用命令行创建管理员：

```bash
docker compose exec -e IK_ADMIN_PASSWORD='你的密码' ik-server ik-server setup --email you@example.com --username 站长
```

## 镜像

`compose.yml` 默认用 `ghcr.io/kawayilab/ikserver-rs:latest`。想用自己改过的代码，在这个目录运行
`docker compose build`（会从仓库根目录的 `Dockerfile` 构建），或者设 `IK_IMAGE` 指向你自己的镜像。

镜像里的 `ik-server` 启动时会自动迁移数据库（`IK_DATABASE__AUTO_MIGRATE=true`），管理后台已经编在里面。

## 搜索

```bash
docker compose --profile search up -d
```

再把 `.env` 里 `IK_SEARCH__URL`、`IK_SEARCH__API_KEY` 两行的注释去掉（`init.sh` 已经填好了密钥），
`docker compose up -d ik-server` 重启，最后建一次索引：

```bash
docker compose exec ik-server ik-server search reindex
```

## 升级

```bash
docker compose pull
docker compose up -d
```

迁移只增不改，升级后数据库迁移自动完成。回滚时把 `IK_IMAGE` 设成旧版本的标签再 `up -d`。

## 备份

数据都在 PostgreSQL 里（图片在对象存储）：

```bash
docker compose exec postgres pg_dump -U interknot interknot | gzip > interknot-$(date +%F).sql.gz
```

## 其他运维命令

```bash
docker compose exec ik-server ik-server --help
docker compose exec ik-server ik-server user set-role 邮箱 moderator
```
