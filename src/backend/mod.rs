//! LLM backend implementations for the browser pilot.

pub mod anthropic;
pub mod generic;
pub mod openai;
pub mod playbook;

/// FIX-9: Canonical backend factory — auto-detect which LLM backend to use
/// based on URL, env vars, and credential availability.
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

    if !llm_cred.is_empty() || url.contains("openai") {
        Box::new(openai::OpenAiBackend::new(
            &llm_cred,
            model,
            max_prompt_length,
        ))
    } else if url.contains("z.ai") || url.contains("anthropic") {
        Box::new(anthropic::AnthropicBackend::new(
            &llm_cred,
            model,
            max_prompt_length,
        ))
    } else {
        Box::new(generic::GenericLlmBackend::new(
            url,
            model,
            max_prompt_length,
        ))
    }
}
