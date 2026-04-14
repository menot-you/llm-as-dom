//! CLI binary for the LLM-as-DOM browser pilot.
//!
//! Usage: `lad --url <URL> [--goal <GOAL> | "goal words..."] [--visible] [--extract-only]`

use llm_as_dom::engine::chromium::ChromiumEngine;
use llm_as_dom::engine::webkit::WebKitEngine;
use llm_as_dom::engine::{BrowserEngine, EngineConfig, PageHandle};
use llm_as_dom::{a11y, backend, pilot};

use clap::Parser;

/// CLI arguments for the `lad` browser pilot.
#[derive(Parser)]
#[command(name = "lad", about = "LLM-as-DOM: AI browser pilot")]
struct Cli {
    /// URL to navigate to.
    #[arg(short, long)]
    url: String,

    /// Goal for the pilot (natural language).
    /// Can be passed as --goal "..." or as a trailing positional argument.
    #[arg(short, long)]
    goal: Option<String>,

    /// Trailing positional arguments (goal words when --goal is not used).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    goal_positional: Vec<String>,

    /// Show browser window (default: headless).
    #[arg(long, default_value_t = false)]
    visible: bool,

    /// Interactive mode: opens a visible Chrome --app window for human
    /// interaction (captcha solving). Implies --visible.
    #[arg(long, default_value_t = false)]
    interactive: bool,

    /// LLM backend: "ollama", "openai", or "anthropic" (auto-detected when LAD_LLM_API_KEY is set).
    #[arg(long, default_value = "auto")]
    backend: String,

    /// LLM base URL (Ollama, Z.AI, or any compatible API).
    #[arg(long, default_value = "http://localhost:11434", alias = "ollama-url")]
    llm_url: String,

    /// LLM model name.
    #[arg(long, default_value = "qwen2.5:7b", alias = "model")]
    llm_model: String,

    /// Max pilot steps before giving up.
    #[arg(long, default_value_t = 10)]
    max_steps: u32,

    /// Only extract and print the `SemanticView` (skip pilot loop).
    #[arg(long, default_value_t = false)]
    extract_only: bool,

    /// Timeout in seconds to wait for SPA content to stabilise (default: 5).
    #[arg(long, default_value_t = a11y::DEFAULT_WAIT_TIMEOUT)]
    wait_timeout: u64,

    /// Directory containing `.json` playbook files for Tier 0 replay.
    #[arg(long, default_value = ".lad/playbooks")]
    playbook_dir: String,

    /// Chrome profile path for cookie reuse. Use "default" for the default profile.
    #[arg(long, alias = "chrome-profile")]
    profile: Option<String>,

    /// Browser engine: "chromium" (default) or "webkit".
    #[arg(long, default_value = "chromium")]
    engine: String,
}

// Sentry MUST be initialised before ANY other setup so that panics raised
// during runtime bootstrap (tokio, tracing subscriber, CLI parsing) are
// reported. If SENTRY_DSN is unset or empty the guard is a no-op and the
// binary behaves exactly as before.
//
// Post-incident hardening: added 2026-04-03 after an npm auth token leaked
// via a Playwright DOM snapshot. Runtime error tracking is now mandatory
// so future production issues are surfaced before they become incidents.
//
// Ops env-var contract:
//   SENTRY_DSN          — enables reporting when set to a non-empty string
//   SENTRY_ENVIRONMENT  — deployment tag (defaults to "production")
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _sentry_guard = init_sentry();

    // Layer the fmt subscriber with Sentry's tracing bridge so `tracing`
    // error events propagate to Sentry. Layering (not replacing) preserves
    // the existing stderr output format used by ops.
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "llm_as_dom=info".into()),
        )
        .with(tracing_subscriber::fmt::layer().compact())
        .with(sentry::integrations::tracing::layer())
        .init();

    let cli = Cli::parse();
    let goal = cli
        .goal
        .clone()
        .unwrap_or_else(|| cli.goal_positional.join(" "));

    let visible = cli.visible || cli.interactive;
    tracing::info!(url = %cli.url, visible, interactive = cli.interactive, "launching browser");

    let engine_config = EngineConfig {
        visible,
        interactive: cli.interactive,
        user_data_dir: std::env::temp_dir().join(format!("lad-chrome-{}", std::process::id())),
        temp_dir: None,
        window_size: if cli.interactive {
            (1024, 768)
        } else {
            (1280, 800)
        },
    };

    let engine: Box<dyn BrowserEngine> = match cli.engine.as_str() {
        "webkit" => Box::new(WebKitEngine::launch(engine_config).await?),
        _ => Box::new(ChromiumEngine::launch(engine_config).await?),
    };
    let page: Box<dyn PageHandle> = engine.new_page(&cli.url).await?;
    page.wait_for_navigation().await?;
    a11y::wait_for_content(page.as_ref(), cli.wait_timeout).await?;
    tracing::info!("page loaded");

    // Inject Chrome profile cookies if --profile is set
    if let Some(ref profile_name) = cli.profile {
        if let Some(profile_path) = llm_as_dom::profile::resolve_profile_path(profile_name) {
            match llm_as_dom::profile::extract_cookies_from_profile(&profile_path) {
                Ok(cookies) => {
                    tracing::info!(count = cookies.len(), "injecting Chrome profile cookies");
                    page.set_cookies(&cookies).await?;
                }
                Err(e) => tracing::warn!(error = %e, "failed to load Chrome profile cookies"),
            }
        } else {
            tracing::warn!(profile = %profile_name, "Chrome profile not found");
        }
    }

    if cli.extract_only || goal.is_empty() {
        let view = a11y::extract_semantic_view(page.as_ref()).await?;
        println!(
            "\n=== SemanticView ({} elements, ~{} tokens) ===\n",
            view.elements.len(),
            view.estimated_tokens()
        );
        println!("{}", view.to_prompt());
        println!("\n=== JSON ===\n");
        println!("{}", serde_json::to_string_pretty(&view)?);
    } else {
        // FIX-9: Use canonical backend factory for auto-detect.
        // Explicit backend names still override for backwards compat.
        let backend_impl: Box<dyn pilot::PilotBackend> = match cli.backend.as_str() {
            "anthropic" => Box::new(backend::anthropic::AnthropicBackend::new(
                "",
                &cli.llm_model,
                None,
            )),
            "openai" => Box::new(backend::openai::OpenAiBackend::new(
                "",
                &cli.llm_model,
                None,
            )),
            "ollama" => Box::new(backend::generic::GenericLlmBackend::new(
                &cli.llm_url,
                &cli.llm_model,
                None,
            )),
            _ => backend::create_backend(&cli.llm_url, &cli.llm_model, None),
        };

        let playbook_path = std::path::PathBuf::from(&cli.playbook_dir);
        let config = pilot::PilotConfig {
            goal: goal.clone(),
            max_steps: cli.max_steps,
            use_hints: true,
            use_heuristics: true,
            playbook_dir: if playbook_path.is_dir() {
                Some(playbook_path)
            } else {
                None
            },
            max_retries_per_step: 2,
            session: None,
            interactive: cli.interactive,
        };

        let result = pilot::run_pilot(page.as_ref(), backend_impl.as_ref(), &config).await?;

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
    engine.close().await?;

    Ok(())
}

/// Initialise Sentry if `SENTRY_DSN` is set and non-empty.
///
/// Returns `Some(ClientInitGuard)` when active (must be held until `main`
/// exits so the Drop impl flushes queued events) or `None` when the env
/// var is absent/empty — the entire SDK is a no-op in that case.
fn init_sentry() -> Option<sentry::ClientInitGuard> {
    let dsn = std::env::var("SENTRY_DSN").ok().filter(|s| !s.is_empty())?;
    let environment = std::env::var("SENTRY_ENVIRONMENT").unwrap_or_else(|_| "production".into());
    Some(sentry::init((
        dsn,
        sentry::ClientOptions {
            release: sentry::release_name!(),
            environment: Some(environment.into()),
            attach_stacktrace: true,
            ..Default::default()
        },
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cli_goal_flag() {
        let cli = Cli::try_parse_from(["lad", "--url", "https://x.com", "--goal", "do X"]).unwrap();
        assert_eq!(cli.goal.as_deref(), Some("do X"));
        assert!(cli.goal_positional.is_empty());
    }

    #[test]
    fn cli_goal_positional() {
        let cli = Cli::try_parse_from(["lad", "--url", "https://x.com", "do", "X"]).unwrap();
        assert!(cli.goal.is_none());
        assert_eq!(cli.goal_positional, vec!["do", "X"]);
    }

    #[test]
    fn cli_goal_flag_takes_priority() {
        let cli = Cli::try_parse_from([
            "lad",
            "--url",
            "https://x.com",
            "--goal",
            "from flag",
            "extra",
        ])
        .unwrap();
        assert_eq!(cli.goal.as_deref(), Some("from flag"));
    }

    #[test]
    fn cli_goal_resolution_from_flag() {
        let cli =
            Cli::try_parse_from(["lad", "--url", "https://x.com", "--goal", "flagged"]).unwrap();
        let resolved = cli.goal.unwrap_or_else(|| cli.goal_positional.join(" "));
        assert_eq!(resolved, "flagged");
    }

    #[test]
    fn cli_goal_resolution_from_positional() {
        let cli = Cli::try_parse_from(["lad", "--url", "https://x.com", "hello", "world"]).unwrap();
        let resolved = cli.goal.unwrap_or_else(|| cli.goal_positional.join(" "));
        assert_eq!(resolved, "hello world");
    }

    #[test]
    fn cli_no_goal_resolves_empty() {
        let cli = Cli::try_parse_from(["lad", "--url", "https://x.com"]).unwrap();
        let resolved = cli.goal.unwrap_or_else(|| cli.goal_positional.join(" "));
        assert!(resolved.is_empty());
    }

    #[test]
    fn cli_llm_url_new_flag() {
        let cli = Cli::try_parse_from([
            "lad",
            "--url",
            "https://x.com",
            "--llm-url",
            "http://custom:1234",
        ])
        .unwrap();
        assert_eq!(cli.llm_url, "http://custom:1234");
    }

    #[test]
    fn cli_ollama_url_alias_still_works() {
        let cli = Cli::try_parse_from([
            "lad",
            "--url",
            "https://x.com",
            "--ollama-url",
            "http://legacy:1234",
        ])
        .unwrap();
        assert_eq!(cli.llm_url, "http://legacy:1234");
    }

    #[test]
    fn cli_llm_model_new_flag() {
        let cli =
            Cli::try_parse_from(["lad", "--url", "https://x.com", "--llm-model", "gpt-4"]).unwrap();
        assert_eq!(cli.llm_model, "gpt-4");
    }

    #[test]
    fn cli_model_alias_still_works() {
        let cli =
            Cli::try_parse_from(["lad", "--url", "https://x.com", "--model", "llama3"]).unwrap();
        assert_eq!(cli.llm_model, "llama3");
    }

    #[test]
    fn cli_backend_defaults_to_auto() {
        let cli = Cli::try_parse_from(["lad", "--url", "https://x.com"]).unwrap();
        assert_eq!(cli.backend, "auto");
    }
}
