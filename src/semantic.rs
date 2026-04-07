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
    /// Index of the iframe this element belongs to (`None` if in the main document).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_index: Option<u32>,
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
    pub fn to_prompt(&self) -> String {
        let mut out = String::with_capacity(512);
        let _ = writeln!(out, "URL: {}", self.url);
        let _ = writeln!(out, "TITLE: {}", self.title);
        let _ = writeln!(out, "HINT: {}", self.page_hint);
        let _ = writeln!(out, "STATE: {:?}", self.state);
        if let Some(cap) = &self.element_cap {
            let _ = writeln!(out, "ELEMENT_CAP: {cap}");
        }
        if let Some(reason) = &self.blocked_reason {
            let _ = writeln!(out, "BLOCKED: {reason}");
        }
        out.push('\n');
        out.push_str("ELEMENTS:\n");
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

    /// Rough token estimate (1 token ~ 4 chars).
    pub fn estimated_tokens(&self) -> usize {
        self.to_prompt().len() / 4
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
            let _ = writeln!(out, "  - visited: {} ({})", entry.url, entry.title);
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
