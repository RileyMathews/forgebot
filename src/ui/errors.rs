use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum UiError {
    InvalidInput(String),
    NotFound(String),
    Message(String),
}

impl fmt::Display for UiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(value) => write!(f, "invalid input: {}", value),
            Self::NotFound(value) => write!(f, "not found: {}", value),
            Self::Message(message) => write!(f, "{}", message),
        }
    }
}

impl Error for UiError {}

pub type Result<T> = std::result::Result<T, UiError>;
