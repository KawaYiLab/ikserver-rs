//! 标签名归一化与 slug 派生（POST-021/022）。纯函数。

use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

pub const MAX_TAGS_PER_POST: usize = 5;
const MAX_NAME_UTF16: usize = 30;
const MAX_SLUG_CHARS: usize = 60;
/// 单个原始输入的长度上限，挡住超长垃圾输入。
pub const MAX_RAW_TAG_LEN: usize = 256;

struct Patterns {
    invisible: Regex,
    name_disallowed: Regex,
    whitespace: Regex,
    has_alnum: Regex,
    slug_separators: Regex,
    slug_disallowed: Regex,
    dashes: Regex,
}

// 正则都是字面量，构造失败只可能是代码错误；单元测试保证它们能编译。
static PATTERNS: LazyLock<Option<Patterns>> = LazyLock::new(|| {
    Some(Patterns {
        // 制表符、换行等空白控制字符留给后面的空白折叠处理成空格，而不是直接删掉。
        invisible: Regex::new(r"[[\p{Default_Ignorable_Code_Point}\p{Cc}]&&[^\s]]").ok()?,
        name_disallowed: Regex::new(r"[^\p{L}\p{M}\p{N}\s_+#.\-]").ok()?,
        whitespace: Regex::new(r"\s+").ok()?,
        has_alnum: Regex::new(r"[\p{L}\p{N}]").ok()?,
        slug_separators: Regex::new(r"[\s_.]+").ok()?,
        slug_disallowed: Regex::new(r"[^\p{L}\p{M}\p{N}+#\-]").ok()?,
        dashes: Regex::new(r"-+").ok()?,
    })
});

fn patterns() -> Option<&'static Patterns> {
    PATTERNS.as_ref()
}

/// 截到最多 `max` 个 UTF-16 码元，不切断代理对（与旧系统按 JS 字符串长度计一致）。
fn truncate_utf16(s: &str, max: usize) -> &str {
    let mut units = 0;
    for (index, ch) in s.char_indices() {
        units += ch.len_utf16();
        if units > max {
            return &s[..index];
        }
    }
    s
}

/// 归一化标签名。结果不含字母或数字时返回 `None`（视为无效）。
pub fn normalize_name(raw: &str) -> Option<String> {
    let p = patterns()?;
    let nfkc: String = raw.nfkc().collect();
    let visible = p.invisible.replace_all(&nfkc, "");
    let allowed = p.name_disallowed.replace_all(&visible, "");
    let collapsed = p.whitespace.replace_all(&allowed, " ");
    let name = truncate_utf16(&collapsed, MAX_NAME_UTF16).trim();
    p.has_alnum.is_match(name).then(|| name.to_owned())
}

/// 由归一化后的名称派生 slug。刻意保留 `+` 与 `#`：c / c++ / c# 互不相同。
pub fn slug(name: &str) -> Option<String> {
    let p = patterns()?;
    let lowered: String = name.to_lowercase().nfkc().collect();
    let dashed = p.slug_separators.replace_all(&lowered, "-");
    let allowed = p.slug_disallowed.replace_all(&dashed, "");
    let collapsed = p.dashes.replace_all(&allowed, "-");
    let slug: String = collapsed
        .trim_matches('-')
        .chars()
        .take(MAX_SLUG_CHARS)
        .collect();
    let slug = slug.trim_end_matches('-').to_owned();
    (!slug.is_empty()).then_some(slug)
}

/// 联想查询词归一化：与 slug 同规则，但允许为空（空表示不过滤）。
pub fn query_slug(q: &str) -> String {
    normalize_name(q)
        .and_then(|name| slug(&name))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_compile() {
        assert!(patterns().is_some());
    }

    #[test]
    fn normalizes_width_invisible_and_symbols() {
        assert_eq!(normalize_name("  ＡＢＣ  ").as_deref(), Some("ABC"));
        assert_eq!(normalize_name("a\u{200B}b").as_deref(), Some("ab"));
        assert_eq!(normalize_name("原神🎮 攻略").as_deref(), Some("原神 攻略"));
        assert_eq!(
            normalize_name("多   空格\t标签").as_deref(),
            Some("多 空格 标签")
        );
        assert_eq!(normalize_name("!!!"), None);
        assert_eq!(normalize_name("___"), None);
        assert_eq!(normalize_name(""), None);
    }

    #[test]
    fn truncates_to_30_utf16_units_without_splitting_surrogates() {
        let long = "字".repeat(40);
        assert_eq!(normalize_name(&long).unwrap().chars().count(), 30);
        // 𝐀 (U+1D400) 经 NFKC 变成 A，换一个 NFKC 不变的增补字符：𠀀 (U+20000)。
        let astral = format!("{}{}", "a".repeat(29), "\u{20000}");
        assert_eq!(normalize_name(&astral).unwrap(), "a".repeat(29));
    }

    #[test]
    fn slug_keeps_plus_and_hash() {
        let c = slug("C").unwrap();
        let cpp = slug("C++").unwrap();
        let cs = slug("C#").unwrap();
        assert_eq!((c.as_str(), cpp.as_str(), cs.as_str()), ("c", "c++", "c#"));
    }

    #[test]
    fn slug_folds_separators() {
        assert_eq!(
            slug("Rust_Lang.Tips  2").as_deref(),
            Some("rust-lang-tips-2")
        );
        assert_eq!(slug("--a--b--").as_deref(), Some("a-b"));
        assert_eq!(slug("绝区零").as_deref(), Some("绝区零"));
    }

    #[test]
    fn query_slug_is_lenient() {
        assert_eq!(query_slug(" C++ "), "c++");
        assert_eq!(query_slug("!!"), "");
    }
}
