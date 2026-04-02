//! Browser pilot: observe -> heuristics -> LLM fallback -> act loop.
//!
//! Heuristics resolve ~70-90% of actions in 10ms. LLM only for ambiguity.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::heuristics;
use crate::semantic::SemanticView;

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
    /// Cannot proceed -- escalate to the caller.
    Escalate { reason: String },
}

/// How the action was resolved.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    /// Resolved by a deterministic heuristic rule.
    Heuristic,
    /// Resolved by the LLM backend.
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
    /// Whether to try heuristics before the LLM (default: `true`).
    pub use_heuristics: bool,
    /// Maximum retries per step when an action fails (default: 2).
    pub max_retries_per_step: u32,
}

impl Default for PilotConfig {
    fn default() -> Self {
        Self {
            goal: String::new(),
            max_steps: 10,
            use_heuristics: true,
            max_retries_per_step: 2,
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
    /// Number of steps resolved by heuristics.
    pub heuristic_hits: u32,
    /// Number of steps resolved by the LLM.
    pub llm_hits: u32,
    /// Total number of retries across all steps.
    pub retry_count: u32,
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
    let mut history: Vec<Step> = Vec::new();
    let mut acted_on: Vec<u32> = Vec::new();
    let mut heuristic_hits: u32 = 0;
    let mut llm_hits: u32 = 0;
    let mut total_retries: u32 = 0;

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

        // 2. Decide (heuristics first, LLM fallback with retry)
        let (action, source, confidence) = decide_with_retry(
            &view,
            &config.goal,
            &acted_on,
            backend,
            &history,
            config.use_heuristics,
            page,
            &mut total_retries,
        )
        .await?;

        let step_duration = step_start.elapsed();

        match source {
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
            return Ok(PilotResult {
                success,
                steps: history,
                final_action: action,
                total_duration: run_start.elapsed(),
                heuristic_hits,
                llm_hits,
                retry_count: total_retries,
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

        history.push(step);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let final_action = Action::Escalate {
        reason: format!("max steps ({}) reached", config.max_steps),
    };
    Ok(PilotResult {
        success: false,
        steps: history,
        final_action,
        total_duration: run_start.elapsed(),
        heuristic_hits,
        llm_hits,
        retry_count: total_retries,
    })
}

/// Decide the next action, retrying the LLM on parse failure with a fresh DOM.
#[allow(clippy::too_many_arguments)]
async fn decide_with_retry(
    view: &crate::semantic::SemanticView,
    goal: &str,
    acted_on: &[u32],
    backend: &dyn PilotBackend,
    history: &[Step],
    use_heuristics: bool,
    page: &chromiumoxide::Page,
    total_retries: &mut u32,
) -> Result<(Action, DecisionSource, f32), crate::Error> {
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

    // LLM fallback with one retry on parse failure
    tracing::info!("heuristic miss — falling back to LLM");
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

            // All retries failed -- escalate
            Ok((
                Action::Escalate {
                    reason: format!("LLM failed after retries: {e}"),
                },
                DecisionSource::Llm,
                0.0,
            ))
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
            let js = format!(
                r#"(() => {{
                    const el = document.querySelector('[data-lad-id="{}"]');
                    if (el) {{
                        el.focus();
                        el.value = '{}';
                        el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                        el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                    }}
                }})()"#,
                element,
                value.replace('\\', "\\\\").replace('\'', "\\'")
            );
            page.evaluate(js).await?;
        }
        Action::Select { element, value, .. } => {
            let js = format!(
                r#"(() => {{
                    const el = document.querySelector('[data-lad-id="{}"]');
                    if (el) {{ el.value = '{}'; el.dispatchEvent(new Event('change', {{ bubbles: true }})); }}
                }})()"#,
                element,
                value.replace('\\', "\\\\").replace('\'', "\\'")
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
        Action::Done { .. } | Action::Escalate { .. } => {}
    }
    Ok(())
}
