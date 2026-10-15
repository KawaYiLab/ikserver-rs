//! 64 位雪花 ID 与带类型标记的 `Id<T>`。
//!
//! 位布局：1 位符号（恒 0）| 41 位毫秒时间戳（自 2026-01-01 UTC）| 10 位节点 | 12 位序号。
//! 迁移自旧系统的行保留原有小整数 id，与新生成的 ID 不会冲突（新 ID 远大于旧 id）。
//! 对外 JSON 一律序列化为字符串，避免 JavaScript 精度丢失。

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::postgres::{PgArgumentBuffer, PgHasArrayType, PgTypeInfo, PgValueRef};
use sqlx::{Decode, Encode, Postgres, Type};

/// 2026-01-01T00:00:00Z 的 Unix 毫秒数。
pub const EPOCH_MS: u64 = 1_767_225_600_000;
pub const MAX_NODE_ID: u16 = (1 << NODE_BITS) - 1;

const NODE_BITS: u32 = 10;
const SEQ_BITS: u32 = 12;
const MAX_SEQ: u64 = (1 << SEQ_BITS) - 1;

/// 雪花 ID 生成器。进程内共享一个实例（放进应用状态）。
#[derive(Debug)]
pub struct Snowflake {
    node: u64,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    last_ms: u64,
    seq: u64,
}

#[derive(Debug, thiserror::Error)]
#[error("雪花 ID 节点号必须在 0–{MAX_NODE_ID} 之间，实际为 {0}")]
pub struct InvalidNodeId(pub u16);

impl Snowflake {
    pub fn new(node: u16) -> Result<Self, InvalidNodeId> {
        if node > MAX_NODE_ID {
            return Err(InvalidNodeId(node));
        }
        Ok(Self {
            node: u64::from(node),
            state: Mutex::new(State { last_ms: 0, seq: 0 }),
        })
    }

    /// 生成一个新 ID。单调递增；系统时钟回拨时沿用上一毫秒继续递增序号，
    /// 同一毫秒内序号用尽则等到下一毫秒。
    pub fn next_raw(&self) -> i64 {
        let mut state = self.state.lock();
        loop {
            let now = now_ms();
            if now > state.last_ms {
                state.last_ms = now;
                state.seq = 0;
            } else if state.seq < MAX_SEQ {
                state.seq += 1;
            } else {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            return compose(state.last_ms, self.node, state.seq);
        }
    }

    pub fn next<T>(&self) -> Id<T> {
        Id::new(self.next_raw())
    }
}

fn now_ms() -> u64 {
    let since_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    u64::try_from(since_unix.as_millis()).unwrap_or(u64::MAX)
}

fn compose(unix_ms: u64, node: u64, seq: u64) -> i64 {
    let ts = unix_ms.saturating_sub(EPOCH_MS) & ((1 << 41) - 1);
    let raw = (ts << (NODE_BITS + SEQ_BITS)) | (node << SEQ_BITS) | seq;
    // 时间戳只占 41 位，左移 22 位后最高位恒为 0，结果一定落在 i64 正数范围内。
    i64::try_from(raw).unwrap_or(i64::MAX)
}

/// 实体类型标记，供 `Id<T>` 使用。放在 kernel 里，跨业务域引用 ID 时不必互相依赖。
pub mod marker {
    pub enum User {}
    pub enum Session {}
    pub enum EmailCode {}
    pub enum Category {}
    pub enum Tag {}
    pub enum Post {}
    pub enum Comment {}
    pub enum ExamQuestion {}
    pub enum ExamAttempt {}
    pub enum Notification {}
}

/// 从雪花 ID 反解出生成时间（Unix 毫秒）。旧系统迁移来的小整数 id 返回 None。
pub fn timestamp_ms(raw: i64) -> Option<u64> {
    let raw = u64::try_from(raw).ok()?;
    let ts = raw >> (NODE_BITS + SEQ_BITS);
    (ts > 0).then_some(ts + EPOCH_MS)
}

/// 带实体类型标记的 ID，例如 `Id<User>`、`Id<Post>`。不同实体的 ID 不能互相赋值。
pub struct Id<T = ()> {
    raw: i64,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Id<T> {
    pub const fn new(raw: i64) -> Self {
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    pub const fn get(self) -> i64 {
        self.raw
    }

    /// 转换为另一种实体标记（仅用于确实是同一个 ID 的场景，例如用户 ID 与其主页 ID）。
    pub const fn cast<U>(self) -> Id<U> {
        Id::new(self.raw)
    }
}

impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Id<T> {}

impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<T> Eq for Id<T> {}

impl<T> PartialOrd for Id<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Id<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.raw.cmp(&other.raw)
    }
}

impl<T> Hash for Id<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({})", self.raw)
    }
}

impl<T> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("ID 格式不正确")]
pub struct ParseIdError;

impl<T> FromStr for Id<T> {
    type Err = ParseIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() || s.len() > 19 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ParseIdError);
        }
        match s.parse::<i64>() {
            Ok(raw) if raw > 0 => Ok(Self::new(raw)),
            _ => Err(ParseIdError),
        }
    }
}

impl<T> Serialize for Id<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.raw)
    }
}

impl<'de, T> Deserialize<'de> for Id<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct IdVisitor<T>(PhantomData<fn() -> T>);

        impl<T> Visitor<'_> for IdVisitor<T> {
            type Value = Id<T>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("字符串形式的正整数 ID")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                v.parse().map_err(|_| E::custom("ID 格式不正确"))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                match i64::try_from(v) {
                    Ok(raw) if raw > 0 => Ok(Id::new(raw)),
                    _ => Err(E::custom("ID 超出范围")),
                }
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                if v > 0 {
                    Ok(Id::new(v))
                } else {
                    Err(E::custom("ID 必须为正数"))
                }
            }
        }

        deserializer.deserialize_any(IdVisitor(PhantomData))
    }
}

impl<T> Type<Postgres> for Id<T> {
    fn type_info() -> PgTypeInfo {
        <i64 as Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <i64 as Type<Postgres>>::compatible(ty)
    }
}

impl<T> PgHasArrayType for Id<T> {
    fn array_type_info() -> PgTypeInfo {
        <i64 as PgHasArrayType>::array_type_info()
    }

    fn array_compatible(ty: &PgTypeInfo) -> bool {
        <i64 as PgHasArrayType>::array_compatible(ty)
    }
}

impl<T> Encode<'_, Postgres> for Id<T> {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        <i64 as Encode<'_, Postgres>>::encode_by_ref(&self.raw, buf)
    }
}

impl<'r, T> Decode<'r, Postgres> for Id<T> {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(Self::new(<i64 as Decode<'r, Postgres>>::decode(value)?))
    }
}

impl<T> utoipa::PartialSchema for Id<T> {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .pattern(Some("^[1-9][0-9]{0,18}$"))
            .description(Some("64 位 ID，以字符串传输"))
            .examples(["7385159247626227712"])
            .into()
    }
}

impl<T> utoipa::ToSchema for Id<T> {
    fn name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("Id")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    struct User;

    #[test]
    fn ids_are_unique_and_increasing() {
        let ids_gen = Snowflake::new(7).unwrap();
        let ids: Vec<i64> = (0..10_000).map(|_| ids_gen.next_raw()).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        assert!(ids.iter().all(|&id| id > 0));
    }

    #[test]
    fn node_id_is_bounded() {
        assert!(Snowflake::new(MAX_NODE_ID).is_ok());
        assert!(Snowflake::new(MAX_NODE_ID + 1).is_err());
    }

    #[test]
    fn timestamp_roundtrip() {
        let ids_gen = Snowflake::new(1).unwrap();
        let before = now_ms();
        let ts = timestamp_ms(ids_gen.next_raw()).unwrap();
        assert!(ts >= before && ts <= now_ms());
        assert_eq!(timestamp_ms(42), None);
    }

    #[test]
    fn serializes_as_string_and_accepts_both_forms() {
        let id: Id<User> = Id::new(7_385_159_247_626_227_712);
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            "\"7385159247626227712\""
        );
        let from_str: Id<User> = serde_json::from_str("\"7385159247626227712\"").unwrap();
        let from_num: Id<User> = serde_json::from_str("42").unwrap();
        assert_eq!(from_str, id);
        assert_eq!(from_num.get(), 42);
    }

    #[test]
    fn rejects_bad_ids() {
        for bad in [
            "\"\"",
            "\"0\"",
            "\"-1\"",
            "\"12a\"",
            "\"99999999999999999999\"",
            "-5",
        ] {
            assert!(serde_json::from_str::<Id<User>>(bad).is_err(), "{bad}");
        }
    }
}
