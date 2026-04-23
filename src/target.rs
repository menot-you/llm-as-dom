//! Semantic element targeting.
//!
//! Lets LAD tools accept `TargetSpec { role, text, label, testid, within_text }`
//! in addition to the numeric element ID from `lad_snapshot`. Eliminates the
//! snapshot+click roundtrip for common flows like "click the Post button"
//! or "fill the Email input". The LLM caller no longer has to hold an
//! element ID in its context window or deal with stale IDs after rerenders.
//!
//! The matcher runs inside the page via `page.evaluate()` and returns the
//! first element that satisfies every provided constraint. Intersection
//! semantics: more fields = stricter match.

use rmcp::schemars;
use rmcp::schemars::JsonSchema;
use serde::Deserialize;

/// A semantic description of an element to interact with.
///
/// At least one field must be set for the spec to match anything. Fields
/// combine with AND semantics — an element must satisfy every provided
/// constraint. Omitted fields are treated as "don't care".
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct TargetSpec {
    /// ARIA role or implicit role (button, link, textbox, checkbox, etc).
    /// Covers HTML implicit roles: `button` matches `<button>` +
    /// `<input type=button|submit|reset>`, `link` matches `<a href>`,
    /// `textbox` matches `<input type=text|email|...>`, `<textarea>`,
    /// and `[contenteditable]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,

    /// Visible text or accessible name. Substring match against the
    /// element's `aria-label`, `innerText`, `value`, and `placeholder`.
    /// Case-insensitive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// Associated form `<label>` text. Resolves to the element whose
    /// `<label for="id">` or `<label>` wrapper matches this string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    /// `data-testid` attribute exact match. Useful for React/Vue apps
    /// that ship test IDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub testid: Option<String>,

    /// CSS selector fallback. When provided, narrows the match to
    /// elements matching this selector. Combine with other fields for
    /// stricter match. Used as a last-resort when role/text aren't enough.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,

    /// Scope the search to elements within an ancestor whose innerText
    /// contains this string. Example: `within_text="Settings"` finds
    /// a "Save" button inside the Settings section even if there are
    /// other "Save" buttons on the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within_text: Option<String>,

    /// Zero-based index to pick among multiple matches. Default is 0
    /// (first match). Use this when a spec is intentionally ambiguous
    /// and you want the Nth candidate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nth: Option<u32>,
}

impl TargetSpec {
    /// Return true if the spec has at least one targeting constraint.
    pub fn is_populated(&self) -> bool {
        self.role.is_some()
            || self.text.is_some()
            || self.label.is_some()
            || self.testid.is_some()
            || self.selector.is_some()
            || self.within_text.is_some()
    }

    /// Render this spec as a JSON object literal for embedding in the JS
    /// matcher body. Escapes every string field via `pilot::js_escape`.
    pub fn to_js_object(&self) -> String {
        let mut parts = Vec::new();
        if let Some(r) = &self.role {
            parts.push(format!("role: '{}'", crate::pilot::js_escape(r)));
        }
        if let Some(t) = &self.text {
            parts.push(format!("text: '{}'", crate::pilot::js_escape(t)));
        }
        if let Some(l) = &self.label {
            parts.push(format!("label: '{}'", crate::pilot::js_escape(l)));
        }
        if let Some(id) = &self.testid {
            parts.push(format!("testid: '{}'", crate::pilot::js_escape(id)));
        }
        if let Some(s) = &self.selector {
            parts.push(format!("selector: '{}'", crate::pilot::js_escape(s)));
        }
        if let Some(w) = &self.within_text {
            parts.push(format!("within: '{}'", crate::pilot::js_escape(w)));
        }
        if let Some(n) = self.nth {
            parts.push(format!("nth: {n}"));
        }
        format!("{{ {} }}", parts.join(", "))
    }
}

/// JS source for the semantic matcher. A single top-level function
/// `__ladFindTarget(spec, root)` that returns the matching element or
/// null. Walks the DOM inside `root` (default `document`) and filters
/// candidates against the spec with AND semantics.
///
/// Intentionally NOT wrapped in an IIFE — callers splice it into larger
/// scripts alongside the body that uses the result.
pub const MATCHER_JS: &str = r#"
function __ladNormText(s) {
    return (s || '').replace(/\s+/g, ' ').trim().toLowerCase();
}

function __ladImplicitRole(el) {
    const tag = el.tagName.toLowerCase();
    const type = (el.type || '').toLowerCase();
    if (tag === 'button') return 'button';
    if (tag === 'a' && el.hasAttribute('href')) return 'link';
    if (tag === 'input' && ['button','submit','reset','image'].includes(type)) return 'button';
    if (tag === 'input' && ['text','email','url','tel','password','search',''].includes(type)) return 'textbox';
    if (tag === 'textarea') return 'textbox';
    if (tag === 'input' && type === 'checkbox') return 'checkbox';
    if (tag === 'input' && type === 'radio') return 'radio';
    if (tag === 'select') return 'combobox';
    if (tag === 'img' && el.hasAttribute('alt')) return 'img';
    if (tag === 'nav') return 'navigation';
    if (['h1','h2','h3','h4','h5','h6'].includes(tag)) return 'heading';
    if (tag === 'ul' || tag === 'ol') return 'list';
    if (tag === 'li') return 'listitem';
    if (el.isContentEditable) return 'textbox';
    return null;
}

function __ladAccessibleName(el) {
    // Priority: aria-label > labelledby > label for > innerText > placeholder > alt > title > value
    const label = el.getAttribute('aria-label');
    if (label) return label;
    const lbid = el.getAttribute('aria-labelledby');
    if (lbid) {
        const other = document.getElementById(lbid);
        if (other) return other.innerText || other.textContent || '';
    }
    if (el.id) {
        const forLabel = document.querySelector('label[for="' + el.id + '"]');
        if (forLabel) return forLabel.innerText || '';
    }
    // Wrapping <label>
    let p = el.parentElement;
    while (p && p.tagName) {
        if (p.tagName.toLowerCase() === 'label') return p.innerText || '';
        p = p.parentElement;
    }
    return (el.innerText || '').trim()
        || el.getAttribute('placeholder')
        || el.getAttribute('alt')
        || el.getAttribute('title')
        || el.value
        || '';
}

function __ladIsVisible(el) {
    if (!el || !el.getClientRects) return false;
    const rect = el.getBoundingClientRect();
    if (rect.width === 0 && rect.height === 0) return false;
    const style = window.getComputedStyle(el);
    if (style.display === 'none' || style.visibility === 'hidden' || parseFloat(style.opacity) === 0) return false;
    return true;
}

function __ladFindTarget(spec, root) {
    root = root || document;
    // Narrow candidates via selector if provided, else all elements.
    let candidates = spec.selector
        ? Array.from(root.querySelectorAll(spec.selector))
        : Array.from(root.querySelectorAll('*'));

    // Filter out hidden elements early.
    candidates = candidates.filter(__ladIsVisible);

    // within_text: keep only elements whose ancestor innerText contains the string.
    if (spec.within) {
        const needle = __ladNormText(spec.within);
        candidates = candidates.filter(el => {
            let p = el.parentElement;
            while (p) {
                const t = __ladNormText(p.innerText);
                if (t.includes(needle)) return true;
                p = p.parentElement;
            }
            return false;
        });
    }

    // testid exact match
    if (spec.testid) {
        candidates = candidates.filter(el =>
            el.getAttribute('data-testid') === spec.testid
        );
    }

    // role match (ARIA explicit or HTML implicit)
    if (spec.role) {
        const wanted = spec.role.toLowerCase();
        candidates = candidates.filter(el => {
            const explicit = (el.getAttribute('role') || '').toLowerCase();
            if (explicit === wanted) return true;
            return __ladImplicitRole(el) === wanted;
        });
    }

    // label: match the element whose associated <label> contains the text
    if (spec.label) {
        const wanted = __ladNormText(spec.label);
        candidates = candidates.filter(el => {
            if (el.id) {
                const forLabel = document.querySelector('label[for="' + el.id + '"]');
                if (forLabel && __ladNormText(forLabel.innerText).includes(wanted)) return true;
            }
            let p = el.parentElement;
            while (p && p.tagName) {
                if (p.tagName.toLowerCase() === 'label' &&
                    __ladNormText(p.innerText).includes(wanted)) return true;
                p = p.parentElement;
            }
            // Also check placeholder as a weaker label source.
            const ph = (el.getAttribute('placeholder') || '').toLowerCase();
            if (ph && ph.includes(wanted)) return true;
            return false;
        });
    }

    // text: match accessible name / visible text / aria-label / value / placeholder
    if (spec.text) {
        const wanted = __ladNormText(spec.text);
        candidates = candidates.filter(el => {
            const name = __ladNormText(__ladAccessibleName(el));
            return name.includes(wanted);
        });
    }

    // If still multiple, prefer leaf elements (innerHTML has no child elements)
    // so we hit the button text rather than a wrapper div containing it.
    if (candidates.length > 1) {
        const leaves = candidates.filter(el => {
            return !Array.from(el.children).some(child => candidates.includes(child));
        });
        if (leaves.length > 0) candidates = leaves;
    }

    const idx = spec.nth || 0;
    return candidates[idx] || null;
}
"#;

/// Build a JS IIFE that resolves the given target, binds it to `el`, and
/// runs the caller-provided `body`. On failure returns a JSON error with
/// the first 5 candidates that partially matched (for agent recovery).
pub fn build_target_js(spec: &TargetSpec, body: &str) -> String {
    let spec_js = spec.to_js_object();
    format!(
        r#"(() => {{
            {MATCHER_JS}
            const spec = {spec_js};
            const el = __ladFindTarget(spec);
            if (!el) {{
                // Collect near-misses: elements matching at least one constraint.
                const near = [];
                const all = Array.from(document.querySelectorAll('*')).filter(__ladIsVisible);
                for (const cand of all) {{
                    const role = __ladImplicitRole(cand) || cand.getAttribute('role') || '';
                    const name = __ladAccessibleName(cand).slice(0, 60).trim();
                    if (spec.text && __ladNormText(name).includes(__ladNormText(spec.text))) {{
                        near.push({{ role, name, tag: cand.tagName.toLowerCase() }});
                    }} else if (spec.role && (role === spec.role.toLowerCase())) {{
                        near.push({{ role, name, tag: cand.tagName.toLowerCase() }});
                    }}
                    if (near.length >= 5) break;
                }}
                return JSON.stringify({{ error: "target not found", spec: spec, candidates: near }});
            }}
            {body}
            return JSON.stringify({{ ok: true }});
        }})()"#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_populated_false_when_empty() {
        let spec = TargetSpec::default();
        assert!(!spec.is_populated());
    }

    #[test]
    fn is_populated_true_when_role_set() {
        let spec = TargetSpec {
            role: Some("button".into()),
            ..Default::default()
        };
        assert!(spec.is_populated());
    }

    #[test]
    fn to_js_object_renders_all_fields() {
        let spec = TargetSpec {
            role: Some("button".into()),
            text: Some("Post".into()),
            label: Some("Email".into()),
            testid: Some("tweetButton".into()),
            selector: Some("[data-test]".into()),
            within_text: Some("Compose".into()),
            nth: Some(2),
        };
        let js = spec.to_js_object();
        assert!(js.contains("role: 'button'"));
        assert!(js.contains("text: 'Post'"));
        assert!(js.contains("label: 'Email'"));
        assert!(js.contains("testid: 'tweetButton'"));
        assert!(js.contains("selector: '[data-test]'"));
        assert!(js.contains("within: 'Compose'"));
        assert!(js.contains("nth: 2"));
    }

    #[test]
    fn to_js_object_escapes_quotes() {
        let spec = TargetSpec {
            text: Some("it's a button".into()),
            ..Default::default()
        };
        let js = spec.to_js_object();
        assert!(js.contains(r"it\'s a button"));
    }

    #[test]
    fn build_target_js_contains_matcher_and_spec() {
        let spec = TargetSpec {
            role: Some("button".into()),
            text: Some("Post".into()),
            ..Default::default()
        };
        let js = build_target_js(&spec, "el.click();");
        assert!(js.contains("__ladFindTarget"));
        assert!(js.contains("role: 'button'"));
        assert!(js.contains("el.click();"));
    }
}
