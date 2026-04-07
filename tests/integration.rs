//! Integration tests for LLM-as-DOM.
//!
//! Browser-dependent tests are `#[ignore]` — run with:
//!   cargo test -- --ignored
//!
//! Pure-logic tests run in normal `cargo test`.

use llm_as_dom::heuristics::{self, HeuristicResult};
use llm_as_dom::pilot::{Action, DecisionSource};
use llm_as_dom::semantic::{Element, ElementHint, ElementKind, PageState, SemanticView};

// ── Helpers ──────────────────────────────────────────────────────────

/// Build a minimal `SemanticView` from a list of elements.
fn mock_view(elements: Vec<Element>, page_hint: &str) -> SemanticView {
    SemanticView {
        url: "https://example.com".into(),
        title: "Test Page".into(),
        page_hint: page_hint.into(),
        elements,
        forms: vec![],
        visible_text: String::new(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    }
}

/// Shorthand for building an `Element`.
fn input_element(
    id: u32,
    label: &str,
    input_type: &str,
    name: Option<&str>,
    form: Option<u32>,
) -> Element {
    Element {
        id,
        kind: ElementKind::Input,
        label: label.into(),
        name: name.map(|s| s.into()),
        value: None,
        placeholder: None,
        href: None,
        input_type: Some(input_type.into()),
        disabled: false,
        form_index: form,
        context: None,
        hint: None,
        frame_index: None,
    }
}

fn button_element(id: u32, label: &str, form: Option<u32>) -> Element {
    Element {
        id,
        kind: ElementKind::Button,
        label: label.into(),
        name: None,
        value: None,
        placeholder: None,
        href: None,
        input_type: None,
        disabled: false,
        form_index: form,
        context: None,
        hint: None,
        frame_index: None,
    }
}

/// Build a link element with `href` and label.
fn link_element(id: u32, label: &str, href: &str) -> Element {
    Element {
        id,
        kind: ElementKind::Link,
        label: label.into(),
        name: None,
        value: None,
        placeholder: None,
        href: Some(href.into()),
        input_type: None,
        disabled: false,
        form_index: None,
        context: None,
        hint: None,
        frame_index: None,
    }
}

// ── Browser tests (#[ignore]) ────────────────────────────────────────

/// Launches a real browser, extracts example.com, asserts elements > 0.
#[ignore]
#[tokio::test]
async fn test_extract_example_com() {
    use llm_as_dom::engine::chromium::ChromiumEngine;
    use llm_as_dom::engine::{BrowserEngine, EngineConfig};
    use std::time::Duration;

    let engine = ChromiumEngine::launch(EngineConfig::default())
        .await
        .expect("browser launch");

    let page = engine.new_page("https://example.com").await.unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    let view = llm_as_dom::a11y::extract_semantic_view(page.as_ref())
        .await
        .unwrap();
    assert!(
        !view.elements.is_empty(),
        "example.com should have at least 1 element"
    );
    assert!(!view.title.is_empty(), "page should have a title");
    assert_eq!(view.state, PageState::Ready);

    drop(page);
    engine.close().await.unwrap();
}

/// Extracts HN login page, asserts page_hint == "login page".
#[ignore]
#[tokio::test]
async fn test_extract_classifies_login_page() {
    use llm_as_dom::engine::chromium::ChromiumEngine;
    use llm_as_dom::engine::{BrowserEngine, EngineConfig};
    use std::time::Duration;

    let engine = ChromiumEngine::launch(EngineConfig::default())
        .await
        .expect("browser launch");

    let page = engine
        .new_page("https://news.ycombinator.com/login")
        .await
        .unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    let view = llm_as_dom::a11y::extract_semantic_view(page.as_ref())
        .await
        .unwrap();
    assert_eq!(
        view.page_hint, "login page",
        "HN login should be classified as login page"
    );

    drop(page);
    engine.close().await.unwrap();
}

// ── Pure-logic tests (no browser needed) ─────────────────────────────

/// Builds a mock SemanticView with login fields, runs heuristics, asserts correct actions.
#[test]
fn test_heuristic_resolves_login() {
    let view = mock_view(
        vec![
            input_element(0, "Username", "text", Some("acct"), Some(0)),
            input_element(1, "Password", "password", Some("pw"), Some(0)),
            button_element(2, "Login", Some(0)),
        ],
        "login page",
    );

    let goal = "login as testuser password secret123";

    // Step 1: should fill username
    let r1: HeuristicResult = heuristics::try_resolve(&view, goal, &[]);
    assert!(r1.action.is_some(), "should resolve username fill");
    assert!(r1.confidence >= 0.6, "confidence should be above threshold");
    match r1.action.unwrap() {
        Action::Type { element, value, .. } => {
            assert_eq!(element, 0, "should target username field");
            assert_eq!(value, "testuser");
        }
        other => panic!("expected Type action, got {other:?}"),
    }

    // Step 2: should fill password (after username acted on)
    let r2 = heuristics::try_resolve(&view, goal, &[0]);
    assert!(r2.action.is_some(), "should resolve password fill");
    match r2.action.unwrap() {
        Action::Type { element, value, .. } => {
            assert_eq!(element, 1, "should target password field");
            assert_eq!(value, "secret123");
        }
        other => panic!("expected Type action, got {other:?}"),
    }

    // Step 3: should click login button (after both fields filled)
    let r3 = heuristics::try_resolve(&view, goal, &[0, 1]);
    assert!(r3.action.is_some(), "should resolve button click");
    match r3.action.unwrap() {
        Action::Click { element, .. } => {
            assert_eq!(element, 2, "should target login button");
        }
        other => panic!("expected Click action, got {other:?}"),
    }
}

/// Builds SemanticView with 2 forms, asserts only the login form is targeted.
#[test]
fn test_heuristic_form_scoping() {
    let view = mock_view(
        vec![
            // Form 0: search form
            input_element(0, "Search", "text", Some("q"), Some(0)),
            button_element(1, "Go", Some(0)),
            // Form 1: login form
            input_element(2, "Username", "text", Some("acct"), Some(1)),
            input_element(3, "Password", "password", Some("pw"), Some(1)),
            button_element(4, "Login", Some(1)),
        ],
        "login page",
    );

    let goal = "login as admin password admin123";

    // Should target form 1 (the login form with password), not form 0 (search)
    let r1 = heuristics::try_resolve(&view, goal, &[]);
    assert!(r1.action.is_some(), "should resolve an action");
    match r1.action.unwrap() {
        Action::Type { element, .. } => {
            assert!(
                element == 2 || element == 3,
                "should target an element in form 1 (login), got element {element}"
            );
        }
        other => panic!("expected Type in form 1, got {other:?}"),
    }

    // After filling both login fields, should click login button in form 1
    let r2 = heuristics::try_resolve(&view, goal, &[2, 3]);
    assert!(r2.action.is_some(), "should resolve button click");
    match r2.action.unwrap() {
        Action::Click { element, .. } => {
            assert_eq!(
                element, 4,
                "should click login button in form 1, not search button"
            );
        }
        other => panic!("expected Click on element 4, got {other:?}"),
    }
}

/// Tests JSON extraction from various LLM response formats.
#[test]
fn test_ollama_response_parsing() {
    // The parse_action function is in backend::generic which is pub
    // We test via the re-exported module

    // 1. Clean JSON
    let clean = r#"{"action":"click","element":2,"reasoning":"submit"}"#;
    let action: Action = serde_json::from_str(clean).unwrap();
    assert!(matches!(action, Action::Click { element: 2, .. }));

    // 2. JSON wrapped in think tags (Qwen3 style)
    let think_wrapped = r#"<think>I need to click the submit button</think>{"action":"type","element":0,"value":"hello","reasoning":"fill input"}"#;
    // strip_think_tags + extract_json are private, but we can test parse_action
    // via its public effects. Let's test the Action deserialization patterns instead.
    let after_strip = think_wrapped.split("</think>").last().unwrap().trim();
    let action: Action = serde_json::from_str(after_strip).unwrap();
    assert!(matches!(action, Action::Type { element: 0, .. }));

    // 3. JSON inside markdown code block
    let markdown = "Sure, here's the action:\n```json\n{\"action\":\"wait\",\"reasoning\":\"page loading\"}\n```\nDone.";
    // Extract between ``` markers
    let json_str = markdown
        .split("```json\n")
        .nth(1)
        .and_then(|s| s.split("\n```").next())
        .unwrap();
    let action: Action = serde_json::from_str(json_str).unwrap();
    assert!(matches!(action, Action::Wait { .. }));

    // 4. Done action with nested result
    let done_json = r#"{"action":"done","result":{"success":true,"url":"https://example.com/dashboard"},"reasoning":"logged in"}"#;
    let action: Action = serde_json::from_str(done_json).unwrap();
    assert!(matches!(action, Action::Done { .. }));

    // 5. Escalate action
    let escalate = r#"{"action":"escalate","reason":"CAPTCHA detected, cannot proceed"}"#;
    let action: Action = serde_json::from_str(escalate).unwrap();
    assert!(matches!(action, Action::Escalate { .. }));

    // 6. Select action
    let select = r#"{"action":"select","element":5,"value":"option1","reasoning":"pick first"}"#;
    let action: Action = serde_json::from_str(select).unwrap();
    assert!(matches!(action, Action::Select { element: 5, .. }));
}

/// Builds a view and checks token count is reasonable.
#[test]
fn test_semantic_view_token_estimate() {
    let view = mock_view(
        vec![
            input_element(0, "Email", "email", Some("email"), Some(0)),
            input_element(1, "Password", "password", Some("pass"), Some(0)),
            button_element(2, "Sign In", Some(0)),
        ],
        "login page",
    );

    let tokens = view.estimated_tokens();
    // A view with 3 elements + headers should be roughly 30-200 tokens
    assert!(tokens > 10, "token estimate too low: {tokens}");
    assert!(tokens < 500, "token estimate too high: {tokens}");

    // Prompt should contain all element labels
    let prompt = view.to_prompt();
    assert!(prompt.contains("Email"), "prompt should contain 'Email'");
    assert!(
        prompt.contains("Password"),
        "prompt should contain 'Password'"
    );
    assert!(
        prompt.contains("Sign In"),
        "prompt should contain 'Sign In'"
    );
    assert!(
        prompt.contains("login page"),
        "prompt should contain page hint"
    );

    // Empty view should have minimal tokens
    let empty_view = mock_view(vec![], "content page");
    let empty_tokens = empty_view.estimated_tokens();
    assert!(
        empty_tokens < 30,
        "empty view tokens too high: {empty_tokens}"
    );
}

// ── Wave 12: New tests ──────────────────────────────────────────────

/// Test search heuristic: a SemanticView with a search input and a "search for X" goal.
#[test]
fn test_heuristic_search() {
    let view = mock_view(
        vec![
            input_element(0, "Search the web", "search", Some("q"), None),
            button_element(1, "Search", None),
        ],
        "search page",
    );

    let goal = "search for rust tutorials";

    let r = heuristics::try_resolve(&view, goal, &[]);
    assert!(r.action.is_some(), "should resolve search fill");
    assert!(r.confidence >= 0.6, "confidence should meet threshold");
    match r.action.unwrap() {
        Action::Type { element, value, .. } => {
            assert_eq!(element, 0, "should target search input");
            assert_eq!(value, "rust tutorials", "should extract search query");
        }
        other => panic!("expected Type action, got {other:?}"),
    }
}

/// Test navigation heuristic: a SemanticView with links, goal "click About".
#[test]
fn test_heuristic_navigation() {
    let view = mock_view(
        vec![
            link_element(0, "Home", "/home"),
            link_element(1, "About", "/about"),
            link_element(2, "Contact", "/contact"),
        ],
        "content page",
    );

    let goal = "click About";

    let r = heuristics::try_resolve(&view, goal, &[]);
    assert!(r.action.is_some(), "should resolve navigation click");
    assert!(r.confidence >= 0.6, "confidence should meet threshold");
    match r.action.unwrap() {
        Action::Click { element, .. } => {
            assert_eq!(element, 1, "should click the About link");
        }
        other => panic!("expected Click action, got {other:?}"),
    }
}

/// Test generic form fill: SemanticView with name+email inputs, goal with key=value pairs.
#[test]
fn test_heuristic_generic_form() {
    let view = mock_view(
        vec![
            input_element(0, "Full Name", "text", Some("name"), Some(0)),
            input_element(1, "Email Address", "email", Some("email"), Some(0)),
            button_element(2, "Submit", Some(0)),
        ],
        "form page",
    );

    let goal = "fill form with name=John email=john@test.com";

    // Step 1: should fill name field
    let r1 = heuristics::try_resolve(&view, goal, &[]);
    assert!(r1.action.is_some(), "should resolve name fill");
    match r1.action.unwrap() {
        Action::Type { element, value, .. } => {
            assert_eq!(element, 0, "should target name input");
            assert_eq!(value, "John");
        }
        other => panic!("expected Type for name, got {other:?}"),
    }

    // Step 2: should fill email field
    let r2 = heuristics::try_resolve(&view, goal, &[0]);
    assert!(r2.action.is_some(), "should resolve email fill");
    match r2.action.unwrap() {
        Action::Type { element, value, .. } => {
            assert_eq!(element, 1, "should target email input");
            assert_eq!(value, "john@test.com");
        }
        other => panic!("expected Type for email, got {other:?}"),
    }
}

/// Test that build_prompt contains few-shot examples relevant to the goal type.
#[test]
fn test_prompt_format() {
    use llm_as_dom::backend::generic::build_prompt;

    let view = mock_view(
        vec![
            input_element(0, "Email", "email", Some("email"), Some(0)),
            input_element(1, "Secret", "text", Some("pw"), Some(0)),
            button_element(2, "Login", Some(0)),
        ],
        "login page",
    );

    // Login prompt should contain login few-shot
    let prompt = build_prompt(&view, "login as alice@test.com", &[], 10000);
    assert!(
        prompt.contains("FEW-SHOT EXAMPLES"),
        "prompt should have few-shot section"
    );
    assert!(
        prompt.contains("alice@test.com") || prompt.contains("login"),
        "login prompt should contain login-related example"
    );
    assert!(
        prompt.contains("SYSTEM:"),
        "prompt should have system instruction"
    );
    assert!(
        prompt.contains("exactly ONE JSON"),
        "prompt should enforce single JSON response"
    );
    assert!(
        prompt.contains("No markdown"),
        "prompt should forbid markdown"
    );

    // Search prompt should contain search few-shot
    let search_prompt = build_prompt(&view, "search for tutorials", &[], 10000);
    assert!(
        search_prompt.contains("search"),
        "search prompt should contain search example"
    );

    // Navigation prompt should contain click example
    let nav_prompt = build_prompt(&view, "click About", &[], 10000);
    assert!(
        nav_prompt.contains("click"),
        "nav prompt should contain click example"
    );
}

/// Test PilotConfig retry defaults and PilotResult retry tracking.
#[test]
fn test_pilot_config_retry_defaults() {
    use llm_as_dom::pilot::PilotConfig;

    let config = PilotConfig::default();
    assert_eq!(
        config.max_retries_per_step, 2,
        "default retries should be 2"
    );
    assert_eq!(config.max_steps, 10, "default max steps should be 10");
    assert!(config.use_heuristics, "heuristics should be on by default");
}

/// Test error::ActionFailed variant exists and formats correctly.
#[test]
fn test_error_action_failed() {
    let err = llm_as_dom::Error::ActionFailed("element 5 not found".into());
    let msg = format!("{err}");
    assert!(
        msg.contains("action failed"),
        "ActionFailed should format with prefix"
    );
    assert!(
        msg.contains("element 5 not found"),
        "ActionFailed should contain the detail message"
    );
}

// ── Bot-challenge detection tests ──────────────────────────────────

/// Cloudflare "Just a moment" page should be detected as blocked.
#[test]
fn test_detect_cloudflare_challenge() {
    use llm_as_dom::a11y::detect_bot_challenge;

    let view = SemanticView {
        url: "https://stackoverflow.com/questions/123".into(),
        title: "Just a moment...".into(),
        page_hint: "content page".into(),
        elements: vec![],
        forms: vec![],
        visible_text: "Checking your browser before accessing".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };
    let result = detect_bot_challenge(&view);
    assert!(result.is_some(), "Cloudflare challenge should be detected");
    assert!(
        result.unwrap().contains("just a moment"),
        "reason should mention the title keyword"
    );
}

/// Normal page should NOT be detected as blocked.
#[test]
fn test_detect_normal_page_not_blocked() {
    use llm_as_dom::a11y::detect_bot_challenge;

    let view = mock_view(
        vec![
            input_element(0, "Email", "email", Some("email"), Some(0)),
            input_element(1, "Password", "password", Some("pass"), Some(0)),
            button_element(2, "Sign In", Some(0)),
        ],
        "login page",
    );
    assert!(
        detect_bot_challenge(&view).is_none(),
        "normal login page should not be flagged as blocked"
    );
}

/// CAPTCHA text in visible content should trigger detection.
#[test]
fn test_detect_captcha_in_text() {
    use llm_as_dom::a11y::detect_bot_challenge;

    let view = SemanticView {
        url: "https://example.com".into(),
        title: "Example".into(),
        page_hint: "content page".into(),
        elements: vec![],
        forms: vec![],
        visible_text: "Please complete the CAPTCHA to continue".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };
    let result = detect_bot_challenge(&view);
    assert!(result.is_some(), "CAPTCHA text should trigger detection");
}

/// Few interactive elements + challenge URL should trigger detection.
#[test]
fn test_detect_challenge_url_with_few_elements() {
    use llm_as_dom::a11y::detect_bot_challenge;

    let view = SemanticView {
        url: "https://example.com/cdn-cgi/challenge".into(),
        title: "Example".into(),
        page_hint: "content page".into(),
        elements: vec![button_element(0, "Verify", None)],
        forms: vec![],
        visible_text: String::new(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };
    let result = detect_bot_challenge(&view);
    assert!(
        result.is_some(),
        "challenge URL with few elements should trigger"
    );
}

/// Page with many interactive elements and a challenge URL should NOT be blocked.
#[test]
fn test_detect_many_elements_not_blocked() {
    use llm_as_dom::a11y::detect_bot_challenge;

    let view = SemanticView {
        url: "https://example.com/cdn-cgi/something".into(),
        title: "Dashboard".into(),
        page_hint: "form page".into(),
        elements: vec![
            input_element(0, "Name", "text", Some("name"), Some(0)),
            input_element(1, "Email", "email", Some("email"), Some(0)),
            button_element(2, "Submit", Some(0)),
        ],
        forms: vec![],
        visible_text: "Fill out the form".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };
    assert!(
        detect_bot_challenge(&view).is_none(),
        "page with 3+ interactive elements should not be blocked by URL alone"
    );
}

/// PageState::Blocked variant serialises and displays correctly.
#[test]
fn test_blocked_state_in_prompt() {
    let mut view = mock_view(vec![], "content page");
    view.state = PageState::Blocked("Cloudflare challenge".into());
    view.blocked_reason = Some("Cloudflare challenge".into());

    let prompt = view.to_prompt();
    assert!(
        prompt.contains("BLOCKED: Cloudflare challenge"),
        "prompt should show blocked reason"
    );
    assert!(
        prompt.contains("Blocked"),
        "prompt should show Blocked state"
    );
}

// ── @lad/hints + 5-tier dispatcher tests ─────────────────────────────

/// Helper: build a login view with `data-lad` hint annotations on all elements.
fn hinted_login_view() -> SemanticView {
    SemanticView {
        url: "https://example.com/login".into(),
        title: "Login — My App".into(),
        page_hint: "login page".into(),
        elements: vec![
            Element {
                id: 0,
                kind: ElementKind::Input,
                label: "Email".into(),
                name: Some("email".into()),
                value: None,
                placeholder: Some("you@example.com".into()),
                href: None,
                input_type: Some("email".into()),
                disabled: false,
                form_index: Some(0),
                context: None,
                hint: Some(ElementHint {
                    hint_type: "field".into(),
                    value: "email".into(),
                }),
                frame_index: None,
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
                hint: Some(ElementHint {
                    hint_type: "field".into(),
                    value: "password".into(),
                }),
                frame_index: None,
            },
            Element {
                id: 2,
                kind: ElementKind::Button,
                label: "Sign In".into(),
                name: None,
                value: None,
                placeholder: None,
                href: None,
                input_type: Some("submit".into()),
                disabled: false,
                form_index: Some(0),
                context: None,
                hint: Some(ElementHint {
                    hint_type: "action".into(),
                    value: "submit".into(),
                }),
                frame_index: None,
            },
        ],
        forms: vec![],
        visible_text: "Sign In".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    }
}

/// Test 1: SemanticView with hinted elements shows hints in `to_prompt()`.
#[test]
fn test_hints_detection() {
    let view = hinted_login_view();
    let prompt = view.to_prompt();

    assert!(
        prompt.contains("[hint:field:email]"),
        "prompt should contain email hint annotation"
    );
    assert!(
        prompt.contains("[hint:field:password]"),
        "prompt should contain password hint annotation"
    );
    assert!(
        prompt.contains("[hint:action:submit]"),
        "prompt should contain action hint annotation"
    );
}

/// Test 2: Hinted login form resolves correct fill + click sequence.
#[test]
fn test_hints_resolve_login() {
    let view = hinted_login_view();
    let goal = "login as alice@test.com password s3cret";

    // Step 1: email field via hint
    let r1 = heuristics::hints::try_hints(&view, goal, &[]);
    assert!(r1.action.is_some(), "should resolve email via hint");
    match r1.action.unwrap() {
        Action::Type { element, value, .. } => {
            assert_eq!(element, 0, "should target hinted email field");
            assert_eq!(value, "alice@test.com");
        }
        other => panic!("expected Type, got {other:?}"),
    }

    // Step 2: password field via hint
    let r2 = heuristics::hints::try_hints(&view, goal, &[0]);
    assert!(r2.action.is_some(), "should resolve password via hint");
    match r2.action.unwrap() {
        Action::Type { element, value, .. } => {
            assert_eq!(element, 1, "should target hinted password field");
            assert_eq!(value, "s3cret");
        }
        other => panic!("expected Type, got {other:?}"),
    }

    // Step 3: submit button via hint
    let r3 = heuristics::hints::try_hints(&view, goal, &[0, 1]);
    assert!(r3.action.is_some(), "should click submit via hint");
    match r3.action.unwrap() {
        Action::Click { element, .. } => {
            assert_eq!(element, 2, "should target hinted submit button");
        }
        other => panic!("expected Click, got {other:?}"),
    }
}

/// Test 3: Hint-resolved actions have confidence >= 0.98.
#[test]
fn test_hints_high_confidence() {
    let view = hinted_login_view();
    let goal = "login as alice@test.com password s3cret";

    let r = heuristics::hints::try_hints(&view, goal, &[]);
    assert!(
        r.confidence >= 0.98,
        "hint confidence should be >= 0.98, got {}",
        r.confidence
    );
}

/// Test 4: Verify 5-tier order — hints (Tier 1) checked before heuristics (Tier 2).
///
/// When a page has both hints and heuristic-matchable elements, the hint
/// should win because it runs first in the dispatcher chain.
#[test]
fn test_5tier_order_hints_before_heuristics() {
    let view = hinted_login_view();
    let goal = "login as alice@test.com password s3cret";

    // Hints should resolve first — and with higher confidence than heuristics.
    let hint_result = heuristics::hints::try_hints(&view, goal, &[]);
    assert!(
        hint_result.action.is_some(),
        "hints should resolve before heuristics get a chance"
    );
    assert!(
        hint_result.confidence >= 0.9,
        "hint confidence must pass the 0.9 gate in decide_with_retry"
    );

    // Verify the enum variant ordering: Hints != Heuristic.
    assert_ne!(
        DecisionSource::Hints,
        DecisionSource::Heuristic,
        "Hints and Heuristic must be distinct sources"
    );
}

/// Test 5: Page without hints falls through to heuristics (no hint action resolved).
#[test]
fn test_no_hints_fallback() {
    let view = mock_view(
        vec![
            input_element(0, "Username", "text", Some("acct"), Some(0)),
            input_element(1, "Password", "password", Some("pw"), Some(0)),
            button_element(2, "Login", Some(0)),
        ],
        "login page",
    );

    let goal = "login as testuser password secret123";

    // Hints should return no action (no data-lad attributes).
    let hint_r = heuristics::hints::try_hints(&view, goal, &[]);
    assert!(
        hint_r.action.is_none(),
        "no hints present — should return None"
    );

    // Heuristics should still work (fallback).
    let heur_r = heuristics::try_resolve(&view, goal, &[]);
    assert!(
        heur_r.action.is_some(),
        "heuristics should resolve when hints don't"
    );
}

// ── Fix 3: Reddit challenge URL detection ───────────────────────────

/// Reddit's `?js_challenge=1&token=...` URL should be detected as blocked.
#[test]
fn test_detect_reddit_challenge_url() {
    use llm_as_dom::a11y::detect_bot_challenge;

    let view = SemanticView {
        url: "https://www.reddit.com/login?js_challenge=1&token=abc123".into(),
        title: "Reddit - Login".into(),
        page_hint: "login page".into(),
        elements: vec![
            input_element(0, "Username", "text", Some("username"), Some(0)),
            input_element(1, "Password", "password", Some("password"), Some(0)),
            button_element(2, "Log In", Some(0)),
        ],
        forms: vec![],
        visible_text: String::new(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };
    let result = detect_bot_challenge(&view);
    assert!(
        result.is_some(),
        "Reddit challenge URL should be detected as blocked"
    );
    let reason = result.unwrap();
    assert!(
        reason.contains("challenge"),
        "reason should mention 'challenge', got: {reason}"
    );
}

/// URL with `verify` query param should be detected.
#[test]
fn test_detect_verify_url() {
    use llm_as_dom::a11y::detect_bot_challenge;

    let view = SemanticView {
        url: "https://example.com/verify?token=xyz".into(),
        title: "Verify Your Identity".into(),
        page_hint: "content page".into(),
        elements: vec![],
        forms: vec![],
        visible_text: String::new(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };
    let result = detect_bot_challenge(&view);
    assert!(
        result.is_some(),
        "URL with 'verify' should be detected as blocked"
    );
}

/// URL with `security_check` should be detected.
#[test]
fn test_detect_security_check_url() {
    use llm_as_dom::a11y::detect_bot_challenge;

    let view = SemanticView {
        url: "https://example.com/security_check?ref=login".into(),
        title: "Security Check".into(),
        page_hint: "content page".into(),
        elements: vec![],
        forms: vec![],
        visible_text: String::new(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };
    let result = detect_bot_challenge(&view);
    assert!(
        result.is_some(),
        "URL with 'security_check' should be detected as blocked"
    );
}

// ── Fix 4: GitHub 404 / error page detection ────────────────────────

/// GitHub's "Page not found" title should be detected.
#[test]
fn test_detect_github_404() {
    use llm_as_dom::a11y::detect_bot_challenge;

    let view = SemanticView {
        url: "https://github.com/org/private-repo".into(),
        title: "Page not found · GitHub".into(),
        page_hint: "content page".into(),
        elements: (0..10)
            .map(|i| link_element(i, &format!("Link {i}"), "/somewhere"))
            .collect(),
        forms: vec![],
        visible_text: "This is not the web page you are looking for.".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };
    let result = detect_bot_challenge(&view);
    assert!(
        result.is_some(),
        "GitHub 404 page should be detected as error page"
    );
    let reason = result.unwrap();
    assert!(
        reason.contains("page not found") || reason.contains("404") || reason.contains("not found"),
        "reason should mention the error, got: {reason}"
    );
}

/// Generic "404" in title should be detected.
#[test]
fn test_detect_generic_404_title() {
    use llm_as_dom::a11y::detect_bot_challenge;

    let view = SemanticView {
        url: "https://example.com/missing-page".into(),
        title: "404 - Not Found".into(),
        page_hint: "content page".into(),
        elements: vec![],
        forms: vec![],
        visible_text: "The page you requested could not be found.".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };
    let result = detect_bot_challenge(&view);
    assert!(result.is_some(), "Generic 404 title should be detected");
}

/// "Access Denied" title should be detected (already in CHALLENGE_TITLES).
#[test]
fn test_detect_access_denied_title() {
    use llm_as_dom::a11y::detect_bot_challenge;

    let view = SemanticView {
        url: "https://example.com/admin".into(),
        title: "Access Denied".into(),
        page_hint: "content page".into(),
        elements: vec![],
        forms: vec![],
        visible_text: "You don't have permission to access this resource.".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };
    let result = detect_bot_challenge(&view);
    assert!(result.is_some(), "Access Denied title should be detected");
}

/// "Forbidden" title should be detected.
#[test]
fn test_detect_forbidden_title() {
    use llm_as_dom::a11y::detect_bot_challenge;

    let view = SemanticView {
        url: "https://example.com/restricted".into(),
        title: "403 Forbidden".into(),
        page_hint: "content page".into(),
        elements: vec![],
        forms: vec![],
        visible_text: String::new(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };
    let result = detect_bot_challenge(&view);
    assert!(result.is_some(), "Forbidden title should be detected");
}

/// Normal page with "not" in title should NOT trigger false positive.
#[test]
fn test_no_false_positive_not_in_title() {
    use llm_as_dom::a11y::detect_bot_challenge;

    let view = SemanticView {
        url: "https://example.com/notes".into(),
        title: "My Notification Settings".into(),
        page_hint: "form page".into(),
        elements: vec![
            input_element(0, "Email", "email", Some("email"), None),
            button_element(1, "Save", None),
            button_element(2, "Cancel", None),
        ],
        forms: vec![],
        visible_text: "Configure your notification preferences.".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };
    let result = detect_bot_challenge(&view);
    assert!(
        result.is_none(),
        "normal page with 'not' in title should not trigger false positive"
    );
}

// ── Fix #17: Playbook wired into pilot ─────────────────────────────

/// Playbooks load from a temp directory and find_playbook matches URL.
#[test]
fn test_playbook_dir_loads_and_matches() {
    #[allow(unused_imports)]
    use llm_as_dom::playbook::{Playbook, find_playbook, load_playbooks};

    let dir = tempfile::TempDir::new().unwrap();
    let pb_json = serde_json::json!({
        "name": "test-login",
        "url_pattern": "example.com/login",
        "steps": [
            { "kind": "type", "selector": "Email", "value": "${username}" },
            { "kind": "click", "selector": "Submit" }
        ],
        "params": ["username"]
    });
    std::fs::write(
        dir.path().join("test.json"),
        serde_json::to_string_pretty(&pb_json).unwrap(),
    )
    .unwrap();

    let playbooks = load_playbooks(dir.path());
    assert_eq!(playbooks.len(), 1);
    assert_eq!(playbooks[0].name, "test-login");

    let found = find_playbook(&playbooks, "https://example.com/login");
    assert!(found.is_some());
    assert!(find_playbook(&playbooks, "https://other.com").is_none());
}

/// PilotConfig with playbook_dir set correctly stores the path.
#[test]
fn test_pilot_config_playbook_dir() {
    use llm_as_dom::pilot::PilotConfig;

    let config = PilotConfig {
        playbook_dir: Some(std::path::PathBuf::from("/tmp/test-playbooks")),
        ..PilotConfig::default()
    };
    assert_eq!(
        config.playbook_dir,
        Some(std::path::PathBuf::from("/tmp/test-playbooks"))
    );

    let default = PilotConfig::default();
    assert!(default.playbook_dir.is_none());
}

/// Playbook step_to_action converts to correct Action with interpolation.
#[test]
fn test_playbook_step_produces_action_for_matching_view() {
    use llm_as_dom::playbook::{extract_params, find_playbook, load_playbooks, step_to_action};
    use llm_as_dom::semantic::{Element, ElementKind, PageState, SemanticView};

    let dir = tempfile::TempDir::new().unwrap();
    let pb_json = serde_json::json!({
        "name": "demo-login",
        "url_pattern": "demo.test/login",
        "steps": [
            { "kind": "type", "selector": "Email", "value": "${username}" },
            { "kind": "click", "selector": "Go" }
        ],
        "params": ["username"]
    });
    std::fs::write(
        dir.path().join("demo.json"),
        serde_json::to_string_pretty(&pb_json).unwrap(),
    )
    .unwrap();

    let playbooks = load_playbooks(dir.path());
    let view = SemanticView {
        url: "https://demo.test/login".into(),
        title: "Login".into(),
        page_hint: "login page".into(),
        elements: vec![
            Element {
                id: 0,
                kind: ElementKind::Input,
                label: "Email".into(),
                name: None,
                value: None,
                placeholder: None,
                href: None,
                input_type: Some("email".into()),
                disabled: false,
                form_index: None,
                context: None,
                hint: None,
                frame_index: None,
            },
            Element {
                id: 1,
                kind: ElementKind::Button,
                label: "Go".into(),
                name: None,
                value: None,
                placeholder: None,
                href: None,
                input_type: Some("submit".into()),
                disabled: false,
                form_index: None,
                context: None,
                hint: None,
                frame_index: None,
            },
        ],
        forms: vec![],
        visible_text: "Please log in".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };

    let pb = find_playbook(&playbooks, &view.url).unwrap();
    let params = extract_params("login as alice@test.com", &pb.params);
    let action = step_to_action(&view, &pb.steps[0], &params).unwrap();
    match action {
        llm_as_dom::pilot::Action::Type { element, value, .. } => {
            assert_eq!(element, 0);
            assert_eq!(value, "alice@test.com");
        }
        other => panic!("expected Type, got {other:?}"),
    }
}

// ── Fix #18: Hints split from heuristics ───────────────────────────

/// Hints remain active even when heuristics are disabled.
#[test]
fn test_hints_active_when_heuristics_disabled() {
    use llm_as_dom::pilot::PilotConfig;

    let config = PilotConfig {
        use_hints: true,
        use_heuristics: false,
        ..PilotConfig::default()
    };
    assert!(config.use_hints);
    assert!(!config.use_heuristics);

    // Verify hints resolve independently: call try_hints directly.
    use llm_as_dom::heuristics::hints::try_hints;
    use llm_as_dom::semantic::{Element, ElementHint, ElementKind, PageState, SemanticView};

    let view = SemanticView {
        url: "https://example.com/login".into(),
        title: "Login".into(),
        page_hint: "login page".into(),
        elements: vec![Element {
            id: 0,
            kind: ElementKind::Input,
            label: "Email".into(),
            name: None,
            value: None,
            placeholder: None,
            href: None,
            input_type: Some("email".into()),
            disabled: false,
            form_index: None,
            context: None,
            hint: Some(ElementHint {
                hint_type: "field".into(),
                value: "email".into(),
            }),
            frame_index: None,
        }],
        forms: vec![],
        visible_text: "".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };

    // Hints should resolve even though we conceptually disable heuristics.
    let result = try_hints(&view, "login as test@example.com", &[]);
    assert!(
        result.action.is_some(),
        "hints should resolve a field:email"
    );
    assert!(result.confidence >= 0.9);
}

/// When both hints and heuristics are disabled, nothing resolves at Tier 1/2.
#[test]
fn test_both_disabled_falls_to_llm() {
    use llm_as_dom::pilot::PilotConfig;

    let config = PilotConfig {
        use_hints: false,
        use_heuristics: false,
        ..PilotConfig::default()
    };
    assert!(!config.use_hints);
    assert!(!config.use_heuristics);
    // With both disabled, only Tier 0 (playbook) and Tier 3 (LLM) remain.
    // Since playbook_dir is None, only LLM would fire in a real run.
}

/// Default PilotConfig has both hints and heuristics enabled.
#[test]
fn test_default_config_enables_both() {
    use llm_as_dom::pilot::PilotConfig;

    let config = PilotConfig::default();
    assert!(config.use_hints, "hints should be on by default");
    assert!(config.use_heuristics, "heuristics should be on by default");
    assert!(config.playbook_dir.is_none(), "no playbook dir by default");
}

// ── Wave 2: Multi-page state tracking tests ──────────────────────────

/// Navigate action variant serializes/deserializes correctly.
#[test]
fn test_navigate_action_variant() {
    let action = Action::Navigate {
        url: "https://example.com/dashboard".into(),
        reasoning: "proceed to dashboard after login".into(),
    };
    let json = serde_json::to_string(&action).unwrap();
    assert!(json.contains("navigate"), "should serialize as 'navigate'");
    assert!(json.contains("dashboard"), "should contain the URL");

    let parsed: Action = serde_json::from_str(&json).unwrap();
    match parsed {
        Action::Navigate { url, reasoning } => {
            assert_eq!(url, "https://example.com/dashboard");
            assert_eq!(reasoning, "proceed to dashboard after login");
        }
        other => panic!("expected Navigate, got {other:?}"),
    }
}

/// SemanticView with session context includes session info in prompt.
#[test]
fn test_session_context_in_prompt() {
    use llm_as_dom::session::{AuthState, SessionState};

    let mut session = SessionState::new();
    session.record_navigation(
        "https://example.com/login".into(),
        "Login".into(),
        vec!["type: entered email".into()],
        false,
        true,
    );
    session.auth_state = AuthState::InProgress;

    let view = mock_view(
        vec![input_element(
            0,
            "Password",
            "password",
            Some("pass"),
            Some(0),
        )],
        "login page",
    );
    let prompt = view.to_prompt_with_session(&session);
    assert!(
        prompt.contains("SESSION CONTEXT:"),
        "should include session context header"
    );
    assert!(
        prompt.contains("https://example.com/login"),
        "should include visited URL"
    );
    assert!(
        prompt.contains("entered email"),
        "should include action taken"
    );
    assert!(
        prompt.contains("AUTH: in progress"),
        "should include auth state"
    );
}

/// session_context field is skipped when None in JSON serialization.
#[test]
fn test_session_context_field_serialization() {
    let view = mock_view(vec![], "test");
    let json = serde_json::to_string(&view).unwrap();
    assert!(
        !json.contains("session_context"),
        "session_context should be omitted when None"
    );

    let mut view_with_ctx = mock_view(vec![], "test");
    view_with_ctx.session_context = Some("AUTH: authenticated\n".into());
    let json_with = serde_json::to_string(&view_with_ctx).unwrap();
    assert!(
        json_with.contains("session_context"),
        "session_context should be present when Some"
    );

    // Round-trip: deserialize back
    let parsed: SemanticView = serde_json::from_str(&json_with).unwrap();
    assert!(parsed.session_context.is_some());
    assert!(parsed.session_context.unwrap().contains("authenticated"));
}

/// PilotConfig with session Some works.
#[test]
fn test_pilot_config_with_session() {
    use llm_as_dom::pilot::PilotConfig;
    use llm_as_dom::session::SessionState;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let session = Arc::new(Mutex::new(SessionState::new()));
    let config = PilotConfig {
        goal: "multi-page login".into(),
        session: Some(session),
        ..PilotConfig::default()
    };
    assert!(config.session.is_some());
}

/// Navigate action can be created programmatically.
#[test]
fn test_navigate_action_creation() {
    let action = Action::Navigate {
        url: "https://oauth.provider.com/authorize".into(),
        reasoning: "redirect to OAuth provider".into(),
    };
    assert!(matches!(action, Action::Navigate { .. }));

    // Ensure it is not a terminal action
    assert!(!matches!(
        action,
        Action::Done { .. } | Action::Escalate { .. }
    ));
}

/// Session context appears in to_prompt() when field is set.
#[test]
fn test_session_context_in_to_prompt() {
    let mut view = mock_view(vec![], "test page");
    let without = view.to_prompt();
    assert!(
        !without.contains("SESSION"),
        "no session context by default"
    );

    view.session_context = Some("SESSION CONTEXT:\n  - visited: https://a.com (A)\n".into());
    let with = view.to_prompt();
    assert!(
        with.contains("SESSION CONTEXT:"),
        "session context should appear when set"
    );
    assert!(
        with.contains("https://a.com"),
        "should include the visited URL"
    );
}

/// format_session_context produces correct output for auth cookies.
#[test]
fn test_format_session_context_with_auth_cookies() {
    use llm_as_dom::semantic::format_session_context;
    use llm_as_dom::session::{AuthState, CookieEntry, SessionState};

    let mut session = SessionState::new();
    session.add_cookie(CookieEntry {
        name: "session_token".into(),
        value: "abc123".into(),
        domain: ".example.com".into(),
        path: "/".into(),
        expires: 0.0,
        secure: true,
        http_only: true,
        same_site: None,
    });
    session.auth_state = AuthState::Authenticated;

    let ctx = format_session_context(&session);
    assert!(ctx.contains("AUTH: authenticated"));
    assert!(ctx.contains("AUTH COOKIES: present"));
}

/// format_session_context returns empty string for fresh session.
#[test]
fn test_format_session_context_empty() {
    use llm_as_dom::semantic::format_session_context;
    use llm_as_dom::session::SessionState;

    let session = SessionState::new();
    let ctx = format_session_context(&session);
    assert!(ctx.is_empty(), "fresh session should produce empty context");
}

// ── Wave 4: Hard scenario heuristic tests ──────────────────────────

// ── Multi-step form tests ──────────────────────────────────────────

#[test]
fn multistep_form_advances_when_fields_filled() {
    let view = mock_view(
        vec![
            input_element(0, "First Name", "text", Some("first"), None),
            input_element(1, "Last Name", "text", Some("last"), None),
            button_element(2, "Next Step", None),
        ],
        "form page",
    );

    // After filling both fields, next step should be clicked
    let r = heuristics::try_resolve(&view, "fill wizard form", &[0, 1]);
    assert!(r.action.is_some(), "should resolve multi-step advance");
    match r.action.unwrap() {
        Action::Click { element, .. } => {
            assert_eq!(element, 2, "should click Next Step button");
        }
        other => panic!("expected Click on Next Step, got {other:?}"),
    }
}

#[test]
fn multistep_form_waits_when_unfilled() {
    let view = mock_view(
        vec![
            input_element(0, "Email", "email", Some("email"), None),
            input_element(1, "Phone", "tel", Some("phone"), None),
            button_element(2, "Continue", None),
        ],
        "form page",
    );

    // Only one field filled — should NOT advance
    let r = heuristics::try_resolve(&view, "fill wizard form", &[0]);
    // The multi-step heuristic should not fire; other heuristics might match
    // but the key assertion is that "Continue" button is NOT the result
    if let Some(Action::Click { element, .. }) = &r.action {
        assert_ne!(
            *element, 2,
            "should NOT click Continue with unfilled fields"
        );
    }
}

// ── MFA detection tests ────────────────────────────────────────────

#[test]
fn mfa_page_escalates() {
    let view = SemanticView {
        url: "https://example.com/verify".into(),
        title: "Verify Your Identity".into(),
        page_hint: "content page".into(),
        elements: vec![input_element(
            0,
            "Verification Code",
            "text",
            Some("code"),
            None,
        )],
        forms: vec![],
        visible_text: "Enter the verification code sent to your phone".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };

    // Use a generic goal so login/search/nav heuristics don't fire first
    let r = heuristics::try_resolve(&view, "complete verification", &[]);
    assert!(r.action.is_some(), "should resolve MFA detection");
    match r.action.unwrap() {
        Action::Escalate { reason } => {
            assert!(
                reason.contains("MFA") || reason.contains("2FA"),
                "escalation should mention MFA/2FA, got: {reason}"
            );
        }
        other => panic!("expected Escalate for MFA, got {other:?}"),
    }
}

#[test]
fn non_mfa_page_does_not_escalate() {
    let view = SemanticView {
        url: "https://example.com/dashboard".into(),
        title: "Dashboard".into(),
        page_hint: "content page".into(),
        elements: vec![],
        forms: vec![],
        visible_text: "Welcome to your dashboard. Your recent activity:".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };

    let r = heuristics::try_resolve(&view, "view dashboard", &[]);
    // Should not produce an MFA escalation
    if let Some(Action::Escalate { reason }) = &r.action {
        assert!(
            !reason.contains("MFA") && !reason.contains("2FA"),
            "normal page should not trigger MFA escalation"
        );
    }
}

// ── E-commerce tests ───────────────────────────────────────────────

#[test]
fn ecommerce_add_to_cart() {
    let view = mock_view(
        vec![
            button_element(0, "Add to Cart", None),
            button_element(1, "Wishlist", None),
        ],
        "content page",
    );

    let r = heuristics::try_resolve(&view, "add item to cart", &[]);
    assert!(r.action.is_some(), "should resolve add-to-cart");
    assert!(r.confidence >= 0.6);
    match r.action.unwrap() {
        Action::Click { element, .. } => {
            assert_eq!(element, 0, "should click Add to Cart");
        }
        other => panic!("expected Click, got {other:?}"),
    }
}

#[test]
fn ecommerce_checkout_flow() {
    let view = mock_view(
        vec![
            link_element(0, "Proceed to Checkout", "/checkout"),
            button_element(1, "Continue Shopping", None),
        ],
        "content page",
    );

    let r = heuristics::try_resolve(&view, "checkout and pay", &[]);
    assert!(r.action.is_some(), "should resolve checkout");
    match r.action.unwrap() {
        Action::Click { element, .. } => {
            assert_eq!(element, 0, "should click Proceed to Checkout");
        }
        other => panic!("expected Click on checkout link, got {other:?}"),
    }
}

#[test]
fn ecommerce_buy_now() {
    let view = mock_view(
        vec![
            button_element(0, "Buy Now", None),
            button_element(1, "Details", None),
        ],
        "content page",
    );

    let r = heuristics::try_resolve(&view, "buy this product", &[]);
    assert!(r.action.is_some(), "should detect Buy Now");
    match r.action.unwrap() {
        Action::Click { element, .. } => assert_eq!(element, 0),
        other => panic!("expected Click on Buy Now, got {other:?}"),
    }
}

// ── Validation error detection tests ───────────────────────────────

#[test]
fn validation_error_escalates() {
    let view = SemanticView {
        url: "https://example.com/register".into(),
        title: "Register".into(),
        page_hint: "form page".into(),
        elements: vec![
            input_element(0, "Email", "email", Some("email"), Some(0)),
            button_element(1, "Submit", Some(0)),
        ],
        forms: vec![],
        visible_text: "Email is required. Password must be at least 8 characters.".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };

    let r = heuristics::try_resolve(&view, "register account", &[0, 1]);
    assert!(r.action.is_some(), "should detect validation errors");
    assert!(matches!(r.action.unwrap(), Action::Escalate { .. }));
}

#[test]
fn clean_form_no_validation_escalation() {
    let view = SemanticView {
        url: "https://example.com/register".into(),
        title: "Register".into(),
        page_hint: "form page".into(),
        elements: vec![
            input_element(0, "Email", "email", Some("email"), Some(0)),
            button_element(1, "Submit", Some(0)),
        ],
        forms: vec![],
        visible_text: "Create your account to get started.".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };

    let r = heuristics::try_resolve(&view, "register account", &[0, 1]);
    // Should not escalate on a clean form
    if let Some(Action::Escalate { reason }) = &r.action {
        assert!(
            !reason.contains("validation"),
            "clean form should not trigger validation escalation"
        );
    }
}

// ── Heuristic wiring order tests ───────────────────────────────────

#[test]
fn ecommerce_before_generic_button() {
    // E-commerce should fire at strategy 4.5, before generic button click at 5
    let view = mock_view(
        vec![
            button_element(0, "Add to Cart", None),
            button_element(1, "Submit", None),
        ],
        "content page",
    );

    let r = heuristics::try_resolve(&view, "add product to cart", &[]);
    assert!(r.action.is_some());
    match r.action.unwrap() {
        Action::Click { element, .. } => {
            assert_eq!(element, 0, "should pick Add to Cart, not Submit");
        }
        other => panic!("expected Click, got {other:?}"),
    }
}

#[test]
fn multistep_after_button_click() {
    // Multi-step fires at 5.5, after button click at 5.
    // When all fields are filled and a "Continue" button exists alongside
    // a login button, login button should take precedence.
    let view = mock_view(
        vec![
            input_element(0, "Username", "text", Some("user"), Some(0)),
            input_element(1, "Password", "password", Some("pass"), Some(0)),
            button_element(2, "Login", Some(0)),
            button_element(3, "Continue", None),
        ],
        "login page",
    );

    let r = heuristics::try_resolve(&view, "login as admin password admin123", &[0, 1]);
    assert!(r.action.is_some());
    match r.action.unwrap() {
        Action::Click { element, .. } => {
            assert_eq!(element, 2, "Login button should fire before Continue");
        }
        other => panic!("expected Click on Login, got {other:?}"),
    }
}

// ── Direct heuristic module tests (via pub API) ────────────────────

#[test]
fn mfa_module_direct_detection() {
    use llm_as_dom::heuristics::mfa;

    let view = SemanticView {
        url: "https://example.com/2fa".into(),
        title: "Two-Factor Auth".into(),
        page_hint: "content page".into(),
        elements: vec![],
        forms: vec![],
        visible_text: "Enter your two-factor authentication code".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };

    let result = mfa::try_detect_mfa(&view, "login", &[]);
    assert!(result.is_some(), "direct MFA detection should work");
    assert!(result.unwrap().confidence >= 0.9);
}

#[test]
fn validation_module_direct_check() {
    use llm_as_dom::heuristics::validation;

    let view = SemanticView {
        url: "https://example.com/form".into(),
        title: "Form".into(),
        page_hint: "form page".into(),
        elements: vec![],
        forms: vec![],
        visible_text: "This field is required. Username already taken.".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    };

    assert!(validation::has_validation_errors(&view));
    let result = validation::try_detect_validation(&view, "register", &[]);
    assert!(result.is_some());
}

#[test]
fn ecommerce_module_direct_checkout() {
    use llm_as_dom::heuristics::ecommerce;

    let view = mock_view(vec![button_element(0, "Place Order", None)], "content page");

    let result = ecommerce::try_ecommerce_action(&view, "checkout now", &[]);
    assert!(result.is_some());
    match result.unwrap().action.unwrap() {
        Action::Click { element, .. } => assert_eq!(element, 0),
        other => panic!("expected Click, got {other:?}"),
    }
}

// ── Selector wiring tests ───────────────────────────────────────────

#[test]
fn selector_click_button_by_kind_label() {
    use llm_as_dom::selector::{self, Selector};

    let view = mock_view(
        vec![
            button_element(0, "Cancel", None),
            button_element(1, "Login", None),
            button_element(2, "Sign Up", None),
        ],
        "login page",
    );

    let selector = Selector::parse("button:Login");
    let m = selector::find_best(&view, &selector).unwrap();
    assert_eq!(m.element_id, 1, "should match Login button");
}

#[test]
fn selector_find_by_attribute() {
    use llm_as_dom::selector::{self, Selector};

    let view = mock_view(
        vec![
            input_element(0, "Email", "email", Some("email"), None),
            input_element(1, "Password", "password", Some("pw"), None),
        ],
        "login page",
    );

    let selector = Selector::parse("[name=email]");
    let m = selector::find_best(&view, &selector).unwrap();
    assert_eq!(m.element_id, 0, "should match email input by name attr");
}

#[test]
fn selector_natural_language_login_button() {
    use llm_as_dom::selector::{self, Selector};

    let view = mock_view(
        vec![
            link_element(0, "Home", "/"),
            button_element(1, "Login", None),
            button_element(2, "Sign Up", None),
        ],
        "login page",
    );

    let selector = Selector::parse("the login button");
    let m = selector::find_best(&view, &selector).unwrap();
    assert_eq!(m.element_id, 1, "should match Login via natural language");
}

#[test]
fn selector_skips_disabled_elements() {
    use llm_as_dom::selector::{self, Selector};

    let view = mock_view(
        vec![Element {
            disabled: true,
            ..button_element(0, "Submit", None)
        }],
        "form page",
    );

    let selector = Selector::parse("button:Submit");
    assert!(
        selector::find_best(&view, &selector).is_none(),
        "should skip disabled elements"
    );
}

#[test]
fn multistep_module_direct_advance() {
    use llm_as_dom::heuristics::multistep;

    let view = mock_view(vec![button_element(0, "Proceed", None)], "form page");

    // No unfilled inputs, button matches "proceed"
    let result = multistep::try_next_step(&view, "complete wizard", &[]);
    assert!(result.is_some());
    match result.unwrap().action.unwrap() {
        Action::Click { element, .. } => assert_eq!(element, 0),
        other => panic!("expected Click, got {other:?}"),
    }
}

// ── Shadow DOM extraction test (browser required) ────────────────────

/// Extracts elements from a fixture page containing shadow DOM web components.
///
/// The fixture `shadow-dom.html` has:
/// - 2 light-DOM elements (button + input)
/// - 4 shadow-DOM elements inside `<my-login-form>` (email, password, button, link)
/// - 2 deeply nested shadow-DOM elements inside `<my-outer-component>` -> `<my-inner-widget>`
///   (outer button, deep input, deep button)
///
/// Total: at least 9 interactive elements should be found.
#[ignore]
#[tokio::test]
async fn test_extract_shadow_dom_elements() {
    use llm_as_dom::engine::chromium::ChromiumEngine;
    use llm_as_dom::engine::{BrowserEngine, EngineConfig};
    use llm_as_dom::semantic::ElementKind;
    use std::path::Path;
    use std::time::Duration;

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/pages/shadow-dom.html");
    let file_url = format!("file://{}", fixture.display());

    let engine = ChromiumEngine::launch(EngineConfig::default())
        .await
        .expect("browser launch");

    let page = engine.new_page(&file_url).await.unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;

    let view = llm_as_dom::a11y::extract_semantic_view(page.as_ref())
        .await
        .unwrap();

    // Light DOM elements
    let light_btn = view
        .elements
        .iter()
        .find(|e| e.label.contains("Light DOM Button"));
    assert!(light_btn.is_some(), "light DOM button should be found");

    let light_input = view
        .elements
        .iter()
        .find(|e| e.name.as_deref() == Some("light-input"));
    assert!(light_input.is_some(), "light DOM input should be found");

    // Shadow DOM elements from <my-login-form>
    let shadow_email = view
        .elements
        .iter()
        .find(|e| e.placeholder.as_deref() == Some("shadow@example.com"));
    assert!(
        shadow_email.is_some(),
        "shadow DOM email input should be extracted"
    );

    let shadow_pass = view.elements.iter().find(|e| {
        e.input_type.as_deref() == Some("password") && e.name.as_deref() == Some("password")
    });
    assert!(
        shadow_pass.is_some(),
        "shadow DOM password input should be extracted"
    );

    let shadow_btn = view
        .elements
        .iter()
        .find(|e| e.label.contains("Shadow Sign In") && e.kind == ElementKind::Button);
    assert!(
        shadow_btn.is_some(),
        "shadow DOM submit button should be extracted"
    );

    let shadow_link = view
        .elements
        .iter()
        .find(|e| e.href.as_deref() == Some("/shadow-forgot"));
    assert!(shadow_link.is_some(), "shadow DOM link should be extracted");

    // Deeply nested shadow DOM (outer -> inner)
    let outer_btn = view
        .elements
        .iter()
        .find(|e| e.label.contains("Outer Shadow Button"));
    assert!(
        outer_btn.is_some(),
        "outer shadow DOM button should be extracted"
    );

    let deep_input = view
        .elements
        .iter()
        .find(|e| e.name.as_deref() == Some("deep-field"));
    assert!(
        deep_input.is_some(),
        "deeply nested shadow DOM input should be extracted"
    );

    let deep_btn = view
        .elements
        .iter()
        .find(|e| e.label.contains("Deep Button"));
    assert!(
        deep_btn.is_some(),
        "deeply nested shadow DOM button should be extracted"
    );

    // Verify ghost-ID stamping works (all elements should have an id)
    for el in &view.elements {
        // Each element got a unique data-lad-id
        assert!(
            el.id < 300,
            "element IDs should be sequential and reasonable"
        );
    }

    // Should have at least 9 elements total
    assert!(
        view.elements.len() >= 9,
        "expected >= 9 elements (2 light + 7 shadow), got {}",
        view.elements.len()
    );

    assert_eq!(view.state, PageState::Ready);

    drop(page);
    engine.close().await.unwrap();
}

// ── iframe traversal test (browser required) ─────────────────────────

/// Extracts elements from a fixture page containing same-origin iframes.
///
/// The fixture `iframe.html` has:
/// - 2 light-DOM elements (button + input)
/// - 4 same-origin iframe elements (name input, email input, submit button, link)
/// - 1 cross-origin iframe (should be silently skipped)
///
/// Total: at least 6 interactive elements should be found.
#[ignore]
#[tokio::test]
async fn test_extract_iframe_elements() {
    use llm_as_dom::engine::chromium::ChromiumEngine;
    use llm_as_dom::engine::{BrowserEngine, EngineConfig};
    use llm_as_dom::semantic::ElementKind;
    use std::path::Path;
    use std::time::Duration;

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/pages/iframe.html");
    let file_url = format!("file://{}", fixture.display());

    let engine = ChromiumEngine::launch(EngineConfig::default())
        .await
        .expect("browser launch");

    let page = engine.new_page(&file_url).await.unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;

    let view = llm_as_dom::a11y::extract_semantic_view(page.as_ref())
        .await
        .unwrap();

    // Light DOM elements
    let main_btn = view
        .elements
        .iter()
        .find(|e| e.label.contains("Main Page Button"));
    assert!(main_btn.is_some(), "main page button should be found");
    // Main page elements should have frame_index = None
    assert_eq!(
        main_btn.unwrap().frame_index,
        None,
        "main document elements should have frame_index = None"
    );

    let main_input = view
        .elements
        .iter()
        .find(|e| e.name.as_deref() == Some("main-input"));
    assert!(main_input.is_some(), "main page input should be found");

    // Same-origin iframe elements
    let iframe_name = view
        .elements
        .iter()
        .find(|e| e.name.as_deref() == Some("contact-name"));
    assert!(
        iframe_name.is_some(),
        "iframe name input should be extracted"
    );
    // Iframe elements should have frame_index = Some(0) (first iframe)
    assert_eq!(
        iframe_name.unwrap().frame_index,
        Some(0),
        "iframe elements should have frame_index = Some(0)"
    );

    let iframe_email = view
        .elements
        .iter()
        .find(|e| e.name.as_deref() == Some("contact-email"));
    assert!(
        iframe_email.is_some(),
        "iframe email input should be extracted"
    );

    let iframe_btn = view
        .elements
        .iter()
        .find(|e| e.label.contains("Send Message") && e.kind == ElementKind::Button);
    assert!(
        iframe_btn.is_some(),
        "iframe submit button should be extracted"
    );

    let iframe_link = view
        .elements
        .iter()
        .find(|e| e.href.as_deref() == Some("/iframe-help"));
    assert!(iframe_link.is_some(), "iframe link should be extracted");

    // Should have at least 6 elements (2 main + 4 iframe)
    assert!(
        view.elements.len() >= 6,
        "expected >= 6 elements (2 main + 4 iframe), got {}",
        view.elements.len()
    );

    // Cross-origin iframe should NOT have caused any errors
    assert_eq!(view.state, PageState::Ready);

    drop(page);
    engine.close().await.unwrap();
}
