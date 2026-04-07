//! `lad_browse` tool — autonomous goal-based browsing.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;

use crate::LadServer;
use crate::helpers::{mcp_err, to_pretty_json};
use crate::params::BrowseParams;

use llm_as_dom::{a11y, pilot};

impl LadServer {
    /// Browse a URL and accomplish a goal autonomously.
    /// The pilot uses heuristics + cheap LLM to navigate, fill forms, click buttons.
    /// Returns structured result: success/failure, steps taken, timing.
    pub(crate) async fn tool_lad_browse(
        &self,
        params: Parameters<BrowseParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let p = params.0;
        // FIX-13: Mask anything after "password" keyword in goal for logging.
        let log_goal = if let Some(idx) = p.goal.to_lowercase().find("password") {
            let boundary = p.goal.ceil_char_boundary(idx + "password".len());
            format!("{}[REDACTED]", &p.goal[..boundary])
        } else {
            p.goal.clone()
        };
        tracing::info!(url = %p.url, goal = %log_goal, "lad_browse");

        tracing::info!(url = %p.url, "launching page");
        let engine = self.ensure_engine().await.map_err(mcp_err)?;
        let page = engine.new_page(&p.url).await.map_err(mcp_err)?;
        tracing::info!("waiting for navigation");
        page.wait_for_navigation().await.map_err(mcp_err)?;
        tracing::info!("waiting for content to stabilise");
        a11y::wait_for_content(page.as_ref(), a11y::DEFAULT_WAIT_TIMEOUT)
            .await
            .map_err(mcp_err)?;
        tracing::info!("page ready, initialising pilot");

        // Inject Chrome profile cookies if LAD_CHROME_PROFILE is set
        self.inject_profile_cookies(page.as_ref()).await;

        let backend = Self::create_backend(&self.llm_url, &self.llm_model, p.max_length);
        let config = pilot::PilotConfig {
            goal: p.goal.clone(),
            max_steps: p.max_steps,
            use_hints: true,
            use_heuristics: true,
            playbook_dir: None,
            max_retries_per_step: 2,
            session: None,
            interactive: self.interactive,
        };

        tracing::info!("running pilot");
        let result = pilot::run_pilot(page.as_ref(), backend.as_ref(), &config)
            .await
            .map_err(mcp_err)?;
        tracing::info!(
            success = result.success,
            steps = result.steps.len(),
            duration_secs = result.total_duration.as_secs_f64(),
            "pilot complete"
        );

        // Update session state
        {
            let mut session = self.session.lock().await;
            session.browse_count += 1;
            session.visited_urls.push(p.url.clone());
            if result.success {
                session.last_success_goal = Some(p.goal.clone());
                // Detect if login was the goal
                let goal_lower = p.goal.to_lowercase();
                if goal_lower.contains("login") || goal_lower.contains("sign in") {
                    session.authenticated = true;
                }
            }
        }

        // FIX-5: Persist the page and final view into active_page so follow-up
        // tools (click, type, eval, screenshot) work after lad_browse.
        {
            let final_view = a11y::extract_semantic_view(page.as_ref())
                .await
                .unwrap_or_else(|_| llm_as_dom::semantic::SemanticView {
                    url: p.url.clone(),
                    title: String::new(),
                    page_hint: String::new(),
                    elements: vec![],
                    forms: vec![],
                    visible_text: String::new(),
                    state: llm_as_dom::semantic::PageState::Ready,
                    element_cap: None,
                    blocked_reason: None,
                    session_context: None,
                });
            let mut active = self.active_page.lock().await;
            *active = Some(crate::state::ActivePage {
                page,
                url: p.url.clone(),
                view: final_view,
            });
        }

        // Always capture a final screenshot for visual verification.
        tracing::info!("capturing final screenshot");
        let active_guard = self.active_page.lock().await;
        let final_screenshot = if let Some(ap) = active_guard.as_ref() {
            pilot::take_screenshot(ap.page.as_ref()).await
        } else {
            None
        };
        drop(active_guard);

        let session_snapshot = {
            let session = self.session.lock().await;
            serde_json::json!({
                "authenticated": session.authenticated,
                "browse_count": session.browse_count,
                "visited_urls_count": session.visited_urls.len(),
            })
        };

        let output = serde_json::json!({
            "success": result.success,
            "steps": result.steps.len(),
            "heuristic_steps": result.heuristic_hits,
            "llm_steps": result.llm_hits,
            "duration_secs": result.total_duration.as_secs_f64(),
            "final_action": format!("{:?}", result.final_action),
            "session": session_snapshot,
            "actions": result.steps.iter().map(|s| {
                serde_json::json!({
                    "step": s.index,
                    "source": format!("{:?}", s.source),
                    "action": format!("{:?}", s.action),
                    "duration_ms": s.duration.as_millis() as u64,
                })
            }).collect::<Vec<_>>(),
        });

        let mut content_blocks: Vec<Content> = vec![Content::text(to_pretty_json(&output))];

        // Append in-flight screenshots (e.g. from escalation retries).
        for b64_png in &result.screenshots {
            content_blocks.push(Content::image(b64_png, "image/png"));
        }

        // Append final screenshot (success or fail).
        if let Some(b64_png) = &final_screenshot {
            content_blocks.push(Content::image(b64_png, "image/png"));
        }

        Ok(CallToolResult::success(content_blocks))
    }
}
