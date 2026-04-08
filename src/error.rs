/// Unified error type for the LLM-as-DOM browser pilot.
///
/// SS-2: Structured error hierarchy with typed variants for matching.
/// `Backend(String)` is kept as a migration fallback — new code should
/// prefer specific variants.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Browser engine error (CDP, WebKit protocol, etc.).
    #[error("engine error: {0}")]
    Engine(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// DOM extraction or a11y tree parsing failed.
    #[error("DOM extraction failed: {0}")]
    Dom(String),

    /// LLM backend error (request, parse, model error).
    #[error("LLM backend error: {0}")]
    Llm(String),

    /// Input sanitization error.
    #[error("sanitization error: {0}")]
    Sanitize(String),

    /// JS evaluation or navigation timed out.
    #[error("JS evaluation timed out after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },

    /// SSRF blocked: attempted navigation to a private/loopback/dangerous URL.
    #[error("SSRF blocked: {url}")]
    Ssrf { url: String },

    /// Navigation error (page load failure, redirect loop, etc.).
    #[error("navigation error: {0}")]
    Navigation(String),

    /// Browser or CDP error (stringified — engine-agnostic).
    /// Kept for backward compatibility during migration.
    #[error("browser: {0}")]
    Browser(String),

    /// LLM backend error (legacy — prefer `Llm` for new code).
    #[error("backend: {0}")]
    Backend(String),

    /// An action execution failed (element not found, stale DOM, etc.).
    #[error("action failed: {0}")]
    ActionFailed(String),

    /// Standard I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
