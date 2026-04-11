//! Form interaction tools: `lad_fill_form`, `lad_clear`.
//!
//! SS-4: Extracted from interact.rs to keep each file under 300 LOC.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{build_element_js, check_js_result, mcp_err};
use crate::params::{ClearParams, FillFormParams};

use llm_as_dom::pilot;

/// Shorter delay for simple value-setting where no navigation occurs.
const VALUE_SET_DELAY_MS: u64 = 100;

impl LadServer {
    /// Clear an input field by selecting all content and deleting.
    ///
    /// DX-W3-3: Works with React/Vue controlled components that don't respond
    /// to `el.value = ''`. Uses select-all + delete + input event dispatch.
    pub(crate) async fn tool_lad_clear(
        &self,
        params: Parameters<ClearParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(element = ?p.element, target = ?p.target, tab_id = ?p.tab_id, "lad_clear");

        let body = "\
            el.focus();\n\
            el.select();\n\
            document.execCommand('delete');\n\
            el.dispatchEvent(new Event('input', { bubbles: true }));\n\
            el.dispatchEvent(new Event('change', { bubbles: true }));";
        let js = crate::helpers::build_element_js_or_target(p.element, p.target.as_ref(), body)?;
        self.interact_and_refresh(&js, VALUE_SET_DELAY_MS, p.tab_id)
            .await
    }

    /// Fill multiple form fields at once and optionally submit.
    ///
    /// DX-W2-3: Batch form-fill reduces 3+ tool calls (type email, type password,
    /// click submit) down to 1. Fields are matched by label, name, or placeholder
    /// (case-insensitive) against the current semantic view.
    pub(crate) async fn tool_lad_fill_form(
        &self,
        params: Parameters<FillFormParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(
            fields = p.fields.len(),
            submit = p.submit,
            form_index = ?p.form_index,
            tab_id = ?p.tab_id,
            "lad_fill_form"
        );

        if p.fields.is_empty() {
            return Err(rmcp::ErrorData::invalid_params(
                "fields must not be empty",
                None,
            ));
        }

        // Snapshot current view to find matching elements.
        let view = {
            let guard = self.lock_active_page().await;
            guard.resolve(p.tab_id)?.view.clone()
        };

        // Build JS to fill each field in one eval call.
        let mut fill_js = String::new();
        let mut matched = 0u32;
        let mut submitted = false;

        for (field_key, field_value) in &p.fields {
            let key_lower = field_key.to_lowercase();
            let escaped_val = pilot::js_escape(field_value);

            // Find matching element in the semantic view.
            let matched_el = view.elements.iter().find(|el| {
                // Scope to form_index if specified.
                if let Some(fi) = p.form_index
                    && el.form_index != Some(fi)
                {
                    return false;
                }
                // Match by label, name, or placeholder (case-insensitive).
                let label_match = el.label.to_lowercase().contains(&key_lower);
                let name_match = el
                    .name
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().contains(&key_lower));
                let ph_match = el
                    .placeholder
                    .as_deref()
                    .is_some_and(|p| p.to_lowercase().contains(&key_lower));
                label_match || name_match || ph_match
            });

            if let Some(el) = matched_el {
                // DX-12 FIX: React-compatible native setter (same as lad_type).
                let body = format!(
                    "el.focus();\n\
                     const nativeSetter = Object.getOwnPropertyDescriptor(\n\
                         el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype,\n\
                         'value'\n\
                     )?.set;\n\
                     if (nativeSetter) {{ nativeSetter.call(el, '{escaped_val}'); }}\n\
                     else {{ el.value = '{escaped_val}'; }}\n\
                     el.dispatchEvent(new Event('input', {{ bubbles: true }}));\n\
                     el.dispatchEvent(new Event('change', {{ bubbles: true }}));"
                );
                fill_js.push_str(&build_element_js(el.id, &body));
                fill_js.push_str(";\n");
                matched += 1;
            } else {
                tracing::warn!(field = %field_key, "lad_fill_form: no matching element");
            }
        }

        if matched == 0 {
            return Err(rmcp::ErrorData::invalid_params(
                format!(
                    "no form elements matched any field key: {:?}",
                    p.fields.keys().collect::<Vec<_>>()
                ),
                None,
            ));
        }

        // Execute all field fills.
        {
            let guard = self.lock_active_page().await;
            let ap = guard.resolve(p.tab_id)?;
            ap.page.eval_js(&fill_js).await.map_err(mcp_err)?;
        }

        tokio::time::sleep(std::time::Duration::from_millis(VALUE_SET_DELAY_MS)).await;

        // Submit if requested.
        if p.submit {
            let submit_el = view.elements.iter().find(|el| {
                if let Some(fi) = p.form_index
                    && el.form_index != Some(fi)
                {
                    return false;
                }
                if el.kind != llm_as_dom::semantic::ElementKind::Button {
                    return false;
                }
                let label_lower = el.label.to_lowercase();
                let is_submit_type = el
                    .input_type
                    .as_deref()
                    .is_some_and(|t| t.eq_ignore_ascii_case("submit"));
                is_submit_type
                    || label_lower.contains("submit")
                    || label_lower.contains("login")
                    || label_lower.contains("log in")
                    || label_lower.contains("sign in")
                    || label_lower.contains("sign up")
                    || label_lower.contains("register")
                    || label_lower.contains("continue")
                    || label_lower.contains("send")
                    || label_lower.contains("save")
            });

            if let Some(btn) = submit_el {
                // DX-8 FIX: Full pointer event sequence for React compatibility.
                let click_js = build_element_js(
                    btn.id,
                    r#"el.scrollIntoView({ block: 'center' });
                    el.focus();
                    el.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, cancelable: true }));
                    el.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true }));
                    el.dispatchEvent(new PointerEvent('pointerup', { bubbles: true, cancelable: true }));
                    el.dispatchEvent(new MouseEvent('mouseup', { bubbles: true, cancelable: true }));
                    el.click();"#,
                );
                {
                    let mut guard = self.lock_active_page().await;
                    let ap = guard.resolve(p.tab_id)?;
                    check_js_result(&ap.page.eval_js(&click_js).await.map_err(mcp_err)?)?;

                    // Wait for potential navigation after submit.
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        ap.page.wait_for_navigation(),
                    )
                    .await;

                    // SSRF gate after navigation.
                    let current_url = ap.page.url().await.map_err(mcp_err)?;
                    if !llm_as_dom::sanitize::is_safe_url(&current_url) {
                        Self::invalidate_tab_on_ssrf(&mut guard, p.tab_id);
                        return Err(mcp_err(format!(
                            "blocked: form submission navigated to unsafe URL {current_url}"
                        )));
                    }
                }
                submitted = true;
            } else {
                tracing::warn!("lad_fill_form: submit=true but no submit button found");
            }
        }

        let view = self.refresh_view_for(p.tab_id).await?;
        let total = p.fields.len();
        let submit_msg = if p.submit {
            if submitted {
                " + submitted"
            } else {
                " (submit requested but no button found)"
            }
        } else {
            ""
        };
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Filled {matched}/{total} fields{submit_msg}\n\n{}",
            view.to_prompt(),
        ))]))
    }
}
