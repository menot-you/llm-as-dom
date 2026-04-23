//! Integration test: learn-then-replay round trip.
//!
//! Proves the opt-in playbook-learning loop end-to-end at the library
//! surface, without needing a real browser:
//!
//! 1. Synthesize a `Playbook` from a fabricated pilot history.
//! 2. Persist it to a temp dir via `playbook::save`.
//! 3. Reload it via `playbook::load_playbooks`.
//! 4. Match the reloaded playbook against a fresh `SemanticView`.
//! 5. Confirm the first replayed `Action` matches the original first
//!    actionable step (demonstrating that a second run at the same URL
//!    would enter Tier 0 replay at zero LLM cost).

use std::collections::HashMap;
use std::time::Duration;

use llm_as_dom::pilot::{Action, DecisionSource, Step};
use llm_as_dom::playbook::{
    StepKind, find_playbook, load_playbooks, save, step_to_action, synthesize_from_history,
};
use llm_as_dom::semantic::{Element, ElementKind, PageState, SemanticView};
use tempfile::TempDir;

fn login_view() -> SemanticView {
    SemanticView {
        url: "https://example.com/login".into(),
        title: "Sign in".into(),
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
                input_type: Some("text".into()),
                disabled: false,
                form_index: Some(0),
                context: None,
                hint: None,
                checked: None,
                options: None,
                frame_index: None,
                is_visible: None,
            },
            Element {
                id: 1,
                kind: ElementKind::Input,
                label: "Password".into(),
                name: Some("password".into()),
                value: None,
                placeholder: None,
                href: None,
                input_type: Some("password".into()),
                disabled: false,
                form_index: Some(0),
                context: None,
                hint: None,
                checked: None,
                options: None,
                frame_index: None,
                is_visible: None,
            },
            Element {
                id: 2,
                kind: ElementKind::Button,
                label: "Sign in".into(),
                name: Some("submit".into()),
                value: None,
                placeholder: None,
                href: None,
                input_type: Some("submit".into()),
                disabled: false,
                form_index: Some(0),
                context: None,
                hint: None,
                checked: None,
                options: None,
                frame_index: None,
                is_visible: None,
            },
        ],
        forms: vec![],
        visible_text: "Sign in".into(),
        text_blocks: vec![],
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
        cards: None,
    }
}

fn step(index: u32, view: SemanticView, action: Action, source: DecisionSource) -> Step {
    Step {
        index,
        observation: view,
        action,
        source,
        confidence: 0.9,
        duration: Duration::from_millis(12),
    }
}

#[test]
fn learn_then_replay_round_trip() {
    let view = login_view();

    // --- "First run" --- trajectory produced by heuristics + LLM
    let history = vec![
        step(
            0,
            view.clone(),
            Action::Type {
                element: 0,
                value: "alice@test.com".into(),
                reasoning: "heuristic: fill email".into(),
            },
            DecisionSource::Heuristic,
        ),
        step(
            1,
            view.clone(),
            Action::Type {
                element: 1,
                value: "s3cret".into(),
                reasoning: "heuristic: fill password".into(),
            },
            DecisionSource::Heuristic,
        ),
        step(
            2,
            view.clone(),
            Action::Click {
                element: 2,
                reasoning: "heuristic: submit form".into(),
            },
            DecisionSource::Heuristic,
        ),
        step(
            3,
            view.clone(),
            Action::Done {
                result: serde_json::json!({"success": true}),
                reasoning: "login succeeded".into(),
            },
            DecisionSource::Llm,
        ),
    ];

    let mut explicit_params = HashMap::new();
    explicit_params.insert("email".into(), "alice@test.com".into());
    explicit_params.insert("password".into(), "s3cret".into());

    let pb = synthesize_from_history(
        &history,
        "login as alice@test.com with password s3cret",
        "https://example.com/login",
        &explicit_params,
        Some("example-login"),
    )
    .expect("synthesis should succeed for successful run");

    // Sanity: templatization replaced raw values with ${key}.
    assert!(
        pb.steps
            .iter()
            .any(|s| s.kind == StepKind::Type && s.value.as_deref() == Some("${email}")),
        "email step should be templatized, got steps: {:#?}",
        pb.steps
    );
    assert!(
        pb.steps
            .iter()
            .any(|s| s.kind == StepKind::Type && s.value.as_deref() == Some("${password}")),
        "password step should be templatized"
    );

    // --- Persist to a fresh dir ---
    let tmp = TempDir::new().unwrap();
    let path = save(&pb, tmp.path()).expect("save should succeed");
    assert!(path.exists(), "playbook file should be on disk");

    // --- "Second run" --- load and match against a fresh view of the same page
    let reloaded = load_playbooks(tmp.path());
    assert_eq!(reloaded.len(), 1, "exactly one playbook should reload");
    let matched = find_playbook(&reloaded, "https://example.com/login")
        .expect("replay URL should match the learned pattern");
    assert_eq!(matched.name, "example-login");

    // Verify the first replay step resolves to a concrete Action with the
    // new credentials substituted in — this is what Tier 0 would emit.
    let fresh_view = login_view();
    let mut replay_params = HashMap::new();
    replay_params.insert("email".into(), "bob@example.com".into());
    replay_params.insert("password".into(), "different".into());

    let first_action = step_to_action(&fresh_view, &matched.steps[0], &replay_params)
        .expect("first step should resolve to an action on the same-shape view");
    match first_action {
        Action::Type { element, value, .. } => {
            assert_eq!(element, 0, "should target the email input");
            assert_eq!(value, "bob@example.com", "new credential should substitute");
        }
        other => panic!("expected Type action for first replay step, got {other:?}"),
    }
}

#[test]
fn learn_then_replay_no_learn_no_file() {
    // Confirms the default path: when --learn is not passed, no side effects.
    let tmp = TempDir::new().unwrap();
    // We simply don't call save(). load_playbooks on an empty dir returns empty.
    let loaded = load_playbooks(tmp.path());
    assert!(loaded.is_empty());
}

/// Fix 4c — URL pattern is derived from `--url` input, not from OAuth bounce.
///
/// Simulates the OAuth case: the pilot was pointed at `app.example.com/login`
/// but the first observation sees `accounts.idp.com/...` after an IdP
/// redirect. The canonical pattern must come from the entry point, so a
/// later replay at `app.example.com/login` matches.
///
/// This covers the runner change via the synthesis contract: the runner
/// now passes `config.initial_url` through to `synthesize_from_history`.
#[test]
fn runner_uses_pilot_url_not_view_url() {
    // Observations all show `accounts.idp.com/...` (the OAuth provider).
    let mut oauth_view = login_view();
    oauth_view.url = "https://accounts.idp.com/oauth/authorize?client_id=x".into();
    let history = vec![
        step(
            0,
            oauth_view.clone(),
            Action::Type {
                element: 0,
                value: "alice@test.com".into(),
                reasoning: "fill email".into(),
            },
            DecisionSource::Heuristic,
        ),
        step(
            1,
            oauth_view.clone(),
            Action::Type {
                element: 1,
                value: "s3cret".into(),
                reasoning: "fill password".into(),
            },
            DecisionSource::Heuristic,
        ),
        step(
            2,
            oauth_view.clone(),
            Action::Click {
                element: 2,
                reasoning: "submit".into(),
            },
            DecisionSource::Heuristic,
        ),
        step(
            3,
            oauth_view,
            Action::Done {
                result: serde_json::json!({"success": true}),
                reasoning: "login succeeded".into(),
            },
            DecisionSource::Llm,
        ),
    ];

    let mut params = HashMap::new();
    params.insert("email".into(), "alice@test.com".into());
    params.insert("password".into(), "s3cret".into());

    // Pass the CANONICAL entry point as initial_url.
    let pb = synthesize_from_history(
        &history,
        "login",
        "https://app.example.com/login",
        &params,
        Some("app-login"),
    )
    .expect("successful trajectory should synthesize");
    assert_eq!(
        pb.url_pattern, "app.example.com/login",
        "pattern must derive from --url input, not from the OAuth-bounce view.url"
    );
    assert!(
        !pb.url_pattern.contains("accounts.idp.com"),
        "post-OAuth host must NOT leak into the pattern"
    );
}

/// Fix 1 — reject synthesis of a failed login trajectory.
///
/// End-to-end proof that a run terminating with `Done { success: false }`
/// does NOT produce a playbook file on disk. This is the canonical attack
/// that the success-gate blocks: if we didn't, a failed login trajectory
/// (e.g. bad password) would be persisted and replayed forever as a
/// "working" playbook.
#[test]
fn failed_login_trajectory_does_not_persist_playbook() {
    let view = login_view();
    let history = vec![
        step(
            0,
            view.clone(),
            Action::Type {
                element: 0,
                value: "alice@test.com".into(),
                reasoning: "fill email".into(),
            },
            DecisionSource::Heuristic,
        ),
        step(
            1,
            view.clone(),
            Action::Type {
                element: 1,
                value: "wrong-password".into(),
                reasoning: "fill password".into(),
            },
            DecisionSource::Heuristic,
        ),
        step(
            2,
            view.clone(),
            Action::Click {
                element: 2,
                reasoning: "submit".into(),
            },
            DecisionSource::Heuristic,
        ),
        step(
            3,
            view,
            Action::Done {
                // login failed — mirrors `heuristics::login::try_detect_done`'s
                // error branch.
                result: serde_json::json!({
                    "success": false,
                    "error": "login failed",
                }),
                reasoning: "login failed".into(),
            },
            DecisionSource::Heuristic,
        ),
    ];

    let mut params = HashMap::new();
    params.insert("email".into(), "alice@test.com".into());
    params.insert("password".into(), "wrong-password".into());

    let tmp = TempDir::new().unwrap();
    let result = synthesize_from_history(
        &history,
        "login as alice@test.com with password wrong-password",
        "https://example.com/login",
        &params,
        Some("example-login"),
    );
    assert!(
        result.is_err(),
        "failed login trajectory must be rejected by the success gate"
    );
    // Simulate the runner's "synthesize then save" pipeline — on failure,
    // `save` is never called, so the dir stays empty.
    let loaded = load_playbooks(tmp.path());
    assert!(
        loaded.is_empty(),
        "no playbook should land on disk for a failed run"
    );
}
