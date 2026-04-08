//! `llm-as-dom-mcp`: MCP server exposing the browser pilot as semantic tools.
//!
//! Provides tools: `lad_browse`, `lad_extract`, `lad_assert`, `lad_locate`,
//! `lad_audit`, `lad_session`, `lad_snapshot`, `lad_click`, `lad_type`, `lad_select`,
//! `lad_eval`, `lad_close`, `lad_press_key`, `lad_back`, `lad_screenshot`,
//! `lad_wait`, `lad_network`, `lad_hover`, `lad_dialog`, `lad_upload`.

mod assertions;
mod helpers;
mod params;
mod state;
mod tools;

use helpers::{mcp_err, read_env_with_fallback, same_origin};
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
    pub(crate) async fn ensure_engine(&self) -> Result<Arc<dyn BrowserEngine>, llm_as_dom::Error> {
        let mut engine_lock = self.engine.lock().await;
        if let Some(e) = engine_lock.as_ref() {
            return Ok(Arc::clone(e));
        }

        let mode = if self.interactive {
            "interactive (--app)"
        } else {
            "headless"
        };
        tracing::info!(mode, "launching browser");

        // FIX-R3-12: Use tempfile::Builder for cryptographically random temp dir
        // instead of predictable PID-based path.
        let user_data_dir = tempfile::Builder::new()
            .prefix("lad-chrome-")
            .tempdir()
            .map(|td| td.keep())
            .unwrap_or_else(|_| {
                std::env::temp_dir().join(format!("lad-chrome-{}", std::process::id()))
            });
        let config = EngineConfig {
            visible: self.interactive,
            interactive: self.interactive,
            user_data_dir,
            window_size: if self.interactive {
                (1024, 768)
            } else {
                (1280, 800)
            },
        };

        let engine_name = std::env::var("LAD_ENGINE").unwrap_or_default();
        let e: Arc<dyn BrowserEngine> = if engine_name == "webkit" {
            Arc::new(WebKitEngine::launch(config).await?)
        } else {
            Arc::new(ChromiumEngine::launch(config).await?)
        };
        *engine_lock = Some(Arc::clone(&e));
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

        // Inject cookies on the correct origin, then reload to apply them.
        let has_cookies = self.has_profile_cookies();
        if has_cookies {
            self.inject_profile_cookies(page.as_ref()).await;
            page.navigate(url).await.map_err(mcp_err)?;
            page.wait_for_navigation().await.map_err(mcp_err)?;
        }

        a11y::wait_for_content(page.as_ref(), a11y::DEFAULT_WAIT_TIMEOUT)
            .await
            .map_err(mcp_err)?;

        let view = a11y::extract_semantic_view(page.as_ref())
            .await
            .map_err(mcp_err)?;
        Ok((page, view))
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
                a11y::wait_for_content(ap.page.as_ref(), a11y::DEFAULT_WAIT_TIMEOUT)
                    .await
                    .map_err(mcp_err)?;
            }
            let view = a11y::extract_semantic_view(ap.page.as_ref())
                .await
                .map_err(mcp_err)?;
            let mut ap_owned = active.take().unwrap();
            ap_owned.url = url.to_string();
            ap_owned.view = view.clone();
            *active = Some(ap_owned);
            return Ok(view);
        }

        // Different origin or no active page — must release the lock before calling
        // navigate_and_extract (which acquires the engine lock). Then reacquire once
        // to store the result. This is safe because we're creating a fresh page.
        drop(active);
        let (page, view) = self.navigate_and_extract(url).await?;
        let mut active = self.active_page.lock().await;
        *active = Some(ActivePage {
            page,
            url: url.to_string(),
            view: view.clone(),
        });
        Ok(view)
    }

    /// Re-extract semantic view from the active page and update stored state.
    pub(crate) async fn refresh_active_view(
        &self,
    ) -> Result<semantic::SemanticView, rmcp::ErrorData> {
        let mut active = self.active_page.lock().await;
        let ap = active.as_mut().ok_or_else(helpers::no_active_page)?;
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
        description = "Extract structured info from a URL: interactive elements, text, page type. Never returns raw HTML."
    )]
    async fn lad_extract(
        &self,
        params: Parameters<ExtractParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_extract(params).await
    }

    #[tool(
        description = "Check assertions on a URL. Returns pass/fail for each. Supports: has login form, title contains X, has button Y, url contains Z."
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
        description = "Get a structured semantic snapshot of the current page. Returns elements with IDs that can be used with lad_click/lad_type. Like Playwright's browser_snapshot but 10-60x fewer tokens."
    )]
    async fn lad_snapshot(
        &self,
        params: Parameters<SnapshotParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_snapshot(params).await
    }

    #[tool(
        description = "Click an element by its ID from lad_snapshot. Requires a prior lad_snapshot call."
    )]
    async fn lad_click(
        &self,
        params: Parameters<ClickParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_click(params).await
    }

    #[tool(
        description = "Type text into an element by its ID from lad_snapshot. Requires a prior lad_snapshot call."
    )]
    async fn lad_type(
        &self,
        params: Parameters<TypeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_type(params).await
    }

    #[tool(
        description = "Select an option in a dropdown by element ID from lad_snapshot. Requires a prior lad_snapshot call."
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
        description = "Wait for a condition to be true on the active page. Uses natural language conditions like lad_assert but blocks until satisfied or timeout. Default timeout: 10s, poll interval: 500ms."
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
        description = "Hover over an element by its ID from lad_snapshot. Triggers mouseenter, mouseover, and mousemove events. Useful for dropdown menus, tooltips, and hover states. Requires a prior lad_snapshot call."
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
        description = "Upload file(s) to a file input element by its ID from lad_snapshot. Provide absolute file paths. Currently supported on Chromium engine only; WebKit will return an error. Requires a prior lad_snapshot call."
    )]
    async fn lad_upload(
        &self,
        params: Parameters<UploadParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_upload(params).await
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
        .with_instructions("lad (LLM-as-DOM) is an AI browser pilot. It navigates web pages autonomously using heuristics + cheap LLM. Use lad_browse for goal-based navigation, lad_extract for page analysis, lad_assert for verification, lad_locate for source mapping, lad_audit for page quality checks, lad_session for session state inspection/reset, lad_snapshot for semantic page snapshots, lad_click/lad_type/lad_select for element interaction, lad_hover for hover states/tooltips/dropdowns, lad_screenshot for visual capture, lad_wait for blocking condition checks, lad_network for traffic inspection, lad_dialog for JS alert/confirm/prompt handling, lad_upload for file input uploads (Chromium only). Escape hatches: lad_eval for raw JS, lad_press_key for keyboard events, lad_back for history navigation, lad_close for cleanup.")
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
                let r = Resource {
                    raw: RawResource::new(ws.resource_uri(), format!("Watch: {}", ws.url))
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

#[cfg(test)]
mod tests {
    use super::assertions::{check_assertion, normalize_assertion};
    use super::helpers::{check_js_result, extract_origin, key_to_code, same_origin};
    use super::params::*;

    use llm_as_dom::semantic;

    fn empty_view() -> semantic::SemanticView {
        semantic::SemanticView {
            url: String::new(),
            title: String::new(),
            page_hint: String::new(),
            elements: vec![],
            forms: vec![],
            visible_text: String::new(),
            state: semantic::PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        }
    }

    fn make_element(kind: semantic::ElementKind, label: &str) -> semantic::Element {
        semantic::Element {
            id: 1,
            kind,
            label: label.into(),
            name: None,
            value: None,
            placeholder: None,
            href: None,
            input_type: None,
            disabled: false,
            form_index: None,
            context: None,
            hint: None,
            frame_index: None,
        }
    }

    #[test]
    fn assert_has_email_input() {
        let mut view = empty_view();
        let mut el = make_element(semantic::ElementKind::Input, "Email address");
        el.input_type = Some("email".into());
        view.elements.push(el);

        assert!(check_assertion("has email input", &view, ""));
        assert!(check_assertion("has input email", &view, ""));
    }

    #[test]
    fn assert_has_button_reordered() {
        let mut view = empty_view();
        view.elements.push(make_element(
            semantic::ElementKind::Button,
            "Get Early Access",
        ));

        assert!(check_assertion("has button get early access", &view, ""));
        assert!(check_assertion("has get early access button", &view, ""));
    }

    #[test]
    fn assert_has_button_with_icon() {
        let mut view = empty_view();
        view.elements.push(make_element(
            semantic::ElementKind::Button,
            "Get Early Access \u{203a}",
        ));

        assert!(check_assertion("has button get early access", &view, ""));
    }

    #[test]
    fn assert_has_github_link() {
        let mut view = empty_view();
        let mut el = make_element(semantic::ElementKind::Link, "GitHub");
        el.href = Some("https://github.com/menot-you".into());
        view.elements.push(el);

        assert!(check_assertion("has link github", &view, ""));
        assert!(check_assertion("has github link", &view, ""));
    }

    #[test]
    fn assert_has_link_by_href() {
        let mut view = empty_view();
        let mut el = make_element(semantic::ElementKind::Link, "Star us");
        el.href = Some("https://github.com/menot-you".into());
        view.elements.push(el);

        assert!(check_assertion("has link github", &view, ""));
    }

    #[test]
    fn assert_has_form() {
        let mut view = empty_view();
        view.forms.push(semantic::FormMeta {
            index: 0,
            action: Some("/subscribe".into()),
            method: "POST".into(),
            id: None,
            name: None,
        });

        assert!(check_assertion("has form", &view, ""));
    }

    #[test]
    fn assert_input_matches_by_type() {
        let mut view = empty_view();
        let mut el = make_element(semantic::ElementKind::Input, "");
        el.input_type = Some("email".into());
        view.elements.push(el);

        assert!(check_assertion("has input email", &view, ""));
    }

    #[test]
    fn assert_input_matches_by_placeholder() {
        let mut view = empty_view();
        let mut el = make_element(semantic::ElementKind::Input, "");
        el.placeholder = Some("Enter your email".into());
        view.elements.push(el);

        assert!(check_assertion("has input email", &view, ""));
    }

    #[test]
    fn normalize_assertion_reorders_words() {
        assert_eq!(normalize_assertion("has email input"), "has input email");
        assert_eq!(
            normalize_assertion("has get early access button"),
            "has button get early access"
        );
        assert_eq!(normalize_assertion("has github link"), "has link github");
        assert_eq!(
            normalize_assertion("has button submit"),
            "has button submit"
        );
        assert_eq!(normalize_assertion("has input email"), "has input email");
    }

    // ── W1/W3 unit tests ─────────────────────────────────────────

    #[test]
    fn same_origin_matches() {
        assert!(same_origin(
            "https://example.com/foo",
            "https://example.com/bar"
        ));
        assert!(same_origin(
            "http://localhost:3000/a",
            "http://localhost:3000/b"
        ));
    }

    #[test]
    fn same_origin_rejects_different() {
        assert!(!same_origin(
            "https://example.com/foo",
            "https://other.com/foo"
        ));
        assert!(!same_origin(
            "http://localhost:3000",
            "https://localhost:3000"
        ));
        assert!(!same_origin(
            "http://localhost:3000",
            "http://localhost:4000"
        ));
    }

    #[test]
    fn extract_origin_works() {
        assert_eq!(
            extract_origin("https://example.com/path?q=1"),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            extract_origin("http://localhost:8080/foo"),
            Some("http://localhost:8080".to_string())
        );
        assert_eq!(extract_origin("ftp://nope"), None);
    }

    #[test]
    fn check_js_result_ok() {
        let ok = serde_json::json!(r#"{"ok":true}"#);
        assert!(check_js_result(&ok).is_ok());
    }

    #[test]
    fn check_js_result_err() {
        let err = serde_json::json!(r#"{"error":"element 5 not found"}"#);
        assert!(check_js_result(&err).is_err());
    }

    // ── Escape hatch helper tests ────────────────────────────────

    #[test]
    fn key_to_code_standard_keys() {
        assert_eq!(key_to_code("Enter"), "Enter");
        assert_eq!(key_to_code("Tab"), "Tab");
        assert_eq!(key_to_code("Escape"), "Escape");
        assert_eq!(key_to_code("Backspace"), "Backspace");
        assert_eq!(key_to_code("Delete"), "Delete");
        assert_eq!(key_to_code("Space"), "Space");
        assert_eq!(key_to_code(" "), "Space");
    }

    #[test]
    fn key_to_code_arrow_keys() {
        assert_eq!(key_to_code("ArrowUp"), "ArrowUp");
        assert_eq!(key_to_code("ArrowDown"), "ArrowDown");
        assert_eq!(key_to_code("ArrowLeft"), "ArrowLeft");
        assert_eq!(key_to_code("ArrowRight"), "ArrowRight");
    }

    #[test]
    fn key_to_code_function_keys() {
        assert_eq!(key_to_code("F1"), "F1");
        assert_eq!(key_to_code("F12"), "F12");
    }

    #[test]
    fn key_to_code_unknown_falls_back() {
        assert_eq!(key_to_code("a"), "a");
        assert_eq!(key_to_code("Shift"), "Shift");
    }

    #[test]
    fn key_to_code_navigation_keys() {
        assert_eq!(key_to_code("Home"), "Home");
        assert_eq!(key_to_code("End"), "End");
        assert_eq!(key_to_code("PageUp"), "PageUp");
        assert_eq!(key_to_code("PageDown"), "PageDown");
    }

    // ── W2: lad_wait assertion reuse tests ──────────────────────

    #[test]
    fn check_assertion_title_contains() {
        let mut view = empty_view();
        view.title = "Welcome to Dashboard".into();
        assert!(check_assertion("title contains dashboard", &view, ""));
        assert!(!check_assertion("title contains settings", &view, ""));
    }

    #[test]
    fn check_assertion_url_contains() {
        let mut view = empty_view();
        view.url = "https://example.com/dashboard".into();
        assert!(check_assertion("url contains dashboard", &view, ""));
        assert!(!check_assertion("url contains settings", &view, ""));
    }

    #[test]
    fn check_assertion_visible_text_fallback() {
        let mut view = empty_view();
        view.visible_text = "Loading complete. Welcome back, user!".into();
        assert!(check_assertion("welcome back", &view, ""));
        assert!(!check_assertion("error occurred", &view, ""));
    }

    // ── W2: param defaults ──────────────────────────────────────

    #[test]
    fn wait_params_defaults() {
        let json = r#"{"condition":"has button submit"}"#;
        let p: WaitParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.timeout_ms, 10_000);
        assert_eq!(p.poll_ms, 500);
    }

    #[test]
    fn network_params_defaults() {
        let json = r#"{}"#;
        let p: NetworkParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.filter, "all");
    }

    #[test]
    fn network_params_custom_filter() {
        let json = r#"{"filter":"auth"}"#;
        let p: NetworkParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.filter, "auth");
    }

    // ── W3: param parsing tests ─────────────────────────────────

    #[test]
    fn hover_params_parse() {
        let json = r#"{"element":42}"#;
        let p: HoverParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.element, 42);
    }

    #[test]
    fn dialog_params_accept_with_text() {
        let json = r#"{"action":"accept","text":"hello"}"#;
        let p: DialogParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.action, "accept");
        assert_eq!(p.text.as_deref(), Some("hello"));
    }

    #[test]
    fn dialog_params_status_no_text() {
        let json = r#"{"action":"status"}"#;
        let p: DialogParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.action, "status");
        assert!(p.text.is_none());
    }

    #[test]
    fn dialog_params_dismiss() {
        let json = r#"{"action":"dismiss"}"#;
        let p: DialogParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.action, "dismiss");
    }

    #[test]
    fn upload_params_parse() {
        let json = r#"{"element":7,"files":["/tmp/a.png","/tmp/b.pdf"]}"#;
        let p: UploadParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.element, 7);
        assert_eq!(p.files.len(), 2);
        assert_eq!(p.files[0], "/tmp/a.png");
        assert_eq!(p.files[1], "/tmp/b.pdf");
    }

    #[test]
    fn upload_params_single_file() {
        let json = r#"{"element":1,"files":["/tmp/test.csv"]}"#;
        let p: UploadParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.element, 1);
        assert_eq!(p.files.len(), 1);
    }

    #[test]
    fn upload_params_empty_files() {
        let json = r#"{"element":1,"files":[]}"#;
        let p: UploadParams = serde_json::from_str(json).unwrap();
        assert!(p.files.is_empty());
    }

    // ── FIX-1: SSRF scheme bypass tests (unit) ────────────

    #[test]
    fn ssrf_file_single_slash_blocked() {
        assert!(!llm_as_dom::sanitize::is_safe_url("file:/etc/passwd"));
    }

    #[test]
    fn ssrf_file_triple_slash_blocked() {
        assert!(!llm_as_dom::sanitize::is_safe_url("file:///etc/passwd"));
    }

    #[test]
    fn ssrf_data_scheme_blocked() {
        assert!(!llm_as_dom::sanitize::is_safe_url(
            "data:text/html,<h1>xss</h1>"
        ));
    }

    // ── FIX-12: watch interval validation ─────────────────

    #[test]
    fn watch_params_zero_interval() {
        let json = r#"{"action":"start","url":"https://example.com","interval_ms":0}"#;
        let p: WatchParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.interval_ms, Some(0));
        // The actual validation happens in watch_start() runtime
    }

    // ── FIX-14: upload path must be absolute ──────────────

    #[test]
    fn upload_params_relative_path_detected() {
        let json = r#"{"element":1,"files":["./relative/file.txt"]}"#;
        let p: UploadParams = serde_json::from_str(json).unwrap();
        // Validation happens at runtime, but we can assert path checking
        assert!(!std::path::Path::new(&p.files[0]).is_absolute());
    }

    #[test]
    fn upload_params_absolute_path_detected() {
        let json = r#"{"element":1,"files":["/tmp/file.txt"]}"#;
        let p: UploadParams = serde_json::from_str(json).unwrap();
        assert!(std::path::Path::new(&p.files[0]).is_absolute());
    }
}
