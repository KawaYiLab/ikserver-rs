# nginx

网页都和接口同源：页面所在域名的 nginx 把 `/v1` 反代到 ik-server。原因见
[ADR 0002](../../docs/adr/0002-same-origin-web-api.md)。

| 文件 | 放到 | 作用 |
| --- | --- | --- |
| `ik-upstream.conf` | `/etc/nginx/conf.d/` | ik-server 上游，WebSocket 的 `Connection` 映射 |
| `ik-api.inc` | `/etc/nginx/snippets/` | `/v1` 与 `/v1/realtime` 的反代，每个网页域名都 include |
| `interk.net.conf` | `/etc/nginx/sites-enabled/` | 主站：Nuxt + 同源 `/v1` |
| `ik.tiwat.cn.conf` | `/etc/nginx/sites-enabled/` | 后端域名：App 接口 + `/admin` 管理后台（页面由 ik-server 提供，nginx 只转发） |

后端相应的环境变量（完整模板见 [`../ik-server.env.example`](../ik-server.env.example)）：

```
IK_HTTP__TRUSTED_PROXY_HOPS=1   # 只有 nginx 一层；nginx 前面还有 CDN 时设为 2
```

`allowed_origins` 在正式环境不用配置：网页都同源，App 用 Bearer 令牌。

改完先跑 `nginx -t`，再 `systemctl reload nginx`。
