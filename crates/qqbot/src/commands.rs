//! QQ 指令（ADR 0008 第 7 节）：以 `/` 开头，群里需要 @机器人。不调用模型、不计次、不计费。
use chrono::{Duration, Utc};
use ik_kernel::{Problem, bus::Effects};
use sqlx::PgConnection;

use crate::{QqState, admin::Settings, me, repo::Turn};

/// 绑定码连续用错这么多次以后，这个身份当天不能再试（码有 31^8 种，挡住枚举）。
const BIND_FAILURES_PER_DAY: i64 = 10;

/// 一条指令的回复。
pub(crate) struct Reply {
    pub text: String,
    /// 记在 turn 上的原因（排障与限次用）。
    pub reason: Option<&'static str>,
    /// 提交后发出的实时事件（绑定成功的站内通知）。
    pub effects: Effects,
}

impl Reply {
    fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            reason: None,
            effects: Effects::new(),
        }
    }

    fn failed(text: impl Into<String>, reason: &'static str) -> Self {
        Self {
            reason: Some(reason),
            ..Self::text(text)
        }
    }
}

const HELP: &str = "在群里 @我，或者直接给我发消息，就能和我聊天。\n\
指令：/额度 查看今天的额度；/绑定 <码> 绑定绳网账号；/解绑 解除绑定；/角色 查看或切换角色（单聊）；/重置 清空这段对话的上下文（单聊）。\n\
没绑定时每天有免费次数（北京时间 04:00 重置）。在绳网「设置 → 绑定 QQ」生成绑定码，再发「/绑定 绑定码」，\
就按你的个人额度使用，我也会记住你。在哪个群用我，就在哪个群绑一次。";

/// 执行指令。在调用方的事务里；回复由调用方保存并发出。
pub(crate) async fn run(
    s: &QqState,
    tx: &mut PgConnection,
    t: &Turn,
    bound: Option<i64>,
    settings: &Settings,
) -> Result<Reply, Problem> {
    let mut parts = t.content.trim().splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or_default();
    let arg = parts.next().unwrap_or_default().trim();
    Ok(match command {
        "/帮助" | "/help" => Reply::text(HELP),
        "/额度" => Reply::text(quota(s, tx, t, bound, settings).await?),
        "/绑定" => bind(s, tx, t, bound, arg).await?,
        "/解绑" => {
            if bound.is_some() && me::release(tx, t.identity_id, bound).await? {
                Reply::text("已解除绑定。之后按游客的免费次数使用，我也不会再记住新的事。")
            } else {
                Reply::text("这个 QQ 还没有绑定绳网账号。")
            }
        }
        "/角色" => Reply::text(role(s, tx, t, arg).await?),
        "/重置" => {
            if t.group_id.is_some() {
                Reply::text("群里的对话上下文只能由管理员重置。")
            } else {
                sqlx::query!(
                    "UPDATE qq_identities SET context_reset_at=$2 WHERE id=$1",
                    t.identity_id,
                    t.message_at
                )
                .execute(&mut *tx)
                .await?;
                Reply::text("好的，我们从头开始聊吧。（长期记忆不受影响）")
            }
        }
        _ => Reply::text("暂不支持这个指令，发送 /帮助 查看可用指令。"),
    })
}

async fn quota(
    s: &QqState,
    tx: &mut PgConnection,
    t: &Turn,
    bound: Option<i64>,
    settings: &Settings,
) -> Result<String, Problem> {
    let now = Utc::now();
    if let Some(user_id) = bound {
        // 只给百分比（AI-023）：token 数与上限不外露。
        let limit = s.ai.catalog.get().await?.limits.per_user;
        if limit <= 0 {
            return Ok("你已绑定绳网账号，今天的 AI 额度不限。".to_owned());
        }
        let used = ik_ai::quota::read(tx, user_id, now).await?.used;
        let percent = (used.saturating_mul(100) / limit).min(100);
        return Ok(format!(
            "你已绑定绳网账号，今天的 AI 额度已用 {percent}%，04:00 重置。"
        ));
    }
    let day = ik_kernel::time::biz_day(now);
    let used = sqlx::query_scalar!(
        "SELECT turns FROM qq_guest_usage WHERE day=$1 AND identity_id=$2",
        day,
        t.identity_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .unwrap_or(0);
    Ok(format!(
        "今天还剩 {} 次免费对话，04:00 重置。绑定绳网账号后按个人额度使用，发送 /帮助 查看方法。",
        (settings.guest_daily_turns - used).max(0)
    ))
}

async fn bind(
    s: &QqState,
    tx: &mut PgConnection,
    t: &Turn,
    bound: Option<i64>,
    arg: &str,
) -> Result<Reply, Problem> {
    if bound.is_some() {
        return Ok(Reply::text(
            "这个 QQ 已经绑定了绳网账号。要换账号，先发 /解绑。",
        ));
    }
    let now = Utc::now();
    let failures = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM qq_turns WHERE identity_id=$1 AND reason='bind_failed' AND created_at>$2"#,
        t.identity_id,
        now - Duration::days(1)
    )
    .fetch_one(&mut *tx)
    .await?;
    if failures >= BIND_FAILURES_PER_DAY {
        return Ok(Reply::failed(
            "绑定失败的次数太多了，明天再试吧。",
            "bind_limited",
        ));
    }
    let invalid = || {
        Reply::failed(
            "绑定码无效或已过期。请在绳网「设置 → 绑定 QQ」重新生成，10 分钟内发送「/绑定 绑定码」。",
            "bind_failed",
        )
    };
    let Some(code) = me::normalize_code(arg) else {
        return Ok(invalid());
    };
    let hash = me::code_hash(&s.code_key, &code);
    let Some(row) = sqlx::query!(
        "SELECT user_id, expires_at, used_at FROM qq_bind_codes WHERE code_hash=$1 FOR UPDATE",
        hash
    )
    .fetch_optional(&mut *tx)
    .await?
    .filter(|row| row.used_at.is_none() && row.expires_at > now) else {
        return Ok(invalid());
    };
    // 码在群里发出后别人也能看到：按平台时间，只有最先发出这个码的身份能用。若别的身份更早发过，
    // 说明码已经泄露（或被抢先），直接作废，谁都绑不上。不同身份的任务在不同队列里，处理顺序不等于发送顺序。
    let earlier = sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM qq_messages
               WHERE direction = 'in' AND identity_id <> $1 AND left(content, 3) = '/绑定'
                 AND created_at >= $2::timestamptz - interval '15 minutes' AND (created_at, id) < ($2::timestamptz, $3::bigint)
                 AND upper(regexp_replace(content, '[[:space:]-]', '', 'g')) LIKE '%' || $4 || '%'
           ) AS "earlier!""#,
        t.identity_id,
        t.message_at,
        t.message_id,
        code
    )
    .fetch_one(&mut *tx)
    .await?;
    if earlier {
        sqlx::query!(
            "UPDATE qq_bind_codes SET used_at=$2 WHERE code_hash=$1",
            hash,
            now
        )
        .execute(&mut *tx)
        .await?;
        return Ok(Reply::failed(
            "这个绑定码已经被别人先发过，已作废。请在绳网重新生成，最好在单聊里发给我。",
            "bind_failed",
        ));
    }
    // Binding introduces a new owner after locking this previously guest identity.
    // NOWAIT avoids reversing the deletion hook's user -> identity lock order;
    // a concurrent deletion causes the whole command transaction to retry.
    let active = ik_auth::interop::try_lock_user_status(tx, row.user_id)
        .await?
        .as_deref()
        == Some("active");
    if !active {
        return Ok(invalid());
    }
    let changed = sqlx::query(
        "UPDATE qq_identities SET user_id=$2,bound_at=$3,
             binding_version=binding_version+1,context_reset_at=NULL
         WHERE id=$1 AND user_id IS NULL",
    )
    .bind(t.identity_id)
    .bind(row.user_id)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Ok(Reply::text(
            "这个 QQ 已经绑定了绳网账号。要换账号，先发 /解绑。",
        ));
    }
    sqlx::query!(
        "UPDATE qq_bind_codes SET used_at=$2,used_by_identity_id=$3 WHERE code_hash=$1",
        hash,
        now,
        t.identity_id
    )
    .execute(&mut *tx)
    .await?;
    // 站内通知：万一不是本人操作，能及时发现并解绑。群名只用管理员填的展示名，不出现 openid。
    let place = match t.group_id {
        Some(group_id) => {
            let group = sqlx::query!(
                "SELECT display_name, group_openid FROM qq_groups WHERE id=$1",
                group_id
            )
            .fetch_one(&mut *tx)
            .await?;
            format!(
                "QQ 群「{}」里的身份",
                crate::repo::group_name(group.display_name, &group.group_openid)
            )
        }
        None => "QQ 单聊身份".to_owned(),
    };
    let mut effects = Effects::new();
    ik_notify::interop::system(
        tx,
        &s.ids,
        &[row.user_id],
        &format!(
            "你的绳网账号绑定了一个{place}。如果不是你本人操作，请到「设置 → 绑定 QQ」解除绑定。"
        ),
        None,
        now,
        &mut effects,
    )
    .await?;
    // 群里不说绑定的是哪个绳网账号，避免把 QQ 与站内账号公开关联起来。
    Ok(Reply {
        text: "绑定成功！之后按你的个人额度使用，我也会记住你。".to_owned(),
        reason: Some("bound"),
        effects,
    })
}

async fn role(s: &QqState, tx: &mut PgConnection, t: &Turn, arg: &str) -> Result<String, Problem> {
    let catalog = s.ai.catalog.get().await?;
    let cards: Vec<_> = catalog
        .cards
        .iter()
        .filter(|card| card.enabled && card.qq_enabled)
        .collect();
    let current = t
        .card_slug
        .as_deref()
        .and_then(|slug| cards.iter().find(|card| card.slug == slug))
        .map_or("（未设置）", |card| card.display_name.as_str());
    if arg.is_empty() || t.group_id.is_some() {
        let names: Vec<&str> = cards
            .iter()
            .map(|card| card.display_name.as_str())
            .collect();
        let tail = if t.group_id.is_some() {
            "群里的角色由管理员设置。"
        } else {
            "发送「/角色 名字」切换。"
        };
        return Ok(format!(
            "现在的角色：{current}。可以选的角色：{}。{tail}",
            if names.is_empty() {
                "（无）".to_owned()
            } else {
                names.join("、")
            }
        ));
    }
    let Some(card) = cards
        .iter()
        .find(|card| card.display_name == arg || card.slug.eq_ignore_ascii_case(arg))
    else {
        return Ok(format!(
            "没有叫「{}」的角色。发送 /角色 查看可以选的角色。",
            crate::text::nickname(arg)
        ));
    };
    sqlx::query!(
        "UPDATE qq_identities SET card_slug=$2 WHERE id=$1",
        t.identity_id,
        card.slug
    )
    .execute(&mut *tx)
    .await?;
    Ok(format!("已切换到「{}」。", card.display_name))
}
