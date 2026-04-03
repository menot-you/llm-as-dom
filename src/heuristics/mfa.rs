//! MFA/2FA heuristic: detect verification code input pages.
//!
//! When a page requires a verification code (SMS, authenticator app, email),
//! the pilot cannot auto-generate the code and must escalate to the caller.

use crate::pilot::Action;
use crate::semantic::{ElementKind, SemanticView};

/// Text patterns that indicate an MFA/2FA verification page.
const MFA_INDICATORS: &[&str] = &[
    "verification code",
    "two-factor",
    "2fa",
    "authenticator",
    "one-time",
    "security code",
    "confirm your identity",
    "enter the code",
    "sent a code",
    "check your email",
    "check your phone",
    "sms code",
    "backup code",
];

/// Detect MFA/2FA pages and escalate (we cannot generate codes).
///
/// If a code input field is found, its element ID is included in the
/// escalation reason for the orchestrator to use.
pub fn try_detect_mfa(
    view: &SemanticView,
    _goal: &str,
    _acted_on: &[u32],
) -> Option<super::HeuristicResult> {
    let text_lower = view.visible_text.to_lowercase();

    let is_mfa = MFA_INDICATORS.iter().any(|ind| text_lower.contains(ind));
    if !is_mfa {
        return None;
    }

    // Look for a code input field (typically short text/number/tel input)
    let code_field = view.elements.iter().find(|e| {
        matches!(e.kind, ElementKind::Input)
            && !e.disabled
            && matches!(
                e.input_type.as_deref(),
                Some("text") | Some("number") | Some("tel") | None
            )
    });

    let reason = if let Some(field) = code_field {
        format!(
            "MFA/2FA page detected — code input field [{}] found. Cannot auto-generate verification code.",
            field.id
        )
    } else {
        "MFA/2FA page detected — cannot auto-generate verification code.".into()
    };

    Some(super::HeuristicResult {
        action: Some(Action::Escalate {
            reason: reason.clone(),
        }),
        confidence: 0.95,
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{Element, ElementKind, PageState, SemanticView};

    fn mfa_view(text: &str, elements: Vec<Element>) -> SemanticView {
        SemanticView {
            url: "https://example.com/verify".into(),
            title: "Verify".into(),
            page_hint: "content page".into(),
            elements,
            forms: vec![],
            visible_text: text.into(),
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        }
    }

    fn code_input(id: u32) -> Element {
        Element {
            id,
            kind: ElementKind::Input,
            label: "Code".into(),
            name: Some("code".into()),
            value: None,
            placeholder: Some("Enter code".into()),
            href: None,
            input_type: Some("text".into()),
            disabled: false,
            form_index: None,
            context: None,
            hint: None,
        }
    }

    #[test]
    fn mfa_detected_without_field() {
        let view = mfa_view("Enter the verification code sent to your phone", vec![]);
        let result = try_detect_mfa(&view, "login", &[]);
        assert!(result.is_some(), "should detect MFA page");
        let r = result.unwrap();
        assert!(r.confidence >= 0.9);
        assert!(matches!(r.action, Some(Action::Escalate { .. })));
    }

    #[test]
    fn mfa_with_code_field() {
        let view = mfa_view(
            "We sent a code to your email. Enter it below.",
            vec![code_input(5)],
        );
        let result = try_detect_mfa(&view, "login", &[]);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(
            r.reason.contains("[5]"),
            "reason should reference code input field ID"
        );
    }

    #[test]
    fn not_mfa_page() {
        let view = mfa_view("Welcome to your dashboard. Here are your stats.", vec![]);
        let result = try_detect_mfa(&view, "view dashboard", &[]);
        assert!(result.is_none(), "normal page should not trigger MFA");
    }
}
