# ADR 0005：生产拓扑——EdgeOne 直接回源 ik-server，主站跨站调用

- 状态：已接受（2026-10-23），取代 [ADR 0002](0002-same-origin-web-api.md) 的部署方式
- 背景：ADR 0002 设想由页面所在域名的 nginx 把 `/v1` 反代给 ik-server。实际上线的是：

| 域名 | 由谁提供 | 内容 |
| --- | --- | --- |
| `interk.net` | Cloudflare Pages（静态站点） | 主站 InterKnot-Web，构建时 `NUXT_PUBLIC_API_BASE_URL=https://ik.tiwat.cn` |
| `ik.tiwat.cn` | 腾讯云 EdgeOne → 服务器上 ik-server 的端口 | `/v1`、`/v1/realtime`、`/admin`；`/app/*` 另回源到对象存储（安卓更新渠道） |
| `im.tiwat.cn` | 对象存储的 CDN | 用户上传的图片等文件 |

服务器上没有 nginx：EdgeOne 直接连 ik-server 监听的端口。网页、管理后台、安卓 App 都调用 `ik.tiwat.cn`。

## 决策

按上线的样子定下来：

1. **ik-server 直接作为 EdgeOne 的源站。** 监听公网端口，`IK_HTTP__TRUSTED_PROXY_HOPS=1`：
   EdgeOne 回源时把它看到的对端地址追加在 `X-Forwarded-For` 末尾，从右数第 1 个就是用户 IP。
2. **源站只接受 EdgeOne 的连接。** 端口对公网开放时，任何人绕过 EdgeOne 直连都能伪造
   `X-Forwarded-For`，绕过限流、写进审计日志的 IP 也不可信。所以防火墙 / 安全组只放行
   EdgeOne 的回源 IP 段（EdgeOne 控制台「源站防护」给出列表并在变动时通知）。
3. **主站跨站调用接口。** `IK_HTTP__ALLOWED_ORIGINS` 写主站，`IK_SESSION__COOKIE_CROSS_SITE=true`：
   会话 Cookie 是 `SameSite=None; Secure; Partitioned`，按顶层站点分区，只在主站页面里随请求发出；
   跨站写请求由 CSRF 白名单把关。WebSocket 握手同样校验 `Origin` 白名单。
4. **管理后台**与接口同源（`ik.tiwat.cn/admin` 调 `ik.tiwat.cn/v1`），不受跨站影响；安卓 App 用 Bearer 令牌。

## 后果

- 部署说明（`deploy/interknot/README.md`）以这个拓扑为准；仓库不再带 nginx 配置。
- 主站每次请求都是跨源的：写请求先发一次 OPTIONS 预检（预检结果缓存 10 分钟，`access-control-max-age: 600`）。
- 主站登录依赖浏览器接受分区的跨站 Cookie。将来要彻底避开第三方 Cookie 与预检，按 ADR 0002 的思路改成同源：
  例如在 Cloudflare 上用 Worker 把 `interk.net/v1/*`（含 WebSocket）转给 `ik.tiwat.cn`，主站构建时
  `NUXT_PUBLIC_API_BASE_URL` 留空，后端关掉 `COOKIE_CROSS_SITE`。届时客户端 IP 的取法与 `TRUSTED_PROXY_HOPS`
  要按新的代理链重新确定，另写 ADR。
- EdgeOne 到源站是明文 HTTP（ik-server 不终止 TLS）；源站防护只放行 EdgeOne 之后，这段链路在腾讯云网络与服务器之间。
  需要加密时在 EdgeOne 用 HTTPS 回源，并在服务器上加一层终止 TLS 的代理（此时 `TRUSTED_PROXY_HOPS=2`）。
