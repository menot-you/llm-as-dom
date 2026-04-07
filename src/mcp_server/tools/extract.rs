//! `lad_extract`, `lad_snapshot`, `lad_screenshot` tools.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{mcp_err, no_active_page, to_pretty_json};
use crate::params::{ExtractParams, SnapshotParams};

impl LadServer {
    /// Extract structured information from a web page.
    /// Returns interactive elements, visible text, page classification.
    /// Never returns raw HTML.
    pub(crate) async fn tool_lad_extract(
        &self,
        params: Parameters<ExtractParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        let (_page, mut view) = self.navigate_and_extract(&p.url).await?;

        if let Some(max_len) = p.max_length
            && view.visible_text.len() > max_len
        {
            let mut end = max_len;
            while !view.visible_text.is_char_boundary(end) && end > 0 {
                end -= 1;
            }
            view.visible_text.truncate(end);
        }

        let output = serde_json::json!({
            "url": view.url,
            "title": view.title,
            "page_type": view.page_hint,
            "elements_count": view.elements.len(),
            "estimated_tokens": view.estimated_tokens(),
            "elements": view.elements,
            "forms": view.forms,
            "visible_text": view.visible_text,
            "query": p.what,
        });

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }

    /// Get a structured semantic snapshot of the current page.
    /// Returns elements with IDs usable by lad_click/lad_type/lad_select.
    pub(crate) async fn tool_lad_snapshot(
        &self,
        params: Parameters<SnapshotParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(url = %p.url, "lad_snapshot");

        let view = self.navigate_or_reuse(&p.url).await?;
        Ok(CallToolResult::success(vec![Content::text(
            view.to_prompt(),
        )]))
    }

    /// Take a screenshot of the active page.
    pub(crate) async fn tool_lad_screenshot(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        tracing::info!("lad_screenshot");
        let guard = self.active_page.lock().await;
        let active = guard.as_ref().ok_or_else(no_active_page)?;
        let png = active.page.screenshot_png().await.map_err(mcp_err)?;
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png);
        Ok(CallToolResult::success(vec![Content::image(
            b64,
            "image/png",
        )]))
    }
}
