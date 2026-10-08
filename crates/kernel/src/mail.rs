//! 发信抽象。业务代码只依赖 `Mailer`；具体通道（阿里 SMTP / Brevo 双通道）由 server 装配。
//!
//! 调用方必须在数据库事务提交之后再发信（AGENTS.md 规则 6）。

use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct OutgoingMail {
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("邮件发送失败：{0}")]
pub struct MailError(pub String);

#[async_trait]
pub trait Mailer: Send + Sync + 'static {
    async fn send(&self, mail: OutgoingMail) -> Result<(), MailError>;
}

/// 仅开发环境使用：不真正发信，而是把整封邮件（含验证码）写进日志。
/// 配置校验保证生产环境不能选它。
#[derive(Debug, Default, Clone, Copy)]
pub struct DevLogMailer;

#[async_trait]
impl Mailer for DevLogMailer {
    async fn send(&self, mail: OutgoingMail) -> Result<(), MailError> {
        tracing::info!(to = %mail.to, subject = %mail.subject, body = %mail.text, "[dev] 邮件未真正发送");
        Ok(())
    }
}

/// 日志里展示邮箱时的掩码：`ab***@example.com`，短地址 `a***@example.com`。
pub fn mask_email(email: &str) -> String {
    match email.rsplit_once('@') {
        Some((local, domain)) => {
            let keep: String = local
                .chars()
                .take(if local.chars().count() > 2 { 2 } else { 1 })
                .collect();
            format!("{keep}***@{domain}")
        }
        None => "***".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_local_part() {
        assert_eq!(mask_email("alice@example.com"), "al***@example.com");
        assert_eq!(mask_email("ab@example.com"), "a***@example.com");
        assert_eq!(mask_email("nope"), "***");
    }
}
