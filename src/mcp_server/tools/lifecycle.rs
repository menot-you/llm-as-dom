//! Lifecycle tools: `lad_close`, `lad_session`.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{mcp_err, to_pretty_json};
use crate::params::SessionParams;
use crate::state::McpSessionState;

impl LadServer {
    /// Close the browser and release all resources.
    pub(crate) async fn tool_lad_close(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        tracing::info!("lad_close");

        // FIX-11: Abort any active watch before closing the browser so
        // the polling task doesn't leak.
        if let Some(ws) = self.watch_state.lock().await.take() {
            ws.stop();
        }

        // Clear active page first
        *self.active_page.lock().await = None;

        // Close the engine if one was launched
        let mut engine_lock = self.engine.lock().await;
        if let Some(engine) = engine_lock.take() {
            engine.close().await.map_err(mcp_err)?;
        }

        Ok(CallToolResult::success(vec![Content::text(
            r#"{"status": "browser closed"}"#.to_string(),
        )]))
    }

    /// Inspect or reset the MCP session state.
    pub(crate) async fn tool_lad_session(
        &self,
        params: Parameters<SessionParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(action = %p.action, "lad_session");

        match p.action.as_str() {
            "get" => {
                let session = self.session.lock().await;
                let output = serde_json::to_value(&*session)
                    .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));
                Ok(CallToolResult::success(vec![Content::text(
                    to_pretty_json(&output),
                )]))
            }
            "clear" => {
                let mut session = self.session.lock().await;
                *session = McpSessionState::default();
                Ok(CallToolResult::success(vec![Content::text(
                    r#"{"status": "session cleared"}"#.to_string(),
                )]))
            }
            other => {
                let msg = format!(
                    "unknown session action '{}'. Valid actions: 'get', 'clear'.",
                    other
                );
                Err(rmcp::ErrorData::invalid_params(msg, None))
            }
        }
    }
}
