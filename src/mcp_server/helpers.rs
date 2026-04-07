//! Shared helper functions for the MCP server.

/// Read an environment variable with fallback to a deprecated name.
pub(crate) fn read_env_with_fallback(new_name: &str, old_name: &str, default: &str) -> String {
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
pub(crate) fn mcp_err(e: impl std::fmt::Display) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(e.to_string(), None)
}

/// Serialize a value to pretty JSON, returning a fallback on failure.
pub(crate) fn to_pretty_json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

/// Extract origin (scheme + host + port) from a URL string.
pub(crate) fn extract_origin(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .map(|r| ("https", r))
        .or_else(|| url.strip_prefix("http://").map(|r| ("http", r)))?;
    let (scheme, rest) = rest;
    let authority = rest.split('/').next().unwrap_or(rest);
    Some(format!("{scheme}://{authority}"))
}

/// Compare two URLs by origin (scheme + host + port).
pub(crate) fn same_origin(a: &str, b: &str) -> bool {
    match (extract_origin(a), extract_origin(b)) {
        (Some(oa), Some(ob)) => oa == ob,
        _ => false,
    }
}

/// Build "no active page" error.
pub(crate) fn no_active_page() -> rmcp::ErrorData {
    rmcp::ErrorData::invalid_params("no active page — call lad_snapshot first".to_string(), None)
}

/// Check JS eval result for `{ error: "..." }` pattern and surface it.
pub(crate) fn check_js_result(value: &serde_json::Value) -> Result<(), rmcp::ErrorData> {
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
pub(crate) fn key_to_code(key: &str) -> &str {
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

/// Build JS to find an element by `data-lad-id` and execute a body expression.
///
/// Returns an IIFE that queries `[data-lad-id="N"]`, returns an error JSON if
/// not found, otherwise executes `body` and returns `{"ok": true}`.
pub(crate) fn build_element_js(element_id: u32, body: &str) -> String {
    format!(
        r#"(() => {{
            const el = document.querySelector('[data-lad-id="{id}"]');
            if (!el) return JSON.stringify({{ error: "element {id} not found" }});
            {body}
            return JSON.stringify({{ ok: true }});
        }})()"#,
        id = element_id,
        body = body,
    )
}

/// Default network filter value ("all").
pub(crate) fn default_network_filter() -> String {
    "all".to_string()
}
