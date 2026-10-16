//! Brevo 通道（MAIL-007）：`POST {api_url}/smtp/email`，`api-key` 头，正文同时带纯文本与 HTML。

use std::time::Duration;

use async_trait::async_trait;
use ik_kernel::config::Brevo;
use ik_kernel::mail::{MailError, OutgoingMail};
use lettre::message::Mailbox;
use serde::Serialize;

use crate::{Channel, parse_mailbox};

pub struct BrevoChannel {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    sender: Party,
    reply_to: Option<Party>,
}

#[derive(Debug, Clone, Serialize)]
struct Party {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    email: String,
}

impl From<Mailbox> for Party {
    fn from(mailbox: Mailbox) -> Self {
        Self {
            name: mailbox.name,
            email: mailbox.email.to_string(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Payload<'a> {
    sender: &'a Party,
    to: [Recipient<'a>; 1],
    subject: &'a str,
    text_content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    html_content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to: Option<&'a Party>,
}

#[derive(Serialize)]
struct Recipient<'a> {
    email: &'a str,
}

/// 上游错误信息只留开头一段，防止把整页 HTML 写进日志。
const ERROR_BODY_LIMIT: usize = 200;

impl BrevoChannel {
    pub fn new(config: &Brevo) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()?;
        Ok(Self {
            client,
            endpoint: format!("{}/smtp/email", config.api_url.trim_end_matches('/')),
            api_key: config.api_key.expose().to_owned(),
            sender: parse_mailbox("mail.brevo.from", &config.from)?.into(),
            reply_to: if config.reply_to.is_empty() {
                None
            } else {
                Some(parse_mailbox("mail.brevo.reply_to", &config.reply_to)?.into())
            },
        })
    }
}

#[async_trait]
impl Channel for BrevoChannel {
    fn name(&self) -> &'static str {
        "brevo"
    }

    async fn send(&self, mail: &OutgoingMail) -> Result<(), MailError> {
        let payload = Payload {
            sender: &self.sender,
            to: [Recipient { email: &mail.to }],
            subject: &mail.subject,
            text_content: &mail.text,
            html_content: mail.html.as_deref(),
            reply_to: self.reply_to.as_ref(),
        };
        let response = self
            .client
            .post(&self.endpoint)
            .header("api-key", &self.api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| MailError(format!("Brevo 请求失败：{e}")))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        let body: String = body.chars().take(ERROR_BODY_LIMIT).collect();
        Err(MailError(format!("Brevo 返回 {status}：{body}")))
    }
}
