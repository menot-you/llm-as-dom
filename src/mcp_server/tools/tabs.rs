//! Wave 2: tab management tools — `lad_tabs_list`, `lad_tabs_switch`,
//! `lad_tabs_close`.
//!
//! Tool shapes intentionally match Opera Neon's MCP Connector
//! (`list-tabs`, `switch-tab`, `close-tab`) so clients can swap LAD in as
//! a drop-in replacement for Opera's browser agent.
//!
//! All three tools route through [`crate::ActivePageGuard`] which holds the
//! `tabs` and `active_tab_id` mutexes in the documented lock order.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{mcp_err, to_pretty_json};
use crate::params::{TabCloseParams, TabSwitchParams, TabsListParams};

impl LadServer {
    /// List every open tab. Returns an array of `{tab_id, title, url,
    /// is_active}` objects — matching Opera Neon's `list-tabs` shape so the
    /// tool can be used as a drop-in replacement.
    pub(crate) async fn tool_lad_tabs_list(
        &self,
        _params: Parameters<TabsListParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let guard = self.lock_active_page().await;
        let active_id = guard.active_id();

        // Collect into a sorted Vec so the output is deterministic across
        // runs (HashMap iteration order is randomized).
        let mut entries: Vec<(u32, &crate::state::ActivePage)> =
            guard.tabs.iter().map(|(id, ap)| (*id, ap)).collect();
        entries.sort_by_key(|(id, _)| *id);

        let tabs: Vec<serde_json::Value> = entries
            .into_iter()
            .map(|(id, ap)| {
                serde_json::json!({
                    "tab_id": id,
                    "title": ap.view.title,
                    "url": llm_as_dom::sanitize::redact_url_secrets(&ap.url),
                    "is_active": Some(id) == active_id,
                })
            })
            .collect();

        let output = serde_json::json!({
            "count": tabs.len(),
            "active_tab_id": active_id,
            "tabs": tabs,
        });

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }

    /// Switch the active tab to the given ID. Errors if the tab does not
    /// exist. Idempotent: switching to the already-active tab is a no-op
    /// that still returns success.
    pub(crate) async fn tool_lad_tabs_switch(
        &self,
        params: Parameters<TabSwitchParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(tab_id = p.tab_id, "lad_tabs_switch");

        let mut guard = self.lock_active_page().await;
        if !guard.tabs.contains_key(&p.tab_id) {
            return Err(mcp_err(format!("tab_id {} not found", p.tab_id)));
        }
        *guard.active_id = Some(p.tab_id);

        let output = serde_json::json!({
            "status": "switched",
            "tab_id": p.tab_id,
        });
        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }

    /// Close the tab with the given ID. If this was the active tab, the
    /// active slot is cleared (the caller must switch to another tab before
    /// the next non-tab tool invocation). Errors if the tab does not exist.
    pub(crate) async fn tool_lad_tabs_close(
        &self,
        params: Parameters<TabCloseParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(tab_id = p.tab_id, "lad_tabs_close");

        let mut guard = self.lock_active_page().await;

        // Removing under the lock means there is no TOCTOU window where a
        // concurrent tools_list could observe a half-closed state.
        let Some(_removed) = guard.tabs.remove(&p.tab_id) else {
            return Err(mcp_err(format!("tab_id {} not found", p.tab_id)));
        };

        // If the closed tab was active, clear the slot. Otherwise leave
        // `active_tab_id` alone — other tabs remain reachable.
        let cleared_active = if *guard.active_id == Some(p.tab_id) {
            *guard.active_id = None;
            true
        } else {
            false
        };

        // Best-effort engine-side page close happens when `ActivePage` is
        // dropped (via the underlying `Box<dyn PageHandle>`). The engine
        // trait doesn't expose an explicit page-close today, so dropping is
        // the single canonical teardown path.
        drop(_removed);

        let output = serde_json::json!({
            "status": "closed",
            "tab_id": p.tab_id,
            "cleared_active": cleared_active,
        });
        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }
}

// ── Unit tests ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LadServer;
    use crate::state::ActivePage;
    use async_trait::async_trait;
    use llm_as_dom::engine::PageHandle;
    use llm_as_dom::semantic::{PageState, SemanticView};

    // ── Minimal fake PageHandle used for tab registry tests ─────────
    //
    // Every method is a no-op/default so we never need a real browser.
    // Only the methods `PageHandle` declares as required need bodies.

    struct FakePage;

    #[async_trait]
    impl PageHandle for FakePage {
        async fn eval_js(&self, _script: &str) -> Result<serde_json::Value, llm_as_dom::Error> {
            Ok(serde_json::Value::Null)
        }
        async fn navigate(&self, _url: &str) -> Result<(), llm_as_dom::Error> {
            Ok(())
        }
        async fn wait_for_navigation(&self) -> Result<(), llm_as_dom::Error> {
            Ok(())
        }
        async fn url(&self) -> Result<String, llm_as_dom::Error> {
            Ok("https://example.com/fake".to_string())
        }
        async fn title(&self) -> Result<String, llm_as_dom::Error> {
            Ok("Fake".to_string())
        }
        async fn screenshot_png(&self) -> Result<Vec<u8>, llm_as_dom::Error> {
            Ok(vec![])
        }
        async fn cookies(
            &self,
        ) -> Result<Vec<llm_as_dom::session::CookieEntry>, llm_as_dom::Error> {
            Ok(vec![])
        }
        async fn set_cookies(
            &self,
            _cookies: &[llm_as_dom::session::CookieEntry],
        ) -> Result<(), llm_as_dom::Error> {
            Ok(())
        }
    }

    fn fake_active_page(title: &str, url: &str) -> ActivePage {
        let view = SemanticView {
            url: url.to_string(),
            title: title.to_string(),
            page_hint: String::new(),
            elements: vec![],
            forms: vec![],
            visible_text: String::new(),
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        };
        ActivePage {
            page: Box::new(FakePage),
            url: url.to_string(),
            view,
        }
    }

    /// Builds a fresh `LadServer` plus `n` pre-populated fake tabs.
    ///
    /// Returns `(server, Vec<tab_id>)` so tests can pick which tab to
    /// manipulate. IDs start at 1 and are allocated by the real
    /// `insert_tab` path so we exercise the same code as production.
    async fn make_server_with_fake_tabs(n: usize) -> (LadServer, Vec<u32>) {
        let server = LadServer::new();
        let mut ids = Vec::with_capacity(n);
        for i in 0..n {
            let ap = fake_active_page(
                &format!("Fake Tab {i}"),
                &format!("https://example.com/tab-{i}"),
            );
            ids.push(server.insert_tab(ap).await);
        }
        (server, ids)
    }

    // ── Task 2.1: state refactor tests ──────────────────────────

    #[tokio::test]
    async fn insert_tab_allocates_sequential_ids() {
        let (_server, ids) = make_server_with_fake_tabs(3).await;
        assert_eq!(ids, vec![1, 2, 3], "ids should start at 1 and increment");
    }

    #[tokio::test]
    async fn resolve_tab_id_uses_active_when_none() {
        let (server, ids) = make_server_with_fake_tabs(2).await;
        // insert_tab marks the last-inserted tab as active.
        let guard = server.lock_active_page().await;
        let ap = guard.resolve(None).expect("active tab should resolve");
        assert_eq!(ap.view.title, "Fake Tab 1"); // index 1 = second tab (last inserted)
        assert_eq!(guard.active_id(), Some(ids[1]));
    }

    #[tokio::test]
    async fn resolve_tab_id_errors_on_unknown_id() {
        let (server, _ids) = make_server_with_fake_tabs(1).await;
        let guard = server.lock_active_page().await;
        // `ActivePage` intentionally doesn't impl Debug (it holds a boxed
        // trait object), so we can't use `expect_err` — match on the
        // Result shape directly.
        match guard.resolve(Some(999)) {
            Ok(_) => panic!("unknown id must error"),
            Err(err) => assert!(
                err.message.contains("999"),
                "error should mention the bad id: {}",
                err.message
            ),
        }
    }

    #[tokio::test]
    async fn tabs_hashmap_survives_close_and_reopen() {
        let server = LadServer::new();
        // Open → close → reopen: allocator should NOT be reset by clear_active
        // (only `clear_all_tabs` does that). Each insert_tab gets a fresh id.
        let id1 = server
            .insert_tab(fake_active_page("One", "https://a.test"))
            .await;
        {
            let mut guard = server.lock_active_page().await;
            guard.clear_active();
        }
        let id2 = server
            .insert_tab(fake_active_page("Two", "https://b.test"))
            .await;
        assert_ne!(id1, id2, "allocator must not collide after clear_active");
        assert!(id2 > id1, "monotonic allocation");
    }

    // ── Task 2.2: tab management tool tests ─────────────────────

    #[tokio::test]
    async fn tabs_list_returns_all_with_correct_active_flag() {
        let (server, ids) = make_server_with_fake_tabs(3).await;
        // Make the middle tab active explicitly.
        *server.active_tab_id.lock().await = Some(ids[1]);

        let result = server
            .tool_lad_tabs_list(Parameters(TabsListParams {}))
            .await
            .unwrap();

        // Parse the JSON text content back for assertions.
        // `content` is `Vec<Content>`; `Content = Annotated<RawContent>`
        // so deref gives us `RawContent`'s `as_text()`.
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .expect("list should return a text block")
            .text
            .clone();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();

        assert_eq!(parsed["count"], 3);
        assert_eq!(parsed["active_tab_id"], ids[1]);

        let tabs = parsed["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 3);
        assert_eq!(tabs[0]["tab_id"], ids[0]);
        assert_eq!(tabs[0]["is_active"], false);
        assert_eq!(tabs[1]["tab_id"], ids[1]);
        assert_eq!(tabs[1]["is_active"], true);
        assert_eq!(tabs[2]["is_active"], false);
    }

    #[tokio::test]
    async fn tabs_switch_changes_active_tab_id() {
        let (server, ids) = make_server_with_fake_tabs(2).await;
        // Initially the last-inserted tab is active.
        assert_eq!(*server.active_tab_id.lock().await, Some(ids[1]));

        server
            .tool_lad_tabs_switch(Parameters(TabSwitchParams { tab_id: ids[0] }))
            .await
            .unwrap();

        assert_eq!(*server.active_tab_id.lock().await, Some(ids[0]));
    }

    #[tokio::test]
    async fn tabs_switch_errors_on_unknown_id() {
        let (server, _ids) = make_server_with_fake_tabs(1).await;
        let err = server
            .tool_lad_tabs_switch(Parameters(TabSwitchParams { tab_id: 9999 }))
            .await
            .expect_err("unknown id must fail");
        assert!(err.message.contains("9999"));
    }

    #[tokio::test]
    async fn tabs_close_removes_tab() {
        let (server, ids) = make_server_with_fake_tabs(2).await;
        server
            .tool_lad_tabs_close(Parameters(TabCloseParams { tab_id: ids[0] }))
            .await
            .unwrap();

        let guard = server.lock_active_page().await;
        assert!(!guard.tabs.contains_key(&ids[0]));
        assert!(guard.tabs.contains_key(&ids[1]));
    }

    #[tokio::test]
    async fn tabs_close_clears_active_when_closing_active_tab() {
        let (server, ids) = make_server_with_fake_tabs(2).await;
        // Last-inserted tab is active.
        assert_eq!(*server.active_tab_id.lock().await, Some(ids[1]));

        server
            .tool_lad_tabs_close(Parameters(TabCloseParams { tab_id: ids[1] }))
            .await
            .unwrap();

        assert_eq!(*server.active_tab_id.lock().await, None);
    }

    #[tokio::test]
    async fn tabs_close_preserves_active_when_closing_other_tab() {
        let (server, ids) = make_server_with_fake_tabs(3).await;
        // Active = last-inserted = ids[2]
        assert_eq!(*server.active_tab_id.lock().await, Some(ids[2]));

        server
            .tool_lad_tabs_close(Parameters(TabCloseParams { tab_id: ids[0] }))
            .await
            .unwrap();

        // Still active; unrelated close should not flip the slot.
        assert_eq!(*server.active_tab_id.lock().await, Some(ids[2]));
    }

    #[tokio::test]
    async fn tabs_close_errors_on_unknown_id() {
        let (server, _ids) = make_server_with_fake_tabs(1).await;
        let err = server
            .tool_lad_tabs_close(Parameters(TabCloseParams { tab_id: 9999 }))
            .await
            .expect_err("unknown id must fail");
        assert!(err.message.contains("9999"));
    }
}
