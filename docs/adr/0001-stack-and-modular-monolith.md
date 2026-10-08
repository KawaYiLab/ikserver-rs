# ADR 0001：技术栈与模块化单体

- 状态：已接受（2026-10-07）
- 背景：旧后端为 Strapi 5（~3.5 万行 TS），维护者为单人 + AI 编码。决定用 Rust 重写，
  接口与表结构全部重新设计，数据从旧库迁移，上线一次性切换。

## 决策

1. **Axum 0.8 + SQLx 0.9 + PostgreSQL**。
   - Actix-web 只在合成跑分快 3–8%，论坛瓶颈在数据库，选生态与 Tokio 官方维护的 Axum。
   - 不选 ORM（SeaORM/Diesel）：复杂 SQL（热榜、未读聚合）本就手写，SQLx 的编译期
     检查对 AI 生成代码是更强的安全网。
   - 不选 Loco：其用户/认证/迁移约定与"全新接口 + 数据迁移"冲突。
2. **模块化单体，不用微服务**。单进程含 HTTP、WS、任务、定时任务、AI 编排；
   业务域为独立 crate，边界靠 `pub` 可见性强制。
3. **去 Redis 化**：队列与定时任务用 graphile_worker（PG，SKIP LOCKED，与业务同事务入队）；
   缓存/限流/在线状态用进程内 moka/governor；多实例扩展点预留 PG LISTEN/NOTIFY。
4. **去 AstrBot**：AI 直连 OpenAI 兼容大模型 API（async-openai + 自写工具调用循环），
   人格/模型/额度全部进 `/v1/admin` 管理。
5. **不透明会话令牌**替代 JWT+refresh：每次请求查库（本会话表本来就热），吊销即时生效；
   Argon2id 为主，旧 bcrypt 哈希兼容校验并在登录时透明升级。
6. **接口重新设计**，不兼容旧 `/api`：网页/安卓用 OpenAPI 生成的 SDK 对接；
   旧 App 由旧后端返回 426 引导升级（上线窗口内）。
7. **管理后台**：独立 Nuxt 4 SPA + shadcn-vue + Tailwind v4，不复用主站 UI。
8. **Windows 开发用 gnu 工具链 + MinGW-w64**（不装 6GB 的 VS Build Tools）。优先选纯 Rust
   依赖；TLS 终端在 nginx，内网到 PG 不加密（同机/内网）。

## 后果

- 优点：编译期保证最大化（类型 + SQL 检查 + lints），部署为单二进制，运行内存目标
  <150MB，依赖服务从 6 个减到 3 个（PG、Meilisearch、S3）。
- 代价：首次切换需要三端（后端、网页、安卓）同日发布；旧 App 强制升级。
- 风险与缓解见根方案文档（Factory spec 2026-10-07-interknot-rust.md）第 16 节。
