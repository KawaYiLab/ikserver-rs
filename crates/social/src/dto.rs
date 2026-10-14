//! 对外的请求与响应类型。

use chrono::{DateTime, Utc};
use ik_kernel::Id;
use ik_kernel::id::marker::User;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FollowResult {
    pub following: bool,
    /// 对方当前的粉丝数。
    pub follower_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlockResult {
    pub blocked: bool,
}

/// 关注列表、拉黑列表里的一个用户。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserCard {
    #[schema(value_type = String)]
    pub id: Id<User>,
    pub username: String,
    pub level: i16,
    /// 关注 / 拉黑的时间。
    pub since: DateTime<Utc>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    pub cursor: Option<String>,
    /// 默认 20，最大 50。
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct RelationsQuery {
    /// 逗号分隔的用户 id；去重后最多取前 100 个。
    pub user_ids: String,
}

/// 我与某个用户的关系。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RelationView {
    #[schema(value_type = String)]
    pub user_id: Id<User>,
    pub following: bool,
    /// 只反映「我拉黑了对方」，不暴露「对方拉黑了我」（D-053）。
    pub blocking: bool,
}
