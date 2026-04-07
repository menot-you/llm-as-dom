//! Action enum and execution logic.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::util::js_escape;

/// A single action the pilot can take on the page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Click an interactive element by its `data-lad-id`.
    Click { element: u32, reasoning: String },
    /// Type text into an input/textarea by its `data-lad-id`.
    Type {
        element: u32,
        value: String,
        reasoning: String,
    },
    /// Select an option in a `<select>` element.
    Select {
        element: u32,
        value: String,
        reasoning: String,
    },
    /// Scroll the viewport in a given direction.
    Scroll {
        direction: String,
        reasoning: String,
    },
    /// Pause and wait for the page to settle.
    Wait { reasoning: String },
    /// Goal achieved -- includes the structured result.
    Done {
        result: serde_json::Value,
        reasoning: String,
    },
    /// Navigate to a different URL (multi-page flow support).
    Navigate { url: String, reasoning: String },
    /// Cannot proceed -- escalate to the caller.
    Escalate { reason: String },
}

/// Execute an action on the page via the engine-agnostic page handle.
pub async fn execute_action(
    page: &dyn crate::engine::PageHandle,
    action: &Action,
) -> Result<(), crate::Error> {
    match action {
        Action::Click { element, .. } => {
            let js = format!(
                r#"document.querySelector('[data-lad-id="{}"]')?.click()"#,
                element
            );
            let _ = page.eval_js(&js).await?;
        }
        Action::Type { element, value, .. } => {
            let escaped = js_escape(value);
            let js = format!(
                r#"(() => {{
                    const el = document.querySelector('[data-lad-id="{}"]');
                    if (el) {{
                        el.focus();
                        el.value = '{escaped}';
                        el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                        el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                    }}
                }})()"#,
                element,
            );
            let _ = page.eval_js(&js).await?;
        }
        Action::Select { element, value, .. } => {
            let escaped = js_escape(value);
            let js = format!(
                r#"(() => {{
                    const el = document.querySelector('[data-lad-id="{}"]');
                    if (el) {{ el.value = '{escaped}'; el.dispatchEvent(new Event('change', {{ bubbles: true }})); }}
                }})()"#,
                element,
            );
            let _ = page.eval_js(&js).await?;
        }
        Action::Scroll { direction, .. } => {
            let (x, y) = match direction.as_str() {
                "up" => (0, -300),
                "down" => (0, 300),
                "left" => (-300, 0),
                "right" => (300, 0),
                _ => (0, 300),
            };
            let js = format!("window.scrollBy({x}, {y})");
            let _ = page.eval_js(&js).await?;
        }
        Action::Wait { .. } => {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Action::Navigate { url, .. } => {
            if !crate::sanitize::is_safe_url(url) {
                return Err(crate::Error::ActionFailed(format!(
                    "blocked navigation to unsafe URL: {url}"
                )));
            }
            page.navigate(url).await?;
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
        Action::Done { .. } | Action::Escalate { .. } => {}
    }
    Ok(())
}

/// Execute an action with retry on failure (stale DOM recovery).
pub async fn execute_action_with_retry(
    page: &dyn crate::engine::PageHandle,
    action: &Action,
    max_retries: u32,
    total_retries: &mut u32,
) -> Result<(), crate::Error> {
    match execute_action(page, action).await {
        Ok(()) => Ok(()),
        Err(first_err) => {
            tracing::warn!(error = %first_err, "action execution failed, retrying");
            let mut last_err = first_err;

            for attempt in 1..=max_retries {
                *total_retries += 1;
                tracing::info!(attempt, max_retries, "retry: re-extracting DOM");
                tokio::time::sleep(Duration::from_millis(300)).await;

                match execute_action(page, action).await {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        tracing::warn!(attempt, error = %e, "retry failed");
                        last_err = e;
                    }
                }
            }

            Err(crate::Error::ActionFailed(format!(
                "action failed after {} retries: {}",
                max_retries, last_err
            )))
        }
    }
}
