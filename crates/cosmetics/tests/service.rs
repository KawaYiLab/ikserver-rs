//! 装扮：目录导入、列表、装备，以及装备头像在作者展示里的优先级。

// clippy.toml 的测试豁免只覆盖 #[test] 函数本身，不覆盖集成测试里的辅助函数。
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use ik_auth::Principal;
use ik_cosmetics::CosmeticsState;
use ik_cosmetics::dto::{
    CardQuery, CatalogItem, CosmeticKind, EquipAvatarRequest, EquipCardRequest,
};
use ik_cosmetics::service;
use ik_kernel::{Id, Role, Snowflake};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

struct Harness {
    state: CosmeticsState,
    db: PgPool,
}

fn harness(db: PgPool) -> Harness {
    let ids = Arc::new(Snowflake::new(1).unwrap());
    let media = ik_media::MediaState::for_tests(db.clone(), ids.clone());
    Harness {
        state: CosmeticsState::new(db.clone(), ids, media),
        db,
    }
}

impl Harness {
    async fn user(&self, id: i64) -> Principal {
        sqlx::query!(
            "INSERT INTO users (id, username, email, role, level) VALUES ($1, $2, $3, 'user', 0)",
            id,
            format!("user{id}"),
            format!("user{id}@example.com"),
        )
        .execute(&self.db)
        .await
        .unwrap();
        Principal {
            user_id: Id::new(id),
            session_id: Id::new(id),
            role: Role::User,
            // 装备不要求通过考试（COMMON-006）。
            exam_passed: false,
            is_ai_agent: false,
            level: 0,
        }
    }

    async fn media(&self, id: i64) {
        sqlx::query!(
            "INSERT INTO media_files (id, object_key, mime, size_bytes, sha256, receipt)
             VALUES ($1, $2, 'image/png', 100, $3, $4)",
            id,
            format!("cos/{id}.png"),
            "e".repeat(64),
            format!("r{id}"),
        )
        .execute(&self.db)
        .await
        .unwrap();
    }

    /// 作者展示用的头像（经 auth，各域都走这里）。
    async fn shown_avatar(&self, user_id: i64) -> Option<i64> {
        ik_auth::service::authors(&self.db, &[user_id])
            .await
            .unwrap()
            .remove(&user_id)
            .unwrap()
            .avatar_media_id
    }

    async fn catalog(&self) -> (i64, i64) {
        for id in 1..=6 {
            self.media(id).await;
        }
        let items: Vec<CatalogItem> = serde_json::from_value(serde_json::json!([
            { "type": "avatar", "name": "新闻头像", "kind": "news", "imageId": "1" },
            { "type": "avatar", "name": "角色头像", "imageId": "2" },
            { "type": "card", "name": "城市名片", "kind": "city", "description": "城", "story": "# 故事", "imageId": "3" },
            { "type": "card", "name": "角色名片一", "kind": "character", "imageId": "4" },
            { "type": "card", "name": "角色名片二", "kind": "character", "imageId": "5" },
        ]))
        .unwrap();
        let report = service::import(&self.state, items).await.unwrap();
        assert_eq!((report.avatars, report.cards), (2, 3));
        let list = service::my_avatars(&self.state, &self.user(99).await)
            .await
            .unwrap();
        (list.items[0].id.get(), list.items[1].id.get())
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn equipped_avatar_wins_over_custom_until_a_new_custom_one_is_set(db: PgPool) {
    let h = harness(db);
    let (character, news) = h.catalog().await;
    let me = h.user(1).await;

    // 按分类排序：character 在 news 前面。
    let list = service::my_avatars(&h.state, &me).await.unwrap();
    let names: Vec<&str> = list.items.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["角色头像", "新闻头像"]);
    assert_eq!(list.items[0].kind, CosmeticKind::Character);
    assert!(list.equipped_avatar_id.is_none());
    assert_eq!(
        list.items[0].image.thumb_url,
        "https://cdn.test/cos/2.png?image_process=resize,w_360/format,webp/quality,q_80"
    );

    // 有自定义头像时，装备的系统头像优先。
    h.media(10).await;
    let mut tx = h.db.begin().await.unwrap();
    ik_auth::interop::set_avatar(&mut tx, 1, Some(10))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(h.shown_avatar(1).await, Some(10));
    let equipped = service::equip_avatar(
        &h.state,
        &me,
        EquipAvatarRequest {
            avatar_id: Some(Id::new(news)),
        },
    )
    .await
    .unwrap();
    assert_eq!(equipped.equipped_avatar.unwrap().name, "新闻头像");
    assert_eq!(h.shown_avatar(1).await, Some(1));
    assert_eq!(
        service::my_avatars(&h.state, &me)
            .await
            .unwrap()
            .equipped_avatar_id
            .map(|i| i.get()),
        Some(news)
    );

    // 卸下后回到自定义头像。
    service::equip_avatar(&h.state, &me, EquipAvatarRequest { avatar_id: None })
        .await
        .unwrap();
    assert_eq!(h.shown_avatar(1).await, Some(10));

    // 设置自定义头像会卸下系统头像（COSMETIC-006）。
    service::equip_avatar(
        &h.state,
        &me,
        EquipAvatarRequest {
            avatar_id: Some(Id::new(character)),
        },
    )
    .await
    .unwrap();
    assert_eq!(h.shown_avatar(1).await, Some(2));
    let mut tx = h.db.begin().await.unwrap();
    ik_auth::interop::set_avatar(&mut tx, 1, Some(10))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(h.shown_avatar(1).await, Some(10));
    assert!(
        service::my_avatars(&h.state, &me)
            .await
            .unwrap()
            .equipped_avatar_id
            .is_none()
    );

    // 清除自定义头像不影响装备。
    service::equip_avatar(
        &h.state,
        &me,
        EquipAvatarRequest {
            avatar_id: Some(Id::new(character)),
        },
    )
    .await
    .unwrap();
    let mut tx = h.db.begin().await.unwrap();
    ik_auth::interop::set_avatar(&mut tx, 1, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(h.shown_avatar(1).await, Some(2));

    let err = service::equip_avatar(
        &h.state,
        &me,
        EquipAvatarRequest {
            avatar_id: Some(Id::new(424242)),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "NOT_FOUND");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn changing_a_catalog_image_updates_everyone_wearing_it(db: PgPool) {
    let h = harness(db);
    let (character, _) = h.catalog().await;
    let me = h.user(1).await;
    h.user(2).await;
    service::equip_avatar(
        &h.state,
        &me,
        EquipAvatarRequest {
            avatar_id: Some(Id::new(character)),
        },
    )
    .await
    .unwrap();

    let items: Vec<CatalogItem> = serde_json::from_value(serde_json::json!([
        { "type": "avatar", "name": "角色头像", "imageId": "6" },
    ]))
    .unwrap();
    let report = service::import(&h.state, items).await.unwrap();
    assert_eq!(report.refreshed_users, 1);
    assert_eq!(h.shown_avatar(1).await, Some(6));
    assert_eq!(h.shown_avatar(2).await, None);

    // 有一项不合格：整份不写入。
    let items: Vec<CatalogItem> = serde_json::from_value(serde_json::json!([
        { "type": "avatar", "name": "角色头像", "imageId": "5" },
        { "type": "card", "name": "", "imageId": "999" },
    ]))
    .unwrap();
    let err = service::import(&h.state, items).await.unwrap_err();
    let fields: Vec<&str> = err
        .field_errors()
        .iter()
        .map(|e| e.field.as_str())
        .collect();
    assert_eq!(fields, ["items[1].name", "items[1].imageId"]);
    assert_eq!(h.shown_avatar(1).await, Some(6));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cards_page_by_kind_and_equip(db: PgPool) {
    let h = harness(db);
    h.catalog().await;
    let me = h.user(1).await;
    let query = |kind, cursor| CardQuery {
        kind,
        cursor,
        limit: Some(2),
    };

    let first = service::my_cards(&h.state, &me, query(None, None))
        .await
        .unwrap();
    let names: Vec<&str> = first.items.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["角色名片一", "角色名片二"]);
    assert!(first.equipped_card.is_none());
    let second = service::my_cards(&h.state, &me, query(None, first.next_cursor.clone()))
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].name, "城市名片");
    assert_eq!(second.items[0].story, "# 故事");
    assert_eq!(
        second.items[0].image_url,
        "https://cdn.test/cos/3.png?image_process=format,webp/quality,q_80"
    );
    assert!(second.next_cursor.is_none());

    let city = service::my_cards(&h.state, &me, query(Some(CosmeticKind::City), None))
        .await
        .unwrap();
    assert_eq!(city.items.len(), 1);
    let card_id = city.items[0].id;
    let equipped = service::equip_card(
        &h.state,
        &me,
        EquipCardRequest {
            card_id: Some(card_id),
        },
    )
    .await
    .unwrap();
    assert_eq!(equipped.equipped_card.unwrap().name, "城市名片");
    let page = service::my_cards(&h.state, &me, query(None, None))
        .await
        .unwrap();
    assert_eq!(page.equipped_card.unwrap().id, card_id);
    service::equip_card(&h.state, &me, EquipCardRequest { card_id: None })
        .await
        .unwrap();
    assert!(
        service::my_cards(&h.state, &me, query(None, None))
            .await
            .unwrap()
            .equipped_card
            .is_none()
    );

    let err = service::my_cards(&h.state, &me, query(None, Some("bad".into())))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "CURSOR_INVALID");

    // 封禁账号不能装备。
    sqlx::query!("UPDATE users SET status = 'blocked' WHERE id = 1")
        .execute(&h.db)
        .await
        .unwrap();
    let err = service::equip_card(
        &h.state,
        &me,
        EquipCardRequest {
            card_id: Some(card_id),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "ACCOUNT_BLOCKED");
}

fn operator(principal: &Principal) -> ik_auth::Operator {
    ik_auth::Operator {
        principal: principal.clone(),
        meta: ik_auth::service::RequestMeta {
            ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            user_agent: None,
        },
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn emote_manifest_follows_admin_changes(db: PgPool) {
    use ik_cosmetics::emotes::{self, EmoteGroupInput, EmoteInput};

    let h = harness(db);
    let admin = operator(&h.user(1).await);
    h.media(10).await;
    h.media(11).await;

    let empty = emotes::manifest(&h.state).await.unwrap();
    assert!(empty.emotes.is_empty() && empty.groups.is_empty());

    let group = emotes::save_group(
        &h.state,
        &admin,
        None,
        EmoteGroupInput {
            name: "绳网".into(),
            sort_order: 0,
            icon_id: None,
            active: true,
        },
    )
    .await
    .unwrap();
    let fields = |name: &str, image: i64| EmoteInput {
        name: name.into(),
        group_id: Some(group.id),
        image_id: Id::new(image),
        sort_order: 0,
        active: true,
    };
    let smile = emotes::save_emote(&h.state, &admin, None, Some("ik-smile"), fields("笑", 10))
        .await
        .unwrap();
    emotes::save_emote(&h.state, &admin, None, Some("ik-cry"), fields("哭", 11))
        .await
        .unwrap();

    // 不合法的代码、重复的代码、不存在的图片与分组。
    let code = |r: Result<emotes::AdminEmote, ik_kernel::Problem>| r.unwrap_err().code();
    assert_eq!(
        code(emotes::save_emote(&h.state, &admin, None, Some("smile"), fields("x", 10)).await),
        "VALIDATION_FAILED"
    );
    assert_eq!(
        code(emotes::save_emote(&h.state, &admin, None, Some("ik-smile"), fields("x", 10)).await),
        "ALREADY_EXISTS"
    );
    assert_eq!(
        code(emotes::save_emote(&h.state, &admin, None, Some("ik-x"), fields("x", 99)).await),
        "VALIDATION_FAILED"
    );
    let mut orphan = fields("x", 10);
    orphan.group_id = Some(Id::new(12345));
    assert_eq!(
        code(emotes::save_emote(&h.state, &admin, None, Some("ik-x"), orphan).await),
        "VALIDATION_FAILED"
    );

    let listed = emotes::manifest(&h.state).await.unwrap();
    let codes: Vec<&str> = listed.emotes.iter().map(|e| e.code.as_str()).collect();
    assert_eq!(codes, vec!["ik-smile", "ik-cry"]);
    assert!(
        listed
            .emotes
            .iter()
            .all(|e| e.selectable && e.group_id == Some(group.id))
    );
    assert_eq!(listed.groups.len(), 1);

    // 下架：仍在清单里（旧评论照常显示），只是不出现在面板；版本随之变化。
    let mut hidden = fields("笑", 10);
    hidden.active = false;
    emotes::save_emote(&h.state, &admin, Some(smile.id.get()), None, hidden)
        .await
        .unwrap();
    let after = emotes::manifest(&h.state).await.unwrap();
    assert_ne!(after.version, listed.version);
    let smile_view = after.emotes.iter().find(|e| e.code == "ik-smile").unwrap();
    assert!(!smile_view.selectable);

    // 删除分组：表情留下、变成未分组。
    emotes::delete_group(&h.state, &admin, group.id.get())
        .await
        .unwrap();
    let ungrouped = emotes::manifest(&h.state).await.unwrap();
    assert!(ungrouped.groups.is_empty());
    assert!(ungrouped.emotes.iter().all(|e| e.group_id.is_none()));
    let cry = ungrouped
        .emotes
        .iter()
        .find(|e| e.code == "ik-cry")
        .unwrap();
    assert!(cry.selectable);

    // 删除表情：从清单里消失。
    emotes::delete_emote(&h.state, &admin, smile.id.get())
        .await
        .unwrap();
    let left = emotes::manifest(&h.state).await.unwrap();
    assert_eq!(left.emotes.len(), 1);
    assert_eq!(
        emotes::delete_emote(&h.state, &admin, smile.id.get())
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
}
