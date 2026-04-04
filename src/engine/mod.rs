//! Browser engine abstraction layer.
//!
//! Decouples the pilot, a11y, session, and network modules from any
//! specific browser engine (Chromium, WebKit, etc.).

pub mod chromium;
pub mod webkit;
pub(crate) mod webkit_proto;

use async_trait::async_trait;
use serde::de::DeserializeOwned;

/// Configuration for launching a browser engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Show the browser window (false = headless).
    pub visible: bool,
    /// Interactive mode: opens an app-mode window for human interaction.
    pub interactive: bool,
    /// User data directory for browser profile isolation.
    pub user_data_dir: std::path::PathBuf,
    /// Browser window dimensions (width, height).
    pub window_size: (u32, u32),
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            visible: false,
            interactive: false,
            user_data_dir: std::env::temp_dir().join(format!("lad-browser-{}", std::process::id())),
            window_size: (1280, 800),
        }
    }
}

/// A browser engine that can create pages.
#[async_trait]
pub trait BrowserEngine: Send + Sync {
    /// Open a new page/tab and navigate to the given URL.
    async fn new_page(&self, url: &str) -> Result<Box<dyn PageHandle>, crate::Error>;

    /// Human-readable engine name (e.g. "chromium", "webkit").
    fn name(&self) -> &str;

    /// Shut down the browser and release resources.
    async fn close(&self) -> Result<(), crate::Error>;
}

/// A page handle — the single abstraction over browser-specific page types.
///
/// Every method maps to one (or a small group) of browser API calls.
/// The trait is object-safe (no generic methods on required items).
#[async_trait]
pub trait PageHandle: Send + Sync {
    /// Evaluate JS and return the result as `serde_json::Value`.
    /// For void expressions, return `Value::Null`.
    async fn eval_js(&self, script: &str) -> Result<serde_json::Value, crate::Error>;

    /// Navigate to a URL.
    async fn navigate(&self, url: &str) -> Result<(), crate::Error>;

    /// Wait for navigation to complete after e.g. a click-triggered redirect.
    async fn wait_for_navigation(&self) -> Result<(), crate::Error>;

    /// Get the current page URL.
    async fn url(&self) -> Result<String, crate::Error>;

    /// Get the current page title.
    async fn title(&self) -> Result<String, crate::Error>;

    /// Full-page screenshot as PNG bytes.
    async fn screenshot_png(&self) -> Result<Vec<u8>, crate::Error>;

    /// Get cookies for the current page context via JS `document.cookie`.
    async fn cookies(&self) -> Result<Vec<crate::session::CookieEntry>, crate::Error>;

    /// Set cookies via JS `document.cookie` assignment.
    async fn set_cookies(
        &self,
        cookies: &[crate::session::CookieEntry],
    ) -> Result<(), crate::Error>;

    /// Enable network traffic monitoring. Returns `false` if unsupported.
    async fn enable_network_monitoring(&self) -> Result<bool, crate::Error> {
        Ok(false)
    }
}

/// Convenience: evaluate JS and deserialize into `T`.
///
/// Standalone function (not on trait) to keep `PageHandle` object-safe.
pub async fn eval_js_into<T: DeserializeOwned>(
    page: &dyn PageHandle,
    script: &str,
) -> Result<T, crate::Error> {
    let value = page.eval_js(script).await?;
    serde_json::from_value(value)
        .map_err(|e| crate::Error::Backend(format!("JS result parse failed: {e:?}")))
}
