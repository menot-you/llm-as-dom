//! Chromium browser engine adapter.
//!
//! Wraps `chromiumoxide::Browser` and `chromiumoxide::Page` behind the
//! `BrowserEngine` / `PageHandle` traits.
//!
//! Wave 3: gained [`ChromiumEngine::attach`] — a sibling constructor
//! that connects to an already-running Chrome/Chromium/Brave/Edge/Opera
//! via its remote debugging port (CDP). The attach path lives in
//! [`crate::engine::chromium_attach`] to keep this file under the
//! 550-LOC clean-code cap.

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
    pub(super) browser: Arc<chromiumoxide::Browser>,
    pub(super) _handler: tokio::task::JoinHandle<()>,
    pub(super) _temp_dir: Option<std::sync::Arc<tempfile::TempDir>>,
    /// CHAOS-04: Set to `false` when the CDP event-stream handler exits,
    /// indicating Chrome has crashed or the WebSocket is dead.
    pub(super) alive: Arc<AtomicBool>,
    /// Wave 3: `true` when LAD launched the browser and owns the process,
    /// `false` when LAD attached to a user-owned Chrome via CDP. Controls
    /// whether [`ChromiumEngine::close`] is allowed to tear down the
    /// underlying `chromiumoxide::Browser` (which would kill the user's
    /// Chrome). Attach mode MUST NOT kill the user's browser on close.
    pub(super) owned: bool,
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
        //
        // Extra flags for hardened container hosts (e.g. Talos k3s nodes with SELinux
        // and/or LSM-level restrictions on namespace creation):
        //   --disable-setuid-sandbox: skip the SUID helper (not present in runner image)
        //   --no-zygote:              skip the zygote forking process
        //   --single-process:         run browser+renderer in one process — eliminates
        //                             every subprocess that would try to unshare() into
        //                             a new PID/network/user namespace. Without this,
        //                             Chromium dies at zygote_host_impl_linux.cc:207 with
        //                             "Failed to move to new namespace: Operation not
        //                             permitted" even when --no-zygote is passed.
        //   --disable-dev-shm-usage:  k8s caps /dev/shm at 64MB, avoid SIGBUS on larger
        //                             shared memory allocations.
        if should_disable_sandbox() {
            builder = builder
                .arg("--no-sandbox")
                .arg("--disable-setuid-sandbox")
                .arg("--no-zygote")
                .arg("--single-process")
                .arg("--disable-dev-shm-usage");
            tracing::info!("chromium sandbox/zygote disabled (container or LAD_NO_SANDBOX=true)");
        }

        let browser_config = builder.build().map_err(crate::Error::Browser)?;

        let (browser, handler) = chromiumoxide::Browser::launch(browser_config)
            .await
            .map_err(|e| crate::Error::Browser(format!("{e}")))?;

        let (alive, handle) = spawn_cdp_handler(handler);

        Ok(Self {
            browser: Arc::new(browser),
            _handler: handle,
            _temp_dir: config.temp_dir,
            alive,
            owned: true,
        })
    }
}

/// Wave 3: spawn the CDP event-stream drainer task shared by both
/// [`ChromiumEngine::launch`] and [`ChromiumEngine::attach`]. Returns
/// the liveness `AtomicBool` the engine tracks and the spawned task
/// handle. The task flips `alive` to `false` when the stream ends
/// (Chrome crashed, WS closed, user killed the browser) so every
/// subsequent `eval_js` call can fail fast with a clear error.
pub(super) fn spawn_cdp_handler<H>(mut handler: H) -> (Arc<AtomicBool>, tokio::task::JoinHandle<()>)
where
    H: futures::Stream + Unpin + Send + 'static,
{
    let alive = Arc::new(AtomicBool::new(true));
    let alive_clone = Arc::clone(&alive);
    let handle = tokio::spawn(async move {
        use futures::StreamExt;
        while handler.next().await.is_some() {}
        alive_clone.store(false, Ordering::Relaxed);
        tracing::error!("chromium CDP event stream ended — browser presumed dead");
    });
    (alive, handle)
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
        // Wave 3: attach mode (`owned: false`) keeps the user's Chrome
        // alive. We flip the liveness flag so stale `ChromiumPage`
        // handles fail fast on the next CDP call, but we do NOT call
        // `browser.close()` — the Drop on `chromiumoxide::Browser`
        // created via `connect` will not terminate the underlying
        // process when the outer `Arc` drops (there's nothing to
        // terminate; LAD never spawned it).
        //
        // Launch mode (`owned: true`) keeps the pre-Wave-3 behaviour:
        // dropping the browser via `Arc` drop triggers graceful
        // shutdown of the spawned Chromium. Don't set `alive = false`
        // here — launch-mode callers may still hold valid `ChromiumPage`
        // handles until the Arc is actually dropped (this is the case
        // when `LadServer` replaces the engine Arc shortly after).
        if !self.owned {
            self.alive.store(false, Ordering::Relaxed);
            tracing::info!("attached chromium engine closed — user browser left running");
        }
        Ok(())
    }

    async fn adopt_existing_pages(&self) -> Result<Vec<Box<dyn PageHandle>>, crate::Error> {
        // Wave 3: enumerate every `chromiumoxide::Page` the underlying
        // browser currently has open and wrap each into a `ChromiumPage`
        // so LAD's multi-tab map can adopt them. Only attach mode calls
        // this in practice (launch mode starts with zero pages) but the
        // implementation is engine-wide so `adopt_existing` can surface
        // tabs that launch mode inherited from a pre-existing user data
        // dir as well.
        let pages = self
            .browser
            .pages()
            .await
            .map_err(|e| crate::Error::Browser(format!("failed to list pages: {e}")))?;
        let alive = Arc::clone(&self.alive);
        Ok(pages
            .into_iter()
            .map(|page| {
                Box::new(ChromiumPage {
                    page,
                    alive: Arc::clone(&alive),
                }) as Box<dyn PageHandle>
            })
            .collect())
    }
}

/// Chromium-backed page handle.
pub(super) struct ChromiumPage {
    pub(super) page: chromiumoxide::Page,
    /// Shared liveness flag — mirrors `ChromiumEngine::alive`.
    pub(super) alive: Arc<AtomicBool>,
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
    ///
    /// Set cookies via CDP `Network.setCookies` instead of `document.cookie`
    /// assignment in JS. The JS approach silently drops HttpOnly cookies
    /// (cannot be set via `document.cookie` by spec), which broke profile
    /// cookie injection for sites like X that use HttpOnly `auth_token`
    /// for session state. CDP-level setting bypasses every restriction:
    /// HttpOnly, Secure, SameSite=None, cross-domain — all work.
    async fn set_cookies(
        &self,
        cookies: &[crate::session::CookieEntry],
    ) -> Result<(), crate::Error> {
        use chromiumoxide::cdp::browser_protocol::network::{
            CookieParam, CookieSameSite, SetCookiesParams, TimeSinceEpoch,
        };

        if cookies.is_empty() {
            return Ok(());
        }

        // Map our internal CookieEntry to CDP CookieParam. Domain must be
        // present (without leading dot is fine). If expires <= 0 the cookie
        // is a session cookie — omit the field entirely.
        let cdp_cookies: Vec<CookieParam> = cookies
            .iter()
            .filter(|c| !c.name.is_empty())
            .map(|c| {
                let domain = if c.domain.is_empty() {
                    None
                } else {
                    Some(c.domain.clone())
                };
                let path = if c.path.is_empty() {
                    Some("/".to_string())
                } else {
                    Some(c.path.clone())
                };
                let expires = if c.expires > 0.0 {
                    Some(TimeSinceEpoch::new(c.expires))
                } else {
                    None
                };
                let same_site = c.same_site.as_deref().and_then(|s| match s {
                    "Strict" => Some(CookieSameSite::Strict),
                    "Lax" => Some(CookieSameSite::Lax),
                    "None" => Some(CookieSameSite::None),
                    _ => None,
                });
                CookieParam {
                    name: c.name.clone(),
                    value: c.value.clone(),
                    url: None,
                    domain,
                    path,
                    secure: Some(c.secure),
                    http_only: Some(c.http_only),
                    same_site,
                    expires,
                    priority: None,
                    same_party: None,
                    source_scheme: None,
                    source_port: None,
                    partition_key: None,
                }
            })
            .collect();

        let total = cdp_cookies.len();
        let params = SetCookiesParams::new(cdp_cookies);

        let timeout = std::time::Duration::from_secs(EVAL_JS_TIMEOUT_SECS);
        match tokio::time::timeout(timeout, self.page.execute(params)).await {
            Ok(Ok(_)) => {
                tracing::info!(count = total, "injected cookies via CDP Network.setCookies");
                Ok(())
            }
            Ok(Err(e)) => {
                tracing::warn!(count = total, error = %e, "CDP setCookies failed");
                Err(cdp_err(e))
            }
            Err(_) => Err(crate::Error::Timeout {
                timeout_secs: EVAL_JS_TIMEOUT_SECS,
            }),
        }
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
