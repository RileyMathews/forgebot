use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum ForgejoError {
    Http(reqwest::Error),
    UnexpectedStatus {
        status: u16,
        reason: String,
        body: String,
    },
    Message(String),
}

impl fmt::Display for ForgejoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(err) => write!(f, "http error: {}", err),
            Self::UnexpectedStatus {
                status,
                reason,
                body,
            } => {
                write!(f, "forgejo api error: {} {} - {}", status, reason, body)
            }
            Self::Message(message) => write!(f, "{}", message),
        }
    }
}

impl Error for ForgejoError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Http(err) => Some(err),
            Self::UnexpectedStatus { .. } | Self::Message(_) => None,
        }
    }
}

impl From<reqwest::Error> for ForgejoError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

pub type Result<T> = std::result::Result<T, ForgejoError>;
