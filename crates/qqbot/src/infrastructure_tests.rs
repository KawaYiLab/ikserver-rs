use super::*;

#[sqlx::test(migrator = "MIGRATOR")]
async fn lifecycle_uses_platform_time_but_keeps_equal_and_missing_time_compatible(db: PgPool) {
    let (s, _, _, server) = setup(db).await;
    let base = Utc::now().timestamp();
    for (event, time, receipt) in [
        ("GROUP_DEL_ROBOT", Some(base), "removed"),
        ("GROUP_ADD_ROBOT", Some(base - 1), "late-add"),
    ] {
        repo::lifecycle(
            &s,
            "123",
            event,
            &json!({"group_openid":"group", "timestamp":time}),
            receipt,
        )
        .await
        .unwrap();
    }
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_groups WHERE left_at IS NOT NULL"
        )
        .await,
        1
    );
    repo::lifecycle(
        &s,
        "123",
        "GROUP_ADD_ROBOT",
        &json!({"group_openid":"group", "timestamp":base.to_string()}),
        "same-second",
    )
    .await
    .unwrap();
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_groups WHERE left_at IS NULL").await,
        1
    );
    repo::lifecycle(
        &s,
        "123",
        "GROUP_DEL_ROBOT",
        &json!({"group_openid":"group"}),
        "missing-time",
    )
    .await
    .unwrap();
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_groups WHERE left_at IS NOT NULL"
        )
        .await,
        1
    );
    // Even a replay of a previously applied add must not undo the last removal.
    repo::lifecycle(
        &s,
        "123",
        "GROUP_ADD_ROBOT",
        &json!({"group_openid":"group", "timestamp":base}),
        "same-second",
    )
    .await
    .unwrap();
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_groups WHERE left_at IS NOT NULL"
        )
        .await,
        1
    );

    for (event, time, receipt) in [
        ("FRIEND_DEL", Some(base), "friend-removed"),
        ("FRIEND_ADD", Some(base - 1), "late-friend"),
    ] {
        repo::lifecycle(
            &s,
            "123",
            event,
            &json!({"openid":"friend", "timestamp":time}),
            receipt,
        )
        .await
        .unwrap();
    }
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_identities WHERE scene='c2c' AND NOT available"
        )
        .await,
        1
    );
    repo::lifecycle(
        &s,
        "123",
        "FRIEND_ADD",
        &json!({"user_openid":"friend", "timestamp":null}),
        "friend-no-time",
    )
    .await
    .unwrap();
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_identities WHERE scene='c2c' AND available"
        )
        .await,
        1
    );
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn signed_optional_fields_keep_receipt_fallback_and_unusable_events_ack(db: PgPool) {
    use crate::webhook::tests_support::{call, sign};
    let (s, _, _, server) = setup(db).await;
    let before: chrono::DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&s.db)
        .await
        .unwrap();
    for (index, timestamp) in [
        Value::Null,
        json!("昨天"),
        json!(Utc::now() + Duration::days(2)),
    ]
    .into_iter()
    .enumerate()
    {
        let body = serde_json::to_vec(&json!({
            "op":0,"t":"C2C_MESSAGE_CREATE","d":{
                "id":format!("fallback-{index}"),"timestamp":timestamp,"content":null,
                "author":{"id":"fallback-user","username":null,"bot":null},
                "mentions":null,"attachments":null,"future_field":{"arbitrary":true}
            }
        }))
        .unwrap();
        let (status, ack) = call(
            &s,
            "123",
            "abc",
            &sign("abc", "sig-time", &body),
            "sig-time",
            body,
        )
        .await;
        assert_eq!(status, http::StatusCode::OK);
        assert_eq!(ack, json!({"op":12,"d":0}));
    }
    let timestamps: Vec<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT created_at FROM qq_messages")
            .fetch_all(&s.db)
            .await
            .unwrap();
    assert_eq!(timestamps.len(), 3);
    let after: chrono::DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert!(timestamps.iter().all(|t| *t >= before && *t <= after));
    for d in [
        json!({"id":"bad","author":true}),
        json!({"id":"no-author"}),
        json!({"author":{"id":"somebody"}}),
    ] {
        let body = serde_json::to_vec(&json!({"op":0,"t":"C2C_MESSAGE_CREATE","d":d})).unwrap();
        let (status, _) = call(&s, "123", "abc", &sign("abc", "time", &body), "time", body).await;
        assert_eq!(status, http::StatusCode::OK);
    }
    assert_eq!(count(&s, "SELECT count(*) FROM qq_messages").await, 3);
    server.abort();
}
