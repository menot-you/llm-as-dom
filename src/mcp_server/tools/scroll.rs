//! Scroll tool: `lad_scroll`.
//!
//! SS-4: Extracted from interact.rs to keep each file under 300 LOC.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{build_element_js, check_js_result, mcp_err, no_active_page};
use crate::params::ScrollParams;

impl LadServer {
    /// Scroll the page or scroll to a specific element.
    ///
    /// DX-5: Dedicated scroll tool so agents don't need `lad_eval` for scrolling.
    /// After scrolling, waits 200ms for lazy-loaded content, then returns updated view.
    pub(crate) async fn tool_lad_scroll(
        &self,
        params: Parameters<ScrollParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(direction = %p.direction, element = ?p.element, pixels = p.pixels, "lad_scroll");

        let js = if let Some(el_id) = p.element {
            // FIX-R6-04: Use deepQuerySelector to find elements in shadow DOM/iframes.
            build_element_js(
                el_id,
                "el.scrollIntoView({ behavior: 'smooth', block: 'center' });",
            )
        } else {
            // Directional scroll
            let scroll_cmd = match p.direction.as_str() {
                "up" => format!("window.scrollBy(0, -{})", p.pixels),
                "bottom" => "window.scrollTo(0, document.body.scrollHeight)".to_string(),
                "top" => "window.scrollTo(0, 0)".to_string(),
                // "down" is the default
                _ => format!("window.scrollBy(0, {})", p.pixels),
            };
            format!(
                r#"(() => {{
                    {scroll_cmd};
                    return JSON.stringify({{ ok: true }});
                }})()"#
            )
        };

        {
            let active = self.active_page.lock().await;
            let ap = active.as_ref().ok_or_else(no_active_page)?;
            check_js_result(&ap.page.eval_js(&js).await.map_err(mcp_err)?)?;
        }

        // Wait for lazy-loaded content to settle
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let view = self.refresh_active_view().await?;
        Ok(CallToolResult::success(vec![Content::text(
            view.to_prompt(),
        )]))
    }
}
