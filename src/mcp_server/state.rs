//! Server and session state types.

use llm_as_dom::engine::PageHandle;
use llm_as_dom::semantic;
use serde::Serialize;

/// A page kept alive between `lad_snapshot` and subsequent interaction tools.
pub(crate) struct ActivePage {
    pub page: Box<dyn PageHandle>,
    pub url: String,
    pub view: semantic::SemanticView,
}

/// Lightweight session state tracked across MCP tool calls.
///
/// Persists auth status, visited URLs, and browse counts between
/// consecutive `lad_browse` invocations within the same MCP session.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct McpSessionState {
    /// Whether the pilot has successfully logged in during this session.
    pub authenticated: bool,
    /// Total number of `lad_browse` calls in this session.
    pub browse_count: u32,
    /// URLs visited during this session (most recent last).
    pub visited_urls: Vec<String>,
    /// Last goal that succeeded (if any).
    pub last_success_goal: Option<String>,
}
