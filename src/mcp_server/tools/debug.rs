//! Debug/escape-hatch tools: `lad_eval`, `lad_network`, `lad_locate`.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{mcp_err, to_pretty_json};
use crate::params::{EvalParams, LocateParams, NetworkParams};

use llm_as_dom::{locate, network};

/// FIX-13: Compute SHA256 hash prefix for audit trail logging.
/// Logs a hash instead of the content to prevent secrets from appearing in logs.
fn sha256_prefix(data: &str) -> String {
    use sha2::{Digest, Sha256};
    // sha2 0.11 — `Output` (hybrid_array::Array) no longer implements `LowerHex`.
    // Hex-encode by hand to keep behaviour identical.
    let hash = Sha256::digest(data.as_bytes());
    let mut out = String::with_capacity(16);
    for b in hash.iter().take(8) {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

impl LadServer {
    /// Evaluate arbitrary JavaScript on the active page.
    ///
    /// FIX-R3-07: Gated behind `LAD_ALLOW_EVAL=true|1`. Returns an error
    /// when the env var is absent or any other value, preventing accidental
    /// arbitrary JS execution in production.
    pub(crate) async fn tool_lad_eval(
        &self,
        params: Parameters<EvalParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        // FIX-R3-07: Environment gate — reject if not explicitly enabled.
        let eval_allowed = std::env::var("LAD_ALLOW_EVAL")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        if !eval_allowed {
            return Err(mcp_err(
                "lad_eval disabled — set LAD_ALLOW_EVAL=true to enable arbitrary JS execution",
            ));
        }

        let p = params.0;
        // FIX-13: Log SHA256 hash of the script for audit trail, NOT the content.
        // Prevents secrets from leaking into tracing output.
        let script_hash = sha256_prefix(&p.script);
        tracing::info!(script_hash = %script_hash, len = p.script.len(), tab_id = ?p.tab_id, "lad_eval");
        tracing::warn!(script_hash = %script_hash, len = p.script.len(), "lad_eval: arbitrary JS execution");

        let guard = self.lock_active_page().await;
        let ap = guard.resolve(p.tab_id)?;
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
        tracing::info!(filter = %p.filter, tab_id = ?p.tab_id, "lad_network");

        let guard = self.lock_active_page().await;
        let active = guard.resolve(p.tab_id)?;

        // DX-W3-7: Extract initiatorType for method heuristic, and responseStatus
        // (available on Chrome 109+ via PerformanceResourceTiming).
        let js = r#"JSON.stringify(
            performance.getEntriesByType('resource').concat(
                performance.getEntriesByType('navigation')
            ).map(e => ({
                url: e.name,
                type: e.initiatorType || e.entryType,
                duration_ms: Math.round(e.duration),
                transfer_size: e.transferSize || 0,
                start_ms: Math.round(e.startTime),
                response_status: e.responseStatus || 0
            }))
        )"#;

        let raw_value = active.page.eval_js(js).await.map_err(mcp_err)?;
        let json_str = raw_value
            .as_str()
            .ok_or_else(|| mcp_err("performance.getEntries() returned non-string"))?;

        let entries: Vec<serde_json::Value> = serde_json::from_str(json_str)
            .map_err(|e| mcp_err(format!("parse performance entries: {e}")))?;

        // Build a NetworkCapture from JS entries for classification.
        // DX-W3-7: Map initiatorType to HTTP method heuristic.
        let mut capture = network::NetworkCapture::new();
        for (i, entry) in entries.iter().enumerate() {
            let url = entry["url"].as_str().unwrap_or("").to_string();
            let initiator = entry["type"].as_str().unwrap_or("");
            // Heuristic: xmlhttprequest/fetch/beacon often use POST for APIs,
            // but we can't know for sure from perf entries alone. Default GET.
            let method = match initiator {
                "beacon" => "POST",
                "navigation" => "GET",
                _ => "GET",
            };
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

        // Wave 5 (Pain #17): build the serialized request list first so we
        // can detect placeholder status codes (status=0) and attach a
        // top-level `note` that explains the cross-origin limitation.
        // TODO(wave6): switch to CDP Network domain for real status codes
        // and byte counts — `performance.getEntries()` cannot read HTTP
        // metadata for cross-origin responses.
        let request_objects: Vec<serde_json::Value> = filtered
            .iter()
            .map(|r| {
                // Match by URL to correlate with performance entries (HashMap has no order)
                let entry = entries
                    .iter()
                    .find(|e| e["name"].as_str().is_some_and(|name| name == r.url));
                let status = entry
                    .and_then(|e| e["responseStatus"].as_u64())
                    .unwrap_or(r.status as u64);
                let initiator = entry
                    .and_then(|e| e["initiatorType"].as_str())
                    .unwrap_or("unknown");
                serde_json::json!({
                    "url": llm_as_dom::sanitize::redact_url_secrets(&r.url),
                    "kind": r.kind,
                    "method": r.method,
                    "status": status,
                    "initiator": initiator,
                    "timestamp_ms": r.timestamp_ms,
                })
            })
            .collect();

        // Attach the cross-origin caveat note when any entry has status=0,
        // which is the tell-tale for placeholder values. Additive, only
        // present when relevant — zero-impact on fully same-origin pages.
        let has_placeholder_status = request_objects
            .iter()
            .any(|r| r.get("status").and_then(|s| s.as_u64()) == Some(0));

        let mut output = serde_json::json!({
            "summary": summary,
            "filter": p.filter,
            "count": request_objects.len(),
            "requests": request_objects,
        });
        if has_placeholder_status && let Some(obj) = output.as_object_mut() {
            obj.insert(
                "note".to_string(),
                serde_json::Value::String(
                    "status=0 and total_bytes=0 are placeholder values — \
                     performance.getEntries() cannot read cross-origin HTTP \
                     metadata. See README."
                        .to_string(),
                ),
            );
        }

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }
}
