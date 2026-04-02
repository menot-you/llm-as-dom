//! Navigation heuristic: match "click X" or "go to X" goals to links/buttons.

use crate::pilot::Action;
use crate::semantic::{ElementKind, SemanticView};

/// If the goal says "click X" or "go to X", find a link or button with matching text.
///
/// Scores candidates by exact-vs-partial match and prefers links for "go to" goals.
pub fn try_navigation(
    view: &SemanticView,
    goal: &str,
    acted_on: &[u32],
) -> Option<super::HeuristicResult> {
    let target = extract_nav_target(goal)?;
    let target_lower = target.to_lowercase();

    let mut best: Option<(u32, f32)> = None;

    for el in &view.elements {
        if acted_on.contains(&el.id) || el.disabled {
            continue;
        }

        let is_clickable = matches!(el.kind, ElementKind::Link | ElementKind::Button);
        if !is_clickable {
            continue;
        }

        let label_lower = el.label.to_lowercase();
        let href_lower = el.href.as_deref().unwrap_or("").to_lowercase();

        // Exact label match is highest confidence
        let score = if label_lower == target_lower {
            0.95
        } else if label_lower.contains(&target_lower) {
            0.85
        } else if href_lower.contains(&target_lower) {
            0.75
        } else {
            continue;
        };

        if best.is_none_or(|(_, s)| score > s) {
            best = Some((el.id, score));
        }
    }

    best.map(|(id, conf)| super::HeuristicResult {
        action: Some(Action::Click {
            element: id,
            reasoning: format!(
                "heuristic: click element [{}] matching navigation target \"{target}\"",
                id
            ),
        }),
        confidence: conf,
        reason: format!("navigation target \"{target}\" matched"),
    })
}

/// Extract the navigation target from a goal string (case-insensitive prefix match).
///
/// Supports patterns: "click X", "go to X", "navigate to X", "open X".
/// Preserves the original case of the extracted target.
fn extract_nav_target(goal: &str) -> Option<String> {
    let lower = goal.to_lowercase();
    let prefixes = ["click ", "go to ", "navigate to ", "open "];
    for prefix in &prefixes {
        if let Some(pos) = lower.find(prefix) {
            let rest = goal[pos + prefix.len()..].trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_click_target() {
        assert_eq!(extract_nav_target("click About"), Some("About".into()));
    }

    #[test]
    fn extract_go_to_target() {
        assert_eq!(
            extract_nav_target("go to Settings"),
            Some("Settings".into())
        );
    }

    #[test]
    fn no_nav_target() {
        assert_eq!(extract_nav_target("login as admin"), None);
    }
}
