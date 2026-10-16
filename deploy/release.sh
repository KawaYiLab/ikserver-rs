#!/usr/bin/env bash
# 发布 ik-server：编译管理后台 → 编译 ik-server（把后台嵌进去）→（可选）上传、迁移、重启。
#
#   deploy/release.sh                              只编译，产物是 target/release/ik-server
#   deploy/release.sh root@服务器                   编译并发布
#   deploy/release.sh --binary ik-server root@服务器  发布现成的可执行文件（GitHub Actions「Release」的产物）
#
# 编译需要 Linux（Windows 上在 WSL 里运行）、Rust、Node 22 与 pnpm 10；
# 只发布现成文件时有 bash、ssh、scp 就行（Windows 上用 Git Bash）。
# 服务器的一次性准备见 deploy/README.md。登录服务器的账号要能免密码 sudo（或直接用 root）。
set -euo pipefail

bin=
if [ "${1:-}" = "--binary" ]; then
    bin=$(realpath "${2:?--binary 后面跟可执行文件的路径}")
    shift 2
fi
host="${1:-}"
cd "$(dirname "$0")/.."

build() {
    local manifest=admin/.output/public/_nuxt/builds/latest.json

    echo "==> 编译管理后台"
    (
        cd admin
        pnpm install --frozen-lockfile
        # 后台里「查看帖子 / 主页」链接的主站地址，构建时写进页面
        NUXT_PUBLIC_SITE_URL="${NUXT_PUBLIC_SITE_URL:-https://interk.net}" pnpm build
    )

    echo "==> 编译 ik-server"
    SQLX_OFFLINE=true cargo build --release --locked -p ik-server
    bin=target/release/ik-server

    # 确认嵌进去的是刚编译的这一版后台（版本清单里的 id 每次构建都不同）
    local build_id
    build_id=$(sed -n 's/.*"id":"\([^"]*\)".*/\1/p' "$manifest")
    if [ -z "$build_id" ] || ! grep -qaF "$build_id" "$bin"; then
        echo "ik-server 里没有这次编译的管理后台（$manifest）" >&2
        exit 1
    fi
    echo "    $bin（管理后台 $build_id）"
}

if [ -z "$bin" ]; then
    build
fi

if [ -z "$host" ]; then
    exit 0
fi

echo "==> 上传到 $host"
scp -q "$bin" "$host:/tmp/ik-server.new"
scp -q deploy/systemd/ik-server.service deploy/systemd/ik "$host:/tmp/"

echo "==> 迁移数据库并重启"
ssh "$host" 'sudo bash -s' <<'REMOTE'
set -euo pipefail
install -m 0755 /tmp/ik /usr/local/bin/ik
install -m 0644 /tmp/ik-server.service /etc/systemd/system/ik-server.service
install -m 0755 /tmp/ik-server.new /opt/interknot/ik-server.new
rm -f /tmp/ik /tmp/ik-server.service /tmp/ik-server.new
systemctl daemon-reload

# 先用新版本迁移（失败就到此为止，正在运行的旧版本不受影响），再替换并重启。
# 迁移只增不改，迁移完成后旧版本仍能正常工作，所以回滚只需要换回旧的可执行文件。
IK_BIN=/opt/interknot/ik-server.new ik migrate
if [ -f /opt/interknot/ik-server ]; then
    mv -f /opt/interknot/ik-server /opt/interknot/ik-server.prev
fi
mv -f /opt/interknot/ik-server.new /opt/interknot/ik-server
systemctl enable --quiet ik-server
systemctl restart ik-server

for _ in $(seq 30); do
    if curl -fsS -o /dev/null http://127.0.0.1:8080/readyz; then
        echo "ik-server 已就绪"
        exit 0
    fi
    sleep 1
done
echo "ik-server 30 秒内没有就绪，看日志：journalctl -u ik-server -n 100" >&2
exit 1
REMOTE
