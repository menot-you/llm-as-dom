//! Multi-step form heuristic: detect and navigate wizard-style forms.
//!
//! Handles wizard-style forms with next/back/continue buttons.
//! Only advances when all visible required fields have been acted on.

use crate::pilot::Action;
use crate::semantic::{ElementKind, SemanticView};

/// Keywords that indicate a "next step" button in a wizard form.
const STEP_KEYWORDS: &[&str] = &[
    "next",
    "continue",
    "proceed",
    "next step",
    "step ",
    "forward",
    "go to step",
    "advance",
];

/// Detect wizard-style "Next" / "Continue" / "Step N" buttons.
///
/// Only suggests advancing when all visible input fields have been acted on
/// (i.e., no unfilled required fields remain).
pub fn try_next_step(
    view: &SemanticView,
    _goal: &str,
    acted_on: &[u32],
) -> Option<super::HeuristicResult> {
    // Count unfilled input fields that haven't been acted on
    let unfilled_inputs = view
        .elements
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                ElementKind::Input | ElementKind::Textarea | ElementKind::Select
            ) && !acted_on.contains(&e.id)
                && !e.disabled
                && e.value.as_deref().is_none_or(|v| v.is_empty())
        })
        .count();

    // If there are still unfilled inputs, don't advance yet
    if unfilled_inputs > 0 {
        return None;
    }

    for el in &view.elements {
        if acted_on.contains(&el.id) || el.disabled {
            continue;
        }
        if !matches!(el.kind, ElementKind::Button | ElementKind::Link) {
            continue;
        }

        let label_lower = el.label.to_lowercase();
        if STEP_KEYWORDS.iter().any(|kw| label_lower.contains(kw)) {
            return Some(super::HeuristicResult {
                action: Some(Action::Click {
                    element: el.id,
                    reasoning: format!(
                        "heuristic: advance multi-step form [{}] — '{}'",
                        el.id, el.label
                    ),
                }),
                confidence: 0.85,
                reason: format!("multi-step form: detected '{}' button", el.label),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{Element, ElementKind, PageState, SemanticView};

    fn mock_view(elements: Vec<Element>) -> SemanticView {
        SemanticView {
            url: "https://example.com/wizard".into(),
            title: "Wizard Form".into(),
            page_hint: "form page".into(),
            elements,
            forms: vec![],
            visible_text: String::new(),
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        }
    }

    fn input_el(id: u32, label: &str) -> Element {
        Element {
            id,
            kind: ElementKind::Input,
            label: label.into(),
            name: None,
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
        }
    }

    fn button_el(id: u32, label: &str) -> Element {
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
    fn next_step_detected_when_fields_filled() {
        let view = mock_view(vec![
            input_el(0, "First Name"),
            input_el(1, "Last Name"),
            button_el(2, "Next Step"),
        ]);

        // All inputs acted on
        let result = try_next_step(&view, "fill wizard form", &[0, 1]);
        assert!(result.is_some(), "should detect Next Step button");
        let r = result.unwrap();
        assert!(r.confidence >= 0.8);
        match r.action.unwrap() {
            Action::Click { element, .. } => assert_eq!(element, 2),
            other => panic!("expected Click, got {other:?}"),
        }
    }

    #[test]
    fn no_advance_with_unfilled_fields() {
        let view = mock_view(vec![
            input_el(0, "First Name"),
            input_el(1, "Last Name"),
            button_el(2, "Continue"),
        ]);

        // Only first input acted on — second still unfilled
        let result = try_next_step(&view, "fill wizard form", &[0]);
        assert!(result.is_none(), "should NOT advance with unfilled fields");
    }

    #[test]
    fn no_step_button_returns_none() {
        let view = mock_view(vec![input_el(0, "Email"), button_el(1, "Submit")]);

        let result = try_next_step(&view, "fill form", &[0]);
        assert!(
            result.is_none(),
            "Submit is not a step keyword — should return None"
        );
    }
}
