use super::assertions::{check_assertion, normalize_assertion};
use super::helpers::{check_js_result, extract_origin, key_to_code, same_origin};
use super::params::*;

use llm_as_dom::semantic;

fn empty_view() -> semantic::SemanticView {
    semantic::SemanticView {
        url: String::new(),
        title: String::new(),
        page_hint: String::new(),
        elements: vec![],
        forms: vec![],
        visible_text: String::new(),
        text_blocks: vec![],
        state: semantic::PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
        cards: None,
    }
}

fn make_element(kind: semantic::ElementKind, label: &str) -> semantic::Element {
    semantic::Element {
        id: 1,
        kind,
        label: label.into(),
        name: None,
        value: None,
        placeholder: None,
        href: None,
        input_type: None,
        disabled: false,
        form_index: None,
        context: None,
        hint: None,
        checked: None,
        options: None,
        frame_index: None,
        is_visible: None,
    }
}

#[test]
fn assert_has_email_input() {
    let mut view = empty_view();
    let mut el = make_element(semantic::ElementKind::Input, "Email address");
    el.input_type = Some("email".into());
    view.elements.push(el);

    assert!(check_assertion("has email input", &view, ""));
    assert!(check_assertion("has input email", &view, ""));
}

#[test]
fn assert_has_button_reordered() {
    let mut view = empty_view();
    view.elements.push(make_element(
        semantic::ElementKind::Button,
        "Get Early Access",
    ));

    assert!(check_assertion("has button get early access", &view, ""));
    assert!(check_assertion("has get early access button", &view, ""));
}

#[test]
fn assert_has_button_with_icon() {
    let mut view = empty_view();
    view.elements.push(make_element(
        semantic::ElementKind::Button,
        "Get Early Access \u{203a}",
    ));

    assert!(check_assertion("has button get early access", &view, ""));
}

#[test]
fn assert_has_github_link() {
    let mut view = empty_view();
    let mut el = make_element(semantic::ElementKind::Link, "GitHub");
    el.href = Some("https://github.com/menot-you".into());
    view.elements.push(el);

    assert!(check_assertion("has link github", &view, ""));
    assert!(check_assertion("has github link", &view, ""));
}

#[test]
fn assert_has_link_by_href() {
    let mut view = empty_view();
    let mut el = make_element(semantic::ElementKind::Link, "Star us");
    el.href = Some("https://github.com/menot-you".into());
    view.elements.push(el);

    assert!(check_assertion("has link github", &view, ""));
}

#[test]
fn assert_has_form() {
    let mut view = empty_view();
    view.forms.push(semantic::FormMeta {
        index: 0,
        action: Some("/subscribe".into()),
        method: "POST".into(),
        id: None,
        name: None,
    });

    assert!(check_assertion("has form", &view, ""));
}

#[test]
fn assert_input_matches_by_type() {
    let mut view = empty_view();
    let mut el = make_element(semantic::ElementKind::Input, "");
    el.input_type = Some("email".into());
    view.elements.push(el);

    assert!(check_assertion("has input email", &view, ""));
}

#[test]
fn assert_input_matches_by_placeholder() {
    let mut view = empty_view();
    let mut el = make_element(semantic::ElementKind::Input, "");
    el.placeholder = Some("Enter your email".into());
    view.elements.push(el);

    assert!(check_assertion("has input email", &view, ""));
}

#[test]
fn normalize_assertion_reorders_words() {
    assert_eq!(normalize_assertion("has email input"), "has input email");
    assert_eq!(
        normalize_assertion("has get early access button"),
        "has button get early access"
    );
    assert_eq!(normalize_assertion("has github link"), "has link github");
    assert_eq!(
        normalize_assertion("has button submit"),
        "has button submit"
    );
    assert_eq!(normalize_assertion("has input email"), "has input email");
}

// ── W1/W3 unit tests ─────────────────────────────────────────

#[test]
fn same_origin_matches() {
    assert!(same_origin(
        "https://example.com/foo",
        "https://example.com/bar"
    ));
    assert!(same_origin(
        "http://localhost:3000/a",
        "http://localhost:3000/b"
    ));
}

#[test]
fn same_origin_rejects_different() {
    assert!(!same_origin(
        "https://example.com/foo",
        "https://other.com/foo"
    ));
    assert!(!same_origin(
        "http://localhost:3000",
        "https://localhost:3000"
    ));
    assert!(!same_origin(
        "http://localhost:3000",
        "http://localhost:4000"
    ));
}

#[test]
fn extract_origin_works() {
    assert_eq!(
        extract_origin("https://example.com/path?q=1"),
        Some("https://example.com".to_string())
    );
    assert_eq!(
        extract_origin("http://localhost:8080/foo"),
        Some("http://localhost:8080".to_string())
    );
    assert_eq!(extract_origin("ftp://nope"), None);
}

#[test]
fn check_js_result_ok() {
    // check_js_result expects a Value::String wrapping serialized JSON.
    // This is the format returned by browser JS eval (JSON stringified result).
    let ok = serde_json::json!(r#"{"ok":true}"#);
    assert!(check_js_result(&ok).is_ok());
}

#[test]
fn check_js_result_err() {
    // Error case: stringified JSON containing an "error" key.
    let err = serde_json::json!(r#"{"error":"element 5 not found"}"#);
    let result = check_js_result(&err);
    assert!(result.is_err(), "should detect error in stringified JSON");
}

#[test]
fn check_js_result_object_passthrough() {
    // NOTE: If the value is a raw JSON object (not a string), check_js_result
    // skips the check (as_str() returns None) and returns Ok.
    // This is intentional — browser eval returns stringified JSON.
    let obj = serde_json::json!({"error": "this is an object, not a string"});
    assert!(
        check_js_result(&obj).is_ok(),
        "raw JSON objects bypass the string-parse check — by design"
    );
}

// ── Escape hatch helper tests ────────────────────────────────

#[test]
fn key_to_code_standard_keys() {
    assert_eq!(key_to_code("Enter"), "Enter");
    assert_eq!(key_to_code("Tab"), "Tab");
    assert_eq!(key_to_code("Escape"), "Escape");
    assert_eq!(key_to_code("Backspace"), "Backspace");
    assert_eq!(key_to_code("Delete"), "Delete");
    assert_eq!(key_to_code("Space"), "Space");
    assert_eq!(key_to_code(" "), "Space");
}

#[test]
fn key_to_code_arrow_keys() {
    assert_eq!(key_to_code("ArrowUp"), "ArrowUp");
    assert_eq!(key_to_code("ArrowDown"), "ArrowDown");
    assert_eq!(key_to_code("ArrowLeft"), "ArrowLeft");
    assert_eq!(key_to_code("ArrowRight"), "ArrowRight");
}

#[test]
fn key_to_code_function_keys() {
    assert_eq!(key_to_code("F1"), "F1");
    assert_eq!(key_to_code("F12"), "F12");
}

#[test]
fn key_to_code_unknown_falls_back() {
    assert_eq!(key_to_code("a"), "a");
    assert_eq!(key_to_code("Shift"), "Shift");
}

#[test]
fn key_to_code_navigation_keys() {
    assert_eq!(key_to_code("Home"), "Home");
    assert_eq!(key_to_code("End"), "End");
    assert_eq!(key_to_code("PageUp"), "PageUp");
    assert_eq!(key_to_code("PageDown"), "PageDown");
}

// ── W2: lad_wait assertion reuse tests ──────────────────────

#[test]
fn check_assertion_title_contains() {
    let mut view = empty_view();
    view.title = "Welcome to Dashboard".into();
    assert!(check_assertion("title contains dashboard", &view, ""));
    assert!(!check_assertion("title contains settings", &view, ""));
}

#[test]
fn check_assertion_url_contains() {
    let mut view = empty_view();
    view.url = "https://example.com/dashboard".into();
    assert!(check_assertion("url contains dashboard", &view, ""));
    assert!(!check_assertion("url contains settings", &view, ""));
}

#[test]
fn check_assertion_visible_text_fallback() {
    let mut view = empty_view();
    view.visible_text = "Loading complete. Welcome back, user!".into();
    assert!(check_assertion("welcome back", &view, ""));
    assert!(!check_assertion("error occurred", &view, ""));
}

// ── W2: param defaults ──────────────────────────────────────

#[test]
fn wait_params_defaults() {
    let json = r#"{"condition":"has button submit"}"#;
    let p: WaitParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.timeout_ms, 10_000);
    assert_eq!(p.poll_ms, 500);
}

#[test]
fn network_params_defaults() {
    let json = r#"{}"#;
    let p: NetworkParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.filter, "all");
}

#[test]
fn network_params_custom_filter() {
    let json = r#"{"filter":"auth"}"#;
    let p: NetworkParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.filter, "auth");
}

// ── W3: param parsing tests ─────────────────────────────────

#[test]
fn hover_params_parse() {
    let json = r#"{"element":42}"#;
    let p: HoverParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.element, Some(42));
}

#[test]
fn dialog_params_accept_with_text() {
    let json = r#"{"action":"accept","text":"hello"}"#;
    let p: DialogParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.action, "accept");
    assert_eq!(p.text.as_deref(), Some("hello"));
}

#[test]
fn dialog_params_status_no_text() {
    let json = r#"{"action":"status"}"#;
    let p: DialogParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.action, "status");
    assert!(p.text.is_none());
}

#[test]
fn dialog_params_dismiss() {
    let json = r#"{"action":"dismiss"}"#;
    let p: DialogParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.action, "dismiss");
}

#[test]
fn upload_params_parse() {
    let json = r#"{"element":7,"files":["/tmp/a.png","/tmp/b.pdf"]}"#;
    let p: UploadParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.element, 7);
    assert_eq!(p.files.len(), 2);
    assert_eq!(p.files[0], "/tmp/a.png");
    assert_eq!(p.files[1], "/tmp/b.pdf");
}

#[test]
fn upload_params_single_file() {
    let json = r#"{"element":1,"files":["/tmp/test.csv"]}"#;
    let p: UploadParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.element, 1);
    assert_eq!(p.files.len(), 1);
}

#[test]
fn upload_params_empty_files() {
    let json = r#"{"element":1,"files":[]}"#;
    let p: UploadParams = serde_json::from_str(json).unwrap();
    assert!(p.files.is_empty());
}

// ── FIX-1: SSRF scheme bypass tests (unit) ────────────

#[test]
fn ssrf_file_single_slash_blocked() {
    assert!(!llm_as_dom::sanitize::is_safe_url("file:/etc/passwd"));
}

#[test]
fn ssrf_file_triple_slash_blocked() {
    assert!(!llm_as_dom::sanitize::is_safe_url("file:///etc/passwd"));
}

#[test]
fn ssrf_data_scheme_blocked() {
    assert!(!llm_as_dom::sanitize::is_safe_url(
        "data:text/html,<h1>xss</h1>"
    ));
}

// ── FIX-12: watch interval validation ─────────────────

#[test]
fn watch_params_zero_interval() {
    let json = r#"{"action":"start","url":"https://example.com","interval_ms":0}"#;
    let p: WatchParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.interval_ms, Some(0));
    // The actual validation happens in watch_start() runtime
}

// ── FIX-14: upload path must be absolute ──────────────

#[test]
fn upload_params_relative_path_detected() {
    let json = r#"{"element":1,"files":["./relative/file.txt"]}"#;
    let p: UploadParams = serde_json::from_str(json).unwrap();
    // Validation happens at runtime, but we can assert path checking
    assert!(!std::path::Path::new(&p.files[0]).is_absolute());
}

#[test]
fn upload_params_absolute_path_detected() {
    let json = r#"{"element":1,"files":["/tmp/file.txt"]}"#;
    let p: UploadParams = serde_json::from_str(json).unwrap();
    assert!(std::path::Path::new(&p.files[0]).is_absolute());
}

// ── DX-1: snapshot optional URL ─────────────────────────

#[test]
fn snapshot_params_with_url() {
    let json = r#"{"url":"https://example.com"}"#;
    let p: SnapshotParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.url.as_deref(), Some("https://example.com"));
}

#[test]
fn snapshot_params_without_url() {
    let json = r#"{}"#;
    let p: SnapshotParams = serde_json::from_str(json).unwrap();
    assert!(p.url.is_none());
}

#[test]
fn snapshot_params_null_url() {
    let json = r#"{"url":null}"#;
    let p: SnapshotParams = serde_json::from_str(json).unwrap();
    assert!(p.url.is_none());
}

// ── DX-4: type with press_enter ─────────────────────────

#[test]
fn type_params_default_no_enter() {
    let json = r#"{"element":1,"text":"hello"}"#;
    let p: TypeParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.element, Some(1));
    assert_eq!(p.text, "hello");
    assert!(!p.press_enter);
}

#[test]
fn type_params_with_press_enter() {
    let json = r#"{"element":5,"text":"search query","press_enter":true}"#;
    let p: TypeParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.element, Some(5));
    assert_eq!(p.text, "search query");
    assert!(p.press_enter);
}

// ── DX-5: scroll params ─────────────────────────────────

#[test]
fn scroll_params_defaults() {
    let json = r#"{}"#;
    let p: ScrollParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.direction, "down");
    assert!(p.element.is_none());
    assert_eq!(p.pixels, 600);
}

#[test]
fn scroll_params_custom_direction() {
    let json = r#"{"direction":"bottom"}"#;
    let p: ScrollParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.direction, "bottom");
}

#[test]
fn scroll_params_to_element() {
    let json = r#"{"element":42}"#;
    let p: ScrollParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.element, Some(42));
}

#[test]
fn scroll_params_custom_pixels() {
    let json = r#"{"direction":"up","pixels":300}"#;
    let p: ScrollParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.direction, "up");
    assert_eq!(p.pixels, 300);
}

// ── DX-W2-1: assert/extract optional URL ──────────────────

#[test]
fn assert_params_with_url() {
    let json = r#"{"url":"https://example.com","assertions":["has button"]}"#;
    let p: AssertParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.url.as_deref(), Some("https://example.com"));
    assert_eq!(p.assertions.len(), 1);
}

#[test]
fn assert_params_without_url() {
    let json = r#"{"assertions":["has button"]}"#;
    let p: AssertParams = serde_json::from_str(json).unwrap();
    assert!(p.url.is_none());
}

#[test]
fn assert_params_null_url() {
    let json = r#"{"url":null,"assertions":["title contains X"]}"#;
    let p: AssertParams = serde_json::from_str(json).unwrap();
    assert!(p.url.is_none());
}

#[test]
fn extract_params_with_url() {
    let json = r#"{"url":"https://example.com","what":"links"}"#;
    let p: ExtractParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.url.as_deref(), Some("https://example.com"));
}

#[test]
fn extract_params_without_url() {
    let json = r#"{"what":"form fields"}"#;
    let p: ExtractParams = serde_json::from_str(json).unwrap();
    assert!(p.url.is_none());
    assert_eq!(p.what, "form fields");
}

#[test]
fn extract_params_null_url() {
    let json = r#"{"url":null,"what":"prices"}"#;
    let p: ExtractParams = serde_json::from_str(json).unwrap();
    assert!(p.url.is_none());
}

// ── DX-W2-2 / Wave 5b (Pain #13): checked + options in to_prompt ─────

#[test]
fn to_prompt_renders_checked_state() {
    let mut view = empty_view();
    let mut el = make_element(semantic::ElementKind::Checkbox, "Remember me");
    el.checked = Some(true);
    view.elements.push(el);

    let prompt = view.to_prompt();
    // Wave 5b: bare `checked` marker, not `checked=true`.
    assert!(
        prompt.contains(" checked"),
        "prompt should contain bare `checked` marker: {prompt}"
    );
    assert!(
        !prompt.contains("checked=true"),
        "legacy `checked=true` form must not leak back: {prompt}"
    );
}

#[test]
fn to_prompt_omits_unchecked_state() {
    // Wave 5b (Pain #13): `Some(false)` emits nothing to keep lines terse.
    let mut view = empty_view();
    let mut el = make_element(semantic::ElementKind::Checkbox, "Agree to terms");
    el.checked = Some(false);
    view.elements.push(el);

    let prompt = view.to_prompt();
    assert!(
        !prompt.contains("checked"),
        "prompt should omit `checked` for unchecked elements: {prompt}"
    );
}

#[test]
fn to_prompt_radio_checked_emits_marker() {
    // Wave 5b (Pain #13): radio buttons use the same marker path as checkboxes.
    let mut view = empty_view();
    let mut el = make_element(semantic::ElementKind::Input, "Medium");
    el.input_type = Some("radio".into());
    el.name = Some("size".into());
    el.value = Some("medium".into());
    el.checked = Some(true);
    view.elements.push(el);

    let prompt = view.to_prompt();
    assert!(
        prompt.contains(" checked"),
        "radio with checked=Some(true) must emit `checked`: {prompt}"
    );
}

#[test]
fn to_prompt_renders_select_options() {
    let mut view = empty_view();
    let mut el = make_element(semantic::ElementKind::Select, "Country");
    el.options = Some(vec!["US".into(), "BR".into(), "DE".into()]);
    view.elements.push(el);

    let prompt = view.to_prompt();
    assert!(
        prompt.contains(r#"options=["US", "BR", "DE"]"#),
        "prompt should contain options list: {prompt}"
    );
}

#[test]
fn to_prompt_skips_none_checked_and_options() {
    let mut view = empty_view();
    let el = make_element(semantic::ElementKind::Input, "Email");
    view.elements.push(el);

    let prompt = view.to_prompt();
    assert!(
        !prompt.contains("checked="),
        "should not render checked for non-checkbox"
    );
    assert!(
        !prompt.contains("options="),
        "should not render options for non-select"
    );
}

// ── DX-W2-3: fill_form params ─────────────────────────────

#[test]
fn fill_form_params_basic() {
    let json = r#"{"fields":{"Email":"user@test.com","Password":"secret"},"submit":true}"#;
    let p: FillFormParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.fields.len(), 2);
    assert_eq!(p.fields["Email"], "user@test.com");
    assert_eq!(p.fields["Password"], "secret");
    assert!(p.submit);
    assert!(p.form_index.is_none());
}

#[test]
fn fill_form_params_with_form_index() {
    let json = r#"{"fields":{"Name":"Alice"},"form_index":1}"#;
    let p: FillFormParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.form_index, Some(1));
    assert!(!p.submit);
}

#[test]
fn fill_form_params_empty_fields() {
    let json = r#"{"fields":{}}"#;
    let p: FillFormParams = serde_json::from_str(json).unwrap();
    assert!(p.fields.is_empty());
}

// ── Wave 5b (Pain #15): empty fields + submit=true is valid ────

/// Mirror of the guard in `tool_lad_fill_form` — kept out of the handler
/// so we can unit-test the invariant without standing up a full LadServer.
fn fill_form_guard_rejects(p: &FillFormParams) -> Option<&'static str> {
    if p.fields.is_empty() && !p.submit {
        Some("fields must not be empty unless submit=true")
    } else {
        None
    }
}

#[test]
fn fill_form_guard_allows_empty_fields_when_submitting() {
    let json = r#"{"fields":{},"submit":true}"#;
    let p: FillFormParams = serde_json::from_str(json).unwrap();
    assert!(p.fields.is_empty());
    assert!(p.submit);
    assert!(
        fill_form_guard_rejects(&p).is_none(),
        "empty fields with submit=true must pass the guard"
    );
}

#[test]
fn fill_form_guard_rejects_empty_fields_without_submit() {
    let json = r#"{"fields":{}}"#;
    let p: FillFormParams = serde_json::from_str(json).unwrap();
    assert!(!p.submit);
    let reason = fill_form_guard_rejects(&p).expect("guard must reject");
    assert!(
        reason.contains("unless submit=true"),
        "error message should explain the submit=true escape hatch: {reason}"
    );
}

#[test]
fn fill_form_guard_allows_non_empty_fields_without_submit() {
    let json = r#"{"fields":{"Email":"a@b.c"}}"#;
    let p: FillFormParams = serde_json::from_str(json).unwrap();
    assert!(fill_form_guard_rejects(&p).is_none());
}

// ── DX-W2-4: form_index in to_prompt ──────────────────────

#[test]
fn to_prompt_renders_form_index() {
    let mut view = empty_view();
    let mut el = make_element(semantic::ElementKind::Input, "Email");
    el.input_type = Some("email".into());
    el.form_index = Some(0);
    view.elements.push(el);

    let prompt = view.to_prompt();
    assert!(
        prompt.contains("form=0"),
        "prompt should contain form=0: {prompt}"
    );
}

#[test]
fn to_prompt_skips_form_index_when_none() {
    let mut view = empty_view();
    let el = make_element(semantic::ElementKind::Input, "Search");
    view.elements.push(el);

    let prompt = view.to_prompt();
    assert!(
        !prompt.contains("form="),
        "should not render form= when form_index is None"
    );
}

// ── DX-W2-5: select params unchanged (label matching is in JS) ──

#[test]
fn select_params_with_label() {
    let json = r#"{"element":5,"value":"United States"}"#;
    let p: SelectParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.element, Some(5));
    assert_eq!(p.value, "United States");
    assert!(!p.wait_for_navigation);
}

// ── DX-W3-1: wait OR conditions ──────────────────────────────

#[test]
fn wait_params_single_condition_backward_compat() {
    let json = r#"{"condition":"has button Dashboard"}"#;
    let p: WaitParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.condition.as_deref(), Some("has button Dashboard"));
    assert!(p.conditions.is_none());
    assert!(p.mode.is_none());
    assert_eq!(p.timeout_ms, 10_000);
    assert_eq!(p.poll_ms, 500);
}

#[test]
fn wait_params_multiple_conditions_any() {
    let json = r#"{
        "conditions": ["has button Dashboard", "text contains Invalid password"],
        "mode": "any",
        "timeout_ms": 5000
    }"#;
    let p: WaitParams = serde_json::from_str(json).unwrap();
    assert!(p.condition.is_none());
    let conds = p.conditions.unwrap();
    assert_eq!(conds.len(), 2);
    assert_eq!(conds[0], "has button Dashboard");
    assert_eq!(conds[1], "text contains Invalid password");
    assert_eq!(p.mode.as_deref(), Some("any"));
    assert_eq!(p.timeout_ms, 5000);
}

#[test]
fn wait_params_both_singular_and_plural() {
    let json = r#"{
        "condition": "has form",
        "conditions": ["has button submit"],
        "mode": "all"
    }"#;
    let p: WaitParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.condition.as_deref(), Some("has form"));
    assert_eq!(p.conditions.as_ref().unwrap().len(), 1);
}

#[test]
fn wait_params_empty_produces_defaults() {
    let json = r#"{}"#;
    let p: WaitParams = serde_json::from_str(json).unwrap();
    assert!(p.condition.is_none());
    assert!(p.conditions.is_none());
    assert!(p.mode.is_none());
}

// ── DX-W3-3: clear params ────────────────────────────────────

#[test]
fn clear_params_parse() {
    let json = r#"{"element":7}"#;
    let p: ClearParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.element, Some(7));
    assert!(p.target.is_none());
}

#[test]
fn clear_params_parse_with_target() {
    let json = r#"{"target":{"role":"textbox","label":"Email"}}"#;
    let p: ClearParams = serde_json::from_str(json).unwrap();
    assert!(p.element.is_none());
    let t = p.target.unwrap();
    assert_eq!(t.role.as_deref(), Some("textbox"));
    assert_eq!(t.label.as_deref(), Some("Email"));
}

// ── DX-W3-5: element count summary in to_prompt ──────────────

#[test]
fn to_prompt_element_summary_empty() {
    let view = empty_view();
    let prompt = view.to_prompt();
    assert!(
        prompt.contains("ELEMENTS: 0"),
        "empty view should show ELEMENTS: 0: {prompt}"
    );
}

#[test]
fn to_prompt_element_summary_mixed() {
    let mut view = empty_view();
    view.elements
        .push(make_element(semantic::ElementKind::Button, "Submit"));
    view.elements
        .push(make_element(semantic::ElementKind::Button, "Cancel"));
    view.elements
        .push(make_element(semantic::ElementKind::Input, "Email"));
    view.elements
        .push(make_element(semantic::ElementKind::Link, "Home"));

    let prompt = view.to_prompt();
    assert!(
        prompt.contains("ELEMENTS: 4 (2 buttons, 1 input, 1 link)"),
        "should show element summary: {prompt}"
    );
}

#[test]
fn to_prompt_element_summary_single() {
    let mut view = empty_view();
    view.elements
        .push(make_element(semantic::ElementKind::Select, "Country"));

    let prompt = view.to_prompt();
    assert!(
        prompt.contains("ELEMENTS: 1 (1 select)"),
        "should use singular: {prompt}"
    );
}

// ── DX-W3-6: extract format param ────────────────────────────

#[test]
fn extract_params_format_default() {
    let json = r#"{"what":"links"}"#;
    let p: ExtractParams = serde_json::from_str(json).unwrap();
    assert!(p.format.is_none());
}

#[test]
fn extract_params_format_prompt() {
    let json = r#"{"what":"links","format":"prompt"}"#;
    let p: ExtractParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.format.as_deref(), Some("prompt"));
}

#[test]
fn extract_params_format_json() {
    let json = r#"{"what":"links","format":"json"}"#;
    let p: ExtractParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.format.as_deref(), Some("json"));
}

// ── BUG-2: AuditParams return_tab ────────────────────────────

#[test]
fn audit_params_default_return_tab_none() {
    let json = r#"{"url":"https://example.com"}"#;
    let p: AuditParams = serde_json::from_str(json).unwrap();
    assert!(p.return_tab.is_none());
}

#[test]
fn audit_params_return_tab_true() {
    let json = r#"{"url":"https://example.com","return_tab":true}"#;
    let p: AuditParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.return_tab, Some(true));
}

#[test]
fn audit_params_return_tab_false_explicit() {
    let json = r#"{"url":"https://example.com","return_tab":false}"#;
    let p: AuditParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.return_tab, Some(false));
}

// ── BUG-3: text contains / page contains predicates ──────────

#[test]
fn check_assertion_text_contains_matches_title() {
    let mut view = empty_view();
    view.title = "LLM-as-DOM at DuckDuckGo".into();
    assert!(check_assertion("text contains llm", &view, ""));
    assert!(check_assertion("text contains duckduckgo", &view, ""));
}

#[test]
fn check_assertion_text_contains_matches_url() {
    let mut view = empty_view();
    view.url = "https://duckduckgo.com/?q=llm-as-dom".into();
    assert!(check_assertion("text contains llm-as-dom", &view, ""));
    assert!(check_assertion("text contains duckduckgo.com", &view, ""));
}

#[test]
fn check_assertion_text_contains_matches_body() {
    let mut view = empty_view();
    view.visible_text = "Invalid password — please try again".into();
    assert!(check_assertion("text contains invalid password", &view, ""));
}

#[test]
fn check_assertion_page_contains_alias_equivalent() {
    let mut view = empty_view();
    view.title = "Dashboard".into();
    // Both prefixes must resolve to the same union-text path.
    assert_eq!(
        check_assertion("text contains dashboard", &view, ""),
        check_assertion("page contains dashboard", &view, ""),
    );
    assert!(check_assertion("page contains dashboard", &view, ""));
}

#[test]
fn check_assertion_text_contains_miss_returns_false() {
    let mut view = empty_view();
    view.title = "Home".into();
    view.url = "https://example.com".into();
    view.visible_text = "Welcome".into();
    assert!(!check_assertion("text contains nonexistent", &view, ""));
}

#[test]
fn check_assertion_text_contains_empty_needle_false() {
    let mut view = empty_view();
    view.title = "anything".into();
    // Empty needle should not trivially match — guards against
    // `.contains("")` returning true for every string.
    assert!(!check_assertion(r#"text contains """#, &view, ""));
}

// ── FR-2: extract limit resolution ──────────────────────────

#[test]
fn extract_params_default_limit_none() {
    let json = r#"{"what":"links"}"#;
    let p: ExtractParams = serde_json::from_str(json).unwrap();
    assert!(p.limit.is_none());
}

#[test]
fn extract_params_limit_parses() {
    let json = r#"{"what":"links","limit":5}"#;
    let p: ExtractParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.limit, Some(5));
}

#[test]
fn resolve_limit_explicit_wins() {
    use super::tools::extract::resolve_extract_limit;
    let (limit, user_asked_more) = resolve_extract_limit(Some(7), true, "top 3 items");
    assert_eq!(limit, Some(7));
    assert!(!user_asked_more);
}

#[test]
fn resolve_limit_explicit_clamps_to_hard_cap() {
    use super::tools::extract::resolve_extract_limit;
    let (limit, user_asked_more) = resolve_extract_limit(Some(9_999), false, "");
    assert_eq!(limit, Some(200));
    assert!(user_asked_more);
}

#[test]
fn resolve_limit_explicit_zero_is_no_limit() {
    use super::tools::extract::resolve_extract_limit;
    // Explicit `limit=0` reads as "unset" (footgun avoidance), NOT
    // "explicit empty". Falls through to no-limit. Without strict, no NL
    // parse — full list.
    let (limit, user_asked_more) = resolve_extract_limit(Some(0), false, "");
    assert_eq!(limit, None);
    assert!(!user_asked_more);
}

#[test]
fn resolve_limit_explicit_zero_falls_through_to_nl_when_strict() {
    use super::tools::extract::resolve_extract_limit;
    // `limit=0` + strict + NL phrase: zero falls through, NL parse fires.
    // Documents that `0` is interchangeable with `None` for downstream rules.
    let (limit, _) = resolve_extract_limit(Some(0), true, "top 5 story titles");
    assert_eq!(limit, Some(5));
}

#[test]
fn resolve_limit_nl_top_n_when_strict() {
    use super::tools::extract::resolve_extract_limit;
    let (limit, _) = resolve_extract_limit(None, true, "top 5 story titles");
    assert_eq!(limit, Some(5));
}

#[test]
fn resolve_limit_nl_first_n_when_strict() {
    use super::tools::extract::resolve_extract_limit;
    let (limit, _) = resolve_extract_limit(None, true, "first 3 comments");
    assert_eq!(limit, Some(3));
}

#[test]
fn resolve_limit_nl_ptbr_primeiras() {
    use super::tools::extract::resolve_extract_limit;
    let (limit, _) = resolve_extract_limit(None, true, "primeiras 4 histórias");
    assert_eq!(limit, Some(4));
    let (limit, _) = resolve_extract_limit(None, true, "primeiros 2 itens");
    assert_eq!(limit, Some(2));
}

#[test]
fn resolve_limit_nl_best_n() {
    use super::tools::extract::resolve_extract_limit;
    let (limit, _) = resolve_extract_limit(None, true, "best 10 matches");
    assert_eq!(limit, Some(10));
    let (limit, _) = resolve_extract_limit(None, true, "melhores 6 resultados");
    assert_eq!(limit, Some(6));
}

#[test]
fn resolve_limit_nl_ignored_when_not_strict() {
    use super::tools::extract::resolve_extract_limit;
    // Non-strict mode: NL parse does NOT fire. User gets full list.
    let (limit, user_asked_more) = resolve_extract_limit(None, false, "top 5 story titles");
    assert_eq!(limit, None);
    assert!(!user_asked_more);
}

#[test]
fn resolve_limit_nonnumeric_phrase_no_limit() {
    use super::tools::extract::resolve_extract_limit;
    // "top scoring" has no digit — fallback: no implicit limit.
    let (limit, _) = resolve_extract_limit(None, true, "top scoring items");
    assert_eq!(limit, None);
}

#[test]
fn resolve_limit_zero_numeral_ignored() {
    use super::tools::extract::resolve_extract_limit;
    // `top 0 items` is nonsense — don't return Some(0).
    let (limit, _) = resolve_extract_limit(None, true, "top 0 items");
    assert_eq!(limit, None);
}

#[test]
fn resolve_limit_empty_what_no_limit() {
    use super::tools::extract::resolve_extract_limit;
    let (limit, _) = resolve_extract_limit(None, true, "");
    assert_eq!(limit, None);
}

// ── BUG-4 + FR-1: include_cards param + Card shape ────────────

#[test]
fn extract_params_default_include_cards_none() {
    let json = r#"{"what":"anything"}"#;
    let p: ExtractParams = serde_json::from_str(json).unwrap();
    assert!(p.include_cards.is_none());
}

#[test]
fn extract_params_include_cards_true() {
    let json = r#"{"what":"anything","include_cards":true}"#;
    let p: ExtractParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.include_cards, Some(true));
}

#[test]
fn snapshot_params_include_cards_true() {
    let json = r#"{"include_cards":true}"#;
    let p: SnapshotParams = serde_json::from_str(json).unwrap();
    assert_eq!(p.include_cards, Some(true));
}

#[test]
fn semantic_view_cards_omitted_when_none() {
    let view = empty_view();
    let json = serde_json::to_string(&view).unwrap();
    assert!(
        !json.contains("\"cards\""),
        "default-None cards must be skipped in serialization: {json}"
    );
}

#[test]
fn semantic_view_cards_serialized_when_some() {
    let mut view = empty_view();
    view.cards = Some(vec![semantic::Card {
        id: "c0".into(),
        title: "Alberta startup sells no-tech tractors".into(),
        metadata: vec![
            ("points".into(), "647".into()),
            ("author".into(), "Kaibeezy".into()),
        ],
        child_element_ids: vec![11, 16],
    }]);
    let json = serde_json::to_string(&view).unwrap();
    assert!(json.contains("\"cards\""));
    assert!(json.contains("\"c0\""));
    assert!(json.contains("\"points\""));
    assert!(json.contains("\"647\""));
    assert!(json.contains("\"child_element_ids\""));
}

#[test]
fn card_id_string_does_not_collide_with_element_id_int() {
    // Regression gate: element IDs are u32, card IDs are `cN` strings —
    // a caller cross-indexing the two should never see collision since
    // string vs number are distinct JSON types.
    let mut view = empty_view();
    view.cards = Some(vec![semantic::Card {
        id: "c0".into(),
        title: "card zero".into(),
        metadata: vec![],
        child_element_ids: vec![0],
    }]);
    let json = serde_json::to_string(&view).unwrap();
    assert!(json.contains("\"id\":\"c0\""));
}
