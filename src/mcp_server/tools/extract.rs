//! `lad_extract`, `lad_snapshot`, `lad_screenshot`, `lad_jq` tools.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{mcp_err, to_pretty_json};
use crate::params::{ExtractParams, JqParams, SnapshotParams};

/// Issue #36 — apply `what` as a semantic filter to a `SemanticView`.
///
/// Pure function, no I/O — spliced out of `tool_lad_extract` so the
/// scoring logic can be unit-tested without a browser or MCP server.
///
/// Behavior:
/// 1. Tokenize `what` on whitespace, drop single-char noise.
/// 2. Score each element by token-match over label/name/placeholder/href.
/// 3. Stable-sort elements by score desc; return `relevant_count`
///    (elements with score > 0).
/// 4. If `strict` and the query is non-empty, drop score-zero elements.
/// 5. If `text_blocks` is non-empty and the query is non-empty, replace
///    `visible_text` with the top-K scoring blocks joined by " … ". K
///    scales from `max_length` when provided, clamped to `[3, 20]`.
/// 6. Zero-match pages leave `visible_text` untouched so the caller
///    never loses page context to an overly strict filter.
/// 7. `max_length` is applied LAST as a byte-budget safety cap with a
///    UTF-8 char-boundary walk to avoid splitting multibyte codepoints.
///
/// Back-compat: empty `what` or empty `text_blocks` falls back to the
/// pre-#36 behavior (sort-only, raw banner).
pub(crate) fn apply_what_filter(
    view: &mut llm_as_dom::semantic::SemanticView,
    what: &str,
    max_length: Option<usize>,
    strict: bool,
) -> usize {
    let what_lower = what.to_lowercase();
    let what_words: Vec<String> = what_lower
        .split_whitespace()
        .filter(|w| w.len() >= 2) // skip single-char noise (a, I, …)
        .map(|w| w.to_string())
        .collect();
    let has_query = !what_words.is_empty();

    let score_element = |el: &llm_as_dom::semantic::Element| -> u32 {
        if !has_query {
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
                if field.contains(word.as_str()) {
                    score += 1;
                }
            }
        }
        score
    };

    // Sort relevant elements first (stable sort preserves DOM order within same score).
    view.elements
        .sort_by_key(|el| std::cmp::Reverse(score_element(el)));
    let relevant_count = view
        .elements
        .iter()
        .filter(|el| score_element(el) > 0)
        .count();

    // Strict mode drops zero-score elements entirely.
    if strict && has_query {
        view.elements.retain(|el| score_element(el) > 0);
    }

    // Replace `visible_text` with top-K scoring blocks when available.
    if has_query && !view.text_blocks.is_empty() {
        let score_block = |block: &str| -> u32 {
            let lower = block.to_lowercase();
            what_words
                .iter()
                .filter(|w| lower.contains(w.as_str()))
                .count() as u32
        };
        let mut scored: Vec<(u32, &String)> = view
            .text_blocks
            .iter()
            .map(|b| (score_block(b), b))
            .filter(|(s, _)| *s > 0)
            .collect();
        // Sort by score desc, then by block length desc as a cheap
        // "prefer substantial matches over one-word hits" tiebreak.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.len().cmp(&a.1.len())));
        // K scales with max_length when the caller set one; default to
        // 8 otherwise. Clamped to [3, 20].
        let top_k = max_length
            .map(|m| (m / 240).max(3))
            .unwrap_or(8)
            .clamp(3, 20);
        let joined = scored
            .iter()
            .take(top_k)
            .map(|(_, b)| b.as_str())
            .collect::<Vec<_>>()
            .join(" … ");
        if !joined.is_empty() {
            view.visible_text = joined;
        }
        // If zero blocks matched, leave visible_text unchanged so the
        // caller still sees page context — empty is worse than noise
        // when the query genuinely doesn't match anything.
    }

    // Apply `max_length` AFTER scoring as a final safety cap. UTF-8 safe.
    if let Some(max_len) = max_length
        && view.visible_text.len() > max_len
    {
        let mut end = max_len;
        while !view.visible_text.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        view.visible_text.truncate(end);
    }

    relevant_count
}

impl LadServer {
    /// Extract structured information from a web page.
    /// Returns interactive elements, visible text, page classification.
    /// Never returns raw HTML.
    ///
    /// FIX-18: The `what` parameter scores elements by relevance.
    /// Elements whose label, name, placeholder, or href contain any word
    /// from `what` are promoted to the front; `relevant_count` tells the
    /// caller how many matched.
    ///
    /// Issue #36: `what` is now also a semantic filter over `visible_text`.
    /// The JS walker emits `text_blocks` (individual headings/paragraphs/
    /// td/span/a/pre/code) which we score against `what` and concatenate
    /// the top-K matches into `visible_text`, replacing the hard-capped
    /// 500-char banner. Empty `what` or zero-match pages fall back to the
    /// original banner so the caller never loses page context.
    ///
    /// When `strict=true` and `what` is non-empty, elements with score 0
    /// are dropped instead of just re-ordered — useful on inventory-heavy
    /// pages (GitHub, HN) where the full element list is noise.
    ///
    /// DX-W2-1: `url` is now optional. When omitted, extracts from the
    /// current active page without navigating — preserving session state.
    pub(crate) async fn tool_lad_extract(
        &self,
        params: Parameters<ExtractParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        let include_hidden = p.include_hidden.unwrap_or(false);
        let strict = p.strict.unwrap_or(false);
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

        // FIX-18 / Issue #36: apply `what` as a semantic filter — scores
        // elements, rewrites visible_text from top-K matching blocks, and
        // (when strict=true) drops zero-score elements. Returns the
        // number of elements that matched for the JSON response.
        let relevant_count = apply_what_filter(&mut view, &p.what, p.max_length, strict);

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
            text_blocks: vec![],
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

    // ── Issue #36: apply_what_filter ──────────────────────────────────

    use super::apply_what_filter;

    /// Content-heavy fixture — mimics a GitHub-style page: a handful of
    /// relevant elements buried in a pile of navigation noise, plus 30+
    /// text_blocks where only a few match a focused `what` query.
    fn content_heavy_fixture() -> SemanticView {
        let mut elements = vec![
            // Navigation noise (score 0 against "install star").
            make_button(0, "Navigation Menu"),
            make_button(1, "Saved searches"),
            make_button(2, "Sign in"),
            make_button(3, "Sign up"),
            make_button(4, "Skip to content"),
        ];
        // Relevant elements (score > 0 for "install star").
        let mut install = make_button(10, "Install");
        install.href = Some("/browser-use#installation".into());
        elements.push(install);
        let mut stargazers = make_button(11, "Stargazers");
        stargazers.href = Some("/browser-use/stargazers".into());
        elements.push(stargazers);
        // Padding noise to mimic GitHub's ~120-element inventory.
        for i in 20..60 {
            elements.push(make_button(i, &format!("Folder item {i}")));
        }

        SemanticView {
            url: "https://github.com/browser-use/browser-use".to_string(),
            title: "browser-use/browser-use".to_string(),
            page_hint: "content page".to_string(),
            elements,
            forms: vec![],
            visible_text: "Navigation Menu Saved searches browser-use/browser-use Folders and files Latest commit History Repository files navigation Want to skip the setup?".to_string(),
            text_blocks: vec![
                // Noise blocks.
                "Navigation Menu".into(),
                "Saved searches".into(),
                "Folders and files".into(),
                "Latest commit".into(),
                "History".into(),
                "Repository files navigation".into(),
                // Relevant blocks.
                "Make websites accessible for AI agents. Installation: pip install browser-use. Star count: 12k on GitHub.".into(),
                "Quick start: pip install browser-use && browser-use --help".into(),
                "Tagline: Open-source browser automation for LLM agents.".into(),
                // More noise.
                "Contributors".into(),
                "Releases".into(),
                "Packages".into(),
                "Languages".into(),
                "Want to skip the setup?".into(),
            ],
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        }
    }

    #[test]
    fn apply_what_filter_empty_query_is_noop() {
        let mut view = content_heavy_fixture();
        let banner_before = view.visible_text.clone();
        let elements_before = view.elements.len();

        let relevant = apply_what_filter(&mut view, "", None, false);

        assert_eq!(relevant, 0, "empty query yields zero relevant count");
        assert_eq!(view.visible_text, banner_before, "banner unchanged");
        assert_eq!(
            view.elements.len(),
            elements_before,
            "element count unchanged"
        );
    }

    #[test]
    fn apply_what_filter_rewrites_visible_text_with_matching_blocks() {
        let mut view = content_heavy_fixture();
        let banner_before = view.visible_text.clone();

        apply_what_filter(&mut view, "install star count", None, false);

        // The matching blocks replace the banner. Noise like "Navigation
        // Menu Saved searches" must NOT be the whole answer anymore.
        assert_ne!(
            view.visible_text, banner_before,
            "visible_text should be rewritten when text_blocks match"
        );
        // Matches: "install", "star count", "pip install", "browser-use"...
        let lower = view.visible_text.to_lowercase();
        assert!(
            lower.contains("install"),
            "install should surface: got {:?}",
            view.visible_text
        );
        assert!(
            lower.contains("star count"),
            "star count should surface: got {:?}",
            view.visible_text
        );
        // Navigation noise should be absent — the scored blocks rejected it.
        assert!(
            !view.visible_text.contains("Navigation Menu Saved searches"),
            "noise banner should not dominate the rewritten visible_text"
        );
    }

    #[test]
    fn apply_what_filter_promotes_relevant_elements() {
        let mut view = content_heavy_fixture();

        apply_what_filter(&mut view, "install star", None, false);

        // Elements with matches (Install, Stargazers) should be on top.
        let top_labels: Vec<&str> = view
            .elements
            .iter()
            .take(2)
            .map(|e| e.label.as_str())
            .collect();
        assert!(
            top_labels.contains(&"Install") || top_labels.contains(&"Stargazers"),
            "relevant elements should sort to the top, got: {top_labels:?}"
        );
    }

    #[test]
    fn apply_what_filter_strict_drops_zero_score_elements() {
        let mut view = content_heavy_fixture();
        let elements_before = view.elements.len();

        let relevant = apply_what_filter(&mut view, "install star", None, true);

        assert_eq!(
            view.elements.len(),
            relevant,
            "strict mode keeps exactly the scored elements"
        );
        assert!(
            view.elements.len() < elements_before,
            "strict mode should shrink the inventory"
        );
        assert!(relevant > 0, "at least one element should match");
    }

    #[test]
    fn apply_what_filter_no_match_preserves_banner() {
        let mut view = content_heavy_fixture();
        let banner_before = view.visible_text.clone();

        // "zebra" matches nothing in the fixture.
        apply_what_filter(&mut view, "zebra tapeworm kombucha", None, false);

        assert_eq!(
            view.visible_text, banner_before,
            "zero-match query must not erase the banner — empty is worse \
             than noise when the query genuinely doesn't match"
        );
    }

    #[test]
    fn apply_what_filter_no_text_blocks_falls_back_to_banner() {
        let mut view = content_heavy_fixture();
        view.text_blocks.clear(); // legacy view simulation
        let banner_before = view.visible_text.clone();

        apply_what_filter(&mut view, "install star", None, false);

        assert_eq!(
            view.visible_text, banner_before,
            "without text_blocks, scorer falls back to raw banner"
        );
    }

    #[test]
    fn apply_what_filter_max_length_final_safety_cap() {
        let mut view = content_heavy_fixture();

        apply_what_filter(&mut view, "install star count", Some(50), false);

        assert!(
            view.visible_text.len() <= 50,
            "max_length must be honored as final byte cap, got {} bytes",
            view.visible_text.len()
        );
    }

    #[test]
    fn apply_what_filter_ignores_single_char_tokens() {
        let mut view = content_heavy_fixture();
        let elements_before: Vec<u32> = view.elements.iter().map(|e| e.id).collect();

        // "a I" would match nothing useful; scorer skips <2-char tokens.
        apply_what_filter(&mut view, "a I", None, false);

        let elements_after: Vec<u32> = view.elements.iter().map(|e| e.id).collect();
        assert_eq!(
            elements_before, elements_after,
            "single-char tokens should not reorder elements"
        );
    }

    #[test]
    fn apply_what_filter_case_insensitive() {
        let mut view = content_heavy_fixture();

        apply_what_filter(&mut view, "INSTALL STAR", None, false);

        let lower = view.visible_text.to_lowercase();
        assert!(
            lower.contains("install"),
            "case-insensitive match should surface install, got {:?}",
            view.visible_text
        );
    }

    #[test]
    fn apply_what_filter_handles_unicode_blocks_without_panic() {
        let mut view = SemanticView {
            url: "https://example.com".into(),
            title: "Unicode".into(),
            page_hint: "".into(),
            elements: vec![],
            forms: vec![],
            visible_text: String::new(),
            text_blocks: vec![
                "install \u{200B}token\u{200D}".into(),
                "emoji 🚀 rocket install".into(),
                "rtl \u{202E}reversed install".into(),
                "installation guide here".into(),
            ],
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        };
        // Must not panic on zero-width, emoji, RTL markers.
        apply_what_filter(&mut view, "install", Some(200), false);
        // The substring "install" matches "installation" — matching block
        // should be selected without boundary panics.
        assert!(
            view.visible_text.to_lowercase().contains("install"),
            "expected install to surface: got {:?}",
            view.visible_text
        );
    }

    #[test]
    fn apply_what_filter_prefers_longer_matching_block() {
        let mut view = SemanticView {
            url: "https://example.com".into(),
            title: "T".into(),
            page_hint: "".into(),
            elements: vec![],
            forms: vec![],
            visible_text: "short".into(),
            text_blocks: vec![
                // Same score (1 hit on "foo"), different lengths — longer wins.
                "foo".into(),
                "detailed explanation of foo with all the context you need".into(),
            ],
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        };
        apply_what_filter(&mut view, "foo", None, false);
        assert!(
            view.visible_text.contains("detailed explanation"),
            "tiebreak should prefer the longer block: got {:?}",
            view.visible_text
        );
    }
}
