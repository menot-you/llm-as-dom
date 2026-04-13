//! `llm-as-dom-mcp`: MCP server exposing the browser pilot as semantic tools.
//!
//! Provides six tools: `lad_browse`, `lad_extract`, `lad_assert`, `lad_locate`, `lad_audit`, `lad_session`.

use llm_as_dom::engine::chromium::ChromiumEngine;
use llm_as_dom::engine::webkit::WebKitEngine;
use llm_as_dom::engine::{BrowserEngine, EngineConfig, PageHandle};
use llm_as_dom::{a11y, audit, backend, locate, pilot, semantic};

use std::sync::Arc;
use tokio::sync::Mutex;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::schemars;
use rmcp::schemars::JsonSchema;
use rmcp::service::ServiceExt;
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

/// Check if a URL is safe to fetch (SSRF protection).
fn is_safe_url(url: &str) -> bool {
    let allow_local = std::env::var("LAD_ALLOW_LOCAL_URLS").unwrap_or_default() == "true";
    if allow_local {
        return true;
    }

    if let Ok(parsed) = url::Url::parse(url) {
        if parsed.scheme() == "file" {
            return false;
        }
        if let Some(host) = parsed.host_str() {
            let host_lower = host.to_lowercase();
            // Basic SSRF blocks:
            if host_lower == "localhost" 
                || host_lower == "127.0.0.1" 
                || host_lower == "[::1]"
                || host_lower == "0.0.0.0" 
            {
                return false;
            }
            if host_lower.starts_with("169.254.") 
                || host_lower.starts_with("10.")  
                || host_lower.starts_with("192.168.") 
            {
                return false;
            }
        }
    }
    true
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
    /// Action: "start" or "stop".
    action: String,
    /// URL to watch (only needed for start).
    url: Option<String>,
    /// Polling interval in ms (default: 1000).
    interval_ms: Option<u32>,
    /// JavaScript to evaluate periodically.
    script: Option<String>,
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
        if !is_safe_url(url) {
            return Err(rmcp::ErrorData::invalid_params("SEC-002: SSRF defense active - Disallowed URL. Set LAD_ALLOW_LOCAL_URLS=true to bypass.", None));
        }

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

        if !is_safe_url(&p.url) {
            return Err(rmcp::ErrorData::invalid_params("SEC-002: SSRF defense active - Disallowed URL. Set LAD_ALLOW_LOCAL_URLS=true to bypass.", None));
        }

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
        description = "Watch page state over time: start polling a URL with interval_ms and optional JS script, or stop watching. Pushes events to log."
    )]
    async fn lad_watch(
        &self,
        params: Parameters<WatchParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(action = %p.action, "lad_watch");

        if p.action == "start" {
            let url = p.url.as_deref().unwrap_or("about:blank");
            let (page, _view) = self.navigate_and_extract(url).await?;
            let _ = page.enable_network_monitoring().await;

            let interval = p.interval_ms.unwrap_or(1000);
            let script = p.script.as_deref().unwrap_or(
                "return { w: window.innerWidth, h: window.innerHeight, title: document.title, location: window.location.href };"
            );

            page.start_monitoring(interval, script)
                .await
                .map_err(mcp_err)?;

            Ok(CallToolResult::success(vec![Content::text(format!(
                "Started watching {} every {}ms. Check sidecar terminal for push events.",
                url, interval
            ))]))
        } else if p.action == "stop" {
            if let Some(engine) = self.engine.lock().await.as_ref() {
                // To stop monitoring, we just tell the bridge to stop it globally for the page
                // But we don't hold a persistent `PageHandle` per se in the MCP server if we didn't store it.
                // Re-creating or re-getting the last tracked page?
                // For MVP, we'll navigate to blank or just ask the engine to stop monitoring the active view.
                // We'll create a dummy page to send the command to the bridge.
                let page = engine.new_page("about:blank").await.map_err(mcp_err)?;
                page.stop_monitoring().await.map_err(mcp_err)?;
            }
            Ok(CallToolResult::success(vec![Content::text(
                "Stopped monitoring".to_string(),
            )]))
        } else {
            Err(rmcp::ErrorData::invalid_params(
                "action must be start or stop",
                None,
            ))
        }
    }
}

/// Evaluate a single assertion against a semantic view.
///
/// Supported patterns:
/// - `has login form` / `has login`
/// - `has password`
/// - `title contains <text>`
/// - `url contains <text>`
/// - `has button <label>`
/// - `has link <label>`
/// - `has input <name>`
/// - Fallback: all words present in combined page text.
fn check_assertion(assertion: &str, view: &semantic::SemanticView, prompt_text: &str) -> bool {
    let full_text = format!(
        "{} {} {} {}",
        view.url, view.title, view.visible_text, prompt_text
    )
    .to_lowercase();

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
            e.kind == semantic::ElementKind::Button
                && (e.label.to_lowercase().contains(&label)
                    || e.value
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&label))
        });
    }
    if let Some(rest) = assertion.strip_prefix("has link ") {
        let label = rest.trim().trim_matches('"').to_lowercase();
        return view.elements.iter().any(|e| {
            e.kind == semantic::ElementKind::Link && e.label.to_lowercase().contains(&label)
        });
    }
    if let Some(rest) = assertion.strip_prefix("has input ") {
        let name = rest.trim().trim_matches('"').to_lowercase();
        return view.elements.iter().any(|e| {
            e.kind == semantic::ElementKind::Input
                && (e
                    .name
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&name)
                    || e.label.to_lowercase().contains(&name))
        });
    }

    // Fallback: all words present in page
    let words: Vec<&str> = assertion.split_whitespace().collect();
    words.iter().all(|w| full_text.contains(w))
}

// ── ServerHandler ──────────────────────────────────────────────────

#[tool_handler]
impl ServerHandler for LadServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("lad (LLM-as-DOM) is an AI browser pilot. It navigates web pages autonomously using heuristics + cheap LLM. Use lad_browse for goal-based navigation, lad_extract for page analysis, lad_assert for verification, lad_locate for source mapping, lad_audit for page quality checks, lad_session for session state inspection/reset.")
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
