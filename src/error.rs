//! A failure with a chosen process exit code
//!
//! Exit codes: 0 ok · 1 usage · 2 state conflict (e.g. no gate open) · 3 no such run.

use std::fmt;

#[derive(Debug)]
pub struct OttoError {
    pub message: String,
    pub code: i32,
}

impl OttoError {
    pub fn new(message: impl Into<String>, code: i32) -> Self {
        Self { message: message.into(), code }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(message, 1)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(message, 2)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(message, 3)
    }
}

impl OttoError {
    /// The same distinctions as the exit code, for a caller answering over HTTP.
    pub fn http_status(&self) -> u16 {
        match self.code {
            1 => 400,
            2 => 409,
            3 => 404,
            _ => 500,
        }
    }
}

impl fmt::Display for OttoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for OttoError {}

impl From<std::io::Error> for OttoError {
    fn from(err: std::io::Error) -> Self {
        OttoError::usage(err.to_string())
    }
}

impl From<serde_json::Error> for OttoError {
    fn from(err: serde_json::Error) -> Self {
        OttoError::usage(err.to_string())
    }
}
