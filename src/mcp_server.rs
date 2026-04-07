//! `llm-as-dom-mcp`: MCP server exposing the browser pilot as semantic tools.
//!
//! Provides tools: `lad_browse`, `lad_extract`, `lad_assert`, `lad_locate`,
//! `lad_audit`, `lad_session`, `lad_snapshot`, `lad_click`, `lad_type`, `lad_select`,
//! `lad_eval`, `lad_close`, `lad_press_key`, `lad_back`, `lad_screenshot`,
//! `lad_wait`, `lad_network`, `lad_hover`, `lad_dialog`, `lad_upload`.

use llm_as_dom::engine::chromium::ChromiumEngine;
use llm_as_dom::engine::webkit::WebKitEngine;
use llm_as_dom::engine::{BrowserEngine, EngineConfig, PageHandle};
use llm_as_dom::{a11y, audit, backend, locate, network, pilot, semantic, watch};

use base64::Engine as _;
use std::sync::Arc;
use tokio::sync::Mutex;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::schemars;
use rmcp::schemars::JsonSchema;
use rmcp::service::{RequestContext, ServiceExt};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};

// ── Tool parameter types ───────────────────────────────────────────

/// Parameters for the `lad_browse` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct BrowseParams {
    /// URL to navigate to.
    url: String,
    /// Goal in natural language (e.g. "login as user@test.com with password secret123").
    goal: String,
    /// Max steps before giving up (default: 10).
    #[serde(default = "default_max_steps")]
    max_steps: u32,
    /// Optional maximum length of the HTML/DOM text embedded into the prompt.
    max_length: Option<usize>,
}

/// Default step limit for browsing goals.
fn default_max_steps() -> u32 {
    10
}

/// Parameters for the `lad_extract` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct ExtractParams {
    /// URL to navigate to and extract from.
    url: String,
    /// What to extract (e.g. "product prices", "form fields", "navigation links").
    what: String,
    /// Optional maximum length of the HTML/DOM text embedded into the prompt.
    max_length: Option<usize>,
}

/// Parameters for the `lad_assert` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct AssertParams {
    /// URL to navigate to and check.
    url: String,
    /// Assertions to verify (e.g. ["has login form", "title contains Dashboard"]).
    assertions: Vec<String>,
}

/// Parameters for the `lad_locate` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct LocateParams {
    /// URL to navigate to.
    url: String,
    /// CSS selector or text description of the element to locate.
    selector: String,
}

/// Parameters for the `lad_audit` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct AuditParams {
    /// URL to audit.
    url: String,
    /// Categories to check: "a11y", "forms", "links" (default: all).
    #[serde(default = "audit::default_categories")]
    categories: Vec<String>,
}

/// Parameters for the `lad_session` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct SessionParams {
    /// Action: "get" to view current session state, "clear" to reset.
    action: String,
}

/// Parameters for the `lad_watch` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct WatchParams {
    /// Action: "start", "stop", or "events".
    action: String,
    /// URL to watch (only needed for start).
    url: Option<String>,
    /// Polling interval in ms (default: 1000).
    interval_ms: Option<u32>,
    /// For "events" action: return only events with seq > since_seq.
    since_seq: Option<u64>,
}

/// Parameters for the `lad_snapshot` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct SnapshotParams {
    /// URL to navigate to.
    url: String,
}

/// Parameters for the `lad_click` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct ClickParams {
    /// Element ID from snapshot.
    element: u32,
}

/// Parameters for the `lad_type` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct TypeParams {
    /// Element ID from snapshot.
    element: u32,
    /// Text to type into the element.
    text: String,
}

/// Parameters for the `lad_select` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct SelectParams {
    /// Element ID from snapshot.
    element: u32,
    /// Value to select.
    value: String,
}

/// Parameters for the `lad_eval` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct EvalParams {
    /// JavaScript expression to evaluate on the active page.
    script: String,
}

/// Parameters for the `lad_press_key` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct PressKeyParams {
    /// Key name: "Enter", "Tab", "Escape", "ArrowDown", "ArrowUp", "Backspace", "Delete", "Space".
    key: String,
    /// Optional element ID from snapshot to focus before pressing the key.
    element: Option<u32>,
}

/// Parameters for the `lad_wait` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct WaitParams {
    /// Natural language condition, e.g. "has button Dashboard", "title contains Welcome".
    condition: String,
    /// Max wait time in ms (default: 10000).
    #[serde(default = "default_wait_timeout")]
    timeout_ms: u64,
    /// Poll interval in ms (default: 500).
    #[serde(default = "default_wait_poll")]
    poll_ms: u64,
}

fn default_wait_timeout() -> u64 {
    10_000
}
fn default_wait_poll() -> u64 {
    500
}

/// Parameters for the `lad_network` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct NetworkParams {
    /// Filter by request kind: "auth", "api", "navigation", "asset", or "all" (default).
    #[serde(default = "default_network_filter")]
    filter: String,
}

fn default_network_filter() -> String {
    "all".to_string()
}

/// Parameters for the `lad_hover` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct HoverParams {
    /// Element ID from a prior lad_snapshot.
    element: u32,
}

/// Parameters for the `lad_dialog` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct DialogParams {
    /// Action: "accept", "dismiss", or "status".
    action: String,
    /// Optional text to enter for prompt() dialogs (only used with "accept").
    text: Option<String>,
}

/// Parameters for the `lad_upload` tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct UploadParams {
    /// Element ID of the file input from a prior lad_snapshot.
    element: u32,
    /// Absolute file paths to upload.
    files: Vec<String>,
}

// ── Active page (persistent across snapshot -> click/type/select) ─

/// A page kept alive between `lad_snapshot` and subsequent interaction tools.
struct ActivePage {
    page: Box<dyn PageHandle>,
    url: String,
    view: semantic::SemanticView,
}

// ── Session state (lightweight, server-side) ──────────────────────

/// Lightweight session state tracked across MCP tool calls.
///
/// Persists auth status, visited URLs, and browse counts between
/// consecutive `lad_browse` invocations within the same MCP session.
#[derive(Debug, Clone, Default, Serialize)]
struct McpSessionState {
    /// Whether the pilot has successfully logged in during this session.
    authenticated: bool,
    /// Total number of `lad_browse` calls in this session.
    browse_count: u32,
    /// URLs visited during this session (most recent last).
    visited_urls: Vec<String>,
    /// Last goal that succeeded (if any).
    last_success_goal: Option<String>,
}

// ── Server state ───────────────────────────────────────────────────

/// MCP server that manages a headless browser and exposes pilot tools.
#[derive(Clone)]
#[allow(dead_code)] // tool_router is used internally by rmcp macros
struct LadServer {
    /// Router that dispatches MCP tool calls to handler methods.
    tool_router: ToolRouter<Self>,
    /// Shared browser engine (lazy-initialised on first tool call).
    engine: Arc<Mutex<Option<Arc<dyn BrowserEngine>>>>,
    /// LLM API base URL (Ollama, Z.AI, or any compatible endpoint).
    llm_url: String,
    /// LLM model name.
    llm_model: String,
    /// Session state carried across tool calls within this MCP session.
    session: Arc<Mutex<McpSessionState>>,
    /// Whether interactive mode is enabled (captcha pause for human).
    interactive: bool,
    /// Persistent page from `lad_snapshot`, reused by click/type/select.
    active_page: Arc<Mutex<Option<ActivePage>>>,
    /// Active watch state (at most one watch at a time).
    watch_state: Arc<Mutex<Option<watch::WatchState>>>,
    /// MCP peer for server-initiated push notifications.
    peer: Arc<Mutex<Option<rmcp::Peer<rmcp::service::RoleServer>>>>,
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
    async fn ensure_engine(&self) -> Result<Arc<dyn BrowserEngine>, llm_as_dom::Error> {
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

        let config = EngineConfig {
            visible: self.interactive,
            interactive: self.interactive,
            user_data_dir: std::env::temp_dir().join(format!("lad-chrome-{}", std::process::id())),
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
    async fn navigate_and_extract(
        &self,
        url: &str,
    ) -> Result<(Box<dyn PageHandle>, semantic::SemanticView), rmcp::ErrorData> {
        let engine = self.ensure_engine().await.map_err(mcp_err)?;
        let page = engine.new_page(url).await.map_err(mcp_err)?;
        page.wait_for_navigation().await.map_err(mcp_err)?;
        a11y::wait_for_content(page.as_ref(), a11y::DEFAULT_WAIT_TIMEOUT)
            .await
            .map_err(mcp_err)?;

        // Inject Chrome profile cookies if LAD_CHROME_PROFILE is set
        self.inject_profile_cookies(page.as_ref()).await;

        let view = a11y::extract_semantic_view(page.as_ref())
            .await
            .map_err(mcp_err)?;
        Ok((page, view))
    }

    /// Navigate to a URL (or reuse the active page if same origin), returning
    /// a fresh semantic view. Stores the result in `active_page`.
    async fn navigate_or_reuse(
        &self,
        url: &str,
    ) -> Result<semantic::SemanticView, rmcp::ErrorData> {
        let mut active = self.active_page.lock().await;

        // Reuse existing page if same origin
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

        // Different origin or no active page — create fresh
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
    async fn refresh_active_view(&self) -> Result<semantic::SemanticView, rmcp::ErrorData> {
        let mut active = self.active_page.lock().await;
        let ap = active.as_mut().ok_or_else(no_active_page)?;
        let view = a11y::extract_semantic_view(ap.page.as_ref())
            .await
            .map_err(mcp_err)?;
        ap.view = view.clone();
        Ok(view)
    }

    /// Inject cookies from the user's Chrome profile if `LAD_CHROME_PROFILE` is set.
    async fn inject_profile_cookies(&self, page: &dyn PageHandle) {
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

    // ── Watch helpers ────────────────────────────────────────────────

    /// Start watching a URL: navigate, extract initial view, spawn polling loop.
    async fn watch_start(&self, p: WatchParams) -> Result<CallToolResult, rmcp::ErrorData> {
        // Reject if already watching
        if self.watch_state.lock().await.is_some() {
            return Err(rmcp::ErrorData::invalid_params(
                "a watch is already active — stop it first",
                None,
            ));
        }

        let url = p.url.as_deref().unwrap_or("about:blank");
        let interval_ms = p.interval_ms.unwrap_or(1000);

        // Navigate and capture the initial semantic view
        let (page, initial_view) = self.navigate_and_extract(url).await?;

        // Build extract closure: captures the page handle so the polling
        // loop can re-extract semantic views without dropping the page.
        let page: Arc<dyn PageHandle> = Arc::from(page);
        let page_clone = Arc::clone(&page);
        let extract_fn = move || {
            let p = Arc::clone(&page_clone);
            async move { a11y::extract_semantic_view(p.as_ref()).await.ok() }
        };

        let watch = watch::start_watch(
            watch::WatchConfig {
                url: url.to_owned(),
                interval_ms,
                initial_view,
                peer: Some(Arc::clone(&self.peer)),
            },
            extract_fn,
        );

        let resource_uri = watch.resource_uri();
        *self.watch_state.lock().await = Some(watch);

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Watching {url} every {interval_ms}ms. Use lad_watch(action=\"events\") to retrieve diffs. Resource URI: {resource_uri}"
        ))]))
    }

    /// Return buffered watch events since an optional cursor.
    async fn watch_events(&self, p: WatchParams) -> Result<CallToolResult, rmcp::ErrorData> {
        let guard = self.watch_state.lock().await;
        let watch = guard.as_ref().ok_or_else(|| {
            rmcp::ErrorData::invalid_params("no active watch — start one first", None)
        })?;

        let events = watch.events.events_since(p.since_seq).await;
        let output = serde_json::json!({
            "url": watch.url,
            "event_count": events.len(),
            "current_seq": watch.events.current_seq(),
            "events": events,
        });

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }

    /// Stop an active watch and return summary.
    async fn watch_stop(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let watch = self
            .watch_state
            .lock()
            .await
            .take()
            .ok_or_else(|| rmcp::ErrorData::invalid_params("no active watch to stop", None))?;

        let url = watch.url.clone();
        let buf = watch.stop();
        let total = buf.current_seq();

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Stopped watching {url}. Total events captured: {total}"
        ))]))
    }

    /// Auto-detect which LLM backend to use based on env/URL.
    fn create_backend(
        url: &str,
        model: &str,
        max_prompt_length: Option<usize>,
    ) -> Box<dyn pilot::PilotBackend> {
        let llm_cred = read_env_with_fallback("LAD_LLM_API_KEY", "ANTHROPIC_API_KEY", "");
        if !llm_cred.is_empty() || url.contains("openai") {
            Box::new(backend::openai::OpenAiBackend::new(
                &llm_cred,
                model,
                max_prompt_length,
            ))
        } else if url.contains("z.ai") || url.contains("anthropic") {
            Box::new(backend::anthropic::AnthropicBackend::new(
                &llm_cred,
                model,
                max_prompt_length,
            ))
        } else {
            Box::new(backend::generic::GenericLlmBackend::new(
                url,
                model,
                max_prompt_length,
            ))
        }
    }
}

/// Read an environment variable with fallback to a deprecated name.
fn read_env_with_fallback(new_name: &str, old_name: &str, default: &str) -> String {
    if let Ok(val) = std::env::var(new_name) {
        return val;
    }
    if let Ok(val) = std::env::var(old_name) {
        tracing::warn!(
            old = old_name,
            new = new_name,
            "deprecated env var — please use {} instead",
            new_name
        );
        return val;
    }
    default.to_string()
}

/// Convert any `Display` error into an MCP internal-error response.
fn mcp_err(e: impl std::fmt::Display) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(e.to_string(), None)
}

/// Serialize a value to pretty JSON, returning a fallback on failure.
fn to_pretty_json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

/// Extract origin (scheme + host + port) from a URL string.
fn extract_origin(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .map(|r| ("https", r))
        .or_else(|| url.strip_prefix("http://").map(|r| ("http", r)))?;
    let (scheme, rest) = rest;
    let authority = rest.split('/').next().unwrap_or(rest);
    Some(format!("{scheme}://{authority}"))
}

/// Compare two URLs by origin (scheme + host + port).
fn same_origin(a: &str, b: &str) -> bool {
    match (extract_origin(a), extract_origin(b)) {
        (Some(oa), Some(ob)) => oa == ob,
        _ => false,
    }
}

/// Build "no active page" error.
fn no_active_page() -> rmcp::ErrorData {
    rmcp::ErrorData::invalid_params("no active page — call lad_snapshot first".to_string(), None)
}

/// Check JS eval result for `{ error: "..." }` pattern and surface it.
fn check_js_result(value: &serde_json::Value) -> Result<(), rmcp::ErrorData> {
    if let Some(s) = value.as_str()
        && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s)
        && let Some(err) = parsed.get("error").and_then(|v| v.as_str())
    {
        return Err(rmcp::ErrorData::invalid_params(err.to_string(), None));
    }
    Ok(())
}

/// Map a key name to its `KeyboardEvent.code` string.
///
/// Standard keys (Enter, Tab, Escape, arrows, etc.) have well-known codes.
/// For single characters, the code is `Key{UPPER}`.
/// Unknown keys fall back to the key name itself.
fn key_to_code(key: &str) -> &str {
    match key {
        "Enter" => "Enter",
        "Tab" => "Tab",
        "Escape" => "Escape",
        "Backspace" => "Backspace",
        "Delete" => "Delete",
        "Space" | " " => "Space",
        "ArrowUp" => "ArrowUp",
        "ArrowDown" => "ArrowDown",
        "ArrowLeft" => "ArrowLeft",
        "ArrowRight" => "ArrowRight",
        "Home" => "Home",
        "End" => "End",
        "PageUp" => "PageUp",
        "PageDown" => "PageDown",
        "F1" => "F1",
        "F2" => "F2",
        "F3" => "F3",
        "F4" => "F4",
        "F5" => "F5",
        "F6" => "F6",
        "F7" => "F7",
        "F8" => "F8",
        "F9" => "F9",
        "F10" => "F10",
        "F11" => "F11",
        "F12" => "F12",
        _ => key,
    }
}

// ── Tool implementations ───────────────────────────────────────────

#[tool_router]
impl LadServer {
    /// Browse a URL and accomplish a goal autonomously.
    /// The pilot uses heuristics + cheap LLM to navigate, fill forms, click buttons.
    /// Returns structured result: success/failure, steps taken, timing.
    #[tool(
        description = "Navigate to a URL and accomplish a browsing goal autonomously (login, fill form, click, search). Returns structured result."
    )]
    async fn lad_browse(
        &self,
        params: Parameters<BrowseParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(url = %p.url, goal = %p.goal, "lad_browse");

        tracing::info!(url = %p.url, "launching page");
        let engine = self.ensure_engine().await.map_err(mcp_err)?;
        let page = engine.new_page(&p.url).await.map_err(mcp_err)?;
        tracing::info!("waiting for navigation");
        page.wait_for_navigation().await.map_err(mcp_err)?;
        tracing::info!("waiting for content to stabilise");
        a11y::wait_for_content(page.as_ref(), a11y::DEFAULT_WAIT_TIMEOUT)
            .await
            .map_err(mcp_err)?;
        tracing::info!("page ready, initialising pilot");

        // Inject Chrome profile cookies if LAD_CHROME_PROFILE is set
        self.inject_profile_cookies(page.as_ref()).await;

        let backend = Self::create_backend(&self.llm_url, &self.llm_model, p.max_length);
        let config = pilot::PilotConfig {
            goal: p.goal.clone(),
            max_steps: p.max_steps,
            use_hints: true,
            use_heuristics: true,
            playbook_dir: None,
            max_retries_per_step: 2,
            session: None,
            interactive: self.interactive,
        };

        tracing::info!("running pilot");
        let result = pilot::run_pilot(page.as_ref(), backend.as_ref(), &config)
            .await
            .map_err(mcp_err)?;
        tracing::info!(
            success = result.success,
            steps = result.steps.len(),
            duration_secs = result.total_duration.as_secs_f64(),
            "pilot complete"
        );

        // Update session state
        {
            let mut session = self.session.lock().await;
            session.browse_count += 1;
            session.visited_urls.push(p.url.clone());
            if result.success {
                session.last_success_goal = Some(p.goal.clone());
                // Detect if login was the goal
                let goal_lower = p.goal.to_lowercase();
                if goal_lower.contains("login") || goal_lower.contains("sign in") {
                    session.authenticated = true;
                }
            }
        }

        // Always capture a final screenshot for visual verification.
        tracing::info!("capturing final screenshot");
        let final_screenshot = pilot::take_screenshot(page.as_ref()).await;

        let session_snapshot = {
            let session = self.session.lock().await;
            serde_json::json!({
                "authenticated": session.authenticated,
                "browse_count": session.browse_count,
                "visited_urls_count": session.visited_urls.len(),
            })
        };

        let output = serde_json::json!({
            "success": result.success,
            "steps": result.steps.len(),
            "heuristic_steps": result.heuristic_hits,
            "llm_steps": result.llm_hits,
            "duration_secs": result.total_duration.as_secs_f64(),
            "final_action": format!("{:?}", result.final_action),
            "session": session_snapshot,
            "actions": result.steps.iter().map(|s| {
                serde_json::json!({
                    "step": s.index,
                    "source": format!("{:?}", s.source),
                    "action": format!("{:?}", s.action),
                    "duration_ms": s.duration.as_millis() as u64,
                })
            }).collect::<Vec<_>>(),
        });

        let mut content_blocks: Vec<Content> = vec![Content::text(to_pretty_json(&output))];

        // Append in-flight screenshots (e.g. from escalation retries).
        for b64_png in &result.screenshots {
            content_blocks.push(Content::image(b64_png, "image/png"));
        }

        // Append final screenshot (success or fail).
        if let Some(b64_png) = &final_screenshot {
            content_blocks.push(Content::image(b64_png, "image/png"));
        }

        Ok(CallToolResult::success(content_blocks))
    }

    /// Extract structured information from a web page.
    /// Returns interactive elements, visible text, page classification.
    /// Never returns raw HTML.
    #[tool(
        description = "Extract structured info from a URL: interactive elements, text, page type. Never returns raw HTML."
    )]
    async fn lad_extract(
        &self,
        params: Parameters<ExtractParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        let (_page, mut view) = self.navigate_and_extract(&p.url).await?;

        if let Some(max_len) = p.max_length
            && view.visible_text.len() > max_len
        {
            let mut end = max_len;
            while !view.visible_text.is_char_boundary(end) && end > 0 {
                end -= 1;
            }
            view.visible_text.truncate(end);
        }

        let output = serde_json::json!({
            "url": view.url,
            "title": view.title,
            "page_type": view.page_hint,
            "elements_count": view.elements.len(),
            "estimated_tokens": view.estimated_tokens(),
            "elements": view.elements,
            "forms": view.forms,
            "visible_text": view.visible_text,
            "query": p.what,
        });

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }

    /// Assert conditions about a web page and return pass/fail results.
    /// Supports: "has login form", "title contains X", "has button Y", "has input Z", etc.
    #[tool(
        description = "Check assertions on a URL. Returns pass/fail for each. Supports: has login form, title contains X, has button Y, url contains Z."
    )]
    async fn lad_assert(
        &self,
        params: Parameters<AssertParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(url = %p.url, assertions = ?p.assertions, "lad_assert");

        let (_page, view) = self.navigate_and_extract(&p.url).await?;
        let prompt_text = view.to_prompt();

        let mut results = Vec::new();
        for assertion in &p.assertions {
            let pass = check_assertion(&assertion.to_lowercase(), &view, &prompt_text);
            results.push(serde_json::json!({
                "assertion": assertion,
                "pass": pass,
            }));
        }

        let all_pass = results.iter().all(|r| r["pass"].as_bool().unwrap_or(false));

        let output = serde_json::json!({
            "url": view.url,
            "title": view.title,
            "all_pass": all_pass,
            "results": results,
        });

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }

    /// Locate a DOM element's source file using dev-mode source maps.
    /// Checks React __source, data-ds (domscribe), data-lad hints, and DOM path fallback.
    #[tool(
        description = "Map a DOM element back to its source file. Checks React dev source, data-ds, data-lad attributes. Returns source file/line or DOM path fallback."
    )]
    async fn lad_locate(
        &self,
        params: Parameters<LocateParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(url = %p.url, selector = %p.selector, "lad_locate");

        let (page, _view) = self.navigate_and_extract(&p.url).await?;
        let js = locate::build_locate_js(&p.selector);
        let raw_value = page.eval_js(&js).await.map_err(mcp_err)?;

        let raw: locate::RawLocateResult = serde_json::from_value(raw_value)
            .map_err(|e| mcp_err(format!("locate JS parse failed: {e:?}")))?;

        match locate::parse_locate_result(raw) {
            Ok(locate_result) => {
                let output = serde_json::to_value(&locate_result)
                    .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));
                Ok(CallToolResult::success(vec![Content::text(
                    to_pretty_json(&output),
                )]))
            }
            Err(msg) => Ok(CallToolResult::success(vec![Content::text(
                to_pretty_json(&serde_json::json!({
                    "error": msg,
                    "source_maps": "not available",
                })),
            )])),
        }
    }

    /// Audit a web page for accessibility, forms, and links issues.
    /// Returns structured issues with severity, element, message, and suggestion.
    #[tool(
        description = "Audit a URL for quality issues: a11y (alt text, labels, lang), forms (autocomplete, minlength), links (void hrefs, noopener). Returns issues with severity and fix suggestions."
    )]
    async fn lad_audit(
        &self,
        params: Parameters<AuditParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(url = %p.url, categories = ?p.categories, "lad_audit");

        let (page, _view) = self.navigate_and_extract(&p.url).await?;
        let js = audit::build_audit_js(&p.categories);
        let raw_value = page.eval_js(&js).await.map_err(mcp_err)?;

        let raw: Vec<audit::RawAuditIssue> = serde_json::from_value(raw_value)
            .map_err(|e| mcp_err(format!("audit JS parse failed: {e:?}")))?;

        let audit_result = audit::parse_audit_result(&p.url, raw);
        let output = serde_json::to_value(&audit_result)
            .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }

    /// Inspect or reset the MCP session state.
    /// Tracks authentication status, visited URLs, and browse history across tool calls.
    #[tool(
        description = "View or reset MCP session state: auth status, visited URLs, browse count. Actions: 'get' or 'clear'."
    )]
    async fn lad_session(
        &self,
        params: Parameters<SessionParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(action = %p.action, "lad_session");

        match p.action.as_str() {
            "get" => {
                let session = self.session.lock().await;
                let output = serde_json::to_value(&*session)
                    .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));
                Ok(CallToolResult::success(vec![Content::text(
                    to_pretty_json(&output),
                )]))
            }
            "clear" => {
                let mut session = self.session.lock().await;
                *session = McpSessionState::default();
                Ok(CallToolResult::success(vec![Content::text(
                    r#"{"status": "session cleared"}"#.to_string(),
                )]))
            }
            other => {
                let msg = format!(
                    "unknown session action '{}'. Valid actions: 'get', 'clear'.",
                    other
                );
                Err(rmcp::ErrorData::invalid_params(msg, None))
            }
        }
    }

    /// Manage monitoring and watching of a web page
    #[tool(
        description = "Watch page state over time. Actions: 'start' begins polling a URL at interval_ms, diffing semantic views each cycle. 'events' returns captured diffs (pass since_seq for cursor-based pagination). 'stop' ends the watch."
    )]
    async fn lad_watch(
        &self,
        params: Parameters<WatchParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(action = %p.action, "lad_watch");

        match p.action.as_str() {
            "start" => self.watch_start(p).await,
            "events" => self.watch_events(p).await,
            "stop" => self.watch_stop().await,
            other => Err(rmcp::ErrorData::invalid_params(
                format!("action must be start, events, or stop (got '{other}')"),
                None,
            )),
        }
    }

    // ── W1: lad_snapshot ──────────────────────────────────────────

    /// Get a structured semantic snapshot of the current page.
    /// Returns elements with IDs usable by lad_click/lad_type/lad_select.
    #[tool(
        description = "Get a structured semantic snapshot of the current page. Returns elements with IDs that can be used with lad_click/lad_type. Like Playwright's browser_snapshot but 10-60x fewer tokens."
    )]
    async fn lad_snapshot(
        &self,
        params: Parameters<SnapshotParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(url = %p.url, "lad_snapshot");

        let view = self.navigate_or_reuse(&p.url).await?;
        Ok(CallToolResult::success(vec![Content::text(
            view.to_prompt(),
        )]))
    }

    // ── W2: lad_click / lad_type / lad_select ─────────────────────

    /// Click an element by its ID from lad_snapshot.
    #[tool(
        description = "Click an element by its ID from lad_snapshot. Requires a prior lad_snapshot call."
    )]
    async fn lad_click(
        &self,
        params: Parameters<ClickParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(element = p.element, "lad_click");

        {
            let active = self.active_page.lock().await;
            let ap = active.as_ref().ok_or_else(no_active_page)?;
            let js = format!(
                r#"(() => {{
                    const el = document.querySelector('[data-lad-id="{}"]');
                    if (!el) return JSON.stringify({{ error: "element {} not found" }});
                    el.click();
                    return JSON.stringify({{ ok: true }});
                }})()"#,
                p.element, p.element
            );
            check_js_result(&ap.page.eval_js(&js).await.map_err(mcp_err)?)?;
        }

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let view = self.refresh_active_view().await?;
        Ok(CallToolResult::success(vec![Content::text(
            view.to_prompt(),
        )]))
    }

    /// Type text into an element by its ID from lad_snapshot.
    #[tool(
        description = "Type text into an element by its ID from lad_snapshot. Requires a prior lad_snapshot call."
    )]
    async fn lad_type(
        &self,
        params: Parameters<TypeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(element = p.element, text = %p.text, "lad_type");

        {
            let active = self.active_page.lock().await;
            let ap = active.as_ref().ok_or_else(no_active_page)?;
            let escaped = p.text.replace('\\', "\\\\").replace('\'', "\\'");
            let js = format!(
                r#"(() => {{
                    const el = document.querySelector('[data-lad-id="{}"]');
                    if (!el) return JSON.stringify({{ error: "element {} not found" }});
                    el.focus();
                    el.value = '{}';
                    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                    return JSON.stringify({{ ok: true }});
                }})()"#,
                p.element, p.element, escaped
            );
            check_js_result(&ap.page.eval_js(&js).await.map_err(mcp_err)?)?;
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let view = self.refresh_active_view().await?;
        Ok(CallToolResult::success(vec![Content::text(
            view.to_prompt(),
        )]))
    }

    /// Select an option in a `<select>` element by its ID from lad_snapshot.
    #[tool(
        description = "Select an option in a dropdown by element ID from lad_snapshot. Requires a prior lad_snapshot call."
    )]
    async fn lad_select(
        &self,
        params: Parameters<SelectParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(element = p.element, value = %p.value, "lad_select");

        {
            let active = self.active_page.lock().await;
            let ap = active.as_ref().ok_or_else(no_active_page)?;
            let escaped = p.value.replace('\\', "\\\\").replace('\'', "\\'");
            let js = format!(
                r#"(() => {{
                    const el = document.querySelector('[data-lad-id="{}"]');
                    if (!el) return JSON.stringify({{ error: "element {} not found" }});
                    if (el.tagName !== 'SELECT') return JSON.stringify({{ error: "element {} is not a <select>" }});
                    el.value = '{}';
                    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                    return JSON.stringify({{ ok: true }});
                }})()"#,
                p.element, p.element, p.element, escaped
            );
            check_js_result(&ap.page.eval_js(&js).await.map_err(mcp_err)?)?;
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let view = self.refresh_active_view().await?;
        Ok(CallToolResult::success(vec![Content::text(
            view.to_prompt(),
        )]))
    }

    // ── W1-escape: lad_eval ──────────────────────────────────────

    /// Evaluate arbitrary JavaScript on the active page.
    /// This is an escape hatch for when semantic tools cannot handle
    /// a specific interaction.
    #[tool(
        description = "Evaluate arbitrary JavaScript on the active page. This is an escape hatch for when semantic tools (browse, click, type) cannot handle a specific interaction. Requires a prior lad_snapshot or lad_browse call. Returns the raw JS result."
    )]
    async fn lad_eval(
        &self,
        params: Parameters<EvalParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(script = %p.script, "lad_eval");

        let active = self.active_page.lock().await;
        let ap = active.as_ref().ok_or_else(no_active_page)?;
        let result = ap.page.eval_js(&p.script).await.map_err(mcp_err)?;

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&result),
        )]))
    }

    // ── W1-escape: lad_close ─────────────────────────────────────

    /// Close the browser and release all resources.
    #[tool(
        description = "Close the browser and release all resources. Use this when done with browser automation to prevent resource leaks."
    )]
    async fn lad_close(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        tracing::info!("lad_close");

        // Clear active page first
        *self.active_page.lock().await = None;

        // Close the engine if one was launched
        let mut engine_lock = self.engine.lock().await;
        if let Some(engine) = engine_lock.take() {
            engine.close().await.map_err(mcp_err)?;
        }

        Ok(CallToolResult::success(vec![Content::text(
            r#"{"status": "browser closed"}"#.to_string(),
        )]))
    }

    // ── W1-escape: lad_press_key ─────────────────────────────────

    /// Press a keyboard key on the active page.
    /// Optionally focus an element first by its ID from a prior snapshot.
    #[tool(
        description = "Press a keyboard key on the active page. Optionally focus an element first by its ID from a prior snapshot. Common keys: Enter, Tab, Escape, ArrowDown, ArrowUp, Backspace, Delete, Space."
    )]
    async fn lad_press_key(
        &self,
        params: Parameters<PressKeyParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(key = %p.key, element = ?p.element, "lad_press_key");

        {
            let active = self.active_page.lock().await;
            let ap = active.as_ref().ok_or_else(no_active_page)?;

            // If element specified, focus it first
            if let Some(id) = p.element {
                let focus_js = format!(
                    r#"(() => {{
                        const el = document.querySelector('[data-lad-id="{}"]');
                        if (!el) return JSON.stringify({{ error: "element {} not found" }});
                        el.focus();
                        return JSON.stringify({{ ok: true }});
                    }})()"#,
                    id, id
                );
                check_js_result(&ap.page.eval_js(&focus_js).await.map_err(mcp_err)?)?;
            }

            // Dispatch keyboard event sequence: keydown, keypress, keyup
            let code = key_to_code(&p.key);
            let key_escaped = p.key.replace('\\', "\\\\").replace('\'', "\\'");
            let code_escaped = code.replace('\\', "\\\\").replace('\'', "\\'");
            let js = format!(
                r#"(() => {{
                    const target = document.activeElement || document.body;
                    for (const type of ['keydown', 'keypress', 'keyup']) {{
                        target.dispatchEvent(new KeyboardEvent(type, {{
                            key: '{}', code: '{}', bubbles: true, cancelable: true
                        }}));
                    }}
                }})()"#,
                key_escaped, code_escaped
            );
            ap.page.eval_js(&js).await.map_err(mcp_err)?;
        }

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let view = self.refresh_active_view().await?;
        Ok(CallToolResult::success(vec![Content::text(
            view.to_prompt(),
        )]))
    }

    // ── W1-escape: lad_back ──────────────────────────────────────

    /// Navigate back in browser history.
    #[tool(
        description = "Navigate back in browser history (equivalent to clicking the back button). Returns the semantic view of the previous page."
    )]
    async fn lad_back(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        tracing::info!("lad_back");

        {
            let active = self.active_page.lock().await;
            let ap = active.as_ref().ok_or_else(no_active_page)?;
            ap.page.eval_js("history.back()").await.map_err(mcp_err)?;
        }

        // Wait for navigation to settle
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let view = self.refresh_active_view().await?;

        // Update stored URL to match where we ended up
        {
            let mut active = self.active_page.lock().await;
            if let Some(ap) = active.as_mut()
                && let Ok(url) = ap.page.url().await
            {
                ap.url = url;
            }
        }

        Ok(CallToolResult::success(vec![Content::text(
            view.to_prompt(),
        )]))
    }

    // ── W2: lad_screenshot ──────────────────────────────────────

    /// Take a screenshot of the active page.
    #[tool(
        description = "Take a screenshot of the active page. Returns a base64-encoded PNG image. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_screenshot(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        tracing::info!("lad_screenshot");
        let guard = self.active_page.lock().await;
        let active = guard.as_ref().ok_or_else(no_active_page)?;
        let png = active.page.screenshot_png().await.map_err(mcp_err)?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        Ok(CallToolResult::success(vec![Content::image(
            b64,
            "image/png",
        )]))
    }

    // ── W2: lad_wait ────────────────────────────────────────────

    /// Wait for a condition to be true on the active page.
    #[tool(
        description = "Wait for a condition to be true on the active page. Uses natural language conditions like lad_assert but blocks until satisfied or timeout. Default timeout: 10s, poll interval: 500ms."
    )]
    async fn lad_wait(
        &self,
        params: Parameters<WaitParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(condition = %p.condition, timeout_ms = p.timeout_ms, "lad_wait");

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(p.timeout_ms);
        let poll_dur = std::time::Duration::from_millis(p.poll_ms);
        let cond_lower = p.condition.to_lowercase();

        loop {
            let view = self.refresh_active_view().await?;
            let prompt_text = view.to_prompt();
            if check_assertion(&cond_lower, &view, &prompt_text) {
                return Ok(CallToolResult::success(vec![Content::text(
                    view.to_prompt(),
                )]));
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(rmcp::ErrorData::internal_error(
                    format!(
                        "timeout after {}ms waiting for condition: {}",
                        p.timeout_ms, p.condition
                    ),
                    None,
                ));
            }

            tokio::time::sleep(poll_dur).await;
        }
    }

    // ── W2: lad_network ─────────────────────────────────────────

    /// Inspect network traffic captured during browsing.
    #[tool(
        description = "Inspect network traffic captured during browsing. Uses performance.getEntries() to collect requests with timing data. Optionally filter by type: auth, api, navigation, asset, all."
    )]
    async fn lad_network(
        &self,
        params: Parameters<NetworkParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(filter = %p.filter, "lad_network");

        let guard = self.active_page.lock().await;
        let active = guard.as_ref().ok_or_else(no_active_page)?;

        // Use performance.getEntries() to gather network timing data via JS.
        let js = r#"JSON.stringify(
            performance.getEntriesByType('resource').concat(
                performance.getEntriesByType('navigation')
            ).map(e => ({
                url: e.name,
                type: e.initiatorType || e.entryType,
                duration_ms: Math.round(e.duration),
                transfer_size: e.transferSize || 0,
                start_ms: Math.round(e.startTime)
            }))
        )"#;

        let raw_value = active.page.eval_js(js).await.map_err(mcp_err)?;
        let json_str = raw_value
            .as_str()
            .ok_or_else(|| mcp_err("performance.getEntries() returned non-string"))?;

        let entries: Vec<serde_json::Value> = serde_json::from_str(json_str)
            .map_err(|e| mcp_err(format!("parse performance entries: {e}")))?;

        // Build a NetworkCapture from JS entries for classification.
        let mut capture = network::NetworkCapture::new();
        for (i, entry) in entries.iter().enumerate() {
            let url = entry["url"].as_str().unwrap_or("").to_string();
            // performance entries don't carry HTTP method; default to GET.
            let method = "GET";
            capture.on_request(i.to_string(), url, method.to_string(), None);
        }

        let summary = capture.summary();
        let filter_kind = match p.filter.as_str() {
            "auth" => Some(network::RequestKind::Auth),
            "api" => Some(network::RequestKind::Api),
            "navigation" => Some(network::RequestKind::Navigation),
            "asset" => Some(network::RequestKind::Asset),
            _ => None,
        };

        let filtered: Vec<&network::CapturedRequest> = if let Some(kind) = filter_kind {
            capture
                .requests
                .values()
                .filter(|r| r.kind == kind)
                .collect()
        } else {
            capture.requests.values().collect()
        };

        let output = serde_json::json!({
            "summary": summary,
            "filter": p.filter,
            "count": filtered.len(),
            "requests": filtered.iter().map(|r| serde_json::json!({
                "url": r.url,
                "kind": r.kind,
                "method": r.method,
                "timestamp_ms": r.timestamp_ms,
            })).collect::<Vec<_>>(),
        });

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }

    // ── W3: lad_hover ───────────────────────────────────────────────

    /// Hover over an element by its ID from lad_snapshot.
    #[tool(
        description = "Hover over an element by its ID from lad_snapshot. Triggers mouseenter, mouseover, and mousemove events. Useful for dropdown menus, tooltips, and hover states. Requires a prior lad_snapshot call."
    )]
    async fn lad_hover(
        &self,
        params: Parameters<HoverParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(element = p.element, "lad_hover");

        {
            let active = self.active_page.lock().await;
            let ap = active.as_ref().ok_or_else(no_active_page)?;
            let js = format!(
                r#"(() => {{
                    const el = document.querySelector('[data-lad-id="{}"]');
                    if (!el) return JSON.stringify({{ error: "element {} not found" }});
                    for (const type of ['mouseenter', 'mouseover', 'mousemove']) {{
                        el.dispatchEvent(new MouseEvent(type, {{
                            bubbles: true, cancelable: true, view: window
                        }}));
                    }}
                    return JSON.stringify({{ ok: true }});
                }})()"#,
                p.element, p.element
            );
            check_js_result(&ap.page.eval_js(&js).await.map_err(mcp_err)?)?;
        }

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let view = self.refresh_active_view().await?;
        Ok(CallToolResult::success(vec![Content::text(
            view.to_prompt(),
        )]))
    }

    // ── W3: lad_dialog ──────────────────────────────────────────────

    /// Handle JavaScript dialogs (alert, confirm, prompt).
    #[tool(
        description = "Handle JavaScript dialogs (alert, confirm, prompt). Actions: 'accept' auto-accepts future dialogs (with optional text for prompt inputs), 'dismiss' auto-dismisses, 'status' returns captured dialog history. Call before triggering actions that may show dialogs."
    )]
    async fn lad_dialog(
        &self,
        params: Parameters<DialogParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(action = %p.action, text = ?p.text, "lad_dialog");

        let active = self.active_page.lock().await;
        let ap = active.as_ref().ok_or_else(no_active_page)?;

        // Ensure dialog overrides are installed
        let setup_js = r#"
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
        ap.page.eval_js(setup_js).await.map_err(mcp_err)?;

        match p.action.as_str() {
            "accept" => {
                let text_escaped = p
                    .text
                    .as_deref()
                    .unwrap_or("")
                    .replace('\\', "\\\\")
                    .replace('\'', "\\'");
                let js = format!(
                    "window.__lad_dialog_auto = 'accept'; \
                     window.__lad_dialog_response = '{}';",
                    text_escaped
                );
                ap.page.eval_js(&js).await.map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(
                    r#"{"status": "dialogs will be auto-accepted"}"#.to_string(),
                )]))
            }
            "dismiss" => {
                ap.page
                    .eval_js("window.__lad_dialog_auto = 'dismiss';")
                    .await
                    .map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(
                    r#"{"status": "dialogs will be auto-dismissed"}"#.to_string(),
                )]))
            }
            "status" => {
                let result = ap
                    .page
                    .eval_js("JSON.stringify(window.__lad_dialogs || [])")
                    .await
                    .map_err(mcp_err)?;
                let text = result.as_str().unwrap_or("[]");
                Ok(CallToolResult::success(vec![Content::text(
                    text.to_string(),
                )]))
            }
            other => Err(rmcp::ErrorData::invalid_params(
                format!(
                    "unknown dialog action '{}' — use 'accept', 'dismiss', or 'status'",
                    other
                ),
                None,
            )),
        }
    }

    // ── W3: lad_upload ──────────────────────────────────────────────

    /// Upload file(s) to a file input element.
    #[tool(
        description = "Upload file(s) to a file input element by its ID from lad_snapshot. Provide absolute file paths. Currently supported on Chromium engine only; WebKit will return an error. Requires a prior lad_snapshot call."
    )]
    async fn lad_upload(
        &self,
        params: Parameters<UploadParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(element = p.element, files = ?p.files, "lad_upload");

        if p.files.is_empty() {
            return Err(rmcp::ErrorData::invalid_params(
                "files array must not be empty",
                None,
            ));
        }

        // Validate all file paths exist on disk
        for path in &p.files {
            if !std::path::Path::new(path).exists() {
                return Err(rmcp::ErrorData::invalid_params(
                    format!("file not found: {path}"),
                    None,
                ));
            }
        }

        let selector = format!(r#"[data-lad-id="{}"]"#, p.element);

        {
            let active = self.active_page.lock().await;
            let ap = active.as_ref().ok_or_else(no_active_page)?;

            // Verify element exists and is a file input
            let check_js = format!(
                r#"(() => {{
                    const el = document.querySelector('[data-lad-id="{}"]');
                    if (!el) return JSON.stringify({{ error: "element {} not found" }});
                    if (el.tagName !== 'INPUT' || el.type !== 'file')
                        return JSON.stringify({{ error: "element {} is not a file input" }});
                    return JSON.stringify({{ ok: true }});
                }})()"#,
                p.element, p.element, p.element
            );
            check_js_result(&ap.page.eval_js(&check_js).await.map_err(mcp_err)?)?;

            // Use engine-level file upload (CDP on Chromium)
            ap.page
                .set_input_files(&selector, &p.files)
                .await
                .map_err(mcp_err)?;

            // Dispatch change event so frameworks react
            let change_js = format!(
                r#"document.querySelector('[data-lad-id="{}"]')
                    .dispatchEvent(new Event('change', {{ bubbles: true }}))"#,
                p.element
            );
            ap.page.eval_js(&change_js).await.map_err(mcp_err)?;
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            r#"{{"status": "uploaded", "files": {}, "element": {}}}"#,
            p.files.len(),
            p.element
        ))]))
    }
}

/// Evaluate a single assertion against a semantic view.
///
/// Supported patterns (after normalization):
/// - `has login form` / `has login`
/// - `has password`
/// - `title contains <text>`
/// - `url contains <text>`
/// - `has button <label>` (also matches `has <label> button`)
/// - `has link <label>` (also matches `has <label> link`)
/// - `has input <name>` (also matches `has <name> input`, plus input_type)
/// - `has form` — any form on page
/// - `has image` / `has img` — any img element in visible text
/// - `page has section <title>` — section title in visible text
/// - Fallback: all words present in combined page text.
fn check_assertion(assertion: &str, view: &semantic::SemanticView, prompt_text: &str) -> bool {
    let full_text = format!(
        "{} {} {} {}",
        view.url, view.title, view.visible_text, prompt_text
    )
    .to_lowercase();

    // Normalize word order: "has X input" → "has input X", etc.
    let normalized = normalize_assertion(assertion);
    let assertion = &normalized;

    if assertion.contains("has login form") || assertion.contains("has login") {
        return view.page_hint == "login page";
    }
    if assertion.contains("has password") {
        return view
            .elements
            .iter()
            .any(|e| e.input_type.as_deref() == Some("password"));
    }
    if let Some(rest) = assertion.strip_prefix("title contains ") {
        return view
            .title
            .to_lowercase()
            .contains(rest.trim().trim_matches('"'));
    }
    if let Some(rest) = assertion.strip_prefix("url contains ") {
        return view
            .url
            .to_lowercase()
            .contains(rest.trim().trim_matches('"'));
    }
    if let Some(rest) = assertion.strip_prefix("has button ") {
        let label = rest.trim().trim_matches('"').to_lowercase();
        return view.elements.iter().any(|e| {
            e.kind == semantic::ElementKind::Button && fuzzy_label_match(&e.label, &label)
                || e.value
                    .as_deref()
                    .is_some_and(|v| fuzzy_label_match(v, &label))
        });
    }
    if let Some(rest) = assertion.strip_prefix("has link ") {
        let label = rest.trim().trim_matches('"').to_lowercase();
        return view.elements.iter().any(|e| {
            e.kind == semantic::ElementKind::Link
                && (fuzzy_label_match(&e.label, &label)
                    || e.href
                        .as_deref()
                        .is_some_and(|h| h.to_lowercase().contains(&label)))
        });
    }
    if let Some(rest) = assertion.strip_prefix("has input ") {
        let name = rest.trim().trim_matches('"').to_lowercase();
        return view.elements.iter().any(|e| {
            e.kind == semantic::ElementKind::Input
                && (e
                    .name
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().contains(&name))
                    || e.label.to_lowercase().contains(&name)
                    || e.input_type
                        .as_deref()
                        .is_some_and(|t| t.to_lowercase() == name)
                    || e.placeholder
                        .as_deref()
                        .is_some_and(|p| p.to_lowercase().contains(&name)))
        });
    }
    if assertion == "has form" {
        return !view.forms.is_empty() || view.elements.iter().any(|e| e.form_index.is_some());
    }
    if assertion == "has image" || assertion == "has img" {
        return full_text.contains("img") || full_text.contains("image");
    }
    if let Some(rest) = assertion.strip_prefix("page has section ") {
        let section = rest.trim().trim_matches('"').to_lowercase();
        return view.visible_text.to_lowercase().contains(&section);
    }

    // Fallback: all words present in page
    let words: Vec<&str> = assertion.split_whitespace().collect();
    words.iter().all(|w| full_text.contains(w))
}

/// Normalize assertion word order so callers can write either
/// `"has email input"` or `"has input email"` and get the same result.
fn normalize_assertion(assertion: &str) -> String {
    let a = assertion.trim().to_lowercase();
    let words: Vec<&str> = a.split_whitespace().collect();

    // Pattern: "has <qualifier> input|button|link" → "has input|button|link <qualifier>"
    if words.len() >= 3 && words[0] == "has" {
        let kind_keywords = ["input", "button", "link"];
        if let Some(&last) = words.last()
            && kind_keywords.contains(&last)
            && !kind_keywords.contains(&words[1])
        {
            let qualifier: Vec<&str> = words[1..words.len() - 1].to_vec();
            return format!("has {} {}", last, qualifier.join(" "));
        }
    }
    a
}

/// Fuzzy label match: checks `contains` after stripping non-alphanumeric
/// trailing characters (arrows, icons, extra whitespace).
fn fuzzy_label_match(element_label: &str, target: &str) -> bool {
    let el = element_label.to_lowercase();
    let tgt = target.to_lowercase();
    if el.contains(&tgt) {
        return true;
    }
    // Strip non-alphanumeric chars and retry
    let clean_el: String = el
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .collect();
    let clean_tgt: String = tgt
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .collect();
    clean_el.contains(&clean_tgt)
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
    use super::*;

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
        el.href = Some("https://github.com/example-org".into());
        view.elements.push(el);

        assert!(check_assertion("has link github", &view, ""));
        assert!(check_assertion("has github link", &view, ""));
    }

    #[test]
    fn assert_has_link_by_href() {
        let mut view = empty_view();
        let mut el = make_element(semantic::ElementKind::Link, "Star us");
        el.href = Some("https://github.com/example-org".into());
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
}
