//! MCP tool parameter types.

use rmcp::schemars;
use rmcp::schemars::JsonSchema;
use serde::Deserialize;

/// Parameters for the `lad_browse` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct BrowseParams {
    /// URL to navigate to.
    pub url: String,
    /// Goal in natural language (e.g. "login as user@test.com with password secret123").
    pub goal: String,
    /// Max steps before giving up (default: 10).
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
    /// Optional maximum length of the HTML/DOM text embedded into the prompt.
    pub max_length: Option<usize>,
    /// Open the browser window visibly (default: false = headless).
    /// When toggled, restarts the browser engine with the new mode.
    #[serde(default)]
    pub visible: bool,
}

/// Default step limit for browsing goals.
fn default_max_steps() -> u32 {
    10
}

/// Parameters for the `lad_extract` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ExtractParams {
    /// URL to navigate to and extract from. If omitted and there is an active page
    /// (from a prior `lad_browse` or `lad_snapshot`), re-extracts from the current
    /// page without navigating — preserving session state (logged-in pages, etc.).
    #[serde(default)]
    pub url: Option<String>,
    /// What to extract (e.g. "product prices", "form fields", "navigation links").
    pub what: String,
    /// Optional maximum length of the HTML/DOM text embedded into the prompt.
    pub max_length: Option<usize>,
    /// Output format: "json" (default, structured JSON) or "prompt" (compact text like lad_snapshot).
    #[serde(default)]
    pub format: Option<String>,
}

/// Parameters for the `lad_assert` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AssertParams {
    /// URL to navigate to and check. If omitted and there is an active page
    /// (from a prior `lad_browse` or `lad_snapshot`), asserts against the current
    /// page without navigating — preserving session state.
    #[serde(default)]
    pub url: Option<String>,
    /// Assertions to verify (e.g. ["has login form", "title contains Dashboard"]).
    pub assertions: Vec<String>,
}

/// Parameters for the `lad_locate` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LocateParams {
    /// URL to navigate to.
    pub url: String,
    /// CSS selector or text description of the element to locate.
    pub selector: String,
}

/// Parameters for the `lad_audit` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AuditParams {
    /// URL to audit.
    pub url: String,
    /// Categories to check: "a11y", "forms", "links" (default: all).
    #[serde(default = "llm_as_dom::audit::default_categories")]
    pub categories: Vec<String>,
}

/// Parameters for the `lad_session` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SessionParams {
    /// Action: "get" to view current session state, "clear" to reset.
    pub action: String,
}

/// Parameters for the `lad_watch` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct WatchParams {
    /// Action: "start", "stop", or "events".
    pub action: String,
    /// URL to watch (only needed for start).
    pub url: Option<String>,
    /// Polling interval in ms (default: 1000).
    pub interval_ms: Option<u32>,
    /// For "events" action: return only events with seq > since_seq.
    pub since_seq: Option<u64>,
}

/// Parameters for the `lad_snapshot` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SnapshotParams {
    /// URL to navigate to. If omitted and there is an active page (from a prior
    /// `lad_browse` or `lad_snapshot`), re-extracts the current page without navigating.
    #[serde(default)]
    pub url: Option<String>,
    /// Open the browser window visibly (default: false = headless).
    /// When toggled, restarts the browser engine with the new mode.
    #[serde(default)]
    pub visible: bool,
}

/// Parameters for the `lad_click` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ClickParams {
    /// Element ID from snapshot.
    pub element: u32,
    /// If true, wait for the page to navigate after clicking before taking a new snapshot. Useful for links and submit buttons.
    #[serde(default)]
    pub wait_for_navigation: bool,
}

/// Parameters for the `lad_type` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TypeParams {
    /// Element ID from snapshot.
    pub element: u32,
    /// Text to type into the element.
    pub text: String,
    /// If true, press Enter after typing (saves a separate `lad_press_key` call).
    #[serde(default)]
    pub press_enter: bool,
}

/// Parameters for the `lad_select` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SelectParams {
    /// Element ID from snapshot.
    pub element: u32,
    /// Value to select.
    pub value: String,
    /// If true, wait for the page to navigate after selecting before taking a new snapshot. Useful for dropdowns that auto-submit.
    #[serde(default)]
    pub wait_for_navigation: bool,
}

/// Parameters for the `lad_eval` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct EvalParams {
    /// JavaScript expression to evaluate on the active page.
    pub script: String,
}

/// Parameters for the `lad_press_key` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct PressKeyParams {
    /// Key name: "Enter", "Tab", "Escape", "ArrowDown", "ArrowUp", "Backspace", "Delete", "Space".
    pub key: String,
    /// Optional element ID from snapshot to focus before pressing the key.
    pub element: Option<u32>,
}

/// Parameters for the `lad_wait` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct WaitParams {
    /// Natural language condition, e.g. "has button Dashboard", "title contains Welcome".
    /// Used as a single condition. If `conditions` is also provided, this is prepended.
    #[serde(default)]
    pub condition: Option<String>,
    /// Multiple conditions to wait for. Use with `mode` to control matching.
    #[serde(default)]
    pub conditions: Option<Vec<String>>,
    /// Matching mode: "all" (default) waits for ALL conditions, "any" returns on first match.
    #[serde(default)]
    pub mode: Option<String>,
    /// Max wait time in ms (default: 10000).
    #[serde(default = "default_wait_timeout")]
    pub timeout_ms: u64,
    /// Poll interval in ms (default: 500).
    #[serde(default = "default_wait_poll")]
    pub poll_ms: u64,
}

fn default_wait_timeout() -> u64 {
    10_000
}
fn default_wait_poll() -> u64 {
    500
}

/// FIX-17: Default network filter value ("all") — moved here from helpers.rs
/// since it's only used as a serde default for `NetworkParams`.
fn default_network_filter() -> String {
    "all".to_string()
}

/// Parameters for the `lad_network` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct NetworkParams {
    /// Filter by request kind: "auth", "api", "navigation", "asset", or "all" (default).
    #[serde(default = "default_network_filter")]
    pub filter: String,
}

/// Parameters for the `lad_hover` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct HoverParams {
    /// Element ID from a prior lad_snapshot.
    pub element: u32,
}

/// Parameters for the `lad_dialog` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct DialogParams {
    /// Action: "accept", "dismiss", or "status".
    pub action: String,
    /// Optional text to enter for prompt() dialogs (only used with "accept").
    pub text: Option<String>,
}

/// Default scroll direction.
fn default_scroll_direction() -> String {
    "down".to_string()
}

/// Default scroll pixel amount.
fn default_scroll_pixels() -> u32 {
    600
}

/// Parameters for the `lad_scroll` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ScrollParams {
    /// Direction: "down", "up", "bottom", "top". Default: "down".
    #[serde(default = "default_scroll_direction")]
    pub direction: String,
    /// Scroll to a specific element by its ID from a prior snapshot.
    #[serde(default)]
    pub element: Option<u32>,
    /// Custom scroll amount in pixels (only for "up"/"down"). Default: 600.
    #[serde(default = "default_scroll_pixels")]
    pub pixels: u32,
}

/// Parameters for the `lad_fill_form` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FillFormParams {
    /// Field-value pairs. Keys match element labels, names, or placeholders
    /// (case-insensitive). Example: `{"Email": "user@test.com", "Password": "secret"}`.
    pub fields: std::collections::HashMap<String, String>,
    /// Submit the form after filling (clicks the submit button).
    #[serde(default)]
    pub submit: bool,
    /// Optional form index (for pages with multiple forms). Matches `form_index`
    /// from the semantic view. When omitted, searches all elements.
    #[serde(default)]
    pub form_index: Option<u32>,
}

/// Parameters for the `lad_upload` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct UploadParams {
    /// Element ID of the file input from a prior lad_snapshot.
    pub element: u32,
    /// Absolute file paths to upload.
    pub files: Vec<String>,
}

/// Parameters for the `lad_clear` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ClearParams {
    /// Element ID from a prior lad_snapshot.
    pub element: u32,
}
