//! Upload tool: `lad_upload`.
//!
//! SS-4: Extracted from interact.rs to keep each file under 300 LOC.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{build_element_js, check_js_result, mcp_err, no_active_page};
use crate::params::UploadParams;

/// FIX-7: Default delay (ms) after interaction before re-extracting the DOM.
const DEFAULT_INTERACT_DELAY_MS: u64 = 150;

impl LadServer {
    /// Upload file(s) to a file input element.
    pub(crate) async fn tool_lad_upload(
        &self,
        params: Parameters<UploadParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        // FIX-12: Log only filenames, not full paths (may contain user info).
        let file_names: Vec<&str> = p
            .files
            .iter()
            .map(|f| {
                std::path::Path::new(f)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("[invalid]")
            })
            .collect();
        tracing::info!(element = p.element, files = ?file_names, "lad_upload");

        if p.files.is_empty() {
            return Err(rmcp::ErrorData::invalid_params(
                "files array must not be empty",
                None,
            ));
        }

        // FIX-4: Validate all file paths are absolute AND within allowed roots.
        for path in &p.files {
            let file_path = std::path::Path::new(path);
            if !file_path.is_absolute() {
                return Err(rmcp::ErrorData::invalid_params(
                    format!("file path must be absolute: {path}"),
                    None,
                ));
            }
            if !file_path.exists() {
                return Err(rmcp::ErrorData::invalid_params(
                    format!("file not found: {path}"),
                    None,
                ));
            }
            if !llm_as_dom::sanitize::is_safe_upload_path(file_path) {
                return Err(rmcp::ErrorData::invalid_params(
                    format!(
                        "upload blocked: path '{}' is outside allowed roots (cwd, /tmp). \
                         Set LAD_UPLOAD_ROOT to allow custom directories.",
                        path
                    ),
                    None,
                ));
            }
        }

        let file_count = p.files.len();
        let element_id = p.element;
        let selector = format!(r#"[data-lad-id="{}"]"#, p.element);

        {
            let active = self.active_page.lock().await;
            let ap = active.as_ref().ok_or_else(no_active_page)?;

            // Verify element exists and is a file input
            let check_body = format!(
                "if (el.tagName !== 'INPUT' || el.type !== 'file')\n\
                     return JSON.stringify({{ error: \"element {id} is not a file input\" }});",
                id = p.element,
            );
            let check_js = build_element_js(p.element, &check_body);
            check_js_result(&ap.page.eval_js(&check_js).await.map_err(mcp_err)?)?;

            ap.page
                .set_input_files(&selector, &p.files)
                .await
                .map_err(mcp_err)?;

            // FIX-R6-04: Use deepQuerySelector for change event dispatch.
            let change_js = build_element_js(
                p.element,
                "el.dispatchEvent(new Event('change', { bubbles: true }));",
            );
            ap.page.eval_js(&change_js).await.map_err(mcp_err)?;
        }

        // FIX-R8-02: Route upload through refresh_active_view chokepoint.
        tokio::time::sleep(std::time::Duration::from_millis(DEFAULT_INTERACT_DELAY_MS)).await;
        let view = self.refresh_active_view().await?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "{}\n\n--- Updated View ---\n{}",
            serde_json::json!({
                "status": "uploaded",
                "files": file_count,
                "element": element_id,
            }),
            view.to_prompt(),
        ))]))
    }
}
