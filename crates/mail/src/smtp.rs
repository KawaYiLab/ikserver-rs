//! SMTP 通道（MAIL-006）：国内邮件服务商，连接池复用连接，TLS 必须（连接即 TLS 或强制 STARTTLS）。

use std::time::Duration;

use async_trait::async_trait;
use ik_kernel::config::{Smtp, SmtpTls};
use ik_kernel::mail::{MailError, OutgoingMail};
use lettre::message::{Mailbox, MultiPart, header::ContentType};
use lettre::transport::smtp::PoolConfig;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::{Channel, parse_mailbox};

pub struct SmtpChannel {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
    reply_to: Option<Mailbox>,
}

impl SmtpChannel {
    pub fn new(config: &Smtp) -> anyhow::Result<Self> {
        let builder = match config.tls {
            SmtpTls::Implicit => AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)?,
            SmtpTls::Starttls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)?
            }
        };
        let mut builder = builder
            .port(config.port)
            .timeout(Some(Duration::from_secs(config.timeout_secs)))
            .pool_config(PoolConfig::new().max_size(config.pool_size));
        if !config.username.is_empty() {
            builder = builder.credentials(Credentials::new(
                config.username.clone(),
                config.password.expose().to_owned(),
            ));
        }
        Ok(Self {
            transport: builder.build(),
            from: parse_mailbox("mail.smtp.from", &config.from)?,
            reply_to: optional_mailbox("mail.smtp.reply_to", &config.reply_to)?,
        })
    }

    fn message(&self, mail: &OutgoingMail) -> Result<Message, MailError> {
        let to: Mailbox = mail
            .to
            .parse()
            .map_err(|e| MailError(format!("收件地址不合法：{e}")))?;
        let mut builder = Message::builder()
            .message_id(Some(message_id(self.from.email.domain())?))
            .from(self.from.clone())
            .to(to)
            .subject(&mail.subject);
        if let Some(reply_to) = &self.reply_to {
            builder = builder.reply_to(reply_to.clone());
        }
        let message = match &mail.html {
            Some(html) => builder.multipart(MultiPart::alternative_plain_html(
                mail.text.clone(),
                html.clone(),
            )),
            None => builder
                .header(ContentType::TEXT_PLAIN)
                .body(mail.text.clone()),
        };
        message.map_err(|e| MailError(format!("邮件构造失败：{e}")))
    }
}

#[async_trait]
impl Channel for SmtpChannel {
    fn name(&self) -> &'static str {
        "smtp"
    }

    async fn send(&self, mail: &OutgoingMail) -> Result<(), MailError> {
        let message = self.message(mail)?;
        self.transport
            .send(message)
            .await
            .map(|_| ())
            .map_err(|e| MailError(format!("SMTP：{e}")))
    }
}

/// `<随机串@发件域名>`：没有 Message-ID 的邮件更容易被收件方判为垃圾邮件，
/// 用发件人的域名而不是本机主机名（服务器的主机名通常不是公网域名）。
fn message_id(domain: &str) -> Result<String, MailError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| MailError(format!("系统随机源不可用：{e}")))?;
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("<{id}@{domain}>"))
}

fn optional_mailbox(key: &str, value: &str) -> anyhow::Result<Option<Mailbox>> {
    if value.is_empty() {
        Ok(None)
    } else {
        parse_mailbox(key, value).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    use super::*;

    /// 只会说最基本 SMTP 的假服务器：收完一封信就把对话记录交回来。
    #[allow(clippy::unwrap_used)]
    async fn fake_server() -> (u16, oneshot::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (done, transcript) = oneshot::channel();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut lines = BufReader::new(read).lines();
            write.write_all(b"220 fake ESMTP\r\n").await.unwrap();
            let mut log = String::new();
            let mut in_data = false;
            while let Some(line) = lines.next_line().await.unwrap() {
                log.push_str(&line);
                log.push('\n');
                if in_data {
                    if line == "." {
                        write.write_all(b"250 OK queued\r\n").await.unwrap();
                        let _ = done.send(log);
                        return;
                    }
                    continue;
                }
                let command = line.to_ascii_uppercase();
                let reply: &[u8] = if command.starts_with("EHLO") {
                    b"250-fake\r\n250-AUTH PLAIN LOGIN\r\n250 8BITMIME\r\n"
                } else if command.starts_with("AUTH") {
                    b"235 2.7.0 Accepted\r\n"
                } else if command.starts_with("DATA") {
                    in_data = true;
                    b"354 End data with <CR><LF>.<CR><LF>\r\n"
                } else {
                    b"250 OK\r\n"
                };
                write.write_all(reply).await.unwrap();
            }
        });
        (port, transcript)
    }

    #[tokio::test]
    async fn sends_multipart_mail_with_auth_and_reply_to() {
        let (port, transcript) = fake_server().await;
        // 测试里没有 TLS：直接用明文传输构造通道，其余逻辑与生产相同。
        let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("127.0.0.1")
            .port(port)
            .credentials(Credentials::new("bot@example.cn".into(), "secret".into()))
            .build();
        let channel = SmtpChannel {
            transport,
            from: parse_mailbox("from", "\"绳网\" <bot@example.cn>").unwrap(),
            reply_to: optional_mailbox("reply_to", "help@example.cn").unwrap(),
        };
        channel
            .send(&OutgoingMail {
                to: "Alice@QQ.com".into(),
                subject: "【123456】注册验证码 - 绳网".into(),
                text: "您的注册验证码是 123456".into(),
                html: Some("<p>123456</p>".into()),
            })
            .await
            .unwrap();

        let log = transcript.await.unwrap();
        assert!(log.contains("AUTH PLAIN"), "{log}");
        assert!(log.contains("MAIL FROM:<bot@example.cn>"), "{log}");
        assert!(log.contains("RCPT TO:<Alice@QQ.com>"), "{log}");
        assert!(log.contains("Reply-To: help@example.cn"), "{log}");
        assert!(log.contains("multipart/alternative"), "{log}");
        assert!(log.contains("text/plain"), "{log}");
        assert!(log.contains("text/html"), "{log}");
        // 非 ASCII 的主题按 RFC 2047 编码
        assert!(log.contains("Subject: =?utf-8?"), "{log}");
        let id = log
            .lines()
            .find_map(|line| line.strip_prefix("Message-ID: "))
            .unwrap();
        assert!(id.starts_with('<') && id.ends_with("@example.cn>"), "{id}");
    }

    #[tokio::test]
    async fn rejects_bad_recipient_before_connecting() {
        let channel = SmtpChannel {
            transport: AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("127.0.0.1").build(),
            from: parse_mailbox("from", "bot@example.cn").unwrap(),
            reply_to: None,
        };
        let mail = OutgoingMail {
            to: "not an address".into(),
            subject: "s".into(),
            text: "t".into(),
            html: None,
        };
        assert!(channel.message(&mail).is_err());
    }
}
