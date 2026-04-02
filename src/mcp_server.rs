//! lad-mcp: MCP server exposing the browser pilot as semantic tools.
//! 3 tools: lad_browse, lad_extract, lad_assert

mod a11y;
mod backend;
mod error;
mod heuristics;
mod pilot;
mod semantic;

pub use error::Error;

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::schemars;
use rmcp::schemars::JsonSchema;
use rmcp::service::ServiceExt;
use rmcp::{ServerHandler, tool, tool_router};
use serde::Deserialize;

// ── Tool parameter types ───────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
struct BrowseParams {
    /// URL to navigate to
    url: String,
    /// Goal in natural language (e.g. "login as user@test.com with password secret123")
    goal: String,
    /// Max steps before giving up (default: 10)
    #[serde(default = "default_max_steps")]
    max_steps: u32,
}
fn default_max_steps() -> u32 { 10 }

#[derive(Debug, Deserialize, JsonSchema)]
struct ExtractParams {
    /// URL to navigate to and extract from
    url: String,
    /// What to extract (e.g. "product prices", "form fields", "navigation links")
    what: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AssertParams {
    /// URL to navigate to and check
    url: String,
    /// Assertions to verify (e.g. ["has login form", "title contains Dashboard", "has button Sign In"])
    assertions: Vec<String>,
}

// ── Server state ───────────────────────────────────────────────────

#[derive(Clone)]
struct LadServer {
    tool_router: ToolRouter<Self>,
    browser: Arc<Mutex<Option<Arc<chromiumoxide::Browser>>>>,
    handler_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    ollama_url: String,
    model: String,
}

impl LadServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            browser: Arc::new(Mutex::new(None)),
            handler_handle: Arc::new(Mutex::new(None)),
            ollama_url: std::env::var("LAD_OLLAMA_URL")
                .unwrap_or_else(|_| "http://localhost:11434".into()),
            model: std::env::var("LAD_MODEL")
                .unwrap_or_else(|_| "qwen3:8b".into()),
        }
    }

    async fn ensure_browser(&self) -> Result<Arc<chromiumoxide::Browser>, Error> {
        let mut browser_lock = self.browser.lock().await;
        if let Some(b) = browser_lock.as_ref() {
            return Ok(b.clone());
        }

        tracing::info!("launching headless browser");
        let config = chromiumoxide::BrowserConfig::builder()
            .arg("--headless=new")
            .arg("--disable-gpu")
            .arg("--no-sandbox")
            .arg("--disable-dev-shm-usage")
            .arg("--window-size=1280,800")
            .build()
            .map_err(|e| Error::BrowserStr(e))?;

        let (browser, mut handler) = chromiumoxide::Browser::launch(config)
            .await
            .map_err(|e| Error::BrowserStr(format!("{e}")))?;

        let handle = tokio::spawn(async move {
            use futures::StreamExt;
            loop {
                if handler.next().await.is_none() {
                    break;
                }
            }
        });

        
        let b = Arc::new(browser);
        *browser_lock = Some(b.clone());
        *self.handler_handle.lock().await = Some(handle);
        Ok(browser_lock.as_ref().unwrap().clone())
    }

    async fn navigate_and_extract(
        &self,
        url: &str,
    ) -> Result<(chromiumoxide::Page, semantic::SemanticView), rmcp::ErrorData> {
        let browser = self.ensure_browser().await.map_err(mcp_err)?;
        let page = browser.new_page(url).await.map_err(mcp_err)?;
        page.wait_for_navigation().await.map_err(mcp_err)?;
        tokio::time::sleep(Duration::from_secs(2)).await;

        let view = a11y::extract_semantic_view(&page).await.map_err(mcp_err)?;
        Ok((page, view))
    }
}

fn mcp_err(e: impl std::fmt::Display) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(e.to_string(), None)
}

// ── Tool implementations ───────────────────────────────────────────

#[tool_router]
impl LadServer {
    /// Browse a URL and accomplish a goal autonomously.
    /// The pilot uses heuristics + cheap LLM to navigate, fill forms, click buttons.
    /// Returns structured result: success/failure, steps taken, timing.
    #[tool(description = "Navigate to a URL and accomplish a browsing goal autonomously (login, fill form, click, search). Returns structured result.")]
    async fn lad_browse(
        &self,
        params: Parameters<BrowseParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(url = %p.url, goal = %p.goal, "lad_browse");

        let browser = self.ensure_browser().await.map_err(mcp_err)?;
        let page = browser.new_page(&p.url).await.map_err(mcp_err)?;
        page.wait_for_navigation().await.map_err(mcp_err)?;
        tokio::time::sleep(Duration::from_secs(2)).await;

        let backend = backend::ollama::OllamaBackend::new(&self.ollama_url, &self.model);
        let config = pilot::PilotConfig {
            goal: p.goal.clone(),
            max_steps: p.max_steps,
            step_timeout: Duration::from_secs(30),
            use_heuristics: true,
        };

        let result = pilot::run_pilot(&page, &backend, &config)
            .await
            .map_err(mcp_err)?;

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

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&output).unwrap(),
        )]))
    }

    /// Extract structured information from a web page.
    /// Returns interactive elements, visible text, page classification.
    /// Never returns raw HTML.
    #[tool(description = "Extract structured info from a URL: interactive elements, text, page type. Never returns raw HTML.")]
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
            "visible_text": view.visible_text,
            "query": p.what,
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&output).unwrap(),
        )]))
    }

    /// Assert conditions about a web page and return pass/fail results.
    /// Supports: "has login form", "title contains X", "has button Y", "has input Z", etc.
    #[tool(description = "Check assertions on a URL. Returns pass/fail for each. Supports: has login form, title contains X, has button Y, url contains Z.")]
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

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&serde_json::json!({
                "url": view.url,
                "title": view.title,
                "all_pass": all_pass,
                "results": results,
            })).unwrap(),
        )]))
    }
}

fn check_assertion(assertion: &str, view: &semantic::SemanticView, prompt_text: &str) -> bool {
    let full_text = format!("{} {} {} {}", view.url, view.title, view.visible_text, prompt_text).to_lowercase();

    if assertion.contains("has login form") || assertion.contains("has login") {
        return view.page_hint == "login page";
    }
    if assertion.contains("has password") {
        return view.elements.iter().any(|e| e.input_type.as_deref() == Some("password"));
    }
    if let Some(rest) = assertion.strip_prefix("title contains ") {
        return view.title.to_lowercase().contains(rest.trim().trim_matches('"'));
    }
    if let Some(rest) = assertion.strip_prefix("url contains ") {
        return view.url.to_lowercase().contains(rest.trim().trim_matches('"'));
    }
    if let Some(rest) = assertion.strip_prefix("has button ") {
        let label = rest.trim().trim_matches('"').to_lowercase();
        return view.elements.iter().any(|e| {
            e.kind == semantic::ElementKind::Button
                && (e.label.to_lowercase().contains(&label)
                    || e.value.as_deref().unwrap_or("").to_lowercase().contains(&label))
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
                && (e.name.as_deref().unwrap_or("").to_lowercase().contains(&name)
                    || e.label.to_lowercase().contains(&name))
        });
    }

    // Fallback: all words present in page
    let words: Vec<&str> = assertion.split_whitespace().collect();
    words.iter().all(|w| full_text.contains(w))
}

// ── ServerHandler ──────────────────────────────────────────────────

impl ServerHandler for LadServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("lad (LLM-as-DOM) is an AI browser pilot. It navigates web pages autonomously using heuristics + cheap LLM. Use lad_browse for goal-based navigation, lad_extract for page analysis, lad_assert for verification.")
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
