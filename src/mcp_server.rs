//! `lad-mcp`: MCP server exposing the browser pilot as semantic tools.
//!
//! Provides five tools: `lad_browse`, `lad_extract`, `lad_assert`, `lad_locate`, `lad_audit`.

use llm_as_dom::{Error, a11y, audit, backend, locate, pilot, semantic};

use std::sync::Arc;
use tokio::sync::Mutex;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::schemars;
use rmcp::schemars::JsonSchema;
use rmcp::service::ServiceExt;
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use serde::Deserialize;

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

// ── Server state ───────────────────────────────────────────────────

/// MCP server that manages a headless browser and exposes pilot tools.
#[derive(Clone)]
#[allow(dead_code)] // tool_router is used internally by rmcp macros
struct LadServer {
    /// Router that dispatches MCP tool calls to handler methods.
    tool_router: ToolRouter<Self>,
    /// Shared browser instance (lazy-initialised on first tool call).
    browser: Arc<Mutex<Option<Arc<chromiumoxide::Browser>>>>,
    /// Handle to the CDP event-loop task.
    handler_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Ollama API base URL.
    ollama_url: String,
    /// Model name to use for LLM decisions.
    model: String,
}

impl LadServer {
    /// Create a new server reading config from environment variables.
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            browser: Arc::new(Mutex::new(None)),
            handler_handle: Arc::new(Mutex::new(None)),
            ollama_url: std::env::var("LAD_OLLAMA_URL")
                .unwrap_or_else(|_| "http://localhost:11434".into()),
            model: std::env::var("LAD_MODEL").unwrap_or_else(|_| "qwen2.5:7b".into()),
        }
    }

    /// Return an existing browser or launch a new headless instance.
    async fn ensure_browser(&self) -> Result<Arc<chromiumoxide::Browser>, Error> {
        let mut browser_lock = self.browser.lock().await;
        if let Some(b) = browser_lock.as_ref() {
            return Ok(Arc::clone(b));
        }

        tracing::info!("launching headless browser");
        let tmp = std::env::temp_dir().join(format!("lad-chrome-{}", std::process::id()));
        let config = chromiumoxide::BrowserConfig::builder()
            .arg("--headless=new")
            .arg("--disable-gpu")
            .arg("--no-sandbox")
            .arg("--disable-dev-shm-usage")
            .arg("--window-size=1280,800")
            .arg(format!("--user-data-dir={}", tmp.display()))
            .build()
            .map_err(Error::BrowserStr)?;

        let (browser, mut handler) = chromiumoxide::Browser::launch(config)
            .await
            .map_err(|e| Error::BrowserStr(format!("{e}")))?;

        let handle = tokio::spawn(async move {
            use futures::StreamExt;
            while handler.next().await.is_some() {}
        });

        let b = Arc::new(browser);
        *browser_lock = Some(Arc::clone(&b));
        *self.handler_handle.lock().await = Some(handle);
        Ok(b)
    }

    /// Navigate to a URL and return the page handle with its semantic view.
    async fn navigate_and_extract(
        &self,
        url: &str,
    ) -> Result<(chromiumoxide::Page, semantic::SemanticView), rmcp::ErrorData> {
        let browser = self.ensure_browser().await.map_err(mcp_err)?;
        let page = browser.new_page(url).await.map_err(mcp_err)?;
        page.wait_for_navigation().await.map_err(mcp_err)?;
        a11y::wait_for_content(&page, a11y::DEFAULT_WAIT_TIMEOUT)
            .await
            .map_err(mcp_err)?;

        let view = a11y::extract_semantic_view(&page).await.map_err(mcp_err)?;
        Ok((page, view))
    }
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

        tracing::info!(url = %p.url, "launching page");
        let browser = self.ensure_browser().await.map_err(mcp_err)?;
        let page = browser.new_page(&p.url).await.map_err(mcp_err)?;
        tracing::info!("waiting for navigation");
        page.wait_for_navigation().await.map_err(mcp_err)?;
        tracing::info!("waiting for content to stabilise");
        a11y::wait_for_content(&page, a11y::DEFAULT_WAIT_TIMEOUT)
            .await
            .map_err(mcp_err)?;
        tracing::info!("page ready, initialising pilot");

        let backend = backend::ollama::OllamaBackend::new(&self.ollama_url, &self.model);
        let config = pilot::PilotConfig {
            goal: p.goal.clone(),
            max_steps: p.max_steps,
            use_heuristics: true,
            max_retries_per_step: 2,
        };

        tracing::info!("running pilot");
        let result = pilot::run_pilot(&page, &backend, &config)
            .await
            .map_err(mcp_err)?;
        tracing::info!(
            success = result.success,
            steps = result.steps.len(),
            duration_secs = result.total_duration.as_secs_f64(),
            "pilot complete"
        );

        // Always capture a final screenshot for visual verification.
        tracing::info!("capturing final screenshot");
        let final_screenshot = pilot::take_screenshot(&page).await;

        let output = serde_json::json!({
            "success": result.success,
            "steps": result.steps.len(),
            "heuristic_steps": result.heuristic_hits,
            "llm_steps": result.llm_hits,
            "duration_secs": result.total_duration.as_secs_f64(),
            "final_action": format!("{:?}", result.final_action),
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
        tracing::info!(url = %p.url, what = %p.what, "lad_extract");

        let (_page, view) = self.navigate_and_extract(&p.url).await?;

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
        let result = page.evaluate(js).await.map_err(mcp_err)?;

        let raw: locate::RawLocateResult = result
            .into_value()
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
        let result = page.evaluate(js).await.map_err(mcp_err)?;

        let raw: Vec<audit::RawAuditIssue> = result
            .into_value()
            .map_err(|e| mcp_err(format!("audit JS parse failed: {e:?}")))?;

        let audit_result = audit::parse_audit_result(&p.url, raw);
        let output = serde_json::to_value(&audit_result)
            .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
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
            .with_instructions("lad (LLM-as-DOM) is an AI browser pilot. It navigates web pages autonomously using heuristics + cheap LLM. Use lad_browse for goal-based navigation, lad_extract for page analysis, lad_assert for verification, lad_locate for source mapping, lad_audit for page quality checks.")
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

    tracing::info!("lad-mcp starting (stdio)");

    let server = LadServer::new();
    let transport = rmcp::transport::io::stdio();
    let service = server.serve(transport).await?;
    service.waiting().await?;

    Ok(())
}
