//! 发信通道与双通道调度（MAIL-001～007）。业务代码只依赖 `ik_kernel::mail::Mailer`，这里由 server 装配。
//!
//! - 收件人是国内邮箱（MAIL-001 名单 + 配置追加）时主通道是 SMTP（国内邮件服务商），其余是 Brevo。
//! - 主通道失败且 `mail.fallback` 开启时改走另一个通道；都失败返回最后一个错误，调用方报 `MAIL_SEND_FAILED`。
//! - 只配置了一个通道时所有邮件都走它。
//! - 日志里的邮箱一律掩码（MAIL-005），不记录正文（验证码）。
//!   降级以 `mail_fallback = true` 的警告日志记录（旧系统的 Redis 计数器 MAIL-004 不再保留，按日志告警）。

mod brevo;
mod domains;
mod smtp;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use async_trait::async_trait;
use ik_kernel::config::{Mail, MailProvider};
use ik_kernel::mail::{DevLogMailer, MailError, Mailer, OutgoingMail, mask_email};
use lettre::message::Mailbox;

pub use brevo::BrevoChannel;
pub use domains::{Region, classify};
pub use smtp::SmtpChannel;

/// 一个具体的发信通道。
#[async_trait]
pub trait Channel: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    async fn send(&self, mail: &OutgoingMail) -> Result<(), MailError>;
}

/// 按配置装配发信器。发件人地址不合法等配置错误在启动时报出。
pub fn build(config: &Mail) -> anyhow::Result<Arc<dyn Mailer>> {
    match config.provider {
        MailProvider::DevLog => Ok(Arc::new(DevLogMailer)),
        MailProvider::Dual => {
            let smtp = if config.smtp.enabled() {
                Some(Arc::new(SmtpChannel::new(&config.smtp)?) as Arc<dyn Channel>)
            } else {
                None
            };
            let brevo = if config.brevo.enabled() {
                Some(Arc::new(BrevoChannel::new(&config.brevo)?) as Arc<dyn Channel>)
            } else {
                None
            };
            if smtp.is_none() || brevo.is_none() {
                tracing::warn!(
                    smtp = smtp.is_some(),
                    brevo = brevo.is_some(),
                    "只配置了一个发信通道，所有邮件都走它，失败时没有备用"
                );
            }
            Ok(Arc::new(Dispatcher::new(
                smtp,
                brevo,
                config.fallback,
                &config.domestic_domains,
            )))
        }
    }
}

/// 双通道调度器。
pub struct Dispatcher {
    domestic: Option<Arc<dyn Channel>>,
    international: Option<Arc<dyn Channel>>,
    fallback: bool,
    extra_domestic: HashSet<String>,
}

impl Dispatcher {
    /// `domestic` 是国内邮箱的主通道（SMTP），`international` 是其余邮箱的主通道（Brevo）。
    pub fn new(
        domestic: Option<Arc<dyn Channel>>,
        international: Option<Arc<dyn Channel>>,
        fallback: bool,
        extra_domestic: &[String],
    ) -> Self {
        Self {
            domestic,
            international,
            fallback,
            extra_domestic: extra_domestic.iter().cloned().collect(),
        }
    }

    /// 按顺序尝试的通道：主通道在前；关闭降级时只留第一个。
    fn route(&self, region: Region) -> Vec<&Arc<dyn Channel>> {
        let (primary, secondary) = match region {
            Region::Domestic => (&self.domestic, &self.international),
            Region::International => (&self.international, &self.domestic),
        };
        let mut route: Vec<_> = [primary, secondary].into_iter().flatten().collect();
        if !self.fallback {
            route.truncate(1);
        }
        route
    }
}

#[async_trait]
impl Mailer for Dispatcher {
    async fn send(&self, mail: OutgoingMail) -> Result<(), MailError> {
        let region = classify(&mail.to, &self.extra_domestic);
        let route = self.route(region);
        let to = mask_email(&mail.to);
        let started = Instant::now();
        let mut last = MailError("没有可用的发信通道".to_owned());
        for (index, channel) in route.iter().enumerate() {
            match channel.send(&mail).await {
                Ok(()) => {
                    tracing::info!(
                        channel = channel.name(),
                        region = region.as_str(),
                        to = %to,
                        fell_back = index > 0,
                        latency_ms = started.elapsed().as_millis() as u64,
                        "邮件已发送"
                    );
                    return Ok(());
                }
                Err(error) => {
                    if index + 1 < route.len() {
                        tracing::warn!(
                            channel = channel.name(),
                            region = region.as_str(),
                            to = %to,
                            %error,
                            mail_fallback = true,
                            "邮件发送失败，改走备用通道"
                        );
                    } else {
                        tracing::error!(
                            channel = channel.name(),
                            region = region.as_str(),
                            to = %to,
                            %error,
                            "邮件发送失败"
                        );
                    }
                    last = error;
                }
            }
        }
        Err(last)
    }
}

/// 解析 `绳网 <a@example.cn>`、`"绳网" <a@example.cn>` 或纯地址。
fn parse_mailbox(key: &str, value: &str) -> anyhow::Result<Mailbox> {
    value
        .trim()
        .parse()
        .with_context(|| format!("{key} 不是合法的邮箱地址：{value}"))
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use super::*;

    struct Fake {
        name: &'static str,
        fail: bool,
        sent: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(name: &'static str, fail: bool) -> Arc<Self> {
            Arc::new(Self {
                name,
                fail,
                sent: Mutex::new(Vec::new()),
            })
        }

        fn sent(&self) -> Vec<String> {
            self.sent.lock().clone()
        }
    }

    #[async_trait]
    impl Channel for Fake {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn send(&self, mail: &OutgoingMail) -> Result<(), MailError> {
            self.sent.lock().push(mail.to.clone());
            if self.fail {
                Err(MailError(format!("{} down", self.name)))
            } else {
                Ok(())
            }
        }
    }

    fn mail(to: &str) -> OutgoingMail {
        OutgoingMail {
            to: to.to_owned(),
            subject: "s".to_owned(),
            text: "t".to_owned(),
            html: None,
        }
    }

    fn dispatcher(smtp: &Arc<Fake>, brevo: &Arc<Fake>, fallback: bool) -> Dispatcher {
        Dispatcher::new(
            Some(smtp.clone() as Arc<dyn Channel>),
            Some(brevo.clone() as Arc<dyn Channel>),
            fallback,
            &["corp.cn".to_owned()],
        )
    }

    #[tokio::test]
    async fn routes_by_region() {
        let (smtp, brevo) = (Fake::new("smtp", false), Fake::new("brevo", false));
        let mailer = dispatcher(&smtp, &brevo, true);
        mailer.send(mail("a@qq.com")).await.unwrap();
        mailer.send(mail("b@corp.cn")).await.unwrap();
        mailer.send(mail("c@gmail.com")).await.unwrap();
        mailer.send(mail("broken")).await.unwrap();
        assert_eq!(smtp.sent(), ["a@qq.com", "b@corp.cn"]);
        assert_eq!(brevo.sent(), ["c@gmail.com", "broken"]);
    }

    #[tokio::test]
    async fn falls_back_to_the_other_channel() {
        let (smtp, brevo) = (Fake::new("smtp", true), Fake::new("brevo", false));
        dispatcher(&smtp, &brevo, true)
            .send(mail("a@qq.com"))
            .await
            .unwrap();
        assert_eq!(smtp.sent(), ["a@qq.com"]);
        assert_eq!(brevo.sent(), ["a@qq.com"]);
    }

    #[tokio::test]
    async fn without_fallback_only_the_primary_is_tried() {
        let (smtp, brevo) = (Fake::new("smtp", false), Fake::new("brevo", true));
        let error = dispatcher(&smtp, &brevo, false)
            .send(mail("a@gmail.com"))
            .await
            .unwrap_err();
        assert_eq!(error.0, "brevo down");
        assert!(smtp.sent().is_empty());
    }

    #[tokio::test]
    async fn both_failing_returns_the_last_error() {
        let (smtp, brevo) = (Fake::new("smtp", true), Fake::new("brevo", true));
        let error = dispatcher(&smtp, &brevo, true)
            .send(mail("a@gmail.com"))
            .await
            .unwrap_err();
        assert_eq!(error.0, "smtp down");
    }

    #[tokio::test]
    async fn a_single_channel_takes_everything() {
        let smtp = Fake::new("smtp", false);
        let mailer = Dispatcher::new(Some(smtp.clone() as Arc<dyn Channel>), None, false, &[]);
        mailer.send(mail("a@gmail.com")).await.unwrap();
        assert_eq!(smtp.sent(), ["a@gmail.com"]);
    }

    #[test]
    fn sender_formats() {
        let quoted = parse_mailbox("k", "\"绳网\" <noreply@example.cn>").unwrap();
        assert_eq!(quoted.name.as_deref(), Some("绳网"));
        assert_eq!(quoted.email.to_string(), "noreply@example.cn");
        let bare = parse_mailbox("k", " noreply@example.cn ").unwrap();
        assert_eq!(bare.name, None);
        assert!(parse_mailbox("k", "not an address").is_err());
    }
}
