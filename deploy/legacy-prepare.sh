#!/usr/bin/env bash
# 旧数据导入的准备工作（crates/legacy/README.md 第 0–2 步，外加迁移与预检）：
#
#   1. 只读导出旧库（旧站不受影响）
#   2. 清空新库，把旧库恢复进去并改名为 legacy schema
#   3. 建新表（ik-server migrate）
#   4. 预检（ik-server legacy check，只读）
#
# 数据库连接从两边的 .env 里读，不用手填。清空新库前会做这些检查并要你输入库名确认：
# 新旧不能是同一个库；新库里不能有旧系统的表（防止新后端的配置误指向旧库）；新后端必须已经停止。
#
#   bash legacy-prepare.sh
#   OLD_DIR=/www/wwwroot/ikserver NEW_DIR=/opt/interknotserver bash legacy-prepare.sh   # 默认值
#
# 可以反复执行：每次都会重新导出旧库、重建新库。
set -euo pipefail

OLD_DIR=${OLD_DIR:-/www/wwwroot/ikserver}
NEW_DIR=${NEW_DIR:-/opt/interknotserver}
WORK=${WORK:-/root/ik-migrate}

die() { echo "✗ $*" >&2; exit 1; }
step() { echo; echo "==> $*"; }

# 取 .env 里某个变量的值（去掉 export、引号与 Windows 换行）。
envval() {
    [ -f "$1" ] || return 0
    sed -n -E "s/^[[:space:]]*(export[[:space:]]+)?$2[[:space:]]*=[[:space:]]*//p" "$1" | tail -n 1 \
        | tr -d '\r' | sed -E "s/^\"(.*)\"\$/\\1/; s/^'(.*)'\$/\\1/"
}

# PostgreSQL 客户端：PATH 里没有时找宝塔或发行版的安装位置。
if ! command -v pg_dump >/dev/null 2>&1; then
    for dir in /www/server/pgsql/bin /usr/lib/postgresql/*/bin /usr/pgsql-*/bin; do
        if [ -x "$dir/pg_dump" ]; then PATH="$dir:$PATH"; break; fi
    done
fi
for tool in psql pg_dump pg_restore; do
    command -v "$tool" >/dev/null 2>&1 || die "找不到 $tool（PostgreSQL 客户端）"
done

[ -f "$OLD_DIR/.env" ] || die "找不到旧后端的配置 $OLD_DIR/.env"
[ -f "$NEW_DIR/.env" ] || die "找不到新后端的配置 $NEW_DIR/.env"
[ -x "$NEW_DIR/ik-server" ] || die "找不到新后端程序 $NEW_DIR/ik-server"

# 旧库：DATABASE_URL，或 Strapi 的 DATABASE_HOST / PORT / NAME / USERNAME / PASSWORD。
old_client=$(envval "$OLD_DIR/.env" DATABASE_CLIENT)
case "${old_client:-postgres}" in
    postgres | pg | postgresql) ;;
    *) die "旧库是 $old_client，这个工具只支持 PostgreSQL" ;;
esac
OLD_URL=$(envval "$OLD_DIR/.env" DATABASE_URL)
OLD_HOST=$(envval "$OLD_DIR/.env" DATABASE_HOST)
OLD_PORT=$(envval "$OLD_DIR/.env" DATABASE_PORT)
OLD_NAME=$(envval "$OLD_DIR/.env" DATABASE_NAME)
OLD_USER=$(envval "$OLD_DIR/.env" DATABASE_USERNAME)
OLD_PASS=$(envval "$OLD_DIR/.env" DATABASE_PASSWORD)
old() {
    if [ -n "$OLD_URL" ]; then
        "$@" -d "$OLD_URL"
    else
        PGHOST=${OLD_HOST:-localhost} PGPORT=${OLD_PORT:-5432} PGDATABASE=${OLD_NAME:-strapi} \
            PGUSER=${OLD_USER:-strapi} PGPASSWORD=$OLD_PASS "$@"
    fi
}

NEW_URL=$(envval "$NEW_DIR/.env" IK_DATABASE__URL)
[ -n "$NEW_URL" ] || NEW_URL=$(envval "$NEW_DIR/.env" DATABASE_URL)
[ -n "$NEW_URL" ] || die "$NEW_DIR/.env 里没有 DATABASE_URL"
new() { "$@" -d "$NEW_URL"; }

identity="SELECT current_database() || ' @ ' || coalesce(host(inet_server_addr()), 'socket') || ':' || inet_server_port()"
step "检查连接"
old_id=$(old psql -X -Atq -c "$identity") || die "连不上旧库"
new_id=$(new psql -X -Atq -c "$identity") || die "连不上新库"
echo "旧库：$old_id"
echo "新库：$new_id"
[ "$old_id" != "$new_id" ] || die "新旧配置指向同一个库，停止"
[ "$(old psql -X -Atq -c "SELECT to_regclass('public.up_users') IS NOT NULL")" = t ] \
    || die "旧库里没有 up_users 表，不像是旧系统的库"
[ "$(new psql -X -Atq -c "SELECT to_regclass('public.up_users') IS NOT NULL")" = f ] \
    || die "新库的 public 里有旧系统的表（up_users），新后端的配置可能指向了旧库，停止"
if pgrep -x ik-server >/dev/null 2>&1; then
    die "新后端还在运行（ik-server），先停掉它：systemctl stop ik-server，或在运行它的终端按 Ctrl+C"
fi
old psql -X -Atq -c "SELECT '旧库账号数：' || count(*) FROM up_users"
if [ "$(new psql -X -Atq -c "SELECT to_regclass('public.users') IS NOT NULL")" = t ]; then
    new psql -X -Atq -c "SELECT '新库现有账号数（会被清空）：' || count(*) FROM users"
fi

new_db=$(new psql -X -Atq -c "SELECT current_database()")
echo
echo "接下来会清空新库 $new_db 里的全部数据（public、graphile_worker、legacy 三个 schema），旧库只读不动。"
if [ -n "${CONFIRM:-}" ]; then
    answer=$CONFIRM # 无人值守时用 CONFIRM=<新库名> 代替手动输入
else
    read -r -p "确认请输入新库的名字 $new_db：" answer </dev/tty
fi
[ "$answer" = "$new_db" ] || die "输入不一致，已取消"

mkdir -p "$WORK"
chmod 700 "$WORK"
dump="$WORK/interknot.dump"

step "1/4 导出旧库 → $dump"
old pg_dump -Fc -n public --no-owner --no-privileges -f "$dump"
ls -lh "$dump"

step "2/4 清空新库，恢复旧数据并改名为 legacy"
new psql -X -q -v ON_ERROR_STOP=1 \
    -c 'DROP SCHEMA IF EXISTS legacy CASCADE' \
    -c 'DROP SCHEMA IF EXISTS graphile_worker CASCADE' \
    -c 'DROP SCHEMA IF EXISTS public CASCADE' \
    -c 'CREATE SCHEMA public'
# public schema 本身与扩展由新库自己负责，只恢复表、序列、索引与数据。
pg_restore -l "$dump" | grep -vE ' SCHEMA - public | COMMENT - SCHEMA public | EXTENSION ' >"$WORK/restore.list"
pg_restore --no-owner --no-privileges --exit-on-error -L "$WORK/restore.list" -d "$NEW_URL" "$dump"
new psql -X -q -v ON_ERROR_STOP=1 \
    -c 'ALTER SCHEMA public RENAME TO legacy' \
    -c 'CREATE SCHEMA public'
new psql -X -Atq -c "SELECT 'legacy 里的表：' || count(*) FROM pg_tables WHERE schemaname = 'legacy'"

step "3/4 建新表"
cd "$NEW_DIR"
./ik-server migrate

step "4/4 预检（只读）"
./ik-server legacy check

step "时区参考（第 5 步导入要用）"
date
timedatectl 2>/dev/null | grep -i 'time zone' || true

echo
echo "✓ 准备完成。把上面的输出发给维护者，确认预检结果后再做第 3 步（读取图片）和第 5 步（导入）。"
