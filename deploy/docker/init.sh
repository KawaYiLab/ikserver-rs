#!/bin/sh
# 生成 .env：从 env.example 复制，填上随机的数据库密码、Meilisearch 主密钥和两把服务密钥。
# 已经有 .env 时不覆盖。
#
#   ./init.sh        正式部署：生成后还要填站点地址和发信配置
#   ./init.sh try    本机试用：直接能用 http://127.0.0.1:8080 打开，不用配发信
set -eu
cd "$(dirname "$0")"

if [ -f .env ]; then
    echo ".env 已存在，没有改动" >&2
    exit 0
fi

rand() { openssl rand -base64 48 | tr -d '\n'; }
hex() { openssl rand -hex 24; }

meili=$(hex)
sed -e "s|^POSTGRES_PASSWORD=.*|POSTGRES_PASSWORD=$(hex)|" \
    -e "s|^MEILI_MASTER_KEY=.*|MEILI_MASTER_KEY=$meili|" \
    -e "s|^# IK_SEARCH__API_KEY=.*|# IK_SEARCH__API_KEY=$meili|" \
    -e "s|^IK_SECRETS__CODE_HMAC_KEY=.*|IK_SECRETS__CODE_HMAC_KEY=$(rand)|" \
    -e "s|^IK_SECRETS__QR_SESSION_KEY=.*|IK_SECRETS__QR_SESSION_KEY=$(rand)|" \
    env.example > .env
chmod 600 .env

if [ "${1:-}" = "try" ]; then
    sed -i.bak \
        -e "s|^IK_ENV=.*|IK_ENV=dev|" \
        -e "s|^IK_SITE_URL=.*|IK_SITE_URL=http://127.0.0.1:8080|" \
        -e "s|^IK_SESSION__COOKIE_SECURE=.*|IK_SESSION__COOKIE_SECURE=false|" \
        -e "s|^IK_MAIL__PROVIDER=.*|IK_MAIL__PROVIDER=dev_log|" \
        .env
    rm -f .env.bak
    echo "已生成本机试用的 .env。运行：docker compose up -d"
else
    echo "已生成 .env。填好站点地址和发信配置后运行：docker compose up -d"
fi
