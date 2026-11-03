//! 检索别称：英文名与玩家常用外号 → 语料里的叫法（数据在 `aliases.json`）。

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;

const DATA: &str = include_str!("../../aliases.json");

#[derive(Deserialize)]
struct File {
    english: HashMap<String, String>,
    nickname: HashMap<String, String>,
}

/// 别称的来历：英文名还是玩家外号，提示里分开说。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    English,
    Nickname,
}

static TABLE: LazyLock<HashMap<String, (String, Kind)>> = LazyLock::new(|| {
    let file: File = serde_json::from_str(DATA).unwrap_or(File {
        english: HashMap::new(),
        nickname: HashMap::new(),
    });
    let mut table = HashMap::new();
    for (map, kind) in [
        (file.english, Kind::English),
        (file.nickname, Kind::Nickname),
    ] {
        for (from, to) in map {
            table.insert(crate::text::fold(&from), (to, kind));
        }
    }
    table
});

/// 已折叠的叫法 → 语料里的叫法。
pub fn lookup(folded: &str) -> Option<(&'static str, Kind)> {
    TABLE.get(folded).map(|(to, kind)| (to.as_str(), *kind))
}

/// 参与语料版本摘要：别称表一改，旧游标随之失效。
pub fn revision_data() -> &'static str {
    DATA
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_parses_and_folds_case() {
        assert!(!TABLE.is_empty(), "aliases.json 解析失败");
        assert_eq!(lookup("miyabi"), Some(("星见雅", Kind::English)));
        assert_eq!(lookup("ellen joe"), Some(("艾莲", Kind::English)));
        assert_eq!(lookup("鲨鱼妹"), Some(("艾莲", Kind::Nickname)));
        assert_eq!(lookup("不存在"), None);
    }
}
