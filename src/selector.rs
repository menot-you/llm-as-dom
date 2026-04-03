//! Semantic selector: find elements by description, not CSS.
//!
//! Matches elements using fuzzy label matching, kind filtering, and
//! contextual scoring. Designed to be more resilient than CSS selectors
//! because it works on the semantic meaning, not DOM structure.

use crate::semantic::{Element, ElementKind, SemanticView};

/// A semantic selector that describes what element to find.
#[derive(Debug, Clone)]
pub enum Selector {
    /// Natural language description: "the login button", "email input"
    Natural(String),
    /// Match by element kind + label pattern
    KindAndLabel {
        kind: Option<ElementKind>,
        label_contains: String,
    },
    /// Match by role attribute pattern (aria role)
    Role {
        role: String,
        label_contains: Option<String>,
    },
    /// Match by form context: "the submit button in form 0"
    InForm {
        form_index: u32,
        inner: Box<Selector>,
    },
    /// Match by numeric lad-id (backward compat)
    Id(u32),
    /// CSS-like attribute match: `[name="email"]`, `[type="password"]`
    Attribute { attr: String, value: String },
}

/// Result of a selector match with confidence score.
#[derive(Debug, Clone)]
pub struct SelectorMatch {
    /// The matched element's lad-id.
    pub element_id: u32,
    /// Confidence score (0.0 .. 1.0).
    pub confidence: f32,
    /// Why this element matched.
    pub reason: String,
}

impl Selector {
    /// Parse a selector from a string.
    ///
    /// Supports multiple formats:
    /// - `"#3"` or `"3"` → `Id(3)`
    /// - `"button:Submit"` → `KindAndLabel { kind: Button, label: "Submit" }`
    /// - `"[name=email]"` → `Attribute { attr: "name", value: "email" }`
    /// - `"the login button"` → `Natural("the login button")`
    /// - `"input in form 0"` → `InForm { form_index: 0, inner: Natural("input") }`
    pub fn parse(s: &str) -> Self {
        let s = s.trim();

        // Numeric ID
        if let Ok(id) = s.parse::<u32>() {
            return Self::Id(id);
        }
        if let Some(id_str) = s.strip_prefix('#')
            && let Ok(id) = id_str.parse::<u32>()
        {
            return Self::Id(id);
        }

        // CSS-like attribute: [name=email] or [type="password"]
        if let Some(inner) = s.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
            && let Some(eq_pos) = inner.find('=')
        {
            let attr = inner[..eq_pos].trim().to_string();
            let value = inner[eq_pos + 1..]
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            return Self::Attribute { attr, value };
        }

        // Kind:label format (e.g., "button:Login", "input:email")
        if let Some(colon_pos) = s.find(':') {
            let kind_str = &s[..colon_pos];
            let label = &s[colon_pos + 1..];
            if let Some(kind) = parse_kind(kind_str) {
                return Self::KindAndLabel {
                    kind: Some(kind),
                    label_contains: label.to_string(),
                };
            }
        }

        // "X in form N" pattern
        if let Some(form_match) = parse_form_context(s) {
            return form_match;
        }

        // Default: natural language
        Self::Natural(s.to_string())
    }
}

/// Parse an element kind from a string.
fn parse_kind(s: &str) -> Option<ElementKind> {
    match s.to_lowercase().as_str() {
        "button" | "btn" => Some(ElementKind::Button),
        "input" | "field" => Some(ElementKind::Input),
        "link" | "a" => Some(ElementKind::Link),
        "select" | "dropdown" => Some(ElementKind::Select),
        "textarea" | "text" => Some(ElementKind::Textarea),
        "checkbox" | "check" => Some(ElementKind::Checkbox),
        "radio" => Some(ElementKind::Radio),
        _ => None,
    }
}

/// Parse "X in form N" pattern.
fn parse_form_context(s: &str) -> Option<Selector> {
    let lower = s.to_lowercase();
    if let Some(pos) = lower.rfind(" in form ") {
        let inner = &s[..pos];
        let form_str = &s[pos + " in form ".len()..];
        if let Ok(form_idx) = form_str.trim().parse::<u32>() {
            return Some(Selector::InForm {
                form_index: form_idx,
                inner: Box::new(Selector::parse(inner)),
            });
        }
    }
    None
}

// ── Public API ───────────────────────────────────────────

/// Find all matching elements in a [`SemanticView`] for a selector.
///
/// Returns matches sorted by confidence (highest first).
/// Disabled elements are skipped.
pub fn find_matches(view: &SemanticView, selector: &Selector) -> Vec<SelectorMatch> {
    let mut matches: Vec<SelectorMatch> = view
        .elements
        .iter()
        .filter(|el| !el.disabled)
        .filter_map(|el| match_element(el, selector))
        .collect();

    matches.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    matches
}

/// Find the single best match, or `None`.
pub fn find_best(view: &SemanticView, selector: &Selector) -> Option<SelectorMatch> {
    find_matches(view, selector).into_iter().next()
}

// ── Match engine ─────────────────────────────────────────

/// Try to match a single element against a selector.
fn match_element(el: &Element, selector: &Selector) -> Option<SelectorMatch> {
    match selector {
        Selector::Id(id) => match_id(el, *id),
        Selector::Attribute { attr, value } => match_attribute(el, attr, value),
        Selector::KindAndLabel {
            kind,
            label_contains,
        } => match_kind_and_label(el, *kind, label_contains),
        Selector::Role {
            role,
            label_contains,
        } => match_role(el, role, label_contains.as_deref()),
        Selector::InForm { form_index, inner } => match_in_form(el, *form_index, inner),
        Selector::Natural(description) => match_natural(el, description),
    }
}

fn match_id(el: &Element, id: u32) -> Option<SelectorMatch> {
    (el.id == id).then(|| SelectorMatch {
        element_id: el.id,
        confidence: 1.0,
        reason: format!("exact ID match [{}]", id),
    })
}

fn match_attribute(el: &Element, attr: &str, value: &str) -> Option<SelectorMatch> {
    let matched = match attr {
        "name" => el.name.as_deref() == Some(value),
        "type" => el.input_type.as_deref() == Some(value),
        "href" => el.href.as_deref().is_some_and(|h| h.contains(value)),
        "placeholder" => el
            .placeholder
            .as_deref()
            .is_some_and(|p| p.to_lowercase().contains(&value.to_lowercase())),
        "value" => el.value.as_deref() == Some(value),
        _ => false,
    };

    matched.then(|| SelectorMatch {
        element_id: el.id,
        confidence: 0.95,
        reason: format!("[{}={}] matched", attr, value),
    })
}

fn match_kind_and_label(
    el: &Element,
    kind: Option<ElementKind>,
    label_contains: &str,
) -> Option<SelectorMatch> {
    if !kind.is_none_or(|k| el.kind == k) {
        return None;
    }

    let label_lower = el.label.to_lowercase();
    let target_lower = label_contains.to_lowercase();

    let (score, reason) = if label_lower == target_lower {
        (0.95, format!("exact label match: \"{}\"", el.label))
    } else if label_lower.contains(&target_lower) {
        (0.85, format!("label contains: \"{}\"", label_contains))
    } else {
        return None;
    };

    Some(SelectorMatch {
        element_id: el.id,
        confidence: score,
        reason,
    })
}

fn match_role(el: &Element, role: &str, label_contains: Option<&str>) -> Option<SelectorMatch> {
    let el_role = kind_to_aria_role(el.kind);

    if el_role != role.to_lowercase() {
        return None;
    }

    if let Some(label_pat) = label_contains {
        let label_lower = el.label.to_lowercase();
        if !label_lower.contains(&label_pat.to_lowercase()) {
            return None;
        }
    }

    Some(SelectorMatch {
        element_id: el.id,
        confidence: 0.90,
        reason: format!("role={} matched", role),
    })
}

fn kind_to_aria_role(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Button => "button",
        ElementKind::Link => "link",
        ElementKind::Checkbox => "checkbox",
        ElementKind::Radio => "radio",
        ElementKind::Input | ElementKind::Textarea => "textbox",
        ElementKind::Select => "combobox",
        ElementKind::Other => "generic",
    }
}

fn match_in_form(el: &Element, form_index: u32, inner: &Selector) -> Option<SelectorMatch> {
    if el.form_index != Some(form_index) {
        return None;
    }

    match_element(el, inner).map(|mut m| {
        m.confidence *= 0.95;
        m.reason = format!("{} (in form {})", m.reason, form_index);
        m
    })
}

// ── Natural language matching ────────────────────────────

/// Natural language matching — the killer feature.
///
/// Matches elements by parsing human descriptions like:
/// - "the login button" → Button with label containing "login"
/// - "email input" → Input with name/label/placeholder containing "email"
/// - "submit" → Button with label "submit" or `type="submit"`
/// - "the password field" → Input with `type="password"`
fn match_natural(el: &Element, description: &str) -> Option<SelectorMatch> {
    let desc_lower = description.to_lowercase();
    let label_lower = el.label.to_lowercase();
    let name_lower = el.name.as_deref().unwrap_or("").to_lowercase();
    let ph_lower = el.placeholder.as_deref().unwrap_or("").to_lowercase();
    let type_lower = el.input_type.as_deref().unwrap_or("").to_lowercase();

    let keywords = extract_keywords(&desc_lower);
    if keywords.is_empty() {
        return None;
    }

    let mut score: f32 = 0.0;
    let mut reasons: Vec<String> = Vec::new();

    // Kind hints in the description
    score += score_kind_hints(&keywords, el.kind, &mut reasons);

    // Special type matching
    score += score_type_hints(&desc_lower, &type_lower, &name_lower, &mut reasons);

    // Label / name / placeholder matching on content keywords
    let content_keywords: Vec<&str> = keywords
        .iter()
        .filter(|w| {
            !matches!(
                **w,
                "button" | "btn" | "input" | "field" | "link" | "submit"
            )
        })
        .copied()
        .collect();

    for kw in &content_keywords {
        if label_lower.contains(kw) {
            score += 0.3;
            reasons.push(format!("label contains \"{}\"", kw));
        } else if name_lower.contains(kw) {
            score += 0.2;
            reasons.push(format!("name contains \"{}\"", kw));
        } else if ph_lower.contains(kw) {
            score += 0.15;
            reasons.push(format!("placeholder contains \"{}\"", kw));
        }
    }

    // Submit button special case
    score += score_submit_hints(&desc_lower, el, &label_lower, &mut reasons);

    if score < 0.3 {
        return None;
    }

    Some(SelectorMatch {
        element_id: el.id,
        confidence: score.min(0.95),
        reason: reasons.join(", "),
    })
}

/// Extract meaningful keywords (drop stop words).
fn extract_keywords(desc: &str) -> Vec<&str> {
    const STOP_WORDS: &[&str] = &[
        "the", "a", "an", "this", "that", "in", "on", "of", "for", "to", "my", "field", "element",
    ];
    desc.split_whitespace()
        .filter(|w| !STOP_WORDS.contains(w))
        .collect()
}

fn score_kind_hints(keywords: &[&str], kind: ElementKind, reasons: &mut Vec<String>) -> f32 {
    let wants_button = keywords
        .iter()
        .any(|w| matches!(*w, "button" | "btn" | "submit" | "click"));
    let wants_input = keywords
        .iter()
        .any(|w| matches!(*w, "input" | "type" | "enter" | "fill"));
    let wants_link = keywords
        .iter()
        .any(|w| matches!(*w, "link" | "url" | "href" | "navigate" | "go"));

    if wants_button && kind == ElementKind::Button {
        reasons.push("kind=button matches description".into());
        0.3
    } else if wants_input && matches!(kind, ElementKind::Input | ElementKind::Textarea) {
        reasons.push("kind=input matches description".into());
        0.3
    } else if wants_link && kind == ElementKind::Link {
        reasons.push("kind=link matches description".into());
        0.3
    } else {
        0.0
    }
}

fn score_type_hints(
    desc: &str,
    type_lower: &str,
    name_lower: &str,
    reasons: &mut Vec<String>,
) -> f32 {
    let mut score = 0.0;

    if desc.contains("password") && type_lower == "password" {
        score += 0.5;
        reasons.push("type=password matches".into());
    }
    if desc.contains("email") && (type_lower == "email" || name_lower.contains("email")) {
        score += 0.5;
        reasons.push("email field matches".into());
    }
    if desc.contains("search")
        && (type_lower == "search" || name_lower.contains("search") || name_lower == "q")
    {
        score += 0.5;
        reasons.push("search field matches".into());
    }

    score
}

fn score_submit_hints(
    desc: &str,
    el: &Element,
    label_lower: &str,
    reasons: &mut Vec<String>,
) -> f32 {
    if !desc.contains("submit") {
        return 0.0;
    }

    let mut score = 0.0;

    if el.input_type.as_deref() == Some("submit") {
        score += 0.4;
        reasons.push("type=submit".into());
    }
    if label_lower.contains("submit")
        || label_lower.contains("send")
        || label_lower.contains("save")
    {
        score += 0.2;
    }

    score
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::*;

    fn make_element(id: u32, kind: ElementKind, label: &str) -> Element {
        Element {
            id,
            kind,
            label: label.into(),
            name: None,
            value: None,
            placeholder: None,
            href: None,
            input_type: None,
            disabled: false,
            form_index: None,
            context: None,
            hint: None,
        }
    }

    fn make_input(id: u32, label: &str, input_type: &str, name: Option<&str>) -> Element {
        Element {
            input_type: Some(input_type.into()),
            name: name.map(Into::into),
            ..make_element(id, ElementKind::Input, label)
        }
    }

    fn make_view(elements: Vec<Element>) -> SemanticView {
        SemanticView {
            url: "https://example.com".into(),
            title: "Test".into(),
            page_hint: "test page".into(),
            elements,
            forms: vec![],
            visible_text: String::new(),
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        }
    }

    // ── Selector::parse ──────────────────────────────────

    #[test]
    fn parse_numeric_id() {
        assert!(matches!(Selector::parse("3"), Selector::Id(3)));
    }

    #[test]
    fn parse_hash_id() {
        assert!(matches!(Selector::parse("#5"), Selector::Id(5)));
    }

    #[test]
    fn parse_attribute() {
        let Selector::Attribute { attr, value } = Selector::parse("[name=email]") else {
            panic!("expected Attribute");
        };
        assert_eq!(attr, "name");
        assert_eq!(value, "email");
    }

    #[test]
    fn parse_kind_label() {
        let Selector::KindAndLabel {
            kind,
            label_contains,
        } = Selector::parse("button:Login")
        else {
            panic!("expected KindAndLabel");
        };
        assert_eq!(kind, Some(ElementKind::Button));
        assert_eq!(label_contains, "Login");
    }

    #[test]
    fn parse_form_context() {
        let Selector::InForm { form_index, .. } = Selector::parse("submit button in form 0") else {
            panic!("expected InForm");
        };
        assert_eq!(form_index, 0);
    }

    #[test]
    fn parse_natural() {
        assert!(matches!(
            Selector::parse("the login button"),
            Selector::Natural(_)
        ));
    }

    // ── find_best ────────────────────────────────────────

    #[test]
    fn find_by_id() {
        let view = make_view(vec![
            make_element(0, ElementKind::Button, "Submit"),
            make_element(1, ElementKind::Link, "Home"),
        ]);
        let m = find_best(&view, &Selector::Id(1)).unwrap();
        assert_eq!(m.element_id, 1);
        assert_eq!(m.confidence, 1.0);
    }

    #[test]
    fn find_by_attribute() {
        let view = make_view(vec![
            make_input(0, "Email", "email", Some("email")),
            make_input(1, "Password", "password", Some("pw")),
        ]);
        let m = find_best(&view, &Selector::parse("[name=email]")).unwrap();
        assert_eq!(m.element_id, 0);
    }

    #[test]
    fn find_login_button_natural() {
        let view = make_view(vec![
            make_element(0, ElementKind::Link, "Home"),
            make_element(1, ElementKind::Button, "Login"),
            make_element(2, ElementKind::Button, "Sign Up"),
        ]);
        let m = find_best(&view, &Selector::parse("the login button")).unwrap();
        assert_eq!(m.element_id, 1);
    }

    #[test]
    fn find_password_field() {
        let view = make_view(vec![
            make_input(0, "Email", "email", Some("email")),
            make_input(1, "Password", "password", Some("password")),
        ]);
        let m = find_best(&view, &Selector::parse("password field")).unwrap();
        assert_eq!(m.element_id, 1);
    }

    #[test]
    fn find_email_input() {
        let view = make_view(vec![
            make_input(0, "Username", "text", Some("user")),
            make_input(1, "Email Address", "email", Some("email")),
        ]);
        let m = find_best(&view, &Selector::parse("email input")).unwrap();
        assert_eq!(m.element_id, 1);
    }

    #[test]
    fn find_submit_button() {
        let view = make_view(vec![
            make_element(0, ElementKind::Button, "Cancel"),
            Element {
                input_type: Some("submit".into()),
                ..make_element(1, ElementKind::Button, "Send")
            },
        ]);
        let m = find_best(&view, &Selector::parse("submit button")).unwrap();
        assert_eq!(m.element_id, 1);
    }

    #[test]
    fn disabled_elements_skipped() {
        let view = make_view(vec![Element {
            disabled: true,
            ..make_element(0, ElementKind::Button, "Login")
        }]);
        assert!(find_best(&view, &Selector::parse("login button")).is_none());
    }

    #[test]
    fn find_in_form() {
        let view = make_view(vec![
            Element {
                form_index: Some(0),
                ..make_element(0, ElementKind::Button, "Submit")
            },
            Element {
                form_index: Some(1),
                ..make_element(1, ElementKind::Button, "Submit")
            },
        ]);
        let m = find_best(&view, &Selector::parse("Submit in form 1")).unwrap();
        assert_eq!(m.element_id, 1);
    }

    #[test]
    fn no_match_returns_none() {
        let view = make_view(vec![make_element(0, ElementKind::Link, "Home")]);
        assert!(find_best(&view, &Selector::parse("delete button")).is_none());
    }
}
