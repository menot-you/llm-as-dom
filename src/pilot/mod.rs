//! Browser pilot: observe -> heuristics -> LLM fallback -> act loop.
//!
//! Heuristics resolve ~70-90% of actions in 10ms. LLM only for ambiguity.

pub mod action;
pub mod captcha;
pub mod decide;
mod runner;
pub mod util;

// Re-export public API so callers see the same surface as before.
pub use action::{Action, execute_action, execute_action_with_retry};
pub use captcha::wait_for_captcha_resolution;
pub use decide::decide_with_retry;
pub use runner::{run_pilot, take_screenshot};
pub use util::js_escape;

use async_trait::async_trait;
use serde::Serialize;
use std::time::Duration;

use crate::semantic::SemanticView;

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

/// Opt-in playbook-learning configuration for a pilot run.
///
/// When present on [`PilotConfig`], a successful run (terminating with
/// [`Action::Done`] and containing at least one non-Tier-0 step) is persisted
/// as a playbook in `output_dir`, so the next invocation can replay it as
/// Tier 0 at zero LLM cost. Absence preserves the default behaviour.
#[derive(Debug, Clone)]
pub struct LearnConfig {
    /// Explicit playbook name. When `None`, the name is derived from the goal.
    pub name: Option<String>,
    /// Param key/value pairs used to templatize captured `Type` / `Select`
    /// values, e.g. `{"email": "octocat", "password": "hunter2"}`.
    pub explicit_params: std::collections::HashMap<String, String>,
    /// Directory where the synthesized playbook JSON will be written.
    pub output_dir: std::path::PathBuf,
}

impl LearnConfig {
    /// Convenience constructor using the default `.lad/playbooks/` directory.
    pub fn new(
        name: Option<String>,
        explicit_params: std::collections::HashMap<String, String>,
    ) -> Self {
        Self {
            name,
            explicit_params,
            output_dir: std::path::PathBuf::from(".lad/playbooks"),
        }
    }
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
    /// Whether to pause for human intervention on captchas (default: false).
    ///
    /// When `true` and a captcha/challenge is detected, the pilot waits for
    /// the user to resolve it in the browser window instead of escalating
    /// immediately.
    pub interactive: bool,
    /// Opt-in playbook learning. When `Some` and the run succeeds, the
    /// trajectory is synthesized into a replayable playbook and saved to
    /// `learn.output_dir`. Default: `None` (no learning).
    pub learn: Option<LearnConfig>,
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
            interactive: false,
            learn: None,
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

#[cfg(test)]
mod tests {
    use super::{LearnConfig, PilotConfig};

    #[test]
    fn pilot_config_interactive_default() {
        let config = PilotConfig::default();
        assert!(!config.interactive);
    }

    #[test]
    fn pilot_config_learn_defaults_off() {
        let config = PilotConfig::default();
        assert!(config.learn.is_none());
    }

    #[test]
    fn learn_config_new_sets_default_dir() {
        let lc = LearnConfig::new(Some("pb".into()), Default::default());
        assert_eq!(lc.output_dir, std::path::PathBuf::from(".lad/playbooks"));
        assert_eq!(lc.name.as_deref(), Some("pb"));
    }
}
