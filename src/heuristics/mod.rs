//! Rule-based action engine -- resolves 70-90% of actions without LLM.
//!
//! Strategies are tried in priority order. Falls back to LLM only when
//! confidence is below the threshold.

mod form;
/// Tier 1: `@lad/hints` — explicit developer annotations.
pub(crate) mod hints;
/// Login-specific heuristics (credential parsing, form fill, submit, done).
pub(crate) mod login;
mod navigation;
mod search;

use crate::pilot::Action;
use crate::semantic::SemanticView;

/// Confidence threshold -- below this, escalate to LLM.
const CONFIDENCE_THRESHOLD: f32 = 0.6;

/// Result of a heuristic evaluation attempt.
pub struct HeuristicResult {
    /// The resolved action, or `None` if no rule matched with enough confidence.
    pub action: Option<Action>,
    /// Confidence score (0.0 .. 1.0) of the match.
    pub confidence: f32,
    /// Human-readable explanation of why this rule matched (or didn't).
    pub reason: String,
}

/// Try to resolve the next action using rules only (no LLM).
///
/// Strategies are tried in order of specificity:
/// 1. Login form fill (credential parsing)
/// 2. Search input detection
/// 3. Navigation target matching ("click X", "go to X")
/// 4. Generic form fill (key=value parsing)
/// 5. Submit button click
/// 6. Goal completion detection
///
/// Returns `None` action if confidence is too low -- caller should use LLM.
pub fn try_resolve(view: &SemanticView, goal: &str, acted_on: &[u32]) -> HeuristicResult {
    let goal_lower = goal.to_lowercase();

    // Strategy 1: Login form fill by goal parsing
    if let Some(result) = login::try_form_fill(view, &goal_lower, acted_on)
        && result.confidence >= CONFIDENCE_THRESHOLD
    {
        return result;
    }

    // Strategy 2: Search input detection (original case for query value)
    if let Some(result) = search::try_search(view, goal, acted_on)
        && result.confidence >= CONFIDENCE_THRESHOLD
    {
        return result;
    }

    // Strategy 3: Navigation target matching (original case for target)
    if let Some(result) = navigation::try_navigation(view, goal, acted_on)
        && result.confidence >= CONFIDENCE_THRESHOLD
    {
        return result;
    }

    // Strategy 4: Generic form fill (original case for key=value pairs)
    if let Some(result) = form::try_generic_form(view, goal, acted_on)
        && result.confidence >= CONFIDENCE_THRESHOLD
    {
        return result;
    }

    // Strategy 5: Button click (after fields filled)
    if let Some(result) = login::try_button_click(view, &goal_lower, acted_on)
        && result.confidence >= CONFIDENCE_THRESHOLD
    {
        return result;
    }

    // Strategy 6: Goal completion detection
    if let Some(result) = login::try_detect_done(view, &goal_lower)
        && result.confidence >= CONFIDENCE_THRESHOLD
    {
        return result;
    }

    HeuristicResult {
        action: None,
        confidence: 0.0,
        reason: "no heuristic matched".into(),
    }
}
