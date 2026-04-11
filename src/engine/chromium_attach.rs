//! Wave 3 — Chromium CDP attach mode.
//!
//! Connects to an already-running Chrome/Chromium/Brave/Edge/Opera
//! instance via its remote debugging port instead of spawning a fresh
//! headless process. Lets LAD operate inside the user's real
//! authenticated browser session — cookies, logins, extensions, VPN —
//! without installing any Opera Neon dependency or running a second
//! Chrome profile.
//!
//! The attach path is deliberately extracted from `chromium.rs` so
//! that file stays under the 550-LOC clean-code cap.
//!
//! # Security model
//!
//! CDP over a debug port is a full RCE vector: whoever can speak
//! `Runtime.evaluate` to Chrome can run arbitrary JS on every page.
//! LAD enforces **loopback-only** CDP endpoints via
//! [`crate::sanitize::is_loopback_only`]. A remote CDP endpoint (even
//! one that looks like a private IP, e.g. `192.168.1.10`) is rejected
//! before any TCP connection is attempted. This is the inverse of
//! SSRF: here we BLOCK non-loopback hosts, because the normal
//! threat model (attacker-controlled target URL) is flipped — the
//! user's own machine is the only safe target for attach.

use std::sync::Arc;

use super::chromium::{ChromiumEngine, spawn_cdp_handler};

/// `GET {endpoint}/json/version` timeout — matches the short-lived
/// nature of the probe. We're only issuing a single request against
/// localhost, so 5s is generous.
const DISCOVERY_TIMEOUT_SECS: u64 = 5;

impl ChromiumEngine {
    /// Attach to an already-running Chrome/Chromium instance via CDP.
    ///
    /// `endpoint` accepts either:
    ///
    /// - A raw WebSocket URL like `ws://127.0.0.1:9222/devtools/browser/<uuid>`
    ///   (passed directly to `chromiumoxide::Browser::connect`), or
    /// - An HTTP debug endpoint like `http://localhost:9222` — LAD issues
    ///   `GET /json/version`, parses the JSON response, and extracts the
    ///   `webSocketDebuggerUrl` field before connecting.
    ///
    /// The resolved URL (WS or HTTP) MUST point at a loopback host. See
    /// the module-level security notes for the threat model.
    ///
    /// The returned engine has `owned: false`, so calling `close()` on
    /// it will NOT terminate the user's Chrome process — only LAD's
    /// liveness flag is flipped off.
    pub async fn attach(endpoint: &str) -> Result<Self, crate::Error> {
        // Defense in depth — the `lad_session` handler checks this too
        // (Task 3.2), but re-validating at the engine boundary means
        // every future caller of `ChromiumEngine::attach` inherits the
        // guard for free.
        if !crate::sanitize::is_loopback_only(endpoint) {
            return Err(crate::Error::Sanitize(format!(
                "CDP attach is loopback-only for safety (got: {endpoint})"
            )));
        }

        let ws_url = resolve_cdp_ws_url(endpoint).await?;

        // The resolved WS URL must ALSO be loopback. This matters for
        // the HTTP-discovery branch: a malicious attacker running a
        // rogue debug server on localhost could return a
        // `webSocketDebuggerUrl` that points anywhere. Double-check.
        if !crate::sanitize::is_loopback_only(&ws_url) {
            return Err(crate::Error::Sanitize(format!(
                "resolved CDP WebSocket URL is non-loopback — refusing to connect: {ws_url}"
            )));
        }

        tracing::info!(endpoint = %endpoint, ws = %ws_url, "attaching to running chrome via CDP");

        let (browser, handler) = chromiumoxide::Browser::connect(&ws_url)
            .await
            .map_err(|e| crate::Error::Browser(format!("CDP connect failed: {e}")))?;

        let (alive, handle) = spawn_cdp_handler(handler);

        Ok(Self {
            browser: Arc::new(browser),
            _handler: handle,
            _temp_dir: None,
            alive,
            owned: false,
        })
    }
}

/// Resolve a CDP endpoint URL to a raw WebSocket URL suitable for
/// `chromiumoxide::Browser::connect`.
///
/// - `ws://` or `wss://` — pass-through.
/// - `http://` or `https://` — `GET {endpoint}/json/version`, parse
///   the JSON body, return `webSocketDebuggerUrl`.
/// - anything else — `Error::Sanitize` (covers malformed and
///   non-web schemes).
///
/// Exposed at `pub(crate)` so the unit tests below can exercise it
/// without spinning up a real browser.
pub(crate) async fn resolve_cdp_ws_url(endpoint: &str) -> Result<String, crate::Error> {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() {
        return Err(crate::Error::Sanitize("CDP endpoint is empty".to_string()));
    }

    // WebSocket URLs pass through unchanged — this is what
    // `chromiumoxide::Browser::connect` wants natively.
    if trimmed.starts_with("ws://") || trimmed.starts_with("wss://") {
        return Ok(trimmed.to_string());
    }

    // HTTP/HTTPS: hit /json/version and parse out the WS URL.
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        // Strip trailing slash so we don't end up with a double slash.
        let base = trimmed.trim_end_matches('/');
        let version_url = format!("{base}/json/version");

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(DISCOVERY_TIMEOUT_SECS))
            .build()
            .map_err(|e| crate::Error::Browser(format!("reqwest client build: {e}")))?;

        let resp = client.get(&version_url).send().await.map_err(|e| {
            crate::Error::Browser(format!(
                "failed to GET {version_url}: {e} — is Chrome running with --remote-debugging-port?"
            ))
        })?;

        if !resp.status().is_success() {
            return Err(crate::Error::Browser(format!(
                "CDP discovery GET {version_url} returned HTTP {}",
                resp.status()
            )));
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| {
            crate::Error::Browser(format!("CDP discovery response parse failed: {e}"))
        })?;

        let ws = body
            .get("webSocketDebuggerUrl")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::Error::Browser(format!(
                    "CDP discovery response missing webSocketDebuggerUrl field: {body}"
                ))
            })?;

        return Ok(ws.to_string());
    }

    Err(crate::Error::Sanitize(format!(
        "CDP endpoint must start with ws://, wss://, http:// or https:// (got: {trimmed})"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Helper to avoid `Result::unwrap_err` requiring `T: Debug`.
    /// `ChromiumEngine` is not `Debug` (holds `JoinHandle`, `Arc<Browser>`,
    /// etc.), and making it so would leak implementation details.
    fn expect_sanitize(result: Result<ChromiumEngine, crate::Error>, ctx: &str) {
        match result {
            Err(crate::Error::Sanitize(_)) => {}
            Err(other) => panic!("{ctx}: expected Sanitize error, got {other:?}"),
            Ok(_) => panic!("{ctx}: expected error, got Ok"),
        }
    }

    // ── resolve_cdp_ws_url (URL passthrough) ────────────────

    #[tokio::test]
    async fn resolve_ws_url_passes_through_unchanged() {
        let input = "ws://127.0.0.1:9222/devtools/browser/abc-123";
        let out = resolve_cdp_ws_url(input).await.unwrap();
        assert_eq!(out, input);
    }

    #[tokio::test]
    async fn resolve_wss_url_passes_through_unchanged() {
        let input = "wss://localhost:9222/devtools/browser/zzz";
        let out = resolve_cdp_ws_url(input).await.unwrap();
        assert_eq!(out, input);
    }

    #[tokio::test]
    async fn resolve_rejects_empty_endpoint() {
        let err = resolve_cdp_ws_url("").await.expect_err("expected error");
        assert!(matches!(err, crate::Error::Sanitize(_)));
    }

    #[tokio::test]
    async fn resolve_rejects_bare_path() {
        let err = resolve_cdp_ws_url("/devtools/browser/abc")
            .await
            .expect_err("expected error");
        assert!(matches!(err, crate::Error::Sanitize(_)));
    }

    #[tokio::test]
    async fn resolve_rejects_unsupported_scheme() {
        let err = resolve_cdp_ws_url("chrome://whatever")
            .await
            .expect_err("expected error");
        assert!(matches!(err, crate::Error::Sanitize(_)));
    }

    // ── resolve_cdp_ws_url (HTTP discovery via mock server) ─

    /// Spawn a tiny HTTP/1.1 mock that returns `body` for one request
    /// and exits. Returns the bound loopback address.
    ///
    /// Intentionally minimal — parses until `\r\n\r\n`, then replies
    /// with a fixed 200 OK. Not a general-purpose server, just
    /// enough to exercise the discovery path without pulling in
    /// mockito or hyper.
    async fn spawn_mock_once(body: String) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                // Drain the request headers.
                let mut buf = [0u8; 2048];
                let mut consumed = 0;
                loop {
                    let Ok(n) = sock.read(&mut buf[consumed..]).await else {
                        return;
                    };
                    if n == 0 {
                        break;
                    }
                    consumed += n;
                    if buf[..consumed].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                    if consumed == buf.len() {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn attach_http_url_triggers_discovery() {
        let body = r#"{
            "Browser": "Chrome/145.0.0.0",
            "Protocol-Version": "1.3",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/browser/test-uuid"
        }"#
        .to_string();
        let addr = spawn_mock_once(body).await;

        let endpoint = format!("http://{addr}");
        let out = resolve_cdp_ws_url(&endpoint).await.unwrap();
        assert_eq!(out, "ws://127.0.0.1:9222/devtools/browser/test-uuid");
    }

    #[tokio::test]
    async fn attach_http_url_missing_ws_field_errors() {
        let body = r#"{"Browser": "Chrome/145.0.0.0"}"#.to_string();
        let addr = spawn_mock_once(body).await;

        let endpoint = format!("http://{addr}");
        let err = resolve_cdp_ws_url(&endpoint)
            .await
            .expect_err("expected error");
        match err {
            crate::Error::Browser(msg) => {
                assert!(
                    msg.contains("webSocketDebuggerUrl"),
                    "unexpected error: {msg}"
                );
            }
            other => panic!("expected Browser error, got {other:?}"),
        }
    }

    // ── ChromiumEngine::attach full-path guard tests ────────

    #[tokio::test]
    async fn attach_rejects_remote_host_ws() {
        expect_sanitize(
            ChromiumEngine::attach("ws://192.168.1.1:9222/devtools/browser/x").await,
            "private IP ws",
        );
    }

    #[tokio::test]
    async fn attach_rejects_remote_host_http() {
        expect_sanitize(
            ChromiumEngine::attach("http://10.0.0.1:9222").await,
            "private IP http",
        );
    }

    #[tokio::test]
    async fn attach_rejects_evil_hostname() {
        expect_sanitize(
            ChromiumEngine::attach("ws://evil.com:9222/devtools/browser/x").await,
            "evil.com",
        );
    }

    #[tokio::test]
    async fn attach_rejects_empty_input() {
        expect_sanitize(ChromiumEngine::attach("").await, "empty endpoint");
    }

    #[tokio::test]
    async fn attach_rejects_missing_scheme() {
        expect_sanitize(
            ChromiumEngine::attach("localhost:9222").await,
            "missing scheme",
        );
    }

    #[tokio::test]
    async fn attach_accepts_loopback_variants_then_fails_connect() {
        // Loopback gate passes, then `Browser::connect` fails because
        // no Chrome is running on these ports. We just assert the
        // gate doesn't reject them as Sanitize errors.
        for endpoint in [
            "ws://localhost:1/devtools/browser/never",
            "ws://127.0.0.1:1/devtools/browser/never",
            "ws://[::1]:1/devtools/browser/never",
        ] {
            match ChromiumEngine::attach(endpoint).await {
                Err(crate::Error::Sanitize(msg)) => {
                    panic!("{endpoint}: rejected by loopback gate: {msg}")
                }
                Err(crate::Error::Browser(_)) => { /* connect failure, expected */ }
                Err(other) => panic!("{endpoint}: unexpected error {other:?}"),
                Ok(_) => panic!("{endpoint}: unexpected success — is something on port 1?"),
            }
        }
    }

    #[tokio::test]
    async fn attach_rejects_http_discovery_pointing_to_remote_ws() {
        // Attacker runs a rogue /json/version on localhost that returns
        // a remote webSocketDebuggerUrl. The second loopback gate must
        // catch this before `Browser::connect` fires.
        let body = r#"{
            "webSocketDebuggerUrl": "ws://evil.com:9222/devtools/browser/pwned"
        }"#
        .to_string();
        let addr = spawn_mock_once(body).await;

        let endpoint = format!("http://{addr}");
        match ChromiumEngine::attach(&endpoint).await {
            Err(crate::Error::Sanitize(msg)) => {
                assert!(msg.contains("non-loopback"), "unexpected error: {msg}");
            }
            Err(other) => panic!("expected Sanitize error, got {other:?}"),
            Ok(_) => panic!("expected Sanitize error, got Ok"),
        }
    }

    // ── Live attach test — gated ────────────────────────────

    /// End-to-end attach test. Requires a real Chrome running with
    /// `--remote-debugging-port=9222`. Ignored by default because
    /// most dev machines and CI do not have that.
    ///
    /// To run manually:
    ///
    /// ```sh
    /// google-chrome --remote-debugging-port=9222 \
    ///   --user-data-dir=/tmp/lad-attach-test &
    /// cargo test -p menot-you-mcp-lad attach_real_chrome_instance \
    ///   --lib -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "needs live chrome with --remote-debugging-port=9222"]
    async fn attach_real_chrome_instance() {
        let engine = ChromiumEngine::attach("http://localhost:9222")
            .await
            .expect("attach to running chrome");
        use crate::engine::BrowserEngine;
        assert_eq!(engine.name(), "chromium");
        let pages = engine.adopt_existing_pages().await.expect("list pages");
        // Real Chrome always has at least one tab.
        assert!(!pages.is_empty(), "expected at least one existing tab");
    }
}
