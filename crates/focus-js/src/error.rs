//! The engine's error type.
//!
//! Upstream throws `new Error(message)` and its callers, tests and users see
//! only the message text, so the message is the contract. `EngineError` carries
//! exactly that. Typed variants can be layered on per crate once a consumer
//! needs to branch on the kind of failure.

use std::fmt;

/// A failure whose message mirrors the text upstream would have thrown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError(pub String);

impl EngineError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    pub fn message(&self) -> &str {
        &self.0
    }

    /// Upstream's `throw new Error(`${file}: ${errorMessage(err)}`)` wrapper.
    #[must_use]
    pub fn prefixed(self, prefix: impl fmt::Display) -> Self {
        Self(format!("{prefix}: {}", self.0))
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EngineError {}

pub type Result<T> = std::result::Result<T, EngineError>;

/// `return Err(EngineError::new(format!(...)))`.
#[macro_export]
macro_rules! bail {
    ($($arg:tt)*) => {
        return ::core::result::Result::Err($crate::EngineError::new(format!($($arg)*)))
    };
}
