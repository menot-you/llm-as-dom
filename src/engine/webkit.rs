//! WebKit browser engine adapter — communicates with `lad-webkit-bridge`
//! (Swift macOS sidecar) over stdin/stdout NDJSON.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{Mutex, oneshot};

use super::webkit_proto::{Request, Response, cookie_from_wire, cookie_to_wire};
use super::{BrowserEngine, EngineConfig, PageHandle};

/// Shared connection state for sending requests to the WebKit bridge.
struct BridgeConnection {
    writer: Mutex<Option<ChildStdin>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Response>>>,
    next_id: AtomicU64,
    /// Set to `false` when the reader loop detects bridge exit (EOF/error).
    alive: Arc<AtomicBool>,
}

impl BridgeConnection {
    /// Send a request and wait for the correlated response.
    async fn request(&self, mut req: Request) -> Result<Response, crate::Error> {
        // Fail fast if bridge is dead.
        if !self.alive.load(Ordering::Relaxed) {
            return Err(crate::Error::Browser("webkit bridge is not running".into()));
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        req.id = id;

        let (tx, rx) = oneshot::channel();

        // Write FIRST — only insert into pending map after successful write.
        let json = serde_json::to_string(&req)
            .map_err(|e| crate::Error::Backend(format!("serialize request: {e}")))?;

        {
            let mut writer_opt = self.writer.lock().await;
            if let Some(writer) = writer_opt.as_mut() {
                writer
                    .write_all(json.as_bytes())
                    .await
                    .map_err(|e| crate::Error::Browser(format!("write to webkit bridge: {e}")))?;
                writer
                    .write_all(b"\n")
                    .await
                    .map_err(|e| crate::Error::Browser(format!("write newline: {e}")))?;
                writer
                    .flush()
                    .await
                    .map_err(|e| crate::Error::Browser(format!("flush: {e}")))?;
            } else {
                return Err(crate::Error::Browser("webkit bridge stdin closed".into()));
            }
        }

        // Insert AFTER successful write — no orphan entries on write failure.
        self.pending.lock().await.insert(id, tx);

        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(resp)) => {
                if resp.ok == Some(false) {
                    Err(crate::Error::Browser(
                        resp.error.unwrap_or_else(|| "unknown webkit error".into()),
                    ))
                } else {
                    Ok(resp)
                }
            }
            Ok(Err(_)) => Err(crate::Error::Browser(
                "webkit bridge response channel closed".into(),
            )),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(crate::Error::Timeout)
            }
        }
    }
}

/// WebKit browser engine via macOS sidecar.
pub struct WebKitEngine {
    conn: Arc<BridgeConnection>,
    reader_task: tokio::task::JoinHandle<()>,
    child: Option<Child>,
}

impl WebKitEngine {
    /// Launch the `lad-webkit-bridge` sidecar process.
    pub async fn launch(config: EngineConfig) -> Result<Self, crate::Error> {
        let bridge_path =
            std::env::var("LAD_WEBKIT_BRIDGE").unwrap_or_else(|_| "lad-webkit-bridge".to_string());

        let mut cmd = tokio::process::Command::new(&bridge_path);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());

        if config.visible || config.interactive {
            cmd.env("LAD_WEBKIT_VISIBLE", "1");
        }
        cmd.env("LAD_WEBKIT_WIDTH", config.window_size.0.to_string());
        cmd.env("LAD_WEBKIT_HEIGHT", config.window_size.1.to_string());

        let mut child = cmd.spawn().map_err(|e| {
            crate::Error::Browser(format!(
                "failed to launch webkit bridge at '{bridge_path}': {e}"
            ))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| crate::Error::Browser("no stdin on webkit bridge".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| crate::Error::Browser("no stdout on webkit bridge".into()))?;

        let alive = Arc::new(AtomicBool::new(true));

        let conn = Arc::new(BridgeConnection {
            writer: Mutex::new(Some(stdin)),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            alive: Arc::clone(&alive),
        });

        // Ready handshake: reader task signals when it sees {"event":"ready"}.
        let (ready_tx, ready_rx) = oneshot::channel::<()>();
        let ready_tx = Arc::new(tokio::sync::Mutex::new(Some(ready_tx)));

        let reader_task = tokio::spawn(Self::read_loop(
            stdout,
            Arc::clone(&conn),
            Arc::clone(&alive),
            ready_tx,
        ));

        // Wait for the bridge to emit "ready" — 5s timeout.
        match tokio::time::timeout(std::time::Duration::from_secs(5), ready_rx).await {
            Ok(Ok(())) => tracing::info!("webkit bridge ready"),
            _ => {
                let _ = child.kill().await;
                return Err(crate::Error::Browser(
                    "webkit bridge failed to start within 5 seconds".into(),
                ));
            }
        }

        Ok(Self {
            conn,
            reader_task,
            child: Some(child),
        })
    }

    /// Background task: read stdout lines, dispatch responses / log events.
    async fn read_loop(
        stdout: tokio::process::ChildStdout,
        conn: Arc<BridgeConnection>,
        alive: Arc<AtomicBool>,
        ready_tx: Arc<tokio::sync::Mutex<Option<oneshot::Sender<()>>>>,
    ) {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Response>(trimmed) {
                        Ok(resp) => {
                            Self::dispatch(resp, &conn, &ready_tx).await;
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                line = trimmed,
                                "failed to parse webkit bridge response"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "webkit bridge stdout read error");
                    break;
                }
            }
        }

        // Bridge exited — mark dead and drain all pending requests.
        alive.store(false, Ordering::Relaxed);
        tracing::error!("webkit bridge process exited");

        let mut pending = conn.pending.lock().await;
        for (_, sender) in pending.drain() {
            let error_resp = Response {
                ok: Some(false),
                error: Some("webkit bridge process exited".into()),
                ..Default::default()
            };
            let _ = sender.send(error_resp);
        }
    }

    /// Route a parsed response to its pending sender or log as event.
    async fn dispatch(
        resp: Response,
        conn: &BridgeConnection,
        ready_tx: &Arc<tokio::sync::Mutex<Option<oneshot::Sender<()>>>>,
    ) {
        if let Some(id) = resp.id {
            let mut map = conn.pending.lock().await;
            if let Some(sender) = map.remove(&id) {
                let _ = sender.send(resp);
            }
        } else if let Some(ref evt) = resp.event {
            match evt.as_str() {
                "ready" => {
                    if let Some(version) = resp.value.as_ref().and_then(|v| v.as_str()) {
                        tracing::info!(version, "webkit bridge ready (event)");
                    } else {
                        tracing::info!("webkit bridge ready (event)");
                    }
                    if let Some(tx) = ready_tx.lock().await.take() {
                        let _ = tx.send(());
                    }
                }
                "console" => {
                    let level = resp.level.as_deref().unwrap_or("log");
                    let msg = resp.message.as_deref().unwrap_or("");
                    tracing::debug!(engine = "webkit", level, "console: {msg}");
                }
                "load" => {
                    let url = resp
                        .value
                        .as_ref()
                        .and_then(|v| v.as_str())
                        .or(resp.message.as_deref())
                        .unwrap_or("?");
                    tracing::debug!(url, "webkit page loaded");
                }
                "monitor" => {
                    if let Some(val) = &resp.value {
                        tracing::info!(
                            "monitor script emitted: {}",
                            serde_json::to_string_pretty(val).unwrap_or_default()
                        );
                    } else if let Some(msg) = resp.message {
                        tracing::info!("monitor notice: {}", msg);
                    }
                }
                "network" => {
                    let kind = resp.req_type.as_deref().unwrap_or("Network");
                    let method = resp.method.as_deref().unwrap_or("GET");
                    let url = resp.url.as_deref().unwrap_or("");
                    tracing::info!(kind, method, url, "network activity");
                }
                other => tracing::debug!(event = other, "webkit event"),
            }
        }
    }
}

/// Clean up the Swift sidecar on drop: abort reader, kill child process.
impl Drop for WebKitEngine {
    fn drop(&mut self) {
        // 1. Abort the reader task (stops reading stdout).
        self.reader_task.abort();

        // 2. Extract child process and spawn a graceful shutdown wait task.
        if let Some(mut child) = self.child.take() {
            let conn = Arc::clone(&self.conn);
            tokio::spawn(async move {
                // Force close stdin so Swift sees EOF immediately
                if let Ok(mut lock) = conn.writer.try_lock() {
                    let _ = lock.take();
                } else {
                    let mut lock = conn.writer.lock().await;
                    let _ = lock.take();
                }

                // Wait up to 3 seconds for clean exit (e.g. following EOF or 'close' command)
                match tokio::time::timeout(std::time::Duration::from_secs(3), child.wait()).await {
                    Ok(Ok(status)) => {
                        tracing::debug!(%status, "webkit bridge exited cleanly via drop")
                    }
                    Ok(Err(e)) => tracing::warn!(error = %e, "webkit bridge error on wait"),
                    Err(_) => {
                        tracing::warn!("webkit bridge failed to exit within 3s, force killing");
                        let _ = child.start_kill();
                    }
                }
            });
        }
    }
}

#[async_trait]
impl BrowserEngine for WebKitEngine {
    async fn new_page(&self, url: &str) -> Result<Box<dyn PageHandle>, crate::Error> {
        let mut req = Request::cmd("navigate");
        req.url = Some(url.into());
        self.conn.request(req).await?;

        self.conn
            .request(Request::cmd("wait_for_navigation"))
            .await?;

        Ok(Box::new(WebKitPage {
            conn: Arc::clone(&self.conn),
        }))
    }

    fn name(&self) -> &str {
        "webkit"
    }

    async fn close(&self) -> Result<(), crate::Error> {
        let _ = self.conn.request(Request::cmd("close")).await;
        Ok(())
    }
}

struct WebKitPage {
    conn: Arc<BridgeConnection>,
}

#[async_trait]
impl PageHandle for WebKitPage {
    async fn eval_js(&self, script: &str) -> Result<serde_json::Value, crate::Error> {
        let mut req = Request::cmd("eval_js");
        req.script = Some(script.into());
        let resp = self.conn.request(req).await?;
        Ok(resp.value.unwrap_or(serde_json::Value::Null))
    }

    async fn navigate(&self, url: &str) -> Result<(), crate::Error> {
        let mut req = Request::cmd("navigate");
        req.url = Some(url.into());
        self.conn.request(req).await?;
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        Ok(())
    }

    async fn wait_for_navigation(&self) -> Result<(), crate::Error> {
        self.conn
            .request(Request::cmd("wait_for_navigation"))
            .await?;
        Ok(())
    }

    async fn url(&self) -> Result<String, crate::Error> {
        let resp = self.conn.request(Request::cmd("url")).await?;
        Ok(resp
            .value
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "unknown".into()))
    }

    async fn title(&self) -> Result<String, crate::Error> {
        let resp = self.conn.request(Request::cmd("title")).await?;
        Ok(resp
            .value
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default())
    }

    async fn screenshot_png(&self) -> Result<Vec<u8>, crate::Error> {
        let resp = self.conn.request(Request::cmd("screenshot")).await?;
        let b64 = resp
            .png_b64
            .ok_or_else(|| crate::Error::Browser("no screenshot data".into()))?;
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .map_err(|e| crate::Error::Browser(format!("base64 decode: {e}")))
    }

    async fn cookies(&self) -> Result<Vec<crate::session::CookieEntry>, crate::Error> {
        let resp = self.conn.request(Request::cmd("cookies")).await?;
        Ok(resp
            .cookies
            .unwrap_or_default()
            .into_iter()
            .map(cookie_from_wire)
            .collect())
    }

    async fn set_cookies(
        &self,
        cookies: &[crate::session::CookieEntry],
    ) -> Result<(), crate::Error> {
        let mut req = Request::cmd("set_cookies");
        req.cookies = Some(cookies.iter().map(cookie_to_wire).collect());
        self.conn.request(req).await?;
        Ok(())
    }

    async fn enable_network_monitoring(&self) -> Result<bool, crate::Error> {
        Ok(true) // We hooked URLProtocol in Swift sidecar
    }

    async fn start_monitoring(&self, interval_ms: u32, script: &str) -> Result<(), crate::Error> {
        let mut req = Request::cmd("start_monitoring");
        req.interval = Some(interval_ms);
        req.script = Some(script.to_string());
        self.conn.request(req).await?;
        Ok(())
    }

    async fn stop_monitoring(&self) -> Result<(), crate::Error> {
        let req = Request::cmd("stop_monitoring");
        self.conn.request(req).await?;
        Ok(())
    }
}
