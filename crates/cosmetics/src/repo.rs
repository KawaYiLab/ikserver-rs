use sqlx::PgConnection;

pub(crate) struct AvatarRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub image_media_id: i64,
}

pub(crate) struct CardRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub description: String,
    pub story: String,
    pub image_media_id: i64,
}

pub(crate) async fn all_avatars(conn: &mut PgConnection) -> sqlx::Result<Vec<AvatarRow>> {
    sqlx::query_as!(
        AvatarRow,
        "SELECT id, name, kind, image_media_id FROM system_avatars ORDER BY kind, id"
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn avatars_by_ids(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<AvatarRow>> {
    sqlx::query_as!(
        AvatarRow,
        "SELECT id, name, kind, image_media_id FROM system_avatars WHERE id = ANY($1)",
        ids
    )
    .fetch_all(conn)
    .await
}

/// 按 `(kind, id)` 键集分页；`after` 是上一页最后一条。
pub(crate) async fn cards_page(
    conn: &mut PgConnection,
    kind: Option<&str>,
    after: Option<(&str, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<CardRow>> {
    let (after_kind, after_id) = after.unzip();
    sqlx::query_as!(
        CardRow,
        "SELECT id, name, kind, description, story, image_media_id FROM business_cards
         WHERE ($1::text IS NULL OR kind = $1)
           AND ($2::text IS NULL OR (kind, id) > ($2, $3))
         ORDER BY kind, id
         LIMIT $4",
        kind,
        after_kind,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn cards_by_ids(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<CardRow>> {
    sqlx::query_as!(
        CardRow,
        "SELECT id, name, kind, description, story, image_media_id FROM business_cards
         WHERE id = ANY($1)",
        ids
    )
    .fetch_all(conn)
    .await
}

/// 按名字新增或更新，返回 `(id, 图片是否变了)`。
pub(crate) async fn upsert_avatar(
    conn: &mut PgConnection,
    id: i64,
    name: &str,
    kind: &str,
    image_media_id: i64,
) -> sqlx::Result<(i64, bool)> {
    let previous = sqlx::query_scalar!(
        "SELECT image_media_id FROM system_avatars WHERE name = $1 FOR UPDATE",
        name
    )
    .fetch_optional(&mut *conn)
    .await?;
    let id = sqlx::query_scalar!(
        "INSERT INTO system_avatars (id, name, kind, image_media_id) VALUES ($1, $2, $3, $4)
         ON CONFLICT (name) DO UPDATE
         SET kind = EXCLUDED.kind, image_media_id = EXCLUDED.image_media_id, updated_at = now()
         RETURNING id",
        id,
        name,
        kind,
        image_media_id,
    )
    .fetch_one(conn)
    .await?;
    Ok((id, previous.is_some_and(|p| p != image_media_id)))
}

pub(crate) struct NewCard<'a> {
    pub id: i64,
    pub name: &'a str,
    pub kind: &'a str,
    pub description: &'a str,
    pub story: &'a str,
    pub image_media_id: i64,
}

pub(crate) async fn upsert_card(conn: &mut PgConnection, card: &NewCard<'_>) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        "INSERT INTO business_cards (id, name, kind, description, story, image_media_id)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (name) DO UPDATE
         SET kind = EXCLUDED.kind, description = EXCLUDED.description, story = EXCLUDED.story,
             image_media_id = EXCLUDED.image_media_id, updated_at = now()
         RETURNING id",
        card.id,
        card.name,
        card.kind,
        card.description,
        card.story,
        card.image_media_id,
    )
    .fetch_one(conn)
    .await
}

// ---------- 管理后台 ----------

pub(crate) async fn all_cards(conn: &mut PgConnection) -> sqlx::Result<Vec<CardRow>> {
    sqlx::query_as!(
        CardRow,
        "SELECT id, name, kind, description, story, image_media_id FROM business_cards
         ORDER BY kind, id"
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn insert_avatar(
    conn: &mut PgConnection,
    id: i64,
    name: &str,
    kind: &str,
    image_media_id: i64,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO system_avatars (id, name, kind, image_media_id) VALUES ($1, $2, $3, $4)",
        id,
        name,
        kind,
        image_media_id,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn update_avatar(
    conn: &mut PgConnection,
    id: i64,
    name: &str,
    kind: &str,
    image_media_id: i64,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE system_avatars SET name = $2, kind = $3, image_media_id = $4, updated_at = now()
         WHERE id = $1",
        id,
        name,
        kind,
        image_media_id,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn delete_avatar(conn: &mut PgConnection, id: i64) -> sqlx::Result<bool> {
    Ok(sqlx::query!("DELETE FROM system_avatars WHERE id = $1", id)
        .execute(conn)
        .await?
        .rows_affected()
        > 0)
}

pub(crate) async fn insert_card(conn: &mut PgConnection, card: &NewCard<'_>) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO business_cards (id, name, kind, description, story, image_media_id)
         VALUES ($1, $2, $3, $4, $5, $6)",
        card.id,
        card.name,
        card.kind,
        card.description,
        card.story,
        card.image_media_id,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn update_card(conn: &mut PgConnection, card: &NewCard<'_>) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE business_cards SET name = $2, kind = $3, description = $4, story = $5,
             image_media_id = $6, updated_at = now()
         WHERE id = $1",
        card.id,
        card.name,
        card.kind,
        card.description,
        card.story,
        card.image_media_id,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn delete_card(conn: &mut PgConnection, id: i64) -> sqlx::Result<bool> {
    Ok(sqlx::query!("DELETE FROM business_cards WHERE id = $1", id)
        .execute(conn)
        .await?
        .rows_affected()
        > 0)
}
