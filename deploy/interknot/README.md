# 部署

## 线上拓扑

```
浏览器（interk.net 页面）──┐
管理后台（ik.tiwat.cn/admin）─┼─▶ EdgeOne（ik.tiwat.cn）──▶ 服务器:8080  ik-server
安卓 App ──────────────────┘          │
                                      └─ /app/* ──▶ 对象存储（安卓更新渠道）
```

| 域名 | 由谁提供 | 内容 |
| --- | --- | --- |
| `ik.tiwat.cn` | 腾讯云 EdgeOne，回源到服务器上 ik-server 的端口（**没有 nginx**） | 接口 `/v1`、实时通道 `/v1/realtime`、管理后台 `/admin`；`/app/*` 回源到对象存储 |
| `interk.net` | Cloudflare Pages 静态站点（InterKnot-Web 仓库） | 主站；构建时 `NUXT_PUBLIC_API_BASE_URL=https://ik.tiwat.cn`，跨站调用接口 |
| `im.tiwat.cn` | 对象存储的 CDN | 上传的文件（`IK_STORAGE__PUBLIC_BASE_URL`） |

为什么是这样、有什么代价见 [ADR 0005](../docs/adr/0005-edgeone-direct-origin.md)。

`ik-server` 是一个可执行文件，管理后台（`admin/`）在编译时嵌进去，不需要单独发布。
依赖的服务：PostgreSQL、Meilisearch、对象存储。

| 文件 | 作用 |
| --- | --- |
| [`release.sh`](release.sh) | 编译后台与 ik-server，上传、迁移、重启 |
| [`../.github/workflows/release.yml`](../.github/workflows/release.yml) | 在 GitHub Actions 上编译同样的可执行文件 |
| [`systemd/ik-server.service`](systemd/ik-server.service) | 服务定义（发布时自动安装） |
| [`systemd/ik`](systemd/ik) | 在服务器上跑运维命令的包装（发布时装到 `/usr/local/bin/ik`） |
| [`ik-server.env.example`](ik-server.env.example) | 生产配置模板 |
| [`legacy-prepare.sh`](legacy-prepare.sh) | 旧数据导入的准备：导出旧库、恢复到新库的 legacy schema、迁移、预检（见 crates/legacy/README.md） |

## 服务器一次性准备

```bash
sudo useradd --system --home-dir /opt/interknot --shell /usr/sbin/nologin interknot
sudo install -d -m 0755 /opt/interknot
sudo install -d -m 0750 -g interknot /etc/interknot
# 从仓库拷过去后填写（数据库、密钥、对象存储……）
sudo install -m 0640 -o root -g interknot ik-server.env.example /etc/interknot/ik-server.env
```

数据库要先建好空库和账号，表结构由发布脚本迁移。

与拓扑有关的配置（模板里已写好）：

```
IK_HTTP__ADDR=0.0.0.0:8080                        # EdgeOne 回源到这个端口；release.sh 用 127.0.0.1:8080/readyz 检查就绪
IK_HTTP__TRUSTED_PROXY_HOPS=1                     # 只有 EdgeOne 一层
IK_HTTP__ALLOWED_ORIGINS=["https://interk.net"]   # 主站跨站调用（CORS、CSRF、WebSocket 握手都看它）
IK_SESSION__COOKIE_CROSS_SITE=true                # 主站的会话 Cookie 用 SameSite=None; Partitioned
```

**防火墙 / 安全组：8080 只放行 EdgeOne 的回源 IP 段**（EdgeOne 控制台 → 站点 → 安全防护 → 源站防护，
那里有当前 IP 列表，变动时会通知）。不这样做，谁都能绕过 EdgeOne 直连 8080 并伪造 `X-Forwarded-For`，
限流按 IP 计的部分就失效了，审计日志里的 IP 也不可信。PostgreSQL、Meilisearch 只监听本机。

层数是否配对：登录后在「登录设备」或审计日志里看到的 IP 应是自己的公网 IP。看到的是腾讯云的地址说明层数少了，
看到的是自己在请求头里随便写的 `X-Forwarded-For` 说明源站没有只放行 EdgeOne。

## EdgeOne

站点 `ik.tiwat.cn` 的设置（控制台里的名字可能随版本略有不同）：

| 项 | 设置 | 原因 |
| --- | --- | --- |
| 源站 | 服务器 IP，HTTP 回源，端口 8080 | ik-server 不终止 TLS |
| HTTPS | 强制 HTTPS（现网已开，HTTP 请求 302 到 HTTPS） | 会话 Cookie 带 `Secure`，明文请求拿不到 |
| 规则：URL 路径等于 `/` | 可选：重定向到 `https://interk.net/` | ik-server 在根路径返回 404 |
| 回源 Host | 保持加速域名 `ik.tiwat.cn` | 同源判断（管理后台的 CSRF 回退校验、WebSocket 握手）比较 `Origin` 与 `Host` |
| WebSocket | 开启，超时 ≥ 120 秒 | 实时通道心跳 30 秒、60 秒没有往来就断开；超时小于心跳间隔会被 EdgeOne 先掐断 |
| 缓存（`/v1/*`、`/admin/*`、`/healthz`、`/readyz`） | 遵循源站 `Cache-Control` | 公开 GET 由后端给 `public, max-age=…`，带凭据的请求一律 `private`/`no-store`；不要配「全部缓存」，会把一个人的数据发给另一个人 |
| 缓存键 | 保留全部查询参数，**并加上请求头 `Origin`** | 分页游标、筛选都在查询参数里。EdgeOne 不认 `Vary: Origin`（实测 `/v1/emotes`：先被不带 `Origin` 的 App 请求填进缓存，主站再请求就命中一份没有 `Access-Control-Allow-Origin` 的响应，浏览器按跨域错误拦下） |
| 回源超时 | 默认即可；`/v1/admin/story/corpus` 调到 120 秒 | 剧情库语料包上传后要建索引 |
| 上传大小 | ≥ 64 MB | 剧情库语料包（约 6 MB，后端上限 64 MB）；其余接口正文上限 2 MB，图片直传对象存储不经过这里 |
| 规则：URL 路径 `/app/*` | 回源到安卓更新渠道的对象存储（回源 Host 填存储域名），缓存遵循源站 | 安卓 App 的 `version.json` 与安装包，由 InterKnot-Android 的发版工作流上传，见该仓库 `docs/release.md` |

`https://ik.tiwat.cn/app/android/version.json` 返回 404 而且是 `application/problem+json`，说明 `/app/*` 规则没生效、请求落到了 ik-server。

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

## 剧情库（MCP）

`POST /v1/story/mcp` 是给剧情总结 AI 用的只读 MCP 服务，语料来自
[zzz-story-extractor](https://github.com/KawaYiLab/zzz-story-extractor) 的产出，存在数据库里（`story_corpora`），
服务器上不需要放任何文件。还没有上传语料时接口返回 503 `STORY_DISABLED`，其余功能不受影响。

1. 服务器配置访问密钥（只需一次，改完 `sudo systemctl restart ik-server`）：

   ```bash
   # /etc/interknot/ik-server.env
   IK_STORY__ACCESS_KEY=<openssl rand -hex 32>
   ```

2. 在本地提取并一键上传（管理员账号；密码从环境变量 `IK_PASSWORD` 读，不设则交互输入）：

   ```powershell
   .\.venv\Scripts\python.exe -m zzz_story extract --out out
   .\.venv\Scripts\python.exe -m zzz_story upload --server https://ik.tiwat.cn --account 管理员邮箱
   ```

   上传的是 gzip 压缩的语料包（约 6 MB）。服务端校验并建好索引后设为生效版本，本实例立即切换，
   多实例时其他实例一分钟内跟上；同一份数据重复上传不会多存一份。数据更新时重复这一步即可，不用重启。

3. 版本与回滚（管理员）：`GET /v1/admin/story/corpora` 查看最近 5 个版本，
   `POST /v1/admin/story/corpora/{id}/activate` 回滚到其中一个。上传与回滚都记在审计日志里。

EdgeOne 的上传大小与这个接口的回源超时要放宽，见上文「EdgeOne」。

本地调试工具（不连数据库；将 `--source` 替换为实际的提取输出目录或 `.json.gz` 语料包）：

```bash
cargo run --release -p ik-server -- story call --source /path/to/corpus \
    ik_search '{"keyword":"零号空洞"}'
```

客户端（Claude Code / Claude Desktop / Cursor 等）配置：

```json
{
  "mcpServers": {
    "绳网档案": {
      "type": "streamable-http",
      "url": "https://ik.tiwat.cn/v1/story/mcp",
      "headers": { "Authorization": "Bearer <IK_STORY__ACCESS_KEY>" }
    }
  }
}
```

工具：`ik_catalog`（章节与场景目录）、`ik_search`（多词先同篇 AND，无完整匹配时默认标注部分匹配；`allowPartial=false` 要求严格匹配）、`ik_read`（按字数预算顺读，
`follow=true` 跨场景读完整章）、`ik_character`（角色出场与档案）、`ik_guide`（章节 / 角色 / 顺序与时间 / 检索与出处的契约参考）、
`ik_coverage`（合并调用者累计的 `read.coverageDelta`，对照 `catalog.manifest` 核对资料覆盖与缺口）。

版本别名支持 `ik_catalog(path="3.3版本")`、`ik_search(keyword="3.3 核心")` 和
`ik_read(docId="v3.3")`，限定该版新增主线。别名映射与检索规则见 [剧情库检索说明](../docs/story-search.md)。

## QQ 官方机器人（ADR 0008）

提供游客对话、站内绑定（个人额度与记忆）、群共享记忆和后台管理。默认关闭。

1. QQ 开放平台建立官方机器人，选择 Webhook，登记服务器调用 OpenAPI 的出口 IP；准备沙箱群。
2. 按 `ik-server.env.example` 配置 `IK_QQBOT__APPS__MAIN__APP_ID` 与 `SECRET`，启用 `IK_QQBOT__ENABLED=true`。
   测试时设置 `SANDBOX=true`；未显式设置 API_BASE 时自动选沙箱域名。不要把密钥写入数据库或提交仓库。
3. 回调地址设置为 `https://ik.tiwat.cn/v1/qq/webhook`，经 EdgeOne 443 直接回源；保留 X-Bot-Appid、签名头及原始请求体。
   此路径禁用缓存，并避免按来源 IP 的边缘限流误伤集中回调（服务内部按 AppID 限流）。
4. 仅在填写/修改回调地址的几分钟内开启 `IK_QQBOT__ALLOW_VALIDATION=true` 并重启；平台验证完成后立即改回 false 并重启。
   无签名验证只接受数字时间戳和字母数字 token；不要放宽格式去签任意文本。
5. 后台「AI 助手」给角色打开 QQ，后台「QQ 机器人」选机器人的默认角色并开启机器人、全站 QQ 开关。
   绑定码与验证码共用 `IK_SECRETS__CODE_HMAC_KEY`（多实例必须相同，否则一个实例发的码另一个实例认不出）。
   群入站后自动进入群列表，可填展示名、覆盖角色、封禁或重置群上下文。
6. 先使用默认 `linkMode=strip`；要试 domain，先在开放平台「开发设置 → 消息 URL 配置」登记 interk.net，
   实测通过才开启。QQ 明确返回 URL 拒绝后，调度器先保存去链接的新正文版本，再按原序号重试一次。

上线前完成 ADR 第 0 期的待测项：带链接能否发送及实际错误码、群主/管理员身份字段。沙箱回复次数不受正式限制，
群 5 分钟/5 次、单聊 60 分钟/4 次的验收需在正式环境完成。运营同时确认隐私政策与角色内容口径。
后台 `qq_turns` 保存状态、原因及用量；重复任务不会重新生成。排障不打印 token、secret 或 openid 全文。

### QQ 调度状态机首次切换

该升级必须让旧 QQ 执行器退出后再启动新执行器，不能滚动混跑投递逻辑。回调接收与任务执行可用已有
`IK_JOBS__RUN_WORKER=false` 分离：维护期间保留接收进程，让新消息继续入库；它同时暂停该进程的其他后台任务。

1. 备份数据库，记录当前可执行文件版本。先停止所有旧任务执行器领取任务并等在途工作结束，保留接收入口。
   首次升级前的版本不认识 `dispatch_paused`，不能仅改这个数据库字段便认为旧执行器已经暂停。
2. 使用新版本执行 `ik-server migrate`。所有旧执行器退出后，启用新版本接收进程，在后台保存“暂停生成和发送”。
   新版本的 `GET /v1/admin/qq/dispatch` 可查看生成、待发和有效租约数量；“执行中”为 0 才表明已排空当前持有者。
3. 启动新 worker。`qq.reconcile` 每分钟恢复旧任务：旧 queued 在有效窗口内分配会话序号；旧 processing 无输出按中断生成结算且不重跑；旧输出不重发无法确认的部分，保留已确认前缀，零确认退游客次数，至少一段确认则保留一次。历史终态账单不重算，也不猜历史绑定关系。
4. 取消暂停，确认后台有新分段尝试记录、队列推进和成本/次数结算时间。消息的原截止不随维护延长，过期消息正常结束。
   检查 `delivery_unknown`、待审核和限频原因时同时查看 HTTP、code、err_code、Trace ID；不要手动换 msg_seq 重发。

后续同协议版本发布可直接用后台独立暂停开关排空；每段 HTTP 最长 10 秒，生成默认最长 90 秒。
当前 systemd 的 `TimeoutStopSec=60` 可能短于生成预算，应先观察排空再重启，避免以强制停止作为正常排空手段。
若进程仍被强停，30 秒租约及 reconcile 恢复终态，已开始的生成不重复执行。

本地发送策略：每 AppID 5 POST/秒、最多 4 个在途请求；每进程最多 2 个 QQ 生成。
每段最多 3 次 POST，HTTP 10 秒，重试延迟 2/5 秒加抖动，输出落库后最多发送 60 秒并受平台截止约束。
这些是我们的运行策略，不是官方配额。[协议和边界依据](../docs/business-rules/qq-bot.md)随代码维护。

## 回滚

通常增量迁移允许换回兼容的可执行文件。**QQ 调度状态机升级后，只能回到认识新表、新任务和结算标记的版本**；
先暂停并排空，不能把旧 `qq.reply` 投递器直接换回来。下面的命令仅适用于已确认数据/任务协议兼容的版本：

```bash
sudo mv /opt/interknot/ik-server.prev /opt/interknot/ik-server && sudo systemctl restart ik-server
```
