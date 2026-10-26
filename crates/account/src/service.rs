use chrono::Utc;
use ik_auth::service::{RequestMeta, sessions_changed_event};
use ik_auth::{AuthState, Principal};
use ik_kernel::Problem;
use ik_kernel::bus::{Audience, Effects, Event, internal};

use crate::dto::{DeleteAccountRequest, DeleteMethod, SecurityView};

fn delete_method(has_email: bool) -> DeleteMethod {
    if has_email {
        DeleteMethod::EmailCode
    } else {
        DeleteMethod::MihoyoQr
    }
}

/// 账号安全概览（DEL-001）。
pub async fn security(state: &AuthState, principal: &Principal) -> Result<SecurityView, Problem> {
    let user_id = principal.user_id.get();
    let credentials = ik_auth::service::credentials(state, user_id).await?;
    let mut conn = state.db.acquire().await?;
    let mihoyo_bound = ik_mihoyo::interop::is_bound(&mut conn, user_id).await?;
    Ok(SecurityView {
        delete_method: delete_method(credentials.email.is_some()),
        email: credentials.email,
        has_password: credentials.has_password,
        mihoyo_bound,
    })
}

/// 发注销验证码到自己的邮箱（DEL-003）。没有邮箱的账号用米游社扫码核验。
pub async fn send_delete_code(
    state: &AuthState,
    principal: &Principal,
    meta: &RequestMeta,
) -> Result<ik_auth::dto::SendCodeResponse, Problem> {
    ik_auth::service::send_delete_code(state, principal, meta).await
}

enum Proof {
    Code(ik_auth::service::DeleteCode),
    Qr(String),
}

fn required(field: &str) -> Problem {
    Problem::field(field, "REQUIRED", "缺少注销核验")
}

/// 注销账号（DEL-004～010，D-071 方案 A：立即生效、不可撤回，内容保留，只脱敏身份）。
///
/// 核验方式按有没有邮箱分：有邮箱用注销验证码（锁外核对、锁内消费），没有用米游社扫码核验。
/// 同一个事务里：消费核验 → 删除米游社绑定 → 清空个人资料 → 账号脱敏并吊销全部会话。
/// 提交后实时网关断开此人的连接，搜索索引里的作者名随之更新。
pub async fn delete_account(
    state: &AuthState,
    principal: &Principal,
    request: DeleteAccountRequest,
) -> Result<(), Problem> {
    let user_id = principal.user_id.get();
    let credentials = ik_auth::service::credentials(state, user_id).await?;
    let proof = if credentials.email.is_some() {
        let code = request.code.ok_or_else(|| required("code"))?;
        Proof::Code(ik_auth::service::check_delete_code(state, principal, &code).await?)
    } else {
        Proof::Qr(request.qr_id.ok_or_else(|| required("qrId"))?)
    };

    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    ik_auth::interop::lock_active_user(&mut tx, user_id)
        .await?
        .ok_or_else(ik_auth::service::account_blocked)?;
    match &proof {
        Proof::Code(code) => {
            ik_auth::interop::consume_delete_code(&mut tx, user_id, code, now).await?
        }
        Proof::Qr(qr_id) => {
            // 锁外看到没有邮箱，锁内邮箱又出现了：核验方式变了，让客户端重来。
            if ik_auth::interop::has_email(&mut tx, user_id).await? {
                return Err(ik_auth::service::account_changed());
            }
            ik_mihoyo::interop::take_delete_verification(&mut tx, user_id, qr_id, now).await?;
        }
    }
    ik_mihoyo::interop::remove_binding(&mut tx, user_id).await?;
    ik_profile::interop::clear_for_deletion(&mut tx, user_id).await?;
    ik_auth::interop::anonymize_deleted(&mut tx, user_id, now).await?;
    state.lifecycle.deleting(&mut tx, user_id, now).await?;
    tx.commit().await?;

    let mut effects = Effects::new();
    effects.publish(sessions_changed_event(user_id)?);
    effects.publish(Event::new(
        internal::USER_RENAMED,
        Audience::Internal,
        &serde_json::json!({ "userId": user_id }),
    )?);
    effects.dispatch(state.bus.as_ref());
    Ok(())
}
