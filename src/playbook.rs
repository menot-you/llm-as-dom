//! Playbook system: deterministic step-by-step replay for known workflows.
//!
//! Playbooks are JSON files stored in `.lad/playbooks/` that describe a
//! sequence of actions for a known page. When a playbook matches the current
//! URL, the pilot replays it step-by-step instead of using heuristics or LLM.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

// ── Synthesis: trajectory -> Playbook ─────────────────────────────────

/// Error returned by [`synthesize_from_history`] when a run cannot be turned
/// into a replayable playbook.
#[derive(Debug, thiserror::Error)]
pub enum SynthesizeError {
    /// The history contains no steps at all.
    #[error("history is empty")]
    EmptyHistory,
    /// The run did not end with a successful [`Action::Done`].
    #[error("history contains no successful completion")]
    NoCompletion,
    /// The initial URL could not be parsed into a `host + path` pattern.
    #[error("cannot derive URL pattern from initial URL: {0}")]
    InvalidUrl(String),
    /// No explicit name was supplied and the goal was too empty to derive one.
    #[error("cannot derive name from goal; pass --learn-name explicitly")]
    NameDerivationFailed,
}

/// Error returned by [`save`] when writing a playbook to disk fails.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    /// The target directory could not be created.
    #[error("playbook directory could not be created: {0}")]
    DirCreate(std::io::Error),
    /// Serialization to JSON failed.
    #[error("failed to serialize playbook: {0}")]
    Serialize(serde_json::Error),
    /// Writing the file (including atomic rename) failed.
    #[error("failed to write playbook file {path}: {source}")]
    Write {
        /// The path that failed to be written.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
}

/// Turn a successful pilot run into a replayable [`Playbook`].
///
/// Keeps only [`Action::Type`], [`Action::Click`], and [`Action::Select`]
/// steps; skips [`Action::Scroll`], [`Action::Wait`], [`Action::Navigate`],
/// [`Action::Escalate`], and the terminal [`Action::Done`].
///
/// When a `Type` / `Select` value matches one of the `explicit_params`
/// values, it is templatized as `${key}` so the same playbook can be
/// replayed later with different credentials.
///
/// Returns an error if the history is empty, contains no successful
/// `Done` action, cannot derive a name, or cannot parse the initial URL.
pub fn synthesize_from_history(
    history: &[crate::pilot::Step],
    goal: &str,
    initial_url: &str,
    explicit_params: &std::collections::HashMap<String, String>,
    name: Option<&str>,
) -> Result<Playbook, SynthesizeError> {
    if history.is_empty() {
        return Err(SynthesizeError::EmptyHistory);
    }

    // The run must conclude with a successful Done.
    let has_done = history
        .iter()
        .any(|s| matches!(s.action, Action::Done { .. }));
    if !has_done {
        return Err(SynthesizeError::NoCompletion);
    }

    // Derive the playbook name.
    let playbook_name = match name {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => derive_name_from_goal(goal).ok_or(SynthesizeError::NameDerivationFailed)?,
    };

    // Derive the url_pattern from the initial URL.
    let url_pattern = derive_url_pattern(initial_url)?;

    // Params list: sorted keys for deterministic output.
    let mut param_keys: Vec<String> = explicit_params.keys().cloned().collect();
    param_keys.sort();

    // Walk history and map actionable steps.
    let mut steps: Vec<PlaybookStep> = Vec::new();
    let mut final_view: Option<&SemanticView> = None;
    for step in history {
        final_view = Some(&step.observation);
        match &step.action {
            Action::Click { element, .. } => {
                if let Some(selector) = element_selector(&step.observation, *element) {
                    steps.push(PlaybookStep {
                        kind: StepKind::Click,
                        selector,
                        value: None,
                        fallbacks: Vec::new(),
                    });
                }
            }
            Action::Type { element, value, .. } => {
                if let Some(selector) = element_selector(&step.observation, *element) {
                    let templated = templatize(value, explicit_params);
                    steps.push(PlaybookStep {
                        kind: StepKind::Type,
                        selector,
                        value: Some(templated),
                        fallbacks: Vec::new(),
                    });
                }
            }
            Action::Select { element, value, .. } => {
                if let Some(selector) = element_selector(&step.observation, *element) {
                    let templated = templatize(value, explicit_params);
                    steps.push(PlaybookStep {
                        kind: StepKind::Select,
                        selector,
                        value: Some(templated),
                        fallbacks: Vec::new(),
                    });
                }
            }
            // Skip non-essential / terminal variants.
            Action::Scroll { .. }
            | Action::Wait { .. }
            | Action::Navigate { .. }
            | Action::Escalate { .. }
            | Action::Done { .. } => {}
        }
    }

    // Derive success signal from the final observation, if we have one.
    let success = final_view.and_then(|v| derive_success(v, initial_url));

    Ok(Playbook {
        name: playbook_name,
        url_pattern,
        steps,
        params: param_keys,
        success,
    })
}

/// Persist a [`Playbook`] as JSON to `dir/<name>.json`.
///
/// Creates `dir` recursively if missing. Writes are atomic: the content is
/// staged to a `.tmp` sibling and then renamed onto the final path. If the
/// target file already exists, it is overwritten with a `tracing::warn`.
pub fn save(playbook: &Playbook, dir: &Path) -> Result<PathBuf, SaveError> {
    if !dir.exists() {
        std::fs::create_dir_all(dir).map_err(SaveError::DirCreate)?;
    }

    let final_path = dir.join(format!("{}.json", playbook.name));
    if final_path.exists() {
        tracing::warn!(
            path = %final_path.display(),
            "overwriting existing playbook file"
        );
    }

    let json = serde_json::to_string_pretty(playbook).map_err(SaveError::Serialize)?;

    let tmp_path = final_path.with_extension("json.tmp");
    std::fs::write(&tmp_path, json.as_bytes()).map_err(|e| SaveError::Write {
        path: tmp_path.clone(),
        source: e,
    })?;
    std::fs::rename(&tmp_path, &final_path).map_err(|e| SaveError::Write {
        path: final_path.clone(),
        source: e,
    })?;

    Ok(final_path)
}

// ── Synthesis helpers ─────────────────────────────────────────────────

/// Extract `host + first path segment` from a URL, e.g.
/// `https://github.com/login?x=1` -> `github.com/login`.
fn derive_url_pattern(url: &str) -> Result<String, SynthesizeError> {
    if url.is_empty() {
        return Err(SynthesizeError::InvalidUrl("empty url".into()));
    }
    let parsed = url::Url::parse(url).map_err(|e| SynthesizeError::InvalidUrl(e.to_string()))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| SynthesizeError::InvalidUrl(format!("no host in {url}")))?;
    let first_segment = parsed
        .path_segments()
        .and_then(|mut s| s.next())
        .filter(|s| !s.is_empty());
    match first_segment {
        Some(seg) => Ok(format!("{host}/{seg}")),
        None => Ok(host.to_string()),
    }
}

/// Derive a playbook name from the goal: take the first three alphanumeric
/// words, lowercase, join with underscores. Returns `None` if the resulting
/// name is empty.
fn derive_name_from_goal(goal: &str) -> Option<String> {
    let tokens: Vec<String> = goal
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .take(3)
        .collect();
    if tokens.is_empty() {
        return None;
    }
    Some(tokens.join("_"))
}

/// Get the `label` of the element with the given id, falling back to `name`
/// then `input_type`. Returns `None` when the element is not present in the
/// view (defensive — should be rare).
fn element_selector(view: &SemanticView, element_id: u32) -> Option<String> {
    let el = view.elements.iter().find(|e| e.id == element_id)?;
    if !el.label.trim().is_empty() {
        return Some(el.label.clone());
    }
    if let Some(name) = &el.name
        && !name.trim().is_empty()
    {
        return Some(name.clone());
    }
    el.input_type.clone()
}

/// Replace any occurrence of each `params[key]` value in `raw` with `${key}`.
fn templatize(raw: &str, params: &std::collections::HashMap<String, String>) -> String {
    let mut out = raw.to_string();
    for (key, value) in params {
        if value.is_empty() {
            continue;
        }
        let placeholder = format!("${{{key}}}");
        out = out.replace(value, &placeholder);
    }
    out
}

/// Derive a [`SuccessSignal`] from the final view URL + title, when either
/// differs usefully from the initial URL. Returns `None` if neither is
/// distinguishable.
fn derive_success(final_view: &SemanticView, initial_url: &str) -> Option<SuccessSignal> {
    let initial_host = url::Url::parse(initial_url)
        .ok()
        .and_then(|u| u.host_str().map(|s| s.to_string()));

    let url_contains = match (initial_host.as_deref(), final_view.url.as_str()) {
        (Some(host), current) if current.contains(host) && current != initial_url => {
            Some(host.to_string())
        }
        _ => None,
    };
    let title_contains = if final_view.title.trim().is_empty() {
        None
    } else {
        Some(final_view.title.clone())
    };

    if url_contains.is_none() && title_contains.is_none() {
        None
    } else {
        Some(SuccessSignal {
            url_contains,
            title_contains,
        })
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

    // ── synthesize_from_history tests ─────────────────────────────────

    use crate::pilot::{DecisionSource, Step as PilotStep};
    use std::time::Duration;

    /// Build a minimal `Step` for synthesis tests.
    fn synth_step(
        idx: u32,
        view: SemanticView,
        action: Action,
        source: DecisionSource,
    ) -> PilotStep {
        PilotStep {
            index: idx,
            observation: view,
            action,
            source,
            confidence: 0.9,
            duration: Duration::from_millis(10),
        }
    }

    #[test]
    fn synthesize_empty_history_fails() {
        let params = HashMap::new();
        let err =
            synthesize_from_history(&[], "some goal", "https://example.com/login", &params, None)
                .unwrap_err();
        assert!(matches!(err, SynthesizeError::EmptyHistory));
    }

    #[test]
    fn synthesize_without_completion_fails() {
        let view = sample_view();
        let history = vec![synth_step(
            0,
            view,
            Action::Click {
                element: 2,
                reasoning: "test".into(),
            },
            DecisionSource::Heuristic,
        )];
        let params = HashMap::new();
        let err =
            synthesize_from_history(&history, "login", "https://github.com/login", &params, None)
                .unwrap_err();
        assert!(matches!(err, SynthesizeError::NoCompletion));
    }

    #[test]
    fn synthesize_basic_login() {
        let view = sample_view();
        let mut params = HashMap::new();
        params.insert("email".into(), "octocat".into());
        params.insert("password".into(), "hunter2".into());

        let history = vec![
            synth_step(
                0,
                view.clone(),
                Action::Type {
                    element: 0,
                    value: "octocat".into(),
                    reasoning: "heuristic".into(),
                },
                DecisionSource::Heuristic,
            ),
            synth_step(
                1,
                view.clone(),
                Action::Type {
                    element: 1,
                    value: "hunter2".into(),
                    reasoning: "heuristic".into(),
                },
                DecisionSource::Heuristic,
            ),
            synth_step(
                2,
                view.clone(),
                Action::Click {
                    element: 2,
                    reasoning: "heuristic".into(),
                },
                DecisionSource::Heuristic,
            ),
            synth_step(
                3,
                view,
                Action::Done {
                    result: serde_json::Value::Null,
                    reasoning: "logged in".into(),
                },
                DecisionSource::Llm,
            ),
        ];
        let pb = synthesize_from_history(
            &history,
            "login as octocat with password hunter2",
            "https://github.com/login",
            &params,
            Some("github-login"),
        )
        .unwrap();

        assert_eq!(pb.name, "github-login");
        assert_eq!(pb.url_pattern, "github.com/login");
        assert_eq!(pb.steps.len(), 3);
        assert_eq!(pb.steps[0].kind, StepKind::Type);
        assert_eq!(pb.steps[1].kind, StepKind::Type);
        assert_eq!(pb.steps[2].kind, StepKind::Click);
        // Params exposed in sorted order.
        assert_eq!(pb.params, vec!["email", "password"]);
    }

    #[test]
    fn synthesize_interpolates_params() {
        let view = sample_view();
        let mut params = HashMap::new();
        params.insert("email".into(), "octocat".into());
        params.insert("password".into(), "hunter2".into());

        let history = vec![
            synth_step(
                0,
                view.clone(),
                Action::Type {
                    element: 0,
                    value: "octocat".into(),
                    reasoning: "".into(),
                },
                DecisionSource::Heuristic,
            ),
            synth_step(
                1,
                view.clone(),
                Action::Type {
                    element: 1,
                    value: "hunter2".into(),
                    reasoning: "".into(),
                },
                DecisionSource::Heuristic,
            ),
            synth_step(
                2,
                view,
                Action::Done {
                    result: serde_json::Value::Null,
                    reasoning: "".into(),
                },
                DecisionSource::Llm,
            ),
        ];
        let pb = synthesize_from_history(
            &history,
            "login",
            "https://github.com/login",
            &params,
            Some("pb"),
        )
        .unwrap();

        assert_eq!(pb.steps[0].value.as_deref(), Some("${email}"));
        assert_eq!(pb.steps[1].value.as_deref(), Some("${password}"));
    }

    #[test]
    fn synthesize_derives_name_from_goal() {
        let view = sample_view();
        let history = vec![
            synth_step(
                0,
                view.clone(),
                Action::Click {
                    element: 2,
                    reasoning: "".into(),
                },
                DecisionSource::Heuristic,
            ),
            synth_step(
                1,
                view,
                Action::Done {
                    result: serde_json::Value::Null,
                    reasoning: "".into(),
                },
                DecisionSource::Llm,
            ),
        ];
        let params = HashMap::new();
        let pb = synthesize_from_history(
            &history,
            "Login as alice with password",
            "https://example.com/login",
            &params,
            None,
        )
        .unwrap();

        // Derived name contains "login" (lowercased, snake-cased from goal).
        assert!(
            pb.name.contains("login"),
            "expected derived name to contain 'login', got: {}",
            pb.name
        );
    }

    #[test]
    fn synthesize_invalid_url_fails() {
        let view = sample_view();
        let history = vec![synth_step(
            0,
            view,
            Action::Done {
                result: serde_json::Value::Null,
                reasoning: "".into(),
            },
            DecisionSource::Llm,
        )];
        let params = HashMap::new();
        let err = synthesize_from_history(&history, "login", "", &params, Some("pb")).unwrap_err();
        assert!(matches!(err, SynthesizeError::InvalidUrl(_)));
    }

    #[test]
    fn synthesize_name_derivation_failed_on_empty_goal() {
        let view = sample_view();
        let history = vec![synth_step(
            0,
            view,
            Action::Done {
                result: serde_json::Value::Null,
                reasoning: "".into(),
            },
            DecisionSource::Llm,
        )];
        let params = HashMap::new();
        let err = synthesize_from_history(&history, "", "https://example.com/login", &params, None)
            .unwrap_err();
        assert!(matches!(err, SynthesizeError::NameDerivationFailed));
    }

    #[test]
    fn synthesize_skips_scroll_and_escalate() {
        let view = sample_view();
        let history = vec![
            synth_step(
                0,
                view.clone(),
                Action::Scroll {
                    direction: "down".into(),
                    reasoning: "".into(),
                },
                DecisionSource::Heuristic,
            ),
            synth_step(
                1,
                view.clone(),
                Action::Click {
                    element: 2,
                    reasoning: "".into(),
                },
                DecisionSource::Heuristic,
            ),
            synth_step(
                2,
                view,
                Action::Done {
                    result: serde_json::Value::Null,
                    reasoning: "".into(),
                },
                DecisionSource::Llm,
            ),
        ];
        let params = HashMap::new();
        let pb = synthesize_from_history(
            &history,
            "login",
            "https://example.com/login",
            &params,
            Some("pb"),
        )
        .unwrap();

        // Scroll and Done are skipped; only the Click is kept.
        assert_eq!(pb.steps.len(), 1);
        assert_eq!(pb.steps[0].kind, StepKind::Click);
    }

    // ── save tests ────────────────────────────────────────────────────

    fn make_playbook(name: &str) -> Playbook {
        Playbook {
            name: name.into(),
            url_pattern: "example.com/login".into(),
            steps: vec![PlaybookStep {
                kind: StepKind::Click,
                selector: "Sign in".into(),
                value: None,
                fallbacks: vec![],
            }],
            params: vec![],
            success: None,
        }
    }

    #[test]
    fn save_creates_dir_and_file() {
        let tmp = TempDir::new().unwrap();
        let target_dir = tmp.path().join("nested").join("playbooks");
        assert!(!target_dir.exists());

        let pb = make_playbook("demo");
        let path = save(&pb, &target_dir).unwrap();

        assert!(target_dir.exists(), "target dir should be created");
        assert!(path.exists(), "playbook file should exist");
        assert_eq!(path.file_name().unwrap(), "demo.json");
    }

    #[test]
    fn save_atomic_overwrite() {
        let tmp = TempDir::new().unwrap();
        let mut pb = make_playbook("overwrite");

        let path1 = save(&pb, tmp.path()).unwrap();
        let first = std::fs::read_to_string(&path1).unwrap();

        pb.url_pattern = "different.com".into();
        let path2 = save(&pb, tmp.path()).unwrap();
        let second = std::fs::read_to_string(&path2).unwrap();

        assert_eq!(path1, path2);
        assert_ne!(first, second);
        assert!(second.contains("different.com"));
    }

    #[test]
    fn save_roundtrips_through_load() {
        let tmp = TempDir::new().unwrap();
        let pb = make_playbook("roundtrip");
        save(&pb, tmp.path()).unwrap();

        let loaded = load_playbooks(tmp.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "roundtrip");
        assert_eq!(loaded[0].steps.len(), 1);
    }
}
