//! Validation heuristic: detect inline errors, field-level validation messages.
//!
//! When validation errors are present on a page, the form needs correction
//! before the pilot can proceed. This heuristic detects those patterns and
//! escalates so the upstream orchestrator can react.

use crate::pilot::Action;
use crate::semantic::SemanticView;

/// Validation error patterns commonly found in form visible text.
const VALIDATION_PATTERNS: &[&str] = &[
    "is required",
    "cannot be empty",
    "must be at least",
    "invalid format",
    "please enter",
    "this field",
    "is not valid",
    "doesn't match",
    "too short",
    "too long",
    "already exists",
    "already taken",
    "must contain",
    "please provide",
];

/// Check if the page has validation errors in its visible text.
pub fn has_validation_errors(view: &SemanticView) -> bool {
    let text_lower = view.visible_text.to_lowercase();
    VALIDATION_PATTERNS.iter().any(|p| text_lower.contains(p))
}

/// If validation errors are present, escalate so upstream can correct fields.
pub fn try_detect_validation(
    view: &SemanticView,
    _goal: &str,
    _acted_on: &[u32],
) -> Option<super::HeuristicResult> {
    if has_validation_errors(view) {
        return Some(super::HeuristicResult {
            action: Some(Action::Escalate {
                reason: "validation errors detected on page — form fields may need correction"
                    .into(),
            }),
            confidence: 0.80,
            reason: "inline validation errors detected".into(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{PageState, SemanticView};

    fn view_with_text(text: &str) -> SemanticView {
        SemanticView {
            url: "https://example.com/form".into(),
            title: "Form".into(),
            page_hint: "form page".into(),
            elements: vec![],
            forms: vec![],
            visible_text: text.into(),
            text_blocks: vec![],
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
            cards: None,
        }
    }

    #[test]
    fn validation_error_detected() {
        let view = view_with_text("Email is required. Please enter a valid address.");
        let result = try_detect_validation(&view, "fill form", &[]);
        assert!(result.is_some(), "should detect validation error");
        let r = result.unwrap();
        assert!(r.confidence >= 0.7);
        assert!(matches!(r.action, Some(Action::Escalate { .. })));
    }

    #[test]
    fn no_error_returns_none() {
        let view = view_with_text("Welcome to our registration form. Fill in your details.");
        let result = try_detect_validation(&view, "fill form", &[]);
        assert!(result.is_none(), "clean page should not trigger validation");
    }

    #[test]
    fn multiple_errors_still_detected() {
        let view = view_with_text(
            "Username is required. Password must be at least 8 characters. Email is not valid.",
        );
        assert!(
            has_validation_errors(&view),
            "should detect multiple validation errors"
        );
        let result = try_detect_validation(&view, "register", &[]);
        assert!(result.is_some());
    }
}
