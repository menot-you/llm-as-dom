//! Tier 1 heuristic: resolve actions from explicit `@lad/hints` annotations.
//!
//! When developers annotate their HTML with `data-lad="field:email"` or
//! `data-lad="action:submit"`, these hints provide near-certain intent —
//! bypassing generic heuristic guessing entirely.

use crate::pilot::Action;
use crate::semantic::{ElementKind, SemanticView};

/// Confidence score for hint-based decisions.
///
/// Very high because hints are explicit developer annotations, not guesses.
const HINT_CONFIDENCE: f32 = 0.98;

/// Try to resolve the next action using `@lad/hints` (`data-lad` attributes).
///
/// Checks elements for `ElementHint` annotations and maps them to actions
/// based on the current goal. Returns a high-confidence result when a hint
/// matches, or `None` action when no hints are relevant.
///
/// # Supported hint types
///
/// - `field:<name>` — input field (e.g. `field:email`, `field:password`)
/// - `action:<name>` — clickable action (e.g. `action:submit`, `action:login`)
/// - `form:<name>` — form container (used for scoping, not direct actions)
pub fn try_hints(view: &SemanticView, goal: &str, acted_on: &[u32]) -> super::HeuristicResult {
    let goal_lower = goal.to_lowercase();

    // Extract credentials from goal for field filling.
    let username = crate::heuristics::login::extract_credential(
        &goal_lower,
        &["as ", "user ", "username ", "email ", "login "],
    );
    let password =
        crate::heuristics::login::extract_credential(&goal_lower, &["password ", "pass ", "pw "]);

    // Pass 1: Fill hinted fields that haven't been acted on yet.
    for el in &view.elements {
        if acted_on.contains(&el.id) || el.disabled {
            continue;
        }

        let hint = match &el.hint {
            Some(h) => h,
            None => continue,
        };

        if hint.hint_type == "field" {
            let value = match hint.value.as_str() {
                "email" | "username" | "user" | "login" => username.clone(),
                "password" | "secret" | "pass" => password.clone(),
                _ => None,
            };

            if let Some(val) = value {
                return super::HeuristicResult {
                    action: Some(Action::Type {
                        element: el.id,
                        value: val,
                        reasoning: format!(
                            "hint: fill [{}] via data-lad=\"field:{}\"",
                            el.id, hint.value
                        ),
                    }),
                    confidence: HINT_CONFIDENCE,
                    reason: format!("hint field:{} matched", hint.value),
                };
            }
        }
    }

    // Pass 2: Click hinted action elements (only if all hinted fields are filled).
    let unfilled_hinted_fields = view
        .elements
        .iter()
        .filter(|e| {
            !acted_on.contains(&e.id)
                && !e.disabled
                && matches!(
                    e.kind,
                    ElementKind::Input | ElementKind::Textarea | ElementKind::Select
                )
                && e.hint.as_ref().is_some_and(|h| h.hint_type == "field")
        })
        .count();

    if unfilled_hinted_fields == 0 && !acted_on.is_empty() {
        for el in &view.elements {
            if acted_on.contains(&el.id) || el.disabled {
                continue;
            }

            let hint = match &el.hint {
                Some(h) if h.hint_type == "action" => h,
                _ => continue,
            };

            return super::HeuristicResult {
                action: Some(Action::Click {
                    element: el.id,
                    reasoning: format!(
                        "hint: click [{}] via data-lad=\"action:{}\"",
                        el.id, hint.value
                    ),
                }),
                confidence: HINT_CONFIDENCE,
                reason: format!("hint action:{} matched", hint.value),
            };
        }
    }

    super::HeuristicResult {
        action: None,
        confidence: 0.0,
        reason: "no hints matched".into(),
    }
}
