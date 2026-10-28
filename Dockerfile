# ik-server 镜像：先编管理后台，再编 ik-server（后台嵌进可执行文件），运行镜像里只有这一个文件。
#
#   docker build -t ik-server .
#
# 用法见 deploy/docker/README.md。

# ---------- 管理后台 ----------
FROM node:22-bookworm-slim AS admin
WORKDIR /src/admin
RUN corepack enable
COPY admin/package.json admin/pnpm-lock.yaml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile
COPY admin/ ./
RUN pnpm build

# ---------- ik-server ----------
FROM rust:1.99-bookworm AS server
WORKDIR /src
COPY . .
COPY --from=admin /src/admin/.output/public admin/.output/public
# 按已提交的 .sqlx 缓存离线检查 SQL，编译时不需要数据库。
ENV SQLX_OFFLINE=true CARGO_INCREMENTAL=0
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p ik-server \
    && cp target/release/ik-server /usr/local/bin/ik-server

# ---------- 运行 ----------
FROM debian:bookworm-slim
# 根证书从编译镜像拿（对象存储等 HTTPS 客户端用系统证书），不用再 apt-get。
COPY --from=server /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin interknot
COPY --from=server /usr/local/bin/ik-server /usr/local/bin/ik-server
USER interknot
ENV IK_HTTP__ADDR=0.0.0.0:8080 \
    IK_DATABASE__AUTO_MIGRATE=true \
    IK_LOG__FORMAT=json \
    RUST_LOG=info,sqlx=warn
EXPOSE 8080
ENTRYPOINT ["ik-server"]
CMD ["serve"]
