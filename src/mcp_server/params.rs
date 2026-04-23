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
    /// Open the browser window visibly. `None` (omitted) inherits the
    /// current engine state — no restart. `Some(true)` forces headed,
    /// `Some(false)` forces headless. A visibility toggle destroys the
    /// active page, so leave this out unless you need the change.
    #[serde(default)]
    pub visible: Option<bool>,
    /// Wave 2 — reserved for future multi-tab browsing. Currently accepted
    /// by the schema but not consumed: `lad_browse` always opens a fresh
    /// tab and marks it as active. Keeps the shape consistent with all the
    /// other tool params and avoids a schema-breaking addition in Wave 3.
    #[serde(default)]
    #[allow(dead_code)]
    pub tab_id: Option<u32>,
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
    /// Wave 1 — pagination: zero-based page index into `elements`. When set,
    /// only the slice `[page*page_size..(page+1)*page_size]` is returned.
    /// `page` is clamped to `[0, total_pages-1]`; out-of-range becomes empty.
    /// Leave unset to get every element (token-heavy for large pages).
    #[serde(default)]
    pub paginate_index: Option<u32>,
    /// Wave 1 — pagination: elements per page. Default 50. Ignored unless
    /// `paginate_index` is set.
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    /// Wave 1 — hidden-element gate: include DOM elements flagged as hidden
    /// (display:none, opacity:0, aria-hidden, zero bounds). Defaults to
    /// `false` so adversarial pages cannot smuggle prompts via invisible
    /// nodes. Set to `true` when you need the full view (debugging, audit).
    #[serde(default)]
    pub include_hidden: Option<bool>,
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    /// Only meaningful when `url` is `None` (reading an already-open tab).
    #[serde(default)]
    pub tab_id: Option<u32>,
    /// Issue #36 — strict semantic filtering. When `true` AND `what` is
    /// non-empty, drop elements whose relevance score is zero instead of
    /// merely sorting them to the front. Defaults to `false` for
    /// back-compat — existing callers keep getting the full inventory,
    /// just re-ordered. Combines with `paginate_index` / `page_size`:
    /// strict filter runs BEFORE pagination so page 0 is the K most
    /// relevant hits, not the first 50 DOM elements.
    #[serde(default)]
    pub strict: Option<bool>,
    /// FR-2 (friction-log-2026-04-22) — hard cap on returned elements.
    /// Applied AFTER strict filtering but BEFORE pagination, so the
    /// caller gets at most `limit` elements even across all pages.
    /// Response includes `truncated: bool` signaling whether the cap
    /// trimmed anything. When `strict=true` and `limit` is omitted, a
    /// leading numeral in `what` (e.g. "top 5 story titles", "primeiras
    /// 3 histórias") is parsed as an implicit limit. Hard cap is 200;
    /// values above 200 are silently clamped and the response sets
    /// `truncated=true` so the caller can request more explicitly.
    #[serde(default)]
    pub limit: Option<u32>,
    /// BUG-4 + FR-1 (friction-log-2026-04-22) — run the structural-card
    /// detector and emit `view.cards` in the response. Default `false`
    /// keeps response JSON byte-identical (cards field omitted via
    /// `serde(skip_serializing_if = Option::is_none)`). On listing
    /// pages (HN, Reddit, GitHub feeds) where 17 story rows currently
    /// expand to 100+ elements, cards surface the structural grouping
    /// + per-row metadata (points, author, age) without touching the
    /// underlying elements list.
    #[serde(default)]
    pub include_cards: Option<bool>,
}

/// Wave 1 — default page size for `lad_extract` / `lad_snapshot` pagination.
pub(crate) fn default_page_size() -> u32 {
    50
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
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    /// Only meaningful when `url` is `None`.
    #[serde(default)]
    pub tab_id: Option<u32>,
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
    /// BUG-2: when `true`, the audited page is promoted into the tab pool
    /// and its `tab_id` is returned in the response under `audit_tab`.
    /// Follow-up tools (`lad_click`, `lad_scroll`, `lad_snapshot`) can
    /// then target it without re-navigating. Default `false` runs the
    /// audit in an ephemeral tab that is closed immediately after,
    /// leaving the previously active tab (e.g. a logged-in session)
    /// untouched.
    #[serde(default)]
    pub return_tab: Option<bool>,
}

/// Parameters for the `lad_session` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SessionParams {
    /// Action: "get" to view current session state, "clear" to reset,
    /// "attach_cdp" to connect to a running Chrome via CDP (Wave 3),
    /// "detach" to release an attached CDP engine without killing the
    /// user's Chrome (Wave 3).
    pub action: String,
    /// Wave 3 — CDP endpoint URL. Required when `action=attach_cdp`.
    /// Accepts either a raw `ws://` URL or an `http://` debug endpoint
    /// (LAD will auto-resolve via `/json/version`). MUST be loopback
    /// (`localhost`, `127.0.0.1`, `::1`) — any other host is rejected.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Wave 3 — when `action=attach_cdp`, enumerate existing tabs on
    /// the running Chrome and insert them into LAD's tab map. Default
    /// `true`. Set to `false` to start with a clean slate.
    #[serde(default)]
    pub adopt_existing: Option<bool>,
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
    /// Open the browser window visibly. `None` (omitted) inherits the
    /// current engine state — no restart, active page preserved. Only set
    /// this to `Some(true)` on the FIRST call of a session when you need
    /// a visible window. Toggling mid-session destroys the active page.
    #[serde(default)]
    pub visible: Option<bool>,
    /// Hard timeout for the whole snapshot call in milliseconds.
    /// Default: 20000 (20s). Covers engine launch + navigation + content
    /// stabilization. Returns a timeout error instead of hanging if the
    /// target site never stabilizes.
    #[serde(default = "default_snapshot_timeout_ms")]
    pub timeout_ms: u64,
    /// Wave 1 — pagination: zero-based page index into `elements`. See
    /// `ExtractParams::paginate_index` for semantics.
    #[serde(default)]
    pub paginate_index: Option<u32>,
    /// Wave 1 — pagination: elements per page. Default 50.
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    /// Wave 1 — hidden-element gate. See `ExtractParams::include_hidden`.
    #[serde(default)]
    pub include_hidden: Option<bool>,
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    /// Only meaningful when `url` is `None` (re-reading an already-open tab).
    #[serde(default)]
    pub tab_id: Option<u32>,
    /// BUG-4 + FR-1 (friction-log-2026-04-22) — opt in to the
    /// structural-card detector. Same semantics as
    /// `ExtractParams::include_cards`.
    #[serde(default)]
    pub include_cards: Option<bool>,
}

/// Default snapshot hard timeout: 20 seconds.
pub(crate) fn default_snapshot_timeout_ms() -> u64 {
    20_000
}

/// Parameters for the `lad_click` tool.
///
/// Specify EITHER `element` (fast numeric ID from `lad_snapshot`) OR
/// `target` (semantic selector — role/text/label/testid — that survives
/// rerenders and skips the snapshot roundtrip). One is required.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ClickParams {
    /// Element ID from a prior `lad_snapshot`. Fast, stable within a
    /// single snapshot cycle but stale after DOM rerenders.
    #[serde(default)]
    pub element: Option<u32>,
    /// Semantic target spec (role, text, label, testid, ...). Resolved
    /// fresh on every call — survives rerenders. Use when you don't
    /// want a snapshot roundtrip or when the page mutates between
    /// snapshot and click.
    #[serde(default)]
    pub target: Option<llm_as_dom::target::TargetSpec>,
    /// If true, wait for the page to navigate after clicking before taking a new snapshot. Useful for links and submit buttons.
    #[serde(default)]
    pub wait_for_navigation: bool,
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    pub tab_id: Option<u32>,
}

/// Parameters for the `lad_type` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TypeParams {
    /// Element ID from `lad_snapshot`. Mutually exclusive with `target`.
    #[serde(default)]
    pub element: Option<u32>,
    /// Semantic target spec. Mutually exclusive with `element`.
    #[serde(default)]
    pub target: Option<llm_as_dom::target::TargetSpec>,
    /// Text to type into the element. Handles multiline via
    /// `insertText`+`insertLineBreak` on Draft.js/Lexical/ProseMirror.
    pub text: String,
    /// If true, press Enter after typing (saves a separate `lad_press_key` call).
    #[serde(default)]
    pub press_enter: bool,
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    pub tab_id: Option<u32>,
}

/// Parameters for the `lad_select` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SelectParams {
    /// Element ID from `lad_snapshot`. Mutually exclusive with `target`.
    #[serde(default)]
    pub element: Option<u32>,
    /// Semantic target spec. Mutually exclusive with `element`.
    #[serde(default)]
    pub target: Option<llm_as_dom::target::TargetSpec>,
    /// Value to select.
    pub value: String,
    /// If true, wait for the page to navigate after selecting before taking a new snapshot. Useful for dropdowns that auto-submit.
    #[serde(default)]
    pub wait_for_navigation: bool,
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    pub tab_id: Option<u32>,
}

/// Parameters for the `lad_eval` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct EvalParams {
    /// JavaScript expression to evaluate on the active page.
    pub script: String,
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    pub tab_id: Option<u32>,
}

/// Parameters for the `lad_press_key` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct PressKeyParams {
    /// Key name: "Enter", "Tab", "Escape", "ArrowDown", "ArrowUp", "Backspace", "Delete", "Space".
    pub key: String,
    /// Optional element ID from snapshot to focus before pressing the key.
    pub element: Option<u32>,
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    pub tab_id: Option<u32>,
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
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    pub tab_id: Option<u32>,
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
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    pub tab_id: Option<u32>,
}

/// Parameters for the `lad_hover` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct HoverParams {
    /// Element ID from `lad_snapshot`. Mutually exclusive with `target`.
    #[serde(default)]
    pub element: Option<u32>,
    /// Semantic target spec. Mutually exclusive with `element`.
    #[serde(default)]
    pub target: Option<llm_as_dom::target::TargetSpec>,
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    pub tab_id: Option<u32>,
}

/// Wave 5 (Pain #16): default timeout for `lad_back` / `lad_refresh`.
/// 10s is generous enough to cover a real page reload while still bailing
/// out quickly when chromium is hung or there is no history to rewind.
pub(crate) fn default_nav_timeout_ms() -> u64 {
    10_000
}

/// Parameters for the `lad_back` tool.
///
/// Wave 5 (Pain #16): previously this tool took no parameters and would
/// block indefinitely when the page had no history or chromium hung. The
/// timeout gives callers an escape hatch.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct BackParams {
    /// Hard timeout for the whole back-navigate cycle in milliseconds.
    /// Default: 10000 (10s).
    #[serde(default = "default_nav_timeout_ms")]
    pub timeout_ms: u64,
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    #[allow(dead_code)]
    pub tab_id: Option<u32>,
}

impl Default for BackParams {
    fn default() -> Self {
        Self {
            timeout_ms: default_nav_timeout_ms(),
            tab_id: None,
        }
    }
}

/// Parameters for the `lad_refresh` tool.
///
/// Wave 5 (Pain #16): same shape as `BackParams`; both wrap a navigation
/// in `tokio::time::timeout` so a hung chromium can't block the session.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RefreshParams {
    /// Hard timeout for the whole refresh cycle in milliseconds.
    /// Default: 10000 (10s).
    #[serde(default = "default_nav_timeout_ms")]
    pub timeout_ms: u64,
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    #[allow(dead_code)]
    pub tab_id: Option<u32>,
}

impl Default for RefreshParams {
    fn default() -> Self {
        Self {
            timeout_ms: default_nav_timeout_ms(),
            tab_id: None,
        }
    }
}

/// Parameters for the `lad_dialog` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct DialogParams {
    /// Action: "accept", "dismiss", or "status".
    pub action: String,
    /// Optional text to enter for prompt() dialogs (only used with "accept").
    pub text: Option<String>,
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    pub tab_id: Option<u32>,
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
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    pub tab_id: Option<u32>,
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
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    pub tab_id: Option<u32>,
}

/// Parameters for the `lad_upload` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct UploadParams {
    /// Element ID of the file input from a prior lad_snapshot.
    pub element: u32,
    /// Absolute file paths to upload.
    pub files: Vec<String>,
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    pub tab_id: Option<u32>,
}

/// Parameters for the Wave 1 `lad_jq` tool.
///
/// Runs a jq expression against the current active page's `SemanticView`
/// (the same JSON shape emitted by `lad_snapshot` / `lad_extract`). Lets
/// callers pull out exactly the slice they need (a list of button labels,
/// a form's fields, a count) without paying the 10-30x token cost of
/// pulling the whole snapshot into the prompt.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct JqParams {
    /// jq expression, e.g. `.title` or
    /// `.elements | map(select(.kind == "button")) | map(.label)`.
    pub query: String,
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    pub tab_id: Option<u32>,
}

/// Parameters for the `lad_clear` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ClearParams {
    /// Element ID from `lad_snapshot`. Mutually exclusive with `target`.
    #[serde(default)]
    pub element: Option<u32>,
    /// Semantic target spec. Mutually exclusive with `element`.
    #[serde(default)]
    pub target: Option<llm_as_dom::target::TargetSpec>,
    /// Wave 2 — target tab ID. Defaults to the active tab when omitted.
    #[serde(default)]
    pub tab_id: Option<u32>,
}

// ── Wave 2: tab management ──────────────────────────────────

/// Parameters for the `lad_tabs_list` tool. Takes no arguments — listed
/// as an empty struct so the rmcp macro generates a JSON schema for it.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(crate) struct TabsListParams {}

/// Parameters for the `lad_tabs_switch` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TabSwitchParams {
    /// ID of the tab to make active. Must exist in the current session.
    pub tab_id: u32,
}

/// Parameters for the `lad_tabs_close` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TabCloseParams {
    /// ID of the tab to close. If this was the active tab, the active tab
    /// slot is cleared. Must exist in the current session.
    pub tab_id: u32,
}

// ── Wave 5 (Pain #16): nav timeout param tests ──────────────────
#[cfg(test)]
mod nav_param_tests {
    use super::*;

    #[test]
    fn back_params_default_timeout_is_10s() {
        assert_eq!(BackParams::default().timeout_ms, 10_000);
        assert!(BackParams::default().tab_id.is_none());
    }

    #[test]
    fn refresh_params_default_timeout_is_10s() {
        assert_eq!(RefreshParams::default().timeout_ms, 10_000);
        assert!(RefreshParams::default().tab_id.is_none());
    }

    #[test]
    fn back_params_roundtrip_preserves_fields() {
        let json = r#"{"timeout_ms":2500,"tab_id":7}"#;
        let p: BackParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.timeout_ms, 2500);
        assert_eq!(p.tab_id, Some(7));
    }

    #[test]
    fn refresh_params_roundtrip_preserves_fields() {
        let json = r#"{"timeout_ms":3000,"tab_id":2}"#;
        let p: RefreshParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.timeout_ms, 3000);
        assert_eq!(p.tab_id, Some(2));
    }

    #[test]
    fn back_params_empty_object_uses_defaults() {
        let p: BackParams = serde_json::from_str("{}").unwrap();
        assert_eq!(p.timeout_ms, 10_000);
        assert!(p.tab_id.is_none());
    }

    #[test]
    fn refresh_params_empty_object_uses_defaults() {
        let p: RefreshParams = serde_json::from_str("{}").unwrap();
        assert_eq!(p.timeout_ms, 10_000);
        assert!(p.tab_id.is_none());
    }
}
