//! `lad_extract`, `lad_snapshot`, `lad_screenshot`, `lad_jq` tools.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{mcp_err, to_pretty_json};
use crate::params::{ExtractParams, JqParams, SnapshotParams};

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
        let include_hidden = p.include_hidden.unwrap_or(false);
        let mut view = if let Some(ref url) = p.url {
            let (_page, view) = self.navigate_and_extract(url).await?;
            view
        } else {
            // Wave 2: route through the tab-aware refresh so `tab_id` opt-in
            // works uniformly across extract/snapshot/assert/wait.
            self.refresh_view_for(p.tab_id).await.map_err(|_| {
                rmcp::ErrorData::invalid_params(
                    "no active page — provide a URL or call lad_browse/lad_snapshot first"
                        .to_string(),
                    None,
                )
            })?
        };

        // Wave 5 (Pain #10): when the caller asks for hidden elements, the
        // default JS walker has already dropped them at Layer 1. Re-extract
        // with the flag lifted so hidden nodes flow through. Only done when
        // the flag is explicitly true to avoid an extra roundtrip on the
        // default path and to keep the signature of
        // `navigate_or_reuse`/`refresh_active_view` unchanged.
        if include_hidden {
            let guard = self.lock_active_page().await;
            let ap = guard.resolve(p.tab_id)?;
            view = llm_as_dom::a11y::extract_semantic_view_with_options(ap.page.as_ref(), true)
                .await
                .map_err(mcp_err)?;
        }

        // Wave 1 — hidden-element gate. Runs BEFORE scoring/pagination so
        // hidden nodes never contribute to the caller's element budget.
        if !include_hidden {
            view.retain_visible_elements();
        }

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

        // Wave 1 — pagination snapshot (used by both output branches).
        let total_elements = view.elements.len();
        let paginate = p.paginate_index;
        let page_size = p.page_size.max(1);

        if use_prompt {
            let text = match paginate {
                Some(page) => view.to_prompt_paginated(page, page_size),
                None => view.to_prompt(),
            };
            Ok(CallToolResult::success(vec![Content::text(text)]))
        } else {
            // JSON branch — slice elements if pagination requested.
            let (elements_slice, paginated_elements, page, total_pages) = match paginate {
                Some(page) => {
                    let size = page_size as usize;
                    let total_pages = if total_elements == 0 || size >= total_elements {
                        1
                    } else {
                        total_elements.div_ceil(size)
                    };
                    let clamped = (page as usize).min(total_pages.saturating_sub(1));
                    let start = clamped * size;
                    let end = (start + size).min(total_elements);
                    let slice = view.elements[start..end].to_vec();
                    let returned = slice.len();
                    (
                        slice,
                        Some(returned),
                        Some(clamped as u32),
                        Some(total_pages as u32),
                    )
                }
                None => (view.elements.clone(), None, None, None),
            };

            let output = serde_json::json!({
                "url": llm_as_dom::sanitize::redact_url_secrets(&view.url),
                "title": view.title,
                "page_type": view.page_hint,
                "elements_count": total_elements,
                "paginated_elements": paginated_elements,
                "page": page,
                "total_pages": total_pages,
                "relevant_count": relevant_count,
                "estimated_tokens": view.estimated_tokens(),
                "elements": elements_slice,
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
        let deadline = std::time::Duration::from_millis(p.timeout_ms);
        let include_hidden = p.include_hidden.unwrap_or(false);
        let paginate = p.paginate_index;
        let page_size = p.page_size.max(1);

        // Wrap the entire snapshot pipeline in a hard timeout so a hung
        // browser launch or a site that never stabilizes can't block the
        // MCP session indefinitely. Returns a timeout error to the caller.
        let work = async {
            self.ensure_engine_visible(p.visible)
                .await
                .map_err(mcp_err)?;

            let mut view = if let Some(ref url) = p.url {
                tracing::info!(url = %url, "lad_snapshot (with url)");
                self.navigate_or_reuse(url).await?
            } else {
                tracing::info!(tab_id = ?p.tab_id, "lad_snapshot (current page)");
                // DX-02b: propagate the real error from refresh_view_for
                // instead of swallowing it as "no active page". The previous
                // catch-all map_err was misleading when the failure was
                // actually a CDP error, SSRF block, or a11y extract failure.
                self.refresh_view_for(p.tab_id).await?
            };

            // Wave 5 (Pain #10): re-extract with include_hidden=true when
            // the caller wants hidden nodes. See `tool_lad_extract` for
            // rationale — avoids threading the flag through
            // `navigate_or_reuse` / `refresh_active_view`.
            if include_hidden {
                let guard = self.lock_active_page().await;
                let ap = guard.resolve(p.tab_id)?;
                view = llm_as_dom::a11y::extract_semantic_view_with_options(ap.page.as_ref(), true)
                    .await
                    .map_err(mcp_err)?;
            }

            // Wave 1 — hidden-element gate (default-on). See `retain_visible_elements`.
            if !include_hidden {
                view.retain_visible_elements();
            }

            // Wave 1 — pagination render.
            let text = match paginate {
                Some(page) => view.to_prompt_paginated(page, page_size),
                None => view.to_prompt(),
            };
            Ok::<CallToolResult, rmcp::ErrorData>(CallToolResult::success(vec![Content::text(
                text,
            )]))
        };

        match tokio::time::timeout(deadline, work).await {
            Ok(result) => result,
            Err(_) => Err(mcp_err(format!(
                "lad_snapshot timed out after {}ms — browser launch or page \
                 stabilization exceeded the deadline. Retry with a longer \
                 timeout_ms, or check browser engine state.",
                p.timeout_ms
            ))),
        }
    }

    /// Wave 1 — run a jq expression against the active page's `SemanticView`.
    ///
    /// The agent can ask for exactly the slice it needs (button labels,
    /// form field names, a count) instead of pulling the full snapshot into
    /// the prompt. On the average login page that's roughly a 10-30x token
    /// reduction compared with `lad_snapshot`.
    ///
    /// Returns the query results as pretty-printed JSON. If the filter
    /// yields a single value, the bare value is returned; if it yields
    /// multiple, they're wrapped in a JSON array (matches `jq` stream
    /// semantics). Errors from parse/compile/run are surfaced as invalid-params.
    pub(crate) async fn tool_lad_jq(
        &self,
        params: Parameters<JqParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        // Snapshot the current view under the guard and release immediately
        // so jq execution doesn't hold a mutex across CPU work.
        let view = {
            let guard = self.lock_active_page().await;
            guard.resolve(p.tab_id)?.view.clone()
        };

        let input = serde_json::to_value(&view).map_err(|e| {
            rmcp::ErrorData::internal_error(
                format!("failed to serialize semantic view for jq: {e}"),
                None,
            )
        })?;

        let results = run_jq(&p.query, input).map_err(|e| {
            rmcp::ErrorData::invalid_params(
                format!("jq query {query:?} failed: {e}", query = p.query),
                None,
            )
        })?;

        let output = match results.len() {
            0 => serde_json::Value::Null,
            1 => results
                .into_iter()
                .next()
                .unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Array(results),
        };

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }

    /// Take a screenshot of the active page (or an explicit tab).
    ///
    /// Wave 2: `lad_screenshot` takes no top-level tool params today, so the
    /// rmcp wrapper invokes this with `tab_id = None` and we always shoot the
    /// active tab. Switching to an explicit tab requires calling
    /// `lad_tabs_switch` first.
    pub(crate) async fn tool_lad_screenshot(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        tracing::info!("lad_screenshot");
        let guard = self.lock_active_page().await;
        let active = guard.resolve(None)?;
        let png = active.page.screenshot_png().await.map_err(mcp_err)?;
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png);
        Ok(CallToolResult::success(vec![Content::image(
            b64,
            "image/png",
        )]))
    }
}

/// Wave 1 — Run a jq query over a JSON value using the jaq crate family.
///
/// Shared by `tool_lad_jq` and its tests so the hot path and the unit tests
/// exercise identical behavior. Returns the filter's output stream as a
/// `Vec<Value>` so callers can distinguish single-result vs multi-result.
fn run_jq(query: &str, input: serde_json::Value) -> Result<Vec<serde_json::Value>, String> {
    use jaq_core::load::{Arena, File, Loader};
    use jaq_core::{Compiler, Ctx, Vars, data, unwrap_valr};
    use jaq_json::Val;

    let arena = Arena::default();
    let defs = jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs());
    let loader = Loader::new(defs);
    let program = File {
        code: query,
        path: (),
    };
    let modules = loader
        .load(&arena, program)
        .map_err(|e| format!("load error: {e:?}"))?;

    let funs = jaq_core::funs()
        .chain(jaq_std::funs())
        .chain(jaq_json::funs());
    let filter = Compiler::<_, _>::default()
        .with_funs(funs)
        .compile(modules)
        .map_err(|e| format!("compile error: {e:?}"))?;

    let val: Val =
        serde_json::from_value(input).map_err(|e| format!("cannot feed input to jaq: {e}"))?;
    let ctx = Ctx::<data::JustLut<Val>>::new(&filter.lut, Vars::new([]));

    let results: Result<Vec<Val>, _> = filter.id.run((ctx, val)).map(unwrap_valr).collect();
    let vals = results.map_err(|e| format!("runtime error: {e}"))?;

    // Val implements fmt::Display as JSON; round-trip through a string to
    // reach serde_json::Value (jaq_json 2.0 only implements Deserialize for Val).
    vals.into_iter()
        .map(|v| {
            let rendered = v.to_string();
            serde_json::from_str(&rendered).map_err(|e| format!("output decode error: {e}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::run_jq;
    use llm_as_dom::semantic::{Element, ElementKind, FormMeta, PageState, SemanticView};

    fn make_button(id: u32, label: &str) -> Element {
        Element {
            id,
            kind: ElementKind::Button,
            label: label.to_string(),
            name: None,
            value: None,
            placeholder: None,
            href: None,
            input_type: None,
            disabled: false,
            form_index: None,
            context: None,
            hint: None,
            checked: None,
            options: None,
            frame_index: None,
            is_visible: None,
        }
    }

    fn make_input(id: u32, label: &str) -> Element {
        Element {
            id,
            kind: ElementKind::Input,
            label: label.to_string(),
            name: Some(label.to_lowercase()),
            value: None,
            placeholder: None,
            href: None,
            input_type: Some("text".to_string()),
            disabled: false,
            form_index: Some(0),
            context: None,
            hint: None,
            checked: None,
            options: None,
            frame_index: None,
            is_visible: None,
        }
    }

    fn fixture_view() -> SemanticView {
        SemanticView {
            url: "https://example.com/login".to_string(),
            title: "Login to Example".to_string(),
            page_hint: "login page".to_string(),
            elements: vec![
                make_input(0, "Email"),
                make_input(1, "Password"),
                make_button(2, "Sign in"),
                make_button(3, "Cancel"),
            ],
            forms: vec![FormMeta {
                index: 0,
                action: Some("/login".to_string()),
                method: "POST".to_string(),
                id: Some("login-form".to_string()),
                name: None,
            }],
            visible_text: "Welcome back".to_string(),
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        }
    }

    fn input_value() -> serde_json::Value {
        serde_json::to_value(fixture_view()).unwrap()
    }

    #[test]
    fn tool_lad_jq_title_returns_string() {
        let out = run_jq(".title", input_value()).unwrap();
        assert_eq!(out, vec![serde_json::json!("Login to Example")]);
    }

    #[test]
    fn tool_lad_jq_button_labels_returns_array() {
        let query = r#".elements | map(select(.kind == "button")) | map(.label)"#;
        let out = run_jq(query, input_value()).unwrap();
        assert_eq!(out, vec![serde_json::json!(["Sign in", "Cancel"])]);
    }

    #[test]
    fn tool_lad_jq_elements_length_returns_number() {
        let out = run_jq(".elements | length", input_value()).unwrap();
        assert_eq!(out, vec![serde_json::json!(4)]);
    }

    #[test]
    fn tool_lad_jq_parse_error_is_surfaced() {
        let result = run_jq(".elements |", input_value());
        assert!(result.is_err(), "syntax error should not silently succeed");
    }
}
