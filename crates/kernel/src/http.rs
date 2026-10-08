//! HTTP 横切 helper：客户端 IP 推导、请求 ID 上下文。

use std::future::Future;
use std::net::IpAddr;

use http::HeaderMap;

tokio::task_local! {
    static REQUEST_ID: String;
}

/// 在请求 ID 上下文中执行 future（由 server 的中间件调用）。
pub async fn with_request_id<F: Future>(request_id: String, future: F) -> F::Output {
    REQUEST_ID.scope(request_id, future).await
}

/// 当前请求的 ID；不在请求上下文中（如后台任务）时返回 None。
pub fn current_request_id() -> Option<String> {
    REQUEST_ID.try_with(Clone::clone).ok()
}

/// 推导真实客户端 IP。
///
/// 每层可信代理都会把它看到的对端地址追加到 `X-Forwarded-For` 末尾，
/// 所以真实客户端位于从右往左数第 `trusted_hops` 个位置；更左边的值可被客户端伪造，绝不采用。
/// `trusted_hops == 0` 表示直连，忽略该头。
pub fn client_ip(headers: &HeaderMap, peer: IpAddr, trusted_hops: u32) -> IpAddr {
    if trusted_hops == 0 {
        return peer;
    }
    let Some(raw) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) else {
        return peer;
    };
    let hops: Vec<&str> = raw.split(',').map(str::trim).collect();
    let hops_usize = usize::try_from(trusted_hops).unwrap_or(usize::MAX);
    let index = hops.len().saturating_sub(hops_usize);
    hops.get(index)
        .and_then(|value| value.parse().ok())
        .unwrap_or(peer)
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;

    use super::*;

    fn headers(xff: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert("x-forwarded-for", HeaderValue::from_str(xff).unwrap());
        map
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn direct_connection_ignores_header() {
        assert_eq!(
            client_ip(&headers("1.1.1.1"), ip("9.9.9.9"), 0),
            ip("9.9.9.9")
        );
    }

    #[test]
    fn one_proxy_takes_rightmost_and_ignores_spoofed_left() {
        let h = headers("6.6.6.6, 1.2.3.4");
        assert_eq!(client_ip(&h, ip("127.0.0.1"), 1), ip("1.2.3.4"));
    }

    #[test]
    fn two_proxies_skip_cdn_hop() {
        let h = headers("6.6.6.6, 1.2.3.4, 10.0.0.2");
        assert_eq!(client_ip(&h, ip("127.0.0.1"), 2), ip("1.2.3.4"));
    }

    #[test]
    fn fewer_entries_than_hops_uses_leftmost() {
        assert_eq!(
            client_ip(&headers("1.2.3.4"), ip("127.0.0.1"), 3),
            ip("1.2.3.4")
        );
    }

    #[test]
    fn garbage_falls_back_to_peer() {
        assert_eq!(
            client_ip(&headers("not-an-ip"), ip("127.0.0.1"), 1),
            ip("127.0.0.1")
        );
        assert_eq!(
            client_ip(&HeaderMap::new(), ip("127.0.0.1"), 1),
            ip("127.0.0.1")
        );
    }

    #[tokio::test]
    async fn request_id_scope() {
        assert_eq!(current_request_id(), None);
        let inside = with_request_id("req-1".into(), async { current_request_id() }).await;
        assert_eq!(inside.as_deref(), Some("req-1"));
    }
}
