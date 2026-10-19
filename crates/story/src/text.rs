//! 文本归一化与片段截取。
//!
//! 检索在「折叠」后的字符上进行：全角 ASCII → 半角、ASCII 大写 → 小写、各种间隔号 → `·`、
//! 全角空格 → 半角空格。折叠是逐字符一对一的映射，所以折叠前后字符下标一致，
//! 命中位置可以直接换算回原文截取片段。

/// 单字符折叠。保证一对一（不会把一个字符变成多个）。
#[inline]
pub fn fold_char(c: char) -> char {
    match c {
        'A'..='Z' => c.to_ascii_lowercase(),
        '\u{FF01}'..='\u{FF5E}' => {
            let ascii = char::from_u32(c as u32 - 0xFEE0).unwrap_or(c);
            ascii.to_ascii_lowercase()
        }
        '\u{3000}' => ' ',
        // 「零号·安比」「简·杜」「新·艾利都」：模型常写成日文中点或项目符号。
        '\u{30FB}' | '\u{FF65}' | '\u{2022}' | '\u{2027}' | '\u{22C5}' => '·',
        _ if !c.is_ascii() && c.is_uppercase() => {
            let mut lower = c.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(l), None) => l,
                _ => c,
            }
        }
        _ => c,
    }
}

/// 折叠整串，写进复用的缓冲区。
pub fn fold_into(src: &str, out: &mut String) {
    out.clear();
    out.reserve(src.len());
    out.extend(src.chars().map(fold_char));
}

pub fn fold(src: &str) -> String {
    let mut out = String::new();
    fold_into(src, &mut out);
    out
}

/// 把查询串切成检索词：按空白切分、折叠、去重，丢掉空词。
pub fn query_terms(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for raw in query.split(|c: char| c.is_whitespace() || c == '\u{3000}') {
        let term =
            fold(raw.trim_matches(|c: char| matches!(c, '"' | '“' | '”' | '「' | '」' | '\'')));
        let term = if term.is_empty() { fold(raw) } else { term };
        if !term.is_empty() && !terms.contains(&term) {
            terms.push(term);
        }
    }
    terms
}

/// 在 `haystack`（原文）里按折叠规则找 `needle_folded` 的第一个命中，返回字符下标。
pub fn find_folded(haystack: &str, needle_folded: &str, scratch: &mut String) -> Option<usize> {
    fold_into(haystack, scratch);
    let byte = memchr::memmem::find(scratch.as_bytes(), needle_folded.as_bytes())?;
    Some(scratch[..byte].chars().count())
}

/// 截取命中附近的片段：短行原样返回，长行以命中为中心截 `width` 个字符。
pub fn snippet(text: &str, hit_char: Option<usize>, width: usize) -> String {
    let total = text.chars().count();
    if total <= width {
        return text.to_owned();
    }
    let center = hit_char.unwrap_or(0);
    let start = center.saturating_sub(width / 3).min(total - width);
    let end = start + width;
    let mut out = String::with_capacity(width * 3 + 8);
    if start > 0 {
        out.push('…');
    }
    out.extend(text.chars().skip(start).take(width));
    if end < total {
        out.push('…');
    }
    out
}

/// 按字符数截断（用于标题、摘要）。
pub fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((byte, _)) => format!("{}…", &text[..byte]),
        None => text.to_owned(),
    }
}

/// 千分位格式化，给模型看的计数更易读。
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// 字数的口语化表示：12345 → "1.2万字"。
pub fn char_amount(n: usize) -> String {
    if n >= 10_000 {
        format!("{:.1}万字", n as f64 / 10_000.0)
    } else {
        format!("{n}字")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_is_one_to_one() {
        let src = "ＡＢＣ　Fairy・零号•安比İ";
        let folded = fold(src);
        assert_eq!(folded.chars().count(), src.chars().count());
        assert_eq!(folded, "abc fairy·零号·安比İ");
    }

    #[test]
    fn query_terms_split_and_dedupe() {
        assert_eq!(
            query_terms("  狡兔屋  妮可 狡兔屋 「以骸」 "),
            vec!["狡兔屋", "妮可", "以骸"]
        );
        assert_eq!(query_terms("Fairy ＦＡＩＲＹ"), vec!["fairy"]);
        assert!(query_terms("   ").is_empty());
    }

    #[test]
    fn find_folded_returns_char_index() {
        let mut scratch = String::new();
        assert_eq!(
            find_folded("你好，FAIRY在吗", "fairy", &mut scratch),
            Some(3)
        );
        assert_eq!(find_folded("你好", "fairy", &mut scratch), None);
    }

    #[test]
    fn snippet_centers_on_hit() {
        let text: String = "一二三四五六七八九十".repeat(5);
        let s = snippet(&text, Some(25), 10);
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert_eq!(s.chars().count(), 12);
        assert_eq!(snippet("短句", Some(0), 10), "短句");
    }

    #[test]
    fn number_formatting() {
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(12), "12");
        assert_eq!(char_amount(12345), "1.2万字");
        assert_eq!(char_amount(999), "999字");
        assert_eq!(truncate_chars("一二三四", 2), "一二…");
    }
}
