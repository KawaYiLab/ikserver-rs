//! 题库、装扮目录与装备、米游社绑定。
//!
//! 考试场次不导入：旧快照的格式与新快照不同，导入后只服务于回顾与冷却；通过状态已经在
//! `users.exam_passed_at` 上，切换时还没通过的人重新开考即可。

use sqlx::{PgConnection, Postgres, Transaction};

use crate::import::{ImportOptions, ImportReport};

pub async fn run(
    tx: &mut Transaction<'_, Postgres>,
    options: &ImportOptions,
    report: &mut ImportReport,
) -> anyhow::Result<()> {
    report.add("exam_questions", questions(tx, options).await?);
    report.add("system_avatars", avatars(tx, options).await?);
    report.add("business_cards", cards(tx, options).await?);
    report.add("users.equipped", equipped(tx).await?);
    report.add("mihoyo_bindings", mihoyo(tx, options).await?);
    Ok(())
}

async fn questions(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO exam_questions (id, question, kind, options, correct_keys, explanation, weight,
                                       enabled, tag, legacy_id, created_at, updated_at)
           SELECT q.id, btrim(q.question), q.kind, q.new_options, q.new_correct_keys,
                  nullif(btrim(q.explanation), ''), greatest(coalesce(q.weight, 1), 1),
                  q.enabled IS DISTINCT FROM false, nullif(btrim(q.tag), ''), q.document_id,
                  coalesce(q.created_at AT TIME ZONE $1, $2),
                  coalesce(q.updated_at AT TIME ZONE $1, q.created_at AT TIME ZONE $1, $2)
           FROM legacy.ik_exam_questions q
           WHERE char_length(btrim(coalesce(q.question, ''))) BETWEEN 1 AND 2000
             AND jsonb_array_length(q.new_options) BETWEEN 2 AND 26
             AND cardinality(q.new_correct_keys) >= 1"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 系统头像：没有可用图片的不导入；名字截到 50 字，重名的只留 id 最小的。
async fn avatars(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO system_avatars (id, name, kind, image_media_id, created_at, updated_at)
           SELECT DISTINCT ON (left(btrim(a.name), 50))
                  a.id, left(btrim(a.name), 50),
                  CASE WHEN a.type IN ('character', 'city', 'news') THEN a.type ELSE 'character' END,
                  i.file_id,
                  coalesce(a.created_at AT TIME ZONE $1, $2),
                  coalesce(a.updated_at AT TIME ZONE $1, a.created_at AT TIME ZONE $1, $2)
           FROM legacy.avatars a
           JOIN legacy.ik_cosmetic_images i
             ON i.related_type = 'api::avatar.avatar' AND i.related_id = a.id
           JOIN media_files m ON m.id = i.file_id
           WHERE btrim(coalesce(a.name, '')) <> ''
           ORDER BY left(btrim(a.name), 50), a.id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 论坛名片：背景故事从 blocks 富文本取文字（段落之间空一行）。
async fn cards(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO business_cards (id, name, kind, description, story, image_media_id, created_at,
                                       updated_at)
           SELECT DISTINCT ON (left(btrim(c.name), 50))
                  c.id, left(btrim(c.name), 50),
                  CASE WHEN c.type IN ('character', 'city', 'news') THEN c.type ELSE 'character' END,
                  left(coalesce(c.description, ''), 500),
                  left(legacy.ik_blocks_text(c.story, E'\n\n'), 20000),
                  i.file_id,
                  coalesce(c.created_at AT TIME ZONE $1, $2),
                  coalesce(c.updated_at AT TIME ZONE $1, c.created_at AT TIME ZONE $1, $2)
           FROM legacy.business_cards c
           JOIN legacy.ik_cosmetic_images i
             ON i.related_type = 'api::business-card.business-card' AND i.related_id = c.id
           JOIN media_files m ON m.id = i.file_id
           WHERE btrim(coalesce(c.name, '')) <> ''
           ORDER BY left(btrim(c.name), 50), c.id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 装备状态存在账号上：系统头像同时记下它的图片（users_equipped_avatar_pair）。
async fn equipped(conn: &mut PgConnection) -> anyhow::Result<u64> {
    let avatars = sqlx::query!(
        r#"UPDATE users u SET equipped_avatar_id = a.id, equipped_avatar_media_id = a.image_media_id
           FROM legacy.up_users_equipped_avatar_lnk l
           JOIN system_avatars a ON a.id = l.avatar_id
           WHERE u.id = l.user_id"#
    )
    .execute(&mut *conn)
    .await?;
    let cards = sqlx::query!(
        r#"UPDATE users u SET equipped_card_id = c.id
           FROM legacy.up_users_equipped_card_lnk l
           JOIN business_cards c ON c.id = l.business_card_id
           WHERE u.id = l.user_id"#
    )
    .execute(conn)
    .await?;
    Ok(avatars.rows_affected() + cards.rows_affected())
}

/// 米游社绑定：每个账号一条、每个 aid 一条（都取最早的）；注销、合并的账号不带绑定（DEL-006）。
async fn mihoyo(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO mihoyo_bindings (user_id, aid, mid, zzz_uid, zzz_nickname, zzz_level, zzz_region,
                                        zzz_region_name, last_synced_at, created_at)
           SELECT DISTINCT ON (b.aid) b.user_id, b.aid, b.mid, b.zzz_uid, b.zzz_nickname, b.zzz_level,
                  b.zzz_region, b.zzz_region_name, b.last_synced_at, b.created_at
           FROM (
               SELECT DISTINCT ON (l.user_id) l.user_id, btrim(m.aid) AS aid,
                      nullif(left(btrim(m.mid), 64), '') AS mid, m.zzz_uid, m.zzz_nickname, m.zzz_level,
                      m.zzz_region, m.zzz_region_name, m.last_synced_at AT TIME ZONE $1 AS last_synced_at,
                      coalesce(m.created_at AT TIME ZONE $1, $2) AS created_at, m.id
               FROM legacy.mihoyo_bindings m
               JOIN legacy.mihoyo_bindings_user_lnk l ON l.mihoyo_binding_id = m.id
               JOIN users u ON u.id = l.user_id AND u.status IN ('active', 'blocked')
               WHERE char_length(btrim(coalesce(m.aid, ''))) BETWEEN 1 AND 64
               ORDER BY l.user_id, m.created_at, m.id
           ) b
           ORDER BY b.aid, b.created_at, b.id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}
