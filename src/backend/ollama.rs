//! Ollama backend for the browser pilot.
//! Talks to Ollama's /api/generate endpoint.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::pilot::{Action, PilotBackend, Step};
use crate::semantic::SemanticView;

pub struct OllamaBackend {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

impl OllamaBackend {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into(),
            model: model.into(),
        }
    }
}

#[derive(Serialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    stream: bool,
    options: GenerateOptions,
}

#[derive(Serialize)]
struct GenerateOptions {
    temperature: f32,
    num_predict: u32,
}

#[derive(Deserialize)]
struct GenerateResponse {
    response: String,
}

#[async_trait]
impl PilotBackend for OllamaBackend {
    async fn decide(
        &self,
        view: &SemanticView,
        goal: &str,
        history: &[Step],
    ) -> Result<Action, crate::Error> {
        let prompt = build_prompt(view, goal, history);
        tracing::debug!(prompt_len = prompt.len(), "sending to ollama");

        let req = GenerateRequest {
            model: self.model.clone(),
            prompt,
            stream: false,
            options: GenerateOptions {
                temperature: 0.1,
                num_predict: 2048,
            },
        };

        let resp = self
            .client
            .post(format!("{}/api/generate", self.base_url))
            .json(&req)
            .send()
            .await
            .map_err(|e| crate::Error::Backend(format!("ollama request failed: {e}")))?;

        let body: GenerateResponse = resp
            .json()
            .await
            .map_err(|e| crate::Error::Backend(format!("ollama response parse failed: {e}")))?;

        tracing::debug!(response_len = body.response.len(), "ollama responded");

        parse_action(&body.response)
    }

    fn name(&self) -> &str {
        "ollama"
    }
}

fn build_prompt(view: &SemanticView, goal: &str, history: &[Step]) -> String {
    let mut prompt = String::with_capacity(1024);

    prompt.push_str("You are a browser pilot. Execute the goal step by step.\n\n");
    prompt.push_str(&format!("GOAL: {goal}\n\n"));
    prompt.push_str(&view.to_prompt());

    if !history.is_empty() {
        prompt.push_str("\nPREVIOUS ACTIONS:\n");
        for step in history.iter().rev().take(5) {
            prompt.push_str(&format!("- {:?}\n", step.action));
        }
    }

    prompt.push_str("\nRespond with ONLY a JSON object. Valid actions:\n");
    prompt.push_str(r#"{"action":"type","element":<id>,"value":"<text>","reasoning":"<why>"}"#);
    prompt.push('\n');
    prompt.push_str(r#"{"action":"click","element":<id>,"reasoning":"<why>"}"#);
    prompt.push('\n');
    prompt.push_str(r#"{"action":"wait","reasoning":"<why>"}"#);
    prompt.push('\n');
    prompt.push_str(r#"{"action":"done","result":{...},"reasoning":"<why>"}"#);
    prompt.push('\n');
    prompt.push_str(r#"{"action":"escalate","reason":"<why>"}"#);
    prompt.push_str("\n\nJSON:\n");

    prompt
}

/// Parse the LLM response into an Action.
/// Handles Qwen3's <think>...</think> blocks by stripping them.
fn parse_action(response: &str) -> Result<Action, crate::Error> {
    // Strip <think>...</think> blocks (Qwen3 reasoning)
    let clean = strip_think_tags(response);
    let trimmed = clean.trim();

    tracing::debug!(clean_response = %trimmed, "after stripping think tags");

    // Find the JSON object in the response
    let json_str = extract_json(trimmed).ok_or_else(|| {
        crate::Error::Backend(format!(
            "no JSON found in LLM response (len={}): {}",
            trimmed.len(),
            &trimmed[..trimmed.len().min(300)]
        ))
    })?;

    serde_json::from_str::<Action>(json_str).map_err(|e| {
        crate::Error::Backend(format!("failed to parse action JSON: {e}\nraw: {json_str}"))
    })
}

fn strip_think_tags(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_think = false;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if !in_think && c == '<' {
            // Check for <think>
            let rest: String = chars.clone().take(6).collect();
            if rest == "think>" {
                in_think = true;
                for _ in 0..6 {
                    chars.next();
                }
                continue;
            }
        }
        if in_think && c == '<' {
            // Check for </think>
            let rest: String = chars.clone().take(7).collect();
            if rest == "/think>" {
                in_think = false;
                for _ in 0..7 {
                    chars.next();
                }
                continue;
            }
        }
        if !in_think {
            result.push(c);
        }
    }
    result
}

fn extract_json(s: &str) -> Option<&str> {
    // Try to find a JSON object first
    if let Some(result) = extract_balanced(s, b'{', b'}') {
        return Some(result);
    }
    // If wrapped in array, extract the first object from the array
    if let Some(arr) = extract_balanced(s, b'[', b']') {
        return extract_balanced(arr, b'{', b'}');
    }
    None
}

fn extract_balanced(s: &str, open: u8, close: u8) -> Option<&str> {
    let start = s.as_bytes().iter().position(|&b| b == open)?;
    let mut depth = 0;
    for (i, &b) in s.as_bytes().iter().enumerate().skip(start) {
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(&s[start..=i]);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_think_tags_works() {
        let input = "<think>I should click the button</think>{\"action\":\"click\",\"element\":0,\"reasoning\":\"submit form\"}";
        let result = strip_think_tags(input);
        assert!(result.contains("action"));
        assert!(!result.contains("think"));
    }

    #[test]
    fn extract_json_from_mixed_text() {
        let input = "Sure, here's the action:\n{\"action\":\"click\",\"element\":2,\"reasoning\":\"test\"}\nDone.";
        let json = extract_json(input).unwrap();
        assert_eq!(json, r#"{"action":"click","element":2,"reasoning":"test"}"#);
    }

    #[test]
    fn parse_click_action() {
        let json = r#"{"action":"click","element":2,"reasoning":"submit the form"}"#;
        let action = parse_action(json).unwrap();
        assert!(matches!(action, Action::Click { element: 2, .. }));
    }

    #[test]
    fn parse_type_action() {
        let json = r#"{"action":"type","element":0,"value":"test@example.com","reasoning":"fill email"}"#;
        let action = parse_action(json).unwrap();
        assert!(matches!(action, Action::Type { element: 0, .. }));
    }

    #[test]
    fn parse_done_action() {
        let json = r#"{"action":"done","result":{"login":true},"reasoning":"dashboard loaded"}"#;
        let action = parse_action(json).unwrap();
        assert!(matches!(action, Action::Done { .. }));
    }
}
