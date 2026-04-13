//! E2E integration tests for lad-relay.
//!
//! Spawns lad-relay as a child process, connects a mock WebSocket client,
//! and verifies NDJSON passthrough in both directions.

use std::process::Stdio;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// Spawn lad-relay on a random port, return (child, port, stdout_reader).
/// Drains stderr in a background task to prevent pipe buffer deadlock.
async fn spawn_relay(
    no_auth: bool,
) -> (
    tokio::process::Child,
    u16,
    BufReader<tokio::process::ChildStdout>,
) {
    let relay_bin = env!("CARGO_BIN_EXE_lad-relay");
    let mut cmd = Command::new(relay_bin);
    cmd.arg("--port").arg("0");
    if no_auth {
        cmd.arg("--no-auth");
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("failed to spawn lad-relay");

    // Read stderr to find the bound port.
    let stderr = child.stderr.take().expect("no stderr");
    let mut stderr_reader = BufReader::new(stderr);

    let port = tokio::time::timeout(Duration::from_secs(10), async {
        let mut line = String::new();
        loop {
            line.clear();
            let n = stderr_reader
                .read_line(&mut line)
                .await
                .expect("read stderr");
            if n == 0 {
                panic!("relay stderr closed before port found");
            }
            // Look for "relay server listening on 0.0.0.0:PORT"
            if let Some(addr_part) = line.split("listening on ").nth(1)
                && let Some(port_str) = addr_part.trim().rsplit(':').next()
                && let Ok(p) = port_str.parse::<u16>()
            {
                return p;
            }
        }
    })
    .await
    .expect("timeout waiting for relay to start");

    // Drain remaining stderr in background to prevent pipe buffer deadlock.
    // The relay prints QR code + pairing info after the "listening" line.
    tokio::spawn(async move {
        let mut line = String::new();
        loop {
            line.clear();
            match stderr_reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {} // Discard stderr lines.
            }
        }
    });

    // Wait for the relay to finish QR output and enter accept loop.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let stdout = child.stdout.take().expect("no stdout");
    let stdout_reader = BufReader::new(stdout);

    (child, port, stdout_reader)
}

#[tokio::test]
async fn relay_e2e_navigate_passthrough() {
    let (mut child, port, mut stdout_reader) = spawn_relay(true).await;

    // Connect mock WebSocket client.
    let url = format!("ws://127.0.0.1:{port}");
    let (ws_stream, _) = tokio::time::timeout(Duration::from_secs(5), connect_async(&url))
        .await
        .expect("timeout connecting")
        .expect("ws connect failed");

    let (mut ws_sink, mut ws_source) = ws_stream.split();

    // Read the synthetic "ready" event from stdout.
    let mut ready_line = String::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        stdout_reader.read_line(&mut ready_line),
    )
    .await
    .expect("timeout reading ready")
    .expect("read ready");
    assert!(
        ready_line.contains("\"event\":\"ready\""),
        "expected ready event, got: {ready_line}"
    );

    // Write a navigate command to relay's stdin.
    let stdin = child.stdin.as_mut().expect("no stdin");
    let cmd = r#"{"id":1,"cmd":"navigate","url":"https://example.com"}"#;
    stdin
        .write_all(format!("{cmd}\n").as_bytes())
        .await
        .expect("write stdin");
    stdin.flush().await.expect("flush stdin");

    // Read the command from WebSocket client side.
    let msg = tokio::time::timeout(Duration::from_secs(5), ws_source.next())
        .await
        .expect("timeout reading ws")
        .expect("ws stream ended")
        .expect("ws error");

    if let Message::Text(text) = msg {
        assert!(
            text.contains("navigate"),
            "expected navigate cmd, got: {text}"
        );
        assert!(
            text.contains("example.com"),
            "expected example.com, got: {text}"
        );
    } else {
        panic!("expected text message, got: {msg:?}");
    }

    // Send a response back via WebSocket.
    let resp = r#"{"id":1,"ok":true}"#;
    ws_sink
        .send(Message::Text(resp.into()))
        .await
        .expect("ws send");

    // Read the response from relay's stdout.
    let mut resp_line = String::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        stdout_reader.read_line(&mut resp_line),
    )
    .await
    .expect("timeout reading response")
    .expect("read response");
    assert!(
        resp_line.contains("\"ok\":true"),
        "expected ok response, got: {resp_line}"
    );

    // Cleanup.
    let _ = ws_sink.send(Message::Close(None)).await;
    child.kill().await.ok();
}

#[tokio::test]
async fn relay_e2e_ready_event_on_connect() {
    let (mut child, port, mut stdout_reader) = spawn_relay(true).await;

    let url = format!("ws://127.0.0.1:{port}");
    let (_ws_stream, _) = tokio::time::timeout(Duration::from_secs(5), connect_async(&url))
        .await
        .expect("timeout connecting")
        .expect("ws connect failed");

    // The ready event should appear on stdout immediately.
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), stdout_reader.read_line(&mut line))
        .await
        .expect("timeout reading ready")
        .expect("read ready");

    let parsed: serde_json::Value = serde_json::from_str(line.trim()).expect("parse json");
    assert_eq!(parsed["event"], "ready");
    assert_eq!(parsed["engine"], "remote-webkit");

    child.kill().await.ok();
}

#[tokio::test]
async fn relay_e2e_auth_rejection() {
    // Spawn WITH auth (no --no-auth flag).
    let (mut child, port, _stdout_reader) = spawn_relay(false).await;

    // Try connecting without a token — should be rejected.
    let url = format!("ws://127.0.0.1:{port}");
    let result = tokio::time::timeout(Duration::from_secs(5), connect_async(&url)).await;

    match result {
        Ok(Ok(_)) => panic!("expected connection to be rejected without token"),
        Ok(Err(_e)) => {
            // Expected: handshake rejected.
        }
        Err(_) => {
            // Timeout is also acceptable — relay didn't respond.
        }
    }

    child.kill().await.ok();
}
