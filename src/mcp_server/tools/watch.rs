//! `lad_watch` tool — page state monitoring.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::to_pretty_json;
use crate::params::WatchParams;

use llm_as_dom::engine::PageHandle;
use llm_as_dom::{a11y, watch};

use std::sync::Arc;

impl LadServer {
    pub(crate) async fn tool_lad_watch(
        &self,
        params: Parameters<WatchParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(action = %p.action, "lad_watch");

        match p.action.as_str() {
            "start" => self.watch_start(p).await,
            "events" => self.watch_events(p).await,
            "stop" => self.watch_stop().await,
            other => Err(rmcp::ErrorData::invalid_params(
                format!("action must be start, events, or stop (got '{other}')"),
                None,
            )),
        }
    }

    /// Start watching a URL: navigate, extract initial view, spawn polling loop.
    async fn watch_start(&self, p: WatchParams) -> Result<CallToolResult, rmcp::ErrorData> {
        // Reject if already watching
        if self.watch_state.lock().await.is_some() {
            return Err(rmcp::ErrorData::invalid_params(
                "a watch is already active — stop it first",
                None,
            ));
        }

        let url = p.url.as_deref().unwrap_or("about:blank");
        let interval_ms = p.interval_ms.unwrap_or(1000);

        // FIX-12: Reject zero or sub-100ms interval to prevent tight-loop CPU burn.
        if interval_ms < 100 {
            return Err(rmcp::ErrorData::invalid_params(
                format!("interval_ms must be >= 100 (got {interval_ms})"),
                None,
            ));
        }

        // Navigate and capture the initial semantic view
        let (page, initial_view) = self.navigate_and_extract(url).await?;

        // Build extract closure: captures the page handle so the polling
        // loop can re-extract semantic views without dropping the page.
        let page: Arc<dyn PageHandle> = Arc::from(page);
        let page_clone = Arc::clone(&page);
        let extract_fn = move || {
            let p = Arc::clone(&page_clone);
            async move { a11y::extract_semantic_view(p.as_ref()).await.ok() }
        };

        let ws = watch::start_watch(
            watch::WatchConfig {
                url: url.to_owned(),
                interval_ms,
                initial_view,
                peer: Some(Arc::clone(&self.peer)),
            },
            extract_fn,
        );

        let resource_uri = ws.resource_uri();
        *self.watch_state.lock().await = Some(ws);

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Watching {url} every {interval_ms}ms. Use lad_watch(action=\"events\") to retrieve diffs. Resource URI: {resource_uri}"
        ))]))
    }

    /// Return buffered watch events since an optional cursor.
    async fn watch_events(&self, p: WatchParams) -> Result<CallToolResult, rmcp::ErrorData> {
        let guard = self.watch_state.lock().await;
        let ws = guard.as_ref().ok_or_else(|| {
            rmcp::ErrorData::invalid_params("no active watch — start one first", None)
        })?;

        let events = ws.events.events_since(p.since_seq).await;
        let output = serde_json::json!({
            "url": ws.url,
            "event_count": events.len(),
            "current_seq": ws.events.current_seq(),
            "events": events,
        });

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }

    /// Stop an active watch and return summary.
    async fn watch_stop(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let ws = self
            .watch_state
            .lock()
            .await
            .take()
            .ok_or_else(|| rmcp::ErrorData::invalid_params("no active watch to stop", None))?;

        let url = ws.url.clone();
        let buf = ws.stop();
        let total = buf.current_seq();

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Stopped watching {url}. Total events captured: {total}"
        ))]))
    }
}
