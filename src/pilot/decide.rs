//! Decision logic: tiered dispatch (playbook -> hints -> heuristics -> LLM).

use super::action::Action;
use super::{DecisionSource, Step};

/// Decide the next action, retrying the LLM on parse failure with a fresh DOM.
///
/// When all retries are exhausted, captures a screenshot and embeds it as
/// base64 PNG in the `Escalate` reason for visual debugging context.
#[allow(clippy::too_many_arguments)]
pub async fn decide_with_retry(
    view: &crate::semantic::SemanticView,
    goal: &str,
    acted_on: &[u32],
    backend: &dyn super::PilotBackend,
    history: &[Step],
    playbooks: &[crate::playbook::Playbook],
    use_hints: bool,
    use_heuristics: bool,
    page: &dyn crate::engine::PageHandle,
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
        let h = crate::heuristics::hints::try_hints(view, goal, acted_on);
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
        let h = crate::heuristics::try_resolve(view, goal, acted_on);
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

    // Tier 2.5: Semantic selector (CSS-like / natural-language patterns in goal).
    if use_heuristics && let Some(action) = try_selector_from_goal(view, goal, acted_on) {
        tracing::info!(source = "selector", "selector matched from goal");
        return Ok((action, DecisionSource::Heuristic, 0.75));
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
            if let Some(b64) = super::take_screenshot(page).await {
                reason.push_str("\n[screenshot attached]");
                screenshots.push(b64);
            }

            Ok((Action::Escalate { reason }, DecisionSource::Llm, 0.0))
        }
    }
}

/// Try to extract a CSS-like selector or natural-language element reference
/// from the goal and match it against the semantic view.
///
/// Recognises patterns such as:
/// - `"click the login button"` (natural language)
/// - `"click button:Login"` (kind:label)
/// - `"type hello into [name=email]"` (attribute selector)
/// - `"click #3"` (element ID)
fn try_selector_from_goal(
    view: &crate::semantic::SemanticView,
    goal: &str,
    acted_on: &[u32],
) -> Option<Action> {
    let selector_text = extract_selector_text(goal)?;
    let selector = crate::selector::Selector::parse(&selector_text);
    let matches = crate::selector::find_matches(view, &selector);

    let best = matches.iter().find(|m| !acted_on.contains(&m.element_id))?;

    let goal_lower = goal.to_lowercase();
    let is_type_action = goal_lower.contains("type ")
        || goal_lower.contains("enter ")
        || goal_lower.contains("fill ");

    if is_type_action {
        let value = extract_type_value(goal);
        if !value.is_empty() {
            return Some(Action::Type {
                element: best.element_id,
                value,
                reasoning: format!("selector: {} ({})", best.reason, selector_text),
            });
        }
    }

    Some(Action::Click {
        element: best.element_id,
        reasoning: format!("selector: {} ({})", best.reason, selector_text),
    })
}

/// Extract a selector pattern from a goal string.
///
/// Looks for CSS-like selectors (`[attr=val]`, `#id`, `kind:label`) after
/// action verbs, or falls back to the target after `click`/`go to` verbs
/// as a natural-language selector.
fn extract_selector_text(goal: &str) -> Option<String> {
    let lower = goal.to_lowercase();

    // Attribute selector anywhere: [name=email]
    if let Some(start) = goal.find('[')
        && let Some(end) = goal[start..].find(']')
    {
        return Some(goal[start..start + end + 1].to_string());
    }

    // kind:label pattern anywhere: button:Login, input:email
    for word in goal.split_whitespace() {
        if word.contains(':') && !word.starts_with("http") {
            let parts: Vec<&str> = word.splitn(2, ':').collect();
            let kind_candidates = [
                "button", "btn", "input", "field", "link", "select", "textarea", "checkbox",
                "radio",
            ];
            if kind_candidates
                .iter()
                .any(|k| parts[0].eq_ignore_ascii_case(k))
            {
                return Some(word.to_string());
            }
        }
    }

    // #id pattern
    for word in goal.split_whitespace() {
        if word.starts_with('#') && word.len() > 1 {
            return Some(word.to_string());
        }
    }

    // Natural language after action verb: "click the login button"
    let prefixes = ["click ", "go to ", "navigate to ", "open ", "press "];
    for prefix in &prefixes {
        if let Some(pos) = lower.find(prefix) {
            let rest = goal[pos + prefix.len()..].trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }

    // "type X into Y" / "enter X into Y" / "fill Y with X"
    if let Some(pos) = lower.find(" into ") {
        let rest = goal[pos + " into ".len()..].trim();
        if !rest.is_empty() {
            return Some(rest.to_string());
        }
    }

    None
}

/// Extract the value to type from a goal string.
///
/// Supports patterns like:
/// - `type "hello" into [name=q]` → `hello`
/// - `type hello into .search` → `hello`
/// - `enter foo into bar` → `foo`
fn extract_type_value(goal: &str) -> String {
    let lower = goal.to_lowercase();

    // "type X into Y" pattern
    for verb in &["type ", "enter ", "fill "] {
        if let Some(verb_pos) = lower.find(verb) {
            let after_verb = &goal[verb_pos + verb.len()..];

            // Quoted value: type "hello world" into ...
            if after_verb.starts_with('"') || after_verb.starts_with('\'') {
                let quote = after_verb.chars().next().unwrap();
                if let Some(end) = after_verb[1..].find(quote) {
                    return after_verb[1..1 + end].to_string();
                }
            }

            // Unquoted: type hello into ...
            if let Some(into_pos) = after_verb.to_lowercase().find(" into ") {
                let val = after_verb[..into_pos].trim();
                if !val.is_empty() {
                    return val.to_string();
                }
            }
        }
    }

    String::new()
}
