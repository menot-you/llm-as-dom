//! OAuth flow heuristics: detect OAuth buttons, handle consent pages.

use crate::pilot::Action;
use crate::semantic::{ElementKind, SemanticView};

/// OAuth button labels to detect "Login with <provider>" buttons.
const OAUTH_BUTTON_PATTERNS: &[(&str, &str)] = &[
    ("google", "Google"),
    ("github", "GitHub"),
    ("facebook", "Facebook"),
    ("microsoft", "Microsoft"),
    ("apple", "Apple"),
    ("twitter", "Twitter"),
    ("sign in with", ""),
    ("login with", ""),
    ("continue with", ""),
    ("log in with", ""),
    ("sso", "SSO"),
];

/// Try to detect and click an OAuth login button.
pub fn try_oauth_button(
    view: &SemanticView,
    goal: &str,
    acted_on: &[u32],
) -> Option<super::HeuristicResult> {
    let goal_lower = goal.to_lowercase();

    // Only activate if goal mentions login/auth
    let wants_login = goal_lower.contains("login") || goal_lower.contains("sign in");

    if !wants_login {
        return None;
    }

    // If there's a password field and the goal has credentials, prefer
    // regular login
    let has_password_field = view
        .elements
        .iter()
        .any(|e| e.input_type.as_deref() == Some("password"));

    if has_password_field && (goal_lower.contains("password") || goal_lower.contains(" pw ")) {
        return None;
    }

    // Find OAuth buttons
    for el in &view.elements {
        if acted_on.contains(&el.id) || el.disabled {
            continue;
        }

        let is_clickable = matches!(el.kind, ElementKind::Button | ElementKind::Link);
        if !is_clickable {
            continue;
        }

        let label_lower = el.label.to_lowercase();

        for (pattern, provider_name) in OAUTH_BUTTON_PATTERNS {
            if label_lower.contains(pattern) {
                let provider: &str = if provider_name.is_empty() {
                    &label_lower
                } else {
                    provider_name
                };
                return Some(super::HeuristicResult {
                    action: Some(Action::Click {
                        element: el.id,
                        reasoning: format!(
                            "heuristic: click OAuth button [{}] — '{}'",
                            el.id, el.label,
                        ),
                    }),
                    confidence: 0.85,
                    reason: format!("OAuth login button detected: {provider}"),
                });
            }
        }
    }

    None
}

/// Try to click "Allow" / "Authorize" on a consent page.
pub fn try_consent_approval(
    view: &SemanticView,
    _goal: &str,
    acted_on: &[u32],
) -> Option<super::HeuristicResult> {
    let text_lower = view.visible_text.to_lowercase();

    // Only activate on consent-like pages
    let is_consent = crate::oauth::CONSENT_KEYWORDS
        .iter()
        .any(|kw| text_lower.contains(kw));

    if !is_consent {
        return None;
    }

    let approval_keywords = [
        "allow",
        "authorize",
        "approve",
        "accept",
        "continue",
        "grant",
        "confirm",
        "agree",
        "yes",
    ];

    for el in &view.elements {
        if acted_on.contains(&el.id) || el.disabled {
            continue;
        }

        let is_button = matches!(el.kind, ElementKind::Button | ElementKind::Link);
        if !is_button {
            continue;
        }

        let label_lower = el.label.to_lowercase();

        if approval_keywords.iter().any(|kw| label_lower.contains(kw)) {
            return Some(super::HeuristicResult {
                action: Some(Action::Click {
                    element: el.id,
                    reasoning: format!(
                        "heuristic: approve OAuth consent [{}] — '{}'",
                        el.id, el.label,
                    ),
                }),
                confidence: 0.90,
                reason: "OAuth consent approval button detected".into(),
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{Element, ElementKind, PageState, SemanticView};

    fn mock_view(elements: Vec<Element>, visible_text: &str) -> SemanticView {
        SemanticView {
            url: "https://example.com/login".into(),
            title: "Login".into(),
            page_hint: "login page".into(),
            elements,
            forms: vec![],
            visible_text: visible_text.into(),
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        }
    }

    fn oauth_button(id: u32, label: &str) -> Element {
        Element {
            id,
            kind: ElementKind::Button,
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
            checked: None,
            options: None,
            frame_index: None,
            is_visible: None,
        }
    }

    #[test]
    fn detect_google_login_button() {
        let view = mock_view(vec![oauth_button(1, "Sign in with Google")], "");
        let result = try_oauth_button(&view, "login to my account", &[]);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.action.is_some());
        assert!(r.confidence >= 0.8);
    }

    #[test]
    fn prefer_password_login_when_credentials_given() {
        let elements = vec![
            oauth_button(1, "Login with Google"),
            Element {
                id: 2,
                kind: ElementKind::Input,
                label: "Password".into(),
                input_type: Some("password".into()),
                ..oauth_button(2, "")
            },
        ];
        let view = mock_view(elements, "");
        let result = try_oauth_button(&view, "login as user@test.com password secret123", &[]);
        assert!(result.is_none()); // Should prefer regular login
    }

    #[test]
    fn consent_approval_clicks_allow() {
        let view = mock_view(
            vec![oauth_button(1, "Allow"), oauth_button(2, "Deny")],
            "MyApp wants to access your Google account",
        );
        let result = try_consent_approval(&view, "login", &[]);
        assert!(result.is_some());
        if let Some(Action::Click { element, .. }) = result.unwrap().action {
            assert_eq!(element, 1); // Should click Allow, not Deny
        }
    }

    #[test]
    fn no_consent_on_regular_page() {
        let view = mock_view(vec![oauth_button(1, "Submit")], "Enter your details");
        let result = try_consent_approval(&view, "login", &[]);
        assert!(result.is_none());
    }
}
