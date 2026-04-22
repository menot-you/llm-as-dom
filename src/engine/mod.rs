//! Browser engine abstraction layer.
//!
//! Decouples the pilot, a11y, session, and network modules from any
//! specific browser engine (Chromium, WebKit, etc.).
//!
//! Wave 3: Chromium gained an `attach` constructor that connects to an
//! already-running browser via CDP (see `chromium_attach`). The trait
//! gained [`BrowserEngine::adopt_existing_pages`] so attach callers can
//! surface pre-existing tabs as LAD tabs on the first CDP handshake.

pub mod chromium;
pub mod chromium_attach;
pub mod cloak_bootstrap;
pub mod singleton_lock;
pub mod stealth;
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
    /// Handle to the temporary directory to ensure it is dropped when the engine is dropped.
    pub temp_dir: Option<std::sync::Arc<tempfile::TempDir>>,
    /// Browser window dimensions (width, height).
    pub window_size: (u32, u32),
}

impl Default for EngineConfig {
    fn default() -> Self {
        // FIX-R3-12: Use tempfile::Builder for cryptographically random directory
        // names with 0o700 permissions, replacing the predictable PID-based path.
        let td = tempfile::Builder::new()
            .prefix("lad-browser-")
            .tempdir()
            .ok();
        let user_data_dir = td
            .as_ref()
            .map(|t| t.path().to_path_buf())
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("lad-browser-{}", std::process::id()))
            });
        Self {
            visible: false,
            interactive: false,
            user_data_dir,
            temp_dir: td.map(std::sync::Arc::new),
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

    /// Wave 3: return any pages that already exist on the browser,
    /// wrapped as `PageHandle` trait objects. The default implementation
    /// returns an empty vec for engines that don't support adoption
    /// (WebKit, future backends). Chromium overrides this to enumerate
    /// `chromiumoxide::Browser::pages()` — used by the CDP attach path
    /// (`lad_session attach_cdp`) to surface the user's already-open
    /// tabs as LAD tabs on the first handshake.
    async fn adopt_existing_pages(&self) -> Result<Vec<Box<dyn PageHandle>>, crate::Error> {
        Ok(Vec::new())
    }
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

    /// Set files on a `<input type="file">` element via CDP or engine-native API.
    ///
    /// `selector` is a CSS selector identifying the file input.
    /// `files` are absolute file paths on the host filesystem.
    ///
    /// Default implementation returns an error — only engines with CDP
    /// or equivalent support override this.
    async fn set_input_files(
        &self,
        _selector: &str,
        _files: &[String],
    ) -> Result<(), crate::Error> {
        Err(crate::Error::Backend(
            "file upload not supported on this engine".into(),
        ))
    }

    /// Enable network traffic monitoring. Returns `false` if unsupported.
    async fn enable_network_monitoring(&self) -> Result<bool, crate::Error> {
        Ok(false)
    }

    /// Start timer-based monitoring.
    async fn start_monitoring(&self, _interval_ms: u32, _script: &str) -> Result<(), crate::Error> {
        Ok(())
    }

    /// Stop timer-based monitoring.
    async fn stop_monitoring(&self) -> Result<(), crate::Error> {
        Ok(())
    }

    /// Close the underlying page/target and release browser resources.
    ///
    /// **Default implementation is a noop** — used by engines whose page
    /// lifetime matches the engine lifetime (e.g. WebKit single-page bridge:
    /// no per-tab target exists; closing the engine releases everything).
    ///
    /// Chromium overrides this to issue `Target.closeTarget` via CDP. After
    /// a successful close, further operations on this handle will fail
    /// naturally (target gone on the Chrome side).
    ///
    /// **Asymmetry warning**: the default returns `Ok(())` without releasing
    /// anything, while the Chromium override returns `Ok(())` only after the
    /// target is freed. On WebKit, ephemeral pages spawned via `lad_audit`
    /// with `return_tab=false` will NOT be released by `close()` — they
    /// persist until the engine itself is shut down. Tracked as a follow-up:
    /// WebKit needs a target-pool abstraction before it can honor the
    /// release contract.
    ///
    /// Callers that produced an ephemeral page (e.g. `lad_audit` with
    /// `return_tab=false`) MUST invoke this before dropping the handle so
    /// that the Chrome target is released instead of leaking; do not rely on
    /// it as a hard release signal when the engine could be WebKit.
    async fn close(&mut self) -> Result<(), crate::Error> {
        Ok(())
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
    // DX-16 FIX: CDP may return the JSON as a string (from JSON.stringify)
    // or as an already-parsed object. Try direct deserialization first,
    // then fall back to parsing the string as JSON.
    match serde_json::from_value::<T>(value.clone()) {
        Ok(v) => Ok(v),
        Err(_) => {
            // If it's a string containing JSON, parse the string.
            if let Some(s) = value.as_str() {
                serde_json::from_str(s)
                    .map_err(|e| crate::Error::Backend(format!("JS result parse failed: {e:?}")))
            } else {
                Err(crate::Error::Backend(format!(
                    "JS result parse failed: expected string or object, got {value}"
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BUG-2: stub-based regression for the default `close()` impl.
    /// Stands in for the WebKit-shaped contract — no per-page target to
    /// release, just confirm the call returns `Ok(())` and never panics.
    /// The Chromium override is exercised by the `#[ignore]` integration
    /// test `test_chromium_page_close_releases_target`.
    struct StubPage;

    #[async_trait]
    impl PageHandle for StubPage {
        async fn eval_js(&self, _: &str) -> Result<serde_json::Value, crate::Error> {
            Ok(serde_json::Value::Null)
        }
        async fn navigate(&self, _: &str) -> Result<(), crate::Error> {
            Ok(())
        }
        async fn wait_for_navigation(&self) -> Result<(), crate::Error> {
            Ok(())
        }
        async fn url(&self) -> Result<String, crate::Error> {
            Ok(String::new())
        }
        async fn title(&self) -> Result<String, crate::Error> {
            Ok(String::new())
        }
        async fn screenshot_png(&self) -> Result<Vec<u8>, crate::Error> {
            Ok(Vec::new())
        }
        async fn cookies(&self) -> Result<Vec<crate::session::CookieEntry>, crate::Error> {
            Ok(Vec::new())
        }
        async fn set_cookies(&self, _: &[crate::session::CookieEntry]) -> Result<(), crate::Error> {
            Ok(())
        }
        // close, set_input_files, enable_network_monitoring, start_monitoring,
        // stop_monitoring all use trait defaults — that's the point.
    }

    #[tokio::test]
    async fn pagehandle_close_default_returns_ok() {
        let mut p = StubPage;
        let result = (&mut p as &mut dyn PageHandle).close().await;
        assert!(
            result.is_ok(),
            "default PageHandle::close() must return Ok(()), got {result:?}"
        );
    }
}
