use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum SessionError {
    UnknownAction(String),
    UnknownState(String),
    UnknownCloneStatus(String),
    Message(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAction(value) => write!(f, "unknown session action: {}", value),
            Self::UnknownState(value) => write!(f, "unknown session state: {}", value),
            Self::UnknownCloneStatus(value) => write!(f, "unknown clone status: {}", value),
            Self::Message(message) => write!(f, "{}", message),
        }
    }
}

impl Error for SessionError {}

pub type Result<T> = std::result::Result<T, SessionError>;
