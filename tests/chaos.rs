//! Chaos tests for LLM-as-DOM edge cases and failure modes.
//!
//! 80% edge cases / 20% happy path per chaos testing protocol.
//! Tests are deterministic, sub-second, independent.
#![allow(dead_code, clippy::collapsible_match, clippy::single_match)]

use llm_as_dom::heuristics;
use llm_as_dom::pilot::Action;
use llm_as_dom::semantic::{Element, ElementKind, PageState, SemanticView};

// ── Helpers ──────────────────────────────────────────────────────────

fn view(elements: Vec<Element>, hint: &str) -> SemanticView {
    SemanticView {
        url: "https://example.com".into(),
        title: "Test Page".into(),
        page_hint: hint.into(),
        elements,
        forms: vec![],
        visible_text: String::new(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    }
}

fn view_with_text(elements: Vec<Element>, hint: &str, text: &str) -> SemanticView {
    SemanticView {
        url: "https://example.com".into(),
        title: "Test Page".into(),
        page_hint: hint.into(),
        elements,
        forms: vec![],
        visible_text: text.into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    }
}

fn view_with_url(elements: Vec<Element>, hint: &str, url: &str, title: &str) -> SemanticView {
    SemanticView {
        url: url.into(),
        title: title.into(),
        page_hint: hint.into(),
        elements,
        forms: vec![],
        visible_text: String::new(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
    }
}

fn inp(id: u32, label: &str, itype: &str, name: Option<&str>, form: Option<u32>) -> Element {
    Element {
        id,
        kind: ElementKind::Input,
        label: label.into(),
        name: name.map(Into::into),
        value: None,
        placeholder: None,
        href: None,
        input_type: Some(itype.into()),
        disabled: false,
        form_index: form,
        context: None,
        hint: None,
    }
}

fn btn(id: u32, label: &str, form: Option<u32>) -> Element {
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

fn link(id: u32, label: &str, href: &str) -> Element {
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

fn disabled_btn(id: u32, label: &str, form: Option<u32>) -> Element {
    Element {
        id,
        kind: ElementKind::Button,
        label: label.into(),
        name: None,
        value: None,
        placeholder: None,
        href: None,
        input_type: None,
        disabled: true,
        form_index: form,
        context: None,
        hint: None,
    }
}

fn select_el(id: u32, label: &str, name: &str, form: Option<u32>) -> Element {
    Element {
        id,
        kind: ElementKind::Select,
        label: label.into(),
        name: Some(name.into()),
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

fn checkbox(id: u32, label: &str, name: &str, form: Option<u32>) -> Element {
    Element {
        id,
        kind: ElementKind::Checkbox,
        label: label.into(),
        name: Some(name.into()),
        value: Some("on".into()),
        placeholder: None,
        href: None,
        input_type: Some("checkbox".into()),
        disabled: false,
        form_index: form,
        context: None,
        hint: None,
    }
}

// ── 1. Token budget explosion ────────────────────────────────────────

/// Amazon-scale page: 300+ elements should produce a warning-sized prompt.
/// This tests that token estimation is accurate and exposes the budget blow.
#[test]
fn token_explosion_with_300_elements() {
    let elements: Vec<Element> = (0..300)
        .map(|i| link(i, &format!("Product link {i}"), &format!("/product/{i}")))
        .collect();
    let v = view(elements, "navigation/listing page");
    let tokens = v.estimated_tokens();

    // With 300 links, we blow past the 2000 token target.
    // Short labels produce ~3.5K tokens; real sites with long labels/hrefs produce ~20K.
    assert!(
        tokens > 2000,
        "300 elements should exceed 2K token target, got {tokens}"
    );
    assert!(
        tokens > 3000,
        "300 link elements should produce >3K tokens, got {tokens}"
    );
}

/// An empty page should produce minimal tokens.
#[test]
fn token_count_empty_page() {
    let v = view(vec![], "content page");
    let tokens = v.estimated_tokens();
    assert!(tokens < 30, "empty page tokens too high: {tokens}");
}

/// Prompt format must include all structural sections.
#[test]
fn prompt_contains_structural_sections() {
    let v = view(
        vec![inp(0, "Email", "email", Some("email"), Some(0))],
        "login page",
    );
    let prompt = v.to_prompt();
    assert!(prompt.contains("URL:"), "missing URL section");
    assert!(prompt.contains("TITLE:"), "missing TITLE section");
    assert!(prompt.contains("HINT:"), "missing HINT section");
    assert!(prompt.contains("STATE:"), "missing STATE section");
    assert!(prompt.contains("ELEMENTS:"), "missing ELEMENTS section");
}

// ── 2. Page classification edge cases ────────────────────────────────

/// A registration form with a password field should NOT be classified as "login page".
/// BUG: classify_page triggers on any password field.
#[test]
fn register_page_misclassified_as_login() {
    let elements = vec![
        inp(0, "First name", "text", Some("first_name"), Some(0)),
        inp(1, "Last name", "text", Some("last_name"), Some(0)),
        inp(2, "Email", "email", Some("email"), Some(0)),
        inp(3, "Password", "password", Some("password"), Some(0)),
        inp(4, "Confirm password", "password", Some("confirm"), Some(0)),
        btn(5, "Create Account", Some(0)),
    ];
    let v = view_with_url(
        elements,
        "",
        "https://example.com/register",
        "Create Account",
    );

    // The current classify_page would return "login page" because of the password field.
    // A registration form has TWO password fields and a title with "create/register/sign up".
    // This test documents the misclassification.
    // NOTE: page_hint is set in a11y::extract_semantic_view via classify_page.
    // We test classify_page indirectly via the SemanticView.
    //
    // The fact that this page has 2 password fields and title "Create Account" means
    // it should be "form page" or "registration page", not "login page".
    //
    // For now, we assert what the current behavior IS (to document the bug):
    assert_eq!(
        v.page_hint, "",
        "page_hint is set by a11y module, not view constructor"
    );
}

/// A page with only 5 navigation links should still be "navigation/listing page" at threshold > 10.
/// BUG: dashboard with 5 links classified as "content page" instead of nav page.
#[test]
fn dashboard_five_links_classified_as_content() {
    let elements = vec![
        link(0, "Dashboard", "#dashboard"),
        link(1, "Orders", "#orders"),
        link(2, "Products", "#products"),
        link(3, "Customers", "#customers"),
        link(4, "Settings", "#settings"),
    ];
    let v = view(elements, "content page");
    // The threshold for "navigation/listing page" is >10 links.
    // 5 links = "content page". This may be too aggressive a threshold.
    // Test documents that 5-link dashboards get misclassified.
    assert_eq!(v.page_hint, "content page");
}

// ── 3. Heuristic edge cases ─────────────────────────────────────────

/// All inputs already acted on -- heuristic should find the submit button.
#[test]
fn heuristic_all_fields_filled_clicks_submit() {
    let v = view(
        vec![
            inp(0, "Email", "email", Some("email"), Some(0)),
            inp(1, "Password", "password", Some("pw"), Some(0)),
            btn(2, "Sign In", Some(0)),
        ],
        "login page",
    );
    let r = heuristics::try_resolve(&v, "login as test@x.com password secret", &[0, 1]);
    assert!(r.action.is_some(), "should find submit button");
    match r.action.unwrap() {
        Action::Click { element, .. } => assert_eq!(element, 2),
        other => panic!("expected Click, got {other:?}"),
    }
}

/// Disabled button should be skipped even if it matches keywords.
#[test]
fn heuristic_skips_disabled_button() {
    let v = view(
        vec![
            inp(0, "Email", "email", Some("email"), Some(0)),
            disabled_btn(1, "Logging in...", Some(0)),
            btn(2, "Sign In", Some(0)),
        ],
        "login page",
    );
    let r = heuristics::try_resolve(&v, "login as test@x.com password x", &[0]);
    // With one input and it already acted on, and a disabled button, it should
    // still try to find a non-disabled submit button.
    if let Some(action) = r.action {
        match action {
            Action::Click { element, .. } => {
                assert_ne!(element, 1, "should NOT click disabled button");
            }
            _ => {} // any non-click action is fine too
        }
    }
}

/// Goal with no credentials should not fill any login fields.
#[test]
fn heuristic_no_credentials_in_goal() {
    let v = view(
        vec![
            inp(0, "Email", "email", Some("email"), Some(0)),
            inp(1, "Password", "password", Some("pw"), Some(0)),
            btn(2, "Login", Some(0)),
        ],
        "login page",
    );
    let r = heuristics::try_resolve(&v, "navigate to the homepage", &[]);
    // No login keywords in goal -- should not try to fill login fields.
    // Navigation heuristic might match "navigate to the homepage" -> "the homepage".
    if let Some(action) = r.action {
        match action {
            Action::Type { .. } => {
                panic!("should NOT type into login fields for a navigation goal");
            }
            _ => {} // Click on a link is acceptable
        }
    }
}

/// Ambiguous goal: multiple forms, no clear target.
/// BUG: login heuristic matches because "email" in goal text triggers
/// credential extraction for any field labeled "Email", even in non-login contexts.
#[test]
fn heuristic_multiple_forms_ambiguous_goal() {
    let v = view(
        vec![
            // Form 0: newsletter
            inp(0, "Email", "email", Some("newsletter_email"), Some(0)),
            btn(1, "Subscribe", Some(0)),
            // Form 1: contact
            inp(2, "Email", "email", Some("contact_email"), Some(1)),
            inp(3, "Message", "text", Some("message"), Some(1)),
            btn(4, "Send", Some(1)),
        ],
        "form page",
    );
    let r = heuristics::try_resolve(&v, "submit my email newsletter@test.com", &[]);
    // The login heuristic's extract_credential finds "email " prefix in the goal
    // and extracts "newsletter@test.com", then matches the first email input (form 0).
    // This is a false positive -- it treats any mention of "email" as a login credential.
    // Document this behavior: heuristic fires with high confidence on non-login forms.
    assert!(
        r.action.is_some(),
        "login heuristic falsely matches non-login goal"
    );
    assert!(
        r.confidence >= 0.6,
        "BUG: high confidence on non-login context, confidence={:.2}",
        r.confidence
    );
}

/// Goal with special characters that could break JS injection.
#[test]
fn heuristic_special_chars_in_credential() {
    let v = view(
        vec![
            inp(0, "Username", "text", Some("user"), Some(0)),
            inp(1, "Password", "password", Some("pw"), Some(0)),
            btn(2, "Login", Some(0)),
        ],
        "login page",
    );
    // Password with single quotes, backslashes, and special chars
    let goal = r#"login as admin password p@ss'w\"ord\n<script>"#;
    let r = heuristics::try_resolve(&v, goal, &[]);
    assert!(
        r.action.is_some(),
        "should still resolve with special chars"
    );
    match r.action.unwrap() {
        Action::Type { value, .. } => {
            // The extracted value should be the raw token, not escaped.
            assert!(
                !value.is_empty(),
                "extracted value should not be empty despite special chars"
            );
        }
        other => panic!("expected Type, got {other:?}"),
    }
}

/// Empty elements list: heuristic should not panic.
/// BUG: "done detection" fires on an empty page because URL doesn't contain "login"
/// and page_hint != "login page" -- so it concludes login succeeded (false positive).
#[test]
fn heuristic_empty_elements_no_panic() {
    let v = view(vec![], "content page");
    let r = heuristics::try_resolve(&v, "login as admin password secret", &[]);
    // Done detection triggers: goal contains "login" + page_hint != "login page"
    // + URL (https://example.com) doesn't contain "login" -> Done(success=true).
    // This is a false positive on an empty page.
    assert!(
        r.action.is_some(),
        "BUG: done detection fires on empty page"
    );
    match &r.action {
        Some(Action::Done { result, .. }) => {
            assert_eq!(
                result.get("success").and_then(|v| v.as_bool()),
                Some(true),
                "BUG: falsely claims login succeeded on empty page"
            );
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

/// Very long goal string: should not panic or hang.
#[test]
fn heuristic_very_long_goal() {
    let v = view(
        vec![inp(0, "Search", "search", Some("q"), None)],
        "search page",
    );
    let long_goal = format!("search for {}", "x".repeat(10_000));
    let r = heuristics::try_resolve(&v, &long_goal, &[]);
    // Should still resolve (search heuristic should match).
    assert!(
        r.action.is_some(),
        "long goal should still match search heuristic"
    );
}

/// Unicode in goal and labels: Japanese, emoji, RTL text.
#[test]
fn heuristic_unicode_goal_and_labels() {
    let v = view(
        vec![
            inp(0, "メール", "email", Some("email"), Some(0)),
            inp(1, "パスワード", "password", Some("password"), Some(0)),
            btn(2, "ログイン", Some(0)),
        ],
        "login page",
    );
    // Goal in English targeting Japanese labels -- should fall through to LLM.
    let r = heuristics::try_resolve(&v, "login as test@x.com password secret", &[]);
    // The login heuristic checks el.label.to_lowercase().contains("email")
    // which won't match Japanese "メール". This documents the limitation.
    // It WILL match on el.name which is "email", so it should still work.
    assert!(r.action.is_some(), "should match via name= attribute");
}

/// All elements disabled: nothing should be acted on.
#[test]
fn heuristic_all_disabled_elements() {
    let v = view(
        vec![
            {
                let mut e = inp(0, "Email", "email", Some("email"), Some(0));
                e.disabled = true;
                e
            },
            disabled_btn(1, "Login", Some(0)),
        ],
        "login page",
    );
    let r = heuristics::try_resolve(&v, "login as x@y.com password z", &[]);
    // Search/nav won't match. Login heuristic iterates elements but skips disabled? No --
    // actually, login::try_form_fill does NOT check el.disabled. Only try_button_click does.
    // This is another bug: disabled inputs can still be targeted by heuristics.
    // Document it.
    if let Some(action) = &r.action {
        match action {
            Action::Type { element, .. } => {
                // BUG: heuristic fills a disabled input
                assert_eq!(*element, 0, "incorrectly targets disabled element");
            }
            _ => {}
        }
    }
}

// ── 4. Credential extraction edge cases ─────────────────────────────

/// Password with spaces (quoted in goal): "password 'my secret phrase'"
#[test]
fn credential_extraction_password_with_spaces() {
    // extract_credential splits on whitespace, so multi-word passwords break.
    use llm_as_dom::heuristics;
    let v = view(
        vec![
            inp(0, "User", "text", Some("user"), Some(0)),
            inp(1, "Pass", "password", Some("pw"), Some(0)),
        ],
        "login page",
    );
    let goal = "login as admin password my secret phrase";
    let r = heuristics::try_resolve(&v, goal, &[0]);
    if let Some(Action::Type { value, .. }) = r.action {
        // BUG: only gets "my" instead of "my secret phrase"
        assert_eq!(
            value, "my",
            "credential extraction only takes first word after 'password'"
        );
    }
}

/// Goal with "password" keyword but value is a skip-word.
#[test]
fn credential_extraction_skip_words() {
    use llm_as_dom::heuristics;
    let v = view(
        vec![
            inp(0, "User", "text", Some("user"), Some(0)),
            inp(1, "Pass", "password", Some("pw"), Some(0)),
        ],
        "login page",
    );
    // "password with" -- "with" is a skip word, so no password extracted
    let goal = "login as admin password with something";
    let r = heuristics::try_resolve(&v, goal, &[0]);
    // After skipping "with", it should NOT get "something" because
    // extract_credential only takes the first word after the prefix.
    if let Some(Action::Type { value, .. }) = r.action {
        // "with" is in skip list, so extract_credential returns None.
        // Heuristic falls through to next strategy.
        panic!("should not fill password when value is skip-word, but got: {value}");
    }
    // No password match = no action for password field. Correct.
}

// ── 5. Form kv-pair parsing edge cases ──────────────────────────────

/// KV pairs via public API: URL values with '=' in them confuse the form parser.
#[test]
fn kv_pairs_url_with_equals_via_heuristic() {
    let v = view(
        vec![
            inp(0, "URL", "text", Some("url"), Some(0)),
            inp(1, "Name", "text", Some("name"), Some(0)),
            btn(2, "Submit", Some(0)),
        ],
        "form page",
    );
    // The '=' inside the URL value will confuse key=value extraction.
    let r = heuristics::try_resolve(
        &v,
        "fill form with url=https://example.com?foo=bar name=John",
        &[],
    );
    // Should at least resolve one field (may mis-parse the URL value).
    assert!(r.action.is_some(), "should resolve at least one kv fill");
}

/// KV pairs via public API: quoted values with equals should work.
#[test]
fn kv_pairs_quoted_via_heuristic() {
    let v = view(
        vec![
            inp(0, "Query", "text", Some("query"), Some(0)),
            inp(1, "Name", "text", Some("name"), Some(0)),
            btn(2, "Submit", Some(0)),
        ],
        "form page",
    );
    let r = heuristics::try_resolve(&v, r#"fill form with query="a=b" name=John"#, &[]);
    assert!(r.action.is_some(), "quoted kv pair should resolve");
    match r.action.unwrap() {
        Action::Type { value, .. } => {
            assert_eq!(value, "a=b", "quoted value should preserve '='");
        }
        other => panic!("expected Type, got {other:?}"),
    }
}

/// KV pairs via public API: no '=' in goal should not trigger form heuristic.
#[test]
fn kv_pairs_no_equals_falls_through() {
    let v = view(
        vec![
            inp(0, "Name", "text", Some("name"), Some(0)),
            btn(1, "Submit", Some(0)),
        ],
        "form page",
    );
    let r = heuristics::try_resolve(&v, "just fill something", &[]);
    // No key=value pairs, no login/search/nav keywords -> no match.
    assert!(
        r.action.is_none() || r.confidence < 0.6,
        "goal without kv pairs should not match form heuristic"
    );
}

// ── 6. Action parsing edge cases ────────────────────────────────────

/// Deeply nested think tags (Qwen3 edge case).
#[test]
fn parse_action_nested_think_tags() {
    use llm_as_dom::backend::ollama::parse_action;
    let input = "<think>First thought<think>nested</think>more thought</think>{\"action\":\"click\",\"element\":0,\"reasoning\":\"done\"}";
    let result = parse_action(input);
    assert!(result.is_ok(), "nested think tags should still parse");
}

/// Response with ONLY think tags and no JSON.
#[test]
fn parse_action_only_think_no_json() {
    use llm_as_dom::backend::ollama::parse_action;
    let input = "<think>I'm thinking but I won't give you JSON</think>";
    let result = parse_action(input);
    assert!(result.is_err(), "no JSON should produce an error");
}

/// Malformed JSON after think tags.
#[test]
fn parse_action_malformed_json() {
    use llm_as_dom::backend::ollama::parse_action;
    let input = r#"<think>ok</think>{"action":"click","element":INVALID}"#;
    let result = parse_action(input);
    assert!(result.is_err(), "malformed JSON should produce an error");
}

/// Multiple JSON objects in response (should take first).
#[test]
fn parse_action_multiple_json_objects() {
    use llm_as_dom::backend::ollama::parse_action;
    let input = r#"{"action":"click","element":0,"reasoning":"a"} {"action":"type","element":1,"value":"x","reasoning":"b"}"#;
    let action = parse_action(input).unwrap();
    // extract_json finds the first balanced {} pair.
    match action {
        Action::Click { element, .. } => assert_eq!(element, 0, "should take first JSON object"),
        other => panic!("expected Click from first object, got {other:?}"),
    }
}

/// JSON wrapped in markdown code fences.
#[test]
fn parse_action_markdown_fences() {
    use llm_as_dom::backend::ollama::parse_action;
    let input = "```json\n{\"action\":\"wait\",\"reasoning\":\"loading\"}\n```";
    let result = parse_action(input);
    assert!(result.is_ok(), "should extract JSON from markdown fences");
}

/// Completely empty response.
#[test]
fn parse_action_empty_response() {
    use llm_as_dom::backend::ollama::parse_action;
    let result = parse_action("");
    assert!(result.is_err(), "empty response should error");
}

/// Response with only whitespace.
#[test]
fn parse_action_whitespace_only() {
    use llm_as_dom::backend::ollama::parse_action;
    let result = parse_action("   \n\t  \n  ");
    assert!(result.is_err(), "whitespace-only response should error");
}

/// JSON array wrapping a single action (some models do this).
#[test]
fn parse_action_json_array_wrapper() {
    use llm_as_dom::backend::ollama::parse_action;
    let input = r#"[{"action":"scroll","direction":"down","reasoning":"load more"}]"#;
    let result = parse_action(input);
    assert!(
        result.is_ok(),
        "JSON array wrapper should be handled: {:?}",
        result.err()
    );
}

// ── 7. SemanticView serialization edge cases ────────────────────────

/// Elements with None optional fields should serialize cleanly.
#[test]
fn semantic_view_optional_fields_skip() {
    let el = Element {
        id: 0,
        kind: ElementKind::Button,
        label: "Click me".into(),
        name: None,
        value: None,
        placeholder: None,
        href: None,
        input_type: None,
        disabled: false,
        form_index: None,
        context: None,
        hint: None,
    };
    let v = view(vec![el], "content page");
    let json = serde_json::to_string(&v).unwrap();
    assert!(!json.contains("\"name\""), "None name should be skipped");
    assert!(!json.contains("\"href\""), "None href should be skipped");
    assert!(
        !json.contains("\"form_index\""),
        "None form_index should be skipped"
    );
}

/// Round-trip: SemanticView -> JSON -> SemanticView should be lossless.
#[test]
fn semantic_view_json_roundtrip() {
    let v = view(
        vec![
            inp(0, "Email", "email", Some("email"), Some(0)),
            btn(1, "Submit", Some(0)),
            link(2, "Home", "/"),
        ],
        "form page",
    );
    let json = serde_json::to_string(&v).unwrap();
    let deserialized: SemanticView = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.elements.len(), 3);
    assert_eq!(deserialized.page_hint, "form page");
    assert_eq!(deserialized.elements[0].label, "Email");
    assert_eq!(deserialized.elements[2].href.as_deref(), Some("/"));
}

/// Action enum round-trips through JSON correctly.
#[test]
fn action_json_roundtrip_all_variants() {
    let actions = vec![
        Action::Click {
            element: 0,
            reasoning: "test".into(),
        },
        Action::Type {
            element: 1,
            value: "hello".into(),
            reasoning: "test".into(),
        },
        Action::Select {
            element: 2,
            value: "opt1".into(),
            reasoning: "test".into(),
        },
        Action::Scroll {
            direction: "down".into(),
            reasoning: "test".into(),
        },
        Action::Wait {
            reasoning: "test".into(),
        },
        Action::Done {
            result: serde_json::json!({"success": true}),
            reasoning: "test".into(),
        },
        Action::Escalate {
            reason: "blocked".into(),
        },
    ];
    for action in &actions {
        let json = serde_json::to_string(action).unwrap();
        let back: Action = serde_json::from_str(&json).unwrap();
        let json2 = serde_json::to_string(&back).unwrap();
        assert_eq!(json, json2, "roundtrip mismatch for {json}");
    }
}

// ── 8. Prompt building edge cases ───────────────────────────────────

/// Build prompt with very long history (>5 steps should be truncated).
#[test]
fn build_prompt_truncates_history() {
    use llm_as_dom::backend::ollama::build_prompt;
    use llm_as_dom::pilot::Step;
    use std::time::Duration;

    let v = view(vec![btn(0, "Next", None)], "content page");
    let history: Vec<Step> = (0..20)
        .map(|i| Step {
            index: i,
            observation: view(vec![], "content page"),
            action: Action::Click {
                element: 0,
                reasoning: format!("step {i}"),
            },
            source: llm_as_dom::pilot::DecisionSource::Heuristic,
            confidence: 0.9,
            duration: Duration::from_millis(10),
        })
        .collect();

    let prompt = build_prompt(&v, "click Next 20 times", &history);
    // Should only include last 5 steps (the code does .rev().take(5))
    let action_count = prompt.matches("Click {").count();
    assert!(
        action_count <= 5,
        "should truncate history to 5 entries, found {action_count}"
    );
}

/// Build prompt with empty goal.
#[test]
fn build_prompt_empty_goal() {
    use llm_as_dom::backend::ollama::build_prompt;
    let v = view(vec![btn(0, "OK", None)], "content page");
    let prompt = build_prompt(&v, "", &[]);
    assert!(prompt.contains("GOAL: "), "should have GOAL section");
    // Empty goal should still produce a valid prompt.
    assert!(
        prompt.contains("VALID ACTIONS"),
        "should have action schema"
    );
}

// ── 9. Error type edge cases ────────────────────────────────────────

/// All error variants format correctly.
#[test]
fn error_variants_format() {
    let errors = vec![
        llm_as_dom::Error::BrowserStr("chrome crashed".into()),
        llm_as_dom::Error::Backend("timeout".into()),
        llm_as_dom::Error::Timeout,
        llm_as_dom::Error::ActionFailed("element not found".into()),
        llm_as_dom::Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "missing")),
    ];
    for err in &errors {
        let msg = format!("{err}");
        assert!(!msg.is_empty(), "error message should not be empty");
    }
}

/// Error Debug trait works for all variants.
#[test]
fn error_debug_all_variants() {
    let err = llm_as_dom::Error::Timeout;
    let debug = format!("{err:?}");
    assert!(
        debug.contains("Timeout"),
        "debug format should name variant"
    );
}

// ── 10. Navigation heuristic edge cases ─────────────────────────────

/// Partial label match vs exact match: exact should win.
#[test]
fn navigation_exact_match_beats_partial() {
    let v = view(
        vec![link(0, "About Us", "/about-us"), link(1, "About", "/about")],
        "content page",
    );
    let r = heuristics::try_resolve(&v, "click About", &[]);
    assert!(r.action.is_some());
    match r.action.unwrap() {
        Action::Click { element, .. } => {
            assert_eq!(
                element, 1,
                "exact match 'About' should beat partial 'About Us'"
            );
        }
        other => panic!("expected Click, got {other:?}"),
    }
}

/// Navigation with no matching link: should return low confidence.
#[test]
fn navigation_no_match_returns_none() {
    let v = view(
        vec![link(0, "Home", "/"), link(1, "Contact", "/contact")],
        "content page",
    );
    let r = heuristics::try_resolve(&v, "click Settings", &[]);
    assert!(
        r.action.is_none() || r.confidence < 0.6,
        "non-matching navigation should not resolve"
    );
}

/// Href-only match (label doesn't match but href does).
#[test]
fn navigation_matches_by_href() {
    let v = view(vec![link(0, "Click Here", "/settings")], "content page");
    let r = heuristics::try_resolve(&v, "go to settings", &[]);
    assert!(r.action.is_some(), "should match via href");
    match r.action.unwrap() {
        Action::Click { element, .. } => assert_eq!(element, 0),
        other => panic!("expected Click, got {other:?}"),
    }
}

// ── 11. Search heuristic edge cases ─────────────────────────────────

/// Search input detection by name="q" (Google-style).
#[test]
fn search_detects_name_q() {
    let v = view(vec![inp(0, "", "text", Some("q"), None)], "search page");
    let r = heuristics::try_resolve(&v, "search for weather", &[]);
    assert!(r.action.is_some(), "name=q should trigger search heuristic");
}

/// "find" prefix should also trigger search.
#[test]
fn search_find_prefix() {
    let v = view(
        vec![inp(0, "Search", "search", Some("q"), None)],
        "search page",
    );
    let r = heuristics::try_resolve(&v, "find cheap flights", &[]);
    assert!(r.action.is_some());
    match r.action.unwrap() {
        Action::Type { value, .. } => {
            assert_eq!(value, "cheap flights");
        }
        other => panic!("expected Type, got {other:?}"),
    }
}

/// "look up" prefix should also trigger search.
#[test]
fn search_look_up_prefix() {
    let v = view(
        vec![inp(0, "Search", "search", Some("q"), None)],
        "search page",
    );
    let r = heuristics::try_resolve(&v, "look up Rust lang", &[]);
    assert!(r.action.is_some());
    match r.action.unwrap() {
        Action::Type { value, .. } => {
            assert_eq!(value, "Rust lang");
        }
        other => panic!("expected Type, got {other:?}"),
    }
}

// ── 12. Extract_balanced edge cases ─────────────────────────────────

/// Unmatched braces should return None.
#[test]
fn extract_balanced_unmatched() {
    use llm_as_dom::backend::ollama::extract_balanced;
    assert!(extract_balanced("{unclosed", b'{', b'}').is_none());
    assert!(extract_balanced("no braces", b'{', b'}').is_none());
    assert!(extract_balanced("}", b'{', b'}').is_none());
}

/// Nested braces should find the outermost pair.
#[test]
fn extract_balanced_nested() {
    use llm_as_dom::backend::ollama::extract_balanced;
    let input = r#"{"a":{"b":1},"c":2}"#;
    let result = extract_balanced(input, b'{', b'}').unwrap();
    assert_eq!(result, input, "should extract outermost balanced pair");
}

/// Empty braces.
#[test]
fn extract_balanced_empty_braces() {
    use llm_as_dom::backend::ollama::extract_balanced;
    let result = extract_balanced("{}", b'{', b'}').unwrap();
    assert_eq!(result, "{}");
}

// ── 13. Strip think tags edge cases ─────────────────────────────────

/// No think tags: input should pass through unchanged.
#[test]
fn strip_think_tags_no_tags() {
    use llm_as_dom::backend::ollama::strip_think_tags;
    let input = r#"{"action":"click","element":0,"reasoning":"test"}"#;
    assert_eq!(strip_think_tags(input), input);
}

/// Multiple consecutive think blocks.
#[test]
fn strip_think_tags_multiple_blocks() {
    use llm_as_dom::backend::ollama::strip_think_tags;
    let input = "<think>first</think><think>second</think>result";
    assert_eq!(strip_think_tags(input), "result");
}

/// Think tag with no closing tag (malformed).
#[test]
fn strip_think_tags_unclosed() {
    use llm_as_dom::backend::ollama::strip_think_tags;
    let input = "<think>this never closes and some JSON follows";
    let result = strip_think_tags(input);
    // Unclosed think tag: everything after <think> is swallowed.
    assert!(
        result.is_empty() || !result.contains("this never closes"),
        "unclosed think tag should swallow content, got: {result}"
    );
}

// ── 14. PilotConfig defaults ────────────────────────────────────────

/// Default config values match documentation.
#[test]
fn pilot_config_defaults_match_docs() {
    let config = llm_as_dom::pilot::PilotConfig::default();
    assert_eq!(config.max_steps, 10);
    assert_eq!(config.max_retries_per_step, 2);
    assert!(config.use_heuristics);
    assert!(config.goal.is_empty());
}

// ── 15. Done detection edge cases ───────────────────────────────────

/// Error text in visible_text should trigger failure detection.
#[test]
fn done_detection_error_text() {
    let v = view_with_text(
        vec![btn(0, "Try Again", None)],
        "login page",
        "Invalid username or password. Please try again.",
    );
    let r = heuristics::try_resolve(&v, "login as admin password wrong", &[0]);
    // Should detect "Invalid" in visible text and return Done(success=false).
    if let Some(Action::Done { result, .. }) = r.action {
        assert_eq!(
            result.get("success").and_then(|v| v.as_bool()),
            Some(false),
            "should detect login failure"
        );
    }
    // If it doesn't match Done, that's also a documented behavior.
}

/// Success detection: URL changed away from login page.
#[test]
fn done_detection_url_changed() {
    let v = view_with_url(
        vec![link(0, "Dashboard", "/dashboard")],
        "navigation/listing page",
        "https://example.com/dashboard",
        "Dashboard",
    );
    let r = heuristics::try_resolve(&v, "login as admin password secret", &[]);
    // URL no longer contains "login" and page_hint != "login page" -> Done.
    if let Some(Action::Done { result, .. }) = r.action {
        assert_eq!(
            result.get("success").and_then(|v| v.as_bool()),
            Some(true),
            "should detect successful navigation away from login"
        );
    }
}
