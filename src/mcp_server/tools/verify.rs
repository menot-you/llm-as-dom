//! `lad_assert`, `lad_audit`, `lad_wait` tools.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::assertions::check_assertion;
use crate::helpers::{mcp_err, to_pretty_json};
use crate::params::{AssertParams, AuditParams, WaitParams};

use llm_as_dom::audit;

impl LadServer {
    /// Assert conditions about a web page and return pass/fail results.
    pub(crate) async fn tool_lad_assert(
        &self,
        params: Parameters<AssertParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(url = %p.url, assertions = ?p.assertions, "lad_assert");

        let (_page, view) = self.navigate_and_extract(&p.url).await?;
        let prompt_text = view.to_prompt();

        let mut results = Vec::new();
        for assertion in &p.assertions {
            let pass = check_assertion(&assertion.to_lowercase(), &view, &prompt_text);
            results.push(serde_json::json!({
                "assertion": assertion,
                "pass": pass,
            }));
        }

        let all_pass = results.iter().all(|r| r["pass"].as_bool().unwrap_or(false));

        let output = serde_json::json!({
            "url": llm_as_dom::sanitize::redact_url_secrets(&view.url),
            "title": view.title,
            "all_pass": all_pass,
            "results": results,
        });

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }

    /// Audit a web page for accessibility, forms, and links issues.
    pub(crate) async fn tool_lad_audit(
        &self,
        params: Parameters<AuditParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(url = %p.url, categories = ?p.categories, "lad_audit");

        let (page, _view) = self.navigate_and_extract(&p.url).await?;
        let js = audit::build_audit_js(&p.categories);
        let raw_value = page.eval_js(&js).await.map_err(mcp_err)?;

        let raw: Vec<audit::RawAuditIssue> = serde_json::from_value(raw_value)
            .map_err(|e| mcp_err(format!("audit JS parse failed: {e:?}")))?;

        // FIX-5: Redact URL secrets from audit result.
        let safe_url = llm_as_dom::sanitize::redact_url_secrets(&p.url);
        let audit_result = audit::parse_audit_result(&safe_url, raw);
        let output = serde_json::to_value(&audit_result)
            .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));

        Ok(CallToolResult::success(vec![Content::text(
            to_pretty_json(&output),
        )]))
    }

    /// Wait for a condition to be true on the active page.
    pub(crate) async fn tool_lad_wait(
        &self,
        params: Parameters<WaitParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(condition = %p.condition, timeout_ms = p.timeout_ms, "lad_wait");

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(p.timeout_ms);
        let poll_dur = std::time::Duration::from_millis(p.poll_ms);
        let cond_lower = p.condition.to_lowercase();

        loop {
            let view = self.refresh_active_view().await?;
            let prompt_text = view.to_prompt();
            if check_assertion(&cond_lower, &view, &prompt_text) {
                return Ok(CallToolResult::success(vec![Content::text(
                    view.to_prompt(),
                )]));
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(rmcp::ErrorData::internal_error(
                    format!(
                        "timeout after {}ms waiting for condition: {}",
                        p.timeout_ms, p.condition
                    ),
                    None,
                ));
            }

            tokio::time::sleep(poll_dur).await;
        }
    }
}
