//! Playbook system: deterministic step-by-step replay for known workflows.
//!
//! Playbooks are JSON files stored in `.lad/playbooks/` that describe a
//! sequence of actions for a known page. When a playbook matches the current
//! URL, the pilot replays it step-by-step instead of using heuristics or LLM.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::heuristics::login::extract_credential;
use crate::pilot::Action;
use crate::semantic::SemanticView;

// ── Data model ────────────────────────────────────────────────────────

/// A recorded workflow that can be replayed deterministically.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playbook {
    /// Human-readable name (e.g. `"github-login"`).
    pub name: String,
    /// URL substring to match against (e.g. `"github.com/login"`).
    pub url_pattern: String,
    /// Ordered sequence of actions to replay.
    pub steps: Vec<PlaybookStep>,
    /// Parameter names expected in the goal (e.g. `["username", "password"]`).
    pub params: Vec<String>,
    /// Optional signal that the workflow succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<SuccessSignal>,
}

/// A single step in a playbook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybookStep {
    /// What kind of action to perform.
    pub kind: StepKind,
    /// CSS-like selector label to match against `SemanticView` elements.
    pub selector: String,
    /// Value to type/select. Supports `${param}` interpolation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Fallback selectors if the primary one doesn't match.
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

/// The kind of action a playbook step performs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// Click an element.
    Click,
    /// Type text into an input.
    Type,
    /// Select an option from a dropdown.
    Select,
    /// Navigate to a URL.
    Navigate,
}

/// Signals that indicate the playbook workflow succeeded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessSignal {
    /// URL must contain this substring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_contains: Option<String>,
    /// Page title must contain this substring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_contains: Option<String>,
}

// ── Storage ───────────────────────────────────────────────────────────

/// Load all playbook JSON files from a directory.
///
/// Silently skips files that fail to parse. Returns an empty vec if the
/// directory doesn't exist.
pub fn load_playbooks(dir: &Path) -> Vec<Playbook> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut playbooks = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<Playbook>(&contents) {
                Ok(pb) => playbooks.push(pb),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "skipping invalid playbook");
                }
            },
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to read playbook");
            }
        }
    }
    playbooks
}

/// Find the first playbook whose `url_pattern` is a substring of the given URL.
pub fn find_playbook<'a>(playbooks: &'a [Playbook], url: &str) -> Option<&'a Playbook> {
    playbooks.iter().find(|pb| url.contains(&pb.url_pattern))
}

// ── Parameter interpolation ───────────────────────────────────────────

/// Extract parameter values from a goal string using credential-style parsing.
///
/// Maps playbook param names to goal keywords:
/// - `"username"` -> tries prefixes `["as ", "user ", "username ", "email ", "login "]`
/// - `"password"` -> tries prefixes `["password ", "pass ", "pw "]`
/// - anything else -> tries `["<name> "]`
pub fn extract_params(
    goal: &str,
    param_names: &[String],
) -> std::collections::HashMap<String, String> {
    let goal_lower = goal.to_lowercase();
    let mut params = std::collections::HashMap::new();

    for name in param_names {
        let value = match name.as_str() {
            "username" | "user" | "email" => extract_credential(
                &goal_lower,
                &["as ", "user ", "username ", "email ", "login "],
            ),
            "password" | "pass" | "pw" => {
                extract_credential(&goal_lower, &["password ", "pass ", "pw "])
            }
            other => {
                let prefix = format!("{other} ");
                extract_credential(&goal_lower, &[&prefix])
            }
        };
        if let Some(v) = value {
            params.insert(name.clone(), v);
        }
    }

    params
}

/// Replace `${param}` placeholders in a template string with actual values.
///
/// Unknown params are left as-is (e.g. `${unknown}` stays `${unknown}`).
pub fn interpolate(template: &str, params: &std::collections::HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in params {
        let placeholder = format!("${{{key}}}");
        result = result.replace(&placeholder, value);
    }
    result
}

// ── Selector matching ─────────────────────────────────────────────────

/// Find an element in the `SemanticView` that matches a selector string.
///
/// Matching strategy (in order of priority):
/// 1. Exact label match (case-insensitive)
/// 2. Label contains the selector (case-insensitive)
/// 3. Name attribute matches (case-insensitive)
/// 4. Input type matches (for selectors like `"password"`)
pub fn match_selector(view: &SemanticView, selector: &str) -> Option<u32> {
    let sel_lower = selector.to_lowercase();

    // Pass 1: exact label match
    for el in &view.elements {
        if el.label.to_lowercase() == sel_lower {
            return Some(el.id);
        }
    }

    // Pass 2: label contains
    for el in &view.elements {
        if el.label.to_lowercase().contains(&sel_lower) {
            return Some(el.id);
        }
    }

    // Pass 3: name attribute match
    for el in &view.elements {
        if let Some(ref name) = el.name
            && name.to_lowercase() == sel_lower
        {
            return Some(el.id);
        }
    }

    // Pass 4: input_type match (e.g. selector="password")
    for el in &view.elements {
        if let Some(ref itype) = el.input_type
            && itype.to_lowercase() == sel_lower
        {
            return Some(el.id);
        }
    }

    None
}

/// Try all selectors (primary + fallbacks) and return the first match.
pub fn match_step_selector(view: &SemanticView, step: &PlaybookStep) -> Option<u32> {
    if let Some(id) = match_selector(view, &step.selector) {
        return Some(id);
    }
    for fallback in &step.fallbacks {
        if let Some(id) = match_selector(view, fallback) {
            return Some(id);
        }
    }
    None
}

// ── Step-to-Action conversion ─────────────────────────────────────────

/// Convert a playbook step into a pilot `Action`.
///
/// Returns `None` if the selector can't be matched to any element.
pub fn step_to_action(
    view: &SemanticView,
    step: &PlaybookStep,
    params: &std::collections::HashMap<String, String>,
) -> Option<Action> {
    match step.kind {
        StepKind::Navigate => {
            let url = step.value.as_deref().unwrap_or(&step.selector);
            let resolved = interpolate(url, params);
            // Navigate is not a standard Action — we use it as a Click on a matching link
            // or escalate if no element matches
            Some(Action::Scroll {
                direction: "down".into(),
                reasoning: format!("playbook: navigate to {resolved} (not yet implemented)"),
            })
        }
        StepKind::Click => {
            let element = match_step_selector(view, step)?;
            Some(Action::Click {
                element,
                reasoning: format!("playbook: click \"{}\"", step.selector),
            })
        }
        StepKind::Type => {
            let element = match_step_selector(view, step)?;
            let raw_value = step.value.as_deref().unwrap_or("");
            let resolved = interpolate(raw_value, params);
            Some(Action::Type {
                element,
                value: resolved,
                reasoning: format!("playbook: type into \"{}\"", step.selector),
            })
        }
        StepKind::Select => {
            let element = match_step_selector(view, step)?;
            let raw_value = step.value.as_deref().unwrap_or("");
            let resolved = interpolate(raw_value, params);
            Some(Action::Select {
                element,
                value: resolved,
                reasoning: format!("playbook: select in \"{}\"", step.selector),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{Element, ElementKind, PageState};
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn sample_playbook_json() -> &'static str {
        r#"{
            "name": "github-login",
            "url_pattern": "github.com/login",
            "steps": [
                {
                    "kind": "type",
                    "selector": "Username or email address",
                    "value": "${username}",
                    "fallbacks": ["login_field", "email"]
                },
                {
                    "kind": "type",
                    "selector": "Password",
                    "value": "${password}",
                    "fallbacks": ["password"]
                },
                {
                    "kind": "click",
                    "selector": "Sign in",
                    "fallbacks": ["commit"]
                }
            ],
            "params": ["username", "password"],
            "success": {
                "url_contains": "github.com",
                "title_contains": "GitHub"
            }
        }"#
    }

    fn sample_view() -> SemanticView {
        SemanticView {
            url: "https://github.com/login".into(),
            title: "Sign in to GitHub".into(),
            page_hint: "login page".into(),
            elements: vec![
                Element {
                    id: 0,
                    kind: ElementKind::Input,
                    label: "Username or email address".into(),
                    name: Some("login_field".into()),
                    value: None,
                    placeholder: None,
                    href: None,
                    input_type: Some("text".into()),
                    disabled: false,
                    form_index: Some(0),
                    context: None,
                    hint: None,
                    checked: None,
                    options: None,
                    frame_index: None,
                    is_visible: None,
                },
                Element {
                    id: 1,
                    kind: ElementKind::Input,
                    label: "Password".into(),
                    name: Some("password".into()),
                    value: None,
                    placeholder: None,
                    href: None,
                    input_type: Some("password".into()),
                    disabled: false,
                    form_index: Some(0),
                    context: None,
                    hint: None,
                    checked: None,
                    options: None,
                    frame_index: None,
                    is_visible: None,
                },
                Element {
                    id: 2,
                    kind: ElementKind::Button,
                    label: "Sign in".into(),
                    name: Some("commit".into()),
                    value: None,
                    placeholder: None,
                    href: None,
                    input_type: Some("submit".into()),
                    disabled: false,
                    form_index: Some(0),
                    context: None,
                    hint: None,
                    checked: None,
                    options: None,
                    frame_index: None,
                    is_visible: None,
                },
            ],
            forms: vec![],
            visible_text: "Sign in to GitHub".into(),
            text_blocks: vec![],
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
            cards: None,
        }
    }

    // ── test_playbook_load ────────────────────────────────────────────

    #[test]
    fn test_playbook_load_from_json() {
        let pb: Playbook = serde_json::from_str(sample_playbook_json()).unwrap();
        assert_eq!(pb.name, "github-login");
        assert_eq!(pb.url_pattern, "github.com/login");
        assert_eq!(pb.steps.len(), 3);
        assert_eq!(pb.params, vec!["username", "password"]);
        assert!(pb.success.is_some());
        let success = pb.success.unwrap();
        assert_eq!(success.url_contains, Some("github.com".into()));
        assert_eq!(success.title_contains, Some("GitHub".into()));
    }

    #[test]
    fn test_playbook_load_step_fields() {
        let pb: Playbook = serde_json::from_str(sample_playbook_json()).unwrap();
        let step0 = &pb.steps[0];
        assert_eq!(step0.kind, StepKind::Type);
        assert_eq!(step0.selector, "Username or email address");
        assert_eq!(step0.value, Some("${username}".into()));
        assert_eq!(step0.fallbacks, vec!["login_field", "email"]);
    }

    #[test]
    fn test_playbook_load_from_dir() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("github.json"), sample_playbook_json()).unwrap();
        // Also write a non-json file that should be skipped
        std::fs::write(dir.path().join("readme.txt"), "not a playbook").unwrap();

        let playbooks = load_playbooks(dir.path());
        assert_eq!(playbooks.len(), 1);
        assert_eq!(playbooks[0].name, "github-login");
    }

    #[test]
    fn test_playbook_load_nonexistent_dir() {
        let playbooks = load_playbooks(Path::new("/tmp/nonexistent-lad-playbooks-xyz"));
        assert!(playbooks.is_empty());
    }

    #[test]
    fn test_playbook_load_skips_invalid_json() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("broken.json"), "{ not valid json }").unwrap();
        let playbooks = load_playbooks(dir.path());
        assert!(playbooks.is_empty());
    }

    // ── test_playbook_match ───────────────────────────────────────────

    #[test]
    fn test_playbook_match_url() {
        let pb: Playbook = serde_json::from_str(sample_playbook_json()).unwrap();
        let playbooks = vec![pb];

        let found = find_playbook(&playbooks, "https://github.com/login");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "github-login");
    }

    #[test]
    fn test_playbook_no_match() {
        let pb: Playbook = serde_json::from_str(sample_playbook_json()).unwrap();
        let playbooks = vec![pb];

        let found = find_playbook(&playbooks, "https://gitlab.com/login");
        assert!(found.is_none());
    }

    #[test]
    fn test_playbook_match_substring() {
        let pb: Playbook = serde_json::from_str(sample_playbook_json()).unwrap();
        let playbooks = vec![pb];

        // Should match even with query params
        let found = find_playbook(&playbooks, "https://github.com/login?return_to=%2F");
        assert!(found.is_some());
    }

    // ── test_playbook_interpolation ───────────────────────────────────

    #[test]
    fn test_interpolate_single_param() {
        let mut params = HashMap::new();
        params.insert("username".into(), "alice".into());

        assert_eq!(interpolate("${username}", &params), "alice");
    }

    #[test]
    fn test_interpolate_multiple_params() {
        let mut params = HashMap::new();
        params.insert("username".into(), "alice".into());
        params.insert("password".into(), "s3cret".into());

        assert_eq!(
            interpolate("user=${username}&pw=${password}", &params),
            "user=alice&pw=s3cret"
        );
    }

    #[test]
    fn test_interpolate_unknown_param_preserved() {
        let params = HashMap::new();
        assert_eq!(interpolate("${unknown}", &params), "${unknown}");
    }

    #[test]
    fn test_extract_params_from_goal() {
        let params = extract_params(
            "login as alice@test.com password s3cret",
            &["username".into(), "password".into()],
        );
        assert_eq!(params.get("username"), Some(&"alice@test.com".into()));
        assert_eq!(params.get("password"), Some(&"s3cret".into()));
    }

    // ── test_playbook_to_action ───────────────────────────────────────

    #[test]
    fn test_step_to_action_type() {
        let view = sample_view();
        let step = PlaybookStep {
            kind: StepKind::Type,
            selector: "Username or email address".into(),
            value: Some("${username}".into()),
            fallbacks: vec![],
        };
        let mut params = HashMap::new();
        params.insert("username".into(), "alice".into());

        let action = step_to_action(&view, &step, &params).unwrap();
        match action {
            Action::Type { element, value, .. } => {
                assert_eq!(element, 0);
                assert_eq!(value, "alice");
            }
            other => panic!("expected Type, got {other:?}"),
        }
    }

    #[test]
    fn test_step_to_action_click() {
        let view = sample_view();
        let step = PlaybookStep {
            kind: StepKind::Click,
            selector: "Sign in".into(),
            value: None,
            fallbacks: vec![],
        };
        let params = HashMap::new();

        let action = step_to_action(&view, &step, &params).unwrap();
        match action {
            Action::Click { element, .. } => assert_eq!(element, 2),
            other => panic!("expected Click, got {other:?}"),
        }
    }

    #[test]
    fn test_step_to_action_with_fallback() {
        let view = sample_view();
        let step = PlaybookStep {
            kind: StepKind::Type,
            selector: "nonexistent".into(),
            value: Some("s3cret".into()),
            fallbacks: vec!["password".into()],
        };
        let params = HashMap::new();

        let action = step_to_action(&view, &step, &params).unwrap();
        match action {
            Action::Type { element, value, .. } => {
                assert_eq!(element, 1); // matched via name="password"
                assert_eq!(value, "s3cret");
            }
            other => panic!("expected Type, got {other:?}"),
        }
    }

    #[test]
    fn test_step_to_action_no_match_returns_none() {
        let view = sample_view();
        let step = PlaybookStep {
            kind: StepKind::Click,
            selector: "totally nonexistent button".into(),
            value: None,
            fallbacks: vec![],
        };
        let params = HashMap::new();

        assert!(step_to_action(&view, &step, &params).is_none());
    }

    #[test]
    fn test_selector_matches_by_input_type() {
        let view = sample_view();
        // "password" matches element 1 via input_type
        let id = match_selector(&view, "password");
        assert_eq!(id, Some(1));
    }

    #[test]
    fn test_selector_matches_by_name() {
        let view = sample_view();
        let id = match_selector(&view, "login_field");
        assert_eq!(id, Some(0));
    }

    #[test]
    fn test_step_select_action() {
        let mut view = sample_view();
        view.elements.push(Element {
            id: 3,
            kind: ElementKind::Select,
            label: "Country".into(),
            name: Some("country".into()),
            value: None,
            placeholder: None,
            href: None,
            input_type: None,
            disabled: false,
            form_index: Some(0),
            context: None,
            hint: None,
            checked: None,
            options: None,
            frame_index: None,
            is_visible: None,
        });

        let step = PlaybookStep {
            kind: StepKind::Select,
            selector: "Country".into(),
            value: Some("BR".into()),
            fallbacks: vec![],
        };
        let params = HashMap::new();

        let action = step_to_action(&view, &step, &params).unwrap();
        match action {
            Action::Select { element, value, .. } => {
                assert_eq!(element, 3);
                assert_eq!(value, "BR");
            }
            other => panic!("expected Select, got {other:?}"),
        }
    }
}
