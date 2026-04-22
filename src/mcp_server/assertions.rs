//! Assertion engine for `lad_assert` and `lad_wait` tools.

use llm_as_dom::semantic;

/// Evaluate a single assertion against a semantic view.
///
/// Supported patterns (after normalization):
/// - `has login form` / `has login`
/// - `has password`
/// - `title contains <text>` — match against `<title>` only
/// - `url contains <text>` — match against URL only
/// - `text contains <text>` / `page contains <text>` — UNION match: true if
///   the substring appears in URL, `<title>`, visible body text, OR the
///   pre-rendered prompt text. Use this when you don't care where on the
///   page the text shows up (BUG-3: this is what `lad_wait` documented as
///   an example but did not actually implement until now).
/// - `has button <label>` (also matches `has <label> button`)
/// - `has link <label>` (also matches `has <label> link`)
/// - `has input <name>` (also matches `has <name> input`, plus input_type)
/// - `has form` — any form on page
/// - `has image` / `has img` — any img element in visible text
/// - `page has section <title>` — section title in visible text
/// - Fallback: all words present in combined page text.
pub(crate) fn check_assertion(
    assertion: &str,
    view: &semantic::SemanticView,
    prompt_text: &str,
) -> bool {
    let full_text = format!(
        "{} {} {} {}",
        view.url, view.title, view.visible_text, prompt_text
    )
    .to_lowercase();

    // Normalize word order: "has X input" -> "has input X", etc.
    let normalized = normalize_assertion(assertion);
    let assertion = &normalized;

    if assertion.contains("has login form") || assertion.contains("has login") {
        return view.page_hint == "login page";
    }
    if assertion.contains("has password") {
        return view
            .elements
            .iter()
            .any(|e| e.input_type.as_deref() == Some("password"));
    }
    if let Some(rest) = assertion.strip_prefix("title contains ") {
        return view
            .title
            .to_lowercase()
            .contains(rest.trim().trim_matches('"'));
    }
    if let Some(rest) = assertion.strip_prefix("url contains ") {
        return view
            .url
            .to_lowercase()
            .contains(rest.trim().trim_matches('"'));
    }
    // BUG-3 (friction-log-2026-04-22): `text contains X` and its alias
    // `page contains X` were documented as `lad_wait` examples but fell
    // through to the whole-phrase fallback, which required "text" and
    // "contains" to be literal words on the page. Match against the
    // full union text now so the documented behavior matches reality.
    if let Some(rest) = assertion
        .strip_prefix("text contains ")
        .or_else(|| assertion.strip_prefix("page contains "))
    {
        let needle = rest.trim().trim_matches('"');
        if needle.is_empty() {
            return false;
        }
        return full_text.contains(needle);
    }
    if let Some(rest) = assertion.strip_prefix("has button ") {
        let label = rest.trim().trim_matches('"').to_lowercase();
        return view.elements.iter().any(|e| {
            e.kind == semantic::ElementKind::Button && fuzzy_label_match(&e.label, &label)
                || e.value
                    .as_deref()
                    .is_some_and(|v| fuzzy_label_match(v, &label))
        });
    }
    if let Some(rest) = assertion.strip_prefix("has link ") {
        let label = rest.trim().trim_matches('"').to_lowercase();
        return view.elements.iter().any(|e| {
            e.kind == semantic::ElementKind::Link
                && (fuzzy_label_match(&e.label, &label)
                    || e.href
                        .as_deref()
                        .is_some_and(|h| h.to_lowercase().contains(&label)))
        });
    }
    if let Some(rest) = assertion.strip_prefix("has input ") {
        let name = rest.trim().trim_matches('"').to_lowercase();
        return view.elements.iter().any(|e| {
            e.kind == semantic::ElementKind::Input
                && (e
                    .name
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().contains(&name))
                    || e.label.to_lowercase().contains(&name)
                    || e.input_type
                        .as_deref()
                        .is_some_and(|t| t.to_lowercase() == name)
                    || e.placeholder
                        .as_deref()
                        .is_some_and(|p| p.to_lowercase().contains(&name)))
        });
    }
    if assertion == "has form" {
        return !view.forms.is_empty() || view.elements.iter().any(|e| e.form_index.is_some());
    }
    if assertion == "has image" || assertion == "has img" {
        return full_text.contains("img") || full_text.contains("image");
    }
    if let Some(rest) = assertion.strip_prefix("page has section ") {
        let section = rest.trim().trim_matches('"').to_lowercase();
        return view.visible_text.to_lowercase().contains(&section);
    }

    // Fallback: all words present in page
    let words: Vec<&str> = assertion.split_whitespace().collect();
    words.iter().all(|w| full_text.contains(w))
}

/// Normalize assertion word order so callers can write either
/// `"has email input"` or `"has input email"` and get the same result.
pub(crate) fn normalize_assertion(assertion: &str) -> String {
    let a = assertion.trim().to_lowercase();
    let words: Vec<&str> = a.split_whitespace().collect();

    // Pattern: "has <qualifier> input|button|link" -> "has input|button|link <qualifier>"
    if words.len() >= 3 && words[0] == "has" {
        let kind_keywords = ["input", "button", "link"];
        if let Some(&last) = words.last()
            && kind_keywords.contains(&last)
            && !kind_keywords.contains(&words[1])
        {
            let qualifier: Vec<&str> = words[1..words.len() - 1].to_vec();
            return format!("has {} {}", last, qualifier.join(" "));
        }
    }
    a
}

/// Fuzzy label match: checks `contains` after stripping non-alphanumeric
/// trailing characters (arrows, icons, extra whitespace).
pub(crate) fn fuzzy_label_match(element_label: &str, target: &str) -> bool {
    let el = element_label.to_lowercase();
    let tgt = target.to_lowercase();
    if el.contains(&tgt) {
        return true;
    }
    // Strip non-alphanumeric chars and retry
    let clean_el: String = el
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .collect();
    let clean_tgt: String = tgt
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .collect();
    clean_el.contains(&clean_tgt)
}
