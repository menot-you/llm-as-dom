//! Interaction tools: `lad_click`, `lad_type`, `lad_select`, `lad_hover`,
//! `lad_press_key`, `lad_upload`.
//!
//! Uses `interact_and_refresh` helper to DRY the common pattern.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{build_element_js, check_js_result, key_to_code, mcp_err, no_active_page};
use crate::params::{
    ClickParams, HoverParams, PressKeyParams, ScrollParams, SelectParams, TypeParams, UploadParams,
};

use llm_as_dom::pilot;

/// FIX-7: Default delay (ms) after interaction before re-extracting the DOM.
/// 150ms gives SPAs enough time to react without feeling slow.
const DEFAULT_INTERACT_DELAY_MS: u64 = 150;

/// Shorter delay for simple value-setting (type/select) where no navigation occurs.
const VALUE_SET_DELAY_MS: u64 = 100;

impl LadServer {
    /// Common pattern: execute JS on active page, wait, refresh view, return prompt.
    ///
    /// FIX-R6-01: After the interaction delay, checks the current browser URL
    /// against SSRF rules before refreshing the view. This prevents click/type/
    /// select/keypress from silently navigating to `localhost` or private IPs
    /// via page-driven links or form submissions.
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

        // FIX-R6-01: SSRF gate — verify the browser hasn't navigated to an unsafe URL
        // as a result of the interaction (e.g. click on a link to localhost).
        // FIX-R8-01: Invalidate active_page on SSRF so subsequent tools can't
        // operate on the unsafe page.
        {
            let mut active = self.active_page.lock().await;
            let ap = active.as_ref().ok_or_else(no_active_page)?;
            let current_url = ap.page.url().await.map_err(mcp_err)?;
            if !llm_as_dom::sanitize::is_safe_url(&current_url) {
                *active = None;
                return Err(mcp_err(format!(
                    "blocked: interaction navigated to unsafe URL {current_url}"
                )));
            }
        }

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
        tracing::info!(
            element = p.element,
            wait_for_navigation = p.wait_for_navigation,
            "lad_click"
        );

        let js = build_element_js(p.element, "el.click();");

        if p.wait_for_navigation {
            {
                let mut active = self.active_page.lock().await;
                let ap = active.as_ref().ok_or_else(no_active_page)?;
                check_js_result(&ap.page.eval_js(&js).await.map_err(mcp_err)?)?;
                ap.page.wait_for_navigation().await.map_err(mcp_err)?;

                // FIX-R6-01: SSRF gate after navigation
                // FIX-R8-01: Invalidate active_page on SSRF detection.
                let current_url = ap.page.url().await.map_err(mcp_err)?;
                if !llm_as_dom::sanitize::is_safe_url(&current_url) {
                    *active = None;
                    return Err(mcp_err(format!(
                        "blocked: click navigated to unsafe URL {current_url}"
                    )));
                }
            }
            let view = self.refresh_active_view().await?;
            Ok(CallToolResult::success(vec![Content::text(
                view.to_prompt(),
            )]))
        } else {
            self.interact_and_refresh(&js, DEFAULT_INTERACT_DELAY_MS)
                .await
        }
    }

    /// Type text into an element by its ID from lad_snapshot.
    pub(crate) async fn tool_lad_type(
        &self,
        params: Parameters<TypeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        // FIX-12+13: Redact typed text if the target is a sensitive field.
        // Checks both input_type AND name for password/secret patterns.
        let log_text = {
            let active = self.active_page.lock().await;
            let is_sensitive = active.as_ref().is_some_and(|ap| {
                ap.view.elements.iter().any(|el| {
                    el.id == p.element
                        && (el
                            .input_type
                            .as_deref()
                            .is_some_and(|t| t.eq_ignore_ascii_case("password"))
                            || el.name.as_deref().is_some_and(|n| {
                                let lower = n.to_lowercase();
                                lower.contains("password")
                                    || lower.contains("passwd")
                                    || lower.contains("secret")
                            }))
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

        // DX-4: If press_enter=true, append Enter key events after typing.
        let enter_snippet = if p.press_enter {
            let code = key_to_code("Enter");
            let key_escaped = pilot::js_escape("Enter");
            let code_escaped = pilot::js_escape(code);
            format!(
                "\nfor (const type of ['keydown', 'keypress', 'keyup']) {{\
                     el.dispatchEvent(new KeyboardEvent(type, {{\
                         key: '{key_escaped}', code: '{code_escaped}', bubbles: true, cancelable: true\
                     }}));\
                 }}"
            )
        } else {
            String::new()
        };

        let body = format!(
            "el.focus();\n\
             el.value = '{escaped}';\n\
             el.dispatchEvent(new Event('input', {{ bubbles: true }}));\n\
             el.dispatchEvent(new Event('change', {{ bubbles: true }}));{enter_snippet}"
        );
        let js = build_element_js(p.element, &body);

        if p.press_enter {
            // FIX-R6-05: Form submission via Enter may trigger navigation.
            // Use wait_for_navigation with a timeout instead of a fixed sleep.
            {
                let mut active = self.active_page.lock().await;
                let ap = active.as_ref().ok_or_else(no_active_page)?;
                check_js_result(&ap.page.eval_js(&js).await.map_err(mcp_err)?)?;

                // Wait up to 5s for potential navigation; timeout is fine (page didn't navigate).
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    ap.page.wait_for_navigation(),
                )
                .await;

                // FIX-R6-01: SSRF gate after potential navigation
                // FIX-R8-01: Invalidate active_page on SSRF detection.
                let current_url = ap.page.url().await.map_err(mcp_err)?;
                if !llm_as_dom::sanitize::is_safe_url(&current_url) {
                    *active = None;
                    return Err(mcp_err(format!(
                        "blocked: form submission navigated to unsafe URL {current_url}"
                    )));
                }
            }
            let view = self.refresh_active_view().await?;
            Ok(CallToolResult::success(vec![Content::text(
                view.to_prompt(),
            )]))
        } else {
            self.interact_and_refresh(&js, VALUE_SET_DELAY_MS).await
        }
    }

    /// Select an option in a `<select>` element by its ID from lad_snapshot.
    pub(crate) async fn tool_lad_select(
        &self,
        params: Parameters<SelectParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(element = p.element, value = %p.value, wait_for_navigation = p.wait_for_navigation, "lad_select");

        let escaped = pilot::js_escape(&p.value);
        let body = format!(
            "if (el.tagName !== 'SELECT') return JSON.stringify({{ error: \"element {id} is not a <select>\" }});\n\
             el.value = '{escaped}';\n\
             el.dispatchEvent(new Event('change', {{ bubbles: true }}));",
            id = p.element,
        );
        let js = build_element_js(p.element, &body);

        if p.wait_for_navigation {
            {
                let mut active = self.active_page.lock().await;
                let ap = active.as_ref().ok_or_else(no_active_page)?;
                check_js_result(&ap.page.eval_js(&js).await.map_err(mcp_err)?)?;
                ap.page.wait_for_navigation().await.map_err(mcp_err)?;

                // FIX-R6-01: SSRF gate after navigation
                // FIX-R8-01: Invalidate active_page on SSRF detection.
                let current_url = ap.page.url().await.map_err(mcp_err)?;
                if !llm_as_dom::sanitize::is_safe_url(&current_url) {
                    *active = None;
                    return Err(mcp_err(format!(
                        "blocked: select navigated to unsafe URL {current_url}"
                    )));
                }
            }
            let view = self.refresh_active_view().await?;
            Ok(CallToolResult::success(vec![Content::text(
                view.to_prompt(),
            )]))
        } else {
            self.interact_and_refresh(&js, VALUE_SET_DELAY_MS).await
        }
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

        // FIX-R6-01: SSRF gate — key presses (e.g. Enter) can trigger navigation
        // FIX-R8-01: Invalidate active_page on SSRF detection.
        {
            let mut active = self.active_page.lock().await;
            let ap = active.as_ref().ok_or_else(no_active_page)?;
            let current_url = ap.page.url().await.map_err(mcp_err)?;
            if !llm_as_dom::sanitize::is_safe_url(&current_url) {
                *active = None;
                return Err(mcp_err(format!(
                    "blocked: key press navigated to unsafe URL {current_url}"
                )));
            }
        }

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
        // Prevents uploading /etc/passwd, SSH keys, etc. to attacker pages.
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

            // Use engine-level file upload (CDP on Chromium).
            //
            // FIX-R7-03: Known limitation — `set_input_files` uses flat CSS via
            // chromiumoxide's `find_element(selector)`, which does NOT pierce shadow
            // DOM or iframes (including same-origin). The precheck and change event
            // above use `build_element_js` (deepQuerySelector) so they DO find
            // elements in shadow roots, but the actual upload will fail silently
            // for those cases. A full fix would require resolving the element to a
            // CDP `backendNodeId` via JS evaluation, then calling
            // `DOM.setFileInputFiles` directly with that ID. This is deferred
            // because shadow-DOM/iframe file inputs are extremely rare in practice.
            // The tool description documents this limitation.
            ap.page
                .set_input_files(&selector, &p.files)
                .await
                .map_err(mcp_err)?;

            // FIX-R6-04: Use deepQuerySelector for change event dispatch so it
            // works with elements inside shadow DOM and iframes.
            let change_js = build_element_js(
                p.element,
                "el.dispatchEvent(new Event('change', { bubbles: true }));",
            );
            ap.page.eval_js(&change_js).await.map_err(mcp_err)?;
        }

        // FIX-R8-02: Route upload through refresh_active_view chokepoint.
        // A malicious `change` handler could navigate to an internal target;
        // refresh_active_view includes the SSRF gate and will invalidate
        // active_page if the URL is unsafe.
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
