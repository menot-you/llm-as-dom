//! Search heuristic: detect search inputs and fill from goal.

use crate::pilot::Action;
use crate::semantic::{ElementKind, SemanticView};

/// Detect search inputs and fill them with the search term from the goal.
///
/// Matches elements by: `name=q`, `type=search`, `role=searchbox`, or label containing "search".
/// Extracts the search term from goal patterns like:
/// - `"search for X"`
/// - `"search X"`
/// - `"find X"`
/// - `"look up X"`
pub fn try_search(
    view: &SemanticView,
    goal: &str,
    acted_on: &[u32],
) -> Option<super::HeuristicResult> {
    let query = extract_search_query(goal)?;

    // Find the best search input
    for el in &view.elements {
        if acted_on.contains(&el.id) || el.disabled {
            continue;
        }

        let is_search = match el.kind {
            ElementKind::Input | ElementKind::Textarea => {
                el.input_type.as_deref() == Some("search")
                    || el
                        .name
                        .as_deref()
                        .map(|n| n == "q" || n == "query" || n == "search")
                        .unwrap_or(false)
                    || el.label.to_lowercase().contains("search")
                    || el
                        .placeholder
                        .as_deref()
                        .map(|p| p.to_lowercase().contains("search"))
                        .unwrap_or(false)
            }
            _ => false,
        };

        if is_search {
            return Some(super::HeuristicResult {
                action: Some(Action::Type {
                    element: el.id,
                    value: query.clone(),
                    reasoning: format!("heuristic: fill search input [{}] with query", el.id),
                }),
                confidence: 0.90,
                reason: format!("search input matched, query=\"{query}\""),
            });
        }
    }

    None
}

/// Extract the search query from a goal string (case-insensitive prefix match).
///
/// Supports patterns: "search for X", "search X", "find X", "look up X".
/// Preserves the original case of the extracted query.
fn extract_search_query(goal: &str) -> Option<String> {
    let lower = goal.to_lowercase();
    let prefixes = ["search for ", "search ", "find ", "look up "];
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
    fn extract_search_for_query() {
        assert_eq!(
            extract_search_query("search for rust tutorials"),
            Some("rust tutorials".into())
        );
    }

    #[test]
    fn extract_find_query() {
        assert_eq!(
            extract_search_query("find cheap flights"),
            Some("cheap flights".into())
        );
    }

    #[test]
    fn no_search_returns_none() {
        assert_eq!(extract_search_query("login as admin"), None);
    }
}
