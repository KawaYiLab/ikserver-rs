//! The persisted wire format is independent of the role card's site output mode.
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema, sqlx::Type,
)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[schema(as = QqMessageFormat)]
pub enum MessageFormat {
    Plain,
    Markdown,
}

impl MessageFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Markdown => "markdown",
        }
    }

    pub fn output_mode(self) -> ik_ai::catalog::OutputMode {
        match self {
            Self::Plain => ik_ai::catalog::OutputMode::Plain,
            Self::Markdown => ik_ai::catalog::OutputMode::Markdown,
        }
    }

    /// Versioned payload identity includes both wire format and exact body.
    pub(crate) fn hash(self, content: &str) -> Vec<u8> {
        let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
        digest.update(self.as_str().as_bytes());
        digest.update(b"\0");
        digest.update(content.as_bytes());
        digest.finish().as_ref().to_vec()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Message<'a> {
    Plain(&'a str),
    Markdown(&'a str),
}

impl<'a> Message<'a> {
    pub fn new(format: MessageFormat, content: &'a str) -> Self {
        match format {
            MessageFormat::Plain => Self::Plain(content),
            MessageFormat::Markdown => Self::Markdown(content),
        }
    }
}
