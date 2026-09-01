//! Structured error type + exit-code mapping for the AXI family convention.
//!
//! Exit codes: 0 = success (including partial), 1 = operational failure,
//! 2 = usage / validation error.

use std::fmt;

/// A structured CLI error carrying a message, `code`, and optional `help` suggestions.
#[derive(Debug)]
pub struct CmuxError {
    pub message: String,
    pub code: &'static str,
    pub suggestions: Vec<String>,
}

impl CmuxError {
    pub fn operational(message: impl Into<String>, code: &'static str) -> Self {
        CmuxError {
            message: message.into(),
            code,
            suggestions: Vec::new(),
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        CmuxError {
            message: message.into(),
            code: "VALIDATION_ERROR",
            suggestions: Vec::new(),
        }
    }

    pub fn with_suggestions(mut self, suggestions: Vec<String>) -> Self {
        self.suggestions = suggestions;
        self
    }

    /// 2 for validation/usage, 1 otherwise.
    pub fn exit_code(&self) -> i32 {
        if self.code == "VALIDATION_ERROR" {
            2
        } else {
            1
        }
    }
}

impl fmt::Display for CmuxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CmuxError {}

pub type Result<T> = std::result::Result<T, CmuxError>;
