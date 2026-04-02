use futures::StreamExt;
mod a11y;
mod backend;
mod error;
mod heuristics;
mod pilot;
mod semantic;

pub use error::Error;

use clap::Parser;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "lad", about = "LLM-as-DOM: AI browser pilot")]
struct Cli {
    /// URL to navigate to
    #[arg(short, long)]
    url: String,

    /// Goal for the pilot (natural language)
    #[arg(short, long, default_value = "")]
    goal: String,

    /// Show browser window (default: headless)
    #[arg(long, default_value_t = false)]
    visible: bool,

    /// Ollama base URL
    #[arg(long, default_value = "http://localhost:11434")]
    ollama_url: String,

    /// LLM model name
    #[arg(long, default_value = "qwen3:8b")]
    model: String,

    /// Max pilot steps
    #[arg(long, default_value_t = 10)]
    max_steps: u32,

    /// Only extract and print SemanticView (no pilot)
    #[arg(long, default_value_t = false)]
    extract_only: bool,
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

    let config = builder.build().map_err(|e| Error::BrowserStr(e))?;
    let (browser, mut handler) = chromiumoxide::Browser::launch(config)
        .await
        .map_err(|e| Error::BrowserStr(format!("{e}")))?;

    let handle = tokio::spawn(async move {
        loop {
            if handler.next().await.is_none() {
                break;
            }
        }
    });

    let page = browser.new_page(&cli.url).await?;
    page.wait_for_navigation().await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
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
        let backend = backend::ollama::OllamaBackend::new(&cli.ollama_url, &cli.model);

        let config = pilot::PilotConfig {
            goal: cli.goal.clone(),
            max_steps: cli.max_steps,
            step_timeout: Duration::from_secs(30),
            use_heuristics: true,
        };

        let result = pilot::run_pilot(&page, &backend, &config).await?;

        println!("\n=== Pilot Result ===");
        println!("Success: {}", result.success);
        println!("Steps: {} (heuristic: {}, llm: {})", result.steps.len(), result.heuristic_hits, result.llm_hits);
        println!("Duration: {:.1}s", result.total_duration.as_secs_f64());
        println!("\nFinal: {:?}", result.final_action);

        for step in &result.steps {
            println!(
                "  [{}] {:?} {:?} ({:.1}s)",
                step.index, step.source, step.action, step.duration.as_secs_f64()
            );
        }
    }

    drop(page);
    drop(browser);
    handle.abort();

    Ok(())
}
