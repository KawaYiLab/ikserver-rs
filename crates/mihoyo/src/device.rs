//! 从发起扫码的浏览器 User-Agent 推出上游需要的设备信息（旧系统 `mihoyo.ts:115-162`）。

use crate::upstream::{Device, FALLBACK_USER_AGENT};

/// 新的设备信息：随机 device_id，型号与系统取自客户端 UA。
pub fn new_device(user_agent: Option<&str>) -> Result<Device, ik_kernel::Problem> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| ik_kernel::Problem::internal_from(&e))?;
    let device_id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let ua = user_agent
        .map(str::trim)
        .filter(|ua| !ua.is_empty())
        .unwrap_or(FALLBACK_USER_AGENT);
    Ok(Device {
        device_id,
        model: browser(ua),
        os: os(ua),
        // 上游只接受可打印 ASCII 的请求头。
        user_agent: if ua.bytes().all(|b| (0x20..0x7f).contains(&b)) {
            ua.to_owned()
        } else {
            FALLBACK_USER_AGENT.to_owned()
        },
    })
}

/// 浏览器名 + 主版本号，依次匹配 Edge / Opera / Chrome / Chromium / Firefox / Safari。
fn browser(ua: &str) -> String {
    const RULES: [(&str, &str); 6] = [
        ("Edg/", "Edge"),
        ("OPR/", "Opera"),
        ("Chrome/", "Chrome"),
        ("Chromium/", "Chromium"),
        ("Firefox/", "Firefox"),
        ("Version/", "Safari"),
    ];
    for (marker, name) in RULES {
        if name == "Safari" && !ua.contains("Safari/") {
            continue;
        }
        if let Some(rest) = ua.split(marker).nth(1) {
            let version: String = rest.chars().take_while(char::is_ascii_digit).collect();
            return if version.is_empty() {
                name.to_owned()
            } else {
                format!("{name} {version}")
            };
        }
    }
    "Unknown Browser".to_owned()
}

fn os(ua: &str) -> String {
    let inner = ua
        .split_once('(')
        .and_then(|(_, rest)| rest.split_once(')'))
        .map_or("", |(inner, _)| inner);
    let os = if inner.contains("iPhone") || inner.contains("iPad") {
        "iOS"
    } else if inner.contains("Android") {
        "Android"
    } else if inner.contains("Windows NT 10.0") {
        "Windows 10"
    } else if inner.contains("Windows NT 6.3") {
        "Windows 8.1"
    } else if inner.contains("Windows NT 6.2") {
        "Windows 8"
    } else if inner.contains("Windows NT 6.1") {
        "Windows 7"
    } else if inner.contains("Mac OS X") || inner.contains("Macintosh") {
        "macOS"
    } else if inner.contains("Linux") {
        "Linux"
    } else {
        "Web"
    };
    os.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_common_browsers() {
        let chrome = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
        let edge = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36 Edg/124.0.2478.51";
        let safari = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1";
        let firefox = "Mozilla/5.0 (X11; Linux x86_64; rv:125.0) Gecko/20100101 Firefox/125.0";
        assert_eq!(
            (browser(chrome), os(chrome)),
            ("Chrome 124".into(), "Windows 10".into())
        );
        assert_eq!(browser(edge), "Edge 124");
        assert_eq!(
            (browser(safari), os(safari)),
            ("Safari 17".into(), "iOS".into())
        );
        assert_eq!(
            (browser(firefox), os(firefox)),
            ("Firefox 125".into(), "Linux".into())
        );
        assert_eq!(
            (browser("curl/8"), os("curl/8")),
            ("Unknown Browser".into(), "Web".into())
        );
    }

    #[test]
    fn devices_are_random_and_fall_back_on_odd_agents() {
        let a = new_device(None).unwrap();
        let b = new_device(Some("中文 UA")).unwrap();
        assert_eq!(a.device_id.len(), 32);
        assert_ne!(a.device_id, b.device_id);
        assert_eq!(a.user_agent, FALLBACK_USER_AGENT);
        assert_eq!(b.user_agent, FALLBACK_USER_AGENT);
    }
}
