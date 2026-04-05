//! Rule-based action engine -- resolves 70-90% of actions without LLM.
//!
//! Strategies are tried in priority order. Falls back to LLM only when
//! confidence is below the threshold.

/// E-commerce flow detection (add-to-cart, checkout, payment buttons).
pub mod ecommerce;
mod form;
/// Tier 1: `@lad/hints` — explicit developer annotations.
pub mod hints;
/// Login-specific heuristics (credential parsing, form fill, submit, done).
pub mod login;
/// MFA/2FA detection and escalation.
pub mod mfa;
/// Multi-step wizard form navigation.
pub mod multistep;
mod navigation;
/// OAuth flow heuristics (OAuth buttons, consent approval).
pub mod oauth;
mod search;
pub mod selector;
pub mod validation;

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
///
/// - S1: Login form fill (credential parsing)
/// - S2: Search input detection
/// - S3: Navigation target matching ("click X", "go to X")
/// - S4: Generic form fill (key=value parsing)
/// - S4b: E-commerce actions (add-to-cart / checkout)
/// - S5: Submit button click
/// - S5b: Multi-step form navigation (next / continue)
/// - S6: Goal completion detection
/// - S6b: MFA/2FA detection (escalate)
/// - S6c: Validation error detection (escalate)
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

    // Strategy 1.5: OAuth button click (when no password field or no credentials)
    if let Some(result) = oauth::try_oauth_button(view, &goal_lower, acted_on)
        && result.confidence >= CONFIDENCE_THRESHOLD
    {
        return result;
    }

    // Strategy 1.6: OAuth consent approval
    if let Some(result) = oauth::try_consent_approval(view, &goal_lower, acted_on)
        && result.confidence >= CONFIDENCE_THRESHOLD
    {
        return result;
    }

    // Strategy 1.7: Semantic Selector (Tier 2.5)
    if let Some(result) = selector::try_selector(view, goal, acted_on)
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

    // Strategy 4.5: E-commerce actions (add-to-cart, checkout)
    if let Some(result) = ecommerce::try_ecommerce_action(view, goal, acted_on)
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

    // Strategy 5.5: Multi-step form navigation (next/continue)
    if let Some(result) = multistep::try_next_step(view, goal, acted_on)
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

    // Strategy 6.5: MFA/2FA detection (escalate)
    if let Some(result) = mfa::try_detect_mfa(view, goal, acted_on)
        && result.confidence >= CONFIDENCE_THRESHOLD
    {
        return result;
    }

    // Strategy 6.6: Validation error detection (escalate)
    if let Some(result) = validation::try_detect_validation(view, goal, acted_on)
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
