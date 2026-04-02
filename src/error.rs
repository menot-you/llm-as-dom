/// Unified error type for the LLM-as-DOM browser pilot.
///
/// Covers browser/CDP failures, LLM backend errors, timeouts, and I/O.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// CDP error wrapped in a `Box` (for `From` on boxed variants).
    #[error("browser: {0}")]
    Browser(#[from] Box<chromiumoxide::error::CdpError>),

    /// CDP or browser-launch error represented as a string.
    #[error("browser: {0}")]
    BrowserStr(String),

    /// LLM backend error (request, parse, timeout).
    #[error("backend: {0}")]
    Backend(String),

    /// Operation exceeded the configured timeout.
    #[error("timeout")]
    Timeout,

    /// An action execution failed (element not found, stale DOM, etc.).
    #[error("action failed: {0}")]
    ActionFailed(String),

    /// Standard I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

// chromiumoxide returns bare `CdpError`, not `Box<CdpError>`.
// This impl lets `?` convert it automatically.
impl From<chromiumoxide::error::CdpError> for Error {
    fn from(e: chromiumoxide::error::CdpError) -> Self {
        Self::Browser(Box::new(e))
    }
}
