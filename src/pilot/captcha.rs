//! Captcha/challenge resolution and blocked-page handling.

use std::time::{Duration, Instant};

use crate::semantic::PageState;

use super::action::Action;
use super::{DecisionSource, PilotConfig, PilotResult, Step};

/// Wait for a captcha/challenge page to be resolved by the user.
///
/// Polls every 500ms, re-extracts the DOM, and checks if the page state
/// is no longer `Blocked`. Returns `Ok(())` when resolved, or
/// `Err(Error::Timeout)` if the timeout elapses.
pub async fn wait_for_captcha_resolution(
    page: &dyn crate::engine::PageHandle,
    timeout: Duration,
) -> Result<(), crate::Error> {
    let start = Instant::now();
    let poll_interval = Duration::from_millis(500);

    while start.elapsed() < timeout {
        tokio::time::sleep(poll_interval).await;

        if let Ok(view) = crate::a11y::extract_semantic_view(page).await
            && !matches!(view.state, PageState::Blocked(_))
        {
            return Ok(());
        }
    }

    Err(crate::Error::Timeout {
        timeout_secs: timeout.as_secs(),
    })
}

/// Outcome of handling a blocked page.
pub(super) enum BlockedOutcome {
    /// Continue the main loop (challenge resolved).
    Continue,
    /// Return immediately with a `PilotResult` (escalation).
    Return(Box<PilotResult>),
    /// Fall through to the decision phase (e.g. auth wall).
    FallThrough,
}

/// Handle a blocked page (CAPTCHA, WAF, auth wall) and return the appropriate outcome.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_blocked_page(
    page: &dyn crate::engine::PageHandle,
    config: &PilotConfig,
    view: &crate::semantic::SemanticView,
    reason: &str,
    step_idx: u32,
    step_start: Instant,
    history: &mut Vec<Step>,
    session: &Option<std::sync::Arc<tokio::sync::Mutex<crate::session::SessionState>>>,
    screenshots: &mut Vec<String>,
    run_start: Instant,
    playbook_hits: u32,
    hints_hits: u32,
    heuristic_hits: u32,
    llm_hits: u32,
    total_retries: u32,
) -> Option<BlockedOutcome> {
    let kind = crate::a11y::classify_challenge(reason);

    match kind {
        crate::a11y::ChallengeKind::CloudflareTurnstile => {
            tracing::info!(
                step = step_idx,
                "Turnstile detected — waiting for auto-resolve"
            );
            if wait_for_captcha_resolution(page, Duration::from_secs(5))
                .await
                .is_ok()
            {
                tracing::info!(step = step_idx, "Turnstile auto-resolved");
                return Some(BlockedOutcome::Continue);
            }
        }
        crate::a11y::ChallengeKind::AuthWall => {
            tracing::info!(
                step = step_idx,
                "auth wall detected — continuing pilot loop"
            );
        }
        crate::a11y::ChallengeKind::WafBlock => {
            tracing::warn!(step = step_idx, reason = %reason, "WAF block — escalating");
            if let Some(b64) = super::runner::take_screenshot(page).await {
                screenshots.push(b64);
            }
            let final_action = Action::Escalate {
                reason: format!("page blocked (WAF): {reason}"),
            };
            let step = Step {
                index: step_idx,
                observation: view.clone(),
                action: final_action.clone(),
                source: DecisionSource::Heuristic,
                confidence: 1.0,
                duration: step_start.elapsed(),
            };
            history.push(step);
            let session_snapshot = match session {
                Some(s) => Some(s.lock().await.clone()),
                None => None,
            };
            return Some(BlockedOutcome::Return(Box::new(PilotResult {
                success: false,
                steps: std::mem::take(history),
                final_action,
                total_duration: run_start.elapsed(),
                playbook_hits,
                hints_hits,
                heuristic_hits,
                llm_hits,
                retry_count: total_retries,
                screenshots: std::mem::take(screenshots),
                session_snapshot,
            })));
        }
        crate::a11y::ChallengeKind::Captcha => {
            // Will be handled below by interactive or escalate.
        }
    }

    // Interactive mode: pause for human on captcha/turnstile challenges.
    if config.interactive
        && matches!(
            kind,
            crate::a11y::ChallengeKind::Captcha | crate::a11y::ChallengeKind::CloudflareTurnstile
        )
    {
        tracing::warn!(step = step_idx, reason = %reason, "captcha detected — waiting for human");
        eprintln!();
        eprintln!("  CAPTCHA DETECTED: {reason}");
        eprintln!("  Resolve it in the browser window...");
        eprintln!();

        match wait_for_captcha_resolution(page, Duration::from_secs(120)).await {
            Ok(()) => {
                tracing::info!(step = step_idx, "captcha resolved — continuing");
                eprintln!("  Captcha resolved! Continuing...");
                return Some(BlockedOutcome::Continue);
            }
            Err(_) => {
                tracing::warn!(step = step_idx, "captcha timeout — escalating");
            }
        }
    }

    // Non-interactive or timed-out: escalate (unless AuthWall which continues).
    if !matches!(kind, crate::a11y::ChallengeKind::AuthWall) {
        tracing::warn!(step = step_idx, reason = %reason, "page blocked — escalating");
        if let Some(b64) = super::runner::take_screenshot(page).await {
            screenshots.push(b64);
        }
        let final_action = Action::Escalate {
            reason: format!("page blocked: {reason}"),
        };
        let step = Step {
            index: step_idx,
            observation: view.clone(),
            action: final_action.clone(),
            source: DecisionSource::Heuristic,
            confidence: 1.0,
            duration: step_start.elapsed(),
        };
        history.push(step);
        let session_snapshot = match session {
            Some(s) => Some(s.lock().await.clone()),
            None => None,
        };
        return Some(BlockedOutcome::Return(Box::new(PilotResult {
            success: false,
            steps: std::mem::take(history),
            final_action,
            total_duration: run_start.elapsed(),
            playbook_hits,
            hints_hits,
            heuristic_hits,
            llm_hits,
            retry_count: total_retries,
            screenshots: std::mem::take(screenshots),
            session_snapshot,
        })));
    }

    Some(BlockedOutcome::FallThrough)
}

/// Track session state after an action: cookies, navigation, auth transitions.
pub(super) async fn track_session(
    session_arc: &std::sync::Arc<tokio::sync::Mutex<crate::session::SessionState>>,
    page: &dyn crate::engine::PageHandle,
    step: &Step,
    action: &Action,
) {
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

    let action_desc = match action {
        Action::Click { reasoning, .. } => format!("click: {reasoning}"),
        Action::Type { reasoning, .. } => format!("type: {reasoning}"),
        Action::Select { reasoning, .. } => format!("select: {reasoning}"),
        Action::Scroll { reasoning, .. } => format!("scroll: {reasoning}"),
        Action::Wait { reasoning } => format!("wait: {reasoning}"),
        Action::Navigate { url, reasoning } => format!("navigate to {url}: {reasoning}"),
        _ => String::new(),
    };

    let form_submitted = matches!(action, Action::Click { .. })
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
