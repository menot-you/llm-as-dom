//! Rule-based action engine — resolves 70-90% of actions without LLM.
//! Falls back to LLM only when confidence is low.

use crate::pilot::Action;
use crate::semantic::{ElementKind, SemanticView};

/// Confidence threshold — below this, escalate to LLM.
const CONFIDENCE_THRESHOLD: f32 = 0.6;

/// Result of a heuristic evaluation attempt.
pub struct HeuristicResult {
    /// The resolved action, or `None` if no rule matched with enough confidence.
    pub action: Option<Action>,
    /// Confidence score (0.0 .. 1.0) of the match.
    pub confidence: f32,
    /// Human-readable explanation of why this rule matched (or didn't).
    pub reason: String,
}

/// Try to resolve the next action using rules only (no LLM).
/// Returns None if confidence is too low — caller should use LLM.
pub fn try_resolve(view: &SemanticView, goal: &str, acted_on: &[u32]) -> HeuristicResult {
    let goal_lower = goal.to_lowercase();

    // Strategy 1: Form fill by goal parsing
    if let Some(result) = try_form_fill(view, &goal_lower, acted_on)
        && result.confidence >= CONFIDENCE_THRESHOLD
    {
        return result;
    }

    // Strategy 2: Button click by goal keywords
    if let Some(result) = try_button_click(view, &goal_lower, acted_on)
        && result.confidence >= CONFIDENCE_THRESHOLD
    {
        return result;
    }

    // Strategy 3: Goal completion detection
    if let Some(result) = try_detect_done(view, &goal_lower)
        && result.confidence >= CONFIDENCE_THRESHOLD
    {
        return result;
    }

    HeuristicResult {
        action: None,
        confidence: 0.0,
        reason: "no heuristic matched".into(),
    }
}

/// Determine which form to target based on goal keywords.
///
/// For login goals, picks the first form containing a password field.
/// Returns `None` to allow all forms (no scoping).
fn target_form(view: &SemanticView, goal: &str) -> Option<u32> {
    let is_login = goal.contains("login") || goal.contains("sign in") || goal.contains("log in");
    if !is_login {
        return None;
    }
    // Find the first form that has a password input
    view.elements
        .iter()
        .find(|e| e.input_type.as_deref() == Some("password") && e.form_index.is_some())
        .and_then(|e| e.form_index)
}

/// Returns `true` if the element belongs to the target form (or no scoping is active).
fn in_target_form(el: &crate::semantic::Element, target: Option<u32>) -> bool {
    match target {
        None => true,
        Some(idx) => el.form_index == Some(idx),
    }
}

/// Parse goal for credentials and fill form fields.
fn try_form_fill(
    view: &SemanticView,
    goal: &str,
    acted_on: &[u32],
) -> Option<HeuristicResult> {
    let target = target_form(view, goal);

    // Extract credentials from goal text
    let username = extract_credential(goal, &["as ", "user ", "username ", "email ", "login "]);
    let password = extract_credential(goal, &["password ", "pass ", "pw "]);

    // Find unfilled input fields (scoped to target form)
    for el in &view.elements {
        if acted_on.contains(&el.id) || !in_target_form(el, target) {
            continue;
        }

        match el.kind {
            ElementKind::Input => {
                let is_password = el.input_type.as_deref() == Some("password");
                let is_email = el.input_type.as_deref() == Some("email")
                    || el
                        .name
                        .as_deref()
                        .map(|n| n.contains("email"))
                        .unwrap_or(false)
                    || el.label.to_lowercase().contains("email");
                let is_username = el
                    .name
                    .as_deref()
                    .map(|n| n.contains("user") || n.contains("login") || n.contains("acct"))
                    .unwrap_or(false)
                    || el.label.to_lowercase().contains("user")
                    || el.label.to_lowercase().contains("login");

                if is_password {
                    if let Some(ref pw) = password {
                        return Some(HeuristicResult {
                            action: Some(Action::Type {
                                element: el.id,
                                value: pw.clone(),
                                reasoning: format!("heuristic: fill password field [{}]", el.id),
                            }),
                            confidence: 0.95,
                            reason: "password field matched".into(),
                        });
                    }
                } else if is_email || is_username {
                    if let Some(ref user) = username {
                        return Some(HeuristicResult {
                            action: Some(Action::Type {
                                element: el.id,
                                value: user.clone(),
                                reasoning: format!(
                                    "heuristic: fill username/email field [{}]",
                                    el.id
                                ),
                            }),
                            confidence: 0.90,
                            reason: "username/email field matched".into(),
                        });
                    }
                } else if el.input_type.as_deref() == Some("text") && username.is_some() {
                    // Generic text input — might be username if it's the first unfilled one
                    if let Some(ref user) = username {
                        return Some(HeuristicResult {
                            action: Some(Action::Type {
                                element: el.id,
                                value: user.clone(),
                                reasoning: format!(
                                    "heuristic: fill first text input [{}] (likely username)",
                                    el.id
                                ),
                            }),
                            confidence: 0.70,
                            reason: "generic text field, guessing username".into(),
                        });
                    }
                }
            }
            _ => continue,
        }
    }

    None
}

/// Find a submit/login button to click.
fn try_button_click(
    view: &SemanticView,
    goal: &str,
    acted_on: &[u32],
) -> Option<HeuristicResult> {
    let target = target_form(view, goal);

    // Only click submit after at least one field is filled
    if acted_on.is_empty() {
        return None;
    }

    // Check if all input fields in the target form have been filled
    let unfilled_inputs = view
        .elements
        .iter()
        .filter(|e| {
            e.kind == ElementKind::Input
                && !acted_on.contains(&e.id)
                && in_target_form(e, target)
        })
        .count();

    if unfilled_inputs > 0 {
        return None; // Still have fields to fill
    }

    // Find the best submit button
    let submit_keywords = ["login", "sign in", "submit", "log in", "continue", "enter"];
    let goal_keywords: Vec<&str> = if goal.contains("login") || goal.contains("sign in") {
        vec!["login", "sign in", "log in"]
    } else if goal.contains("search") {
        vec!["search", "go", "find"]
    } else if goal.contains("submit") {
        vec!["submit", "send", "save"]
    } else {
        submit_keywords.to_vec()
    };

    let mut best_button: Option<(u32, f32)> = None;

    for el in &view.elements {
        if el.kind != ElementKind::Button
            || el.disabled
            || !in_target_form(el, target)
            || acted_on.contains(&el.id)
        {
            continue;
        }

        let label_lower = el.label.to_lowercase();
        let value_lower = el.value.as_deref().unwrap_or("").to_lowercase();
        let combined = format!("{} {}", label_lower, value_lower);

        // Score by keyword match
        let mut score: f32 = 0.0;
        for kw in &goal_keywords {
            if combined.contains(kw) {
                score = 0.90;
                break;
            }
        }

        // Fallback: type=submit is likely the main action
        if score < 0.5 && el.input_type.as_deref() == Some("submit") {
            score = 0.75;
        }

        if let Some((_, best_score)) = best_button {
            if score > best_score {
                best_button = Some((el.id, score));
            }
        } else if score > 0.5 {
            best_button = Some((el.id, score));
        }
    }

    best_button.map(|(id, conf)| HeuristicResult {
        action: Some(Action::Click {
            element: id,
            reasoning: format!("heuristic: click submit button [{}]", id),
        }),
        confidence: conf,
        reason: "submit button matched".into(),
    })
}

/// Detect if the goal has been achieved.
fn try_detect_done(view: &SemanticView, goal: &str) -> Option<HeuristicResult> {
    // If we were on a login page and now we're not, login probably succeeded
    if (goal.contains("login") || goal.contains("sign in"))
        && view.page_hint != "login page"
        && !view.url.to_lowercase().contains("login")
    {
        return Some(HeuristicResult {
            action: Some(Action::Done {
                result: serde_json::json!({
                    "success": true,
                    "url": view.url,
                    "title": view.title,
                }),
                reasoning: "heuristic: URL no longer contains login — navigation succeeded".into(),
            }),
            confidence: 0.85,
            reason: "left login page".into(),
        });
    }

    // Check for error messages in visible text
    let text_lower = view.visible_text.to_lowercase();
    if text_lower.contains("invalid")
        || text_lower.contains("incorrect")
        || text_lower.contains("wrong password")
        || text_lower.contains("failed")
    {
        return Some(HeuristicResult {
            action: Some(Action::Done {
                result: serde_json::json!({
                    "success": false,
                    "error": "login failed — error message detected",
                    "visible_text": &view.visible_text[..view.visible_text.len().min(200)],
                }),
                reasoning: "heuristic: error message detected in page text".into(),
            }),
            confidence: 0.80,
            reason: "error text detected".into(),
        });
    }

    None
}

/// Extract a value that follows a keyword in the goal string.
/// e.g. "login as testuser with password test123" → "testuser" for prefix "as "
fn extract_credential(goal: &str, prefixes: &[&str]) -> Option<String> {
    for prefix in prefixes {
        if let Some(pos) = goal.find(prefix) {
            let after = &goal[pos + prefix.len()..];
            let value = after.split_whitespace().next();
            if let Some(v) = value
                && !v.is_empty()
                && !["with", "and", "then", "password", "pass"].contains(&v)
            {
                return Some(v.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_username_from_goal() {
        let v = extract_credential("login as testuser with password secret", &["as "]);
        assert_eq!(v, Some("testuser".into()));
    }

    #[test]
    fn extract_password_from_goal() {
        let v = extract_credential("login as testuser with password secret123", &["password "]);
        assert_eq!(v, Some("secret123".into()));
    }

    #[test]
    fn extract_email_from_goal() {
        let v = extract_credential("login as test@example.com password x", &["as "]);
        assert_eq!(v, Some("test@example.com".into()));
    }

    #[test]
    fn no_credential_returns_none() {
        let v = extract_credential("navigate to homepage", &["as ", "user "]);
        assert_eq!(v, None);
    }
}
