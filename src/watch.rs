//! Watch system: persistent page monitoring with semantic diffing.
//!
//! Manages a background polling loop that extracts [`SemanticView`]s on an
//! interval, diffs them via [`observer::diff`], and stores change events in
//! a bounded ring buffer. MCP resource notifications are pushed via the rmcp
//! `Peer` when a diff is non-empty.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use rmcp::model::ResourceUpdatedNotificationParam;
use rmcp::service::RoleServer;

use crate::observer::{self, SemanticDiff};
use crate::semantic::SemanticView;

/// Maximum events retained in the ring buffer.
const EVENT_CAP: usize = 1000;

// ── Event ────────────────────────────────────────────────────────────

/// A single watch event: a non-empty diff between two polling cycles.
#[derive(Debug, Clone, Serialize)]
pub struct WatchEvent {
    /// Monotonically increasing sequence number.
    pub seq: u64,
    /// Unix-epoch milliseconds when the diff was captured.
    pub timestamp_ms: u64,
    /// URL being watched.
    pub url: String,
    /// The semantic diff that triggered this event.
    pub diff: SemanticDiff,
}

// ── Shared ring buffer ───────────────────────────────────────────────

/// Thread-safe ring buffer of watch events, capped at [`EVENT_CAP`].
#[derive(Debug, Clone)]
pub struct EventBuffer {
    inner: Arc<Mutex<VecDeque<WatchEvent>>>,
    seq: Arc<AtomicU64>,
}

impl Default for EventBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBuffer {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(EVENT_CAP))),
            seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Push a new event, trimming the oldest if at capacity.
    ///
    /// FIX-4: URL is redacted to strip tokens/secrets from watch events.
    pub async fn push(&self, url: &str, diff: SemanticDiff) -> u64 {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let event = WatchEvent {
            seq,
            timestamp_ms: now_ms(),
            url: crate::sanitize::redact_url_secrets(url),
            diff,
        };
        let mut buf = self.inner.lock().await;
        if buf.len() >= EVENT_CAP {
            buf.pop_front();
        }
        buf.push_back(event);
        seq
    }

    /// Return all events with `seq > since_seq`.
    pub async fn events_since(&self, since_seq: Option<u64>) -> Vec<WatchEvent> {
        let buf = self.inner.lock().await;
        match since_seq {
            Some(cursor) => buf.iter().filter(|e| e.seq > cursor).cloned().collect(),
            None => buf.iter().cloned().collect(),
        }
    }

    /// Number of events currently stored.
    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// Whether the buffer is empty.
    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }

    /// Current sequence counter (next seq will be this value).
    pub fn current_seq(&self) -> u64 {
        self.seq.load(Ordering::SeqCst)
    }
}

// ── WatchState ───────────────────────────────────────────────────────

/// Holds the running state of a single watch: the background task handle,
/// the event buffer, and the URL being monitored.
pub struct WatchState {
    pub url: String,
    pub events: EventBuffer,
    task_handle: JoinHandle<()>,
}

impl WatchState {
    /// Stop the background polling task and return the event buffer.
    ///
    /// Clones the `EventBuffer` (cheap — it's `Arc`-backed) because the `Drop`
    /// impl also needs the `task_handle` to abort on implicit drop.
    pub fn stop(self) -> EventBuffer {
        // `Drop` will call `self.task_handle.abort()` automatically.
        self.events.clone()
    }

    /// Build a `watch://` URI for MCP resource notifications.
    pub fn resource_uri(&self) -> String {
        format!("watch://{}", sanitize_uri(&self.url))
    }
}

/// FIX-R3-08: Auto-abort the watch polling task on drop so it never leaks
/// when the server disconnects or the WatchState is replaced without
/// an explicit `stop()` call.
impl Drop for WatchState {
    fn drop(&mut self) {
        self.task_handle.abort();
    }
}

// ── Polling loop ─────────────────────────────────────────────────────

/// Configuration for starting a watch.
pub struct WatchConfig {
    pub url: String,
    pub interval_ms: u32,
    pub initial_view: SemanticView,
    /// Optional MCP peer for pushing resource-updated notifications.
    pub peer: Option<Arc<Mutex<Option<rmcp::Peer<RoleServer>>>>>,
}

/// Start a polling loop. Returns a `WatchState` whose `task_handle` runs
/// until aborted.
///
/// The `extract_fn` closure is called each tick to obtain the current
/// `SemanticView` from the live page. This keeps the watch module
/// decoupled from the engine/a11y layer.
///
/// When a `peer` is provided in the config, a `notifications/resources/updated`
/// notification is pushed after every non-empty diff.
pub fn start_watch<F, Fut>(cfg: WatchConfig, extract_fn: F) -> WatchState
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Option<SemanticView>> + Send,
{
    let events = EventBuffer::new();
    let events_clone = events.clone();
    let url = cfg.url.clone();
    let url_for_task = cfg.url.clone();
    let peer_arc = cfg.peer.clone();

    let task_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(cfg.interval_ms as u64));
        let mut last_view = cfg.initial_view;

        // Skip the first tick (fires immediately) — we already have initial_view.
        interval.tick().await;

        loop {
            interval.tick().await;

            let current_view = match extract_fn().await {
                Some(v) => v,
                None => {
                    tracing::warn!(url = %url_for_task, "watch: failed to extract view, skipping tick");
                    continue;
                }
            };

            let diff = observer::diff(&last_view, &current_view);

            if !diff.added.is_empty()
                || !diff.removed.is_empty()
                || !diff.changed.is_empty()
                || !diff.notifications.is_empty()
            {
                let seq = events_clone.push(&url_for_task, diff).await;
                tracing::debug!(url = %url_for_task, seq, "watch: diff captured");

                // Push MCP resource notification if peer is available.
                if let Some(ref peer_mutex) = peer_arc
                    && let Some(ref peer) = *peer_mutex.lock().await
                {
                    let uri = format!("watch://{}", sanitize_uri(&url_for_task));
                    let _ = peer
                        .notify_resource_updated(ResourceUpdatedNotificationParam::new(uri))
                        .await;
                }
            }

            last_view = current_view;
        }
    });

    WatchState {
        url,
        events,
        task_handle,
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Strip scheme and non-alphanumeric chars for use in URIs.
fn sanitize_uri(url: &str) -> String {
    url.trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_owned()
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{Element, ElementKind, PageState};

    fn make_view(elements: Vec<Element>) -> SemanticView {
        SemanticView {
            url: "http://test".into(),
            title: "Test".into(),
            page_hint: "".into(),
            elements,
            forms: vec![],
            visible_text: "".into(),
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        }
    }

    fn make_el(id: u32, label: &str, val: Option<&str>) -> Element {
        Element {
            id,
            kind: ElementKind::Input,
            label: label.into(),
            name: None,
            value: val.map(|s| s.to_string()),
            placeholder: None,
            href: None,
            input_type: None,
            disabled: false,
            form_index: None,
            context: None,
            hint: None,
            frame_index: None,
        }
    }

    #[tokio::test]
    async fn event_buffer_basic_push_and_read() {
        let buf = EventBuffer::new();
        let diff = observer::diff(
            &make_view(vec![make_el(1, "Email", None)]),
            &make_view(vec![make_el(1, "Email", Some("a@b.com"))]),
        );
        assert_eq!(diff.changed.len(), 1);

        buf.push("http://test", diff).await;
        assert_eq!(buf.len().await, 1);

        let events = buf.events_since(None).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, 0);
        // URL normalized by redact_url_secrets (adds trailing slash).
        assert_eq!(events[0].url, "http://test/");
    }

    #[tokio::test]
    async fn event_buffer_since_cursor() {
        let buf = EventBuffer::new();
        let diff = observer::diff(
            &make_view(vec![]),
            &make_view(vec![make_el(1, "Btn", None)]),
        );

        buf.push("http://test", diff.clone()).await; // seq=0
        buf.push("http://test", diff.clone()).await; // seq=1
        buf.push("http://test", diff).await; // seq=2

        let events = buf.events_since(Some(0)).await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[1].seq, 2);
    }

    #[tokio::test]
    async fn event_buffer_overflow_trims() {
        let buf = EventBuffer::new();
        let diff = observer::diff(&make_view(vec![]), &make_view(vec![make_el(1, "X", None)]));

        for _ in 0..1100 {
            buf.push("http://test", diff.clone()).await;
        }

        assert_eq!(buf.len().await, EVENT_CAP);
        let events = buf.events_since(None).await;
        // Oldest surviving seq should be 100 (1100 - 1000)
        assert_eq!(events.first().unwrap().seq, 100);
        assert_eq!(events.last().unwrap().seq, 1099);
    }

    #[tokio::test]
    async fn start_watch_captures_diffs() {
        let v1 = make_view(vec![make_el(1, "Email", None)]);
        let v2 = make_view(vec![make_el(1, "Email", Some("hello"))]);

        let call_count = Arc::new(AtomicU64::new(0));
        let cc = call_count.clone();
        let v2_clone = v2.clone();

        let state = start_watch(
            WatchConfig {
                url: "http://test".into(),
                interval_ms: 10,
                initial_view: v1,
                peer: None,
            },
            move || {
                let c = cc.fetch_add(1, Ordering::SeqCst);
                let v = v2_clone.clone();
                async move {
                    if c == 0 {
                        Some(v)
                    } else {
                        // After first diff, return same view (no change)
                        Some(v)
                    }
                }
            },
        );

        // Wait for at least one polling cycle
        tokio::time::sleep(Duration::from_millis(50)).await;

        let events = state.events.events_since(None).await;
        // Should have at least 1 event (first diff: None->Some("hello"))
        assert!(!events.is_empty(), "expected at least 1 watch event");
        assert_eq!(events[0].diff.changed.len(), 1);

        // Stop and verify cleanup
        let _buf = state.stop();
    }

    #[tokio::test]
    async fn watch_state_resource_uri() {
        let v = make_view(vec![]);
        let state = start_watch(
            WatchConfig {
                url: "https://example.com/dashboard".into(),
                interval_ms: 1000,
                initial_view: v,
                peer: None,
            },
            || async { None },
        );

        assert_eq!(state.resource_uri(), "watch://example.com/dashboard");
        state.stop();
    }
}
