//! OpenAI-compatible backend — cloud/local LLM for browser piloting.
//!
//! Uses any API that respects the OpenAI Chat Completions API format.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::pilot::{Action, PilotBackend, Step};
use crate::semantic::SemanticView;

/// OpenAI-compatible cloud backend.
pub struct OpenAiBackend {
    client: reqwest::Client,
    api_key: String,
    model: String,
    max_prompt_length: usize,
    base_url: String,
}

impl OpenAiBackend {
    /// Create a new OpenAI-compatible backend.
    ///
    /// Reads `LAD_LLM_API_KEY` (or `OPENAI_API_KEY`) from environment
    /// if `api_key` is empty. Base URL falls back from `LAD_LLM_URL` to
    /// `OPENAI_BASE_URL` to the default OpenAI endpoint.
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
                    .or_else(|_| std::env::var("OPENAI_API_KEY"))
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
                .or_else(|_| std::env::var("OPENAI_BASE_URL"))
                .unwrap_or_else(|_| "https://api.openai.com/v1".into()),
        }
    }
}

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<Message>,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: OutputMessage,
}

#[derive(Deserialize)]
struct OutputMessage {
    content: Option<String>,
}

#[async_trait]
impl PilotBackend for OpenAiBackend {
    async fn decide(
        &self,
        view: &SemanticView,
        goal: &str,
        history: &[Step],
    ) -> Result<Action, crate::Error> {
        let prompt = super::generic::build_prompt(view, goal, history, self.max_prompt_length);
        tracing::debug!(prompt_len = prompt.len(), model = %self.model, "sending to OpenAI-compatible API");

        let req = OpenAiRequest {
            model: self.model.clone(),
            messages: vec![Message {
                role: "user".into(),
                content: prompt,
            }],
        };

        // Trim any trailing slash from base url
        let base = self.base_url.trim_end_matches('/');

        let mut request_builder = self
            .client
            .post(format!("{}/chat/completions", base))
            .header("Content-Type", "application/json");

        if !self.api_key.is_empty() {
            request_builder =
                request_builder.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let resp = request_builder
            .json(&req)
            .send()
            .await
            .map_err(|e| crate::Error::Backend(format!("OpenAI request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(crate::Error::Backend(format!(
                "OpenAI API error {status}: {body}"
            )));
        }

        let body: OpenAiResponse = resp
            .json()
            .await
            .map_err(|e| crate::Error::Backend(format!("OpenAI response parse failed: {e}")))?;

        let text = body
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        tracing::debug!(
            response_len = text.len(),
            "OpenAI-compatible backend responded"
        );

        super::generic::parse_action(&text)
    }
}
