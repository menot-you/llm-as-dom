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

    /// Enable playbook learning. When set, a successful run is synthesized
    /// into a reusable playbook and written to `--learn-dir`.
    #[arg(long, default_value_t = false)]
    learn: bool,

    /// Optional explicit playbook name. Defaults to a name derived from the goal.
    #[arg(long)]
    learn_name: Option<String>,

    /// Comma-separated list of params to templatize in `Type` / `Select` values,
    /// e.g. `--learn-params "email=octocat,password=hunter2"`.
    ///
    /// WARNING: argv is visible via `ps aux`, `/proc/self/cmdline`, shell
    /// history, and core dumps. Prefer `--learn-params-file` or the
    /// `LAD_LEARN_PARAMS` env var for secret values.
    #[arg(long)]
    learn_params: Option<String>,

    /// Read learn-params from a file (`KEY=VALUE` per line; `#` starts
    /// a comment; blank lines ignored). Highest priority source — overrides
    /// both `--learn-params` and `LAD_LEARN_PARAMS`.
    #[arg(long)]
    learn_params_file: Option<String>,

    /// Read learn-params from the `LAD_LEARN_PARAMS` env var (same format
    /// as `--learn-params`). Middle priority — overrides `--learn-params`
    /// but is overridden by `--learn-params-file`.
    #[arg(long, default_value_t = false)]
    learn_params_env: bool,

    /// Directory to write the learned playbook to. Defaults to `.lad/playbooks`.
    #[arg(long, default_value = ".lad/playbooks")]
    learn_dir: String,
}

/// Parse a `KEY=VALUE,KEY=VALUE` string into a map. Entries without `=` or
/// with an empty key are skipped.
fn parse_learn_params(raw: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for pair in raw.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let key = k.trim();
        if key.is_empty() {
            continue;
        }
        out.insert(key.to_string(), v.trim().to_string());
    }
    out
}

/// Parse a `KEY=VALUE` file (one pair per line). Supports `#` line comments
/// and blank lines. Returns a map even if some lines are malformed.
fn parse_learn_params_file(contents: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((k, v)) = trimmed.split_once('=') else {
            continue;
        };
        let key = k.trim();
        if key.is_empty() {
            continue;
        }
        out.insert(key.to_string(), v.trim().to_string());
    }
    out
}

/// Merge learn-params from all three sources in priority order:
/// argv (lowest) < env < file (highest).
///
/// Non-fatal contract (issue #54): a missing or unreadable
/// `--learn-params-file` must NOT abort the run. This function warns and
/// continues with whatever argv/env provided, matching the PR #35 body
/// promise "learning is non-fatal: synthesis or save failures log a
/// warning and never break the run". The file is the only IO source;
/// argv/env parsing are infallible string ops.
///
/// Emits a single `tracing::warn!` when any source carried a key matching
/// the secret-name regex — so operators see "do NOT commit .lad/playbooks/
/// unless .gitignore'd" at least once.
fn merge_learn_params(
    argv: Option<&str>,
    env_enabled: bool,
    file_path: Option<&str>,
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    if let Some(s) = argv {
        for (k, v) in parse_learn_params(s) {
            out.insert(k, v);
        }
    }
    if env_enabled && let Ok(env_val) = std::env::var("LAD_LEARN_PARAMS") {
        for (k, v) in parse_learn_params(&env_val) {
            out.insert(k, v);
        }
    }
    if let Some(path) = file_path {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                for (k, v) in parse_learn_params_file(&contents) {
                    out.insert(k, v);
                }
            }
            Err(e) => {
                // Non-fatal: log and continue with argv/env only.
                tracing::warn!(
                    learn_params_file = %path,
                    error = %e,
                    "learn-params-file unreadable — continuing without it"
                );
            }
        }
    }
    if out.keys().any(|k| llm_as_dom::playbook::is_secret_key(k)) {
        tracing::warn!(
            "learned playbook contains secrets; do NOT commit .lad/playbooks/ unless .gitignore'd"
        );
    }
    out
}

// Note: the `redact_action_for_learn` helper lives in `pilot::runner`
// (exported from the library) so the runner's per-step log and this
// binary's summary print go through identical redaction logic.

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
                &cli.llm_url,
            )),
            "openai" => Box::new(backend::openai::OpenAiBackend::new(
                "",
                &cli.llm_model,
                None,
                &cli.llm_url,
            )),
            "ollama" => Box::new(backend::generic::GenericLlmBackend::new(
                &cli.llm_url,
                &cli.llm_model,
                None,
            )),
            _ => backend::create_backend(&cli.llm_url, &cli.llm_model, None),
        };

        let playbook_path = std::path::PathBuf::from(&cli.playbook_dir);
        let learn_config = if cli.learn {
            let params = merge_learn_params(
                cli.learn_params.as_deref(),
                cli.learn_params_env,
                cli.learn_params_file.as_deref(),
            );
            Some(pilot::LearnConfig {
                name: cli.learn_name.clone(),
                explicit_params: params,
                output_dir: std::path::PathBuf::from(&cli.learn_dir),
            })
        } else {
            None
        };
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
            learn: learn_config,
            // FIX-4c: pass the canonical `--url` input so playbook learning
            // derives the pattern from the entry point, not from post-OAuth
            // view.url.
            initial_url: Some(cli.url.clone()),
        };

        let learn_on = config.learn.is_some();
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
        // Final action: redact Type values in learn mode so the CLI summary
        // doesn't echo raw credentials to stdout.
        if learn_on {
            println!(
                "\nFinal: {}",
                pilot::redact_action_for_learn(&result.final_action)
            );
        } else {
            println!("\nFinal: {:?}", result.final_action);
        }

        for step in &result.steps {
            if learn_on {
                println!(
                    "  [{}] {:?} {} ({:.1}s)",
                    step.index,
                    step.source,
                    pilot::redact_action_for_learn(&step.action),
                    step.duration.as_secs_f64()
                );
            } else {
                println!(
                    "  [{}] {:?} {:?} ({:.1}s)",
                    step.index,
                    step.source,
                    step.action,
                    step.duration.as_secs_f64()
                );
            }
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

    #[test]
    fn cli_learn_defaults_off() {
        let cli = Cli::try_parse_from(["lad", "--url", "https://x.com"]).unwrap();
        assert!(!cli.learn);
        assert!(cli.learn_name.is_none());
        assert!(cli.learn_params.is_none());
        assert_eq!(cli.learn_dir, ".lad/playbooks");
    }

    #[test]
    fn cli_learn_flags_parse() {
        let cli = Cli::try_parse_from([
            "lad",
            "--url",
            "https://x.com",
            "--learn",
            "--learn-name",
            "gh-login",
            "--learn-params",
            "email=octocat,password=hunter2",
            "--learn-dir",
            "/tmp/pb",
        ])
        .unwrap();
        assert!(cli.learn);
        assert_eq!(cli.learn_name.as_deref(), Some("gh-login"));
        assert_eq!(
            cli.learn_params.as_deref(),
            Some("email=octocat,password=hunter2")
        );
        assert_eq!(cli.learn_dir, "/tmp/pb");
    }

    #[test]
    fn parse_learn_params_basic() {
        let map = parse_learn_params("email=alice,password=secret");
        assert_eq!(map.get("email").map(String::as_str), Some("alice"));
        assert_eq!(map.get("password").map(String::as_str), Some("secret"));
    }

    #[test]
    fn parse_learn_params_trims_whitespace() {
        let map = parse_learn_params(" email = alice , password = secret ");
        assert_eq!(map.get("email").map(String::as_str), Some("alice"));
        assert_eq!(map.get("password").map(String::as_str), Some("secret"));
    }

    #[test]
    fn parse_learn_params_skips_invalid_entries() {
        let map = parse_learn_params("=orphan,,key_only,good=yes");
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("good").map(String::as_str), Some("yes"));
    }

    // ── Fix 2c: CLI params-file + env ────────────────────────────────

    #[test]
    fn parse_learn_params_file_tmpdir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("params");
        std::fs::write(&path, "password=hunter2\n#comment line\n\nemail=a@b.c\n").unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        let map = parse_learn_params_file(&contents);
        assert_eq!(map.get("password").map(String::as_str), Some("hunter2"));
        assert_eq!(map.get("email").map(String::as_str), Some("a@b.c"));
        assert_eq!(map.len(), 2, "comment + blank lines must be skipped");
    }

    #[test]
    fn env_overrides_argv_file_overrides_env() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("params");
        std::fs::write(&path, "key=from_file\n").unwrap();

        // SAFETY: single-threaded test, env mutation is scoped to this test.
        unsafe {
            std::env::set_var("LAD_LEARN_PARAMS", "key=from_env");
        }

        // argv only.
        let argv_only = merge_learn_params(Some("key=from_argv"), false, None);
        assert_eq!(argv_only.get("key").map(String::as_str), Some("from_argv"));

        // argv + env → env wins.
        let argv_env = merge_learn_params(Some("key=from_argv"), true, None);
        assert_eq!(argv_env.get("key").map(String::as_str), Some("from_env"));

        // argv + env + file → file wins.
        let all = merge_learn_params(Some("key=from_argv"), true, Some(path.to_str().unwrap()));
        assert_eq!(all.get("key").map(String::as_str), Some("from_file"));

        unsafe {
            std::env::remove_var("LAD_LEARN_PARAMS");
        }
    }

    /// Issue #54 — missing/unreadable `--learn-params-file` must NOT abort
    /// the run. The function warns and returns whatever argv/env provided.
    #[test]
    fn merge_learn_params_non_fatal_missing_file() {
        let got = merge_learn_params(
            Some("key=from_argv"),
            false,
            Some("/does/not/exist/learn-params.json"),
        );
        // File missing → we keep argv.
        assert_eq!(got.get("key").map(String::as_str), Some("from_argv"));
        assert_eq!(got.len(), 1, "only argv should have survived");
    }

    /// Issue #54 — an unreadable file (e.g. permission denied) must also
    /// fall through without aborting. We simulate via a directory path
    /// (read_to_string errors on dirs).
    #[test]
    fn merge_learn_params_non_fatal_unreadable_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Pointing at the directory itself — read_to_string returns IsADirectory.
        let got = merge_learn_params(Some("k=v"), false, Some(tmp.path().to_str().unwrap()));
        assert_eq!(got.get("k").map(String::as_str), Some("v"));
    }

    // ── Fix 2d: redact_action_for_learn ──────────────────────────────

    #[test]
    fn redact_action_for_learn_hides_type_value() {
        let action = llm_as_dom::pilot::Action::Type {
            element: 1,
            value: "s3cret".into(),
            reasoning: "test".into(),
        };
        let rendered = llm_as_dom::pilot::redact_action_for_learn(&action);
        assert!(
            rendered.contains("len=6"),
            "rendered output should show length marker, got {rendered:?}"
        );
        assert!(
            !rendered.contains("s3cret"),
            "raw value must not appear, got {rendered:?}"
        );
    }
}
