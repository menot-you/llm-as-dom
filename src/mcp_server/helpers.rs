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

/// Parse LAD_WINDOW_SIZE env var ("WIDTHxHEIGHT", e.g. "1920x1080").
/// Falls back to detecting screen resolution on macOS via `system_profiler`.
pub(crate) fn parse_window_size_env() -> Option<(u32, u32)> {
    // 1. Explicit env var.
    if let Ok(val) = std::env::var("LAD_WINDOW_SIZE") {
        let parts: Vec<&str> = val.split('x').collect();
        if parts.len() == 2
            && let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>())
            && w >= 320
            && h >= 240
        {
            return Some((w, h));
        }
        tracing::warn!(val = %val, "invalid LAD_WINDOW_SIZE — expected WIDTHxHEIGHT (e.g. 1920x1080)");
    }
    // 2. Auto-detect screen resolution on macOS.
    detect_screen_size()
}

/// Detect largest screen resolution via macOS `system_profiler`.
/// Uses the largest display found (external monitor over laptop).
fn detect_screen_size() -> Option<(u32, u32)> {
    let output = std::process::Command::new("system_profiler")
        .arg("SPDisplaysDataType")
        .arg("-json")
        .output()
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let displays = json.get("SPDisplaysDataType")?.as_array()?;

    let mut best: Option<(u32, u32)> = None;

    for gpu in displays {
        if let Some(screens) = gpu.get("spdisplays_ndrvs").and_then(|v| v.as_array()) {
            for screen in screens {
                if let Some(res) = screen
                    .get("_spdisplays_resolution")
                    .and_then(|v| v.as_str())
                {
                    // Format: "3440 x 1440 @ 120.00Hz" or "1352 x 878 @ 120.00Hz"
                    // Strip everything after " @" before parsing.
                    let clean = res.split(" @").next().unwrap_or(res);
                    let parts: Vec<&str> = clean.split(" x ").collect();
                    if parts.len() == 2
                        && let (Ok(w), Ok(h)) = (
                            parts[0].trim().parse::<u32>(),
                            parts[1].trim().parse::<u32>(),
                        )
                    {
                        // Pick the largest display (external monitor > laptop).
                        if best.is_none_or(|(bw, bh)| w * h > bw * bh) {
                            best = Some((w, h));
                        }
                    }
                }
            }
        }
    }

    if let Some((w, h)) = best {
        // Cap at 1920x1080 — ultrawide/retina resolutions make sites render
        // tiny modals in a corner. Most web content is designed for ≤1920px.
        let w = w.min(1920);
        let h = h.min(1080);
        tracing::info!(
            width = w,
            height = h,
            "auto-detected screen size (capped at 1920x1080)"
        );
        return Some((w, h));
    }
    None
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
    rmcp::ErrorData::invalid_params(
        "no active page — call lad_snapshot or lad_browse first".to_string(),
        None,
    )
}

/// Build a JS IIFE that locates an element and runs `body` with `el`
/// bound to the match. Accepts EITHER a numeric `data-lad-id` (fast
/// path, stable between snapshot and interaction) OR a semantic
/// `TargetSpec` (slow path but eliminates the snapshot roundtrip and
/// survives rerenders). At least one must be `Some`.
pub(crate) fn build_element_js_or_target(
    element_id: Option<u32>,
    target: Option<&llm_as_dom::target::TargetSpec>,
    body: &str,
) -> Result<String, rmcp::ErrorData> {
    match (element_id, target) {
        (Some(id), _) => Ok(build_element_js(id, body)),
        (None, Some(spec)) if spec.is_populated() => Ok(llm_as_dom::target::build_target_js(spec, body)),
        _ => Err(rmcp::ErrorData::invalid_params(
            "must provide either `element` (numeric ID from lad_snapshot) or `target` (semantic selector: role/text/label/testid)".to_string(),
            None,
        )),
    }
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
/// Returns an IIFE that uses `deepQuerySelector` to search shadow roots and
/// iframe contentDocuments recursively (mirrors `deepQueryAll` in a11y.rs).
/// Falls back to `document.querySelector` if the deep search returns nothing.
pub(crate) fn build_element_js(element_id: u32, body: &str) -> String {
    format!(
        r#"(() => {{
            // FIX-6 + CHAOS-03: deepQuerySelector — searches shadow roots and iframes
            // with maxDepth=5 to prevent unbounded recursion.
            function deepQuerySelector(root, sel, depth) {{
                if (depth === undefined) depth = 0;
                if (depth > 5) return null;
                const found = root.querySelector(sel);
                if (found) return found;
                const all = root.querySelectorAll('*');
                for (const node of all) {{
                    if (node.shadowRoot) {{
                        const sr = deepQuerySelector(node.shadowRoot, sel, depth + 1);
                        if (sr) return sr;
                    }}
                }}
                // Same-origin iframes
                const iframes = root.querySelectorAll('iframe');
                for (const iframe of iframes) {{
                    try {{
                        if (iframe.contentDocument) {{
                            const ir = deepQuerySelector(iframe.contentDocument, sel, depth + 1);
                            if (ir) return ir;
                        }}
                    }} catch(_) {{}}
                }}
                return null;
            }}
            const el = deepQuerySelector(document, '[data-lad-id="{id}"]');
            if (!el) return JSON.stringify({{ error: "element {id} not found" }});
            {body}
            return JSON.stringify({{ ok: true }});
        }})()"#,
        id = element_id,
        body = body,
    )
}
