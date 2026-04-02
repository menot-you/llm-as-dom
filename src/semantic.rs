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
    /// Concatenated visible headings/paragraphs (max ~500 chars).
    pub visible_text: String,
    /// Current page lifecycle state.
    pub state: PageState,
    /// Element cap indicator: `"50/316"` means 50 kept out of 316 total.
    /// `None` when no filtering was applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_cap: Option<String>,
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PageState {
    /// Page is fully loaded and interactive.
    Ready,
    /// Page is still loading.
    Loading,
    /// An error prevented the page from loading.
    Error,
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
            if el.disabled {
                out.push_str(" [disabled]");
            }
            out.push('\n');
        }
        if !self.visible_text.is_empty() {
            let _ = write!(out, "\nVISIBLE TEXT: {}\n", self.visible_text);
        }
        out
    }

    /// Rough token estimate (1 token ~ 4 chars).
    pub fn estimated_tokens(&self) -> usize {
        self.to_prompt().len() / 4
    }
}
