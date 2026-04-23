//! Criterion benchmarks for pure-Rust hot paths (no browser needed).

use criterion::{Criterion, black_box, criterion_group, criterion_main};

use llm_as_dom::heuristics;
use llm_as_dom::pilot::Action;
use llm_as_dom::semantic::{Element, ElementKind, PageState, SemanticView};

// ── Helpers ──────────────────────────────────────────────────────────

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
        checked: None,
        options: None,
        frame_index: None,
        is_visible: None,
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
        checked: None,
        options: None,
        frame_index: None,
        is_visible: None,
    }
}

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
        checked: None,
        options: None,
        frame_index: None,
        is_visible: None,
    }
}

/// Build a realistic login page view.
fn login_view() -> SemanticView {
    SemanticView {
        url: "https://news.ycombinator.com/login".into(),
        title: "Login".into(),
        page_hint: "login page".into(),
        text_blocks: vec![],
        elements: vec![
            input_element(0, "Username", "text", Some("acct"), Some(0)),
            input_element(1, "Password", "password", Some("pw"), Some(0)),
            button_element(2, "login", Some(0)),
            link_element(3, "Forgot password?", "/forgot"),
            link_element(4, "Create Account", "/signup"),
        ],
        forms: vec![],
        visible_text: "Login Hacker News".into(),
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
        cards: None,
        cards_truncated: None,
    }
}

/// Build a large view (~50 elements) to stress serialization.
fn large_view() -> SemanticView {
    let mut elements = Vec::with_capacity(50);
    for i in 0..30 {
        elements.push(link_element(
            i,
            &format!("Nav Link {i}"),
            &format!("/page/{i}"),
        ));
    }
    for i in 30..40 {
        elements.push(input_element(
            i,
            &format!("Field {i}"),
            "text",
            Some(&format!("field_{i}")),
            Some(0),
        ));
    }
    for i in 40..50 {
        elements.push(button_element(i, &format!("Button {i}"), Some(0)));
    }

    SemanticView {
        url: "https://example.com/complex-page".into(),
        title: "Complex Page With Many Elements".into(),
        page_hint: "form page".into(),
        elements,
        forms: vec![],
        visible_text: "This is a complex page with navigation, forms, and buttons. It represents a realistic extraction scenario.".into(),
        text_blocks: vec![],
        state: PageState::Ready,
        element_cap: None,
        blocked_reason: None,
        session_context: None,
        cards: None,
        cards_truncated: None,
    }
}

// ── Benchmarks ───────────────────────────────────────────────────────

fn bench_to_prompt(c: &mut Criterion) {
    let login = login_view();
    let large = large_view();

    c.bench_function("to_prompt_login_5el", |b| {
        b.iter(|| black_box(login.to_prompt()));
    });

    c.bench_function("to_prompt_large_50el", |b| {
        b.iter(|| black_box(large.to_prompt()));
    });
}

fn bench_try_resolve(c: &mut Criterion) {
    let view = login_view();
    let goal = "login as testuser password secret123";

    c.bench_function("try_resolve_first_field", |b| {
        b.iter(|| black_box(heuristics::try_resolve(&view, goal, &[])));
    });

    c.bench_function("try_resolve_after_username", |b| {
        b.iter(|| black_box(heuristics::try_resolve(&view, goal, &[0])));
    });

    c.bench_function("try_resolve_all_filled", |b| {
        b.iter(|| black_box(heuristics::try_resolve(&view, goal, &[0, 1])));
    });
}

fn bench_parse_action(c: &mut Criterion) {
    let clean_json = r#"{"action":"click","element":2,"reasoning":"submit the login form"}"#;
    let with_think = r#"<think>The user wants to click the submit button to log in. I should target element 2 which is the login button.</think>{"action":"click","element":2,"reasoning":"submit the login form"}"#;
    let with_markdown = "Here's my action:\n```json\n{\"action\":\"type\",\"element\":0,\"value\":\"testuser\",\"reasoning\":\"fill username\"}\n```";

    c.bench_function("parse_action_clean", |b| {
        b.iter(|| {
            let action: Action = serde_json::from_str(black_box(clean_json)).unwrap();
            black_box(action);
        });
    });

    // For think-tagged input, bench the strip + parse flow
    c.bench_function("parse_action_think_tags", |b| {
        b.iter(|| {
            let input = black_box(with_think);
            // Simulate the strip_think_tags + extract_json flow
            let clean: String = {
                let mut result = String::with_capacity(input.len());
                let mut in_think = false;
                let mut chars = input.chars().peekable();
                while let Some(c) = chars.next() {
                    if !in_think && c == '<' {
                        let rest: String = chars.clone().take(6).collect();
                        if rest == "think>" {
                            in_think = true;
                            for _ in 0..6 {
                                chars.next();
                            }
                            continue;
                        }
                    }
                    if in_think && c == '<' {
                        let rest: String = chars.clone().take(7).collect();
                        if rest == "/think>" {
                            in_think = false;
                            for _ in 0..7 {
                                chars.next();
                            }
                            continue;
                        }
                    }
                    if !in_think {
                        result.push(c);
                    }
                }
                result
            };
            let trimmed = clean.trim();
            // Find JSON object
            if let Some(start) = trimmed.find('{') {
                let json_bytes = trimmed.as_bytes();
                let mut depth = 0;
                for (i, &b) in json_bytes.iter().enumerate().skip(start) {
                    if b == b'{' {
                        depth += 1;
                    } else if b == b'}' {
                        depth -= 1;
                        if depth == 0 {
                            let json_str = &trimmed[start..=i];
                            let action: Action = serde_json::from_str(json_str).unwrap();
                            return black_box(action);
                        }
                    }
                }
            }
            panic!("no json found");
        });
    });

    c.bench_function("parse_action_markdown_block", |b| {
        b.iter(|| {
            let input = black_box(with_markdown);
            let json_str = input
                .split("```json\n")
                .nth(1)
                .and_then(|s| s.split("\n```").next())
                .unwrap();
            let action: Action = serde_json::from_str(json_str).unwrap();
            black_box(action);
        });
    });
}

// ── PERF-P3: Additional benchmarks ──────────────────────────────────

fn bench_sanitize_text(c: &mut Criterion) {
    // Build a realistic 50-element view prompt string with some stego chars.
    let view = large_view();
    let prompt = view.to_prompt();
    // Sprinkle some steganographic chars.
    let input = format!(
        "Title\u{200B}\u{200D} heading\u{FEFF}\n{}",
        &prompt[..prompt.len().min(2000)]
    );

    c.bench_function("sanitize_text_50el_view", |b| {
        b.iter(|| black_box(llm_as_dom::sanitize::sanitize_text(black_box(&input))));
    });
}

fn bench_sanitize_view_300el(c: &mut Criterion) {
    // Build a 300-element view to stress sanitize_for_prompt.
    let mut elements = Vec::with_capacity(300);
    for i in 0..200 {
        elements.push(link_element(
            i,
            &format!("Nav Link {i}"),
            &format!("/page/{i}"),
        ));
    }
    for i in 200..270 {
        elements.push(input_element(
            i,
            &format!("Field {i}"),
            "text",
            Some(&format!("field_{i}")),
            Some(0),
        ));
    }
    for i in 270..300 {
        elements.push(button_element(i, &format!("Button {i}"), Some(0)));
    }

    let view = SemanticView {
        url: "https://example.com/large-page".into(),
        title: "Large Page With 300 Elements".into(),
        page_hint: "form page".into(),
        elements,
        forms: vec![],
        visible_text: "A large page with many interactive elements for stress testing.".into(),
        text_blocks: vec![],
        state: PageState::Ready,
        element_cap: Some("300/500".into()),
        blocked_reason: None,
        session_context: None,
        cards: None,
        cards_truncated: None,
    };

    let raw = view.to_prompt();

    c.bench_function("sanitize_for_prompt_300el", |b| {
        b.iter(|| {
            black_box(llm_as_dom::backend::generic::sanitize_for_prompt(
                black_box(&raw),
                40000,
            ))
        });
    });

    c.bench_function("to_prompt_300el", |b| {
        b.iter(|| black_box(view.to_prompt()));
    });
}

fn bench_is_safe_url(c: &mut Criterion) {
    let safe = "https://example.com/page?q=search&page=2";
    let private = "http://192.168.1.1/admin";
    let javascript = "javascript:alert(1)";
    let relative = "/dashboard";

    c.bench_function("is_safe_url_safe", |b| {
        b.iter(|| black_box(llm_as_dom::sanitize::is_safe_url(black_box(safe))));
    });

    c.bench_function("is_safe_url_private", |b| {
        b.iter(|| black_box(llm_as_dom::sanitize::is_safe_url(black_box(private))));
    });

    c.bench_function("is_safe_url_javascript", |b| {
        b.iter(|| black_box(llm_as_dom::sanitize::is_safe_url(black_box(javascript))));
    });

    c.bench_function("is_safe_url_relative", |b| {
        b.iter(|| black_box(llm_as_dom::sanitize::is_safe_url(black_box(relative))));
    });
}

fn bench_redact_url_secrets(c: &mut Criterion) {
    let url_with_secrets = "https://example.com/cb?code=abc123&state=xyz&access_token=jwt&page=1";
    let url_clean = "https://example.com/page?q=search&page=2";

    c.bench_function("redact_url_secrets_with_secrets", |b| {
        b.iter(|| {
            black_box(llm_as_dom::sanitize::redact_url_secrets(black_box(
                url_with_secrets,
            )))
        });
    });

    c.bench_function("redact_url_secrets_clean", |b| {
        b.iter(|| {
            black_box(llm_as_dom::sanitize::redact_url_secrets(black_box(
                url_clean,
            )))
        });
    });
}

criterion_group!(
    benches,
    bench_to_prompt,
    bench_try_resolve,
    bench_parse_action,
    bench_sanitize_text,
    bench_sanitize_view_300el,
    bench_is_safe_url,
    bench_redact_url_secrets,
);
criterion_main!(benches);
