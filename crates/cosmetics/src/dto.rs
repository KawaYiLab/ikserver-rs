use ik_kernel::Id;
use ik_kernel::id::marker::{BusinessCard, SystemAvatar};
use ik_media::dto::MediaView;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// 装扮的分类（COSMETIC-001）。列表按分类、再按上架先后排序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CosmeticKind {
    /// 角色。
    #[default]
    Character,
    /// 城市。
    City,
    /// 新闻。
    News,
}

impl CosmeticKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::City => "city",
            Self::News => "news",
        }
    }

    pub(crate) fn parse(raw: &str) -> Self {
        match raw {
            "city" => Self::City,
            "news" => Self::News,
            _ => Self::Character,
        }
    }
}

/// 系统头像。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AvatarView {
    #[schema(value_type = String)]
    pub id: Id<SystemAvatar>,
    pub name: String,
    pub kind: CosmeticKind,
    /// 图片；展示用 `thumbUrl`（360 宽）。
    pub image: MediaView,
}

/// 论坛名片。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CardView {
    #[schema(value_type = String)]
    pub id: Id<BusinessCard>,
    pub name: String,
    pub kind: CosmeticKind,
    pub description: String,
    /// 背景故事（Markdown）。
    pub story: String,
    pub image: MediaView,
    /// 名片按原尺寸展示：WebP 原尺寸地址（没有 CDN 时为原图，COSMETIC-009）。
    pub image_url: String,
}

/// 全部系统头像（一次返回，数量很少）与自己装备的那个。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AvatarList {
    pub items: Vec<AvatarView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub equipped_avatar_id: Option<Id<SystemAvatar>>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct CardQuery {
    /// 只看某个分类。
    #[param(inline)]
    pub kind: Option<CosmeticKind>,
    pub cursor: Option<String>,
    /// 默认 20，最大 50。
    pub limit: Option<u32>,
}

/// 名片一页，附带自己装备的名片。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CardPage {
    pub items: Vec<CardView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipped_card: Option<CardView>,
}

/// 装备系统头像；`avatarId` 为 null 表示卸下（之后显示自定义头像或默认头像）。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EquipAvatarRequest {
    #[schema(value_type = Option<String>)]
    pub avatar_id: Option<Id<SystemAvatar>>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EquippedAvatar {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipped_avatar: Option<AvatarView>,
}

/// 装备论坛名片；`cardId` 为 null 表示卸下。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EquipCardRequest {
    #[schema(value_type = Option<String>)]
    pub card_id: Option<Id<BusinessCard>>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EquippedCard {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipped_card: Option<CardView>,
}

/// 运维导入的一项（CLI `cosmetics import`）。按名字新增或更新。
#[derive(Debug, Clone, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type",
    deny_unknown_fields
)]
pub enum CatalogItem {
    Avatar {
        name: String,
        #[serde(default)]
        kind: CosmeticKind,
        /// 已上传的媒体文件 id。
        image_id: Id<ik_kernel::id::marker::Media>,
    },
    Card {
        name: String,
        #[serde(default)]
        kind: CosmeticKind,
        #[serde(default)]
        description: String,
        #[serde(default)]
        story: String,
        image_id: Id<ik_kernel::id::marker::Media>,
    },
}
