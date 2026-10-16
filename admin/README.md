# 绳网管理后台（admin/）

Nuxt 4 SPA + shadcn-vue + Tailwind v4。它是一个独立的子项目，构建产物由 nginx 托管在 `/admin`。
接口走同源的 `/v1/admin/*`，用论坛账号的网页会话（Cookie）登录，只有版主和管理员能进。

## 权限

| 页面 | 版主 | 管理员 |
| --- | --- | --- |
| 仪表盘、举报处理 | ✓ | ✓ |
| 用户：查看、封禁 / 解封 | ✓ | ✓ |
| 用户：角色、AI 账号、手动通过考试、丁尼 / 经验 | 只读 | ✓ |
| 入站题库、分区与标签、装扮 | 只读 | ✓ |
| AI 助手、搜索索引、审计日志 | — | ✓ |

前端只决定显示什么，真正的权限由后端的 `StaffOperator` / `AdminOperator` 校验。
所有写操作都会和审计日志在同一个事务里写入。

## 开发

需要 Node 22 和 pnpm 10。

```bash
cd admin
pnpm install
pnpm dev              # http://localhost:3000/admin/，/v1 会转发给本机的 ik-server（127.0.0.1:8080）
```

先在仓库根目录启动后端（`cargo run -p ik-server -- serve`），再用
`cargo run -p ik-server -- user set-role you@example.com admin` 给自己的账号设好角色。
开发服务器把 `/v1` 代理成同源请求，所以 Cookie 和 CSRF 校验都和线上一致。

## 接口类型

`app/types/api.d.ts` 是根据后端 OpenAPI 生成的，不要手改。后端接口改了以后要重新生成：

```bash
cargo run -p ik-server -- openapi > admin/openapi.json
cd admin && pnpm gen:api && pnpm typecheck
```

`openapi.json` 和 `api.d.ts` 都要提交，这样前端的类型检查不依赖本机能不能编译后端。

## 检查与构建

```bash
pnpm typecheck        # vue-tsc，模板也会检查
pnpm build            # nuxt generate → .output/public
```

## 部署

后台挂在后端域名下：`https://ik.tiwat.cn/admin/`，与 `/v1` 同源（见
[ADR 0002](../docs/adr/0002-same-origin-web-api.md)）。

```bash
pnpm install --frozen-lockfile && pnpm build
rsync -a --delete .output/public/ server:/srv/interknot/admin/
```

nginx 配置在 [`deploy/nginx/ik.tiwat.cn.conf`](../deploy/nginx/ik.tiwat.cn.conf)。预渲染的路由按
`<路由>/index.html` 返回，其余路径回退到 `200.html`；带哈希的 `_nuxt/` 资源长期缓存。

后台里「查看帖子 / 主页」会链接到主站，地址在构建时用 `NUXT_PUBLIC_SITE_URL` 设置（默认 `https://interk.net`）。

## 约定

- UI 组件放在 `app/components/ui/`，从 shadcn-vue 的 new-york-v4 注册表拉取。
  后台自己的组件放在 `app/components/admin/`，自动导入时带 `Admin` 前缀。
- 调接口统一写成 `unwrap(api.GET(...))`，失败时抛 `ApiError`，遇到 401 会自动回到登录页。
  写操作用 `useNotify().run()` 包一层，统一弹提示；确认框用 `useConfirm()`，
  高危操作传 `typeToConfirm` 要求用户手动输入确认。
- 时间一律按北京时间显示（`~/lib/format`）。
- 上传图片用 `~/lib/upload` 的 `uploadImage`：先签名，再直传到对象存储，最后调用完成接口。
