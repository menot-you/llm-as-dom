//! Debug/escape-hatch tools: `lad_eval`, `lad_network`, `lad_locate`.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{mcp_err, no_active_page, to_pretty_json};
use crate::params::{EvalParams, LocateParams, NetworkParams};

use llm_as_dom::{locate, network};

impl LadServer {
    /// Evaluate arbitrary JavaScript on the active page.
    pub(crate) async fn tool_lad_eval(
        &self,
        params: Parameters<EvalParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(script = %p.script, "lad_eval");

        let active = self.active_page.lock().await;
        let ap = active.as_ref().ok_or_else(no_active_page)?;
        let result = ap.page.eval_js(&p.script).await.map_err(mcp_err)?;

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&result),
        )]))
    }

    /// Locate a DOM element's source file using dev-mode source maps.
    pub(crate) async fn tool_lad_locate(
        &self,
        params: Parameters<LocateParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(url = %p.url, selector = %p.selector, "lad_locate");

        let (page, _view) = self.navigate_and_extract(&p.url).await?;
        let js = locate::build_locate_js(&p.selector);
        let raw_value = page.eval_js(&js).await.map_err(mcp_err)?;

        let raw: locate::RawLocateResult = serde_json::from_value(raw_value)
            .map_err(|e| mcp_err(format!("locate JS parse failed: {e:?}")))?;

        match locate::parse_locate_result(raw) {
            Ok(locate_result) => {
                let output = serde_json::to_value(&locate_result)
                    .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));
                Ok(CallToolResult::success(vec![Content::text(
                    to_pretty_json(&output),
                )]))
            }
            Err(msg) => Ok(CallToolResult::success(vec![Content::text(
                to_pretty_json(&serde_json::json!({
                    "error": msg,
                    "source_maps": "not available",
                })),
            )])),
        }
    }

    /// Inspect network traffic captured during browsing.
    pub(crate) async fn tool_lad_network(
        &self,
        params: Parameters<NetworkParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(filter = %p.filter, "lad_network");

        let guard = self.active_page.lock().await;
        let active = guard.as_ref().ok_or_else(no_active_page)?;

        // Use performance.getEntries() to gather network timing data via JS.
        let js = r#"JSON.stringify(
            performance.getEntriesByType('resource').concat(
                performance.getEntriesByType('navigation')
            ).map(e => ({
                url: e.name,
                type: e.initiatorType || e.entryType,
                duration_ms: Math.round(e.duration),
                transfer_size: e.transferSize || 0,
                start_ms: Math.round(e.startTime)
            }))
        )"#;

        let raw_value = active.page.eval_js(js).await.map_err(mcp_err)?;
        let json_str = raw_value
            .as_str()
            .ok_or_else(|| mcp_err("performance.getEntries() returned non-string"))?;

        let entries: Vec<serde_json::Value> = serde_json::from_str(json_str)
            .map_err(|e| mcp_err(format!("parse performance entries: {e}")))?;

        // Build a NetworkCapture from JS entries for classification.
        let mut capture = network::NetworkCapture::new();
        for (i, entry) in entries.iter().enumerate() {
            let url = entry["url"].as_str().unwrap_or("").to_string();
            // performance entries don't carry HTTP method; default to GET.
            let method = "GET";
            capture.on_request(i.to_string(), url, method.to_string(), None);
        }

        let summary = capture.summary();
        let filter_kind = match p.filter.as_str() {
            "auth" => Some(network::RequestKind::Auth),
            "api" => Some(network::RequestKind::Api),
            "navigation" => Some(network::RequestKind::Navigation),
            "asset" => Some(network::RequestKind::Asset),
            _ => None,
        };

        let filtered: Vec<&network::CapturedRequest> = if let Some(kind) = filter_kind {
            capture
                .requests
                .values()
                .filter(|r| r.kind == kind)
                .collect()
        } else {
            capture.requests.values().collect()
        };

        let output = serde_json::json!({
            "summary": summary,
            "filter": p.filter,
            "count": filtered.len(),
            "requests": filtered.iter().map(|r| serde_json::json!({
                "url": r.url,
                "kind": r.kind,
                "method": r.method,
                "timestamp_ms": r.timestamp_ms,
            })).collect::<Vec<_>>(),
        });

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }
}
