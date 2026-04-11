//! Interaction tools: `lad_click`, `lad_type`, `lad_select`, `lad_hover`,
//! `lad_press_key`, `lad_upload`.
//!
//! Uses `interact_and_refresh` helper to DRY the common pattern.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{
    build_element_js, build_element_js_or_target, check_js_result, key_to_code, mcp_err,
    no_active_page,
};
use crate::params::{ClickParams, HoverParams, PressKeyParams, SelectParams, TypeParams};

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
    pub(crate) async fn interact_and_refresh(
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
            element = ?p.element,
            target = ?p.target,
            wait_for_navigation = p.wait_for_navigation,
            "lad_click"
        );

        // DX-8 FIX: Use full pointer event sequence for React/Twitter compatibility.
        // el.click() doesn't trigger React's synthetic event system in some cases.
        let js = build_element_js_or_target(
            p.element,
            p.target.as_ref(),
            r#"el.scrollIntoView({ block: 'center' });
            el.focus();
            el.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, cancelable: true }));
            el.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true }));
            el.dispatchEvent(new PointerEvent('pointerup', { bubbles: true, cancelable: true }));
            el.dispatchEvent(new MouseEvent('mouseup', { bubbles: true, cancelable: true }));
            el.click();"#,
        )?;

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
        // Only possible when the caller used a numeric element ID from
        // the last snapshot (we have element metadata in ap.view). When
        // they used a semantic target spec, we can't pre-inspect — a best-
        // effort name-based heuristic on the spec itself is used instead.
        let log_text = {
            let active = self.active_page.lock().await;
            let is_sensitive_by_element = p.element.is_some()
                && active.as_ref().is_some_and(|ap| {
                    let want_id = p.element.unwrap();
                    ap.view.elements.iter().any(|el| {
                        el.id == want_id
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
            // Best-effort for target spec: redact when the spec's role/label
            // hints at a password field.
            let is_sensitive_by_target = p.target.as_ref().is_some_and(|spec| {
                let check = |s: &str| {
                    let lower = s.to_lowercase();
                    lower.contains("password")
                        || lower.contains("passwd")
                        || lower.contains("secret")
                };
                spec.label.as_deref().is_some_and(check)
                    || spec.text.as_deref().is_some_and(check)
                    || spec.testid.as_deref().is_some_and(check)
            });
            if is_sensitive_by_element || is_sensitive_by_target {
                "[REDACTED]".to_string()
            } else {
                p.text.clone()
            }
        };
        tracing::info!(element = ?p.element, target = ?p.target, text = %log_text, "lad_type");

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

        // DX-12 + DX-CE3: Dual path for typing.
        //
        // Plain <input>/<textarea>:
        //   Use the native HTMLInputElement/HTMLTextAreaElement value setter
        //   so React's synthetic event system fires correctly (React
        //   overrides .value on its managed instances).
        //
        // contenteditable / [role="textbox"] / [aria-multiline="true"]:
        //   Twitter's Draft.js, Discord/Slack's Lexical, Notion's ProseMirror
        //   and similar rich-text editors listen for `beforeinput` and the
        //   synthetic `input` event with `inputType: 'insertText'` — they do
        //   NOT react to `.value = x`. We clear the editor via a Range, then
        //   use `document.execCommand('insertText', false, text)` which
        //   fires the expected event sequence. execCommand is deprecated
        //   but is still the single highest-fidelity path for these editors.
        let body = format!(
            "const isEditor = el.isContentEditable\n\
                 || el.getAttribute('contenteditable') === 'true'\n\
                 || el.getAttribute('contenteditable') === ''\n\
                 || el.getAttribute('role') === 'textbox'\n\
                 || el.getAttribute('aria-multiline') === 'true';\n\
             el.focus();\n\
             if (isEditor) {{\n\
                 // Move caret to end + select all existing content so the\n\
                 // insertText command replaces, not appends.\n\
                 try {{\n\
                     const range = document.createRange();\n\
                     range.selectNodeContents(el);\n\
                     const sel = window.getSelection();\n\
                     sel.removeAllRanges();\n\
                     sel.addRange(range);\n\
                 }} catch (_) {{}}\n\
                 // DX-03 FIX: Split on \\n and alternate insertText +\n\
                 // insertLineBreak. execCommand('insertText') is a PLAIN\n\
                 // text insert that does NOT honor embedded newlines in\n\
                 // rich-text editors (Draft.js/Lexical/ProseMirror silently\n\
                 // drop lines after the first \\n). We iterate line by line\n\
                 // so every newline becomes a proper line-break Draft block.\n\
                 const fullText = '{escaped}';\n\
                 const lines = fullText.split('\\n');\n\
                 let anyOk = true;\n\
                 for (let i = 0; i < lines.length; i++) {{\n\
                     if (lines[i].length > 0) {{\n\
                         try {{\n\
                             if (!document.execCommand('insertText', false, lines[i])) {{\n\
                                 anyOk = false;\n\
                             }}\n\
                         }} catch (_) {{ anyOk = false; }}\n\
                     }}\n\
                     if (i < lines.length - 1) {{\n\
                         // Prefer insertLineBreak; fall back to insertParagraph\n\
                         // (Draft.js treats these as different block types).\n\
                         let lb = false;\n\
                         try {{ lb = document.execCommand('insertLineBreak'); }} catch (_) {{}}\n\
                         if (!lb) {{\n\
                             try {{ document.execCommand('insertParagraph'); }} catch (_) {{}}\n\
                         }}\n\
                     }}\n\
                 }}\n\
                 if (!anyOk) {{\n\
                     // Last-resort fallback: set textContent directly and\n\
                     // fire an input event. Breaks React controlled state but\n\
                     // at least captures the text.\n\
                     el.textContent = fullText;\n\
                     el.dispatchEvent(new InputEvent('input', {{\n\
                         bubbles: true, cancelable: true,\n\
                         data: fullText, inputType: 'insertText'\n\
                     }}));\n\
                 }}\n\
             }} else {{\n\
                 const nativeSetter = Object.getOwnPropertyDescriptor(\n\
                     el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype,\n\
                     'value'\n\
                 )?.set;\n\
                 if (nativeSetter) {{ nativeSetter.call(el, '{escaped}'); }}\n\
                 else {{ el.value = '{escaped}'; }}\n\
                 el.dispatchEvent(new Event('input', {{ bubbles: true }}));\n\
                 el.dispatchEvent(new Event('change', {{ bubbles: true }}));\n\
             }}{enter_snippet}"
        );
        let js = build_element_js_or_target(p.element, p.target.as_ref(), &body)?;

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
        tracing::info!(element = ?p.element, target = ?p.target, value = %p.value, wait_for_navigation = p.wait_for_navigation, "lad_select");

        // DX-W2-5: Match by visible label text first, then fall back to value.
        let escaped = pilot::js_escape(&p.value);
        let body = format!(
            "if (el.tagName !== 'SELECT') return JSON.stringify({{ error: \"element is not a <select>\" }});\n\
             const options = Array.from(el.options);\n\
             let opt = options.find(o => o.textContent.trim().toLowerCase() === '{escaped}'.toLowerCase());\n\
             if (!opt) opt = options.find(o => o.value === '{escaped}');\n\
             if (!opt) return JSON.stringify({{ error: \"no option matching '{escaped}' in select\" }});\n\
             el.value = opt.value;\n\
             el.dispatchEvent(new Event('change', {{ bubbles: true }}));",
        );
        let js = build_element_js_or_target(p.element, p.target.as_ref(), &body)?;

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
        tracing::info!(element = ?p.element, target = ?p.target, "lad_hover");

        let body = "\
            for (const type of ['mouseenter', 'mouseover', 'mousemove']) {\
                el.dispatchEvent(new MouseEvent(type, {\
                    bubbles: true, cancelable: true, view: window\
                }));\
            }";
        let js = build_element_js_or_target(p.element, p.target.as_ref(), body)?;
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
}
