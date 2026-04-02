//! Generic form-fill heuristic: parse `key=value` pairs from the goal and match to inputs.

use crate::pilot::Action;
use crate::semantic::{ElementKind, SemanticView};

/// For goals like `"fill form with name=John email=john@test.com"`, parse key=value pairs
/// and match them to input elements by name or label.
pub fn try_generic_form(
    view: &SemanticView,
    goal: &str,
    acted_on: &[u32],
) -> Option<super::HeuristicResult> {
    let pairs = extract_kv_pairs(goal);
    if pairs.is_empty() {
        return None;
    }

    for (key, value) in &pairs {
        let key_lower = key.to_lowercase();

        for el in &view.elements {
            if acted_on.contains(&el.id) || el.disabled {
                continue;
            }

            let is_input = matches!(
                el.kind,
                ElementKind::Input | ElementKind::Textarea | ElementKind::Select
            );
            if !is_input {
                continue;
            }

            let name_matches = el
                .name
                .as_deref()
                .map(|n| n.to_lowercase() == key_lower)
                .unwrap_or(false);
            let label_matches = el.label.to_lowercase().contains(&key_lower);

            if name_matches || label_matches {
                return Some(super::HeuristicResult {
                    action: Some(Action::Type {
                        element: el.id,
                        value: value.clone(),
                        reasoning: format!(
                            "heuristic: fill field [{}] ({key}) with \"{value}\"",
                            el.id
                        ),
                    }),
                    confidence: if name_matches { 0.90 } else { 0.75 },
                    reason: format!("generic form: matched {key}={value}"),
                });
            }
        }
    }

    None
}

/// Extract `key=value` pairs from a goal string.
///
/// Handles formats like:
/// - `"fill form with name=John email=john@test.com"`
/// - `"name=John email=john@test.com"`
///
/// Values can be quoted: `name="John Doe"`.
pub fn extract_kv_pairs(goal: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut remaining = goal;

    while let Some(eq_pos) = remaining.find('=') {
        // Key is the word immediately before '='
        let before = &remaining[..eq_pos];
        let key = before
            .split_whitespace()
            .next_back()
            .unwrap_or("")
            .to_string();

        if key.is_empty() {
            remaining = &remaining[eq_pos + 1..];
            continue;
        }

        // Value is after '=', either quoted or until next whitespace/key=value
        let after = &remaining[eq_pos + 1..];
        let value = if after.starts_with('"') {
            // Quoted value
            let end = after.get(1..).and_then(|s| s.find('"')).map(|i| i + 2).unwrap_or(after.len());
            after[1..end.saturating_sub(1)].to_string()
        } else {
            // Unquoted: take until whitespace, but stop before a word containing '='
            let mut val_end = after.len();
            for (i, chunk) in after.split_whitespace().enumerate() {
                if i > 0 && chunk.contains('=') {
                    // This chunk starts the next key=value pair
                    let offset = after.find(chunk).unwrap_or(after.len());
                    val_end = offset;
                    break;
                }
            }
            after[..val_end].trim().to_string()
        };

        if !value.is_empty() {
            pairs.push((key, value));
        }

        // Advance past the consumed value
        let consumed = eq_pos + 1 + after.len().min(
            remaining.len() - eq_pos - 1,
        );
        let advance = eq_pos + 1 + {
            if after.starts_with('"') {
                after.get(1..).and_then(|s| s.find('"')).map(|i| i + 2).unwrap_or(after.len())
            } else {
                let mut end = after.len();
                for (i, chunk) in after.split_whitespace().enumerate() {
                    if i > 0 && chunk.contains('=') {
                        end = after.find(chunk).unwrap_or(after.len());
                        break;
                    }
                }
                end
            }
        };
        let _ = consumed; // suppress unused warning
        if advance >= remaining.len() {
            break;
        }
        remaining = &remaining[advance..];
    }

    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_kv_pairs() {
        let pairs = extract_kv_pairs("fill form with name=John email=john@test.com");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("name".into(), "John".into()));
        assert_eq!(pairs[1], ("email".into(), "john@test.com".into()));
    }

    #[test]
    fn parse_quoted_value() {
        let pairs = extract_kv_pairs(r#"name="John Doe" city=NYC"#);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("name".into(), "John Doe".into()));
        assert_eq!(pairs[1], ("city".into(), "NYC".into()));
    }

    #[test]
    fn no_kv_pairs() {
        let pairs = extract_kv_pairs("login as admin password secret");
        assert!(pairs.is_empty());
    }
}
