//! WebKit engine integration tests.
//!
//! All tests are `#[ignore]` -- they need macOS with the lad-webkit-bridge binary.
//! Run with:
//!   LAD_WEBKIT_BRIDGE=./webkit-bridge/.build/debug/lad-webkit-bridge cargo test -- --ignored

use llm_as_dom::engine::{BrowserEngine, EngineConfig, PageHandle};

/// Helper to create a WebKit engine for tests.
async fn create_webkit_engine() -> Box<dyn BrowserEngine> {
    let config = EngineConfig::default();
    let engine = llm_as_dom::engine::webkit::WebKitEngine::launch(config)
        .await
        .expect("webkit engine should launch (set LAD_WEBKIT_BRIDGE env var)");
    Box::new(engine)
}

// ── Basic extraction ────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires macOS + lad-webkit-bridge"]
async fn webkit_extract_basic() {
    let engine = create_webkit_engine().await;
    let page = engine.new_page("https://example.com").await.unwrap();
    llm_as_dom::a11y::wait_for_content(page.as_ref(), 5)
        .await
        .unwrap();

    let view = llm_as_dom::a11y::extract_semantic_view(page.as_ref())
        .await
        .unwrap();

    assert!(!view.elements.is_empty(), "should extract elements");
    assert!(
        view.title.contains("Example"),
        "title should contain 'Example', got: {}",
        view.title
    );

    engine.close().await.unwrap();
}

// ── Screenshot ──────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires macOS + lad-webkit-bridge"]
async fn webkit_screenshot() {
    let engine = create_webkit_engine().await;
    let page = engine.new_page("https://example.com").await.unwrap();
    llm_as_dom::a11y::wait_for_content(page.as_ref(), 5)
        .await
        .unwrap();

    let png = page.screenshot_png().await.unwrap();
    assert!(png.len() > 1000, "screenshot should be a valid PNG");
    assert_eq!(
        &png[..4],
        &[0x89, 0x50, 0x4E, 0x47],
        "should start with PNG magic bytes"
    );

    engine.close().await.unwrap();
}

// ── Cookie isolation ────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires macOS + lad-webkit-bridge"]
async fn webkit_cookies_isolated() {
    let engine1 = create_webkit_engine().await;
    let page1 = engine1.new_page("https://example.com").await.unwrap();
    llm_as_dom::a11y::wait_for_content(page1.as_ref(), 5)
        .await
        .unwrap();

    // Set a cookie in engine 1.
    let cookie = llm_as_dom::session::CookieEntry {
        name: "test_cookie".into(),
        value: "hello".into(),
        domain: "example.com".into(),
        path: "/".into(),
        expires: 0.0,
        secure: false,
        http_only: false,
        same_site: None,
    };
    page1.set_cookies(&[cookie]).await.unwrap();

    // Engine 2 should NOT see the cookie (session isolation).
    let engine2 = create_webkit_engine().await;
    let page2 = engine2.new_page("https://example.com").await.unwrap();
    llm_as_dom::a11y::wait_for_content(page2.as_ref(), 5)
        .await
        .unwrap();
    let cookies2 = page2.cookies().await.unwrap();

    assert!(
        !cookies2.iter().any(|c| c.name == "test_cookie"),
        "cookies should NOT leak between engines"
    );

    engine1.close().await.unwrap();
    engine2.close().await.unwrap();
}

// ── JS eval types ───────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires macOS + lad-webkit-bridge"]
async fn webkit_eval_js_types() {
    let engine = create_webkit_engine().await;
    let page = engine.new_page("https://example.com").await.unwrap();
    llm_as_dom::a11y::wait_for_content(page.as_ref(), 5)
        .await
        .unwrap();

    // String
    let val = page.eval_js("'hello'").await.unwrap();
    assert_eq!(val.as_str().unwrap(), "hello");

    // Number
    let val = page.eval_js("42").await.unwrap();
    assert_eq!(val.as_i64().unwrap(), 42);

    // Null
    let val = page.eval_js("null").await.unwrap();
    assert!(val.is_null());

    // Object
    let val = page.eval_js("({a: 1, b: 'two'})").await.unwrap();
    assert_eq!(val["a"].as_i64().unwrap(), 1);
    assert_eq!(val["b"].as_str().unwrap(), "two");

    engine.close().await.unwrap();
}

// ── Cross-engine parity ─────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Chrome + macOS + lad-webkit-bridge"]
async fn cross_engine_parity() {
    // Chromium
    let chromium = llm_as_dom::engine::chromium::ChromiumEngine::launch(EngineConfig::default())
        .await
        .unwrap();
    let chrome_page = chromium.new_page("https://example.com").await.unwrap();
    llm_as_dom::a11y::wait_for_content(chrome_page.as_ref(), 5)
        .await
        .unwrap();
    let chrome_view = llm_as_dom::a11y::extract_semantic_view(chrome_page.as_ref())
        .await
        .unwrap();

    // WebKit
    let webkit = create_webkit_engine().await;
    let webkit_page = webkit.new_page("https://example.com").await.unwrap();
    llm_as_dom::a11y::wait_for_content(webkit_page.as_ref(), 5)
        .await
        .unwrap();
    let webkit_view = llm_as_dom::a11y::extract_semantic_view(webkit_page.as_ref())
        .await
        .unwrap();

    // Title matches.
    assert_eq!(chrome_view.title, webkit_view.title);

    // Element count within 50% range (browsers may differ in extraction).
    let ratio = webkit_view.elements.len() as f64 / chrome_view.elements.len().max(1) as f64;
    assert!(
        ratio > 0.5 && ratio < 2.0,
        "element counts diverged too much: chrome={} webkit={}",
        chrome_view.elements.len(),
        webkit_view.elements.len()
    );

    chromium.close().await.unwrap();
    webkit.close().await.unwrap();
}
