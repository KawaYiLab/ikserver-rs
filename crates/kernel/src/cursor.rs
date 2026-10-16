//! 不透明分页游标：服务端结构体 → JSON → base64url（无填充）。
//!
//! 客户端只能原样回传，不得解析。非法游标一律映射为 400 `CURSOR_INVALID`。

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Serialize;
use serde::de::DeserializeOwned;
use utoipa::ToSchema;

use crate::Problem;

/// 防止超长游标被当作放大攻击的输入。
const MAX_CURSOR_LEN: usize = 512;

pub fn encode<T: Serialize>(value: &T) -> Result<String, Problem> {
    let json = serde_json::to_vec(value).map_err(|e| Problem::internal_from(&e))?;
    Ok(URL_SAFE_NO_PAD.encode(json))
}

pub fn decode<T: DeserializeOwned>(cursor: &str) -> Result<T, Problem> {
    if cursor.is_empty() || cursor.len() > MAX_CURSOR_LEN {
        return Err(Problem::cursor_invalid());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| Problem::cursor_invalid())?;
    serde_json::from_slice(&bytes).map_err(|_| Problem::cursor_invalid())
}

/// 游标分页的统一响应形状。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    /// 按 "多取 1 条" 的约定组装：`rows` 最多 `limit + 1` 条，多出来的那条只用来判断有无下一页。
    pub fn from_rows<K: Serialize>(
        mut rows: Vec<T>,
        limit: usize,
        key_of: impl Fn(&T) -> K,
    ) -> Result<Self, Problem> {
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        let next_cursor = match (has_more, rows.last()) {
            (true, Some(last)) => Some(encode(&key_of(last))?),
            _ => None,
        };
        Ok(Self {
            items: rows,
            next_cursor,
        })
    }
}

/// 管理后台表格的页码分页（api-guidelines.md「管理后台」：允许 `page`，limit 上限 100）。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Paged<T> {
    pub items: Vec<T>,
    /// 符合条件的总数。
    pub total: i64,
    /// 从 1 开始。
    pub page: u32,
    pub limit: u32,
}

/// 页码与每页条数：page 从 1 开始，limit 默认 `default`、范围 1–100。返回 (page, limit, offset)。
pub fn page_window(page: Option<u32>, limit: Option<u32>, default: u32) -> (u32, u32, i64) {
    let page = page.unwrap_or(1).max(1);
    let limit = limit.unwrap_or(default).clamp(1, 100);
    let offset = i64::from(page - 1) * i64::from(limit);
    (page, limit, offset)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Key {
        at: i64,
        id: i64,
    }

    #[test]
    fn roundtrip() {
        let key = Key {
            at: 1_700_000_000,
            id: 42,
        };
        let cursor = encode(&key).unwrap();
        assert!(!cursor.contains('='));
        assert_eq!(decode::<Key>(&cursor).unwrap(), key);
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["", "!!!", "e30", &"a".repeat(MAX_CURSOR_LEN + 1)] {
            let err = decode::<Key>(bad).unwrap_err();
            assert_eq!(err.code(), "CURSOR_INVALID", "{bad}");
        }
    }

    #[test]
    fn page_from_rows_sets_cursor_only_when_more() {
        let page = Page::from_rows(vec![1, 2, 3], 2, |v| *v).unwrap();
        assert_eq!(page.items, vec![1, 2]);
        assert_eq!(
            decode::<i32>(page.next_cursor.as_deref().unwrap()).unwrap(),
            2
        );

        let last = Page::from_rows(vec![1, 2], 2, |v| *v).unwrap();
        assert!(last.next_cursor.is_none());
    }
}
