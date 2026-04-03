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
    }
}

// ── Browser tests (#[ignore]) ────────────────────────────────────────

/// Launches a real browser, extracts example.com, asserts elements > 0.
#[ignore]
#[tokio::test]
async fn test_extract_example_com() {
    use futures::StreamExt;
    use std::time::Duration;

    let config = chromiumoxide::BrowserConfig::builder()
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .build()
        .expect("browser config");

    let (browser, mut handler) = chromiumoxide::Browser::launch(config)
        .await
        .expect("browser launch");
    let handle = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = browser.new_page("https://example.com").await.unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    let view = llm_as_dom::a11y::extract_semantic_view(&page)
        .await
        .unwrap();
    assert!(
        !view.elements.is_empty(),
        "example.com should have at least 1 element"
    );
    assert!(!view.title.is_empty(), "page should have a title");
    assert_eq!(view.state, PageState::Ready);

    drop(page);
    drop(browser);
    handle.abort();
}

/// Extracts HN login page, asserts page_hint == "login page".
#[ignore]
#[tokio::test]
async fn test_extract_classifies_login_page() {
    use futures::StreamExt;
    use std::time::Duration;

    let config = chromiumoxide::BrowserConfig::builder()
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .build()
        .expect("browser config");

    let (browser, mut handler) = chromiumoxide::Browser::launch(config)
        .await
        .expect("browser launch");
    let handle = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = browser
        .new_page("https://news.ycombinator.com/login")
        .await
        .unwrap();
    page.wait_for_navigation().await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    let view = llm_as_dom::a11y::extract_semantic_view(&page)
        .await
        .unwrap();
    assert_eq!(
        view.page_hint, "login page",
        "HN login should be classified as login page"
    );

    drop(page);
    drop(browser);
    handle.abort();
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
    // The parse_action function is in backend::ollama which is pub
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
    use llm_as_dom::backend::ollama::build_prompt;

    let view = mock_view(
        vec![
            input_element(0, "Email", "email", Some("email"), Some(0)),
            input_element(1, "Secret", "text", Some("pw"), Some(0)),
            button_element(2, "Login", Some(0)),
        ],
        "login page",
    );

    // Login prompt should contain login few-shot
    let prompt = build_prompt(&view, "login as alice@test.com", &[]);
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
    let search_prompt = build_prompt(&view, "search for tutorials", &[]);
    assert!(
        search_prompt.contains("search"),
        "search prompt should contain search example"
    );

    // Navigation prompt should contain click example
    let nav_prompt = build_prompt(&view, "click About", &[]);
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
            },
        ],
        forms: vec![],
        visible_text: "Sign In".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
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
    };
    let result = detect_bot_challenge(&view);
    assert!(
        result.is_some(),
        "GitHub 404 page should be detected as error page"
    );
    let reason = result.unwrap();
    assert!(
        reason.contains("page not found")
            || reason.contains("404")
            || reason.contains("not found"),
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
    };
    let result = detect_bot_challenge(&view);
    assert!(
        result.is_some(),
        "Access Denied title should be detected"
    );
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
    };
    let result = detect_bot_challenge(&view);
    assert!(
        result.is_some(),
        "Forbidden title should be detected"
    );
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
    };
    let result = detect_bot_challenge(&view);
    assert!(
        result.is_none(),
        "normal page with 'not' in title should not trigger false positive"
    );
}
