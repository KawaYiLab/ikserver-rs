//! 收件人分区（MAIL-001）：域名在国内名单里为国内，其余（含格式不对的地址）为国外。

use std::collections::HashSet;

/// 常见国内邮箱域名（MAIL-001 / EMAIL-002，小写精确匹配）。
const DOMESTIC: [&str; 26] = [
    "qq.com",
    "vip.qq.com",
    "foxmail.com",
    "163.com",
    "vip.163.com",
    "126.com",
    "yeah.net",
    "sina.com",
    "sina.cn",
    "vip.sina.com",
    "sohu.com",
    "aliyun.com",
    "139.com",
    "189.cn",
    "wo.cn",
    "tom.com",
    "21cn.com",
    "sina.com.cn",
    "263.net",
    "x263.net",
    "etang.com",
    "eyou.com",
    "56.com",
    "chinaren.com",
    "188.com",
    "vip.188.com",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Domestic,
    International,
}

impl Region {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Domestic => "domestic",
            Self::International => "international",
        }
    }
}

/// 域名取最后一个 `@` 之后并转小写；`extra` 是配置里追加的国内域名（已是小写）。
pub fn classify(email: &str, extra: &HashSet<String>) -> Region {
    let Some((local, domain)) = email.rsplit_once('@') else {
        return Region::International;
    };
    if local.is_empty() || domain.is_empty() {
        return Region::International;
    }
    let domain = domain.to_lowercase();
    if DOMESTIC.contains(&domain.as_str()) || extra.contains(&domain) {
        Region::Domestic
    } else {
        Region::International
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_domestic_domains_and_extras() {
        let none = HashSet::new();
        assert_eq!(classify("a@qq.com", &none), Region::Domestic);
        assert_eq!(classify("a@VIP.163.COM", &none), Region::Domestic);
        assert_eq!(classify("we@ird@sina.cn", &none), Region::Domestic);
        assert_eq!(classify("a@gmail.com", &none), Region::International);
        assert_eq!(classify("a@corp.cn", &none), Region::International);
        let extra = HashSet::from(["corp.cn".to_owned()]);
        assert_eq!(classify("a@Corp.cn", &extra), Region::Domestic);
    }

    #[test]
    fn malformed_addresses_are_international() {
        let none = HashSet::new();
        for email in ["", "qq.com", "@qq.com", "a@"] {
            assert_eq!(classify(email, &none), Region::International, "{email}");
        }
    }
}
