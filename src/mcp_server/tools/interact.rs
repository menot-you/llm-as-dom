//! Interaction tools: `lad_click`, `lad_type`, `lad_select`, `lad_hover`,
//! `lad_press_key`, `lad_upload`.
//!
//! Uses `interact_and_refresh` helper to DRY the common pattern.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{build_element_js, check_js_result, key_to_code, mcp_err, no_active_page};
use crate::params::{
    ClickParams, HoverParams, PressKeyParams, SelectParams, TypeParams, UploadParams,
};

use llm_as_dom::pilot;

/// FIX-7: Default delay (ms) after interaction before re-extracting the DOM.
/// 150ms gives SPAs enough time to react without feeling slow.
const DEFAULT_INTERACT_DELAY_MS: u64 = 150;

/// Shorter delay for simple value-setting (type/select) where no navigation occurs.
const VALUE_SET_DELAY_MS: u64 = 100;

impl LadServer {
    /// Common pattern: execute JS on active page, wait, refresh view, return prompt.
    async fn interact_and_refresh(
        &self,
        js: &str,
        delay_ms: u64,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        {
            let active = self.active_page.lock().await;
            let ap = active.as_ref().ok_or_else(no_active_page)?;
            check_js_result(&ap.page.eval_js(js).await.map_err(mcp_err)?)?;
        }

        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        let view = self.refresh_active_view().await?;
        Ok(CallToolResult::success(vec![Content::text(
            view.to_prompt(),
        )]))
    }

    /// Click an element by its ID from lad_snapshot.
    pub(crate) async fn tool_lad_click(
        &self,
        params: Parameters<ClickParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(element = p.element, "lad_click");

        let js = build_element_js(p.element, "el.click();");
        self.interact_and_refresh(&js, DEFAULT_INTERACT_DELAY_MS)
            .await
    }

    /// Type text into an element by its ID from lad_snapshot.
    pub(crate) async fn tool_lad_type(
        &self,
        params: Parameters<TypeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        // FIX-13: Redact typed text if the target is a sensitive field.
        let log_text = {
            let active = self.active_page.lock().await;
            let is_sensitive = active.as_ref().is_some_and(|ap| {
                ap.view.elements.iter().any(|el| {
                    el.id == p.element
                        && el
                            .input_type
                            .as_deref()
                            .is_some_and(|t| t.eq_ignore_ascii_case("password"))
                })
            });
            if is_sensitive {
                "[REDACTED]".to_string()
            } else {
                p.text.clone()
            }
        };
        tracing::info!(element = p.element, text = %log_text, "lad_type");

        let escaped = pilot::js_escape(&p.text);
        let body = format!(
            "el.focus();\n\
             el.value = '{escaped}';\n\
             el.dispatchEvent(new Event('input', {{ bubbles: true }}));\n\
             el.dispatchEvent(new Event('change', {{ bubbles: true }}));"
        );
        let js = build_element_js(p.element, &body);
        self.interact_and_refresh(&js, VALUE_SET_DELAY_MS).await
    }

    /// Select an option in a `<select>` element by its ID from lad_snapshot.
    pub(crate) async fn tool_lad_select(
        &self,
        params: Parameters<SelectParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(element = p.element, value = %p.value, "lad_select");

        let escaped = pilot::js_escape(&p.value);
        let body = format!(
            "if (el.tagName !== 'SELECT') return JSON.stringify({{ error: \"element {id} is not a <select>\" }});\n\
             el.value = '{escaped}';\n\
             el.dispatchEvent(new Event('change', {{ bubbles: true }}));",
            id = p.element,
        );
        let js = build_element_js(p.element, &body);
        self.interact_and_refresh(&js, VALUE_SET_DELAY_MS).await
    }

    /// Hover over an element by its ID from lad_snapshot.
    pub(crate) async fn tool_lad_hover(
        &self,
        params: Parameters<HoverParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(element = p.element, "lad_hover");

        let body = "\
            for (const type of ['mouseenter', 'mouseover', 'mousemove']) {\
                el.dispatchEvent(new MouseEvent(type, {\
                    bubbles: true, cancelable: true, view: window\
                }));\
            }";
        let js = build_element_js(p.element, body);
        // Hover needs slightly longer for CSS transitions / dropdown menus.
        self.interact_and_refresh(&js, DEFAULT_INTERACT_DELAY_MS + 50)
            .await
    }

    /// Press a keyboard key on the active page.
    /// Optionally focus an element first by its ID from a prior snapshot.
    pub(crate) async fn tool_lad_press_key(
        &self,
        params: Parameters<PressKeyParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(key = %p.key, element = ?p.element, "lad_press_key");

        {
            let active = self.active_page.lock().await;
            let ap = active.as_ref().ok_or_else(no_active_page)?;

            // If element specified, focus it first
            if let Some(id) = p.element {
                let focus_js = build_element_js(id, "el.focus();");
                check_js_result(&ap.page.eval_js(&focus_js).await.map_err(mcp_err)?)?;
            }

            // Dispatch keyboard event sequence: keydown, keypress, keyup
            let code = key_to_code(&p.key);
            let key_escaped = pilot::js_escape(&p.key);
            let code_escaped = pilot::js_escape(code);
            let js = format!(
                r#"(() => {{
                    const target = document.activeElement || document.body;
                    for (const type of ['keydown', 'keypress', 'keyup']) {{
                        target.dispatchEvent(new KeyboardEvent(type, {{
                            key: '{key_escaped}', code: '{code_escaped}', bubbles: true, cancelable: true
                        }}));
                    }}
                }})()"#,
            );
            ap.page.eval_js(&js).await.map_err(mcp_err)?;
        }

        tokio::time::sleep(std::time::Duration::from_millis(DEFAULT_INTERACT_DELAY_MS)).await;
        let view = self.refresh_active_view().await?;
        Ok(CallToolResult::success(vec![Content::text(
            view.to_prompt(),
        )]))
    }

    /// Upload file(s) to a file input element.
    pub(crate) async fn tool_lad_upload(
        &self,
        params: Parameters<UploadParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(element = p.element, files = ?p.files, "lad_upload");

        if p.files.is_empty() {
            return Err(rmcp::ErrorData::invalid_params(
                "files array must not be empty",
                None,
            ));
        }

        // FIX-14: Validate all file paths are absolute (reject relative paths
        // that could be used for path traversal).
        for path in &p.files {
            let p = std::path::Path::new(path);
            if !p.is_absolute() {
                return Err(rmcp::ErrorData::invalid_params(
                    format!("file path must be absolute: {path}"),
                    None,
                ));
            }
            if !p.exists() {
                return Err(rmcp::ErrorData::invalid_params(
                    format!("file not found: {path}"),
                    None,
                ));
            }
        }

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

            // Use engine-level file upload (CDP on Chromium)
            ap.page
                .set_input_files(&selector, &p.files)
                .await
                .map_err(mcp_err)?;

            // Dispatch change event so frameworks react
            let change_js = format!(
                r#"document.querySelector('[data-lad-id="{}"]')
                    .dispatchEvent(new Event('change', {{ bubbles: true }}))"#,
                p.element
            );
            ap.page.eval_js(&change_js).await.map_err(mcp_err)?;
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            r#"{{"status": "uploaded", "files": {}, "element": {}}}"#,
            p.files.len(),
            p.element
        ))]))
    }
}
