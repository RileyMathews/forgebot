use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum WebhookError {
    MissingSignature,
    InvalidSignatureEncoding,
    InvalidSignature,
    InvalidPayload(String),
    Message(String),
}

impl fmt::Display for WebhookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSignature => write!(f, "missing signature header"),
            Self::InvalidSignatureEncoding => write!(f, "invalid signature header encoding"),
            Self::InvalidSignature => write!(f, "invalid signature"),
            Self::InvalidPayload(value) => write!(f, "invalid payload: {}", value),
            Self::Message(message) => write!(f, "{}", message),
        }
    }
}

impl Error for WebhookError {}

pub type Result<T> = std::result::Result<T, WebhookError>;
