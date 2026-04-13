//! SS-3: Centralized goal intent parser.
//!
//! Extracts structured intents from natural-language goal strings.
//! Previously, prefix-matching logic was duplicated across:
//! - `heuristics/navigation.rs` (click/go to/navigate to/open)
//! - `heuristics/search.rs` (search for/find/look up)
//! - `pilot/decide.rs` (click/go to/type/enter/fill + selectors)
//! - `heuristics/login.rs` (as/user/email/password credential parsing)
//!
//! This module provides a single `parse_intent` entry point that returns
//! a structured `Intent` enum. Callers can match on variants instead
//! of re-parsing the goal string.

use std::collections::HashMap;

/// A structured representation of what the user wants to accomplish.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    /// Login with optional credentials.
    Login {
        username: Option<String>,
        password: Option<String>,
    },
    /// Search for a query.
    Search { query: String },
    /// Navigate to a target (click, go to, open).
    Navigate { target: String },
    /// Type text, optionally into a named target.
    Type { text: String, into: Option<String> },
    /// Click a specific target element.
    Click { target: String },
    /// Fill a form with field-value pairs.
    FillForm { fields: HashMap<String, String> },
    /// No specific pattern matched -- use goal as-is.
    Generic { goal: String },
}

/// Parse a natural-language goal string into a structured intent.
///
/// Pattern priority (first match wins):
/// 1. Login patterns ("login as X password Y", "sign in ...")
/// 2. Search patterns ("search for X", "find X", "look up X")
/// 3. Type patterns ("type X into Y", "enter X into Y")
/// 4. Navigation patterns ("click X", "go to X", "navigate to X", "open X")
/// 5. Fill form patterns ("fill email=X password=Y")
/// 6. Generic fallback
pub fn parse_intent(goal: &str) -> Intent {
    let lower = goal.to_lowercase();

    // Priority 1: Explicit action verbs at the start of the goal.
    // These take precedence over keyword-based detection (e.g. "login").

    // 1a. Type / Enter (must come before search to avoid "search box" false positives)
    if let Some(intent) = try_parse_type(&lower) {
        return intent;
    }

    // 1b. Navigation (click, go to, navigate to, open)
    if let Some(intent) = try_parse_navigate(&lower) {
        return intent;
    }

    // 1c. Search (search for, find, look up)
    if let Some(intent) = try_parse_search(&lower) {
        return intent;
    }

    // 1d. Fill form (field=value pairs)
    if let Some(intent) = try_parse_fill_form(&lower) {
        return intent;
    }

    // Priority 2: Keyword-based detection (login/sign in).
    if let Some(intent) = try_parse_login(&lower) {
        return intent;
    }

    // 3. Generic fallback
    Intent::Generic {
        goal: goal.to_string(),
    }
}

// -- Login ---------------------------------------------------------

fn try_parse_login(lower: &str) -> Option<Intent> {
    let is_login = lower.contains("login")
        || lower.contains("log in")
        || lower.contains("sign in")
        || lower.contains("signin");

    if !is_login {
        return None;
    }

    let username = extract_after_prefix(lower, &["as ", "user ", "username ", "email ", "login "]);
    let password = extract_after_prefix(lower, &["password ", "pass ", "pw "]);

    Some(Intent::Login { username, password })
}

// -- Search --------------------------------------------------------

fn try_parse_search(lower: &str) -> Option<Intent> {
    let prefixes = ["search for ", "search ", "find ", "look up "];
    for prefix in &prefixes {
        if let Some(pos) = lower.find(prefix) {
            let rest = lower[pos + prefix.len()..].trim();
            if !rest.is_empty() {
                return Some(Intent::Search {
                    query: rest.to_string(),
                });
            }
        }
    }
    None
}

// -- Type ----------------------------------------------------------

fn try_parse_type(lower: &str) -> Option<Intent> {
    for verb in &["type ", "enter "] {
        if let Some(verb_pos) = lower.find(verb) {
            let after_verb = &lower[verb_pos + verb.len()..];

            // "type X into Y" pattern
            if let Some(into_pos) = after_verb.find(" into ") {
                let text = extract_possibly_quoted(&after_verb[..into_pos]);
                let target = after_verb[into_pos + " into ".len()..].trim();
                if !text.is_empty() {
                    return Some(Intent::Type {
                        text,
                        into: if target.is_empty() {
                            None
                        } else {
                            Some(target.to_string())
                        },
                    });
                }
            }

            // "type X" (no target)
            let text = extract_possibly_quoted(after_verb.trim());
            if !text.is_empty() {
                return Some(Intent::Type { text, into: None });
            }
        }
    }
    None
}

// -- Navigate ------------------------------------------------------

fn try_parse_navigate(lower: &str) -> Option<Intent> {
    let prefixes = ["click ", "go to ", "navigate to ", "open "];
    for prefix in &prefixes {
        if let Some(pos) = lower.find(prefix) {
            let rest = lower[pos + prefix.len()..].trim();
            if !rest.is_empty() {
                // Distinguish "click X" vs generic navigation
                if prefix.starts_with("click") {
                    return Some(Intent::Click {
                        target: rest.to_string(),
                    });
                }
                return Some(Intent::Navigate {
                    target: rest.to_string(),
                });
            }
        }
    }
    None
}

// -- Fill form -----------------------------------------------------

fn try_parse_fill_form(lower: &str) -> Option<Intent> {
    if !lower.starts_with("fill ") {
        return None;
    }
    let rest = &lower["fill ".len()..];

    // Parse field=value pairs separated by whitespace
    let mut fields = HashMap::new();
    for part in rest.split_whitespace() {
        if let Some(eq_pos) = part.find('=') {
            let field_name = &part[..eq_pos];
            let val = &part[eq_pos + 1..];
            if !field_name.is_empty() && !val.is_empty() {
                fields.insert(field_name.to_string(), val.to_string());
            }
        }
    }

    if fields.is_empty() {
        return None;
    }

    Some(Intent::FillForm { fields })
}

// -- Helpers -------------------------------------------------------

/// Extract the first non-stop-word token after any of the given prefixes.
///
/// Supports quoted values: `password "my pass"` -> `"my pass"`.
fn extract_after_prefix(lower: &str, prefixes: &[&str]) -> Option<String> {
    const STOP_WORDS: &[&str] = &[
        "with", "and", "then", "password", "pass", "in", "the", "to", "a", "for", "on", "my",
        "this",
    ];

    for prefix in prefixes {
        if let Some(pos) = lower.find(prefix) {
            let after = &lower[pos + prefix.len()..];
            let trimmed = after.trim_start();
            if trimmed.is_empty() {
                continue;
            }

            // Quoted value
            if trimmed.starts_with('"') || trimmed.starts_with('\'') {
                let quote = trimmed.as_bytes()[0] as char;
                let inner = &trimmed[1..];
                if let Some(end) = inner.find(quote) {
                    let val = &inner[..end];
                    if !val.is_empty() {
                        return Some(val.to_string());
                    }
                }
            }

            // Unquoted: first whitespace-delimited token
            let value = trimmed.split_whitespace().next();
            if let Some(v) = value
                && !v.is_empty()
                && !STOP_WORDS.contains(&v)
            {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Extract a possibly-quoted string value.
fn extract_possibly_quoted(s: &str) -> String {
    let trimmed = s.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_login_with_credentials() {
        let intent = parse_intent("login as admin password secret123");
        assert!(matches!(intent, Intent::Login { .. }));
        if let Intent::Login { username, password } = intent {
            assert_eq!(username, Some("admin".into()));
            assert_eq!(password, Some("secret123".into()));
        }
    }

    #[test]
    fn parse_login_no_credentials() {
        let intent = parse_intent("login to the dashboard");
        assert!(matches!(intent, Intent::Login { .. }));
        if let Intent::Login { username, password } = intent {
            assert!(username.is_none());
            assert!(password.is_none());
        }
    }

    #[test]
    fn parse_sign_in() {
        let intent = parse_intent("sign in as user@test.com pw hunter2");
        assert!(matches!(intent, Intent::Login { .. }));
        if let Intent::Login { username, password } = intent {
            assert_eq!(username, Some("user@test.com".into()));
            assert_eq!(password, Some("hunter2".into()));
        }
    }

    #[test]
    fn parse_search() {
        let intent = parse_intent("search for rust tutorials");
        assert_eq!(
            intent,
            Intent::Search {
                query: "rust tutorials".into()
            }
        );
    }

    #[test]
    fn parse_find() {
        let intent = parse_intent("find cheap flights");
        assert_eq!(
            intent,
            Intent::Search {
                query: "cheap flights".into()
            }
        );
    }

    #[test]
    fn parse_look_up() {
        let intent = parse_intent("look up error codes");
        assert_eq!(
            intent,
            Intent::Search {
                query: "error codes".into()
            }
        );
    }

    #[test]
    fn parse_click() {
        let intent = parse_intent("click the login button");
        assert_eq!(
            intent,
            Intent::Click {
                target: "the login button".into()
            }
        );
    }

    #[test]
    fn parse_go_to() {
        let intent = parse_intent("go to settings");
        assert_eq!(
            intent,
            Intent::Navigate {
                target: "settings".into()
            }
        );
    }

    #[test]
    fn parse_navigate_to() {
        let intent = parse_intent("navigate to dashboard");
        assert_eq!(
            intent,
            Intent::Navigate {
                target: "dashboard".into()
            }
        );
    }

    #[test]
    fn parse_type_into() {
        let intent = parse_intent("type hello into search box");
        assert_eq!(
            intent,
            Intent::Type {
                text: "hello".into(),
                into: Some("search box".into()),
            }
        );
    }

    #[test]
    fn parse_type_quoted_into() {
        let intent = parse_intent("type \"hello world\" into email");
        assert_eq!(
            intent,
            Intent::Type {
                text: "hello world".into(),
                into: Some("email".into()),
            }
        );
    }

    #[test]
    fn parse_fill_form_field_value() {
        let intent = parse_intent("fill email=test@test.com password=secret");
        if let Intent::FillForm { fields } = intent {
            assert_eq!(fields.len(), 2);
            assert_eq!(fields["email"], "test@test.com");
            assert_eq!(fields["password"], "secret");
        } else {
            panic!("expected FillForm, got {intent:?}");
        }
    }

    #[test]
    fn parse_generic() {
        let intent = parse_intent("wait for the page to load");
        assert!(matches!(intent, Intent::Generic { .. }));
    }

    #[test]
    fn parse_login_quoted_password() {
        let intent = parse_intent(r#"login as bob password "my complex pass""#);
        if let Intent::Login { username, password } = intent {
            assert_eq!(username, Some("bob".into()));
            assert_eq!(password, Some("my complex pass".into()));
        } else {
            panic!("expected Login, got {intent:?}");
        }
    }

    #[test]
    fn parse_non_ascii_no_panic() {
        // Must not panic on non-ASCII
        let intent = parse_intent("click Acao Rapida");
        assert!(matches!(intent, Intent::Click { .. }));
    }

    #[test]
    fn parse_open() {
        let intent = parse_intent("open the menu");
        assert_eq!(
            intent,
            Intent::Navigate {
                target: "the menu".into()
            }
        );
    }
}
