//! Anthropic-compatible backend — cloud/local LLM for browser piloting.
//!
//! Uses any API that respects the Anthropic Messages API format.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::pilot::{Action, PilotBackend, Step};
use crate::semantic::SemanticView;

/// Anthropic-compatible cloud backend.
pub struct AnthropicBackend {
    client: reqwest::Client,
    api_key: String,
    model: String,
    max_prompt_length: usize,
    base_url: String,
}

impl AnthropicBackend {
    /// Create a new Anthropic-compatible backend.
    ///
    /// Reads `LAD_LLM_API_KEY` (or deprecated `ANTHROPIC_API_KEY`) from environment
    /// if `api_key` is empty. Base URL falls back from `LAD_LLM_URL` to
    /// `ANTHROPIC_BASE_URL` to the default Anthropic endpoint.
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        max_prompt_length: Option<usize>,
    ) -> Self {
        let max_prompt_length = max_prompt_length.unwrap_or(40000);
        let cred = {
            let k = api_key.into();
            if k.is_empty() {
                std::env::var("LAD_LLM_API_KEY")
                    .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
                    .unwrap_or_default()
            } else {
                k
            }
        };
        Self {
            client: reqwest::Client::new(),
            api_key: cred,
            model: model.into(),
            max_prompt_length,
            base_url: std::env::var("LAD_LLM_URL")
                .or_else(|_| std::env::var("ANTHROPIC_BASE_URL"))
                .unwrap_or_else(|_| "https://api.anthropic.com".into()),
        }
    }
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<Message>,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<ContentBlock>,
    #[allow(dead_code)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ContentBlock {
    text: String,
}

#[derive(Deserialize)]
struct Usage {
    #[allow(dead_code)]
    input_tokens: u32,
    #[allow(dead_code)]
    output_tokens: u32,
}

#[async_trait]
impl PilotBackend for AnthropicBackend {
    async fn decide(
        &self,
        view: &SemanticView,
        goal: &str,
        history: &[Step],
    ) -> Result<Action, crate::Error> {
        let prompt = super::generic::build_prompt(view, goal, history, self.max_prompt_length);
        tracing::debug!(prompt_len = prompt.len(), model = %self.model, "sending to Anthropic-compatible API");

        let req = AnthropicRequest {
            model: self.model.clone(),
            max_tokens: 300,
            messages: vec![Message {
                role: "user".into(),
                content: prompt,
            }],
        };

        let resp = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("Content-Type", "application/json")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&req)
            .send()
            .await
            .map_err(|e| crate::Error::Backend(format!("Anthropic request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(crate::Error::Backend(format!(
                "Anthropic API error {status}: {body}"
            )));
        }

        let body: AnthropicResponse = resp
            .json()
            .await
            .map_err(|e| crate::Error::Backend(format!("Anthropic response parse failed: {e}")))?;

        let text = body
            .content
            .first()
            .map(|c| c.text.clone())
            .unwrap_or_default();

        tracing::debug!(
            response_len = text.len(),
            "Anthropic-compatible backend responded"
        );

        super::generic::parse_action(&text)
    }
}
