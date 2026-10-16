# 把 /v1 转给 ik-server。在每个网页域名的 server 块里 include（ADR 0002）。
# 后端设置 IK_HTTP__TRUSTED_PROXY_HOPS=1（nginx 前面还有 CDN 时设为 2）。

location /v1/ {
    proxy_pass http://ik_server;
    proxy_http_version 1.1;
    proxy_set_header Connection "";
    # 必须原样转发 Host（含端口，所以用 $http_host 而不是 $host）：CSRF 回退校验和
    # WebSocket 同源判断都比较 Origin 与 Host。
    proxy_set_header Host $http_host;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
    # 正文上限 2MB（图片直传到对象存储，不经过这里）；留一点余量，由后端返回规范的 413。
    client_max_body_size 4m;
    proxy_read_timeout 60s;
}

location = /v1/realtime {
    proxy_pass http://ik_server;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection $ik_connection_upgrade;
    proxy_set_header Host $http_host;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
    # 心跳 30 秒一次，读超时要比它长。
    proxy_read_timeout 120s;
    proxy_send_timeout 120s;
}
