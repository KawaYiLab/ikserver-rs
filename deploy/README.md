# 部署

线上只有两样东西：

| 部署物 | 内容 | 位置 |
| --- | --- | --- |
| 主站前端 | InterKnot-web（Nuxt） | `interk.net`，`/v1` 由 nginx 转给 ik-server |
| `ik-server` | 一个可执行文件：接口 `/v1`、实时通道 `/v1/realtime`、管理后台 `/admin`、后台任务 | `ik.tiwat.cn` 与本机 `127.0.0.1:8080` |

管理后台（`admin/`）在编译时嵌进 `ik-server`，不需要单独发布。依赖的服务：PostgreSQL、Meilisearch、对象存储。

| 文件 | 作用 |
| --- | --- |
| [`release.sh`](release.sh) | 编译后台与 ik-server，上传、迁移、重启 |
| [`../.github/workflows/release.yml`](../.github/workflows/release.yml) | 在 GitHub Actions 上编译同样的可执行文件 |
| [`systemd/ik-server.service`](systemd/ik-server.service) | 服务定义（发布时自动安装） |
| [`systemd/ik`](systemd/ik) | 在服务器上跑运维命令的包装（发布时装到 `/usr/local/bin/ik`） |
| [`ik-server.env.example`](ik-server.env.example) | 生产配置模板 |
| [`nginx/`](nginx/) | 两个域名的 nginx 配置 |

## 服务器一次性准备

```bash
sudo useradd --system --home-dir /opt/interknot --shell /usr/sbin/nologin interknot
sudo install -d -m 0755 /opt/interknot
sudo install -d -m 0750 -g interknot /etc/interknot
# 从仓库拷过去后填写（数据库、密钥、对象存储……）
sudo install -m 0640 -o root -g interknot ik-server.env.example /etc/interknot/ik-server.env
```

nginx 按 [`nginx/README.md`](nginx/README.md) 配好。数据库要先建好空库和账号，表结构由发布脚本迁移。

## 发布

在开发机上（Linux；Windows 在 WSL 里），仓库根目录运行：

```bash
deploy/release.sh root@服务器
```

脚本依次：

1. `admin/` 下 `pnpm install --frozen-lockfile && pnpm build`（主站地址用 `NUXT_PUBLIC_SITE_URL` 覆盖，默认 `https://interk.net`）；
2. `SQLX_OFFLINE=true cargo build --release --locked -p ik-server`，并确认嵌进去的是刚编译的后台；
3. 上传可执行文件、服务定义与 `ik` 命令；
4. 用新版本跑 `ik migrate`（失败就停下，线上的旧版本不受影响）；
5. 替换 `/opt/interknot/ik-server`（旧的留作 `ik-server.prev`），重启，等 `/readyz` 就绪。

不带参数只编译，产物是 `target/release/ik-server`。在 Linux 上编译出来的可执行文件依赖编译机的
glibc 版本：编译机的发行版不要比服务器新。

不想在本机编译时（例如 Windows 上没有 WSL），在 GitHub 的 Actions 页面运行「Release」工作流
（推 `v*` 标签也会触发），它在 Ubuntu 24.04 上执行同一个脚本，产物里就是嵌好后台的 `ik-server`。
下载解压后在 Git Bash 里发布：

```bash
deploy/release.sh --binary ~/Downloads/ik-server root@服务器
```

## 运维命令

在服务器上用 `sudo ik …`，和服务使用同一个用户与配置文件：

```bash
sudo ik user set-role someone@example.com admin   # 第一个管理员
sudo ik search reindex
sudo ik legacy check                              # 旧数据导入，见 crates/legacy/README.md
journalctl -u ik-server -f                        # 日志
```

读文件的命令（如 `ik ai import <路径>`）以 `interknot` 用户运行，文件要让它读得到。

## 回滚

迁移只增不改，旧版本在新表结构上照常工作，换回旧的可执行文件即可：

```bash
sudo mv /opt/interknot/ik-server.prev /opt/interknot/ik-server && sudo systemctl restart ik-server
```
