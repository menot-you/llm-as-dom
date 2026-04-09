//! CSS cloaking heuristics + SPA shell detection.
//!
//! DX-CL2 (bug 2): The old cloaking detector flagged any page with zero
//! interactive elements and non-empty visible text as "possible CSS
//! cloaking". Twitter/X, Next.js apps, and many React SPAs render a
//! shell-only HTML shell during hydration — the interactive count is
//! legitimately zero for a few hundred milliseconds. Raising the text
//! threshold and adding an explicit SPA-shell signal eliminates the
//! false positive without weakening detection for true adversarial
//! cloaking pages.
//!
//! Split from `a11y.rs` to keep that file from growing further.

use crate::engine::PageHandle;

/// Minimum visible text length (chars) before a zero-element page is
/// classified as CSS cloaking. Below this threshold we treat it as an
/// empty / placeholder page and refuse to block. 500 is the same limit
/// used by the extractor when it caps visible_text.
pub const CLOAKING_TEXT_THRESHOLD: usize = 500;

/// Markers polled from the live DOM to tell us whether a page is a
/// mid-hydration SPA shell as opposed to a real cloaking page.
///
/// Any single marker being true is enough to flip the decision — a Next.js
/// hydration script, a `#__next`/`#root`/`#react-root` container, or a
/// `document.readyState != "complete"` are all strong signals that more
/// interactive content is coming.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ShellMarkers {
    /// `document.readyState === "complete"`.
    pub ready_complete: bool,
    /// `window.__NEXT_DATA__` is defined (Next.js).
    pub has_next_data: bool,
    /// Document has `#__next`, `#root`, `#react-root`, or `#app`.
    pub has_framework_root: bool,
    /// Number of `<script>` tags currently in the DOM. SPA shells have many.
    pub script_tag_count: u32,
}

impl ShellMarkers {
    /// True when the markers describe a likely SPA shell that is still
    /// hydrating. Used to decide whether to retry extraction and whether
    /// to suppress CSS-cloaking detection.
    ///
    /// Semantics:
    /// 1. Next.js / React / Vue root present → always a SPA shell.
    /// 2. `ready_complete == false` **combined with** any other positive
    ///    shell signal (framework root, `__NEXT_DATA__`, many scripts)
    ///    → SPA shell mid-hydration.
    /// 3. All-false markers (the unit-test `Default`) → not a shell. This
    ///    matters for pure-logic tests that construct a `SemanticView`
    ///    without running the JS probe.
    pub fn looks_like_spa_shell(&self) -> bool {
        // Strong positive markers: SPA framework root or Next.js data.
        if self.has_next_data || self.has_framework_root {
            return true;
        }
        // Weak signal: the document is still loading AND it at least has
        // a script tag to hydrate from.
        if !self.ready_complete && self.script_tag_count > 0 {
            return true;
        }
        false
    }
}

/// Decide whether the current `(interactive_count, visible_text, markers)`
/// tuple should be classified as CSS cloaking.
///
/// Returns `true` only when:
/// 1. `interactive_count == 0`, AND
/// 2. `visible_text.trim().len() > CLOAKING_TEXT_THRESHOLD`, AND
/// 3. the page does NOT look like a SPA shell mid-hydration.
///
/// This replaces the old, trigger-happy check that blocked on any
/// zero-element + non-empty-text combination.
pub fn is_css_cloaking(
    interactive_count: usize,
    visible_text: &str,
    markers: &ShellMarkers,
) -> bool {
    if interactive_count > 0 {
        return false;
    }
    if visible_text.trim().chars().count() <= CLOAKING_TEXT_THRESHOLD {
        return false;
    }
    if markers.looks_like_spa_shell() {
        return false;
    }
    true
}

// ── Browser-side probes ──────────────────────────────────────────────

/// JS snippet that returns a `ShellMarkers`-compatible JSON object.
///
/// Must be self-contained: the caller passes the whole script to
/// `page.eval_js`. Returns an object so [`probe_shell_markers`] can
/// deserialize it directly.
const SHELL_MARKERS_JS: &str = r#"
    (() => {
        try {
            const ready = document.readyState === 'complete';
            const hasNext = typeof window.__NEXT_DATA__ !== 'undefined';
            const hasRoot = !!(
                document.getElementById('__next')
                || document.getElementById('root')
                || document.getElementById('react-root')
                || document.getElementById('app')
            );
            const scriptCount = document.querySelectorAll('script').length;
            return JSON.stringify({
                ready_complete: ready,
                has_next_data: hasNext,
                has_framework_root: hasRoot,
                script_tag_count: scriptCount
            });
        } catch (e) {
            return JSON.stringify({
                ready_complete: false,
                has_next_data: false,
                has_framework_root: false,
                script_tag_count: 0
            });
        }
    })()
"#;

/// Probe the live DOM for SPA shell markers.
///
/// Failure is non-fatal — returns `Default::default()` markers on any
/// error so the caller falls back to the stricter cloaking heuristic.
pub async fn probe_shell_markers(page: &dyn PageHandle) -> ShellMarkers {
    match crate::engine::eval_js_into::<ShellMarkers>(page, SHELL_MARKERS_JS).await {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!(error = %e, "shell markers probe failed — using defaults");
            ShellMarkers::default()
        }
    }
}

// Private deserialize impl lives next to ShellMarkers — keep boilerplate
// out of the public API.
impl<'de> serde::Deserialize<'de> for ShellMarkers {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Raw {
            #[serde(default)]
            ready_complete: bool,
            #[serde(default)]
            has_next_data: bool,
            #[serde(default)]
            has_framework_root: bool,
            #[serde(default)]
            script_tag_count: u32,
        }
        let raw = Raw::deserialize(de)?;
        Ok(Self {
            ready_complete: raw.ready_complete,
            has_next_data: raw.has_next_data,
            has_framework_root: raw.has_framework_root,
            script_tag_count: raw.script_tag_count,
        })
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn lorem_ipsum(len: usize) -> String {
        "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(len / 50 + 1)[..len]
            .to_string()
    }

    #[test]
    fn empty_page_never_cloaked() {
        // No elements, no text.
        assert!(!is_css_cloaking(0, "", &ShellMarkers::default()));
    }

    #[test]
    fn short_text_below_threshold_not_cloaked() {
        // Some visible text but under the 500-char threshold — treat as
        // an empty / placeholder page, not cloaking.
        let text = "Welcome to the site";
        assert!(!is_css_cloaking(0, text, &ShellMarkers::default()));
    }

    #[test]
    fn spa_shell_with_next_data_not_cloaked() {
        // Next.js shell: lots of text, zero elements, __NEXT_DATA__ present.
        let text = lorem_ipsum(1000);
        let markers = ShellMarkers {
            ready_complete: true,
            has_next_data: true,
            has_framework_root: true,
            script_tag_count: 12,
        };
        assert!(
            !is_css_cloaking(0, &text, &markers),
            "SPA shell with __NEXT_DATA__ must not be flagged as cloaking"
        );
    }

    #[test]
    fn spa_shell_with_react_root_not_cloaked() {
        // Plain React: #root + lots of text.
        let text = lorem_ipsum(2000);
        let markers = ShellMarkers {
            ready_complete: true,
            has_next_data: false,
            has_framework_root: true,
            script_tag_count: 5,
        };
        assert!(!is_css_cloaking(0, &text, &markers));
    }

    #[test]
    fn still_loading_not_cloaked() {
        // readyState != "complete" → wait, don't block.
        let text = lorem_ipsum(1000);
        let markers = ShellMarkers {
            ready_complete: false,
            has_next_data: false,
            has_framework_root: false,
            script_tag_count: 1,
        };
        assert!(!is_css_cloaking(0, &text, &markers));
    }

    #[test]
    fn real_cloaking_still_detected() {
        // Static page, complete, no SPA markers, zero elements, lots of text.
        let text = lorem_ipsum(800);
        let markers = ShellMarkers {
            ready_complete: true,
            has_next_data: false,
            has_framework_root: false,
            script_tag_count: 0,
        };
        assert!(
            is_css_cloaking(0, &text, &markers),
            "true static cloaking (no SPA signal, complete readyState) must still block"
        );
    }

    #[test]
    fn interactive_elements_short_circuit() {
        // Any interactive element → never cloaking.
        let text = lorem_ipsum(800);
        let markers = ShellMarkers {
            ready_complete: true,
            has_next_data: false,
            has_framework_root: false,
            script_tag_count: 0,
        };
        assert!(!is_css_cloaking(1, &text, &markers));
    }

    #[test]
    fn shell_markers_serde_roundtrip() {
        // The eval_js_into path parses the markers from a JSON string.
        let json = r#"{
            "ready_complete": true,
            "has_next_data": true,
            "has_framework_root": false,
            "script_tag_count": 7
        }"#;
        let m: ShellMarkers = serde_json::from_str(json).unwrap();
        assert!(m.ready_complete);
        assert!(m.has_next_data);
        assert!(!m.has_framework_root);
        assert_eq!(m.script_tag_count, 7);
        assert!(m.looks_like_spa_shell());
    }

    #[test]
    fn shell_markers_missing_fields_default_false() {
        let m: ShellMarkers = serde_json::from_str("{}").unwrap();
        assert_eq!(m, ShellMarkers::default());
        assert!(!m.looks_like_spa_shell());
    }
}
