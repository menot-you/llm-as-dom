//! Browser pilot: observe → decide → act loop.
//! LLM-agnostic via the PilotBackend trait.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::semantic::SemanticView;

/// A single action the pilot can take.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Click { element: u32, reasoning: String },
    Type { element: u32, value: String, reasoning: String },
    Select { element: u32, value: String, reasoning: String },
    Scroll { direction: String, reasoning: String },
    Wait { reasoning: String },
    Done { result: serde_json::Value, reasoning: String },
    Escalate { reason: String },
}

/// A single step in the pilot's action history.
#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub index: u32,
    pub observation: SemanticView,
    pub action: Action,
    pub duration: Duration,
}

/// LLM-agnostic backend for pilot decisions.
#[async_trait]
pub trait PilotBackend: Send + Sync {
    /// Given a semantic view, goal, and action history, decide the next action.
    async fn decide(
        &self,
        view: &SemanticView,
        goal: &str,
        history: &[Step],
    ) -> Result<Action, crate::Error>;

    /// Backend name for logging.
    fn name(&self) -> &str;
}

/// Pilot configuration.
pub struct PilotConfig {
    pub goal: String,
    pub max_steps: u32,
    pub step_timeout: Duration,
}

impl Default for PilotConfig {
    fn default() -> Self {
        Self {
            goal: String::new(),
            max_steps: 10,
            step_timeout: Duration::from_secs(30),
        }
    }
}

/// Result of a pilot run.
#[derive(Debug, Serialize)]
pub struct PilotResult {
    pub success: bool,
    pub steps: Vec<Step>,
    pub final_action: Action,
    pub total_duration: Duration,
}

/// Run the pilot loop: observe → decide → act → repeat.
pub async fn run_pilot(
    page: &chromiumoxide::Page,
    backend: &dyn PilotBackend,
    config: &PilotConfig,
) -> Result<PilotResult, crate::Error> {
    let run_start = Instant::now();
    let mut history: Vec<Step> = Vec::new();

    for step_idx in 0..config.max_steps {
        let step_start = Instant::now();

        // 1. Observe: extract semantic view
        let view = crate::a11y::extract_semantic_view(page).await?;
        let token_est = view.estimated_tokens();
        tracing::info!(
            step = step_idx,
            elements = view.elements.len(),
            tokens = token_est,
            "observed"
        );

        // 2. Decide: ask backend for next action
        let action = backend.decide(&view, &config.goal, &history).await?;
        let step_duration = step_start.elapsed();

        tracing::info!(
            step = step_idx,
            backend = backend.name(),
            action = ?action,
            duration_ms = step_duration.as_millis() as u64,
            "decided"
        );

        let step = Step {
            index: step_idx as u32,
            observation: view,
            action: action.clone(),
            duration: step_duration,
        };

        // 3. Check for terminal actions before executing
        match &action {
            Action::Done { .. } | Action::Escalate { .. } => {
                history.push(step);
                return Ok(PilotResult {
                    success: matches!(&action, Action::Done { .. }),
                    steps: history,
                    final_action: action,
                    total_duration: run_start.elapsed(),
                });
            }
            _ => {}
        }

        // 4. Act: execute the action on the page
        execute_action(page, &action).await?;
        history.push(step);

        // Brief settle after action
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Max steps reached
    let final_action = Action::Escalate {
        reason: format!("max steps ({}) reached", config.max_steps),
    };
    Ok(PilotResult {
        success: false,
        steps: history,
        final_action,
        total_duration: run_start.elapsed(),
    })
}

/// Execute an action on the page via CDP.
async fn execute_action(page: &chromiumoxide::Page, action: &Action) -> Result<(), crate::Error> {
    match action {
        Action::Click { element, .. } => {
            // Use JS to click by our assigned ghost-id stored in a data attribute
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
        Action::Done { .. } | Action::Escalate { .. } => {
            // Terminal — no browser action needed
        }
    }
    Ok(())
}
