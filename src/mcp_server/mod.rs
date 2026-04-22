//! `llm-as-dom-mcp`: MCP server exposing the browser pilot as semantic tools.
//!
//! Wave 2: multi-tab support. The server now holds a `HashMap<u32, ActivePage>`
//! keyed by stable tab IDs instead of a single active page. Every existing tool
//! accepts an optional `tab_id` parameter (defaulting to the active tab), and
//! three new tools (`lad_tabs_list`, `lad_tabs_switch`, `lad_tabs_close`) expose
//! tab management to callers. Tool shapes match Opera Neon's MCP Connector for
//! drop-in compatibility.
//!
//! Provides 28 tools: `lad_browse`, `lad_extract`, `lad_assert`, `lad_locate`,
//! `lad_audit`, `lad_session`, `lad_snapshot`, `lad_click`, `lad_type`, `lad_select`,
//! `lad_eval`, `lad_close`, `lad_press_key`, `lad_back`, `lad_screenshot`,
//! `lad_wait`, `lad_network`, `lad_hover`, `lad_dialog`, `lad_upload`, `lad_scroll`,
//! `lad_fill_form`, `lad_refresh`, `lad_clear`, `lad_watch`, `lad_jq`,
//! `lad_tabs_list`, `lad_tabs_switch`, `lad_tabs_close`.

mod assertions;
mod helpers;
mod params;
mod state;
mod tools;

use helpers::{
    mcp_err, no_active_page, parse_window_size_env, read_env_with_fallback, same_origin,
};
use params::*;
use state::{ActivePage, McpSessionState};

use llm_as_dom::engine::chromium::ChromiumEngine;
use llm_as_dom::engine::webkit::WebKitEngine;
use llm_as_dom::engine::{BrowserEngine, EngineConfig, PageHandle};
use llm_as_dom::{a11y, backend, pilot, semantic, watch};

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::{Mutex, MutexGuard};

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::service::{RequestContext, ServiceExt};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};

// ── Server state ───────────────────────────────────────────────────

// FIX-R3-03 (Wave 2 update): Lock ordering contract — to prevent deadlocks
// when multiple tools execute concurrently, always acquire locks in this order:
//
//   1. engine
//   2. tabs                  ← Wave 2: was `active_page`
//   3. active_tab_id         ← Wave 2: new
//   4. session
//   5. watch_state
//   6. peer
//
// Never hold a higher-numbered lock while acquiring a lower-numbered one.
// `ActivePageGuard` (below) acquires `tabs` and `active_tab_id` in that order
// so every callsite that routes through the guard is automatically safe.

/// MCP server that manages a headless browser and exposes pilot tools.
///
/// `Clone` is implemented manually rather than derived because `AtomicU32`
/// is not `Clone`. We snapshot the current value on clone — callers that
/// need to share state across clones should use the original `Arc` fields.
#[allow(dead_code)] // tool_router is used internally by rmcp macros
struct LadServer {
    /// Router that dispatches MCP tool calls to handler methods.
    tool_router: ToolRouter<Self>,
    /// Shared browser engine (lazy-initialised on first tool call).
    pub(crate) engine: Arc<Mutex<Option<Arc<dyn BrowserEngine>>>>,
    /// LLM API base URL (Ollama, Z.AI, or any compatible endpoint).
    pub(crate) llm_url: String,
    /// LLM model name.
    pub(crate) llm_model: String,
    /// Session state carried across tool calls within this MCP session.
    pub(crate) session: Arc<Mutex<McpSessionState>>,
    /// Whether interactive mode is enabled (captcha pause for human).
    ///
    /// Stored as `AtomicBool` so `ensure_engine_visible` can toggle it at
    /// runtime without a `&mut self` reference — replacing a prior unsafe
    /// `*const Self → *mut Self` cast that was UB under Rust's aliasing
    /// rules. All reads use `Ordering::Acquire`, writes use `Ordering::Release`.
    pub(crate) interactive: std::sync::atomic::AtomicBool,
    /// Wave 2: all open tabs keyed by stable ID. Replaces the Wave 1
    /// `Option<ActivePage>` single-slot. Lock ordering: always before
    /// `active_tab_id`.
    pub(crate) tabs: Arc<Mutex<HashMap<u32, ActivePage>>>,
    /// Wave 2: ID of the currently focused tab (the "active" one).
    /// `None` when no tab is open. Lock ordering: always after `tabs`.
    pub(crate) active_tab_id: Arc<Mutex<Option<u32>>>,
    /// Wave 2: monotonic tab-ID allocator. Starts at 1 (0 reserved as a
    /// sentinel for tests and optional future use). Stored as an atomic so
    /// `insert_tab` can allocate without holding the `tabs` lock yet.
    pub(crate) next_tab_id: Arc<AtomicU32>,
    /// Active watch state (at most one watch at a time).
    ///
    /// Wave 2: watch is still single-instance, not per-tab. If multi-tab
    /// watch is needed in the future, upgrade this to `HashMap<u32, WatchState>`
    /// keyed by `tab_id`.
    pub(crate) watch_state: Arc<Mutex<Option<watch::WatchState>>>,
    /// MCP peer for server-initiated push notifications.
    pub(crate) peer: Arc<Mutex<Option<rmcp::Peer<rmcp::service::RoleServer>>>>,
}

impl Clone for LadServer {
    fn clone(&self) -> Self {
        Self {
            tool_router: self.tool_router.clone(),
            engine: Arc::clone(&self.engine),
            llm_url: self.llm_url.clone(),
            llm_model: self.llm_model.clone(),
            session: Arc::clone(&self.session),
            interactive: std::sync::atomic::AtomicBool::new(
                self.interactive.load(Ordering::Acquire),
            ),
            tabs: Arc::clone(&self.tabs),
            active_tab_id: Arc::clone(&self.active_tab_id),
            next_tab_id: Arc::clone(&self.next_tab_id),
            watch_state: Arc::clone(&self.watch_state),
            peer: Arc::clone(&self.peer),
        }
    }
}

/// Wave 2: guard that holds both the `tabs` and `active_tab_id` mutexes in
/// the documented lock order, exposing a near-1:1 ergonomic replacement for
/// the old `MutexGuard<Option<ActivePage>>` API.
///
/// Most tool code uses one of four patterns:
///   - `guard.as_ref()`          — read-only access to the active `ActivePage`
///   - `guard.as_mut()`          — mutable access to the active `ActivePage`
///   - `guard.clear_active()`    — detach the active tab (SSRF invalidation)
///   - `guard.resolve(tab_id)?`  — explicit `tab_id` OR fall back to active
///
/// Constructing a new tab uses `LadServer::insert_tab`, which allocates an ID
/// via the atomic counter, inserts into the map, and sets it as active — all
/// under a single lock acquisition to avoid TOCTOU between allocate and insert.
pub(crate) struct ActivePageGuard<'a> {
    tabs: MutexGuard<'a, HashMap<u32, ActivePage>>,
    active_id: MutexGuard<'a, Option<u32>>,
}

impl<'a> ActivePageGuard<'a> {
    /// Immutable access to the active tab's page, or `None` if no active tab.
    /// Mirrors `Option::as_ref()` on the pre-Wave 2 `Option<ActivePage>`.
    pub(crate) fn as_ref(&self) -> Option<&ActivePage> {
        let id = (*self.active_id)?;
        self.tabs.get(&id)
    }

    /// Mutable access to the active tab's page, or `None` if no active tab.
    pub(crate) fn as_mut(&mut self) -> Option<&mut ActivePage> {
        let id = (*self.active_id)?;
        self.tabs.get_mut(&id)
    }

    /// Resolve by explicit `tab_id` (if provided) or fall back to the active
    /// tab. Returns an MCP invalid-params error when the resolved id is
    /// missing from the map. Used by every tool migrated to accept
    /// `tab_id: Option<u32>`.
    pub(crate) fn resolve(&self, explicit: Option<u32>) -> Result<&ActivePage, rmcp::ErrorData> {
        let id = match explicit {
            Some(id) => id,
            None => self.active_id.ok_or_else(no_active_page)?,
        };
        self.tabs
            .get(&id)
            .ok_or_else(|| mcp_err(format!("tab_id {id} not found")))
    }

    /// Mutable resolve — same semantics as [`Self::resolve`].
    pub(crate) fn resolve_mut(
        &mut self,
        explicit: Option<u32>,
    ) -> Result<&mut ActivePage, rmcp::ErrorData> {
        let id = match explicit {
            Some(id) => id,
            None => self.active_id.ok_or_else(no_active_page)?,
        };
        self.tabs
            .get_mut(&id)
            .ok_or_else(|| mcp_err(format!("tab_id {id} not found")))
    }

    /// Clear the active tab slot AND remove its `ActivePage` from the tabs
    /// map. This mirrors the Wave 1 behaviour of `*active_page = None` on
    /// SSRF invalidation: the page is dropped so subsequent tools cannot
    /// operate on a known-unsafe frame.
    pub(crate) fn clear_active(&mut self) {
        if let Some(id) = self.active_id.take() {
            self.tabs.remove(&id);
        }
    }

    /// Currently active tab id (if any).
    pub(crate) fn active_id(&self) -> Option<u32> {
        *self.active_id
    }

    /// Insert an `ActivePage` at `id` and mark it as the active tab.
    ///
    /// This is ADDITIVE: any previously-open tabs stay in the map. The old
    /// Wave 1 behaviour (overwrite the single slot) does not apply once we
    /// have multi-tab semantics; `lad_tabs_close` is the canonical path to
    /// remove a tab.
    pub(crate) fn set_active_page(&mut self, id: u32, ap: ActivePage) {
        self.tabs.insert(id, ap);
        *self.active_id = Some(id);
    }
}

impl LadServer {
    /// Wave 3: constructor exposed for child-module unit tests (e.g.
    /// `tools::lifecycle::tests`). Forwards to `new()` — no test-only
    /// state differs.
    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        Self::new()
    }

    /// Create a new server reading config from environment variables.
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            engine: Arc::new(Mutex::new(None)),
            llm_url: read_env_with_fallback(
                "LAD_LLM_URL",
                "LAD_OLLAMA_URL",
                "http://localhost:11434",
            ),
            llm_model: read_env_with_fallback("LAD_LLM_MODEL", "LAD_MODEL", "qwen2.5:7b"),
            session: Arc::new(Mutex::new(McpSessionState::default())),
            interactive: std::sync::atomic::AtomicBool::new(
                std::env::var("LAD_INTERACTIVE")
                    .map(|v| v == "true" || v == "1")
                    .unwrap_or(false),
            ),
            tabs: Arc::new(Mutex::new(HashMap::new())),
            active_tab_id: Arc::new(Mutex::new(None)),
            // Start at 1 so 0 is reserved as a future-proof sentinel. Simpler
            // than the skip-zero gymnastics the Wave 2 design sketch suggested.
            next_tab_id: Arc::new(AtomicU32::new(1)),
            watch_state: Arc::new(Mutex::new(None)),
            peer: Arc::new(Mutex::new(None)),
        }
    }

    /// Wave 2: acquire the `ActivePageGuard`. Always takes the `tabs` lock
    /// FIRST, then `active_tab_id` — matching the documented lock order so
    /// concurrent tools can never deadlock each other.
    pub(crate) async fn lock_active_page(&self) -> ActivePageGuard<'_> {
        let tabs = self.tabs.lock().await;
        let active_id = self.active_tab_id.lock().await;
        ActivePageGuard { tabs, active_id }
    }

    /// Wave 2: allocate a fresh tab ID, insert the given `ActivePage` into
    /// the tabs map, and mark it as active. Returns the new ID.
    ///
    /// This is the single canonical path for creating a new tab — `lad_browse`
    /// and `navigate_or_reuse`'s fresh-navigation branch both route through
    /// here so no caller can accidentally skip the active-tab-id update.
    pub(crate) async fn insert_tab(&self, ap: ActivePage) -> u32 {
        let id = self.next_tab_id.fetch_add(1, Ordering::AcqRel);
        let mut guard = self.lock_active_page().await;
        guard.set_active_page(id, ap);
        id
    }

    /// Wave 2: clear every open tab and reset the allocator to 1. Called by
    /// `lad_close` (full browser shutdown). The caller is responsible for
    /// closing the underlying engine.
    pub(crate) async fn clear_all_tabs(&self) {
        let mut guard = self.lock_active_page().await;
        guard.tabs.clear();
        *guard.active_id = None;
        // Reset the allocator so a fresh session starts at id=1 again.
        self.next_tab_id.store(1, Ordering::Release);
    }

    /// Return an existing engine or launch a new one.
    ///
    /// - `None`: keep current visibility — NEVER restarts the engine.
    ///   Previous behaviour silently flipped to headless when the caller
    ///   omitted the param, which destroyed the active_page state and
    ///   produced the misleading "no active page" error on the next
    ///   snapshot call. Callers that don't care should pass `None`.
    /// - `Some(v)`: force visibility `v`. Only restarts if `v` differs
    ///   from the current mode.
    pub(crate) async fn ensure_engine_visible(
        &self,
        request_visible: Option<bool>,
    ) -> Result<Arc<dyn BrowserEngine>, llm_as_dom::Error> {
        let mut engine_lock = self.engine.lock().await;
        let current = self.interactive.load(Ordering::Acquire);
        // Determine target visibility: None means "keep current"; Some(v)
        // means "force v". Only restart when target differs from current
        // AND an engine is actually alive to destroy.
        let target = request_visible.unwrap_or(current);
        let need_restart = engine_lock.is_some() && target != current;
        if need_restart {
            tracing::info!(
                from = current,
                to = target,
                "visibility changed — restarting browser"
            );
            // Drop old engine + every open tab. The visibility toggle
            // destroys all browser state, so there is no meaningful way to
            // keep previously-open tabs alive across it.
            *engine_lock = None;
            drop(engine_lock);
            self.clear_all_tabs().await;
            // Re-acquire for ensure_engine_inner below.
            let mut new_lock = self.engine.lock().await;
            if target != current {
                self.interactive.store(target, Ordering::Release);
            }
            return self.ensure_engine_inner(&mut new_lock).await;
        }
        // Store the new mode atomically. Safe under the engine lock because
        // `ensure_engine_inner` below will read this fresh value to launch
        // the new browser.
        if target != current {
            self.interactive.store(target, Ordering::Release);
        }
        self.ensure_engine_inner(&mut engine_lock).await
    }

    /// Return an existing engine or launch a new one.
    pub(crate) async fn ensure_engine(&self) -> Result<Arc<dyn BrowserEngine>, llm_as_dom::Error> {
        let mut engine_lock = self.engine.lock().await;
        self.ensure_engine_inner(&mut engine_lock).await
    }

    async fn ensure_engine_inner(
        &self,
        engine_lock: &mut tokio::sync::MutexGuard<'_, Option<Arc<dyn BrowserEngine>>>,
    ) -> Result<Arc<dyn BrowserEngine>, llm_as_dom::Error> {
        if let Some(e) = engine_lock.as_ref() {
            return Ok(Arc::clone(e));
        }

        let interactive = self.interactive.load(Ordering::Acquire);
        let mode = if interactive {
            "interactive (visible)"
        } else {
            "headless"
        };
        tracing::info!(mode, "launching browser");

        // DX: Persistent user_data_dir so login sessions survive MCP reconnects.
        //
        // Resolution order:
        // 1. $LAD_USER_DATA_DIR — explicit override (absolute path)
        // 2. $LAD_EPHEMERAL=1 — ephemeral random tempdir (tests/CI/one-shot)
        // 3. default: $XDG_CACHE_HOME/lad/chrome-profile or
        //    ~/Library/Caches/lad/chrome-profile (macOS) or
        //    ~/.cache/lad/chrome-profile (Linux)
        //
        // Without persistence, every MCP reconnect = fresh Chromium profile =
        // user has to re-login to every site. FIX-R3-12's crypto-random
        // tempdir trade-off was per-run isolation, which hurts DX badly.
        // The default cache dir lives in the user's own home, so there is
        // no cross-tenant risk — same threat model as ~/.config or ~/.ssh.
        let (user_data_dir, td) = resolve_user_data_dir()?;

        let config = EngineConfig {
            visible: interactive,
            interactive,
            user_data_dir,
            temp_dir: td,
            // DX-5: Window size from LAD_WINDOW_SIZE env var ("WIDTHxHEIGHT"),
            // or defaults: 1440x900 visible, 1280x800 headless.
            window_size: parse_window_size_env().unwrap_or(if interactive {
                (1440, 900)
            } else {
                (1280, 800)
            }),
        };

        let engine_name = std::env::var("LAD_ENGINE").unwrap_or_default();
        let e: Arc<dyn BrowserEngine> = if engine_name == "webkit" {
            Arc::new(WebKitEngine::launch(config).await?)
        } else {
            Arc::new(ChromiumEngine::launch(config).await?)
        };
        **engine_lock = Some(Arc::clone(&e));
        Ok(e)
    }

    /// Navigate to a URL and return the page handle with its semantic view.
    pub(crate) async fn navigate_and_extract(
        &self,
        url: &str,
    ) -> Result<(Box<dyn PageHandle>, semantic::SemanticView), rmcp::ErrorData> {
        // FIX-4: SSRF gate — block file://, javascript:, data:, private IPs.
        if !llm_as_dom::sanitize::is_safe_url(url) {
            return Err(mcp_err(format!("blocked: unsafe URL '{url}'")));
        }
        let engine = self.ensure_engine().await.map_err(mcp_err)?;

        // FIX-5: Navigate to target URL FIRST, then inject cookies, then reload.
        // `about:blank` has null origin and cannot set cross-origin cookies via
        // `document.cookie`. We must be on the target origin for cookie injection.
        let page = engine.new_page(url).await.map_err(mcp_err)?;
        page.wait_for_navigation().await.map_err(mcp_err)?;

        // FIX-R4-01: Post-redirect SSRF validation. Check final URL after
        // the browser may have followed redirects through an open redirect.
        let final_url = page.url().await.map_err(mcp_err)?;
        if !llm_as_dom::sanitize::is_safe_url(&final_url) {
            return Err(mcp_err(format!(
                "blocked: redirected to unsafe URL {final_url}"
            )));
        }

        // Inject cookies on the correct origin, then reload to apply them.
        let has_cookies = self.has_profile_cookies();
        if has_cookies {
            self.inject_profile_cookies(page.as_ref()).await;
            page.navigate(&final_url).await.map_err(mcp_err)?;
            page.wait_for_navigation().await.map_err(mcp_err)?;

            let reloaded_url = page.url().await.map_err(mcp_err)?;
            if !llm_as_dom::sanitize::is_safe_url(&reloaded_url) {
                return Err(mcp_err(format!(
                    "blocked: redirected to unsafe URL {reloaded_url}"
                )));
            }
        }

        a11y::wait_for_content(page.as_ref(), a11y::DEFAULT_WAIT_TIMEOUT)
            .await
            .map_err(mcp_err)?;

        // DX-W3-4: Auto-install dialog overrides on every new page so unexpected
        // alert/confirm/prompt dialogs don't hang the page. Default: auto-accept.
        // `lad_dialog(action="dismiss")` can change the behavior at runtime.
        Self::inject_dialog_overrides(page.as_ref()).await;

        let view = a11y::extract_semantic_view(page.as_ref())
            .await
            .map_err(mcp_err)?;
        Ok((page, view))
    }

    /// DX-W3-4: Inject dialog auto-accept JS on a page.
    ///
    /// Overrides `window.alert`, `window.confirm`, `window.prompt` to
    /// auto-accept by default and capture dialog history. Idempotent.
    async fn inject_dialog_overrides(page: &dyn PageHandle) {
        let js = r#"
            if (!window.__lad_dialogs) {
                window.__lad_dialogs = [];
                window.__lad_dialog_auto = 'accept';
                window.__lad_dialog_response = '';

                window.alert = function(msg) {
                    window.__lad_dialogs.push({
                        type: 'alert', message: String(msg),
                        timestamp: Date.now()
                    });
                };
                window.confirm = function(msg) {
                    window.__lad_dialogs.push({
                        type: 'confirm', message: String(msg),
                        timestamp: Date.now()
                    });
                    return window.__lad_dialog_auto === 'accept';
                };
                window.prompt = function(msg, def) {
                    window.__lad_dialogs.push({
                        type: 'prompt', message: String(msg),
                        default: def || '', timestamp: Date.now()
                    });
                    if (window.__lad_dialog_auto !== 'accept') return null;
                    return window.__lad_dialog_response || def || '';
                };
            }
        "#;
        if let Err(e) = page.eval_js(js).await {
            tracing::warn!(error = %e, "failed to inject dialog overrides");
        }
    }

    /// Navigate to a URL (or reuse the active tab's page if same origin),
    /// returning a fresh semantic view. Updates the active tab in-place on the
    /// reuse path, or opens a fresh tab on the cross-origin path.
    ///
    /// FIX-R3-01: Eliminated TOCTOU race. Previously the lock was dropped and
    /// reacquired between the same-origin check and the write-back, allowing a
    /// concurrent call to mutate state. Now the guard is held for the reuse
    /// path and only released for the fresh-navigation path (which needs
    /// `navigate_and_extract` to acquire `engine` without nesting locks).
    ///
    /// Wave 2: operates on the *active* tab (the one resolved from
    /// `active_tab_id`). Multi-tab callers that want to navigate a specific
    /// non-active tab should call `ActivePageGuard::resolve_mut` directly.
    pub(crate) async fn navigate_or_reuse(
        &self,
        url: &str,
    ) -> Result<semantic::SemanticView, rmcp::ErrorData> {
        // FIX-4: SSRF gate — block file://, javascript:, data:, private IPs.
        if !llm_as_dom::sanitize::is_safe_url(url) {
            return Err(mcp_err(format!("blocked: unsafe URL '{url}'")));
        }
        let mut guard = self.lock_active_page().await;

        // Reuse existing page if same origin — hold the guard through the entire operation.
        if let Some(ap) = guard.as_mut()
            && same_origin(&ap.url, url)
        {
            if ap.url != url {
                ap.page.navigate(url).await.map_err(mcp_err)?;
                ap.page.wait_for_navigation().await.map_err(mcp_err)?;

                let final_url = ap.page.url().await.map_err(mcp_err)?;
                // FIX-R8-01: Invalidate active tab on SSRF detection.
                if !llm_as_dom::sanitize::is_safe_url(&final_url) {
                    guard.clear_active();
                    return Err(mcp_err(format!(
                        "blocked: redirected to unsafe URL {final_url}"
                    )));
                }

                a11y::wait_for_content(ap.page.as_ref(), a11y::DEFAULT_WAIT_TIMEOUT)
                    .await
                    .map_err(mcp_err)?;
            }
            let view = a11y::extract_semantic_view(ap.page.as_ref())
                .await
                .map_err(mcp_err)?;
            // FIX-R7-02: Store the ACTUAL browser URL, not the requested URL.
            // After redirects (e.g. http->https), `url` is stale. Using the
            // browser's real URL prevents same-origin misclassification on the
            // next call.
            let actual_url = ap.page.url().await.map_err(mcp_err)?;
            ap.url = actual_url;
            ap.view = view.clone();
            return Ok(view);
        }

        // Different origin or no active tab — must release the guard before calling
        // navigate_and_extract (which acquires the engine lock). Then reacquire once
        // to store the result. This is safe because we're creating a fresh page.
        drop(guard);
        let (page, view) = self.navigate_and_extract(url).await?;
        // FIX-R7-02: Store the ACTUAL browser URL after navigation + redirects.
        let actual_url = page.url().await.map_err(mcp_err)?;
        self.insert_tab(ActivePage {
            page,
            url: actual_url,
            view: view.clone(),
        })
        .await;
        Ok(view)
    }

    /// Re-extract semantic view from the active tab's page and update stored
    /// state.
    ///
    /// FIX-R6-02: Also syncs `ap.url` with the actual browser URL after every
    /// refresh. Without this, `ActivePage.url` could hold the *requested* URL
    /// while the browser had followed a redirect (e.g. http->https), causing
    /// `navigate_or_reuse` to misclassify same-origin pages and reopen them.
    ///
    /// FIX-R7-01: SSRF chokepoint — every tool calls `refresh_active_view` after
    /// every interaction. By checking the URL here, delayed navigations via
    /// `setTimeout(() => location = "http://127.0.0.1", 500)` are caught even
    /// if they slip past the per-tool SSRF checks (which only sample once after
    /// a short delay). This is the SINGLE defense-in-depth bottleneck.
    ///
    /// Wave 2: operates on the active tab by default. Pass a specific tab id
    /// via `refresh_view_for` if you need to refresh a non-active tab.
    pub(crate) async fn refresh_active_view(
        &self,
    ) -> Result<semantic::SemanticView, rmcp::ErrorData> {
        self.refresh_view_for(None).await
    }

    /// Wave 2: refresh the semantic view for either the active tab (when
    /// `tab_id` is `None`) or a specific tab. Every tool that wires up a
    /// `tab_id: Option<u32>` param forwards it here so the SSRF chokepoint is
    /// preserved per-tab.
    pub(crate) async fn refresh_view_for(
        &self,
        tab_id: Option<u32>,
    ) -> Result<semantic::SemanticView, rmcp::ErrorData> {
        let mut guard = self.lock_active_page().await;
        let ap = guard.resolve_mut(tab_id)?;

        // Sync URL with actual browser URL (handles redirects, click-driven navs)
        let current_url = ap.page.url().await.map_err(mcp_err)?;

        // FIX-R7-01: SSRF gate on EVERY refresh — catches delayed navigations
        // that evade per-tool checks (e.g. setTimeout-based location changes).
        // FIX-R8-01: Invalidate the active tab BEFORE returning the SSRF
        // error. Without this, subsequent tools (screenshot, eval) would
        // still operate on the unsafe page because it remained in the map.
        if !llm_as_dom::sanitize::is_safe_url(&current_url) {
            let redacted = llm_as_dom::sanitize::redact_url_secrets(&current_url);
            // If the caller gave us an explicit tab id, remove just that tab;
            // otherwise clear the active slot (same behaviour as Wave 1).
            match tab_id {
                Some(id) => {
                    guard.tabs.remove(&id);
                    if *guard.active_id == Some(id) {
                        *guard.active_id = None;
                    }
                }
                None => guard.clear_active(),
            }
            return Err(mcp_err(format!(
                "blocked: page navigated to unsafe URL {redacted}",
            )));
        }

        ap.url = current_url;

        let view = a11y::extract_semantic_view(ap.page.as_ref())
            .await
            .map_err(mcp_err)?;
        ap.view = view.clone();
        Ok(view)
    }

    /// FIX-5: Check if Chrome profile cookies are configured (non-async).
    pub(crate) fn has_profile_cookies(&self) -> bool {
        std::env::var("LAD_CHROME_PROFILE")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    /// Inject cookies from the user's Chrome profile if `LAD_CHROME_PROFILE` is set.
    pub(crate) async fn inject_profile_cookies(&self, page: &dyn PageHandle) {
        let profile_name = match std::env::var("LAD_CHROME_PROFILE") {
            Ok(name) if !name.is_empty() => name,
            _ => return,
        };

        let profile_path = match llm_as_dom::profile::resolve_profile_path(&profile_name) {
            Some(p) => p,
            None => {
                tracing::warn!(profile = %profile_name, "Chrome profile not found");
                return;
            }
        };

        match llm_as_dom::profile::extract_cookies_from_profile(&profile_path) {
            Ok(cookies) => {
                tracing::info!(count = cookies.len(), "injecting Chrome profile cookies");
                let _ = page.set_cookies(&cookies).await;
            }
            Err(e) => tracing::warn!(error = %e, "failed to load Chrome profile cookies"),
        }
    }

    /// FIX-9: Delegate to the canonical factory in `backend::create_backend`.
    pub(crate) fn create_backend(
        url: &str,
        model: &str,
        max_prompt_length: Option<usize>,
    ) -> Box<dyn pilot::PilotBackend> {
        backend::create_backend(url, model, max_prompt_length)
    }
}

// ── Tool router ──────────────────────────────────────────────────────

#[tool_router]
impl LadServer {
    #[tool(
        description = "Navigate to a URL and accomplish a browsing goal autonomously (login, fill form, click, search). Returns structured result."
    )]
    async fn lad_browse(
        &self,
        params: Parameters<BrowseParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_browse(params).await
    }

    #[tool(
        description = "Extract structured info from a page: interactive elements, text, page type. Never returns raw HTML. URL is optional — omit to extract from current page without navigating (preserves session state). `what` is a semantic filter: visible_text is rewritten to the top-K paragraphs/headings matching your query (not a hard 500-char banner). Pass `strict=true` to additionally drop elements whose label/name/placeholder/href don't match `what` — useful on inventory-heavy pages (GitHub, HN). Pass `limit=N` to cap returned elements (hard cap 200; applied BEFORE pagination); response includes `truncated: bool`, `limit_applied`, and `total_before_limit` so iterating callers can tell if the cap fired. When `strict=true` and `limit` is omitted, a leading numeral in `what` (e.g. \"top 5 story titles\", \"primeiras 3 histórias\") is parsed as an implicit limit. Pass paginate_index+page_size to slice elements; include_hidden=true to bypass the default hidden-element filter."
    )]
    async fn lad_extract(
        &self,
        params: Parameters<ExtractParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_extract(params).await
    }

    #[tool(
        description = "Run a jq query against the current page's semantic view JSON. Use this to extract specific fields (e.g. '.elements | map(select(.role == \"button\")) | .[].label') instead of pulling the whole snapshot. Saves tokens."
    )]
    async fn lad_jq(
        &self,
        params: Parameters<JqParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_jq(params).await
    }

    #[tool(
        description = "Check assertions on a page. Returns pass/fail for each. URL is optional — omit to assert against the current page without navigating (preserves session state). Supports: has login form, title contains X, has button Y, url contains Z."
    )]
    async fn lad_assert(
        &self,
        params: Parameters<AssertParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_assert(params).await
    }

    #[tool(
        description = "Map a DOM element back to its source file. Checks React dev source, data-ds, data-lad attributes. Returns source file/line or DOM path fallback."
    )]
    async fn lad_locate(
        &self,
        params: Parameters<LocateParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_locate(params).await
    }

    #[tool(
        description = "Audit a URL for quality issues: a11y (alt text, labels, lang), forms (autocomplete, minlength), links (void hrefs, noopener). Returns issues with severity and fix suggestions. Response always includes `audit_ephemeral: bool` (true = audited page was ephemeral and is no longer accessible) and `audit_tab: null | {tab_id, url}` (non-null only when `return_tab=true`). Pass `return_tab=true` if you need follow-up interaction on the audited page."
    )]
    async fn lad_audit(
        &self,
        params: Parameters<AuditParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_audit(params).await
    }

    #[tool(
        description = "View or reset MCP session state: auth status, visited URLs, browse count. Actions: 'get' or 'clear'."
    )]
    async fn lad_session(
        &self,
        params: Parameters<SessionParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_session(params).await
    }

    #[tool(
        description = "Watch page state over time. Actions: 'start' begins polling a URL at interval_ms, diffing semantic views each cycle. 'events' returns captured diffs (pass since_seq for cursor-based pagination). 'stop' ends the watch."
    )]
    async fn lad_watch(
        &self,
        params: Parameters<WatchParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_watch(params).await
    }

    #[tool(
        description = "Get a structured semantic snapshot of the current page. Returns elements with IDs that can be used with lad_click/lad_type. URL is optional — omit it to re-read the current page without navigating (avoids accidentally undoing clicks). Like Playwright's browser_snapshot but 10-60x fewer tokens. Wave 1: pass paginate_index+page_size to get a slice, and include_hidden=true to show DOM nodes hidden via display:none/aria-hidden (filtered by default to block injected prompts)."
    )]
    async fn lad_snapshot(
        &self,
        params: Parameters<SnapshotParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_snapshot(params).await
    }

    #[tool(
        description = "Click an element by its ID from lad_snapshot. Set wait_for_navigation=true to wait for page load after clicking (useful for links/submit buttons). Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_click(
        &self,
        params: Parameters<ClickParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_click(params).await
    }

    #[tool(
        description = "Type text into an element by its ID from lad_snapshot. Set press_enter=true to submit after typing (saves a lad_press_key call). Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_type(
        &self,
        params: Parameters<TypeParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_type(params).await
    }

    #[tool(
        description = "Select an option in a dropdown by element ID from lad_snapshot. Matches by visible label text first, falls back to value attribute. Set wait_for_navigation=true if the dropdown auto-submits. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_select(
        &self,
        params: Parameters<SelectParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_select(params).await
    }

    #[tool(
        description = "Evaluate arbitrary JavaScript on the active page. Requires LAD_ALLOW_EVAL=true env var. This is an escape hatch for when semantic tools (browse, click, type) cannot handle a specific interaction. Requires a prior lad_snapshot or lad_browse call. Returns the raw JS result."
    )]
    async fn lad_eval(
        &self,
        params: Parameters<EvalParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_eval(params).await
    }

    #[tool(
        description = "Close the browser and release all resources. Use this when done with browser automation to prevent resource leaks."
    )]
    async fn lad_close(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_close().await
    }

    #[tool(
        description = "Press a keyboard key on the active page. Optionally focus an element first by its ID from a prior snapshot. Common keys: Enter, Tab, Escape, ArrowDown, ArrowUp, Backspace, Delete, Space."
    )]
    async fn lad_press_key(
        &self,
        params: Parameters<PressKeyParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_press_key(params).await
    }

    #[tool(
        description = "Navigate back in browser history (equivalent to clicking the back button). Returns the semantic view of the previous page. Accepts `timeout_ms` (default 10000) — returns a clear error instead of hanging when the page has no history or chromium is stuck."
    )]
    async fn lad_back(
        &self,
        params: Parameters<BackParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_back(params).await
    }

    #[tool(
        description = "Take a screenshot of the active page. Returns a base64-encoded PNG image. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_screenshot(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_screenshot().await
    }

    #[tool(
        description = "Wait for condition(s) to be true on the active page. Supports single `condition` or multiple `conditions` with mode='any' (first match wins) or mode='all' (default, all must pass). Example: conditions=['has button Dashboard', 'text contains Invalid password'], mode='any'. Supported predicates include `title contains X` (title only), `url contains X` (URL only), `text contains X` / `page contains X` (UNION match — true if X appears in URL, title, visible body text, or rendered prompt), `has button|link|input|form|image|password|login`. Default timeout: 10s, poll interval: 500ms."
    )]
    async fn lad_wait(
        &self,
        params: Parameters<WaitParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_wait(params).await
    }

    #[tool(
        description = "Inspect network traffic captured during browsing. Includes timing data via Performance API. Note: status codes and byte counts are unavailable for cross-origin requests due to `performance.getEntries()` limitations. Future: CDP Network domain integration. Optionally filter by type: auth, api, navigation, asset, all."
    )]
    async fn lad_network(
        &self,
        params: Parameters<NetworkParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_network(params).await
    }

    #[tool(
        description = "Hover over an element by its ID from lad_snapshot. Triggers mouseenter, mouseover, and mousemove events. Useful for dropdown menus, tooltips, and hover states. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_hover(
        &self,
        params: Parameters<HoverParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_hover(params).await
    }

    #[tool(
        description = "Handle JavaScript dialogs (alert, confirm, prompt). Actions: 'accept' auto-accepts future dialogs (with optional text for prompt inputs), 'dismiss' auto-dismisses, 'status' returns captured dialog history. Call before triggering actions that may show dialogs."
    )]
    async fn lad_dialog(
        &self,
        params: Parameters<DialogParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_dialog(params).await
    }

    #[tool(
        description = "Upload file(s) to a file input element by its ID from lad_snapshot. Provide absolute file paths. Currently supported on Chromium engine only; WebKit will return an error. File inputs inside shadow DOM or iframes (including same-origin) are not supported for upload due to Chromium CDP limitations. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_upload(
        &self,
        params: Parameters<UploadParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_upload(params).await
    }

    #[tool(
        description = "Fill multiple form fields at once and optionally submit. Fields are matched by label, name, or placeholder (case-insensitive). Use for login forms, registration, checkout, etc. Example: fields={\"Email\":\"user@test.com\",\"Password\":\"secret\"}, submit=true. Empty `fields` is valid only when `submit=true` — submits a pre-filled form without further input. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_fill_form(
        &self,
        params: Parameters<FillFormParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_fill_form(params).await
    }

    #[tool(
        description = "Scroll the page or scroll to a specific element. Directions: down, up, bottom, top. Optionally scroll to an element by ID. Useful for lazy-loaded content and infinite scroll pages. Returns updated semantic view after scrolling. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_scroll(
        &self,
        params: Parameters<ScrollParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_scroll(params).await
    }

    #[tool(
        description = "Reload the current page. Useful after form submissions or when content needs refreshing. Returns updated semantic view. Accepts `timeout_ms` (default 10000) — returns a clear error instead of hanging when the reload never completes. Requires a prior lad_snapshot or lad_browse call."
    )]
    async fn lad_refresh(
        &self,
        params: Parameters<RefreshParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_refresh(params).await
    }

    #[tool(
        description = "Clear an input field by selecting all content and deleting. Works with React/Vue controlled components that ignore el.value=''. Requires element ID from a prior lad_snapshot."
    )]
    async fn lad_clear(
        &self,
        params: Parameters<ClearParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_clear(params).await
    }

    #[tool(
        description = "List all open browser tabs. Returns an array of tabs, each with tab_id, title, url, and an is_active flag. Tool shape matches Opera Neon's list-tabs for drop-in compatibility."
    )]
    async fn lad_tabs_list(
        &self,
        params: Parameters<TabsListParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_tabs_list(params).await
    }

    #[tool(
        description = "Switch the active tab to the given tab_id. Subsequent tool calls that do not pass an explicit tab_id will target this tab. Errors if the tab does not exist."
    )]
    async fn lad_tabs_switch(
        &self,
        params: Parameters<TabSwitchParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_tabs_switch(params).await
    }

    #[tool(
        description = "Close the tab with the given tab_id. If it was the active tab, active_tab_id is cleared. Errors if the tab does not exist."
    )]
    async fn lad_tabs_close(
        &self,
        params: Parameters<TabCloseParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.tool_lad_tabs_close(params).await
    }
}

// ── ServerHandler ──────────────────────────────────────────────────

#[tool_handler]
impl ServerHandler for LadServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_resources_subscribe()
                .build(),
        )
        .with_instructions("lad (LLM-as-DOM) is an AI browser pilot. It navigates web pages autonomously using heuristics + cheap LLM. Use lad_browse for goal-based navigation, lad_extract for page analysis (URL optional, format='prompt' for compact output; `what` is a semantic filter — visible_text becomes the top-K paragraphs matching your query, pass `strict=true` to also drop non-matching elements), lad_assert for verification (URL optional), lad_locate for source mapping, lad_audit for page quality checks, lad_session for session state inspection/reset, lad_snapshot for semantic page snapshots (URL optional), lad_click/lad_type/lad_select for element interaction, lad_clear to clear input fields (works with React/Vue controlled components), lad_fill_form to fill multiple fields + submit in one call, lad_scroll for scrolling, lad_hover for hover states, lad_screenshot for visual capture, lad_wait for blocking condition checks (supports multiple conditions with mode='any'/'all'), lad_network for traffic inspection (timing data via Performance API; note: status codes and byte counts unavailable for cross-origin requests — CDP Network domain deferred), lad_dialog for JS alert/confirm/prompt handling (auto-accepts by default), lad_refresh to reload the current page, lad_upload for file input uploads. Escape hatches: lad_eval for raw JS, lad_press_key for keyboard events, lad_back for history navigation, lad_close for cleanup.")
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<rmcp::service::RoleServer>,
    ) -> Result<InitializeResult, rmcp::ErrorData> {
        // Capture the peer so the watch polling loop can push notifications.
        *self.peer.lock().await = Some(context.peer.clone());
        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }
        Ok(self.get_info())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::service::RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        let guard = self.watch_state.lock().await;
        let resources = match guard.as_ref() {
            Some(ws) => {
                // FIX-4: Redact URL secrets from resource listing.
                let safe_url = llm_as_dom::sanitize::redact_url_secrets(&ws.url);
                let r = Resource {
                    raw: RawResource::new(ws.resource_uri(), format!("Watch: {}", safe_url))
                        .with_description("Live semantic diff stream from page watch")
                        .with_mime_type("application/json"),
                    annotations: None,
                };
                vec![r]
            }
            None => vec![],
        };
        Ok(ListResourcesResult {
            resources,
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<rmcp::service::RoleServer>,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        let guard = self.watch_state.lock().await;
        let ws = guard
            .as_ref()
            .ok_or_else(|| rmcp::ErrorData::resource_not_found("no active watch", None))?;

        if request.uri != ws.resource_uri() {
            return Err(rmcp::ErrorData::resource_not_found(
                format!("unknown resource: {}", request.uri),
                None,
            ));
        }

        let events = ws.events.events_since(None).await;
        let json = serde_json::to_string_pretty(&events).unwrap_or_default();
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(json, &request.uri).with_mime_type("application/json"),
        ]))
    }
}

// ── Main ───────────────────────────────────────────────────────────

/// Initialise Sentry if `SENTRY_DSN` is set and non-empty.
///
/// Returns `Some(ClientInitGuard)` when active (must be held until `main`
/// exits so the Drop impl flushes queued events) or `None` when the env
/// var is absent/empty — the entire SDK is a no-op in that case.
fn init_sentry() -> Option<sentry::ClientInitGuard> {
    let dsn = std::env::var("SENTRY_DSN").ok().filter(|s| !s.is_empty())?;
    let environment = std::env::var("SENTRY_ENVIRONMENT").unwrap_or_else(|_| "production".into());
    Some(sentry::init((
        dsn,
        sentry::ClientOptions {
            release: sentry::release_name!(),
            environment: Some(environment.into()),
            attach_stacktrace: true,
            ..Default::default()
        },
    )))
}

// Sentry MUST be initialised before ANY other setup so that panics raised
// during runtime bootstrap (tokio, tracing subscriber, rmcp handshake) are
// reported. If SENTRY_DSN is unset or empty the guard is a no-op and the
// binary behaves exactly as before.
//
// Post-incident hardening: added 2026-04-03 after an npm auth token leaked
// via a Playwright DOM snapshot. Runtime error tracking is now mandatory
// so future production issues are surfaced before they become incidents.
//
// Ops env-var contract:
//   SENTRY_DSN          — enables reporting when set to a non-empty string
//   SENTRY_ENVIRONMENT  — deployment tag (defaults to "production")
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _sentry_guard = init_sentry();

    // Layer the fmt subscriber with Sentry's tracing bridge so `tracing`
    // error events propagate to Sentry. Layering (not replacing) preserves
    // the existing stderr output format the MCP client relies on.
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "llm_as_dom=info".into()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .compact(),
        )
        .with(sentry::integrations::tracing::layer())
        .init();

    tracing::info!("llm-as-dom-mcp starting (stdio)");

    let server = LadServer::new();
    let transport = rmcp::transport::io::stdio();
    let service = server.serve(transport).await?;
    service.waiting().await?;

    Ok(())
}

/// Resolve the Chromium user-data directory with the following priority:
///
/// 1. `$LAD_USER_DATA_DIR` — explicit override. Created if it does not exist.
///    Fails loudly if create OR a writable pre-flight check fails — the user
///    set this env var expecting a specific path, so silent fallback would
///    mask misconfiguration.
/// 2. `$LAD_EPHEMERAL` set to `1`/`true` — random tempdir for tests, CI,
///    and one-shot invocations that want zero cross-run state. Returned
///    with a drop guard so the directory is cleaned up on engine drop.
/// 3. Default — `$XDG_CACHE_HOME/lad/chrome-profile`, or
///    `~/Library/Caches/lad/chrome-profile` on macOS, or
///    `~/.cache/lad/chrome-profile` elsewhere. Persistent across runs so
///    login sessions (auth_token, ct0, Google SSO, etc) survive restarts.
///    If the default cache location cannot be created or written to, falls
///    back to an ephemeral tempdir rather than a bare `/tmp` path (the
///    fallback is held by a drop guard so it is cleaned up on exit).
///
/// The directory is created with mode 0o700 on Unix to limit exposure.
fn resolve_user_data_dir() -> Result<
    (
        std::path::PathBuf,
        Option<std::sync::Arc<tempfile::TempDir>>,
    ),
    llm_as_dom::Error,
> {
    // 1. Explicit override via env — fail loudly on create/write failure.
    //    The user set this expecting it to take effect; silent fallback masks
    //    misconfiguration (e.g. pointing at a deleted volume).
    if let Ok(path) = std::env::var("LAD_USER_DATA_DIR")
        && !path.is_empty()
    {
        let pb = std::path::PathBuf::from(&path);
        std::fs::create_dir_all(&pb).map_err(|e| {
            llm_as_dom::Error::Browser(format!(
                "LAD_USER_DATA_DIR='{}' could not be created: {e}",
                pb.display()
            ))
        })?;
        preflight_writable(&pb).map_err(|e| {
            llm_as_dom::Error::Browser(format!(
                "LAD_USER_DATA_DIR='{}' is not writable: {e}",
                pb.display()
            ))
        })?;
        set_dir_mode_700(&pb);
        tracing::info!(path = %pb.display(), "user_data_dir: explicit");
        return Ok((pb, None));
    }

    // 2. Ephemeral opt-in for tests/CI.
    let ephemeral = std::env::var("LAD_EPHEMERAL")
        .ok()
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"));
    if ephemeral && let Ok(td) = tempfile::Builder::new().prefix("lad-chrome-").tempdir() {
        let pb = td.path().to_path_buf();
        tracing::info!(path = %pb.display(), "user_data_dir: ephemeral");
        return Ok((pb, Some(std::sync::Arc::new(td))));
    }
    // If ephemeral was requested but tempdir failed, fall through to default.

    // 3. Default persistent cache dir.
    let base = default_cache_dir();
    let pb = base.join("lad").join("chrome-profile");
    match std::fs::create_dir_all(&pb)
        .and_then(|_| preflight_writable(&pb).map_err(std::io::Error::other))
    {
        Ok(()) => {
            set_dir_mode_700(&pb);
            tracing::info!(path = %pb.display(), "user_data_dir: persistent");
            Ok((pb, None))
        }
        Err(e) => {
            tracing::warn!(
                path = %pb.display(),
                error = %e,
                "default user_data_dir unusable — falling back to ephemeral tempdir"
            );
            // Last-resort fallback: ALWAYS held by a drop guard so we never
            // leak a stray profile on disk. If tempdir itself fails there is
            // nothing sane to return, so surface the error.
            let td = tempfile::Builder::new()
                .prefix("lad-chrome-fallback-")
                .tempdir()
                .map_err(|e2| {
                    llm_as_dom::Error::Browser(format!(
                        "user_data_dir resolution failed: default={e}, fallback tempdir={e2}"
                    ))
                })?;
            let path = td.path().to_path_buf();
            Ok((path, Some(std::sync::Arc::new(td))))
        }
    }
}

/// Pre-flight: verify the directory is writable by creating and removing a
/// probe file. Catches deleted-volume, read-only mount, and permission
/// mismatches before we hand the path to Chromium (which would otherwise
/// hang or crash with a cryptic error).
fn preflight_writable(dir: &std::path::Path) -> Result<(), std::io::Error> {
    let probe = dir.join(".lad-writable-probe");
    std::fs::write(&probe, b"ok")?;
    std::fs::remove_file(&probe)?;
    Ok(())
}

/// Return the platform cache directory without an external crate.
fn default_cache_dir() -> std::path::PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return std::path::PathBuf::from(xdg);
    }
    let home = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    #[cfg(target_os = "macos")]
    {
        home.join("Library").join("Caches")
    }
    #[cfg(not(target_os = "macos"))]
    {
        home.join(".cache")
    }
}

/// Best-effort `chmod 0700` on the user-data dir. No-op on non-Unix.
fn set_dir_mode_700(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            let mut perm = metadata.permissions();
            perm.set_mode(0o700);
            let _ = std::fs::set_permissions(path, perm);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// SS-7: Tests extracted to `tests.rs` to keep mod.rs lean (~740 LOC -> ~740 LOC of tests).
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
