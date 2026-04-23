//! `SemanticView`: compressed DOM representation for LLM consumption.

use std::fmt::Write;

use serde::{Deserialize, Serialize};

/// Compressed view of a web page optimized for LLM reasoning.
///
/// Target: ~500-2000 tokens instead of 15 KB raw DOM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticView {
    /// Current page URL.
    pub url: String,
    /// Document title.
    pub title: String,
    /// Heuristic page classification (e.g. "login page").
    pub page_hint: String,
    /// Interactive elements on the page.
    pub elements: Vec<Element>,
    /// Metadata for each `<form>` on the page (matches `Element::form_index`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forms: Vec<FormMeta>,
    /// Concatenated visible headings/paragraphs (max ~500 chars).
    pub visible_text: String,
    /// Issue #36 — Individual text blocks extracted from the DOM (headings,
    /// paragraphs, td/span/a/pre/code) before concatenation. Used by
    /// `tool_lad_extract` to score blocks against the `what` query and
    /// return top-K matches instead of a pre-concatenated 500-char banner.
    /// Each block is per-block length-capped in the JS walker; the list
    /// itself is capped at 200 entries to bound payload size. Empty by
    /// default (legacy callers, mock views) — scoring falls back to raw
    /// `visible_text` when blocks are absent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_blocks: Vec<String>,
    /// Current page lifecycle state.
    pub state: PageState,
    /// Element cap indicator: `"50/316"` means 50 kept out of 316 total.
    /// `None` when no filtering was applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_cap: Option<String>,
    /// Human-readable reason when the page is blocked (CAPTCHA, WAF, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    /// Session context for multi-page flows (set by pilot when session is active).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_context: Option<String>,
    /// BUG-4 + FR-1 (friction-log-2026-04-22): structural card groups
    /// detected in the DOM (HN story rows, Reddit feed tiles, GitHub
    /// repo lists). `None` by default so legacy JSON is byte-identical.
    /// Populated only when the tool call passes `include_cards=true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cards: Option<Vec<Card>>,
}

/// BUG-4 + FR-1: a structural card representing a repeated sibling in
/// the DOM. Surfaces inline metadata ("647 points by Kaibeezy 3 hours
/// ago") that otherwise lived only in free text while keeping
/// interactive `elements` unchanged so existing click flows work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    /// Stable string ID prefixed `c`, e.g. `c0`. String to avoid
    /// collision with integer `Element::id`.
    pub id: String,
    /// First heading OR first external link text inside the container.
    pub title: String,
    /// Key-value metadata extracted via regex over the sibling's text
    /// (points, comments, author, age). Keys lowercase.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<(String, String)>,
    /// IDs of interactive elements inside the card. Agents drive the
    /// card via existing `lad_click(element=N)` — no new endpoint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_element_ids: Vec<u32>,
}

/// A single interactive element extracted from the DOM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Element {
    /// Stable numeric ID (`data-lad-id`).
    pub id: u32,
    /// Semantic element kind.
    pub kind: ElementKind,
    /// Best-effort label (aria-label, text content, placeholder, etc.).
    pub label: String,
    /// HTML `name` attribute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Current input value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Placeholder text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// `href` attribute (links only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    /// `type` attribute for inputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_type: Option<String>,
    /// Whether the element is disabled.
    pub disabled: bool,
    /// Index of the `<form>` this element belongs to (`None` if outside any form).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub form_index: Option<u32>,
    /// Optional contextual hint added by heuristics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Semantic hint from `@lad/hints` (`data-lad="field:email"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<ElementHint>,
    /// Whether a checkbox/radio is checked (`None` for non-checkbox/radio elements).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    /// Visible option labels for `<select>` elements (top 10).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// Index of the iframe this element belongs to (`None` if in the main document).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_index: Option<u32>,
    /// Wave 1 — hidden-element filter: `Some(false)` when the element was
    /// flagged by the a11y extractor as hidden (display:none, opacity:0,
    /// aria-hidden="true", zero bounds, etc). `None`/`Some(true)` means the
    /// element is treated as visible. Filtered out by default in
    /// `tool_lad_extract` / `tool_lad_snapshot` to block a class of prompt
    /// injection where adversarial pages smuggle instructions into nodes
    /// the human never sees. Pass `include_hidden=true` to bypass.
    ///
    /// Missing on deserialize → `None` → treated as visible, so legacy
    /// fixtures and A/B golden files keep working.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_visible: Option<bool>,
}

/// Semantic hint from `@lad/hints` (`data-lad="field:email"`).
///
/// Provides explicit developer-authored annotations that bypass heuristic
/// guessing. When present, the 5-tier dispatcher uses these at Tier 1
/// with very high confidence (0.98).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ElementHint {
    /// Hint category: `"field"`, `"form"`, or `"action"`.
    pub hint_type: String,
    /// Hint value: `"email"`, `"login"`, `"submit"`, etc.
    pub value: String,
}

/// Metadata about a `<form>` element on the page.
///
/// Provides context for the `form_index` field on [`Element`] so callers
/// know *which* form an element belongs to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FormMeta {
    /// Sequential index matching `Element::form_index`.
    pub index: u32,
    /// `action` attribute (e.g. `"/api/login"`), or `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// HTTP method (`"GET"`, `"POST"`, etc.).
    pub method: String,
    /// `id` attribute, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `name` attribute, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Semantic classification of an interactive element.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ElementKind {
    /// `<button>`, `[role=button]`, or `<input type=submit>`.
    Button,
    /// `<input>` (text, email, number, etc.).
    Input,
    /// `<a>` or `[role=link]`.
    Link,
    /// `<select>`.
    Select,
    /// `<textarea>`.
    Textarea,
    /// Checkbox input or `[role=checkbox]`.
    Checkbox,
    /// Radio input or `[role=radio]`.
    Radio,
    /// Anything else with an interactive role.
    Other,
}

/// Page lifecycle state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PageState {
    /// Page is fully loaded and interactive.
    Ready,
    /// Page is still loading.
    Loading,
    /// An error prevented the page from loading.
    Error,
    /// Page is blocked by a bot-challenge / CAPTCHA / WAF.
    /// Contains the reason string describing what was detected.
    Blocked(String),
}

impl SemanticView {
    /// Format as compact text for an LLM prompt (not JSON -- saves tokens).
    ///
    /// FIX-R4-02: URL is redacted to strip OAuth codes, tokens, magic links.
    pub fn to_prompt(&self) -> String {
        let mut out = String::with_capacity(512);
        let _ = writeln!(
            out,
            "URL: {}",
            crate::sanitize::redact_url_secrets(&self.url)
        );
        let _ = writeln!(out, "TITLE: {}", self.title);
        let _ = writeln!(out, "HINT: {}", self.page_hint);
        let _ = writeln!(out, "STATE: {:?}", self.state);
        if let Some(cap) = &self.element_cap {
            let _ = writeln!(out, "ELEMENT_CAP: {cap}");
        }
        if let Some(reason) = &self.blocked_reason {
            let _ = writeln!(out, "BLOCKED: {reason}");
        }
        // DX-W3-5: Element count summary by kind.
        out.push('\n');
        {
            let total = self.elements.len();
            let mut buttons = 0u32;
            let mut inputs = 0u32;
            let mut links = 0u32;
            let mut selects = 0u32;
            let mut textareas = 0u32;
            let mut checkboxes = 0u32;
            let mut radios = 0u32;
            let mut other = 0u32;
            for el in &self.elements {
                match el.kind {
                    ElementKind::Button => buttons += 1,
                    ElementKind::Input => inputs += 1,
                    ElementKind::Link => links += 1,
                    ElementKind::Select => selects += 1,
                    ElementKind::Textarea => textareas += 1,
                    ElementKind::Checkbox => checkboxes += 1,
                    ElementKind::Radio => radios += 1,
                    ElementKind::Other => other += 1,
                }
            }
            let mut parts = Vec::new();
            if buttons > 0 {
                parts.push(format!(
                    "{buttons} button{}",
                    if buttons == 1 { "" } else { "s" }
                ));
            }
            if inputs > 0 {
                parts.push(format!(
                    "{inputs} input{}",
                    if inputs == 1 { "" } else { "s" }
                ));
            }
            if links > 0 {
                parts.push(format!("{links} link{}", if links == 1 { "" } else { "s" }));
            }
            if selects > 0 {
                parts.push(format!(
                    "{selects} select{}",
                    if selects == 1 { "" } else { "s" }
                ));
            }
            if textareas > 0 {
                parts.push(format!(
                    "{textareas} textarea{}",
                    if textareas == 1 { "" } else { "s" }
                ));
            }
            if checkboxes > 0 {
                parts.push(format!(
                    "{checkboxes} checkbox{}",
                    if checkboxes == 1 { "" } else { "es" }
                ));
            }
            if radios > 0 {
                parts.push(format!(
                    "{radios} radio{}",
                    if radios == 1 { "" } else { "s" }
                ));
            }
            if other > 0 {
                parts.push(format!("{other} other"));
            }
            if parts.is_empty() {
                let _ = writeln!(out, "ELEMENTS: 0");
            } else {
                let _ = writeln!(out, "ELEMENTS: {total} ({parts})", parts = parts.join(", "));
            }
        }
        for el in &self.elements {
            let _ = write!(out, "[{}] {:?}", el.id, el.kind);
            if let Some(itype) = &el.input_type {
                let _ = write!(out, " type={itype}");
            }
            let _ = write!(out, " \"{}\"", el.label);
            if let Some(name) = &el.name {
                let _ = write!(out, " name=\"{name}\"");
            }
            if let Some(ph) = &el.placeholder {
                let _ = write!(out, " ph=\"{ph}\"");
            }
            if let Some(val) = &el.value {
                let _ = write!(out, " val=\"{val}\"");
            }
            if let Some(href) = &el.href {
                let _ = write!(out, " href=\"{href}\"");
            }
            // DX-W2-4: Show form membership so agents can scope interactions.
            if let Some(fi) = el.form_index {
                let _ = write!(out, " form={fi}");
            }
            // DX-W2-2 / Wave 5b (Pain #13): Show checkbox/radio checked state.
            // Only emit the bare `checked` marker when Some(true) — unchecked
            // and "not a checkbox/radio" look identical in the prompt to keep
            // lines terse. Callers that need the tri-state can read the
            // JSON `checked` field directly.
            if el.checked == Some(true) {
                out.push_str(" checked");
            }
            // DX-W2-2: Show select options.
            if let Some(ref opts) = el.options {
                let _ = write!(out, " options={opts:?}");
            }
            if let Some(hint) = &el.hint {
                let _ = write!(out, " [hint:{}:{}]", hint.hint_type, hint.value);
            }
            if let Some(fi) = el.frame_index {
                let _ = write!(out, " [iframe:{fi}]");
            }
            if el.disabled {
                out.push_str(" [disabled]");
            }
            out.push('\n');
        }
        if !self.visible_text.is_empty() {
            let _ = write!(out, "\nVISIBLE TEXT: {}\n", self.visible_text);
        }
        if let Some(ref ctx) = self.session_context {
            out.push('\n');
            out.push_str(ctx);
        }
        out
    }

    /// Format with session context for multi-page awareness.
    pub fn to_prompt_with_session(&self, session: &crate::session::SessionState) -> String {
        let mut out = self.to_prompt();
        out.push('\n');
        out.push_str(&format_session_context(session));
        out
    }

    /// Wave 1 — paginated prompt rendering.
    ///
    /// Returns the same shape as [`to_prompt`] but only includes a slice of
    /// `elements` so large pages don't blow the caller's token budget.
    ///
    /// Semantics:
    /// - `page` is zero-based.
    /// - `size == 0` is clamped to 1 to avoid division-by-zero.
    /// - When `size >= elements.len()` (or elements are empty), returns the
    ///   full page with a `"Page 1/1"` header.
    /// - Otherwise computes `total_pages = ceil(len / size)`, clamps `page`
    ///   to `[0, total_pages-1]`, and emits only that slice.
    /// - A page index past the end yields a `"Page {n}/{m} (empty)"` header
    ///   and no elements.
    ///
    /// Forms, visible_text, and metadata are preserved verbatim so the
    /// caller always sees the page context even on pages that don't
    /// contain any elements.
    pub fn to_prompt_paginated(&self, page: u32, size: u32) -> String {
        let size = size.max(1) as usize;
        let total = self.elements.len();

        // Trivial path: everything fits in one page.
        if total == 0 || size >= total {
            let body = self.to_prompt();
            return format!("Page 1/1\n{body}");
        }

        // Compute total pages and clamp the request.
        let total_pages = total.div_ceil(size);
        let page_idx = (page as usize).min(total_pages.saturating_sub(1));
        let start = page_idx * size;
        let end = (start + size).min(total);

        // Build a cheap clone of `self` with just the slice of elements.
        // We only swap `elements` — forms, visible_text, metadata all
        // remain intact so the caller can still see page context.
        let slice = &self.elements[start..end];
        let sliced_view = SemanticView {
            url: self.url.clone(),
            title: self.title.clone(),
            page_hint: self.page_hint.clone(),
            elements: slice.to_vec(),
            forms: self.forms.clone(),
            visible_text: self.visible_text.clone(),
            text_blocks: self.text_blocks.clone(),
            state: self.state.clone(),
            element_cap: self.element_cap.clone(),
            blocked_reason: self.blocked_reason.clone(),
            session_context: self.session_context.clone(),
            cards: self.cards.clone(),
        };

        let header = if slice.is_empty() {
            format!(
                "Page {current}/{total_pages} (empty)\n",
                current = page_idx + 1,
            )
        } else {
            format!("Page {current}/{total_pages}\n", current = page_idx + 1)
        };

        let mut out = header;
        out.push_str(&sliced_view.to_prompt());
        out
    }

    /// Wave 1 — hidden-element filter.
    ///
    /// Drops elements flagged `is_visible: Some(false)` from the view. Used
    /// by `tool_lad_extract` / `tool_lad_snapshot` when the caller has NOT
    /// set `include_hidden=true`. Closes a class of prompt-injection vector
    /// (Brave disclosure, Oct 2025) where pages hide adversarial instructions
    /// in DOM nodes the user never sees but the LLM still reads.
    ///
    /// Elements with `is_visible == None` are treated as visible so legacy
    /// fixtures and constructs without the field keep working.
    pub fn retain_visible_elements(&mut self) {
        self.elements.retain(|e| e.is_visible.unwrap_or(true));
    }

    /// Rough token estimate (1 token ~ 4 chars).
    ///
    /// SS-5: Approximates byte count without generating the full prompt string.
    /// Avoids the O(elements * label_len) allocation of `to_prompt()`.
    pub fn estimated_tokens(&self) -> usize {
        let header = self.url.len() + self.title.len() + self.page_hint.len() + 50;
        let elements: usize = self
            .elements
            .iter()
            .map(|e| {
                e.label.len()
                    + e.name.as_ref().map_or(0, |n| n.len())
                    + e.value.as_ref().map_or(0, |v| v.len())
                    + e.placeholder.as_ref().map_or(0, |p| p.len())
                    + e.href.as_ref().map_or(0, |h| h.len())
                    + e.input_type.as_ref().map_or(0, |t| t.len())
                    + 30 // per-element overhead (id, kind, formatting)
            })
            .sum();
        let text = self.visible_text.len();
        (header + elements + text) / 4
    }
}

/// Build a compact session context string for LLM prompt injection.
///
/// Includes recent navigation history (last 3 pages) and auth state.
pub fn format_session_context(session: &crate::session::SessionState) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    if !session.navigation_history.is_empty() {
        out.push_str("SESSION CONTEXT:\n");
        for entry in session.navigation_history.iter().rev().take(3) {
            // FIX-R4-02: Redact secrets from URLs in session context.
            let safe_url = crate::sanitize::redact_url_secrets(&entry.url);
            let _ = writeln!(out, "  - visited: {} ({})", safe_url, entry.title);
            for action in &entry.actions_taken {
                let _ = writeln!(out, "    action: {}", action);
            }
        }
    }

    match session.auth_state {
        crate::session::AuthState::InProgress => out.push_str("AUTH: in progress\n"),
        crate::session::AuthState::Authenticated => {
            out.push_str("AUTH: authenticated\n");
        }
        crate::session::AuthState::Failed => out.push_str("AUTH: failed\n"),
        _ => {}
    }

    if session.has_auth_cookies() {
        out.push_str("AUTH COOKIES: present\n");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_element(id: u32, label: &str, is_visible: Option<bool>) -> Element {
        Element {
            id,
            kind: ElementKind::Button,
            label: label.to_string(),
            name: None,
            value: None,
            placeholder: None,
            href: None,
            input_type: None,
            disabled: false,
            form_index: None,
            context: None,
            hint: None,
            checked: None,
            options: None,
            frame_index: None,
            is_visible,
        }
    }

    fn make_view(n: usize) -> SemanticView {
        SemanticView {
            url: "https://example.com".to_string(),
            title: "Test".to_string(),
            page_hint: "test".to_string(),
            elements: (0..n)
                .map(|i| make_element(i as u32, &format!("btn-{i}"), None))
                .collect(),
            forms: vec![],
            visible_text: "Hello".to_string(),
            text_blocks: vec![],
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
            cards: None,
        }
    }

    // ── Wave 1: pagination ──

    #[test]
    fn paginate_page_0_returns_first_slice() {
        let view = make_view(120);
        let out = view.to_prompt_paginated(0, 50);
        assert!(out.starts_with("Page 1/3\n"), "header missing: {out}");
        assert!(out.contains("[0]"), "id 0 should be present");
        assert!(out.contains("[49]"), "id 49 should be present");
        assert!(!out.contains("[50]"), "id 50 must not leak into page 0");
    }

    #[test]
    fn paginate_page_1_returns_middle_slice() {
        let view = make_view(120);
        let out = view.to_prompt_paginated(1, 50);
        assert!(out.starts_with("Page 2/3\n"), "header missing: {out}");
        assert!(out.contains("[50]"));
        assert!(out.contains("[99]"));
        assert!(!out.contains("[49]"));
        assert!(!out.contains("[100]"));
    }

    #[test]
    fn paginate_page_2_returns_tail_slice() {
        let view = make_view(120);
        let out = view.to_prompt_paginated(2, 50);
        assert!(out.starts_with("Page 3/3\n"), "header missing: {out}");
        assert!(out.contains("[100]"));
        assert!(out.contains("[119]"));
        assert!(!out.contains("[99]"));
    }

    #[test]
    fn paginate_page_3_is_empty_clamped() {
        // 120 elements, 50 per page → total_pages = 3 → page 3 is past end.
        let view = make_view(120);
        let out = view.to_prompt_paginated(3, 50);
        // We clamp to the last valid page, so header is "Page 3/3".
        // No new elements should appear beyond id 119.
        assert!(out.contains("Page 3/3"));
        assert!(!out.contains("[120]"));
    }

    #[test]
    fn paginate_page_size_greater_than_total_returns_single_page() {
        let view = make_view(120);
        let out = view.to_prompt_paginated(0, 200);
        assert!(out.starts_with("Page 1/1\n"), "header missing: {out}");
        assert!(out.contains("[0]"));
        assert!(out.contains("[119]"));
    }

    #[test]
    fn paginate_empty_view_returns_single_page_header() {
        let view = make_view(0);
        let out = view.to_prompt_paginated(0, 50);
        assert!(out.starts_with("Page 1/1\n"));
    }

    // ── Wave 1: hidden-element filter ──

    #[test]
    fn retain_visible_drops_hidden_keeps_none_and_true() {
        let mut view = SemanticView {
            url: "https://example.com".to_string(),
            title: "Mixed".to_string(),
            page_hint: "test".to_string(),
            elements: vec![
                make_element(1, "visible-none", None),
                make_element(2, "visible-true", Some(true)),
                make_element(3, "hidden", Some(false)),
                make_element(4, "also-hidden", Some(false)),
                make_element(5, "visible-again", None),
            ],
            forms: vec![],
            visible_text: String::new(),
            text_blocks: vec![],
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
            cards: None,
        };
        view.retain_visible_elements();
        assert_eq!(view.elements.len(), 3, "only visible elements survive");
        assert!(view.elements.iter().all(|e| e.is_visible != Some(false)));
        let labels: Vec<_> = view.elements.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["visible-none", "visible-true", "visible-again"],
        );
    }

    #[test]
    fn retain_visible_noop_when_all_visible() {
        let mut view = make_view(5);
        view.retain_visible_elements();
        assert_eq!(view.elements.len(), 5);
    }
}
