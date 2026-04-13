// Standalone test: exercise the FULL LAD pipeline against CloakBrowser to
// find which step hangs. Mirrors ChromiumEngine::launch + new_page +
// stealth::apply_stealth + wait_for_content.

use llm_as_dom::engine::cloak_bootstrap::resolve_cloak_binary;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("debug,tungstenite=info,tokio_tungstenite=info")
        .with_writer(std::io::stderr)
        .init();

    eprintln!("\n== STEP 1: resolve_cloak_binary ==");
    let path = resolve_cloak_binary()?;
    eprintln!("-> path: {:?}", path);

    eprintln!("\n== STEP 2: build BrowserConfig with LAD flags ==");
    // Use LAD's real persistent profile to reproduce the hang
    let udd_path = std::path::PathBuf::from(std::env::var("HOME").unwrap())
        .join("Library/Caches/lad/chrome-profile");
    std::fs::create_dir_all(&udd_path)?;
    let udd = udd_path.to_string_lossy().to_string();
    eprintln!("-> udd: {}", udd);
    let mut builder = chromiumoxide::BrowserConfig::builder()
        .user_data_dir(udd)
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-blink-features=AutomationControlled")
        .arg("--disable-features=AutomationControlled")
        .arg("--webrtc-ip-handling-policy=disable_non_proxied_udp")
        .arg("--force-webrtc-ip-handling-policy")
        .arg("--disable-dev-shm-usage")
        .arg("--disable-gpu");
    if let Some(p) = path.as_ref() {
        builder = builder.chrome_executable(p);
    }
    let config = builder.build().map_err(|e| format!("build: {e}"))?;
    eprintln!("-> config built");

    eprintln!("\n== STEP 3: Browser::launch ==");
    let t = std::time::Instant::now();
    let (browser, mut handler) = chromiumoxide::Browser::launch(config).await?;
    let handle = tokio::spawn(async move {
        use futures::StreamExt;
        while handler.next().await.is_some() {}
    });
    eprintln!("-> launched in {:?}", t.elapsed());

    eprintln!("\n== STEP 4: new_page(about:blank) ==");
    let t = std::time::Instant::now();
    let page = browser.new_page("about:blank").await?;
    eprintln!("-> blank page in {:?}", t.elapsed());

    eprintln!("\n== STEP 5: apply_stealth (UA override + timezone + script) ==");
    let t = std::time::Instant::now();
    llm_as_dom::engine::stealth::apply_stealth(&page)
        .await
        .map_err(|e| format!("stealth: {e}"))?;
    eprintln!("-> stealth applied in {:?}", t.elapsed());

    eprintln!("\n== STEP 6: goto example.com ==");
    let t = std::time::Instant::now();
    page.goto("https://example.com").await?;
    eprintln!("-> goto in {:?}", t.elapsed());

    eprintln!("\n== STEP 7: wait_for_navigation ==");
    let t = std::time::Instant::now();
    page.wait_for_navigation().await?;
    eprintln!("-> nav complete in {:?}", t.elapsed());

    eprintln!("\n== STEP 8: get_title ==");
    let title = page.get_title().await?;
    eprintln!("-> title: {:?}", title);

    eprintln!("\n== STEP 9: evaluate navigator.webdriver ==");
    let js = "JSON.stringify({webdriver: navigator.webdriver, ua: navigator.userAgent.slice(0,60), plugins: navigator.plugins.length})";
    let eval_result = page.evaluate(js).await?;
    let v: serde_json::Value = eval_result.into_value()?;
    eprintln!("-> {}", v);

    drop(browser);
    handle.abort();
    eprintln!("\n== DONE ==");
    Ok(())
}
