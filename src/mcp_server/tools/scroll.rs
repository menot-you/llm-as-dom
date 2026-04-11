//! Scroll tool: `lad_scroll`.
//!
//! SS-4: Extracted from interact.rs to keep each file under 300 LOC.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{build_element_js, check_js_result, mcp_err};
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
        tracing::info!(
            direction = %p.direction,
            element = ?p.element,
            pixels = p.pixels,
            tab_id = ?p.tab_id,
            "lad_scroll"
        );

        let js = if let Some(el_id) = p.element {
            // FIX-R6-04: Use deepQuerySelector to find elements in shadow DOM/iframes.
            build_element_js(
                el_id,
                "el.scrollIntoView({ behavior: 'smooth', block: 'center' });",
            )
        } else {
            // DX-FIX: Detect active dialog/modal and scroll it instead of the page.
            // Falls back to window.scrollBy if no modal is active.
            let scroll_cmd = match p.direction.as_str() {
                "up" => format!("pixels = -{}", p.pixels),
                "bottom" => "pixels = 999999".to_string(),
                "top" => "pixels = -999999".to_string(),
                _ => format!("pixels = {}", p.pixels),
            };
            format!(
                r#"(() => {{
                    let {scroll_cmd};
                    const dialog = document.querySelector(
                        'dialog[open], [role="dialog"][aria-modal="true"], [role="dialog"]:not([aria-hidden="true"])'
                    );
                    if (dialog) {{
                        // Find scrollable container inside the dialog.
                        let scrollTarget = dialog;
                        for (const child of dialog.querySelectorAll('*')) {{
                            if (child.scrollHeight > child.clientHeight + 10) {{
                                scrollTarget = child;
                                break;
                            }}
                        }}
                        scrollTarget.scrollBy(0, pixels);
                    }} else {{
                        window.scrollBy(0, pixels);
                    }}
                    return JSON.stringify({{ ok: true }});
                }})()"#
            )
        };

        {
            let guard = self.lock_active_page().await;
            let ap = guard.resolve(p.tab_id)?;
            check_js_result(&ap.page.eval_js(&js).await.map_err(mcp_err)?)?;
        }

        // Wait for lazy-loaded content to settle
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let view = self.refresh_view_for(p.tab_id).await?;
        Ok(CallToolResult::success(vec![Content::text(
            view.to_prompt(),
        )]))
    }
}
