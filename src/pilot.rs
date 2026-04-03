//! Browser pilot: observe -> heuristics -> LLM fallback -> act loop.
//!
//! Heuristics resolve ~70-90% of actions in 10ms. LLM only for ambiguity.

use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::heuristics;
use crate::semantic::{PageState, SemanticView};

/// A single action the pilot can take on the page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Click an interactive element by its `data-lad-id`.
    Click { element: u32, reasoning: String },
    /// Type text into an input/textarea by its `data-lad-id`.
    Type {
        element: u32,
        value: String,
        reasoning: String,
    },
    /// Select an option in a `<select>` element.
    Select {
        element: u32,
        value: String,
        reasoning: String,
    },
    /// Scroll the viewport in a given direction.
    Scroll {
        direction: String,
        reasoning: String,
    },
    /// Pause and wait for the page to settle.
    Wait { reasoning: String },
    /// Goal achieved -- includes the structured result.
    Done {
        result: serde_json::Value,
        reasoning: String,
    },
    /// Navigate to a different URL (multi-page flow support).
    Navigate { url: String, reasoning: String },
    /// Cannot proceed -- escalate to the caller.
    Escalate { reason: String },
}

/// How the action was resolved.
///
/// Variants are listed in 5-tier priority order (Tier 0 highest).
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    /// Tier 0: Resolved by replaying a trained playbook.
    Playbook,
    /// Tier 1: Resolved by explicit `data-lad` developer hints.
    Hints,
    /// Tier 2: Resolved by a deterministic heuristic rule.
    Heuristic,
    /// Tier 3: Resolved by the LLM backend.
    Llm,
}

/// A single step in the pilot's action history.
#[derive(Debug, Clone, Serialize)]
pub struct Step {
    /// Zero-based step index within the pilot run.
    pub index: u32,
    /// Semantic view observed at this step.
    pub observation: SemanticView,
    /// The action decided upon.
    pub action: Action,
    /// Whether a heuristic or the LLM produced the action.
    pub source: DecisionSource,
    /// Confidence score (0.0 .. 1.0).
    pub confidence: f32,
    /// Wall-clock duration of this step.
    pub duration: Duration,
}

/// LLM-agnostic backend for pilot decisions.
#[async_trait]
pub trait PilotBackend: Send + Sync {
    /// Given the current page state and history, choose the next action.
    async fn decide(
        &self,
        view: &SemanticView,
        goal: &str,
        history: &[Step],
    ) -> Result<Action, crate::Error>;
}

/// Configuration for a pilot run.
pub struct PilotConfig {
    /// Natural-language goal to accomplish.
    pub goal: String,
    /// Maximum number of steps before auto-escalation.
    pub max_steps: u32,
    /// Whether to check Tier 1 `@lad/hints` before other strategies (default: `true`).
    ///
    /// Hints are explicit developer annotations (`data-lad` attributes) and
    /// should almost always remain enabled — they are not guesses.
    pub use_hints: bool,
    /// Whether to try Tier 2 rule-based heuristics before the LLM (default: `true`).
    pub use_heuristics: bool,
    /// Directory containing `.json` playbook files for Tier 0 replay.
    ///
    /// When `Some`, playbooks are loaded at the start of the pilot run and
    /// checked before hints or heuristics on every step.
    pub playbook_dir: Option<std::path::PathBuf>,
    /// Maximum retries per step when an action fails (default: 2).
    pub max_retries_per_step: u32,
    /// Session state for multi-page tracking. When `Some`, cookies and navigation
    /// history are persisted across steps and can be carried between pilot runs.
    pub session: Option<std::sync::Arc<tokio::sync::Mutex<crate::session::SessionState>>>,
}

impl Default for PilotConfig {
    fn default() -> Self {
        Self {
            goal: String::new(),
            max_steps: 10,
            use_hints: true,
            use_heuristics: true,
            playbook_dir: None,
            max_retries_per_step: 2,
            session: None,
        }
    }
}

/// Result of a pilot run.
#[derive(Debug, Serialize)]
pub struct PilotResult {
    /// Whether the goal was achieved.
    pub success: bool,
    /// Complete step history.
    pub steps: Vec<Step>,
    /// The terminal action (Done or Escalate).
    pub final_action: Action,
    /// Total wall-clock duration of the run.
    pub total_duration: Duration,
    /// Number of steps resolved by playbook replay (Tier 0).
    pub playbook_hits: u32,
    /// Number of steps resolved by `@lad/hints` (Tier 1).
    pub hints_hits: u32,
    /// Number of steps resolved by heuristics (Tier 2).
    pub heuristic_hits: u32,
    /// Number of steps resolved by the LLM (Tier 3).
    pub llm_hits: u32,
    /// Total number of retries across all steps.
    pub retry_count: u32,
    /// Base64-encoded PNG screenshots taken during the run (e.g. on escalation).
    pub screenshots: Vec<String>,
    /// Session state at the end of the run (for multi-page carry-over).
    pub session_snapshot: Option<crate::session::SessionState>,
}

/// Capture a full-page screenshot as a base64-encoded PNG string.
///
/// Returns `None` if the screenshot fails (non-fatal; logs a warning).
pub async fn take_screenshot(page: &chromiumoxide::Page) -> Option<String> {
    match page
        .screenshot(
            chromiumoxide::page::ScreenshotParams::builder()
                .full_page(true)
                .build(),
        )
        .await
    {
        Ok(png_bytes) => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
            tracing::info!(bytes = png_bytes.len(), "screenshot captured");
            Some(b64)
        }
        Err(e) => {
            tracing::warn!(error = %e, "screenshot capture failed");
            None
        }
    }
}

/// Run the pilot loop: observe -> heuristics -> LLM fallback -> act -> repeat.
///
/// Includes retry logic:
/// - If `execute_action` fails, re-extracts the DOM and retries (stale DOM recovery).
/// - If heuristic returns `None` AND LLM returns an unparseable response, retries LLM once.
/// - If all retries fail on a step, logs the failure and continues to the next step.
pub async fn run_pilot(
    page: &chromiumoxide::Page,
    backend: &dyn PilotBackend,
    config: &PilotConfig,
) -> Result<PilotResult, crate::Error> {
    let run_start = Instant::now();

    // Tier 0: load playbooks from disk (empty vec when no dir configured).
    let playbooks = config
        .playbook_dir
        .as_deref()
        .map(crate::playbook::load_playbooks)
        .unwrap_or_default();
    if !playbooks.is_empty() {
        tracing::info!(count = playbooks.len(), "loaded playbooks");
    }

    // Session state: clone the Arc so we can lock it during the loop.
    let session = config.session.clone();

    let mut history: Vec<Step> = Vec::new();
    let mut acted_on: Vec<u32> = Vec::new();
    let mut playbook_hits: u32 = 0;
    let mut hints_hits: u32 = 0;
    let mut heuristic_hits: u32 = 0;
    let mut llm_hits: u32 = 0;
    let mut total_retries: u32 = 0;
    let mut screenshots: Vec<String> = Vec::new();
    let mut prev_url: Option<String> = None;
    let mut prev_element_count: Option<usize> = None;
    let mut stale_streak: u32 = 0;

    for step_idx in 0..config.max_steps {
        let step_start = Instant::now();

        // 1. Observe
        let view = crate::a11y::extract_semantic_view(page).await?;
        tracing::info!(
            step = step_idx,
            elements = view.elements.len(),
            tokens = view.estimated_tokens(),
            "observed"
        );

        // 1b. Stale-state detection: if the URL and element count are
        // unchanged for 2+ consecutive observations after an action,
        // the page is stuck (e.g. button re-click doing nothing).
        // NOTE: This naturally handles OAuth/redirect flows because a URL
        // change resets the streak — even if element count stays the same.
        let current_element_count = view.elements.len();
        let current_url = &view.url;
        if prev_url.as_deref() == Some(current_url.as_str())
            && prev_element_count == Some(current_element_count)
            && step_idx > 0
        {
            stale_streak += 1;
        } else {
            stale_streak = 0;
        }
        prev_url = Some(current_url.clone());
        prev_element_count = Some(current_element_count);

        if stale_streak >= 2 {
            tracing::warn!(
                step = step_idx,
                stale_streak,
                "page state unchanged for {} observations — escalating",
                stale_streak
            );
            let final_action = Action::Escalate {
                reason: format!(
                    "stale state: URL and element count unchanged for {} consecutive observations",
                    stale_streak
                ),
            };
            let step = Step {
                index: step_idx,
                observation: view,
                action: final_action.clone(),
                source: DecisionSource::Heuristic,
                confidence: 1.0,
                duration: step_start.elapsed(),
            };
            history.push(step);
            let session_snapshot = match &session {
                Some(s) => Some(s.lock().await.clone()),
                None => None,
            };
            return Ok(PilotResult {
                success: false,
                steps: history,
                final_action,
                total_duration: run_start.elapsed(),
                playbook_hits,
                hints_hits,
                heuristic_hits,
                llm_hits,
                retry_count: total_retries,
                screenshots,
                session_snapshot,
            });
        }

        // 1c. If the page is blocked (CAPTCHA / WAF), escalate immediately.
        if let PageState::Blocked(ref reason) = view.state {
            tracing::warn!(step = step_idx, reason = %reason, "page blocked — escalating");
            if let Some(b64) = take_screenshot(page).await {
                screenshots.push(b64);
            }
            let final_action = Action::Escalate {
                reason: format!("page blocked: {reason}"),
            };
            let step = Step {
                index: step_idx,
                observation: view,
                action: final_action.clone(),
                source: DecisionSource::Heuristic,
                confidence: 1.0,
                duration: step_start.elapsed(),
            };
            history.push(step);
            let session_snapshot = match &session {
                Some(s) => Some(s.lock().await.clone()),
                None => None,
            };
            return Ok(PilotResult {
                success: false,
                steps: history,
                final_action,
                total_duration: run_start.elapsed(),
                playbook_hits,
                hints_hits,
                heuristic_hits,
                llm_hits,
                retry_count: total_retries,
                screenshots,
                session_snapshot,
            });
        }

        // 1d. Enrich view with session context for multi-page LLM awareness.
        let mut view = view;
        if let Some(ref session_arc) = session {
            let sess = session_arc.lock().await;
            let ctx = crate::semantic::format_session_context(&sess);
            if !ctx.is_empty() {
                view.session_context = Some(ctx);
            }
        }

        // 2. Decide (playbook -> hints -> heuristics -> LLM fallback with retry)
        let (action, source, confidence) = decide_with_retry(
            &view,
            &config.goal,
            &acted_on,
            backend,
            &history,
            &playbooks,
            config.use_hints,
            config.use_heuristics,
            page,
            &mut total_retries,
            &mut screenshots,
        )
        .await?;

        let step_duration = step_start.elapsed();

        match source {
            DecisionSource::Playbook => playbook_hits += 1,
            DecisionSource::Hints => hints_hits += 1,
            DecisionSource::Heuristic => heuristic_hits += 1,
            DecisionSource::Llm => llm_hits += 1,
        }

        tracing::info!(
            step = step_idx,
            source = ?source,
            action = ?action,
            duration_ms = step_duration.as_millis() as u64,
            "decided"
        );

        if let Action::Type { element, .. } | Action::Click { element, .. } = &action {
            acted_on.push(*element);
        }

        let step = Step {
            index: step_idx,
            observation: view,
            action: action.clone(),
            source,
            confidence,
            duration: step_duration,
        };

        // 3. Check for terminal actions
        if matches!(&action, Action::Done { .. } | Action::Escalate { .. }) {
            let success = matches!(&action, Action::Done { .. });
            history.push(step);
            let session_snapshot = match &session {
                Some(s) => Some(s.lock().await.clone()),
                None => None,
            };
            return Ok(PilotResult {
                success,
                steps: history,
                final_action: action,
                total_duration: run_start.elapsed(),
                playbook_hits,
                hints_hits,
                heuristic_hits,
                llm_hits,
                retry_count: total_retries,
                screenshots,
                session_snapshot,
            });
        }

        // 4. Act with retry on failure
        if let Err(e) = execute_action_with_retry(
            page,
            &action,
            config.max_retries_per_step,
            &mut total_retries,
        )
        .await
        {
            tracing::warn!(
                step = step_idx,
                error = %e,
                "action failed after retries, continuing to next step"
            );
        }

        // 5. Session tracking: extract cookies and record navigation.
        if let Some(ref session_arc) = session {
            let mut sess = session_arc.lock().await;

            // Set origin URL on first step.
            if sess.origin_url.is_none() {
                sess.origin_url = Some(step.observation.url.clone());
            }

            // Extract cookies from the browser and merge into session.
            match crate::session::extract_cookies_cdp(page).await {
                Ok(new_cookies) => {
                    for cookie in new_cookies {
                        sess.add_cookie(cookie);
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "cookie extraction skipped");
                }
            }

            // Build action description for the navigation entry.
            let action_desc = match &action {
                Action::Click { reasoning, .. } => format!("click: {reasoning}"),
                Action::Type { reasoning, .. } => format!("type: {reasoning}"),
                Action::Select { reasoning, .. } => format!("select: {reasoning}"),
                Action::Scroll { reasoning, .. } => format!("scroll: {reasoning}"),
                Action::Wait { reasoning } => format!("wait: {reasoning}"),
                Action::Navigate { url, reasoning } => {
                    format!("navigate to {url}: {reasoning}")
                }
                _ => String::new(),
            };

            let form_submitted = matches!(&action, Action::Click { .. })
                && step
                    .observation
                    .elements
                    .iter()
                    .any(|e| e.kind == crate::semantic::ElementKind::Button);

            let auth_related = step.observation.page_hint.to_lowercase().contains("login")
                || step.observation.page_hint.to_lowercase().contains("auth")
                || step.observation.url.to_lowercase().contains("oauth");

            sess.record_navigation(
                step.observation.url.clone(),
                step.observation.title.clone(),
                if action_desc.is_empty() {
                    vec![]
                } else {
                    vec![action_desc]
                },
                form_submitted,
                auth_related,
            );

            // Auth state transitions.
            if auth_related && sess.auth_state == crate::session::AuthState::None {
                sess.auth_state = crate::session::AuthState::InProgress;
            }
            if sess.has_auth_cookies() && sess.auth_state == crate::session::AuthState::InProgress {
                sess.auth_state = crate::session::AuthState::Authenticated;
                tracing::info!("session: auth state -> Authenticated");
            }
        }

        history.push(step);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Max steps reached -- take a screenshot for escalation context.
    if let Some(b64) = take_screenshot(page).await {
        screenshots.push(b64);
    }

    let final_action = Action::Escalate {
        reason: format!("max steps ({}) reached", config.max_steps),
    };
    let session_snapshot = match &session {
        Some(s) => Some(s.lock().await.clone()),
        None => None,
    };
    Ok(PilotResult {
        success: false,
        steps: history,
        final_action,
        total_duration: run_start.elapsed(),
        playbook_hits,
        hints_hits,
        heuristic_hits,
        llm_hits,
        retry_count: total_retries,
        screenshots,
        session_snapshot,
    })
}

/// Decide the next action, retrying the LLM on parse failure with a fresh DOM.
///
/// When all retries are exhausted, captures a screenshot and embeds it as
/// base64 PNG in the `Escalate` reason for visual debugging context.
#[allow(clippy::too_many_arguments)]
async fn decide_with_retry(
    view: &crate::semantic::SemanticView,
    goal: &str,
    acted_on: &[u32],
    backend: &dyn PilotBackend,
    history: &[Step],
    playbooks: &[crate::playbook::Playbook],
    use_hints: bool,
    use_heuristics: bool,
    page: &chromiumoxide::Page,
    total_retries: &mut u32,
    screenshots: &mut Vec<String>,
) -> Result<(Action, DecisionSource, f32), crate::Error> {
    // Tier 0: Playbook replay — match URL and execute the next playbook step.
    if let Some(pb) = crate::playbook::find_playbook(playbooks, &view.url) {
        let params = crate::playbook::extract_params(goal, &pb.params);
        // Walk the playbook steps, skipping those whose selectors already acted on.
        for step in &pb.steps {
            if let Some(id) = crate::playbook::match_step_selector(view, step) {
                if acted_on.contains(&id) {
                    continue;
                }
                if let Some(action) = crate::playbook::step_to_action(view, step, &params) {
                    tracing::info!(
                        playbook = %pb.name,
                        selector = %step.selector,
                        "playbook step matched"
                    );
                    return Ok((action, DecisionSource::Playbook, 0.99));
                }
            }
        }
    }

    // Tier 1: Hints (@lad/hints data-lad attributes).
    if use_hints {
        let h = heuristics::hints::try_hints(view, goal, acted_on);
        if let Some(action) = h.action
            && h.confidence >= 0.9
        {
            tracing::info!(
                source = "hints",
                confidence = h.confidence,
                reason = %h.reason,
                "hint matched"
            );
            return Ok((action, DecisionSource::Hints, h.confidence));
        }
    }

    // Tier 2: Heuristics (rule-based).
    if use_heuristics {
        let h = heuristics::try_resolve(view, goal, acted_on);
        if let Some(action) = h.action {
            tracing::info!(
                source = "heuristic",
                confidence = h.confidence,
                reason = %h.reason,
                "heuristic matched"
            );
            return Ok((action, DecisionSource::Heuristic, h.confidence));
        }
    }

    // Tier 3: LLM fallback with one retry on parse failure.
    tracing::info!("tiers 0-2 miss — falling back to LLM");
    match backend.decide(view, goal, history).await {
        Ok(action) => Ok((action, DecisionSource::Llm, 0.5)),
        Err(e) => {
            tracing::warn!(error = %e, "LLM decision failed, retrying with fresh DOM");
            *total_retries += 1;

            // Re-extract DOM (stale DOM recovery) and retry
            if let Ok(fresh_view) = crate::a11y::extract_semantic_view(page).await {
                if let Ok(action) = backend.decide(&fresh_view, goal, history).await {
                    return Ok((action, DecisionSource::Llm, 0.4));
                }
                *total_retries += 1;
            }

            // All retries failed -- take a screenshot for escalation context.
            let mut reason = format!("LLM failed after retries: {e}");
            if let Some(b64) = take_screenshot(page).await {
                reason.push_str("\n[screenshot attached]");
                screenshots.push(b64);
            }

            Ok((Action::Escalate { reason }, DecisionSource::Llm, 0.0))
        }
    }
}

/// Execute an action with retry on failure (stale DOM recovery).
async fn execute_action_with_retry(
    page: &chromiumoxide::Page,
    action: &Action,
    max_retries: u32,
    total_retries: &mut u32,
) -> Result<(), crate::Error> {
    match execute_action(page, action).await {
        Ok(()) => Ok(()),
        Err(first_err) => {
            tracing::warn!(error = %first_err, "action execution failed, retrying");
            let mut last_err = first_err;

            for attempt in 1..=max_retries {
                *total_retries += 1;
                tracing::info!(attempt, max_retries, "retry: re-extracting DOM");
                tokio::time::sleep(Duration::from_millis(300)).await;

                match execute_action(page, action).await {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        tracing::warn!(attempt, error = %e, "retry failed");
                        last_err = e;
                    }
                }
            }

            Err(crate::Error::ActionFailed(format!(
                "action failed after {} retries: {}",
                max_retries, last_err
            )))
        }
    }
}

/// Escape a string for safe embedding inside a JavaScript single-quoted
/// string literal.
///
/// Handles all characters that could break out of the string context:
/// - Backslash, single quote, double quote, backtick (string delimiters)
/// - `$` (template literal injection via `${}`)
/// - Newline, carriage return (line terminator injection)
/// - Null byte (string truncation in some JS engines)
/// - `</` (prevents `</script>` tag breakout in HTML contexts)
pub fn js_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 8);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '`' => out.push_str("\\`"),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            '<' => {
                // Only escape `</` to prevent `</script>` breakout.
                // Peek-ahead is not needed: we always emit `<\/` for `<`
                // followed by `/` but we process char-by-char. Instead,
                // we escape every `<` that precedes a `/` — but since we
                // only see one char at a time, we use a simpler strategy:
                // always escape `<` as `\\u003c` would be overkill, so we
                // push `<` and let the next char handle `/`.
                out.push('<');
            }
            '/' => {
                // If the previous character was `<`, replace the pair with `<\/`.
                if out.ends_with('<') {
                    out.pop();
                    out.push_str("<\\/");
                } else {
                    out.push('/');
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Execute an action on the page via CDP.
async fn execute_action(page: &chromiumoxide::Page, action: &Action) -> Result<(), crate::Error> {
    match action {
        Action::Click { element, .. } => {
            let js = format!(
                r#"document.querySelector('[data-lad-id="{}"]')?.click()"#,
                element
            );
            page.evaluate(js).await?;
        }
        Action::Type { element, value, .. } => {
            let escaped = js_escape(value);
            let js = format!(
                r#"(() => {{
                    const el = document.querySelector('[data-lad-id="{}"]');
                    if (el) {{
                        el.focus();
                        el.value = '{escaped}';
                        el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                        el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                    }}
                }})()"#,
                element,
            );
            page.evaluate(js).await?;
        }
        Action::Select { element, value, .. } => {
            let escaped = js_escape(value);
            let js = format!(
                r#"(() => {{
                    const el = document.querySelector('[data-lad-id="{}"]');
                    if (el) {{ el.value = '{escaped}'; el.dispatchEvent(new Event('change', {{ bubbles: true }})); }}
                }})()"#,
                element,
            );
            page.evaluate(js).await?;
        }
        Action::Scroll { direction, .. } => {
            let (x, y) = match direction.as_str() {
                "up" => (0, -300),
                "down" => (0, 300),
                "left" => (-300, 0),
                "right" => (300, 0),
                _ => (0, 300),
            };
            let js = format!("window.scrollBy({x}, {y})");
            page.evaluate(js).await?;
        }
        Action::Wait { .. } => {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Action::Navigate { url, .. } => {
            page.goto(url).await?;
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
        Action::Done { .. } | Action::Escalate { .. } => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::js_escape;

    #[test]
    fn escapes_backslash() {
        assert_eq!(js_escape(r"a\b"), r"a\\b");
    }

    #[test]
    fn escapes_single_quote() {
        assert_eq!(js_escape("it's"), "it\\'s");
    }

    #[test]
    fn escapes_double_quote() {
        assert_eq!(js_escape(r#"say "hi""#), r#"say \"hi\""#);
    }

    #[test]
    fn escapes_backtick() {
        assert_eq!(js_escape("foo`bar"), "foo\\`bar");
    }

    #[test]
    fn escapes_dollar_sign() {
        assert_eq!(js_escape("${alert(1)}"), "\\${alert(1)}");
    }

    #[test]
    fn escapes_template_literal_injection() {
        // Full template literal attack: `${...}`
        assert_eq!(
            js_escape("`${document.cookie}`"),
            "\\`\\${document.cookie}\\`"
        );
    }

    #[test]
    fn escapes_newline_and_carriage_return() {
        assert_eq!(js_escape("line1\nline2\rline3"), "line1\\nline2\\rline3");
    }

    #[test]
    fn escapes_null_byte() {
        assert_eq!(js_escape("before\0after"), "before\\0after");
    }

    #[test]
    fn escapes_script_tag_breakout() {
        assert_eq!(js_escape("</script>"), "<\\/script>");
    }

    #[test]
    fn escapes_script_tag_case_variants() {
        // `</SCRIPT>` should also be escaped (same `</` prefix)
        assert_eq!(js_escape("</SCRIPT>"), "<\\/SCRIPT>");
    }

    #[test]
    fn preserves_safe_slash() {
        // A `/` not preceded by `<` should pass through.
        assert_eq!(js_escape("a/b"), "a/b");
    }

    #[test]
    fn preserves_safe_angle_bracket() {
        // A `<` not followed by `/` should pass through.
        assert_eq!(js_escape("<div>"), "<div>");
    }

    #[test]
    fn combined_adversarial_input() {
        let input = "'; alert(1); //";
        let escaped = js_escape(input);
        assert_eq!(escaped, "\\'; alert(1); //");
        // The escaped value, when placed in `'...'`, yields:
        //   '\'; alert(1); //'
        // which is a valid string literal, not an injection.
    }

    #[test]
    fn xss_via_type_value() {
        // Simulates what an LLM might produce as a Type action value.
        let input = "test' + alert(1) + '";
        let escaped = js_escape(input);
        assert_eq!(escaped, "test\\' + alert(1) + \\'");
    }

    #[test]
    fn empty_string() {
        assert_eq!(js_escape(""), "");
    }

    #[test]
    fn plain_text_unchanged() {
        assert_eq!(js_escape("hello world 123"), "hello world 123");
    }

    #[test]
    fn multiple_consecutive_escapes() {
        assert_eq!(js_escape("\\\\''"), "\\\\\\\\\\'\\'");
    }

    #[test]
    fn script_close_mid_string() {
        assert_eq!(js_escape("foo</script>bar"), "foo<\\/script>bar");
    }
}
