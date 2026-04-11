//! Chromium browser engine adapter.
//!
//! Wraps `chromiumoxide::Browser` and `chromiumoxide::Page` behind the
//! `BrowserEngine` / `PageHandle` traits.

use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{BrowserEngine, EngineConfig, PageHandle};

/// Default timeout for JS evaluation via CDP (seconds).
const EVAL_JS_TIMEOUT_SECS: u64 = 30;

/// Maximum screenshot PNG size in bytes (5 MB). Beyond this we fall back
/// to a viewport-only screenshot to prevent OOM on extremely tall pages.
const MAX_SCREENSHOT_BYTES: usize = 5 * 1024 * 1024;

/// Chromium-backed browser engine.
pub struct ChromiumEngine {
    browser: Arc<chromiumoxide::Browser>,
    _handler: tokio::task::JoinHandle<()>,
    _temp_dir: Option<std::sync::Arc<tempfile::TempDir>>,
    /// CHAOS-04: Set to `false` when the CDP event-stream handler exits,
    /// indicating Chrome has crashed or the WebSocket is dead.
    alive: Arc<AtomicBool>,
}

impl ChromiumEngine {
    /// Launch a Chromium browser with the given configuration.
    pub async fn launch(config: EngineConfig) -> Result<Self, crate::Error> {
        // DX-SL1 (bug 1): Clean up stale Singleton{Lock,Socket,Cookie}
        // left behind by a crashed Chrome before launching. Without this the
        // second `Browser::launch` fails with "profile appears to be in use"
        // until the user `rm -rf`s the user-data-dir manually.
        super::singleton_lock::cleanup_stale_singleton_locks(&config.user_data_dir);

        let mut builder = chromiumoxide::BrowserConfig::builder();

        // Visible or interactive mode: show the browser window.
        if config.visible || config.interactive {
            builder = builder
                .with_head() // Disable chromiumoxide's default --headless flag.
                // DX-13: Disable viewport emulation in visible mode so the page
                // renders at the actual window size, not the default 800x600.
                .viewport(None)
                .arg("--app=about:blank")
                .arg("--disable-extensions")
                .arg("--disable-default-apps")
                .arg("--disable-component-extensions-with-background-pages")
                .arg("--disable-translate")
                .arg("--no-first-run")
                .arg("--no-default-browser-check")
                .arg(format!(
                    "--window-size={},{}",
                    config.window_size.0, config.window_size.1
                ));
        } else {
            builder = builder.arg(format!(
                "--window-size={},{}",
                config.window_size.0, config.window_size.1
            ));
        }

        // DX-SL1 (bug 1): Pass user_data_dir via the builder setter, NOT via
        // raw .arg(). chromiumoxide 0.7 unconditionally appends its own
        // `--user-data-dir=$TEMP/chromiumoxide-runner` default whenever
        // `self.user_data_dir` is None — that duplicates the flag, ignores
        // our per-launch temp dir, and means the singleton-lock cleanup
        // code was scrubbing the wrong directory all along.
        builder = builder
            .user_data_dir(&config.user_data_dir)
            .arg("--disable-dev-shm-usage");

        // STEALTH: Only disable GPU in headless mode. A real browser has WebGL
        // enabled — `--disable-gpu` causes `getContext('webgl')` to return null,
        // which is itself a bot-detection signal (no human user has WebGL off).
        // In visible/interactive mode we keep the GPU alive so our WebGL
        // vendor/renderer overrides in the stealth script can actually fire.
        if !config.visible && !config.interactive {
            builder = builder.arg("--disable-gpu");
        }

        // STEALTH: Flag-level anti-detection. Disables the AutomationControlled
        // Blink feature and prevents Chrome from exposing automation indicators
        // on startup. CDP-level JS patches in `stealth::apply_stealth` cover
        // the rest (webdriver, plugins, chrome object, WebGL, etc).
        for flag in super::stealth::STEALTH_FLAGS {
            builder = builder.arg(*flag);
        }

        // CLOAK: Resolve a pre-patched stealth Chromium binary (CloakBrowser)
        // and point chromiumoxide at it. CloakBrowser ships 49 C++-level
        // fingerprint patches that defeat JS-layer detectors like Creepjs's
        // `hasToStringProxy` cascade. Falls back to chromiumoxide's default
        // Chromium detection when disabled or unsupported on this platform.
        match super::cloak_bootstrap::resolve_cloak_binary() {
            Ok(Some(cloak_path)) => {
                tracing::info!(path = %cloak_path.display(), "using cloakbrowser stealth binary");
                builder = builder.chrome_executable(&cloak_path);
            }
            Ok(None) => {
                tracing::debug!("cloakbrowser disabled — using default Chromium");
            }
            Err(e) => {
                tracing::warn!(error = %e, "cloakbrowser resolution failed — falling back to default Chromium");
            }
        }

        // FIX-R3-10: Only disable sandbox when explicitly requested or running in a container.
        // --no-sandbox is a significant security reduction; only enable when necessary.
        if should_disable_sandbox() {
            builder = builder.arg("--no-sandbox");
            tracing::info!("chromium sandbox disabled (container or LAD_NO_SANDBOX=true)");
        }

        let browser_config = builder.build().map_err(crate::Error::Browser)?;

        let (browser, mut handler) = chromiumoxide::Browser::launch(browser_config)
            .await
            .map_err(|e| crate::Error::Browser(format!("{e}")))?;

        let alive = Arc::new(AtomicBool::new(true));
        let alive_clone = Arc::clone(&alive);

        let handle = tokio::spawn(async move {
            use futures::StreamExt;
            while handler.next().await.is_some() {}
            // CHAOS-04: CDP stream ended — Chrome crashed or WS closed.
            alive_clone.store(false, Ordering::Relaxed);
            tracing::error!("chromium CDP event stream ended — browser presumed dead");
        });

        Ok(Self {
            browser: Arc::new(browser),
            _handler: handle,
            _temp_dir: config.temp_dir,
            alive,
        })
    }
}

#[async_trait]
impl BrowserEngine for ChromiumEngine {
    async fn new_page(&self, url: &str) -> Result<Box<dyn PageHandle>, crate::Error> {
        // STEALTH: Create a blank page first so we can install UA override and
        // document-load script BEFORE the real URL navigation happens. If we
        // navigated directly via `new_page(url)`, the target site's detection
        // code would run against an unpatched navigator.
        let page = self
            .browser
            .new_page("about:blank")
            .await
            .map_err(cdp_err)?;

        // JS stealth is OFF by default. Empirical validation on 2026-04-11
        // showed CloakBrowser (Chromium 145 with 49 C++ fingerprint patches)
        // scores 0% Headless / 0% Stealth on Creepjs when running alone.
        // Adding our JS stealth layer REGRESSES scores to 33% / 20%
        // because Creepjs's lies module detects our
        // Function.prototype.toString proxy as hasToStringProxy:true, which
        // cascades via detectProxies mode to flag Navigator.webdriver as
        // a lie even though CloakBrowser already handles it at C++ level.
        //
        // Opt-in: LAD_USE_JS_STEALTH=1 for users running with
        // LAD_CLOAK_DISABLE=1 or a platform without a CloakBrowser binary.
        let use_js_stealth = std::env::var("LAD_USE_JS_STEALTH")
            .ok()
            .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"));
        if use_js_stealth {
            tracing::info!("LAD_USE_JS_STEALTH=1 — applying JS stealth on top of engine");
            super::stealth::apply_stealth(&page).await?;
        }

        if !url.is_empty() && url != "about:blank" {
            page.goto(url).await.map_err(cdp_err)?;
        }

        Ok(Box::new(ChromiumPage {
            page,
            alive: Arc::clone(&self.alive),
        }))
    }

    fn name(&self) -> &str {
        "chromium"
    }

    async fn close(&self) -> Result<(), crate::Error> {
        // Dropping the browser triggers graceful shutdown.
        // The handler task will end when the event stream closes.
        Ok(())
    }
}

/// Chromium-backed page handle.
struct ChromiumPage {
    page: chromiumoxide::Page,
    /// Shared liveness flag — mirrors `ChromiumEngine::alive`.
    alive: Arc<AtomicBool>,
}

#[async_trait]
impl PageHandle for ChromiumPage {
    async fn eval_js(&self, script: &str) -> Result<serde_json::Value, crate::Error> {
        // CHAOS-04: Fail fast if Chrome/CDP is dead.
        if !self.alive.load(Ordering::Relaxed) {
            return Err(crate::Error::Browser(
                "chromium CDP connection is dead — browser may have crashed".into(),
            ));
        }

        // CHAOS-02: Wrap every CDP evaluate call in a timeout to prevent
        // hostile JS (e.g. `while(true){}`) from freezing the MCP session.
        let timeout = std::time::Duration::from_secs(EVAL_JS_TIMEOUT_SECS);
        match tokio::time::timeout(timeout, self.page.evaluate(script)).await {
            Ok(Ok(eval_result)) => {
                // Try to extract a Value; void expressions fail here.
                match eval_result.into_value::<serde_json::Value>() {
                    Ok(v) => Ok(v),
                    Err(_) => Ok(serde_json::Value::Null),
                }
            }
            Ok(Err(e)) => Err(cdp_err(e)),
            Err(_) => Err(crate::Error::Timeout {
                timeout_secs: EVAL_JS_TIMEOUT_SECS,
            }),
        }
    }

    async fn navigate(&self, url: &str) -> Result<(), crate::Error> {
        self.page.goto(url).await.map_err(cdp_err)?;
        Ok(())
    }

    async fn wait_for_navigation(&self) -> Result<(), crate::Error> {
        self.page.wait_for_navigation().await.map_err(cdp_err)?;
        Ok(())
    }

    async fn url(&self) -> Result<String, crate::Error> {
        Ok(self
            .page
            .url()
            .await
            .map_err(cdp_err)?
            .unwrap_or_else(|| "unknown".into()))
    }

    async fn title(&self) -> Result<String, crate::Error> {
        Ok(self
            .page
            .get_title()
            .await
            .map_err(cdp_err)?
            .unwrap_or_default())
    }

    async fn screenshot_png(&self) -> Result<Vec<u8>, crate::Error> {
        // CHAOS-01: Use viewport-only screenshots to prevent OOM on
        // extremely tall pages (50,000px+ = 100s of MB as PNG).
        let params = chromiumoxide::page::ScreenshotParams::builder().build();
        let png = self.page.screenshot(params).await.map_err(cdp_err)?;

        if png.len() > MAX_SCREENSHOT_BYTES {
            tracing::warn!(
                bytes = png.len(),
                cap = MAX_SCREENSHOT_BYTES,
                "screenshot exceeds size cap — returning viewport-only"
            );
            // Already viewport-only; just truncation-warn. Future: resize.
        }

        Ok(png)
    }

    async fn cookies(&self) -> Result<Vec<crate::session::CookieEntry>, crate::Error> {
        let js = r#"
            (() => {
                const url = window.location.href;
                const hostname = window.location.hostname;
                const pathname = window.location.pathname;
                return JSON.stringify({
                    url: url,
                    hostname: hostname,
                    pathname: pathname,
                    cookies: document.cookie.split(';').map(c => {
                        const [name, ...rest] = c.trim().split('=');
                        return { name: name || '', value: rest.join('=') || '' };
                    }).filter(c => c.name.length > 0)
                });
            })()
        "#;

        let timeout = std::time::Duration::from_secs(EVAL_JS_TIMEOUT_SECS);
        let result: String = tokio::time::timeout(timeout, self.page.evaluate(js))
            .await
            .map_err(|_| crate::Error::Timeout {
                timeout_secs: EVAL_JS_TIMEOUT_SECS,
            })?
            .map_err(cdp_err)?
            .into_value()
            .map_err(|e| crate::Error::ActionFailed(e.to_string()))?;

        let parsed: serde_json::Value =
            serde_json::from_str(&result).map_err(|e| crate::Error::ActionFailed(e.to_string()))?;

        let hostname = parsed["hostname"].as_str().unwrap_or_default();
        let pathname = parsed["pathname"].as_str().unwrap_or("/");

        let cookies: Vec<crate::session::CookieEntry> = parsed["cookies"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| {
                        let name = c["name"].as_str()?.to_string();
                        let value = c["value"].as_str().unwrap_or_default().to_string();
                        Some(crate::session::CookieEntry {
                            name,
                            value,
                            domain: hostname.to_string(),
                            path: pathname.to_string(),
                            expires: 0.0,
                            secure: false,
                            http_only: false,
                            same_site: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        tracing::debug!(count = cookies.len(), "extracted cookies via JS");
        Ok(cookies)
    }

    /// FIX-R3-13: Cookie values are NEVER logged. We log only the count.
    /// The JS expression sent to `page.evaluate` contains cookie values but
    /// chromiumoxide does not log evaluate expressions at info/warn level.
    /// If RUST_LOG includes chromiumoxide=debug, CDP traffic may expose values —
    /// avoid debug-level logging for chromiumoxide in production.
    async fn set_cookies(
        &self,
        cookies: &[crate::session::CookieEntry],
    ) -> Result<(), crate::Error> {
        for cookie in cookies {
            let mut parts = vec![format!(
                "{}={}",
                crate::pilot::js_escape(&cookie.name),
                crate::pilot::js_escape(&cookie.value)
            )];

            if !cookie.domain.is_empty() {
                parts.push(format!("domain={}", cookie.domain));
            }
            if !cookie.path.is_empty() {
                parts.push(format!("path={}", cookie.path));
            }
            if cookie.expires > 0.0 {
                parts.push(format!("expires={}", cookie.expires));
            }
            if cookie.secure {
                parts.push("secure".to_string());
            }
            if let Some(ref ss) = cookie.same_site {
                parts.push(format!("samesite={ss}"));
            }

            let cookie_str = parts.join("; ");
            let js = format!(
                "document.cookie = '{}'",
                crate::pilot::js_escape(&cookie_str)
            );
            let timeout = std::time::Duration::from_secs(EVAL_JS_TIMEOUT_SECS);
            let _ = tokio::time::timeout(timeout, self.page.evaluate(js))
                .await
                .map_err(|_| crate::Error::Timeout {
                    timeout_secs: EVAL_JS_TIMEOUT_SECS,
                })?
                .map_err(cdp_err)?;
        }

        tracing::debug!(count = cookies.len(), "injected cookies via JS");
        Ok(())
    }

    async fn set_input_files(&self, selector: &str, files: &[String]) -> Result<(), crate::Error> {
        use chromiumoxide::cdp::browser_protocol::dom::SetFileInputFilesParams;

        let element = self
            .page
            .find_element(selector)
            .await
            .map_err(|e| crate::Error::ActionFailed(format!("element not found: {e}")))?;

        let cmd = SetFileInputFilesParams::builder()
            .files(files.iter().map(String::as_str))
            .backend_node_id(element.backend_node_id)
            .build()
            .map_err(|e| crate::Error::ActionFailed(format!("CDP command build failed: {e}")))?;

        self.page.execute(cmd).await.map_err(|e| {
            crate::Error::ActionFailed(format!("CDP setFileInputFiles failed: {e}"))
        })?;

        Ok(())
    }

    async fn enable_network_monitoring(&self) -> Result<bool, crate::Error> {
        use chromiumoxide::cdp::browser_protocol::network::EnableParams;
        self.page
            .execute(EnableParams::default())
            .await
            .map_err(|e| {
                crate::Error::ActionFailed(format!("failed to enable network tracking: {e}"))
            })?;
        tracing::debug!("network tracking enabled");
        Ok(true)
    }
}

/// FIX-R3-10: Determine whether `--no-sandbox` should be passed to Chromium.
///
/// Returns `true` when the `LAD_NO_SANDBOX` env var is explicitly set to `true`/`1`,
/// or when running inside a Docker/containerd container (auto-detected via
/// `/.dockerenv` or `/proc/1/cgroup`).
fn should_disable_sandbox() -> bool {
    if std::env::var("LAD_NO_SANDBOX").is_ok_and(|v| v == "true" || v == "1") {
        return true;
    }
    // Auto-detect container environment
    if std::path::Path::new("/.dockerenv").exists() {
        return true;
    }
    std::fs::read_to_string("/proc/1/cgroup")
        .is_ok_and(|s| s.contains("docker") || s.contains("containerd"))
}

/// Convert a CDP error to our unified error type.
fn cdp_err(e: chromiumoxide::error::CdpError) -> crate::Error {
    crate::Error::Browser(e.to_string())
}
