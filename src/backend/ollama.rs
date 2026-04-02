//! Ollama backend for the browser pilot.
//!
//! Talks to Ollama's `/api/generate` endpoint with low temperature
//! and a structured JSON-only prompt.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::pilot::{Action, PilotBackend, Step};
use crate::semantic::SemanticView;

/// LLM backend that calls a local Ollama instance.
pub struct OllamaBackend {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

impl OllamaBackend {
    /// Create a new backend pointing at the given Ollama URL and model.
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into(),
            model: model.into(),
        }
    }
}

/// Request body for Ollama's `/api/generate`.
#[derive(Serialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    stream: bool,
    options: GenerateOptions,
}

/// Sampling options sent to Ollama.
#[derive(Serialize)]
struct GenerateOptions {
    temperature: f32,
    num_predict: u32,
}

/// Response body from Ollama's `/api/generate`.
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
}

/// Build the LLM prompt with system instructions, few-shot examples, and page state.
///
/// The prompt is structured to force a single JSON response with no markdown or explanation.
pub fn build_prompt(view: &SemanticView, goal: &str, history: &[Step]) -> String {
    let mut prompt = String::with_capacity(2048);

    // System instruction — explicit single-JSON constraint
    prompt.push_str(
        "SYSTEM: You are a browser automation pilot. \
         Respond with exactly ONE JSON object. \
         No markdown, no explanation, no extra text. \
         Do not wrap in ```json blocks. \
         Do not return multiple actions.\n\n",
    );

    prompt.push_str(&format!("GOAL: {goal}\n\n"));
    prompt.push_str(&view.to_prompt());

    if !history.is_empty() {
        prompt.push_str("\nPREVIOUS ACTIONS:\n");
        for step in history.iter().rev().take(5) {
            prompt.push_str(&format!("- {:?}\n", step.action));
        }
    }

    // Schema reference
    prompt.push_str("\nVALID ACTIONS (respond with exactly one):\n");
    prompt.push_str(r#"{"action":"type","element":<id>,"value":"<text>","reasoning":"<why>"}"#);
    prompt.push('\n');
    prompt.push_str(r#"{"action":"click","element":<id>,"reasoning":"<why>"}"#);
    prompt.push('\n');
    prompt.push_str(r#"{"action":"select","element":<id>,"value":"<text>","reasoning":"<why>"}"#);
    prompt.push('\n');
    prompt
        .push_str(r#"{"action":"scroll","direction":"<up|down|left|right>","reasoning":"<why>"}"#);
    prompt.push('\n');
    prompt.push_str(r#"{"action":"wait","reasoning":"<why>"}"#);
    prompt.push('\n');
    prompt.push_str(r#"{"action":"done","result":{"success":true},"reasoning":"<why>"}"#);
    prompt.push('\n');
    prompt.push_str(r#"{"action":"escalate","reason":"<why>"}"#);

    // Few-shot examples keyed to scenario type
    prompt.push_str("\n\nFEW-SHOT EXAMPLES:\n");
    push_few_shot_examples(&mut prompt, goal);

    prompt.push_str("\nJSON:\n");
    prompt
}

/// Append scenario-relevant few-shot examples to the prompt.
///
/// Picks examples that match the goal type: login, search, todo/task, navigation, or generic.
fn push_few_shot_examples(prompt: &mut String, goal: &str) {
    let g = goal.to_lowercase();

    if g.contains("login") || g.contains("sign in") || g.contains("log in") {
        prompt.push_str(
            r#"Goal: "login as alice@test.com password s3cret"
[0] Input type=email "Email" name="email"
[1] Input type=password "Password" name="password"
[2] Button "Sign In"
Step 1: {"action":"type","element":0,"value":"alice@test.com","reasoning":"fill email field"}
Step 2: {"action":"type","element":1,"value":"s3cret","reasoning":"fill password field"}
Step 3: {"action":"click","element":2,"reasoning":"submit login form"}
"#,
        );
    } else if g.contains("search") || g.contains("find") || g.contains("look up") {
        prompt.push_str(
            r#"Goal: "search for rust tutorials"
[0] Input type=search "Search" name="q"
[1] Button "Search"
Step 1: {"action":"type","element":0,"value":"rust tutorials","reasoning":"fill search box"}
Step 2: {"action":"click","element":1,"reasoning":"submit search"}
"#,
        );
    } else if g.contains("todo") || g.contains("task") || g.contains("add") || g.contains("create")
    {
        prompt.push_str(
            r#"Goal: "add a todo 'buy milk'"
[0] Input type=text "New task" name="task"
[1] Button "Add"
Step 1: {"action":"type","element":0,"value":"buy milk","reasoning":"fill todo input"}
Step 2: {"action":"click","element":1,"reasoning":"submit new todo"}
"#,
        );
    } else if g.contains("click") || g.contains("go to") || g.contains("navigate") {
        prompt.push_str(
            r#"Goal: "click About"
[0] Link "Home" href="/home"
[1] Link "About" href="/about"
[2] Link "Contact" href="/contact"
Step 1: {"action":"click","element":1,"reasoning":"click the About link matching the goal"}
"#,
        );
    } else {
        // Generic fallback example
        prompt.push_str(
            r#"Goal: "fill form with name=John email=j@test.com"
[0] Input type=text "Full Name" name="name"
[1] Input type=email "Email" name="email"
[2] Button "Submit"
Step 1: {"action":"type","element":0,"value":"John","reasoning":"fill name field"}
Step 2: {"action":"type","element":1,"value":"j@test.com","reasoning":"fill email field"}
Step 3: {"action":"click","element":2,"reasoning":"submit the form"}
"#,
        );
    }
}

/// Parse the LLM response into an Action.
/// Handles Qwen3's <think>...</think> blocks by stripping them.
pub fn parse_action(response: &str) -> Result<Action, crate::Error> {
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

pub fn strip_think_tags(s: &str) -> String {
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

pub fn extract_json(s: &str) -> Option<&str> {
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

pub fn extract_balanced(s: &str, open: u8, close: u8) -> Option<&str> {
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
        let json =
            r#"{"action":"type","element":0,"value":"test@example.com","reasoning":"fill email"}"#;
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
