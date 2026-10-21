# 本地开发环境（Windows）

## 一次性安装

| 工具 | 安装 |
|---|---|
| Rust | `winget install Rustlang.Rustup`，然后 `rustup default stable-x86_64-pc-windows-gnu` |
| MinGW-w64 | `winget install BrechtSanders.WinLibs.POSIX.MSVCRT --scope user`（提供 gcc 与 dlltool） |
| sqlx-cli | `cargo install sqlx-cli --no-default-features --features postgres` |
| cargo-nextest | `cargo install cargo-nextest` |
| Node LTS | `winget install OpenJS.NodeJS.LTS`（仅管理后台需要） |

不需要 Docker、不需要 MSVC、不需要本机 PostgreSQL/Redis。

> 注意：windows-gnu 工具链自带的 mingw 只有链接器，不含汇编器；`windows-sys`、`getrandom`
> 等依赖编译时要调用 `dlltool`，必须装 MinGW-w64。选 **MSVCRT** 运行时版本，与 Rust 的
> `x86_64-pc-windows-gnu` 目标一致。安装后新开终端即可在 PATH 中找到 `gcc` / `dlltool`。
>
> 仓库不放 `rust-toolchain.toml`：本机默认宿主是 msvc，写 `stable` 会让 rustup 去下载
> 用不了的 msvc 工具链。Rust 版本由 CI 固定。

## 数据库（远程开发库）

开发统一连远程开发库（PostgreSQL），本机不跑数据库。连接串只写在 `.env` 的
`DATABASE_URL` 里（不入库），向维护者索取。

- 平时编译不连数据库：`.cargo/config.toml` 默认 `SQLX_OFFLINE=true`，SQL 宏按已提交的 `.sqlx/` 缓存检查。
  只有改了 SQL、运行 `cargo sqlx prepare --workspace -- --all-targets` 时才连开发库，这时开发库必须是最新结构
  （`cargo run -p ik-server -- migrate`，再加载一次旧表结构，见下文「旧数据导入」）。
- 开发账号需要 `CREATEDB` 权限：`#[sqlx::test]` 会为每个测试临时建库、测完删除。库在远程，每个测试要建库并跑完
  全部迁移，单个测试约一分钟，所以本地只跑相关的测试，全量交给 CI（见 `AGENTS.md`「本地与 CI 的分工」）。
- 迁移：`cargo sqlx migrate run`（只管 `migrations/`）；`cargo run -p ik-server -- migrate` 还会建好后台任务队列的表
  （schema `graphile_worker`，`serve` 启动执行器时也会自动建）。
- 测试里要入队任务的（例如有粉丝的作者发帖），先调用 `state.jobs.migrate()`；执行任务用
  `graphile_worker::WorkerOptions` 的 `run_once`（任务会安排后续任务时设 `concurrency(1)`，否则可能没跑完就返回），
  参考 `crates/content/tests/service.rs` 的 `run_jobs`。
- 开发库与生产库物理隔离；不要把生产连接串写进 `.env`。

## 环境变量

复制 `.env.example` 为 `.env`：

- `DATABASE_URL`：运行、数据库集成测试与 `cargo sqlx prepare` 使用；日常编译按 `.sqlx/` 缓存离线检查。
- `IK_*`：运行时配置，见 `crates/kernel/src/config.rs` 的层级注释。

## 日常

```powershell
$env:CARGO_INCREMENTAL = "0"
cargo check --workspace
cargo nextest run -p <crate> -E 'test(<测试名>)'
cargo fmt --all -- --check
cargo run -p ik-server -- serve
```

本地只跑本次改动相关的测试；全量测试与 clippy 由 CI 执行。改 bug 时先确认新测试在修之前失败、修之后通过。

打开 API 文档：<http://127.0.0.1:8080/docs>（Scalar）。

### 运维命令

**必须在仓库根目录执行**：程序从当前目录读取 `.env`，在别的目录运行会连到错误的库或连不上。

```powershell
cargo run -p ik-server -- user set-role someone@example.com admin     # 设置角色（创建第一个管理员）
cargo run -p ik-server -- category create --slug guide --name 攻略 --order 1
cargo run -p ik-server -- category create --slug notice --name 公告 --admin-only --allow-accept
cargo run -p ik-server -- exam import questions.json                  # 导入入站考试题库
cargo run -p ik-server -- search reindex                              # 推送搜索索引设置并全量重建
cargo run -p ik-server -- cosmetics import cosmetics.json             # 导入系统头像与论坛名片
```

题库文件是 JSON 数组，每题字段：

```json
[
  {
    "question": "绳网的货币叫什么？",
    "kind": "single",
    "options": [{ "key": "A", "text": "丁尼" }, { "key": "B", "text": "菲林" }],
    "correctKeys": ["A"],
    "explanation": "绳网通用货币是丁尼。",
    "weight": 1,
    "tag": "常识"
  }
]
```

`kind` 为 `single`（≥2 个选项、1 个答案）、`multiple`（≥3 个选项、≥2 个答案）或 `boolean`（恰好 2 个选项、1 个答案）；
`explanation`、`weight`（默认 1）、`enabled`（默认 true）、`tag` 可省略。逐题校验写入，遇到不合格的题会停下并报出是第几题。

装扮文件也是 JSON 数组，图片先用正常的上传流程传到图库，再引用媒体 id：

```json
[
  { "type": "avatar", "name": "邦布", "kind": "character", "imageId": "7251234567890123456" },
  { "type": "card", "name": "新艾利都", "kind": "city", "description": "……", "story": "Markdown 正文", "imageId": "7251234567890123457" }
]
```

### 数据库集成测试

`#[sqlx::test]` 每个测试会新建一个 `_sqlx_test_*` 临时库。远程开发库除了 `CREATEDB` 权限，
`pg_hba.conf` 还必须允许开发账号连接任意库（不能只允许连它自己的那一个库），例如：

```
host  all  <开发账号>  <你的 IP 段>  scram-sha-256
```

改完执行 `SELECT pg_reload_conf();` 生效。CI 用自带的 PostgreSQL 服务容器，不受影响。

### 旧数据导入（crates/legacy）

导入语句在编译期对照旧 Strapi 的表结构检查，所以开发库除了 `migrate`，还要加载一次旧表结构和辅助视图
（只建空表，不影响新表）：

```powershell
psql $env:DATABASE_URL -v ON_ERROR_STOP=1 -f crates/legacy/sql/strapi-schema.sql -f crates/legacy/sql/helpers.sql
```

改了 `helpers.sql` 之后重新执行第二个文件即可（它会先删掉再重建全部 `legacy.ik_*` 视图）。操作手册见
[`crates/legacy/README.md`](../crates/legacy/README.md)。

### 搜索引擎测试

`crates/search/tests` 里搜索引擎相关的用例需要一个 Meilisearch 实例（官方发布页下载单个可执行文件即可）：

```powershell
meilisearch --master-key dev-master-key-0123456789 --no-analytics
$env:IK_TEST_MEILI_URL = "http://127.0.0.1:7700"; $env:IK_TEST_MEILI_KEY = "dev-master-key-0123456789"
cargo nextest run -p ik-search -E 'test(<测试名>)'
```

媒体上传的用例同理需要一个 S3 兼容服务（CI 用的是 `pipx install 'moto[server]==5.2.4'` 后运行 `moto_server -p 9000`），
设置 `IK_TEST_S3_URL`（需要鉴权的服务另设 `IK_TEST_S3_ACCESS_KEY` / `IK_TEST_S3_SECRET_KEY`），每个用例用独立的桶。

没有设置 `IK_TEST_MEILI_URL` 时这些用例直接通过（数据库兜底的用例照常运行）。每个用例用独立的索引前缀，结束时删除索引。

### 管理后台

`admin/` 是独立的 Nuxt 4 子项目（Node 22 + pnpm 10），开发、生成接口类型和部署都写在
[`admin/README.md`](../admin/README.md) 里。它的构建产物在编译 release 版时嵌进 `ik-server`（发布流程见
[`deploy/README.md`](../deploy/README.md)）；没构建后台时后端照常编译，`/admin` 返回 404。后端接口改了以后，要重新生成 `admin/openapi.json`
和 `admin/app/types/api.d.ts`，然后运行 `pnpm typecheck`。

## 目录约定

- `.sqlx/`：编译期查询缓存（**提交**，CI 用 `SQLX_OFFLINE=true`）。
- `target/`：构建产物（gitignore）。

## 常见问题

- **`cargo check` 尝试连接数据库**：检查是否在当前环境中设了 `SQLX_OFFLINE=false`，它会覆盖
  `.cargo/config.toml` 的默认值；日常编译应使用 `SQLX_OFFLINE=true`。
- **SQL 宏提示缺少离线查询缓存**：查询有变化时，先迁移开发库并加载旧表结构，再运行
  `cargo sqlx prepare --workspace -- --all-targets`，提交更新后的 `.sqlx/`。
- **首次编译很慢或磁盘占用大**：编译前设 `CARGO_INCREMENTAL=0`；日常优先用
  `cargo check -p <crate>` 检查受影响的 crate。`target/` 太大时可用 `cargo clean` 清理构建产物。
