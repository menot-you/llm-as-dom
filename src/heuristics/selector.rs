use crate::pilot::Action;
use crate::selector::{Selector, find_best};
use crate::semantic::SemanticView;

/// Heuristic: Execute explicit selection goals like "click the .submit-btn" or "click [name=email]"
///
/// Uses `selector.rs` which supports natural language and CSS-like patterns.
pub fn try_selector(
    view: &SemanticView,
    goal: &str,
    acted_on: &[u32],
) -> Option<super::HeuristicResult> {
    let lower = goal.to_lowercase();

    // Check if the goal implicitly requests clicking something.
    // "click the login button" -> "login button"
    let target = if let Some(pos) = lower.find("click ") {
        goal[pos + 6..].trim()
    } else {
        // Assume the entire goal is a selector request if it contains syntax hinting at a selector like brackets or hashes
        if goal.contains('[') || goal.contains('#') || goal.contains(':') {
            goal.trim()
        } else {
            return None;
        }
    };

    if target.is_empty() {
        return None;
    }

    let sel = Selector::parse(target);
    if let Some(m) = find_best(view, &sel)
        && !acted_on.contains(&m.element_id)
    {
        return Some(super::HeuristicResult {
            action: Some(Action::Click {
                element: m.element_id,
                reasoning: format!(
                    "heuristic: matched explicit selector '{}' (reason: {})",
                    target, m.reason
                ),
            }),
            // We use `.min(0.9)` because explicit rules (like login forms) should typically win over fuzzy
            // selector logic unless it's an exact ID or Attribute match, which score >=0.95.
            // In selector.rs, a natural language "click X" match scores up to 0.95, which is high enough
            // to trigger the 0.6 threshold, but we slightly penalize fuzzy matches.
            confidence: (m.confidence * 0.95).min(0.9),
            reason: format!("selector '{}' matched element {}", target, m.element_id),
        });
    }

    None
}
