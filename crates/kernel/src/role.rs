//! 用户角色。数据库里存为 TEXT（带 CHECK 约束），避免 PostgreSQL 枚举类型难以演进。

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueRef};
use sqlx::{Decode, Encode, Postgres, Type};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
    User,
    /// 版主：管理内容与举报。
    Moderator,
    /// 管理员：全部权限。
    Admin,
}

impl Role {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Moderator => "moderator",
            Self::Admin => "admin",
        }
    }

    /// 版主或管理员。
    pub const fn is_staff(self) -> bool {
        matches!(self, Self::Moderator | Self::Admin)
    }

    pub const fn is_admin(self) -> bool {
        matches!(self, Self::Admin)
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("未知角色：{0}")]
pub struct UnknownRole(String);

impl FromStr for Role {
    type Err = UnknownRole;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "moderator" => Ok(Self::Moderator),
            "admin" => Ok(Self::Admin),
            other => Err(UnknownRole(other.to_owned())),
        }
    }
}

impl Type<Postgres> for Role {
    fn type_info() -> PgTypeInfo {
        <&str as Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <&str as Type<Postgres>>::compatible(ty)
    }
}

impl Encode<'_, Postgres> for Role {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        <&str as Encode<'_, Postgres>>::encode_by_ref(&self.as_str(), buf)
    }
}

impl<'r> Decode<'r, Postgres> for Role {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <&str as Decode<'r, Postgres>>::decode(value)?;
        Ok(s.parse()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_roundtrip() {
        for role in [Role::User, Role::Moderator, Role::Admin] {
            assert_eq!(role.as_str().parse::<Role>().unwrap(), role);
            assert_eq!(serde_json::to_value(role).unwrap(), role.as_str());
        }
        assert!("root".parse::<Role>().is_err());
    }

    #[test]
    fn staff_flags() {
        assert!(!Role::User.is_staff());
        assert!(Role::Moderator.is_staff() && !Role::Moderator.is_admin());
        assert!(Role::Admin.is_staff() && Role::Admin.is_admin());
    }
}
