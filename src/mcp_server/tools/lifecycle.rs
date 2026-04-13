//! Lifecycle tools: `lad_close`, `lad_session`.
//!
//! Wave 3 added two new `lad_session` actions — `attach_cdp` and
//! `detach` — that surface the [`ChromiumEngine::attach`] path to MCP
//! callers. See `docs/attach-chrome.md` for an end-to-end walkthrough.

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use llm_as_dom::a11y;
use llm_as_dom::engine::BrowserEngine;
use llm_as_dom::engine::chromium::ChromiumEngine;

use crate::LadServer;
use crate::helpers::{mcp_err, to_pretty_json};
use crate::params::SessionParams;
use crate::state::{ActivePage, McpSessionState};

impl LadServer {
    /// Close the browser and release all resources.
    ///
    /// Wave 2: clears *every* open tab (not just the active one) and resets
    /// the tab-id allocator back to 1, matching "close the browser" semantics.
    pub(crate) async fn tool_lad_close(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        tracing::info!("lad_close");

        // FIX-11: Abort any active watch before closing the browser so
        // the polling task doesn't leak.
        if let Some(ws) = self.watch_state.lock().await.take() {
            ws.stop();
        }

        // Wave 2: drop every open tab and reset the allocator.
        self.clear_all_tabs().await;

        // Close the engine if one was launched
        let mut engine_lock = self.engine.lock().await;
        if let Some(engine) = engine_lock.take() {
            engine.close().await.map_err(mcp_err)?;
        }

        Ok(CallToolResult::success(vec![Content::text(
            r#"{"status": "browser closed"}"#.to_string(),
        )]))
    }

    /// Inspect or reset the MCP session state.
    pub(crate) async fn tool_lad_session(
        &self,
        params: Parameters<SessionParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(action = %p.action, "lad_session");

        match p.action.as_str() {
            "get" => {
                let session = self.session.lock().await;
                let output = serde_json::to_value(&*session)
                    .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));
                Ok(CallToolResult::success(vec![Content::text(
                    to_pretty_json(&output),
                )]))
            }
            "clear" => {
                let mut session = self.session.lock().await;
                *session = McpSessionState::default();
                Ok(CallToolResult::success(vec![Content::text(
                    r#"{"status": "session cleared"}"#.to_string(),
                )]))
            }
            "attach_cdp" => self.session_attach_cdp(p.endpoint, p.adopt_existing).await,
            "detach" => self.session_detach().await,
            other => {
                let msg = format!(
                    "unknown session action '{}'. Valid actions: 'get', 'clear', 'attach_cdp', 'detach'.",
                    other
                );
                Err(rmcp::ErrorData::invalid_params(msg, None))
            }
        }
    }

    /// Wave 3: handle `lad_session action=attach_cdp`. Validates the
    /// endpoint (loopback-only), tears down any active engine, connects
    /// via [`ChromiumEngine::attach`], and optionally adopts existing
    /// tabs into the LAD tab map.
    ///
    /// Extracted into a helper so `tool_lad_session`'s match arms stay
    /// under 100 LOC (clean-code gate).
    async fn session_attach_cdp(
        &self,
        endpoint: Option<String>,
        adopt_existing: Option<bool>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let endpoint = endpoint.ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                "attach_cdp requires the 'endpoint' param (e.g. http://localhost:9222)".to_string(),
                None,
            )
        })?;

        // Defense in depth: the loopback gate also lives inside
        // `ChromiumEngine::attach`, but we re-check here so callers
        // get a clearer MCP-level error before any network or engine
        // state touches the wire.
        if !llm_as_dom::sanitize::is_loopback_only(&endpoint) {
            return Err(rmcp::ErrorData::invalid_params(
                format!(
                    "attach endpoint must be loopback (localhost/127.0.0.1/::1) — got: {endpoint}"
                ),
                None,
            ));
        }

        // Tear down any currently-active engine + tabs. Lock order:
        // tabs → active_tab_id (via clear_all_tabs) → engine. We
        // acquire `engine` LAST so we never hold it while hitting the
        // tabs mutex. Note: the engine Arc may be a launched headless
        // Chrome OR another attach — either way, close() is idempotent.
        self.clear_all_tabs().await;
        {
            let mut engine_lock = self.engine.lock().await;
            if let Some(prev) = engine_lock.take()
                && let Err(e) = prev.close().await
            {
                tracing::warn!(error = %e, "previous engine close failed — continuing attach");
            }
        }

        // Connect. ChromiumEngine::attach performs the HTTP discovery
        // (if endpoint is http://) and re-validates loopback on the
        // resolved WS URL.
        let attached = ChromiumEngine::attach(&endpoint).await.map_err(mcp_err)?;
        let engine: Arc<dyn BrowserEngine> = Arc::new(attached);
        {
            let mut engine_lock = self.engine.lock().await;
            *engine_lock = Some(Arc::clone(&engine));
        }

        // Optionally adopt pre-existing tabs. Default: true — adopting
        // is the killer value prop of attach mode (instantly see the
        // user's open tabs as LAD tabs). Set to false for a clean slate.
        let adopt = adopt_existing.unwrap_or(true);
        let adopted_tabs = if adopt {
            self.adopt_engine_pages(&engine).await?
        } else {
            0
        };

        let payload = serde_json::json!({
            "status": "attached",
            "endpoint": endpoint,
            "adopted_tabs": adopted_tabs,
        });
        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&payload),
        )]))
    }

    /// Wave 3: enumerate every page on the attached engine and insert
    /// them into LAD's tab map as `ActivePage` entries. Returns the
    /// number of tabs actually inserted — pages that fail semantic
    /// extraction (e.g. chrome://settings, cross-origin iframes) are
    /// skipped and logged, not fatal.
    async fn adopt_engine_pages(
        &self,
        engine: &Arc<dyn BrowserEngine>,
    ) -> Result<usize, rmcp::ErrorData> {
        let pages = engine.adopt_existing_pages().await.map_err(mcp_err)?;
        let total = pages.len();
        let mut inserted = 0usize;
        for page in pages {
            match Self::build_adopted_active_page(page).await {
                Ok(ap) => {
                    self.insert_tab(ap).await;
                    inserted += 1;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "skipping adopted page — extraction failed");
                }
            }
        }
        tracing::info!(total, inserted, "adopted existing CDP tabs");
        Ok(inserted)
    }

    /// Build an `ActivePage` from a freshly-adopted `PageHandle`.
    /// Pulls URL + title, best-effort waits for content, and extracts
    /// the semantic view. Private helper so the attach flow doesn't
    /// grow a god function.
    async fn build_adopted_active_page(
        page: Box<dyn llm_as_dom::engine::PageHandle>,
    ) -> Result<ActivePage, llm_as_dom::Error> {
        let url = page.url().await.unwrap_or_else(|_| "about:blank".into());

        // Best-effort: wait_for_content may fail on chrome:// pages
        // or pages that never fire load events. Don't bail on it —
        // just log and continue.
        if let Err(e) = a11y::wait_for_content(page.as_ref(), a11y::DEFAULT_WAIT_TIMEOUT).await {
            tracing::debug!(error = %e, url = %url, "wait_for_content failed on adopted page");
        }

        let view = a11y::extract_semantic_view(page.as_ref()).await?;
        Ok(ActivePage { page, url, view })
    }

    /// Wave 3: handle `lad_session action=detach`. Clears every tab
    /// and drops the engine Arc without killing the user's Chrome.
    /// Idempotent — calling detach when no engine is attached returns
    /// `{status: "already detached"}` without error.
    async fn session_detach(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let had_engine = {
            let engine_lock = self.engine.lock().await;
            engine_lock.is_some()
        };
        if !had_engine {
            return Ok(CallToolResult::success(vec![Content::text(
                r#"{"status": "already detached"}"#.to_string(),
            )]));
        }

        // Tabs first (lock order), then engine.
        self.clear_all_tabs().await;
        let mut engine_lock = self.engine.lock().await;
        if let Some(engine) = engine_lock.take()
            && let Err(e) = engine.close().await
        {
            tracing::warn!(error = %e, "engine.close() failed on detach — continuing");
        }

        Ok(CallToolResult::success(vec![Content::text(
            r#"{"status": "detached"}"#.to_string(),
        )]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a bare-bones LadServer for unit tests. Does not touch
    /// the network or spawn any browser — just a struct with default
    /// state so we can exercise session match arms.
    fn test_server() -> LadServer {
        LadServer::new_for_test()
    }

    #[tokio::test]
    async fn session_attach_cdp_requires_endpoint() {
        let server = test_server();
        let res = server.session_attach_cdp(None, None).await;
        match res {
            Err(err) => {
                let msg = err.message.to_string();
                assert!(msg.contains("endpoint"), "unexpected message: {msg}");
            }
            Ok(_) => panic!("expected invalid_params error, got Ok"),
        }
    }

    #[tokio::test]
    async fn session_attach_cdp_rejects_remote_endpoint() {
        let server = test_server();
        let res = server
            .session_attach_cdp(Some("http://192.168.1.1:9222".to_string()), None)
            .await;
        match res {
            Err(err) => {
                let msg = err.message.to_string();
                assert!(msg.contains("loopback"), "unexpected message: {msg}");
            }
            Ok(_) => panic!("expected invalid_params error, got Ok"),
        }
    }

    #[tokio::test]
    async fn session_attach_cdp_rejects_evil_ws_host() {
        let server = test_server();
        let res = server
            .session_attach_cdp(
                Some("ws://evil.com:9222/devtools/browser/x".to_string()),
                None,
            )
            .await;
        assert!(res.is_err(), "expected error, got Ok");
    }

    #[tokio::test]
    async fn session_attach_cdp_rejects_missing_host_url() {
        let server = test_server();
        let res = server
            .session_attach_cdp(Some("http:///".to_string()), None)
            .await;
        assert!(res.is_err(), "expected error, got Ok");
    }

    /// Pull the first text-content payload out of a `CallToolResult`,
    /// panicking if the shape is not text. Keeps test assertions terse.
    fn text_body(res: &CallToolResult) -> String {
        match &res.content.first().expect("content must not be empty").raw {
            rmcp::model::RawContent::Text(t) => t.text.clone(),
            _ => panic!("expected text content"),
        }
    }

    #[tokio::test]
    async fn session_detach_without_engine_returns_already_detached() {
        let server = test_server();
        let result = server
            .session_detach()
            .await
            .expect("detach without engine should succeed");
        let body = text_body(&result);
        assert!(body.contains("already detached"), "unexpected body: {body}");
    }

    /// End-to-end attach + detach test. Requires a real Chrome running
    /// with `--remote-debugging-port=9222`. See `docs/attach-chrome.md`
    /// for setup.
    #[tokio::test]
    #[ignore = "needs live chrome with --remote-debugging-port=9222"]
    async fn session_attach_and_detach_real_chrome() {
        let server = test_server();
        let res = server
            .session_attach_cdp(Some("http://localhost:9222".to_string()), Some(true))
            .await
            .expect("attach should succeed");
        let body = text_body(&res);
        assert!(body.contains("attached"), "unexpected body: {body}");

        let detach = server
            .session_detach()
            .await
            .expect("detach should succeed");
        let body = text_body(&detach);
        assert!(body.contains("detached"), "unexpected body: {body}");
    }
}
