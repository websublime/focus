//! The engine's error type.
//!
//! Upstream throws `new Error(message)` and its callers, tests and users see
//! only the message text, so the message is the contract. `EngineError` carries
//! exactly that.
//!
//! Where upstream throws a subclass of `Error` that carries more than its
//! message, for the one consumer that renders it (a guard's refusal, which
//! the tool's entry point prints with its recovery question), the error
//! carries that as a detail of the thrower's own type. Two errors are the
//! same error when their messages are: the detail is for whoever asks for
//! it by type.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

/// A failure whose message mirrors the text upstream would have thrown.
#[derive(Debug, Clone)]
pub struct EngineError(pub String, Option<Arc<dyn Any + Send + Sync>>);

impl PartialEq for EngineError {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for EngineError {}

impl EngineError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into(), None)
    }

    /// An error that carries `detail` beside its message.
    pub fn with_detail(message: impl Into<String>, detail: impl Any + Send + Sync) -> Self {
        Self(message.into(), Some(Arc::new(detail)))
    }

    pub fn message(&self) -> &str {
        &self.0
    }

    /// The detail the error carries, when it carries one of type `T`.
    #[must_use]
    pub fn detail<T: Any>(&self) -> Option<&T> {
        self.1.as_deref().and_then(|detail| detail.downcast_ref())
    }

    /// Upstream's `throw new Error(`${file}: ${errorMessage(err)}`)` wrapper.
    #[must_use]
    pub fn prefixed(self, prefix: impl fmt::Display) -> Self {
        Self(format!("{prefix}: {}", self.0), self.1)
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
