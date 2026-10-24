# ADR 0002：网页同源访问接口（反向代理 `/v1`）

- 状态：已被 [ADR 0005](0005-edgeone-direct-origin.md) 取代（2026-10-23）。线上没有按本文的 nginx 同源方案部署，
  主站跨站调用 `ik.tiwat.cn`；本文保留作为将来改回同源时的依据（同源的好处见下文）。
- 原状态：已接受（2026-10-16）
- 背景：主站在 `interk.net`，后端在 `ik.tiwat.cn`，二者不是同一个站点（registrable domain
  不同）。网页登录用 `HttpOnly; Secure; SameSite=Lax` 会话 Cookie，且不设 `Domain`，
  只属于签发它的主机。主站直接用 fetch 跨站调用 `ik.tiwat.cn` 时，浏览器不会带这个 Cookie，
  网页登录态无法工作。旧 Strapi 把 JWT 放在请求头里，所以以前跨站没有问题。

## 决策

每个网页都和接口**同源**：页面所在域名的 nginx 把 `/v1`（包括 `/v1/realtime`）反向代理到
ik-server。浏览器只和自己的域名通信。

| 域名 | 内容 | 接口 |
| --- | --- | --- |
| `interk.net` | 主站（Nuxt） | `interk.net/v1` → ik-server |
| `ik.tiwat.cn` | 管理后台 `/admin`（SPA，编进 ik-server）；安卓 App 的接口地址 | `ik.tiwat.cn/v1` → ik-server |

当时的 nginx 配置示例在 git 历史的 `deploy/nginx/`。

### 为什么不用 `api.interk.net` 子域

`api.interk.net` 与 `interk.net` 同站，`SameSite=Lax` 的 Cookie 也能带上，但同源代理在每一点上都更好：

- **没有 CORS。** 跨源的 JSON 写请求每次都要先发一个 OPTIONS 预检，写操作的延迟翻倍；同源没有预检。
- **Cookie 作用域最小。** 保持不设 `Domain`、只属于页面所在主机。如果改成 `Domain=interk.net`
  让子域和 SSR 都能读到，`dev.interk.net` 这类子域也会收到正式环境的会话 Cookie。
- **服务端渲染直接可用。** 主站 Nuxt 的 SSR 进程在同一个域名下就能收到会话 Cookie；
  放在子域上时，Cookie 只属于 `api.interk.net`，SSR 看不到登录态。
- **CSRF 判断简单。** 浏览器发 `Sec-Fetch-Site: same-origin`，`CsrfLayer` 直接放行；
  `allowed_origins` 在正式环境可以留空，攻击面最小。
- **不用新增证书和 DNS 记录。**

代价：主站域名的入口必须能按路径转发（nginx 或支持 WebSocket 的 CDN 回源规则），
并且多一跳本机反代（同机或内网，开销可以忽略）。

## 实施要求

1. **原样转发 `Host`**（`proxy_set_header Host $http_host`）。CSRF 的回退校验和 WebSocket 握手的同源判断
   都比较 `Origin` 和 `Host`。
2. **`X-Forwarded-For` 用 `$proxy_add_x_forwarded_for`**，并设置 `IK_HTTP__TRUSTED_PROXY_HOPS`：
   只有 nginx 一层时设为 1；nginx 前面还有 CDN 时设为 2。限流和审计日志里的 IP 都依赖这个值。
3. **WebSocket**：`/v1/realtime` 要转发 `Upgrade` / `Connection` 头，读超时要大于心跳间隔（30 秒）。
4. **主站 SSR**：Nuxt 服务端直接请求 `http://127.0.0.1:8080/v1`（不要绕回公网域名），
   转发浏览器的 `Cookie`，并把收到的 `X-Forwarded-For` 原样带上（不要再追加自己的地址），
   这样后端看到的仍是用户的 IP。SSR 拿到的 `Set-Cookie` 要回传给浏览器。
5. 安卓 App 用 `Authorization: Bearer`，不受同源限制，继续使用 `ik.tiwat.cn`。

## 后果

- `docs/api-guidelines.md` 的基础地址改为「页面所在域名的 `/v1`」。
- WebSocket 握手除了白名单，也放行同源页面（`Origin` 的主机与 `Host` 相同，RL-005）。
- 再加网页域名（例如 `tiwat.cn`）时照同样的方式配一个 server 块，不需要改后端。
