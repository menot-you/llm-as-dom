/// Unified error type for LLM-as-DOM.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("browser: {0}")]
    Browser(#[from] chromiumoxide::error::CdpError),

    #[error("browser: {0}")]
    BrowserStr(String),

    #[error("backend: {0}")]
    Backend(String),

    #[error("timeout")]
    Timeout,

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
