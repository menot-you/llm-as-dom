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
    ///
    /// FIX-18: The `what` parameter now filters elements by relevance.
    /// Elements whose label, name, placeholder, or href contain any word
    /// from `what` are promoted to the front; all elements are still
    /// returned but `relevant_count` tells the caller how many matched.
    ///
    /// DX-W2-1: `url` is now optional. When omitted, extracts from the
    /// current active page without navigating — preserving session state.
    pub(crate) async fn tool_lad_extract(
        &self,
        params: Parameters<ExtractParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        let mut view = if let Some(ref url) = p.url {
            let (_page, view) = self.navigate_and_extract(url).await?;
            view
        } else {
            self.refresh_active_view().await.map_err(|_| {
                rmcp::ErrorData::invalid_params(
                    "no active page — provide a URL or call lad_browse/lad_snapshot first"
                        .to_string(),
                    None,
                )
            })?
        };

        if let Some(max_len) = p.max_length
            && view.visible_text.len() > max_len
        {
            let mut end = max_len;
            while !view.visible_text.is_char_boundary(end) && end > 0 {
                end -= 1;
            }
            view.visible_text.truncate(end);
        }

        // FIX-18: Score elements by relevance to `what` and sort.
        let what_lower = p.what.to_lowercase();
        let what_words: Vec<&str> = what_lower.split_whitespace().collect();

        let relevance_score = |el: &llm_as_dom::semantic::Element| -> u32 {
            if what_words.is_empty() {
                return 0;
            }
            let fields = [
                el.label.to_lowercase(),
                el.name.as_deref().unwrap_or("").to_lowercase(),
                el.placeholder.as_deref().unwrap_or("").to_lowercase(),
                el.href.as_deref().unwrap_or("").to_lowercase(),
            ];
            let mut score = 0u32;
            for word in &what_words {
                for field in &fields {
                    if field.contains(word) {
                        score += 1;
                    }
                }
            }
            score
        };

        // Sort relevant elements first (stable sort preserves DOM order within same score).
        view.elements
            .sort_by_key(|el| std::cmp::Reverse(relevance_score(el)));
        let relevant_count = view
            .elements
            .iter()
            .filter(|el| relevance_score(el) > 0)
            .count();

        // DX-W3-6: Support format="prompt" for compact text output.
        let use_prompt = p
            .format
            .as_deref()
            .is_some_and(|f| f.eq_ignore_ascii_case("prompt"));

        if use_prompt {
            Ok(CallToolResult::success(vec![Content::text(
                view.to_prompt(),
            )]))
        } else {
            let output = serde_json::json!({
                "url": llm_as_dom::sanitize::redact_url_secrets(&view.url),
                "title": view.title,
                "page_type": view.page_hint,
                "elements_count": view.elements.len(),
                "relevant_count": relevant_count,
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
    }

    /// Get a structured semantic snapshot of the current page.
    /// Returns elements with IDs usable by lad_click/lad_type/lad_select.
    ///
    /// DX-1: `url` is now optional. When omitted, re-extracts the current active
    /// page without navigating — preventing the footgun where agents accidentally
    /// undo a click by re-navigating to the old URL.
    pub(crate) async fn tool_lad_snapshot(
        &self,
        params: Parameters<SnapshotParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;

        // Ensure engine matches requested visibility before navigating.
        self.ensure_engine_visible(p.visible)
            .await
            .map_err(mcp_err)?;

        if let Some(ref url) = p.url {
            tracing::info!(url = %url, "lad_snapshot (with url)");
            let view = self.navigate_or_reuse(url).await?;
            Ok(CallToolResult::success(vec![Content::text(
                view.to_prompt(),
            )]))
        } else {
            tracing::info!("lad_snapshot (current page)");
            let view = self.refresh_active_view().await.map_err(|_| {
                rmcp::ErrorData::invalid_params(
                    "no active page — provide a URL or call lad_browse first".to_string(),
                    None,
                )
            })?;
            Ok(CallToolResult::success(vec![Content::text(
                view.to_prompt(),
            )]))
        }
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
