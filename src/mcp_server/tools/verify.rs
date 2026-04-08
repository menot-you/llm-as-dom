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
    ///
    /// DX-W2-1: `url` is now optional. When omitted, asserts against the
    /// current active page without navigating — preserving session state.
    pub(crate) async fn tool_lad_assert(
        &self,
        params: Parameters<AssertParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        tracing::info!(url = ?p.url, assertions = ?p.assertions, "lad_assert");

        let view = if let Some(ref url) = p.url {
            let (_page, view) = self.navigate_and_extract(url).await?;
            view
        } else {
            self.refresh_active_view().await.map_err(|_| {
                rmcp::ErrorData::invalid_params(
                    "no active page — provide a URL or call lad_browse/lad_snapshot first"
                        .to_string(),
                    None,
                )
            })?
        };
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

        let safe_url = llm_as_dom::sanitize::redact_url_secrets(&view.url);
        let output = serde_json::json!({
            "url": safe_url,
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

    /// Wait for condition(s) to be true on the active page.
    ///
    /// DX-W3-1: Supports multiple conditions via `conditions` + `mode`.
    /// - mode="all" (default): wait until ALL conditions pass.
    /// - mode="any": return as soon as ANY condition passes.
    ///
    /// Backward compat: `condition` (singular) works as a single-element list.
    pub(crate) async fn tool_lad_wait(
        &self,
        params: Parameters<WaitParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;

        // Build the final conditions list from singular + plural params.
        let mut all_conditions: Vec<String> = Vec::new();
        if let Some(c) = &p.condition {
            all_conditions.push(c.clone());
        }
        if let Some(cs) = &p.conditions {
            all_conditions.extend(cs.iter().cloned());
        }
        if all_conditions.is_empty() {
            return Err(rmcp::ErrorData::invalid_params(
                "provide at least one condition via 'condition' or 'conditions'".to_string(),
                None,
            ));
        }

        let mode = p.mode.as_deref().unwrap_or("all");
        let is_any = mode == "any";

        tracing::info!(
            conditions = ?all_conditions,
            mode = mode,
            timeout_ms = p.timeout_ms,
            "lad_wait"
        );

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(p.timeout_ms);
        let poll_dur = std::time::Duration::from_millis(p.poll_ms);
        let conditions_lower: Vec<String> =
            all_conditions.iter().map(|c| c.to_lowercase()).collect();

        loop {
            let view = self.refresh_active_view().await?;
            let prompt_text = view.to_prompt();

            if is_any {
                // mode="any": return on first matching condition.
                if let Some(matched) = conditions_lower
                    .iter()
                    .zip(all_conditions.iter())
                    .find(|(cl, _)| check_assertion(cl, &view, &prompt_text))
                    .map(|(_, orig)| orig.clone())
                {
                    return Ok(CallToolResult::success(vec![Content::text(format!(
                        "MATCHED: {matched}\n\n{}",
                        view.to_prompt()
                    ))]));
                }
            } else {
                // mode="all": all conditions must pass.
                let all_pass = conditions_lower
                    .iter()
                    .all(|cl| check_assertion(cl, &view, &prompt_text));
                if all_pass {
                    return Ok(CallToolResult::success(vec![Content::text(
                        view.to_prompt(),
                    )]));
                }
            }

            if tokio::time::Instant::now() >= deadline {
                // Report which conditions failed for debugging.
                let failed: Vec<&str> = conditions_lower
                    .iter()
                    .zip(all_conditions.iter())
                    .filter(|(cl, _)| !check_assertion(cl, &view, &prompt_text))
                    .map(|(_, orig)| orig.as_str())
                    .collect();
                return Err(rmcp::ErrorData::internal_error(
                    format!(
                        "timeout after {}ms — failed conditions: {:?}",
                        p.timeout_ms, failed
                    ),
                    None,
                ));
            }

            tokio::time::sleep(poll_dur).await;
        }
    }
}
