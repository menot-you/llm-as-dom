//! Navigation tools: `lad_back`, `lad_dialog`.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{mcp_err, no_active_page};
use crate::params::DialogParams;

use llm_as_dom::pilot;

impl LadServer {
    /// Navigate back in browser history.
    ///
    /// FIX-R3-02: Hold a single lock through the entire back-navigate-wait-refresh
    /// cycle to eliminate the stale URL window where concurrent tools could observe
    /// inconsistent state between the history.back() and the view refresh.
    pub(crate) async fn tool_lad_back(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        tracing::info!("lad_back");

        let mut active = self.active_page.lock().await;
        let ap = active.as_mut().ok_or_else(no_active_page)?;

        ap.page.eval_js("history.back()").await.map_err(mcp_err)?;

        // CHAOS-14: Use wait_for_navigation instead of fixed sleep to eliminate
        // the SSRF race window where concurrent tools could observe stale state.
        if let Err(e) = ap.page.wait_for_navigation().await {
            tracing::warn!(error = %e, "wait_for_navigation after history.back() failed, falling back to sleep");
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }

        // FIX-1: Check URL safety after history.back() navigation settles.
        if let Ok(ref back_url) = ap.page.url().await
            && !llm_as_dom::sanitize::is_safe_url(back_url)
        {
            return Err(mcp_err(format!(
                "blocked: history.back() navigated to unsafe URL {back_url}"
            )));
        }

        // Refresh view and URL while still holding the lock
        let view = llm_as_dom::a11y::extract_semantic_view(ap.page.as_ref())
            .await
            .map_err(mcp_err)?;
        if let Ok(url) = ap.page.url().await {
            ap.url = url;
        }
        ap.view = view.clone();

        Ok(CallToolResult::success(vec![Content::text(
            view.to_prompt(),
        )]))
    }

    /// Handle JavaScript dialogs (alert, confirm, prompt).
    pub(crate) async fn tool_lad_dialog(
        &self,
        params: Parameters<DialogParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(action = %p.action, text = ?p.text, "lad_dialog");

        let active = self.active_page.lock().await;
        let ap = active.as_ref().ok_or_else(no_active_page)?;

        // Ensure dialog overrides are installed
        let setup_js = r#"
            if (!window.__lad_dialogs) {
                window.__lad_dialogs = [];
                window.__lad_dialog_auto = 'accept';
                window.__lad_dialog_response = '';

                window.alert = function(msg) {
                    window.__lad_dialogs.push({
                        type: 'alert', message: String(msg),
                        timestamp: Date.now()
                    });
                };
                window.confirm = function(msg) {
                    window.__lad_dialogs.push({
                        type: 'confirm', message: String(msg),
                        timestamp: Date.now()
                    });
                    return window.__lad_dialog_auto === 'accept';
                };
                window.prompt = function(msg, def) {
                    window.__lad_dialogs.push({
                        type: 'prompt', message: String(msg),
                        default: def || '', timestamp: Date.now()
                    });
                    if (window.__lad_dialog_auto !== 'accept') return null;
                    return window.__lad_dialog_response || def || '';
                };
            }
        "#;
        ap.page.eval_js(setup_js).await.map_err(mcp_err)?;

        match p.action.as_str() {
            "accept" => {
                let text_escaped = pilot::js_escape(p.text.as_deref().unwrap_or(""));
                let js = format!(
                    "window.__lad_dialog_auto = 'accept'; \
                     window.__lad_dialog_response = '{text_escaped}';",
                );
                ap.page.eval_js(&js).await.map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(
                    r#"{"status": "dialogs will be auto-accepted"}"#.to_string(),
                )]))
            }
            "dismiss" => {
                ap.page
                    .eval_js("window.__lad_dialog_auto = 'dismiss';")
                    .await
                    .map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(
                    r#"{"status": "dialogs will be auto-dismissed"}"#.to_string(),
                )]))
            }
            "status" => {
                let result = ap
                    .page
                    .eval_js("JSON.stringify(window.__lad_dialogs || [])")
                    .await
                    .map_err(mcp_err)?;
                let text = result.as_str().unwrap_or("[]");
                Ok(CallToolResult::success(vec![Content::text(
                    text.to_string(),
                )]))
            }
            other => Err(rmcp::ErrorData::invalid_params(
                format!(
                    "unknown dialog action '{}' — use 'accept', 'dismiss', or 'status'",
                    other
                ),
                None,
            )),
        }
    }
}
