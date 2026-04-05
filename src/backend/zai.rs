//! Z.AI (Zhipu) backend — cloud LLM for browser piloting.
//!
//! Uses the Anthropic-compatible API at `api.z.ai`.
//! Models: glm-4.7, glm-4.5-air, glm-4.5-flash.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::pilot::{Action, PilotBackend, Step};
use crate::semantic::SemanticView;

/// Z.AI cloud backend using the Anthropic-compatible API.
pub struct ZaiBackend {
    client: reqwest::Client,
    api_key: String,
    model: String,
    max_prompt_length: usize,
    base_url: String,
}

impl ZaiBackend {
    /// Create a new Z.AI backend.
    ///
    /// Reads `LAD_LLM_API_KEY` (or deprecated `Z_AI_API_KEY`) from environment
    /// if `api_key` is empty. Base URL falls back from `LAD_LLM_URL` to
    /// `Z_AI_BASE_URL` to the default Z.AI endpoint.
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        max_prompt_length: Option<usize>,
    ) -> Self {
        let max_prompt_length = max_prompt_length.unwrap_or(10000);
        let cred = {
            let k = api_key.into();
            if k.is_empty() {
                std::env::var("LAD_LLM_API_KEY")
                    .or_else(|_| std::env::var("Z_AI_API_KEY"))
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
                .or_else(|_| std::env::var("Z_AI_BASE_URL"))
                .unwrap_or_else(|_| "https://api.z.ai/api/anthropic".into()),
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
impl PilotBackend for ZaiBackend {
    async fn decide(
        &self,
        view: &SemanticView,
        goal: &str,
        history: &[Step],
    ) -> Result<Action, crate::Error> {
        let prompt = super::generic::build_prompt(view, goal, history, self.max_prompt_length);
        tracing::debug!(prompt_len = prompt.len(), model = %self.model, "sending to Z.AI");

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
            .map_err(|e| crate::Error::Backend(format!("Z.AI request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(crate::Error::Backend(format!(
                "Z.AI API error {status}: {body}"
            )));
        }

        let body: AnthropicResponse = resp
            .json()
            .await
            .map_err(|e| crate::Error::Backend(format!("Z.AI response parse failed: {e}")))?;

        let text = body
            .content
            .first()
            .map(|c| c.text.clone())
            .unwrap_or_default();

        tracing::debug!(response_len = text.len(), "Z.AI responded");

        super::generic::parse_action(&text)
    }
}
