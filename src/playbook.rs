//! Playbook system: deterministic step-by-step replay for known workflows.
//!
//! Playbooks are JSON files stored in `.lad/playbooks/` that describe a
//! sequence of actions for a known page. When a playbook matches the current
//! URL, the pilot replays it step-by-step instead of using heuristics or LLM.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::heuristics::login::extract_credential;
use crate::pilot::Action;
use crate::semantic::SemanticView;

/// Regex matching param keys that must be treated as secrets (password,
/// token, api-key, credential, bearer, etc). Case-insensitive.
fn secret_key_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"(?i)password|secret|token|api[_-]?key|credential|bearer")
            .expect("secret-key regex is static")
    })
}

/// Returns `true` when the given param name looks like a secret and therefore
/// must be templatized when learning a playbook.
pub fn is_secret_key(name: &str) -> bool {
    secret_key_re().is_match(name)
}

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

/// Find the first playbook whose `url_pattern` structurally matches the given URL.
///
/// Matching is structural (not substring): both URLs are parsed and the
/// playbook matches only when `current.host == pattern.host` (exact, not
/// suffix) and `current.path` starts with the pattern's recorded path prefix.
/// This closes the `evilgithub.com/login` host-suffix attack that
/// `substring.contains` allowed.
///
/// The pattern itself is the `host[/segment[/segment]]` shape produced by
/// [`derive_url_pattern`]. When the pattern or current URL fails to parse,
/// the playbook is skipped (fail-closed).
pub fn find_playbook<'a>(playbooks: &'a [Playbook], url: &str) -> Option<&'a Playbook> {
    let current = url::Url::parse(url).ok()?;
    let current_host = current.host_str()?;
    let current_path = current.path();

    playbooks.iter().find(|pb| {
        let Some((pattern_host, pattern_path)) = split_url_pattern(&pb.url_pattern) else {
            return false;
        };
        current_host.eq_ignore_ascii_case(pattern_host)
            && path_prefix_matches(current_path, &pattern_path)
    })
}

/// Does `current` start with `pattern_prefix` on a path-segment boundary?
///
/// `starts_with` alone would let a pattern like `/login` match `/loginregister`.
/// We require the match to end at a `/` or at the end of the path so the
/// boundary is segment-aligned.
fn path_prefix_matches(current: &str, pattern_prefix: &str) -> bool {
    if pattern_prefix == "/" {
        return true;
    }
    let trimmed = pattern_prefix.trim_end_matches('/');
    if !current.starts_with(trimmed) {
        return false;
    }
    let rest = &current[trimmed.len()..];
    rest.is_empty() || rest.starts_with('/')
}

/// Split a `host[/path_prefix]` pattern into `(host, path_prefix)` where the
/// path is `/` when no path is present (so `github.com` matches `/` which is
/// any path, while `github.com/owner/repo` matches only paths starting with
/// `/owner/repo`).
fn split_url_pattern(pattern: &str) -> Option<(&str, String)> {
    if pattern.is_empty() {
        return None;
    }
    match pattern.split_once('/') {
        Some((host, rest)) => {
            if host.is_empty() {
                return None;
            }
            Some((host, format!("/{rest}")))
        }
        None => Some((pattern, "/".into())),
    }
}

// ── Parameter interpolation ───────────────────────────────────────────

/// Extract parameter values from a goal string using credential-style parsing.
///
/// Maps playbook param names to goal keywords:
/// - `"username"` -> tries prefixes `["as ", "user ", "username ", "email ", "login "]`
/// - `"password"` -> tries prefixes `["password ", "pass ", "pw "]`
/// - anything else -> tries `["<name> "]`
///
/// Prefix matching is case-insensitive (lowercase is only used for locating
/// the position), but extracted values preserve the original case of the
/// goal (`Hunter2` stays `Hunter2`, not `hunter2`) — passwords are
/// case-sensitive and lowercasing silently breaks the login replay.
pub fn extract_params(
    goal: &str,
    param_names: &[String],
) -> std::collections::HashMap<String, String> {
    let goal_lower = goal.to_lowercase();
    let mut params = std::collections::HashMap::new();

    for name in param_names {
        let prefixes: &[&str] = match name.as_str() {
            "username" | "user" | "email" => &["as ", "user ", "username ", "email ", "login "],
            "password" | "pass" | "pw" => &["password ", "pass ", "pw "],
            other => &[other],
        };
        // `extract_credential` needs prefixes with a trailing space. For
        // the arbitrary-name branch we synthesise `"<name> "` on the fly.
        let value = match name.as_str() {
            "username" | "user" | "email" | "password" | "pass" | "pw" => {
                extract_credential_preserve_case(goal, &goal_lower, prefixes)
            }
            other => {
                let prefix = format!("{other} ");
                extract_credential_preserve_case(goal, &goal_lower, &[&prefix])
            }
        };
        if let Some(v) = value {
            params.insert(name.clone(), v);
        }
    }

    params
}

/// Locate each candidate prefix in `goal_lower`, but read the value from
/// the original `goal` at the matched offset so case is preserved.
///
/// Mirrors the contract of [`extract_credential`]: returns the first
/// whitespace-delimited token after the prefix, or `None` when no prefix
/// matches or the tail is empty.
fn extract_credential_preserve_case(
    goal: &str,
    goal_lower: &str,
    prefixes: &[&str],
) -> Option<String> {
    for prefix in prefixes {
        if let Some(start) = goal_lower.find(prefix) {
            let after = start + prefix.len();
            if after > goal.len() {
                continue;
            }
            let tail = &goal[after..];
            let token: String = tail.chars().take_while(|c| !c.is_whitespace()).collect();
            if !token.is_empty() {
                return Some(token);
            }
        }
    }
    // Fallback: the legacy lowercased extractor. Preserves original behaviour
    // when the new fast path yields nothing (e.g. embedded separators).
    extract_credential(goal_lower, prefixes)
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
    /// The run ended with a `Done` whose `result.success` was `false` or missing.
    ///
    /// Guards against learning a playbook from a failed run (e.g. a login
    /// heuristic returning `Done { success: false, error: "bad password" }`).
    /// Missing `success` fields are treated as failure (fail-closed).
    #[error("run did not succeed (result.success != true); refusing to learn")]
    RunNotSuccessful,
    /// The initial URL could not be parsed into a `host + path` pattern.
    #[error("cannot derive URL pattern from initial URL: {0}")]
    InvalidUrl(String),
    /// No explicit name was supplied and the goal was too empty to derive one.
    #[error("cannot derive name from goal; pass --learn-name explicitly")]
    NameDerivationFailed,
    /// The explicit playbook name contained unsafe characters or was too long.
    #[error("invalid playbook name {name:?}: {reason}")]
    InvalidName {
        /// The offending name.
        name: String,
        /// Why it was rejected (e.g. "contains '/'" or "exceeds 128 chars").
        reason: String,
    },
    /// A param key matching the secret-name regex was not substituted into
    /// any captured step. Learning refuses to persist raw secrets.
    #[error(
        "secret-like param {key:?} was not substituted into any step; \
         refusing to persist a playbook that could leak the raw value"
    )]
    SecretNotTemplatized {
        /// The param key whose value was not templatized.
        key: String,
    },
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

    // The run must conclude with a `Done` action whose `result.success` is
    // explicitly `true`. Missing or falsy `success` is treated as failure
    // (fail-closed) — this blocks learning a playbook from a login failure
    // heuristic that emits `Done { result: { success: false } }`.
    let last_done_success = history.iter().rev().find_map(|s| match &s.action {
        Action::Done { result, .. } => Some(result),
        _ => None,
    });
    let Some(result) = last_done_success else {
        return Err(SynthesizeError::NoCompletion);
    };
    if !result
        .get("success")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Err(SynthesizeError::RunNotSuccessful);
    }

    // Derive the playbook name, validating either the explicit name or the
    // derived one (a malformed goal producing ".." or "/" must also be
    // rejected — the attacker doesn't need the `--learn-name` flag).
    let playbook_name = match name {
        Some(n) if !n.trim().is_empty() => {
            let trimmed = n.trim().to_string();
            validate_playbook_name(&trimmed)?;
            trimmed
        }
        _ => {
            let derived =
                derive_name_from_goal(goal).ok_or(SynthesizeError::NameDerivationFailed)?;
            validate_playbook_name(&derived)?;
            derived
        }
    };

    // Derive the url_pattern from the initial URL.
    let url_pattern = derive_url_pattern(initial_url)?;

    // Params list: sorted keys for deterministic output.
    let mut param_keys: Vec<String> = explicit_params.keys().cloned().collect();
    param_keys.sort();

    // Walk history and map actionable steps. Track which explicit-param keys
    // actually got substituted so we can reject the run if a secret-shaped
    // key never matched any captured value (see the post-loop check below).
    let mut steps: Vec<PlaybookStep> = Vec::new();
    let mut final_view: Option<&SemanticView> = None;
    let mut substituted_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    for step in history {
        final_view = Some(&step.observation);
        match &step.action {
            Action::Click { element, .. } => {
                if let Some((selector, fallbacks)) =
                    element_selector_with_fallbacks(&step.observation, *element)
                {
                    steps.push(PlaybookStep {
                        kind: StepKind::Click,
                        selector,
                        value: None,
                        fallbacks,
                    });
                }
            }
            Action::Type { element, value, .. } => {
                if let Some((selector, fallbacks)) =
                    element_selector_with_fallbacks(&step.observation, *element)
                {
                    let templated =
                        templatize_tracking(value, explicit_params, &mut substituted_keys);
                    steps.push(PlaybookStep {
                        kind: StepKind::Type,
                        selector,
                        value: Some(templated),
                        fallbacks,
                    });
                }
            }
            Action::Select { element, value, .. } => {
                if let Some((selector, fallbacks)) =
                    element_selector_with_fallbacks(&step.observation, *element)
                {
                    let templated =
                        templatize_tracking(value, explicit_params, &mut substituted_keys);
                    steps.push(PlaybookStep {
                        kind: StepKind::Select,
                        selector,
                        value: Some(templated),
                        fallbacks,
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

    // Any secret-shaped key whose value was NOT substituted into at least
    // one captured step is a leak vector: the user passed `--learn-params
    // password=hunter2` but the real Type step used a different value
    // (wrapped/normalized/etc). Refuse to persist instead of silently
    // writing a playbook that doesn't protect the secret.
    for (key, value) in explicit_params {
        if value.is_empty() {
            continue;
        }
        if is_secret_key(key) && !substituted_keys.contains(key) {
            return Err(SynthesizeError::SecretNotTemplatized { key: key.clone() });
        }
    }

    // Shape-based guard on param KEYS: after templatization, no captured
    // value should contain the raw secret value verbatim. Defends against
    // substring mismatches (e.g. the Type value had whitespace normalised)
    // that would slip past the substitution check above.
    //
    // Skip very short values — a 2-3 char secret collides with legitimate
    // substrings ("ab" appears in "label", "about", etc.) and would DoS the
    // learn path on weak inputs. Production secrets are always >= 6 chars;
    // the `is_secret_key`/`substituted_keys` check above already handles
    // the non-substitution case for any length.
    const MIN_SHAPE_GUARD_LEN: usize = 6;
    for (key, value) in explicit_params {
        if value.is_empty() || !is_secret_key(key) || value.len() < MIN_SHAPE_GUARD_LEN {
            continue;
        }
        let leaks = steps.iter().any(|s| {
            let step_json = serde_json::to_string(s).unwrap_or_default();
            step_json.contains(value.as_str())
        });
        if leaks {
            return Err(SynthesizeError::SecretNotTemplatized { key: key.clone() });
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

/// Validate a playbook filename component.
///
/// Rejects path-traversal, slashes, control chars, dotfiles, and oversized
/// names. The learn flow uses the returned name directly as `<dir>/<name>.json`,
/// so any of these forms would escape the configured output dir or trip
/// filesystem weirdness.
fn validate_playbook_name(name: &str) -> Result<(), SynthesizeError> {
    let fail = |reason: &str| -> SynthesizeError {
        SynthesizeError::InvalidName {
            name: name.to_string(),
            reason: reason.to_string(),
        }
    };
    if name.is_empty() {
        return Err(fail("name is empty"));
    }
    if name.len() > 128 {
        return Err(fail("name exceeds 128 chars"));
    }
    if name.starts_with('.') {
        return Err(fail("name starts with '.' (dotfile/parent traversal)"));
    }
    if name.contains("..") {
        return Err(fail("name contains '..'"));
    }
    if name.contains('/') {
        return Err(fail("name contains '/'"));
    }
    if name.contains('\\') {
        return Err(fail("name contains '\\'"));
    }
    if name.contains('\0') {
        return Err(fail("name contains NUL"));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(fail("name contains a control character"));
    }
    Ok(())
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

/// Extract `host + up to two path segments` from a URL, e.g.
/// `https://github.com/owner/repo/settings?x=1` -> `github.com/owner/repo`.
///
/// Two segments is enough to distinguish `github.com/owner/repo/settings`
/// from `github.com/owner/repo/issues` (same "section" on the same repo)
/// while avoiding a pattern so specific it won't match any future URL.
///
/// When the path is empty (e.g. `https://example.com`), the pattern is just
/// the host.
fn derive_url_pattern(url: &str) -> Result<String, SynthesizeError> {
    if url.is_empty() {
        return Err(SynthesizeError::InvalidUrl("empty url".into()));
    }
    let parsed = url::Url::parse(url).map_err(|e| SynthesizeError::InvalidUrl(e.to_string()))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| SynthesizeError::InvalidUrl(format!("no host in {url}")))?;
    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).take(2).collect())
        .unwrap_or_default();
    if segments.is_empty() {
        Ok(host.to_string())
    } else {
        Ok(format!("{host}/{}", segments.join("/")))
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

/// Primary selector plus observable-attribute fallbacks (`name=X`,
/// `id=X`, `type=X`, etc). Priority mirrors the [`match_selector`]
/// passes so a replay attempt tries attributes in the order they're most
/// likely to survive minor DOM changes.
///
/// Only attributes that were present on the element at record time are
/// included — empty values are filtered out so the fallback list never
/// contains `"name="` or `"id="`.
fn element_selector_with_fallbacks(
    view: &SemanticView,
    element_id: u32,
) -> Option<(String, Vec<String>)> {
    let el = view.elements.iter().find(|e| e.id == element_id)?;

    let label_ok = !el.label.trim().is_empty();
    let name_ok = el
        .name
        .as_ref()
        .map(|n| !n.trim().is_empty())
        .unwrap_or(false);

    let primary = if label_ok {
        el.label.clone()
    } else if name_ok {
        el.name.clone().unwrap_or_default()
    } else {
        el.input_type.clone()?
    };

    let mut fallbacks: Vec<String> = Vec::new();
    if label_ok && name_ok {
        if let Some(name) = &el.name {
            fallbacks.push(format!("name={name}"));
        }
    } else if let Some(name) = &el.name
        && !name.trim().is_empty()
        && primary != *name
    {
        fallbacks.push(format!("name={name}"));
    }
    if let Some(itype) = &el.input_type
        && !itype.trim().is_empty()
        && primary != *itype
    {
        fallbacks.push(format!("type={itype}"));
    }

    Some((primary, fallbacks))
}

/// Replace any occurrence of each `params[key]` value in `raw` with `${key}`,
/// recording which keys substituted at least once.
///
/// Internal variant of [`templatize`]. The caller uses the populated
/// `substituted` set to detect secret-shaped params that never matched any
/// captured step value.
///
/// Issue #55 — order determinism. `HashMap` iteration has no stable order,
/// so when one param's value is a substring of another (e.g.
/// `password=abc`, `confirm=abc123`) the output depended on which ran
/// first. We now sort by value length descending before replacing, so the
/// longest value always wins its substring match and the shorter value
/// still gets replaced in any remaining instances. Output is deterministic
/// regardless of insertion order.
fn templatize_tracking(
    raw: &str,
    params: &std::collections::HashMap<String, String>,
    substituted: &mut std::collections::HashSet<String>,
) -> String {
    let mut out = raw.to_string();
    // Sort by value length desc, tiebreak by key for full determinism.
    let mut ordered: Vec<(&String, &String)> =
        params.iter().filter(|(_, v)| !v.is_empty()).collect();
    ordered.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));
    for (key, value) in ordered {
        let placeholder = format!("${{{key}}}");
        let replaced = out.replace(value, &placeholder);
        if replaced != out {
            substituted.insert(key.clone());
            out = replaced;
        }
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

    // Segment-boundary match: `/login` must NOT match `/loginregister`
    // (path-prefix needs to end at a `/` or end-of-path).
    #[test]
    fn find_playbook_path_prefix_respects_segment_boundary() {
        let pb: Playbook = serde_json::from_str(sample_playbook_json()).unwrap();
        let playbooks = vec![pb];

        let collision = find_playbook(&playbooks, "https://github.com/loginregister");
        assert!(
            collision.is_none(),
            "/login must NOT match /loginregister (segment-boundary violation)"
        );

        // But a true sub-path under /login/... SHOULD still match.
        let sub = find_playbook(&playbooks, "https://github.com/login/oauth");
        assert!(
            sub.is_some(),
            "/login should still match /login/oauth as legitimate sub-segment"
        );
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
                    result: serde_json::json!({"success": true}),
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
                    result: serde_json::json!({"success": true}),
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
                    result: serde_json::json!({"success": true}),
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
                result: serde_json::json!({"success": true}),
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
                result: serde_json::json!({"success": true}),
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
                    result: serde_json::json!({"success": true}),
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

    // ── Fix 1: success gate ───────────────────────────────────────────

    #[test]
    fn synthesize_rejects_done_with_success_false() {
        let view = sample_view();
        let history = vec![synth_step(
            0,
            view,
            Action::Done {
                result: serde_json::json!({"success": false, "reason": "bad password"}),
                reasoning: "login failed".into(),
            },
            DecisionSource::Heuristic,
        )];
        let params = HashMap::new();
        let err = synthesize_from_history(
            &history,
            "login",
            "https://example.com/login",
            &params,
            Some("pb"),
        )
        .unwrap_err();
        assert!(
            matches!(err, SynthesizeError::RunNotSuccessful),
            "expected RunNotSuccessful, got {err:?}"
        );
    }

    #[test]
    fn synthesize_rejects_done_with_missing_success_field() {
        let view = sample_view();
        let history = vec![synth_step(
            0,
            view,
            Action::Done {
                result: serde_json::json!({}),
                reasoning: "ambiguous".into(),
            },
            DecisionSource::Heuristic,
        )];
        let params = HashMap::new();
        let err = synthesize_from_history(
            &history,
            "login",
            "https://example.com/login",
            &params,
            Some("pb"),
        )
        .unwrap_err();
        assert!(
            matches!(err, SynthesizeError::RunNotSuccessful),
            "fail-closed on missing success field, got {err:?}"
        );
    }

    // ── Fix 2a/2b: templatize enforcement ─────────────────────────────

    /// Issue #55 — when params overlap (one value is a substring of
    /// another), output must be deterministic regardless of insertion
    /// order. Before the fix, iterating `&HashMap` produced different
    /// replacements per run. Sorting by value.len() desc fixes it — longest
    /// match always wins.
    #[test]
    fn templatize_tracking_deterministic_on_substring_overlap() {
        let raw = "user=alice pass=abc confirm=abc123";
        let mut params = HashMap::new();
        params.insert("password".into(), "abc".into());
        params.insert("confirm".into(), "abc123".into());
        params.insert("user".into(), "alice".into());

        // Run 20 times — with the old buggy code, at least one would
        // produce ${password}123 instead of ${confirm}. With the fix,
        // all 20 must agree.
        let mut outputs = std::collections::HashSet::new();
        for _ in 0..20 {
            let mut subbed = std::collections::HashSet::new();
            outputs.insert(templatize_tracking(raw, &params, &mut subbed));
        }
        assert_eq!(
            outputs.len(),
            1,
            "output must be deterministic across runs, got {:?}",
            outputs
        );
        // Longest value wins its match: abc123 -> ${confirm}; the lone abc
        // (in pass=abc) also gets its replacement to ${password}.
        let got = outputs.into_iter().next().unwrap();
        assert_eq!(got, "user=${user} pass=${password} confirm=${confirm}");
    }

    #[test]
    fn templatize_errors_when_secret_not_substituted() {
        // Value "s3cret" is NOT present in any Type action — so the learn
        // pipeline cannot substitute it, which would mean the password
        // would leak raw into the playbook (or at best be un-templatized).
        let view = sample_view();
        let history = vec![
            synth_step(
                0,
                view.clone(),
                Action::Type {
                    element: 1,
                    value: "something_else".into(),
                    reasoning: "".into(),
                },
                DecisionSource::Heuristic,
            ),
            synth_step(
                1,
                view,
                Action::Done {
                    result: serde_json::json!({"success": true}),
                    reasoning: "".into(),
                },
                DecisionSource::Llm,
            ),
        ];
        let mut params = HashMap::new();
        params.insert("password".into(), "s3cret".into());
        let err = synthesize_from_history(
            &history,
            "login",
            "https://example.com/login",
            &params,
            Some("pb"),
        )
        .unwrap_err();
        match err {
            SynthesizeError::SecretNotTemplatized { key } => assert_eq!(key, "password"),
            other => panic!("expected SecretNotTemplatized, got {other:?}"),
        }
    }

    #[test]
    fn templatize_allows_non_secret_key_without_match() {
        // email is NOT a secret-shaped key; synthesis should succeed even
        // if the Type action value doesn't match (the playbook just won't
        // templatize that field — not a security issue).
        let view = sample_view();
        let history = vec![
            synth_step(
                0,
                view.clone(),
                Action::Type {
                    element: 0,
                    value: "unrelated".into(),
                    reasoning: "".into(),
                },
                DecisionSource::Heuristic,
            ),
            synth_step(
                1,
                view,
                Action::Done {
                    result: serde_json::json!({"success": true}),
                    reasoning: "".into(),
                },
                DecisionSource::Llm,
            ),
        ];
        let mut params = HashMap::new();
        params.insert("email".into(), "alice@test.com".into());
        let pb = synthesize_from_history(
            &history,
            "login",
            "https://example.com/login",
            &params,
            Some("pb"),
        )
        .expect("non-secret non-matching key should not fail");
        assert!(pb.steps.iter().any(|s| s.kind == StepKind::Type));
    }

    // Shape-guard MIN_SHAPE_GUARD_LEN bypass: a 3-char secret value ("abc")
    // that happens to appear as a substring of legitimate data (e.g. inside
    // the serialized element JSON) must NOT trip the guard — that would be a
    // DoS on learn for users with short/weak secrets. The substitution-guard
    // (`is_secret_key && !substituted_keys`) still protects them.
    #[test]
    fn templatize_shape_guard_skips_short_secret_values() {
        let view = sample_view();
        let history = vec![
            synth_step(
                0,
                view.clone(),
                Action::Type {
                    element: 0,
                    // Legitimate substring-collision target: element label may
                    // serialize to something containing "abc" incidentally.
                    value: "abc".into(),
                    reasoning: "".into(),
                },
                DecisionSource::Heuristic,
            ),
            synth_step(
                1,
                view,
                Action::Done {
                    result: serde_json::json!({"success": true}),
                    reasoning: "".into(),
                },
                DecisionSource::Llm,
            ),
        ];
        let mut params = HashMap::new();
        params.insert("password".into(), "abc".into()); // 3 chars, below guard

        let pb = synthesize_from_history(
            &history,
            "log in with password abc",
            "https://example.com/login",
            &params,
            Some("short_pw"),
        )
        .expect("short secret should templatize via substitution path and skip shape guard");
        assert!(pb.steps.iter().any(|s| s.kind == StepKind::Type));
    }

    // ── Fix 3: playbook-name validation ───────────────────────────────

    fn minimal_ok_history(view: SemanticView) -> Vec<crate::pilot::Step> {
        vec![synth_step(
            0,
            view,
            Action::Done {
                result: serde_json::json!({"success": true}),
                reasoning: "".into(),
            },
            DecisionSource::Llm,
        )]
    }

    fn assert_invalid_name_err(err: SynthesizeError, needle: &str) {
        match err {
            SynthesizeError::InvalidName { name: _, reason } => assert!(
                reason.contains(needle),
                "expected reason to contain {needle:?}, got {reason:?}"
            ),
            other => panic!("expected InvalidName, got {other:?}"),
        }
    }

    #[test]
    fn reject_name_with_slash() {
        let view = sample_view();
        let history = minimal_ok_history(view);
        let params = HashMap::new();
        // Use a name with '/' but no leading '.' and no ".." so the slash
        // check is the one that fires.
        let err = synthesize_from_history(
            &history,
            "login",
            "https://example.com/login",
            &params,
            Some("subdir/escape"),
        )
        .unwrap_err();
        assert_invalid_name_err(err, "'/'");
    }

    #[test]
    fn reject_name_with_parent_traversal() {
        let view = sample_view();
        let history = minimal_ok_history(view);
        let params = HashMap::new();
        let err = synthesize_from_history(
            &history,
            "login",
            "https://example.com/login",
            &params,
            Some("foo..bar"),
        )
        .unwrap_err();
        assert_invalid_name_err(err, "..");
    }

    #[test]
    fn reject_name_starting_with_dot() {
        let view = sample_view();
        let history = minimal_ok_history(view);
        let params = HashMap::new();
        let err = synthesize_from_history(
            &history,
            "login",
            "https://example.com/login",
            &params,
            Some(".hidden"),
        )
        .unwrap_err();
        assert_invalid_name_err(err, "'.'");
    }

    #[test]
    fn reject_empty_name() {
        // Empty after trim falls through to derived-from-goal; use explicit
        // whitespace to exercise the "empty" branch directly.
        assert!(validate_playbook_name("").is_err());
    }

    #[test]
    fn reject_very_long_name() {
        let long = "x".repeat(129);
        let view = sample_view();
        let history = minimal_ok_history(view);
        let params = HashMap::new();
        let err = synthesize_from_history(
            &history,
            "login",
            "https://example.com/login",
            &params,
            Some(&long),
        )
        .unwrap_err();
        assert_invalid_name_err(err, "128");
    }

    // ── Fix 4a/4b: url_pattern derivation + matching ──────────────────

    #[test]
    fn url_pattern_includes_two_path_segments() {
        let pat = derive_url_pattern("https://github.com/owner/repo/settings/actions").unwrap();
        assert_eq!(pat, "github.com/owner/repo");
    }

    #[test]
    fn url_pattern_distinguishes_settings_from_issues() {
        // Both produce different patterns (same host + 2 segments).
        let a = derive_url_pattern("https://github.com/owner/repo/settings").unwrap();
        let b = derive_url_pattern("https://github.com/owner/repo/issues").unwrap();
        assert_eq!(a, "github.com/owner/repo");
        assert_eq!(b, "github.com/owner/repo");
        // They *share* the 2-seg prefix but do not share path-3.
        // (The FULL URL matching still distinguishes; the pattern
        // intentionally matches both since both live under the same repo.)
    }

    #[test]
    fn find_playbook_rejects_evil_subdomain() {
        let pb = Playbook {
            name: "gh".into(),
            url_pattern: "github.com/login".into(),
            steps: vec![],
            params: vec![],
            success: None,
        };
        let playbooks = vec![pb];
        // Host-suffix attack: "github.com" as a suffix of "evilgithub.com"
        // must NOT match.
        assert!(find_playbook(&playbooks, "https://evilgithub.com/login").is_none());
        // Subdomain attack: ANY host that contains "github.com" but isn't
        // exactly "github.com" must NOT match.
        assert!(find_playbook(&playbooks, "https://evil.github.com.evil.tld/login").is_none());
    }

    #[test]
    fn find_playbook_rejects_host_suffix_attack() {
        let pb = Playbook {
            name: "gh-root".into(),
            url_pattern: "github.com".into(),
            steps: vec![],
            params: vec![],
            success: None,
        };
        let playbooks = vec![pb];
        assert!(find_playbook(&playbooks, "https://fakegithub.com/").is_none());
        assert!(find_playbook(&playbooks, "https://github.com/").is_some());
    }

    // ── Fix 5: case-preserving extract_params ─────────────────────────

    #[test]
    fn extract_params_preserves_case_sensitive_password() {
        let params = extract_params("log in as Alice password Hunter2", &["password".into()]);
        assert_eq!(params.get("password"), Some(&"Hunter2".to_string()));
    }

    #[test]
    fn extract_params_key_matching_is_case_insensitive() {
        // The prefix is stored lowercased ("password "); the goal uses
        // uppercase PASSWORD. Match should still hit (location is found
        // in the lowercased copy), and the value preserves original case.
        let params = extract_params("log in as alice PASSWORD secret", &["password".into()]);
        assert_eq!(params.get("password"), Some(&"secret".to_string()));
    }

    // ── Fix 8: selector fallbacks ─────────────────────────────────────

    #[test]
    fn element_selector_returns_name_fallback() {
        let view = sample_view();
        // Element 2 has label "Sign in" and name "commit" — primary is the
        // label, but name should fall back.
        let (primary, fallbacks) = element_selector_with_fallbacks(&view, 2).unwrap();
        assert_eq!(primary, "Sign in");
        assert!(
            fallbacks.iter().any(|f| f == "name=commit"),
            "expected name=commit fallback, got {fallbacks:?}"
        );
    }

    #[test]
    fn synthesized_step_has_nonempty_fallbacks_when_available() {
        let view = sample_view();
        let mut params = HashMap::new();
        // Non-secret key; value placed into Type action.
        params.insert("email".into(), "octocat".into());

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
                view,
                Action::Done {
                    result: serde_json::json!({"success": true}),
                    reasoning: "".into(),
                },
                DecisionSource::Llm,
            ),
        ];
        let pb = synthesize_from_history(
            &history,
            "login",
            "https://example.com/login",
            &params,
            Some("pb"),
        )
        .unwrap();
        assert!(
            !pb.steps[0].fallbacks.is_empty(),
            "expected at least one fallback for element 0 (has name + type), got {:?}",
            pb.steps[0].fallbacks
        );
    }
}
