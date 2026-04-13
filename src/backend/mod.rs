//! LLM backend implementations for the browser pilot.

pub mod anthropic;
pub mod generic;
pub mod openai;
pub mod playbook;

/// FIX-8: Canonical backend factory — auto-detect which LLM backend to use.
///
/// Detection order (FIX-8): URL patterns FIRST, then credential-based fallback.
/// This prevents routing to OpenAI when the URL points to Anthropic or Z.AI.
///
/// Called from both the CLI binary (`main.rs`) and the MCP server
/// (`mcp_server/mod.rs`) to eliminate duplicated detection logic.
pub fn create_backend(
    url: &str,
    model: &str,
    max_prompt_length: Option<usize>,
) -> Box<dyn crate::pilot::PilotBackend> {
    let llm_cred = std::env::var("LAD_LLM_API_KEY")
        .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
        .or_else(|_| std::env::var("Z_AI_API_KEY"))
        .unwrap_or_default();

    let lower_url = url.to_lowercase();

    // FIX-8: Check URL patterns FIRST — URL is more specific than credentials.
    if lower_url.contains("anthropic") || lower_url.contains("z.ai") {
        Box::new(anthropic::AnthropicBackend::new(
            &llm_cred,
            model,
            max_prompt_length,
        ))
    } else if lower_url.contains("openai") {
        Box::new(openai::OpenAiBackend::new(
            &llm_cred,
            model,
            max_prompt_length,
        ))
    } else if !llm_cred.is_empty() {
        // Has credentials but no URL hint — default to OpenAI-compatible
        Box::new(openai::OpenAiBackend::new(
            &llm_cred,
            model,
            max_prompt_length,
        ))
    } else {
        // No credentials, no URL match — generic (Ollama-style)
        Box::new(generic::GenericLlmBackend::new(
            url,
            model,
            max_prompt_length,
        ))
    }
}
