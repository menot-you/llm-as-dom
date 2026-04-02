//! CLI binary for the LLM-as-DOM browser pilot.
//!
//! Usage: `lad --url <URL> [--goal <GOAL>] [--visible] [--extract-only]`

use futures::StreamExt;
use llm_as_dom::{Error, a11y, backend, pilot};

use clap::Parser;

/// CLI arguments for the `lad` browser pilot.
#[derive(Parser)]
#[command(name = "lad", about = "LLM-as-DOM: AI browser pilot")]
struct Cli {
    /// URL to navigate to.
    #[arg(short, long)]
    url: String,

    /// Goal for the pilot (natural language).
    #[arg(short, long, default_value = "")]
    goal: String,

    /// Show browser window (default: headless).
    #[arg(long, default_value_t = false)]
    visible: bool,

    /// LLM backend: "ollama" or "zai".
    #[arg(long, default_value = "ollama")]
    backend: String,

    /// Ollama base URL (only for --backend ollama).
    #[arg(long, default_value = "http://localhost:11434")]
    ollama_url: String,

    /// LLM model name.
    #[arg(long, default_value = "qwen2.5:7b")]
    model: String,

    /// Max pilot steps before giving up.
    #[arg(long, default_value_t = 10)]
    max_steps: u32,

    /// Only extract and print the `SemanticView` (skip pilot loop).
    #[arg(long, default_value_t = false)]
    extract_only: bool,

    /// Timeout in seconds to wait for SPA content to stabilise (default: 5).
    #[arg(long, default_value_t = a11y::DEFAULT_WAIT_TIMEOUT)]
    wait_timeout: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "llm_as_dom=info".into()),
        )
        .compact()
        .init();

    let cli = Cli::parse();

    tracing::info!(url = %cli.url, visible = cli.visible, "launching browser");

    let mut builder = chromiumoxide::BrowserConfig::builder();
    if !cli.visible {
        builder = builder.arg("--headless=new");
    }
    builder = builder
        .arg("--disable-gpu")
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .arg("--window-size=1280,800");

    let config = builder.build().map_err(Error::BrowserStr)?;
    let (browser, mut handler) = chromiumoxide::Browser::launch(config)
        .await
        .map_err(|e| Error::BrowserStr(format!("{e}")))?;

    let handle = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = browser.new_page(&cli.url).await?;
    page.wait_for_navigation().await?;
    a11y::wait_for_content(&page, cli.wait_timeout).await?;
    tracing::info!("page loaded");

    if cli.extract_only || cli.goal.is_empty() {
        let view = a11y::extract_semantic_view(&page).await?;
        println!(
            "\n=== SemanticView ({} elements, ~{} tokens) ===\n",
            view.elements.len(),
            view.estimated_tokens()
        );
        println!("{}", view.to_prompt());
        println!("\n=== JSON ===\n");
        println!("{}", serde_json::to_string_pretty(&view)?);
    } else {
        let backend_impl: Box<dyn pilot::PilotBackend> = match cli.backend.as_str() {
            "zai" => Box::new(backend::zai::ZaiBackend::new("", &cli.model)),
            _ => Box::new(backend::ollama::OllamaBackend::new(
                &cli.ollama_url,
                &cli.model,
            )),
        };

        let config = pilot::PilotConfig {
            goal: cli.goal.clone(),
            max_steps: cli.max_steps,
            use_heuristics: true,
            max_retries_per_step: 2,
        };

        let result = pilot::run_pilot(&page, backend_impl.as_ref(), &config).await?;

        println!("\n=== Pilot Result ===");
        println!("Success: {}", result.success);
        println!(
            "Steps: {} (heuristic: {}, llm: {})",
            result.steps.len(),
            result.heuristic_hits,
            result.llm_hits
        );
        println!("Duration: {:.1}s", result.total_duration.as_secs_f64());
        println!("\nFinal: {:?}", result.final_action);

        for step in &result.steps {
            println!(
                "  [{}] {:?} {:?} ({:.1}s)",
                step.index,
                step.source,
                step.action,
                step.duration.as_secs_f64()
            );
        }
    }

    drop(page);
    drop(browser);
    handle.abort();

    Ok(())
}
