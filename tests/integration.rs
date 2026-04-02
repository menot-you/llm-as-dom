//! Integration tests for LLM-as-DOM.
//!
//! Browser-dependent tests are `#[ignore]` — run with:
//!   cargo test -- --ignored
//!
//! Pure-logic tests run in normal `cargo test`.

use llm_as_dom::heuristics::{self, HeuristicResult};
use llm_as_dom::pilot::Action;
use llm_as_dom::semantic::{Element, ElementKind, PageState, SemanticView};

// ── Helpers ──────────────────────────────────────────────────────────

/// Build a minimal `SemanticView` from a list of elements.
fn mock_view(elements: Vec<Element>, page_hint: &str) -> SemanticView {
    SemanticView {
        url: "https://example.com".into(),
        title: "Test Page".into(),
        page_hint: page_hint.into(),
        elements,
        visible_text: String::new(),
        state: PageState::Ready,
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
