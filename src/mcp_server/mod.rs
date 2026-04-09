//! `llm-as-dom-mcp`: MCP server exposing the browser pilot as semantic tools.
//!
//! Provides 25 tools: `lad_browse`, `lad_extract`, `lad_assert`, `lad_locate`,
//! `lad_audit`, `lad_session`, `lad_snapshot`, `lad_click`, `lad_type`, `lad_select`,
//! `lad_eval`, `lad_close`, `lad_press_key`, `lad_back`, `lad_screenshot`,
//! `lad_wait`, `lad_network`, `lad_hover`, `lad_dialog`, `lad_upload`, `lad_scroll`,
//! `lad_fill_form`, `lad_refresh`, `lad_clear`, `lad_watch`.

mod assertions;
mod helpers;
mod params;
mod state;
mod tools;

use helpers::{mcp_err, parse_window_size_env, read_env_with_fallback, same_origin};
use params::*;
use state::{ActivePage, McpSessionState};

use llm_as_dom::engine::chromium::ChromiumEngine;
use llm_as_dom::engine::webkit::WebKitEngine;
use llm_as_dom::engine::{BrowserEngine, EngineConfig, PageHandle};
use llm_as_dom::{a11y, backend, pilot, semantic, watch};

use std::sync::Arc;
use tokio::sync::Mutex;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::service::{RequestContext, ServiceExt};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};

// ── Server state ───────────────────────────────────────────────────

// FIX-R3-03: Lock ordering contract — to prevent deadlocks when multiple
// tools execute concurrently, always acquire locks in this order:
//
//   1. engine
//   2. active_page
//   3. session
//   4. watch_state
//   5. peer
//
// Never hold a higher-numbered lock while acquiring a lower-numbered one.

/// MCP server that manages a headless browser and exposes pilot tools.
#[derive(Clone)]
#[allow(dead_code)] // tool_router is used internally by rmcp macros
struct LadServer {
    /// Router that dispatches MCP tool calls to handler methods.
    tool_router: ToolRouter<Self>,
    /// Shared browser engine (lazy-initialised on first tool call).
    pub(crate) engine: Arc<Mutex<Option<Arc<dyn BrowserEngine>>>>,
    /// LLM API base URL (Ollama, Z.AI, or any compatible endpoint).
    pub(crate) llm_url: String,
    /// LLM model name.
    pub(crate) llm_model: String,
    /// Session state carried across tool calls within this MCP session.
    pub(crate) session: Arc<Mutex<McpSessionState>>,
    /// Whether interactive mode is enabled (captcha pause for human).
    pub(crate) interactive: bool,
    /// Persistent page from `lad_snapshot`, reused by click/type/select.
    pub(crate) active_page: Arc<Mutex<Option<ActivePage>>>,
    /// Active watch state (at most one watch at a time).
    pub(crate) watch_state: Arc<Mutex<Option<watch::WatchState>>>,
    /// MCP peer for server-initiated push notifications.
    pub(crate) peer: Arc<Mutex<Option<rmcp::Peer<rmcp::service::RoleServer>>>>,
}

impl LadServer {
    /// Create a new server reading config from environment variables.
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            engine: Arc::new(Mutex::new(None)),
            llm_url: read_env_with_fallback(
                "LAD_LLM_URL",
                "LAD_OLLAMA_URL",
                "http://localhost:11434",
            ),
            llm_model: read_env_with_fallback("LAD_LLM_MODEL", "LAD_MODEL", "qwen2.5:7b"),
            session: Arc::new(Mutex::new(McpSessionState::default())),
            interactive: std::env::var("LAD_INTERACTIVE")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            active_page: Arc::new(Mutex::new(None)),
            watch_state: Arc::new(Mutex::new(None)),
            peer: Arc::new(Mutex::new(None)),
        }
    }

    /// Return an existing engine or launch a new one.
    /// If `request_visible` differs from the current mode, restart the engine.
    pub(crate) async fn ensure_engine_visible(
        &self,
        request_visible: bool,
    ) -> Result<Arc<dyn BrowserEngine>, llm_as_dom::Error> {
        let mut engine_lock = self.engine.lock().await;
        let need_restart = if engine_lock.is_some() {
            request_visible != self.interactive
        } else {
            false
        };
        if need_restart {
            tracing::info!(
                from = self.interactive,
                to = request_visible,
                "visibility changed — restarting browser"
            );
            // Drop old engine + active page.
            *engine_lock = None;
            *self.active_page.lock().await = None;
        }
        // SAFETY: we cast away the & to mutate interactive. This is fine because
        // we hold the engine lock and no other code reads interactive concurrently.
        #[allow(invalid_reference_casting)]
        if request_visible != self.interactive {
            let self_mut = unsafe { &mut *(self as *const Self as *mut Self) };
            self_mut.interactive = request_visible;
        }
        self.ensure_engine_inner(&mut engine_lock).await
    }

    /// Return an existing engine or launch a new one.
    pub(crate) async fn ensure_engine(&self) -> Result<Arc<dyn BrowserEngine>, llm_as_dom::Error> {
        let mut engine_lock = self.engine.lock().await;
        self.ensure_engine_inner(&mut engine_lock).await
    }

    async fn ensure_engine_inner(
        &self,
        engine_lock: &mut tokio::sync::MutexGuard<'_, Option<Arc<dyn BrowserEngine>>>,
    ) -> Result<Arc<dyn BrowserEngine>, llm_as_dom::Error> {
        if let Some(e) = engine_lock.as_ref() {
            return Ok(Arc::clone(e));
        }

        let mode = if self.interactive {
            "interactive (visible)"
        } else {
            "headless"
        };
        tracing::info!(mode, "launching browser");

        // FIX-R3-12: Use tempfile::Builder for cryptographically random temp dir
        // instead of predictable PID-based path.
        let td = tempfile::Builder::new()
            .prefix("lad-chrome-")
            .tempdir()
            .ok();
        let user_data_dir = td
            .as_ref()
            .map(|t| t.path().to_path_buf())
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("lad-chrome-{}", std::process::id()))
            });

        let config = EngineConfig {
            visible: self.interactive,
            interactive: self.interactive,
            user_data_dir,
            temp_dir: td.map(std::sync::Arc::new),
            // DX-5: Window size from LAD_WINDOW_SIZE env var ("WIDTHxHEIGHT"),
            // or defaults: 1440x900 visible, 1280x800 headless.
            window_size: parse_window_size_env().unwrap_or(if self.interactive {
                (1440, 900)
            } else {
                (1280, 800)
            }),
        };

        let engine_name = std::env::var("LAD_ENGINE").unwrap_or_default();
        let e: Arc<dyn BrowserEngine> = if engine_name == "webkit" {
            Arc::new(WebKitEngine::launch(config).await?)
        } else {
            Arc::new(ChromiumEngine::launch(config).await?)
        };
        **engine_lock = Some(Arc::clone(&e));
        Ok(e)
    }

    /// Navigate to a URL and return the page handle with its semantic view.
    pub(crate) async fn navigate_and_extract(
        &self,
        url: &str,
    ) -> Result<(Box<dyn PageHandle>, semantic::SemanticView), rmcp::ErrorData> {
        // FIX-4: SSRF gate — block file://, javascript:, data:, private IPs.
        if !llm_as_dom::sanitize::is_safe_url(url) {
            return Err(mcp_err(format!("blocked: unsafe URL '{url}'")));
        }
        let engine = self.ensure_engine().await.map_err(mcp_err)?;

        // FIX-5: Navigate to target URL FIRST, then inject cookies, then reload.
        // `about:blank` has null origin and cannot set cross-origin cookies via
        // `document.cookie`. We must be on the target origin for cookie injection.
        let page = engine.new_page(url).await.map_err(mcp_err)?;
        page.wait_for_navigation().await.map_err(mcp_err)?;

        // FIX-R4-01: Post-redirect SSRF validation. Check final URL after
        // the browser may have followed redirects through an open redirect.
        let final_url = page.url().await.map_err(mcp_err)?;
        if !llm_as_dom::sanitize::is_safe_url(&final_url) {
            return Err(mcp_err(format!(
                "blocked: redirected to unsafe URL {final_url}"
            )));
        }

        // Inject cookies on the correct origin, then reload to apply them.
        let has_cookies = self.has_profile_cookies();
        if has_cookies {
            self.inject_profile_cookies(page.as_ref()).await;
            page.navigate(&final_url).await.map_err(mcp_err)?;
            page.wait_for_navigation().await.map_err(mcp_err)?;

            let reloaded_url = page.url().await.map_err(mcp_err)?;
            if !llm_as_dom::sanitize::is_safe_url(&reloaded_url) {
                return Err(mcp_err(format!(
                    "blocked: redirected to unsafe URL {reloaded_url}"
                )));
            }
        }

        a11y::wait_for_content(page.as_ref(), a11y::DEFAULT_WAIT_TIMEOUT)
            .await
            .map_err(mcp_err)?;

        // DX-W3-4: Auto-install dialog overrides on every new page so unexpected
        // alert/confirm/prompt dialogs don't hang the page. Default: auto-accept.
        // `lad_dialog(action="dismiss")` can change the behavior at runtime.
        Self::inject_dialog_overrides(page.as_ref()).await;

        let view = a11y::extract_semantic_view(page.as_ref())
            .await
            .map_err(mcp_err)?;
        Ok((page, view))
    }

    /// DX-W3-4: Inject dialog auto-accept JS on a page.
    ///
    /// Overrides `window.alert`, `window.confirm`, `window.prompt` to
    /// auto-accept by default and capture dialog history. Idempotent.
    async fn inject_dialog_overrides(page: &dyn PageHandle) {
        let js = r#"
            if (!window.__lad_dialogs) {
                window.__lad_dialogs = [];
                window.__lad_dialog_auto = 'accept';
                window.__lad_dialog_response = '';

                window.alert = function(msg) {
                    window.__lad_dialogs.push({
                        type: 'alert', message: String(msg),
                        timestamp: Date.now()
                    });
                };
                window.confirm = function(msg) {
                    window.__lad_dialogs.push({
                        type: 'confirm', message: String(msg),
                        timestamp: Date.now()
                    });
                    return window.__lad_dialog_auto === 'accept';
                };
                window.prompt = function(msg, def) {
                    window.__lad_dialogs.push({
                        type: 'prompt', message: String(msg),
                        default: def || '', timestamp: Date.now()
                    });
                    if (window.__lad_dialog_auto !== 'accept') return null;
                    return window.__lad_dialog_response || def || '';
                };
            }
        "#;
        if let Err(e) = page.eval_js(js).await {
            tracing::warn!(error = %e, "failed to inject dialog overrides");
        }
    }

    /// Navigate to a URL (or reuse the active page if same origin), returning
    /// a fresh semantic view. Stores the result in `active_page`.
    ///
    /// FIX-R3-01: Eliminated TOCTOU race. Previously the lock was dropped and
    /// reacquired between the same-origin check and the write-back, allowing a
    /// concurrent call to mutate state. Now the lock is held for the reuse path
    /// and only released for the fresh-navigation path (which needs `navigate_and_extract`
    /// to acquire `engine` without nesting locks).
    pub(crate) async fn navigate_or_reuse(
        &self,
        url: &str,
    ) -> Result<semantic::SemanticView, rmcp::ErrorData> {
        // FIX-4: SSRF gate — block file://, javascript:, data:, private IPs.
        if !llm_as_dom::sanitize::is_safe_url(url) {
            return Err(mcp_err(format!("blocked: unsafe URL '{url}'")));
        }
        let mut active = self.active_page.lock().await;

        // Reuse existing page if same origin — hold the lock through the entire operation.
        if let Some(ap) = active.as_ref()
            && same_origin(&ap.url, url)
        {
            if ap.url != url {
                ap.page.navigate(url).await.map_err(mcp_err)?;
                ap.page.wait_for_navigation().await.map_err(mcp_err)?;

                let final_url = ap.page.url().await.map_err(mcp_err)?;
                // FIX-R8-01: Invalidate active_page on SSRF detection.
                if !llm_as_dom::sanitize::is_safe_url(&final_url) {
                    *active = None;
                    return Err(mcp_err(format!(
                        "blocked: redirected to unsafe URL {final_url}"
                    )));
                }

                a11y::wait_for_content(ap.page.as_ref(), a11y::DEFAULT_WAIT_TIMEOUT)
                    .await
                    .map_err(mcp_err)?;
            }
            let view = a11y::extract_semantic_view(ap.page.as_ref())
                .await
                .map_err(mcp_err)?;
            // FIX-R7-02: Store the ACTUAL browser URL, not the requested URL.
            // After redirects (e.g. http->https), `url` is stale. Using the
            // browser's real URL prevents same-origin misclassification on the
            // next call.
            let actual_url = ap.page.url().await.map_err(mcp_err)?;
            let mut ap_owned = active.take().unwrap();
            ap_owned.url = actual_url;
            ap_owned.view = view.clone();
            *active = Some(ap_owned);
            return Ok(view);
        }

        // Different origin or no active page — must release the lock before calling
        // navigate_and_extract (which acquires the engine lock). Then reacquire once
        // to store the result. This is safe because we're creating a fresh page.
        drop(active);
        let (page, view) = self.navigate_and_extract(url).await?;
        // FIX-R7-02: Store the ACTUAL browser URL after navigation + redirects.
        let actual_url = page.url().await.map_err(mcp_err)?;
        let mut active = self.active_page.lock().await;
        *active = Some(ActivePage {
            page,
            url: actual_url,
            view: view.clone(),
        });
        Ok(view)
    }

    /// Re-extract semantic view from the active page and update stored state.
    ///
    /// FIX-R6-02: Also syncs `ap.url` with the actual browser URL after every
    /// refresh. Without this, `ActivePage.url` could hold the *requested* URL
    /// while the browser had followed a redirect (e.g. http->https), causing
    /// `navigate_or_reuse` to misclassify same-origin pages and reopen them.
    ///
    /// FIX-R7-01: SSRF chokepoint — every tool calls `refresh_active_view` after
    /// every interaction. By checking the URL here, delayed navigations via
    /// `setTimeout(() => location = "http://127.0.0.1", 500)` are caught even
    /// if they slip past the per-tool SSRF checks (which only sample once after
    /// a short delay). This is the SINGLE defense-in-depth bottleneck.
    pub(crate) async fn refresh_active_view(
        &self,
    ) -> Result<semantic::SemanticView, rmcp::ErrorData> {
        let mut active = self.active_page.lock().await;
        let ap = active.as_mut().ok_or_else(helpers::no_active_page)?;

        // Sync URL with actual browser URL (handles redirects, click-driven navs)
        let current_url = ap.page.url().await.map_err(mcp_err)?;

        // FIX-R7-01: SSRF gate on EVERY refresh — catches delayed navigations
        // that evade per-tool checks (e.g. setTimeout-based location changes).
        // FIX-R8-01: Invalidate active_page BEFORE returning the SSRF error.
        // Without this, subsequent tools (screenshot, eval) still operate on
        // the unsafe page because `active_page` remains populated.
        if !llm_as_dom::sanitize::is_safe_url(&current_url) {
            let redacted = llm_as_dom::sanitize::redact_url_secrets(&current_url);
            *active = None;
            return Err(mcp_err(format!(
                "blocked: page navigated to unsafe URL {redacted}",
            )));
        }

        ap.url = current_url;

        let view = a11y::extract_semantic_view(ap.page.as_ref())
            .await
            .map_err(mcp_err)?;
        ap.view = view.clone();
        Ok(view)
    }

    /// FIX-5: Check if Chrome profile cookies are configured (non-async).
    pub(crate) fn has_profile_cookies(&self) -> bool {
        std::env::var("LAD_CHROME_PROFILE")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    /// Inject cookies from the user's Chrome profile if `LAD_CHROME_PROFILE` is set.
    pub(crate) async fn inject_profile_cookies(&self, page: &dyn PageHandle) {
        let profile_name = match std::env::var("LAD_CHROME_PROFILE") {
            Ok(name) if !name.is_empty() => name,
            _ => return,
        };

        let profile_path = match llm_as_dom::profile::resolve_profile_path(&profile_name) {
            Some(p) => p,
            None => {
                tracing::warn!(profile = %profile_name, "Chrome profile not found");
                return;
            }
        };

        match llm_as_dom::profile::extract_cookies_from_profile(&profile_path) {
            Ok(cookies) => {
                tracing::info!(count = cookies.len(), "injecting Chrome profile cookies");
                let _ = page.set_cookies(&cookies).await;
            }
            Err(e) => tracing::warn!(error = %e, "failed to load Chrome profile cookies"),
        }
    }

    /// FIX-9: Delegate to the canonical factory in `backend::create_backend`.
    pub(crate) fn create_backend(
        url: &str,
        model: &str,
        max_prompt_length: Option<usize>,
    ) -> Box<dyn pilot::PilotBackend> {
        backend::create_backend(url, model, max_prompt_length)
    }
}

// ── Tool router ──────────────────────────────────────────────────────

#[tool_router]
impl LadServer {
    #[tool(
        description = "Navigate to a URL and accomplish a browsing goal autonomously (login, fill form, click, search). Returns structured result."
    )]
    async fn lad_browse(
        &self,
        params: Parameters<BrowseParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_browse(params).await
    }

    #[tool(
        description = "Extract structured info from a page: interactive elements, text, page type. Never returns raw HTML. URL is optional — omit to extract from current page without navigating (preserves session state)."
    )]
    async fn lad_extract(
        &self,
        params: Parameters<ExtractParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_extract(params).await
    }

    #[tool(
        description = "Check assertions on a page. Returns pass/fail for each. URL is optional — omit to assert against the current page without navigating (preserves session state). Supports: has login form, title contains X, has button Y, url contains Z."
    )]
    async fn lad_assert(
        &self,
        params: Parameters<AssertParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_assert(params).await
    }

    #[tool(
        description = "Map a DOM element back to its source file. Checks React dev source, data-ds, data-lad attributes. Returns source file/line or DOM path fallback."
    )]
    async fn lad_locate(
        &self,
        params: Parameters<LocateParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_locate(params).await
    }

    #[tool(
        description = "Audit a URL for quality issues: a11y (alt text, labels, lang), forms (autocomplete, minlength), links (void hrefs, noopener). Returns issues with severity and fix suggestions."
    )]
    async fn lad_audit(
        &self,
        params: Parameters<AuditParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_audit(params).await
    }

    #[tool(
        description = "View or reset MCP session state: auth status, visited URLs, browse count. Actions: 'get' or 'clear'."
    )]
    async fn lad_session(
        &self,
        params: Parameters<SessionParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_session(params).await
    }

    #[tool(
        description = "Watch page state over time. Actions: 'start' begins polling a URL at interval_ms, diffing semantic views each cycle. 'events' returns captured diffs (pass since_seq for cursor-based pagination). 'stop' ends the watch."
    )]
    async fn lad_watch(
        &self,
        params: Parameters<WatchParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_watch(params).await
    }

    #[tool(
        description = "Get a structured semantic snapshot of the current page. Returns elements with IDs that can be used with lad_click/lad_type. URL is optional — omit it to re-read the current page without navigating (avoids accidentally undoing clicks). Like Playwright's browser_snapshot but 10-60x fewer tokens."
    )]
    async fn lad_snapshot(
        &self,
        params: Parameters<SnapshotParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_snapshot(params).await
    }

    #[tool(
        description = "Click an element by its ID from lad_snapshot. Set wait_for_navigation=true to wait for page load after clicking (useful for links/submit buttons). Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_click(
        &self,
        params: Parameters<ClickParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_click(params).await
    }

    #[tool(
        description = "Type text into an element by its ID from lad_snapshot. Set press_enter=true to submit after typing (saves a lad_press_key call). Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_type(
        &self,
        params: Parameters<TypeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_type(params).await
    }

    #[tool(
        description = "Select an option in a dropdown by element ID from lad_snapshot. Matches by visible label text first, falls back to value attribute. Set wait_for_navigation=true if the dropdown auto-submits. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_select(
        &self,
        params: Parameters<SelectParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_select(params).await
    }

    #[tool(
        description = "Evaluate arbitrary JavaScript on the active page. Requires LAD_ALLOW_EVAL=true env var. This is an escape hatch for when semantic tools (browse, click, type) cannot handle a specific interaction. Requires a prior lad_snapshot or lad_browse call. Returns the raw JS result."
    )]
    async fn lad_eval(
        &self,
        params: Parameters<EvalParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_eval(params).await
    }

    #[tool(
        description = "Close the browser and release all resources. Use this when done with browser automation to prevent resource leaks."
    )]
    async fn lad_close(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_close().await
    }

    #[tool(
        description = "Press a keyboard key on the active page. Optionally focus an element first by its ID from a prior snapshot. Common keys: Enter, Tab, Escape, ArrowDown, ArrowUp, Backspace, Delete, Space."
    )]
    async fn lad_press_key(
        &self,
        params: Parameters<PressKeyParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_press_key(params).await
    }

    #[tool(
        description = "Navigate back in browser history (equivalent to clicking the back button). Returns the semantic view of the previous page."
    )]
    async fn lad_back(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_back().await
    }

    #[tool(
        description = "Take a screenshot of the active page. Returns a base64-encoded PNG image. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_screenshot(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_screenshot().await
    }

    #[tool(
        description = "Wait for condition(s) to be true on the active page. Supports single `condition` or multiple `conditions` with mode='any' (first match wins) or mode='all' (default, all must pass). Example: conditions=['has button Dashboard', 'text contains Invalid password'], mode='any'. Default timeout: 10s, poll interval: 500ms."
    )]
    async fn lad_wait(
        &self,
        params: Parameters<WaitParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_wait(params).await
    }

    #[tool(
        description = "Inspect network traffic captured during browsing. Uses performance.getEntries() to collect requests with timing data. Optionally filter by type: auth, api, navigation, asset, all."
    )]
    async fn lad_network(
        &self,
        params: Parameters<NetworkParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_network(params).await
    }

    #[tool(
        description = "Hover over an element by its ID from lad_snapshot. Triggers mouseenter, mouseover, and mousemove events. Useful for dropdown menus, tooltips, and hover states. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_hover(
        &self,
        params: Parameters<HoverParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_hover(params).await
    }

    #[tool(
        description = "Handle JavaScript dialogs (alert, confirm, prompt). Actions: 'accept' auto-accepts future dialogs (with optional text for prompt inputs), 'dismiss' auto-dismisses, 'status' returns captured dialog history. Call before triggering actions that may show dialogs."
    )]
    async fn lad_dialog(
        &self,
        params: Parameters<DialogParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_dialog(params).await
    }

    #[tool(
        description = "Upload file(s) to a file input element by its ID from lad_snapshot. Provide absolute file paths. Currently supported on Chromium engine only; WebKit will return an error. File inputs inside shadow DOM or iframes (including same-origin) are not supported for upload due to Chromium CDP limitations. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_upload(
        &self,
        params: Parameters<UploadParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_upload(params).await
    }

    #[tool(
        description = "Fill multiple form fields at once and optionally submit. Fields are matched by label, name, or placeholder (case-insensitive). Use for login forms, registration, checkout, etc. Example: fields={\"Email\":\"user@test.com\",\"Password\":\"secret\"}, submit=true. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_fill_form(
        &self,
        params: Parameters<FillFormParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_fill_form(params).await
    }

    #[tool(
        description = "Scroll the page or scroll to a specific element. Directions: down, up, bottom, top. Optionally scroll to an element by ID. Useful for lazy-loaded content and infinite scroll pages. Returns updated semantic view after scrolling. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_scroll(
        &self,
        params: Parameters<ScrollParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_scroll(params).await
    }

    #[tool(
        description = "Reload the current page. Useful after form submissions or when content needs refreshing. Returns updated semantic view. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_refresh(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_refresh().await
    }

    #[tool(
        description = "Clear an input field by selecting all content and deleting. Works with React/Vue controlled components that ignore el.value=''. Requires element ID from a prior lad_snapshot."
    )]
    async fn lad_clear(
        &self,
        params: Parameters<ClearParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_clear(params).await
    }
}

// ── ServerHandler ──────────────────────────────────────────────────

#[tool_handler]
impl ServerHandler for LadServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_resources_subscribe()
                .build(),
        )
        .with_instructions("lad (LLM-as-DOM) is an AI browser pilot. It navigates web pages autonomously using heuristics + cheap LLM. Use lad_browse for goal-based navigation, lad_extract for page analysis (URL optional, format='prompt' for compact output), lad_assert for verification (URL optional), lad_locate for source mapping, lad_audit for page quality checks, lad_session for session state inspection/reset, lad_snapshot for semantic page snapshots (URL optional), lad_click/lad_type/lad_select for element interaction, lad_clear to clear input fields (works with React/Vue controlled components), lad_fill_form to fill multiple fields + submit in one call, lad_scroll for scrolling, lad_hover for hover states, lad_screenshot for visual capture, lad_wait for blocking condition checks (supports multiple conditions with mode='any'/'all'), lad_network for traffic inspection (includes HTTP status codes), lad_dialog for JS alert/confirm/prompt handling (auto-accepts by default), lad_refresh to reload the current page, lad_upload for file input uploads. Escape hatches: lad_eval for raw JS, lad_press_key for keyboard events, lad_back for history navigation, lad_close for cleanup.")
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<rmcp::service::RoleServer>,
    ) -> Result<InitializeResult, rmcp::ErrorData> {
        // Capture the peer so the watch polling loop can push notifications.
        *self.peer.lock().await = Some(context.peer.clone());
        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }
        Ok(self.get_info())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::service::RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        let guard = self.watch_state.lock().await;
        let resources = match guard.as_ref() {
            Some(ws) => {
                // FIX-4: Redact URL secrets from resource listing.
                let safe_url = llm_as_dom::sanitize::redact_url_secrets(&ws.url);
                let r = Resource {
                    raw: RawResource::new(ws.resource_uri(), format!("Watch: {}", safe_url))
                        .with_description("Live semantic diff stream from page watch")
                        .with_mime_type("application/json"),
                    annotations: None,
                };
                vec![r]
            }
            None => vec![],
        };
        Ok(ListResourcesResult {
            resources,
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<rmcp::service::RoleServer>,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        let guard = self.watch_state.lock().await;
        let ws = guard
            .as_ref()
            .ok_or_else(|| rmcp::ErrorData::resource_not_found("no active watch", None))?;

        if request.uri != ws.resource_uri() {
            return Err(rmcp::ErrorData::resource_not_found(
                format!("unknown resource: {}", request.uri),
                None,
            ));
        }

        let events = ws.events.events_since(None).await;
        let json = serde_json::to_string_pretty(&events).unwrap_or_default();
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(json, &request.uri).with_mime_type("application/json"),
        ]))
    }
}

// ── Main ───────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "llm_as_dom=info".into()),
        )
        .with_writer(std::io::stderr)
        .compact()
        .init();

    tracing::info!("llm-as-dom-mcp starting (stdio)");

    let server = LadServer::new();
    let transport = rmcp::transport::io::stdio();
    let service = server.serve(transport).await?;
    service.waiting().await?;

    Ok(())
}

/// SS-7: Tests extracted to `tests.rs` to keep mod.rs lean (~740 LOC -> ~740 LOC of tests).
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
