# 部署

`ik-server` 是一个可执行文件：接口 `/v1`、实时通道 `/v1/realtime`、管理后台 `/admin` 都由它提供，
后台页面在编译时嵌进去，不需要单独发布。必须的外部服务只有 **PostgreSQL**；Meilisearch（搜索）、
S3 兼容存储（图片上传）、SMTP（验证码邮件）、大模型接口按需配置。

论坛前台是另一个仓库 [InterKnot-Web](https://github.com/KawaYiLab/InterKnot-Web)（纯静态 SPA）。

用 Docker 部署见 [`docker/README.md`](docker/README.md)；这一页讲直接在服务器上跑可执行文件（systemd）。
绳网自己的线上环境（EdgeOne 直接回源、主站跨站调用）见 [`interknot/README.md`](interknot/README.md)。

| 文件 | 作用 |
| --- | --- |
| [`release.sh`](release.sh) | 编译后台与 ik-server；带服务器参数时上传、迁移、重启 |
| [`ik-server.env.example`](ik-server.env.example) | 生产配置模板 |
| [`systemd/ik-server.service`](systemd/ik-server.service) | 服务定义（发布时自动安装） |
| [`systemd/ik`](systemd/ik) | 在服务器上跑运维命令的包装（发布时装到 `/usr/local/bin/ik`） |

## 推荐拓扑：一个域名

前台和接口放在同一个域名下，由反向代理分流。这样浏览器只和一个站点通信，Cookie 和 CSRF 都不用特殊处理。

```
浏览器 ──▶ Caddy / nginx（forum.example.com，终止 TLS）
             ├─ /v1/*、/admin/*  ──▶ 127.0.0.1:8080  ik-server
             └─ 其余路径          ──▶ InterKnot-Web 的构建产物（静态文件，回退到 index.html）
```

前台构建时 `NUXT_PUBLIC_API_BASE_URL` 留空，就会请求同源的 `/v1`。

Caddy 示例：

```caddyfile
forum.example.com {
    handle /v1/* {
        reverse_proxy 127.0.0.1:8080
    }
    handle /admin* {
        reverse_proxy 127.0.0.1:8080
    }
    handle {
        root * /srv/interknot-web
        try_files {path} /index.html
        file_server
    }
}
```

对应的配置（模板里已写好）：

```
IK_HTTP__ADDR=127.0.0.1:8080
IK_HTTP__TRUSTED_PROXY_HOPS=1                          # 只有反向代理一层
IK_HTTP__ALLOWED_ORIGINS=["https://forum.example.com"]
IK_SESSION__COOKIE_SECURE=true
IK_SITE_URL=https://forum.example.com
```

要点：

- **代理层数要配对。** `TRUSTED_PROXY_HOPS` 是 ik-server 前面会追加 `X-Forwarded-For` 的代理层数，限流和审计日志里的 IP 按它取。
  前面再加一层 CDN 就是 2。登录后在后台「登录设备」里看到的应该是自己的公网 IP。
- **ik-server 只监听本机**（或用防火墙只放行代理）。谁都能直连的话，`X-Forwarded-For` 可以伪造。
- **WebSocket**：`/v1/realtime` 要能升级连接，空闲超时不小于 120 秒（心跳 30 秒）。Caddy 默认支持；nginx 要加 `Upgrade` / `Connection` 头。
- **Host 保持原样**：WebSocket 握手和后台的同源判断比较 `Origin` 与 `Host`，代理不要改写 `Host`。
- **缓存**：如果前面有 CDN，`/v1/*` 遵循源站的 `Cache-Control`，不要配「全部缓存」，否则会把一个人的数据发给另一个人。
- **请求体大小**：接口正文上限 2 MB；图片直传对象存储，不经过这里。

### 前台和接口不在同一个站点

例如前台在 `forum.example.com`，接口在 `api.example.net`（绳网现在就是这样）：前台构建时
`NUXT_PUBLIC_API_BASE_URL=https://api.example.net`，后端打开 `IK_SESSION__COOKIE_CROSS_SITE=true`，
`ALLOWED_ORIGINS` 写前台的地址。代价是每个写请求多一次预检，并且依赖浏览器接受分区的跨站 Cookie，
细节见 [ADR 0005](../docs/adr/0005-edgeone-direct-origin.md)。

## 服务器一次性准备

```bash
sudo useradd --system --home-dir /opt/interknot --shell /usr/sbin/nologin interknot
sudo install -d -m 0755 /opt/interknot
sudo install -d -m 0750 -g interknot /etc/interknot
sudo install -m 0640 -o root -g interknot deploy/ik-server.env.example /etc/interknot/ik-server.env
sudoedit /etc/interknot/ik-server.env    # 填数据库、密钥、发信……
```

数据库先建好空库和账号，表结构由发布脚本迁移。两把密钥用 `openssl rand -base64 48` 各生成一次；
生产环境不接受 `.env.example` 里的开发密钥。

## 发布

在开发机上（Linux；Windows 在 WSL 里），仓库根目录运行：

```bash
deploy/release.sh root@服务器
```

脚本依次：编译管理后台（需要 Node 22 与 pnpm 10）→ 编译 ik-server 并确认嵌进去的是刚编译的后台 →
上传可执行文件、服务定义与 `ik` 命令 → 用新版本跑迁移（失败就停下，线上旧版本不受影响）→
替换 `/opt/interknot/ik-server`（旧的留作 `ik-server.prev`）、重启、等 `/readyz` 就绪。

不带参数只编译，产物是 `target/release/ik-server`。在 Linux 上编译出来的可执行文件依赖编译机的 glibc 版本，
编译机的发行版不要比服务器新。

不想在本机编译时，在 GitHub 的 Actions 页面运行「Release」工作流（推 `v*` 标签也会触发），
下载产物后发布：

```bash
deploy/release.sh --binary ~/Downloads/ik-server root@服务器
```

## 第一个管理员

第一次启动时站里还没有管理员，打开 `https://forum.example.com/admin/` 会自动进入「创建管理员」页面，
填管理员的邮箱、用户名、密码。
创建完自动登录，接着在「系统设置」里配置注册方式、入站考试、公告等，在「分区与标签」里建分区。

不想开网页时用命令行（运行后按提示输入密码）：

```bash
sudo ik setup --email you@example.com --username 站长
```

有了管理员以后安装页就不再起作用，之后的管理员、版主用 `ik user set-role` 或在后台「账号」里设置。

## 运维命令

在服务器上用 `sudo ik …`，和服务使用同一个用户与配置文件：

```bash
sudo ik migrate                     # 迁移数据库（发布脚本会自动跑）
sudo ik user set-role 邮箱 admin     # 设置角色
sudo ik search reindex              # 推送搜索索引设置并全量重建
journalctl -u ik-server -f          # 日志
```

读文件的命令（如 `ik exam import <路径>`）以 `interknot` 用户运行，文件要让它读得到。
全部命令见 `ik-server --help`。

## 回滚

迁移只增不改，迁移完成后旧版本仍能运行，回滚只需要换回旧的可执行文件：

```bash
sudo mv /opt/interknot/ik-server.prev /opt/interknot/ik-server && sudo systemctl restart ik-server
```

个别升级会写明不能直接回滚（例如 QQ 机器人调度的首次切换，见 [`interknot/README.md`](interknot/README.md)）。
