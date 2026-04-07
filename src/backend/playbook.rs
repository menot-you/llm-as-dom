//! Playbook backend: deterministic step replay without LLM.
//!
//! Replays pre-recorded playbook steps sequentially. Each call to `decide()`
//! advances one step. Returns `Action::Done` when all steps are complete.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::pilot::{Action, PilotBackend, Step};
use crate::playbook::{self, Playbook};
use crate::semantic::SemanticView;

/// Backend that replays playbook steps deterministically (no LLM calls).
pub struct PlaybookBackend {
    /// The playbook to replay.
    playbook: Playbook,
    /// Interpolated parameter values extracted from the goal.
    params: HashMap<String, String>,
    /// Current step index (atomically incremented on each `decide()` call).
    step_index: AtomicUsize,
}

impl PlaybookBackend {
    /// Create a new playbook backend from a playbook and the user's goal.
    ///
    /// Extracts parameter values from the goal string using credential parsing.
    pub fn new(playbook: Playbook, goal: &str) -> Self {
        let params = playbook::extract_params(goal, &playbook.params);
        Self {
            playbook,
            params,
            step_index: AtomicUsize::new(0),
        }
    }

    /// How many steps remain in the playbook.
    pub fn remaining_steps(&self) -> usize {
        let current = self.step_index.load(Ordering::Relaxed);
        self.playbook.steps.len().saturating_sub(current)
    }
}

#[async_trait]
impl PilotBackend for PlaybookBackend {
    async fn decide(
        &self,
        view: &SemanticView,
        _goal: &str,
        _history: &[Step],
    ) -> Result<Action, crate::Error> {
        let idx = self.step_index.fetch_add(1, Ordering::Relaxed);

        // All steps completed -- check success signal or return Done.
        if idx >= self.playbook.steps.len() {
            let success = check_success(&self.playbook, view);
            return Ok(Action::Done {
                result: serde_json::json!({
                    "playbook": self.playbook.name,
                    "success": success,
                    "steps_completed": self.playbook.steps.len(),
                }),
                reasoning: format!(
                    "playbook \"{}\" completed ({} steps)",
                    self.playbook.name,
                    self.playbook.steps.len()
                ),
            });
        }

        let step = &self.playbook.steps[idx];
        tracing::info!(
            playbook = %self.playbook.name,
            step = idx,
            total = self.playbook.steps.len(),
            kind = ?step.kind,
            selector = %step.selector,
            "playbook step"
        );

        playbook::step_to_action(view, step, &self.params).ok_or_else(|| {
            crate::Error::ActionFailed(format!(
                "playbook \"{}\": step {} selector \"{}\" did not match any element",
                self.playbook.name, idx, step.selector
            ))
        })
    }
}

/// Check if the playbook's success signal matches the current page state.
fn check_success(playbook: &Playbook, view: &SemanticView) -> bool {
    let Some(ref signal) = playbook.success else {
        return true; // No signal defined = assume success
    };

    let url_ok = signal
        .url_contains
        .as_ref()
        .is_none_or(|s| view.url.contains(s));

    let title_ok = signal
        .title_contains
        .as_ref()
        .is_none_or(|s| view.title.contains(s));

    url_ok && title_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playbook::{PlaybookStep, StepKind, SuccessSignal};
    use crate::semantic::{Element, ElementKind, PageState};

    fn test_playbook() -> Playbook {
        Playbook {
            name: "test-login".into(),
            url_pattern: "example.com/login".into(),
            steps: vec![
                PlaybookStep {
                    kind: StepKind::Type,
                    selector: "Email".into(),
                    value: Some("${username}".into()),
                    fallbacks: vec![],
                },
                PlaybookStep {
                    kind: StepKind::Type,
                    selector: "Password".into(),
                    value: Some("${password}".into()),
                    fallbacks: vec!["pw".into()],
                },
                PlaybookStep {
                    kind: StepKind::Click,
                    selector: "Login".into(),
                    value: None,
                    fallbacks: vec![],
                },
            ],
            params: vec!["username".into(), "password".into()],
            success: Some(SuccessSignal {
                url_contains: Some("dashboard".into()),
                title_contains: None,
            }),
        }
    }

    fn test_view() -> SemanticView {
        SemanticView {
            url: "https://example.com/login".into(),
            title: "Login".into(),
            page_hint: "login page".into(),
            elements: vec![
                Element {
                    id: 0,
                    kind: ElementKind::Input,
                    label: "Email".into(),
                    name: Some("email".into()),
                    value: None,
                    placeholder: None,
                    href: None,
                    input_type: Some("email".into()),
                    disabled: false,
                    form_index: Some(0),
                    context: None,
                    hint: None,
                    frame_index: None,
                },
                Element {
                    id: 1,
                    kind: ElementKind::Input,
                    label: "Password".into(),
                    name: Some("pw".into()),
                    value: None,
                    placeholder: None,
                    href: None,
                    input_type: Some("password".into()),
                    disabled: false,
                    form_index: Some(0),
                    context: None,
                    hint: None,
                    frame_index: None,
                },
                Element {
                    id: 2,
                    kind: ElementKind::Button,
                    label: "Login".into(),
                    name: None,
                    value: None,
                    placeholder: None,
                    href: None,
                    input_type: Some("submit".into()),
                    disabled: false,
                    form_index: Some(0),
                    context: None,
                    hint: None,
                    frame_index: None,
                },
            ],
            forms: vec![],
            visible_text: "Welcome! Please log in.".into(),
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        }
    }

    #[tokio::test]
    async fn test_playbook_backend_sequence() {
        let backend =
            PlaybookBackend::new(test_playbook(), "login as alice@test.com password s3cret");
        let view = test_view();
        assert_eq!(backend.remaining_steps(), 3);

        // Step 0: type email
        let action = backend.decide(&view, "", &[]).await.unwrap();
        match &action {
            Action::Type { element, value, .. } => {
                assert_eq!(*element, 0);
                assert_eq!(value, "alice@test.com");
            }
            other => panic!("expected Type, got {other:?}"),
        }
        assert_eq!(backend.remaining_steps(), 2);

        // Step 1: type password
        let action = backend.decide(&view, "", &[]).await.unwrap();
        match &action {
            Action::Type { element, value, .. } => {
                assert_eq!(*element, 1);
                assert_eq!(value, "s3cret");
            }
            other => panic!("expected Type, got {other:?}"),
        }

        // Step 2: click login
        let action = backend.decide(&view, "", &[]).await.unwrap();
        assert!(matches!(&action, Action::Click { element: 2, .. }));

        // Step 3: done (all steps complete)
        let action = backend.decide(&view, "", &[]).await.unwrap();
        assert!(matches!(&action, Action::Done { .. }));
    }

    #[tokio::test]
    async fn test_playbook_backend_selector_miss() {
        let pb = Playbook {
            name: "broken".into(),
            url_pattern: "example.com".into(),
            steps: vec![PlaybookStep {
                kind: StepKind::Click,
                selector: "nonexistent".into(),
                value: None,
                fallbacks: vec![],
            }],
            params: vec![],
            success: None,
        };
        let backend = PlaybookBackend::new(pb, "");
        let view = test_view();

        let result = backend.decide(&view, "", &[]).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_check_success_with_signal() {
        let pb = test_playbook();
        let mut view = test_view();

        // URL doesn't match success signal
        assert!(!check_success(&pb, &view));

        // URL matches
        view.url = "https://example.com/dashboard".into();
        assert!(check_success(&pb, &view));
    }

    #[test]
    fn test_check_success_no_signal() {
        let mut pb = test_playbook();
        pb.success = None;
        let view = test_view();
        assert!(check_success(&pb, &view));
    }
}
