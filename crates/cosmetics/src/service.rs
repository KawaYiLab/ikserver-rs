use std::collections::HashMap;

use http::StatusCode;
use ik_auth::Principal;
use ik_kernel::cursor::{self, Page};
use ik_kernel::{FieldError, Id, Problem};
use serde::{Deserialize, Serialize};

use crate::CosmeticsState;
use crate::dto::{
    AvatarList, AvatarView, CardPage, CardQuery, CardView, CatalogItem, CosmeticKind,
    EquipAvatarRequest, EquipCardRequest, EquippedAvatar, EquippedCard,
};
use crate::repo::{self, AvatarRow, CardRow, NewCard};

const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 50;
const NAME_MAX_CHARS: usize = 50;
const DESCRIPTION_MAX_CHARS: usize = 500;
const STORY_MAX_CHARS: usize = 20_000;

fn avatar_not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "NOT_FOUND", "头像不存在")
}

fn card_not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "NOT_FOUND", "名片不存在")
}

async fn avatar_views(
    state: &CosmeticsState,
    rows: Vec<AvatarRow>,
) -> Result<Vec<AvatarView>, Problem> {
    let media_ids: Vec<i64> = rows.iter().map(|r| r.image_media_id).collect();
    let media = state.media.views(&media_ids).await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            // 媒体文件从不物理删除；取不到说明数据异常，跳过这一项而不是整页失败。
            let image = media.get(&row.image_media_id).cloned()?;
            Some(AvatarView {
                id: Id::new(row.id),
                name: row.name,
                kind: CosmeticKind::parse(&row.kind),
                image,
            })
        })
        .collect())
}

async fn card_views(state: &CosmeticsState, rows: Vec<CardRow>) -> Result<Vec<CardView>, Problem> {
    let media_ids: Vec<i64> = rows.iter().map(|r| r.image_media_id).collect();
    let media = state.media.views(&media_ids).await?;
    let urls = state.media.webp_urls(&media_ids).await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let image = media.get(&row.image_media_id).cloned()?;
            let image_url = urls.get(&row.image_media_id).cloned()?;
            Some(CardView {
                id: Id::new(row.id),
                name: row.name,
                kind: CosmeticKind::parse(&row.kind),
                description: row.description,
                story: row.story,
                image,
                image_url,
            })
        })
        .collect())
}

/// 按 id 批量取系统头像（供作者主页展示装备）。不存在的 id 不出现。
pub async fn avatars_by_ids(
    state: &CosmeticsState,
    ids: &[i64],
) -> Result<HashMap<i64, AvatarView>, Problem> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut conn = state.db.acquire().await?;
    let rows = repo::avatars_by_ids(&mut conn, ids).await?;
    drop(conn);
    Ok(avatar_views(state, rows)
        .await?
        .into_iter()
        .map(|v| (v.id.get(), v))
        .collect())
}

/// 按 id 批量取论坛名片（供作者主页展示装备）。不存在的 id 不出现。
pub async fn cards_by_ids(
    state: &CosmeticsState,
    ids: &[i64],
) -> Result<HashMap<i64, CardView>, Problem> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut conn = state.db.acquire().await?;
    let rows = repo::cards_by_ids(&mut conn, ids).await?;
    drop(conn);
    Ok(card_views(state, rows)
        .await?
        .into_iter()
        .map(|v| (v.id.get(), v))
        .collect())
}

async fn one_avatar(
    state: &CosmeticsState,
    id: Option<i64>,
) -> Result<Option<AvatarView>, Problem> {
    let Some(id) = id else { return Ok(None) };
    Ok(avatars_by_ids(state, &[id]).await?.remove(&id))
}

async fn one_card(state: &CosmeticsState, id: Option<i64>) -> Result<Option<CardView>, Problem> {
    let Some(id) = id else { return Ok(None) };
    Ok(cards_by_ids(state, &[id]).await?.remove(&id))
}

async fn equipped_ids(
    state: &CosmeticsState,
    user_id: i64,
) -> Result<(Option<i64>, Option<i64>), Problem> {
    let mut conn = state.db.acquire().await?;
    let user = ik_auth::interop::profile_user(&mut conn, user_id)
        .await?
        .ok_or_else(Problem::not_found)?;
    Ok((user.equipped_avatar_id, user.equipped_card_id))
}

/// 全部系统头像与自己装备的那个（COSMETIC-004）。
pub async fn my_avatars(
    state: &CosmeticsState,
    principal: &Principal,
) -> Result<AvatarList, Problem> {
    let (equipped, _) = equipped_ids(state, principal.user_id.get()).await?;
    let mut conn = state.db.acquire().await?;
    let rows = repo::all_avatars(&mut conn).await?;
    drop(conn);
    Ok(AvatarList {
        items: avatar_views(state, rows).await?,
        equipped_avatar_id: equipped.map(Id::new),
    })
}

/// 装备 / 卸下系统头像（COSMETIC-003）。不扣费，不要求通过考试。
pub async fn equip_avatar(
    state: &CosmeticsState,
    principal: &Principal,
    request: EquipAvatarRequest,
) -> Result<EquippedAvatar, Problem> {
    let user_id = principal.user_id.get();
    let mut tx = state.db.begin().await?;
    let target = match request.avatar_id {
        Some(id) => {
            let row = repo::avatars_by_ids(&mut tx, &[id.get()])
                .await?
                .into_iter()
                .next()
                .ok_or_else(avatar_not_found)?;
            Some((row.id, row.image_media_id))
        }
        None => None,
    };
    ik_auth::interop::equip_avatar(&mut tx, user_id, target).await?;
    tx.commit().await?;
    Ok(EquippedAvatar {
        equipped_avatar: one_avatar(state, target.map(|(id, _)| id)).await?,
    })
}

#[derive(Serialize, Deserialize)]
struct CardCursor {
    k: String,
    id: i64,
}

/// 论坛名片（分页，可按分类过滤）与自己装备的那张（COSMETIC-004）。
pub async fn my_cards(
    state: &CosmeticsState,
    principal: &Principal,
    query: CardQuery,
) -> Result<CardPage, Problem> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let after: Option<CardCursor> = query.cursor.as_deref().map(cursor::decode).transpose()?;
    let (_, equipped) = equipped_ids(state, principal.user_id.get()).await?;
    let mut conn = state.db.acquire().await?;
    let rows = repo::cards_page(
        &mut conn,
        query.kind.map(CosmeticKind::as_str),
        after.as_ref().map(|c| (c.k.as_str(), c.id)),
        i64::from(limit) + 1,
    )
    .await?;
    drop(conn);
    let cursor_of = |row: &CardRow| CardCursor {
        k: row.kind.clone(),
        id: row.id,
    };
    // 先用原始行算下一页游标，再转视图（视图可能因数据异常少一项）。
    let page = Page::from_rows(rows, usize::try_from(limit).unwrap_or(20), cursor_of)?;
    Ok(CardPage {
        items: card_views(state, page.items).await?,
        next_cursor: page.next_cursor,
        equipped_card: one_card(state, equipped).await?,
    })
}

/// 装备 / 卸下论坛名片（COSMETIC-003）。不扣费，不要求通过考试。
pub async fn equip_card(
    state: &CosmeticsState,
    principal: &Principal,
    request: EquipCardRequest,
) -> Result<EquippedCard, Problem> {
    let user_id = principal.user_id.get();
    let mut tx = state.db.begin().await?;
    let target = match request.card_id {
        Some(id) => Some(
            repo::cards_by_ids(&mut tx, &[id.get()])
                .await?
                .into_iter()
                .next()
                .ok_or_else(card_not_found)?
                .id,
        ),
        None => None,
    };
    ik_auth::interop::equip_card(&mut tx, user_id, target).await?;
    tx.commit().await?;
    Ok(EquippedCard {
        equipped_card: one_card(state, target).await?,
    })
}

// ---------- 运维 ----------

/// 导入结果。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub avatars: usize,
    pub cards: usize,
    /// 因系统头像换图而同步的账号数。
    pub refreshed_users: u64,
}

fn check_text(field: &str, value: &str, min: usize, max: usize, errors: &mut Vec<FieldError>) {
    let n = value.chars().count();
    if n < min || n > max {
        errors.push(FieldError {
            field: field.to_owned(),
            code: "LENGTH".to_owned(),
            detail: format!("长度须在 {min}–{max} 个字符之间"),
        });
    }
}

/// 导入装扮目录（按名字新增或更新），整份一个事务：任何一项不合格都不写入。
/// 系统头像换图时，同步所有装备着它的账号（作者展示用的冗余图片 id）。
pub async fn import(
    state: &CosmeticsState,
    items: Vec<CatalogItem>,
) -> Result<ImportReport, Problem> {
    let mut errors = Vec::new();
    let mut media_ids = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let prefix = format!("items[{index}]");
        match item {
            CatalogItem::Avatar { name, image_id, .. } => {
                check_text(
                    &format!("{prefix}.name"),
                    name.trim(),
                    1,
                    NAME_MAX_CHARS,
                    &mut errors,
                );
                media_ids.push(image_id.get());
            }
            CatalogItem::Card {
                name,
                description,
                story,
                image_id,
                ..
            } => {
                check_text(
                    &format!("{prefix}.name"),
                    name.trim(),
                    1,
                    NAME_MAX_CHARS,
                    &mut errors,
                );
                check_text(
                    &format!("{prefix}.description"),
                    description,
                    0,
                    DESCRIPTION_MAX_CHARS,
                    &mut errors,
                );
                check_text(
                    &format!("{prefix}.story"),
                    story,
                    0,
                    STORY_MAX_CHARS,
                    &mut errors,
                );
                media_ids.push(image_id.get());
            }
        }
    }
    let existing = state.media.views(&media_ids).await?;
    for (index, id) in media_ids.iter().enumerate() {
        if !existing.contains_key(id) {
            errors.push(FieldError {
                field: format!("items[{index}].imageId"),
                code: "NOT_FOUND".to_owned(),
                detail: "图片不存在".to_owned(),
            });
        }
    }
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }

    let mut report = ImportReport::default();
    let mut tx = state.db.begin().await?;
    for item in items {
        match item {
            CatalogItem::Avatar {
                name,
                kind,
                image_id,
            } => {
                let (id, image_changed) = repo::upsert_avatar(
                    &mut tx,
                    state.ids.next_raw(),
                    name.trim(),
                    kind.as_str(),
                    image_id.get(),
                )
                .await?;
                if image_changed {
                    report.refreshed_users +=
                        ik_auth::interop::refresh_equipped_avatar(&mut tx, id, image_id.get())
                            .await?;
                }
                report.avatars += 1;
            }
            CatalogItem::Card {
                name,
                kind,
                description,
                story,
                image_id,
            } => {
                repo::upsert_card(
                    &mut tx,
                    &NewCard {
                        id: state.ids.next_raw(),
                        name: name.trim(),
                        kind: kind.as_str(),
                        description: &description,
                        story: &story,
                        image_media_id: image_id.get(),
                    },
                )
                .await?;
                report.cards += 1;
            }
        }
    }
    tx.commit().await?;
    Ok(report)
}
