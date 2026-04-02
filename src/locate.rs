//! Source location engine for mapping DOM elements to source files.
//!
//! Checks React dev-mode `__source`, `data-ds` (domscribe), `data-lad` hints,
//! and falls back to a DOM path when no source information is available.

use serde::{Deserialize, Serialize};

/// Source file location for an element.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceLocation {
    /// Source file path (e.g. `"src/LoginForm.tsx"`).
    pub file: String,
    /// Line number in the source file.
    pub line: u32,
}

/// CSS source location for a matched style rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CssSource {
    /// CSS file path.
    pub file: String,
    /// CSS property name.
    pub property: String,
    /// Line number.
    pub line: u32,
}

/// Information about the located element.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementInfo {
    /// HTML tag name.
    pub tag: String,
    /// Visible text content (truncated).
    pub text: String,
    /// CSS selector that uniquely identifies this element.
    pub selector: String,
}

/// Complete locate result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocateResult {
    /// Information about the matched element.
    pub element: ElementInfo,
    /// Source file location (if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceLocation>,
    /// CSS source locations (if available).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub css: Vec<CssSource>,
    /// DOM path fallback selector.
    pub fallback: String,
}

/// Build the JS script that locates an element and extracts source info.
///
/// The script finds the element by CSS selector, then checks:
/// 1. React `__reactFiber$*` / `__source` (dev mode)
/// 2. `data-ds` attribute (domscribe)
/// 3. `data-lad` attribute (lad hints)
/// 4. DOM path as fallback
pub fn build_locate_js(selector: &str) -> String {
    // Escape the selector for embedding in JS
    let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");

    format!(
        r#"(() => {{
    const selector = '{escaped}';

    // Try CSS selector first, then text content match
    let el = document.querySelector(selector);
    if (!el) {{
        // Try finding by text content
        const allEls = document.querySelectorAll('*');
        for (const candidate of allEls) {{
            const text = (candidate.textContent || '').trim();
            if (text === selector || text.toLowerCase().includes(selector.toLowerCase())) {{
                // Prefer interactive elements
                const tag = candidate.tagName.toLowerCase();
                if (['button', 'a', 'input', 'select', 'textarea'].includes(tag)
                    || candidate.getAttribute('role') === 'button') {{
                    el = candidate;
                    break;
                }}
                if (!el) el = candidate;
            }}
        }}
    }}

    if (!el) {{
        return {{ error: 'Element not found: ' + selector }};
    }}

    // Element info
    const tag = el.tagName.toLowerCase();
    const text = (el.textContent || '').trim().substring(0, 80);

    // Build a unique CSS selector
    function buildSelector(element) {{
        if (element.id) return tag + '#' + element.id;
        const parts = [];
        let cur = element;
        for (let depth = 0; depth < 5 && cur && cur !== document.body; depth++) {{
            let seg = cur.tagName.toLowerCase();
            if (cur.id) {{ parts.unshift(seg + '#' + cur.id); break; }}
            const cls = cur.className && typeof cur.className === 'string'
                ? '.' + cur.className.trim().split(/\s+/).slice(0, 2).join('.')
                : '';
            if (cls) seg += cls;
            parts.unshift(seg);
            cur = cur.parentElement;
        }}
        return parts.join(' > ');
    }}

    const uniqueSelector = buildSelector(el);

    // Build DOM path fallback
    function buildPath(element) {{
        const parts = [];
        let cur = element;
        while (cur && cur !== document) {{
            let seg = cur.tagName.toLowerCase();
            if (cur.id) {{ parts.unshift(seg + '#' + cur.id); break; }}
            const cls = cur.className && typeof cur.className === 'string'
                ? '.' + cur.className.trim().split(/\s+/).slice(0, 2).join('.')
                : '';
            if (cls) seg += cls;
            parts.unshift(seg);
            cur = cur.parentElement;
        }}
        return parts.join(' > ');
    }}
    const fallback = buildPath(el);

    // Source detection
    let source = null;

    // 1. React dev mode: __reactFiber$ or __reactInternalInstance$ props
    const fiberKey = Object.keys(el).find(k =>
        k.startsWith('__reactFiber$') || k.startsWith('__reactInternalInstance$')
    );
    if (fiberKey) {{
        let fiber = el[fiberKey];
        // Walk up the fiber tree to find _debugSource
        for (let i = 0; i < 10 && fiber; i++) {{
            if (fiber._debugSource) {{
                source = {{
                    file: fiber._debugSource.fileName || fiber._debugSource.file || '',
                    line: fiber._debugSource.lineNumber || fiber._debugSource.line || 0,
                }};
                break;
            }}
            fiber = fiber.return || fiber._debugOwner;
        }}
    }}

    // 2. data-ds (domscribe)
    if (!source) {{
        const ds = el.getAttribute('data-ds');
        if (ds) {{
            const parts = ds.split(':');
            if (parts.length >= 2) {{
                source = {{ file: parts[0], line: parseInt(parts[1], 10) || 0 }};
            }}
        }}
    }}

    // 3. data-lad with source info
    if (!source) {{
        const lad = el.getAttribute('data-lad');
        if (lad && lad.startsWith('source:')) {{
            const val = lad.substring(7);
            const parts = val.split(':');
            if (parts.length >= 2) {{
                source = {{ file: parts[0], line: parseInt(parts[1], 10) || 0 }};
            }}
        }}
    }}

    return {{
        element: {{ tag, text, selector: uniqueSelector }},
        source: source,
        css: [],
        fallback: fallback,
    }};
}})()"#
    )
}

/// Raw JS result for locate.
#[derive(Debug, Deserialize)]
pub struct RawLocateResult {
    /// Element info.
    #[serde(default)]
    pub element: Option<RawElementInfo>,
    /// Source location.
    #[serde(default)]
    pub source: Option<SourceLocation>,
    /// CSS sources (currently empty, placeholder for v0.3).
    #[serde(default)]
    pub css: Vec<CssSource>,
    /// DOM path fallback.
    #[serde(default)]
    pub fallback: Option<String>,
    /// Error message if element not found.
    #[serde(default)]
    pub error: Option<String>,
}

/// Raw element info from JS.
#[derive(Debug, Deserialize)]
pub struct RawElementInfo {
    /// Tag name.
    pub tag: String,
    /// Text content.
    pub text: String,
    /// CSS selector.
    pub selector: String,
}

/// Parse the raw JS locate result into a structured `LocateResult`.
///
/// Returns `Err` with a message if the element was not found.
pub fn parse_locate_result(raw: RawLocateResult) -> Result<LocateResult, String> {
    if let Some(err) = raw.error {
        return Err(err);
    }

    let element_info = raw.element.ok_or("No element info returned")?;
    let fallback = raw.fallback.unwrap_or_default();

    Ok(LocateResult {
        element: ElementInfo {
            tag: element_info.tag,
            text: element_info.text,
            selector: element_info.selector,
        },
        source: raw.source,
        css: raw.css,
        fallback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_locate_js_contains_selector() {
        let js = build_locate_js("button[type=submit]");
        assert!(
            js.contains("button[type=submit]"),
            "JS should contain the selector"
        );
    }

    #[test]
    fn build_locate_js_escapes_quotes() {
        let js = build_locate_js("button[aria-label='Sign In']");
        assert!(
            js.contains("\\'Sign In\\'"),
            "JS should escape single quotes in selector"
        );
    }

    #[test]
    fn build_locate_js_checks_react_fiber() {
        let js = build_locate_js("div");
        assert!(js.contains("__reactFiber$"), "should check React fiber");
        assert!(js.contains("_debugSource"), "should check _debugSource");
    }

    #[test]
    fn build_locate_js_checks_data_ds() {
        let js = build_locate_js("div");
        assert!(js.contains("data-ds"), "should check domscribe attribute");
    }

    #[test]
    fn build_locate_js_checks_data_lad() {
        let js = build_locate_js("div");
        assert!(
            js.contains("data-lad"),
            "should check lad attribute for source"
        );
    }

    #[test]
    fn parse_locate_result_success() {
        let raw = RawLocateResult {
            element: Some(RawElementInfo {
                tag: "button".into(),
                text: "Sign In".into(),
                selector: "button.btn-primary".into(),
            }),
            source: Some(SourceLocation {
                file: "src/LoginForm.tsx".into(),
                line: 42,
            }),
            css: vec![],
            fallback: Some("body > main > form > button.btn-primary".into()),
            error: None,
        };

        let result = parse_locate_result(raw).unwrap();
        assert_eq!(result.element.tag, "button");
        assert_eq!(result.element.text, "Sign In");
        assert_eq!(result.source.as_ref().unwrap().file, "src/LoginForm.tsx");
        assert_eq!(result.source.as_ref().unwrap().line, 42);
        assert!(result.fallback.contains("button.btn-primary"));
    }

    #[test]
    fn parse_locate_result_no_source() {
        let raw = RawLocateResult {
            element: Some(RawElementInfo {
                tag: "div".into(),
                text: "Hello".into(),
                selector: "div.greeting".into(),
            }),
            source: None,
            css: vec![],
            fallback: Some("body > div.greeting".into()),
            error: None,
        };

        let result = parse_locate_result(raw).unwrap();
        assert!(result.source.is_none(), "should have no source");
        assert_eq!(result.fallback, "body > div.greeting");
    }

    #[test]
    fn parse_locate_result_error() {
        let raw = RawLocateResult {
            element: None,
            source: None,
            css: vec![],
            fallback: None,
            error: Some("Element not found: .nonexistent".into()),
        };

        let err = parse_locate_result(raw).unwrap_err();
        assert!(err.contains("not found"), "should return error message");
    }

    #[test]
    fn locate_result_serialization() {
        let result = LocateResult {
            element: ElementInfo {
                tag: "button".into(),
                text: "Submit".into(),
                selector: "button#submit".into(),
            },
            source: Some(SourceLocation {
                file: "src/Form.tsx".into(),
                line: 10,
            }),
            css: vec![],
            fallback: "body > form > button#submit".into(),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("src/Form.tsx"));
        assert!(json.contains("\"line\":10"));
        // css should be omitted when empty
        assert!(!json.contains("\"css\""), "empty css should be skipped");
    }
}
