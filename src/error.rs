/// Unified error type for the LLM-as-DOM browser pilot.
///
/// Covers browser failures, LLM backend errors, timeouts, and I/O.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Browser or CDP error (stringified — engine-agnostic).
    #[error("browser: {0}")]
    Browser(String),

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
