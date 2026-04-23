//! Login-specific heuristics: credential parsing, form fill, submit click, done detection.

use crate::pilot::Action;
use crate::semantic::{Element, ElementKind, SemanticView};

/// Determine which form to target based on goal keywords.
///
/// For login goals, picks the first form containing a credential field.
/// Returns `None` to allow all forms (no scoping).
pub fn target_form(view: &SemanticView, goal: &str) -> Option<u32> {
    let is_login = goal.contains("login") || goal.contains("sign in") || goal.contains("log in");
    if !is_login {
        return None;
    }
    view.elements
        .iter()
        .find(|e| is_secret_field(e) && e.form_index.is_some())
        .and_then(|e| e.form_index)
}

/// Returns `true` if the element belongs to the target form (or no scoping is active).
pub fn in_target_form(el: &Element, target: Option<u32>) -> bool {
    match target {
        None => true,
        Some(idx) => el.form_index == Some(idx),
    }
}

/// Check if an element is a secret/credential input field.
fn is_secret_field(el: &Element) -> bool {
    el.input_type.as_deref() == Some("password")
}

/// Parse goal for credentials and fill form fields.
pub fn try_form_fill(
    view: &SemanticView,
    goal: &str,
    acted_on: &[u32],
) -> Option<super::HeuristicResult> {
    let target = target_form(view, goal);

    let username = extract_credential(goal, &["as ", "user ", "username ", "email ", "login "]);
    let secret = extract_credential(goal, &["password ", "pass ", "pw "]);

    for el in &view.elements {
        if acted_on.contains(&el.id) || !in_target_form(el, target) {
            continue;
        }

        if el.kind != ElementKind::Input {
            continue;
        }

        let is_pw = is_secret_field(el);
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

        if is_pw {
            if let Some(ref pw) = secret {
                return Some(super::HeuristicResult {
                    action: Some(Action::Type {
                        element: el.id,
                        value: pw.clone(),
                        reasoning: format!("heuristic: fill credential field [{}]", el.id),
                    }),
                    confidence: 0.95,
                    reason: "credential field matched".into(),
                });
            }
        } else if is_email || is_username {
            if let Some(ref user) = username {
                return Some(super::HeuristicResult {
                    action: Some(Action::Type {
                        element: el.id,
                        value: user.clone(),
                        reasoning: format!("heuristic: fill username/email field [{}]", el.id),
                    }),
                    confidence: 0.90,
                    reason: "username/email field matched".into(),
                });
            }
        } else if el.input_type.as_deref() == Some("text")
            && let Some(ref user) = username
        {
            return Some(super::HeuristicResult {
                action: Some(Action::Type {
                    element: el.id,
                    value: user.clone(),
                    reasoning: format!(
                        "heuristic: fill first text input [{}] (likely username)",
                        el.id
                    ),
                }),
                confidence: 0.45, // below threshold — defer to LLM for non-login goals
                reason: "generic text field, guessing username".into(),
            });
        }
    }

    None
}

/// Find a submit/login button to click.
pub fn try_button_click(
    view: &SemanticView,
    goal: &str,
    acted_on: &[u32],
) -> Option<super::HeuristicResult> {
    let target = target_form(view, goal);

    if acted_on.is_empty() {
        return None;
    }

    let unfilled_inputs = view
        .elements
        .iter()
        .filter(|e| {
            e.kind == ElementKind::Input && !acted_on.contains(&e.id) && in_target_form(e, target)
        })
        .count();

    if unfilled_inputs > 0 {
        return None;
    }

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

        let mut score: f32 = 0.0;
        for kw in &goal_keywords {
            if combined.contains(kw) {
                score = 0.90;
                break;
            }
        }

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

    best_button.map(|(id, conf)| super::HeuristicResult {
        action: Some(Action::Click {
            element: id,
            reasoning: format!("heuristic: click submit button [{}]", id),
        }),
        confidence: conf,
        reason: "submit button matched".into(),
    })
}

/// Keywords in visible text that indicate a successful action.
const SUCCESS_KEYWORDS: &[&str] = &[
    "success",
    "successful",
    "welcome",
    "redirecting",
    "logged in",
    "signed in",
];

/// Keywords in visible text that indicate a failed action.
const ERROR_KEYWORDS: &[&str] = &[
    "invalid",
    "incorrect",
    "wrong password",
    "failed",
    "error",
    "bad login",
    "denied",
    "unauthorized",
    "try again",
];

/// Detect if the goal has been achieved.
///
/// Checks three signals in priority order:
/// 1. Visible text error indicators → Done(success=false)
/// 2. Visible text success indicators → Done(success=true)
/// 3. URL navigation away from login page → Done(success=true)
pub fn try_detect_done(view: &SemanticView, goal: &str) -> Option<super::HeuristicResult> {
    let text_lower = view.visible_text.to_lowercase();

    // Check error indicators first (higher priority — a page can show
    // "Login failed" while still on the same URL).
    if ERROR_KEYWORDS.iter().any(|kw| text_lower.contains(kw)) {
        return Some(super::HeuristicResult {
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

    // Check success indicators in visible text (catches in-page messages
    // like "Login successful! Redirecting..." without requiring a URL change).
    if SUCCESS_KEYWORDS.iter().any(|kw| text_lower.contains(kw)) {
        return Some(super::HeuristicResult {
            action: Some(Action::Done {
                result: serde_json::json!({
                    "success": true,
                    // FIX-5: Redact URL secrets from login result.
                    "url": crate::sanitize::redact_url_secrets(&view.url),
                    "title": view.title,
                    "signal": "success text in page",
                }),
                reasoning: "heuristic: success message detected in page text".into(),
            }),
            confidence: 0.90,
            reason: "success text detected".into(),
        });
    }

    // FIX-R4-04: URL-based detection: navigated away from login page.
    // Tightened to require:
    // 1. URL changed away from login
    // 2. Page has at least SOME interactive elements (not empty/error page)
    // 3. Minimum confidence reduced to 0.70 so LLM can override if needed
    if (goal.contains("login") || goal.contains("sign in"))
        && view.page_hint != "login page"
        && !view.url.to_lowercase().contains("login")
        && !view.elements.is_empty()
        && !view.visible_text.trim().is_empty()
    {
        return Some(super::HeuristicResult {
            action: Some(Action::Done {
                result: serde_json::json!({
                    "success": true,
                    "url": crate::sanitize::redact_url_secrets(&view.url),
                    "title": view.title,
                }),
                reasoning: "heuristic: URL no longer contains login and page has content — navigation succeeded".into(),
            }),
            confidence: 0.70,
            reason: "left login page (page has content)".into(),
        });
    }

    None
}

/// Stop words that indicate a credential value should not be extracted
/// (the word is a connector, not the actual value).
const CREDENTIAL_STOP_WORDS: &[&str] = &[
    "with", "and", "then", "password", "pass", "in", "the", "to", "a", "for", "on", "my", "this",
];

/// Extract a value that follows a keyword in the goal string.
///
/// e.g. `"login as testuser with pw test123"` returns `"testuser"` for prefix `"as "`.
///
/// FIX-R4-05: Supports quoted multi-word values:
///   - `password "my complex pass"` -> `"my complex pass"`
///   - `password 'my complex pass'` -> `"my complex pass"`
///   - Unquoted: takes everything until the next recognized keyword or end of string.
pub fn extract_credential(goal: &str, prefixes: &[&str]) -> Option<String> {
    for prefix in prefixes {
        if let Some(pos) = goal.find(prefix) {
            let after = &goal[pos + prefix.len()..];
            let trimmed = after.trim_start();
            if trimmed.is_empty() {
                continue;
            }

            // FIX-R4-05: Check for quoted values first.
            if trimmed.starts_with('"') || trimmed.starts_with('\'') {
                let quote = trimmed.chars().next().unwrap();
                let inner = &trimmed[1..];
                if let Some(end) = inner.find(quote) {
                    let val = &inner[..end];
                    if !val.is_empty() {
                        return Some(val.to_string());
                    }
                }
                // No closing quote -- fall through to unquoted parsing.
            }

            // Unquoted: take the first whitespace-delimited token.
            let value = trimmed.split_whitespace().next();
            if let Some(v) = value
                && !v.is_empty()
                && !CREDENTIAL_STOP_WORDS.contains(&v)
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
    use crate::semantic::{PageState, SemanticView};

    /// Build a minimal `SemanticView` for done-detection tests.
    fn view_with_text(url: &str, title: &str, page_hint: &str, visible_text: &str) -> SemanticView {
        SemanticView {
            url: url.into(),
            title: title.into(),
            page_hint: page_hint.into(),
            elements: vec![],
            forms: vec![],
            visible_text: visible_text.into(),
            text_blocks: vec![],
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
            cards: None,
        }
    }

    #[test]
    fn extract_username_from_goal() {
        let v = extract_credential("login as testuser with pw secret", &["as "]);
        assert_eq!(v, Some("testuser".into()));
    }

    #[test]
    fn extract_secret_from_goal() {
        let v = extract_credential("login as testuser with pw secret123", &["pw "]);
        assert_eq!(v, Some("secret123".into()));
    }

    #[test]
    fn extract_email_from_goal() {
        let v = extract_credential("login as test@example.com pw x", &["as "]);
        assert_eq!(v, Some("test@example.com".into()));
    }

    #[test]
    fn no_credential_returns_none() {
        let v = extract_credential("navigate to homepage", &["as ", "user "]);
        assert_eq!(v, None);
    }

    // ── Fix 1: done-detection via visible text ──────────────────────

    #[test]
    fn detect_done_success_text_on_login_page() {
        // Page still shows login URL but text says "Login successful! Redirecting..."
        let view = view_with_text(
            "https://example.com/login",
            "Login",
            "login page",
            "Login successful! Redirecting...",
        );
        let result = try_detect_done(&view, "login as user pw pass").unwrap();
        assert!(result.action.is_some());
        if let Some(Action::Done { result: val, .. }) = &result.action {
            assert_eq!(val["success"], true);
            assert_eq!(val["signal"], "success text in page");
        } else {
            panic!("expected Done action");
        }
        assert!(result.confidence >= 0.85);
    }

    #[test]
    fn detect_done_welcome_text() {
        let view = view_with_text(
            "https://example.com/dashboard",
            "Dashboard",
            "content page",
            "Welcome back, user!",
        );
        let result = try_detect_done(&view, "login as user pw pass").unwrap();
        assert!(result.action.is_some());
        if let Some(Action::Done { result: val, .. }) = &result.action {
            assert_eq!(val["success"], true);
        } else {
            panic!("expected Done action");
        }
    }

    #[test]
    fn detect_done_error_text_takes_priority() {
        // Page shows both "Login" URL and "Invalid credentials" text.
        // Error should be detected, not success.
        let view = view_with_text(
            "https://example.com/login",
            "Login",
            "login page",
            "Invalid username or password",
        );
        let result = try_detect_done(&view, "login as user pw pass").unwrap();
        if let Some(Action::Done { result: val, .. }) = &result.action {
            assert_eq!(val["success"], false);
        } else {
            panic!("expected Done with success=false");
        }
    }

    #[test]
    fn detect_done_wrong_password_error() {
        let view = view_with_text(
            "https://example.com/login",
            "Login",
            "login page",
            "Wrong password. Please try again.",
        );
        let result = try_detect_done(&view, "login as user pw pass").unwrap();
        if let Some(Action::Done { result: val, .. }) = &result.action {
            assert_eq!(val["success"], false);
        } else {
            panic!("expected Done with success=false");
        }
    }

    #[test]
    fn detect_done_no_signal_returns_none() {
        // Still on login page, no success or error text.
        let view = view_with_text(
            "https://example.com/login",
            "Login",
            "login page",
            "Enter your credentials",
        );
        let result = try_detect_done(&view, "login as user pw pass");
        assert!(result.is_none());
    }

    #[test]
    fn detect_done_signed_in_text() {
        let view = view_with_text(
            "https://example.com/login",
            "Login",
            "login page",
            "You are now signed in",
        );
        let result = try_detect_done(&view, "login as user pw pass").unwrap();
        if let Some(Action::Done { result: val, .. }) = &result.action {
            assert_eq!(val["success"], true);
        } else {
            panic!("expected Done action");
        }
    }

    #[test]
    fn detect_done_url_change_still_works() {
        // Classic case: URL changed away from login.
        // FIX-R4-04: Now requires non-empty elements and visible text.
        let mut view = view_with_text(
            "https://example.com/dashboard",
            "Dashboard",
            "content page",
            "Your projects",
        );
        // Add at least one element so the page is not considered empty.
        view.elements.push(crate::semantic::Element {
            id: 1,
            kind: crate::semantic::ElementKind::Button,
            label: "Create project".into(),
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
            is_visible: None,
        });
        let result = try_detect_done(&view, "login as user pw pass").unwrap();
        if let Some(Action::Done { result: val, .. }) = &result.action {
            assert_eq!(val["success"], true);
        } else {
            panic!("expected Done action");
        }
    }

    #[test]
    fn detect_done_empty_page_does_not_pass() {
        // FIX-R4-04: An empty page after URL change should NOT be treated as success.
        let view = view_with_text("https://example.com/error", "", "content page", "");
        let result = try_detect_done(&view, "login as user pw pass");
        assert!(
            result.is_none(),
            "empty page should not be detected as login success"
        );
    }

    // ── W4b: extract_credential stop list ────────────────────────────

    #[test]
    fn extract_credential_ignores_prepositions() {
        // "enter email in waitlist" — "in" should not be extracted
        let v = extract_credential("enter email in waitlist", &["email "]);
        assert_eq!(v, None);
    }

    #[test]
    fn extract_credential_ignores_articles() {
        let v = extract_credential("type user the field", &["user "]);
        assert_eq!(v, None);
    }

    #[test]
    fn extract_credential_still_works_for_real_values() {
        let v = extract_credential("login as alice@test.com pw secret", &["as "]);
        assert_eq!(v, Some("alice@test.com".into()));
    }

    // -- FIX-R4-05: multi-word password support --

    #[test]
    fn extract_credential_double_quoted() {
        let v = extract_credential(r#"password "my complex pass""#, &["password "]);
        assert_eq!(v, Some("my complex pass".into()));
    }

    #[test]
    fn extract_credential_single_quoted() {
        let v = extract_credential("password 'multi word'", &["password "]);
        assert_eq!(v, Some("multi word".into()));
    }

    #[test]
    fn extract_credential_unquoted_still_works() {
        let v = extract_credential("password simple", &["password "]);
        assert_eq!(v, Some("simple".into()));
    }

    // ── W4c: generic text field confidence ───────────────────────────

    #[test]
    fn generic_text_field_confidence_below_threshold() {
        let view = {
            let mut v = view_with_text("https://example.com", "Home", "content page", "");
            v.elements.push(crate::semantic::Element {
                id: 1,
                kind: crate::semantic::ElementKind::Input,
                label: "Search".into(),
                name: Some("q".into()),
                value: None,
                placeholder: None,
                href: None,
                input_type: Some("text".into()),
                disabled: false,
                form_index: None,
                context: None,
                hint: None,
                checked: None,
                options: None,
                frame_index: None,
                is_visible: None,
            });
            v
        };
        let result = try_form_fill(&view, "enter testuser in search", &[]);
        // Should either be None (deferred) or have confidence < 0.60
        if let Some(r) = result {
            assert!(
                r.confidence < 0.60,
                "generic text field confidence should be below threshold, got {}",
                r.confidence
            );
        }
        // None is also acceptable — means it deferred
    }
}
