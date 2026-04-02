//! SemanticView: compressed DOM representation for LLM consumption.

use serde::{Deserialize, Serialize};

/// Compressed view of a web page optimized for LLM reasoning.
/// Target: ~500-2000 tokens instead of 15KB raw DOM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticView {
    pub url: String,
    pub title: String,
    pub page_hint: String,
    pub elements: Vec<Element>,
    pub visible_text: String,
    pub state: PageState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Element {
    pub id: u32,
    pub kind: ElementKind,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_type: Option<String>,
    pub disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ElementKind {
    Button,
    Input,
    Link,
    Select,
    Textarea,
    Checkbox,
    Radio,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PageState {
    Ready,
    Loading,
    Error,
}

impl SemanticView {
    /// Format as compact text for LLM prompt (not JSON — saves tokens).
    pub fn to_prompt(&self) -> String {
        let mut out = String::with_capacity(512);
        out.push_str(&format!("URL: {}\n", self.url));
        out.push_str(&format!("TITLE: {}\n", self.title));
        out.push_str(&format!("HINT: {}\n", self.page_hint));
        out.push_str(&format!("STATE: {:?}\n\n", self.state));
        out.push_str("ELEMENTS:\n");
        for el in &self.elements {
            out.push_str(&format!("[{}] {:?}", el.id, el.kind));
            if let Some(itype) = &el.input_type {
                out.push_str(&format!(" type={itype}"));
            }
            out.push_str(&format!(" \"{}\"", el.label));
            if let Some(name) = &el.name {
                out.push_str(&format!(" name=\"{name}\""));
            }
            if let Some(ph) = &el.placeholder {
                out.push_str(&format!(" ph=\"{ph}\""));
            }
            if let Some(val) = &el.value {
                out.push_str(&format!(" val=\"{val}\""));
            }
            if let Some(href) = &el.href {
                out.push_str(&format!(" href=\"{href}\""));
            }
            if el.disabled {
                out.push_str(" [disabled]");
            }
            out.push('\n');
        }
        if !self.visible_text.is_empty() {
            out.push_str(&format!("\nVISIBLE TEXT: {}\n", self.visible_text));
        }
        out
    }

    /// Rough token estimate (1 token ≈ 4 chars).
    pub fn estimated_tokens(&self) -> usize {
        self.to_prompt().len() / 4
    }
}
