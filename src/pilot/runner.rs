//! Pilot run loop: observe -> decide -> act -> repeat.

use base64::Engine;
use std::time::{Duration, Instant};

use crate::semantic::PageState;

use super::action::{Action, execute_action_with_retry};
use super::captcha::{BlockedOutcome, handle_blocked_page, track_session};
use super::decide::decide_with_retry;
use super::{DecisionSource, PilotBackend, PilotConfig, PilotResult, Step};

/// Capture a full-page screenshot as a base64-encoded PNG string.
///
/// Returns `None` if the screenshot fails (non-fatal; logs a warning).
pub async fn take_screenshot(page: &dyn crate::engine::PageHandle) -> Option<String> {
    match page.screenshot_png().await {
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
    page: &dyn crate::engine::PageHandle,
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
    let mut prev_acted_count: Option<usize> = None;
    let mut stale_streak: u32 = 0;
    let mut initial_url: Option<String> = None;

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
        if initial_url.is_none() {
            initial_url = Some(view.url.clone());
        }

        // 1b. Stale-state detection
        let current_element_count = view.elements.len();
        let current_url = &view.url;
        let current_acted = acted_on.len();
        if prev_url.as_deref() == Some(current_url.as_str())
            && prev_element_count == Some(current_element_count)
            && prev_acted_count == Some(current_acted)
            && step_idx > 0
        {
            stale_streak += 1;
        } else {
            stale_streak = 0;
        }
        prev_url = Some(current_url.clone());
        prev_element_count = Some(current_element_count);
        prev_acted_count = Some(current_acted);

        if stale_streak >= 3 {
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

        // 1c. If the page is blocked (CAPTCHA / WAF), handle based on challenge kind.
        if let PageState::Blocked(ref reason) = view.state
            && let Some(result) = handle_blocked_page(
                page,
                config,
                &view,
                reason,
                step_idx,
                step_start,
                &mut history,
                &session,
                &mut screenshots,
                run_start,
                playbook_hits,
                hints_hits,
                heuristic_hits,
                llm_hits,
                total_retries,
            )
            .await
        {
            match result {
                BlockedOutcome::Continue => continue,
                BlockedOutcome::Return(pilot_result) => return Ok(*pilot_result),
                BlockedOutcome::FallThrough => { /* proceed to decide */ }
            }
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
            if success {
                maybe_learn_playbook(config, &history, initial_url.as_deref());
            }
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

        // 4b. FIX-1: Post-action SSRF check — Click can trigger navigation
        // (e.g. clicking a link), so verify the current URL is safe after
        // EVERY action, not just Navigate.
        if let Ok(current_url) = page.url().await
            && !crate::sanitize::is_safe_url(&current_url)
        {
            let final_action = Action::Escalate {
                reason: format!("redirected to unsafe URL after action: {current_url}"),
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

        // 5. Session tracking: extract cookies and record navigation.
        if let Some(ref session_arc) = session {
            track_session(session_arc, page, &step, &action).await;
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

/// Synthesize and persist a playbook from a successful run, if `config.learn`
/// is enabled.
///
/// Non-fatal: logs a warning on any failure path and returns silently. Only
/// fires when the run actually contains non-Tier-0 decisions (a pure playbook
/// replay has nothing new to learn).
fn maybe_learn_playbook(config: &PilotConfig, history: &[Step], initial_url: Option<&str>) {
    let Some(learn) = config.learn.as_ref() else {
        return;
    };
    // Only learn when at least one step was *not* a Tier 0 replay — otherwise
    // we'd just overwrite the playbook with itself.
    let has_new_work = history
        .iter()
        .any(|s| !matches!(s.source, DecisionSource::Playbook));
    if !has_new_work {
        tracing::info!("learn: run was pure playbook replay, nothing to synthesize");
        return;
    }
    let Some(initial) = initial_url else {
        tracing::warn!("learn: no initial URL captured, skipping synthesis");
        return;
    };
    let synthesized = crate::playbook::synthesize_from_history(
        history,
        &config.goal,
        initial,
        &learn.explicit_params,
        learn.name.as_deref(),
    );
    match synthesized {
        Ok(pb) => match crate::playbook::save(&pb, &learn.output_dir) {
            Ok(path) => tracing::info!(path = %path.display(), "playbook learned"),
            Err(e) => tracing::warn!(error = %e, "failed to save learned playbook"),
        },
        Err(e) => tracing::warn!(error = %e, "skipped playbook synthesis"),
    }
}
