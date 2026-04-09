//! lad-relay — bidirectional stdin/stdout ↔ WebSocket pipe.
//!
//! Runs as a WebSocket **server** that accepts a single connection from
//! a remote browser engine (e.g., iPhone running WKWebView).
//! LAD spawns this as a child process and communicates via stdin/stdout.
//!
//! The relay:
//!   1. Binds a WebSocket server on a random port
//!   2. Generates a one-time auth token
//!   3. Prints a pairing URL (+ QR code) to stderr
//!   4. Waits for the iPhone to connect
//!   5. Pipes stdin ↔ WebSocket bidirectionally

use std::time::Duration;

use clap::Parser;
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};

mod discovery;
mod qr;
mod token;

/// lad-relay — WebSocket server bridge for remote browser engines.
#[derive(Parser)]
#[command(name = "lad-relay", version)]
struct Cli {
    /// Port to bind the WebSocket server on. 0 = random.
    #[arg(long, default_value = "0")]
    port: u16,

    /// Bind address.
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,

    /// Disable auth token requirement (NOT recommended on shared networks).
    #[arg(long)]
    no_auth: bool,

    /// Timeout waiting for iPhone to connect, in seconds.
    /// SEC-S7: Reduced from 120s to 30s to minimize brute-force window.
    #[arg(long, default_value = "30")]
    connect_timeout: u64,

    /// Also publish via Bonjour/mDNS for local network discovery.
    #[arg(long)]
    bonjour: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    // Generate auth token.
    let auth_token = if cli.no_auth {
        None
    } else {
        Some(token::generate())
    };

    // Bind TCP listener.
    let bind_addr = format!("{}:{}", cli.bind, cli.port);
    let listener = TcpListener::bind(&bind_addr).await?;
    let local_addr = listener.local_addr()?;
    info!("relay server listening on {local_addr}");

    // Determine LAN IP.
    let local_ip = local_ip_address().unwrap_or_else(|| local_addr.ip().to_string());
    let pairing_url = build_pairing_url(&local_ip, local_addr.port(), auth_token.as_deref());

    // Print pairing info to stderr (LAD reads stdout for NDJSON).
    eprintln!();
    eprintln!("  lad-relay ready — scan QR code or enter URL in Nott app:");
    eprintln!();
    eprintln!("  {pairing_url}");
    eprintln!();
    qr::print_qr_stderr(&pairing_url);
    eprintln!();

    // Optionally publish via Bonjour.
    let _bonjour_guard = if cli.bonjour {
        Some(discovery::publish_service(local_addr.port())?)
    } else {
        None
    };

    // Wait for iPhone to connect (with timeout).
    let ws_stream = accept_one(
        &listener,
        auth_token.as_deref(),
        Duration::from_secs(cli.connect_timeout),
    )
    .await?;

    eprintln!("  iPhone connected!");
    eprintln!();

    let (mut ws_sink, mut ws_source) = ws_stream.split();

    // Stdin reader (line-buffered NDJSON from LAD).
    let stdin = tokio::io::stdin();
    let mut stdin_reader = BufReader::new(stdin).lines();

    // Stdout writer (NDJSON responses back to LAD).
    let mut stdout = tokio::io::stdout();

    // Emit a synthetic "ready" event so LAD knows the bridge is up.
    let ready = r#"{"event":"ready","version":"0.1.0","engine":"remote-webkit"}"#;
    stdout.write_all(format!("{ready}\n").as_bytes()).await?;
    stdout.flush().await?;

    // Bidirectional pipe.
    loop {
        tokio::select! {
            // stdin (LAD) → WebSocket (iPhone)
            line = stdin_reader.next_line() => {
                match line {
                    Ok(Some(text)) => {
                        if text.is_empty() { continue; }
                        if let Err(e) = ws_sink.send(Message::Text(text.into())).await {
                            error!("ws send failed: {e}");
                            break;
                        }
                    }
                    Ok(None) => {
                        info!("stdin closed, shutting down");
                        let _ = ws_sink.send(Message::Close(None)).await;
                        break;
                    }
                    Err(e) => {
                        error!("stdin read error: {e}");
                        break;
                    }
                }
            }

            // WebSocket (iPhone) → stdout (LAD)
            msg = ws_source.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // SEC-S4: Drop messages exceeding 20 MB to prevent OOM.
                        if text.len() > 20 * 1024 * 1024 {
                            warn!("dropped oversized WS message: {} bytes", text.len());
                            continue;
                        }
                        if let Err(e) = stdout.write_all(format!("{text}\n").as_bytes()).await {
                            error!("stdout write error: {e}");
                            break;
                        }
                        let _ = stdout.flush().await;
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!("iPhone disconnected");
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = ws_sink.send(Message::Pong(data)).await;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        error!("ws receive error: {e}");
                        break;
                    }
                    None => {
                        info!("ws stream ended");
                        break;
                    }
                }
            }
        }
    }

    info!("relay stopped");
    Ok(())
}

/// Accept exactly one WebSocket connection, validating the auth token.
///
/// SEC-S6: Rate-limits failed attempts (1s delay, max 10).
/// SEC-S8: 5s handshake timeout prevents TCP tarpit DoS.
async fn accept_one(
    listener: &TcpListener,
    expected_token: Option<&str>,
    timeout: Duration,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    let accept_fut = async {
        let mut failed_attempts: u32 = 0;
        const MAX_FAILED: u32 = 10;

        loop {
            if failed_attempts >= MAX_FAILED {
                return Err::<_, Box<dyn std::error::Error + Send + Sync>>(
                    format!("too many failed attempts ({MAX_FAILED})").into(),
                );
            }

            let (stream, peer) = listener.accept().await?;
            info!("incoming connection from {peer}");

            // SEC-S8: 5s handshake timeout — reject slow/tarpit connections.
            let handshake = async {
                #[allow(clippy::result_large_err)]
                let callback = |req: &http::Request<()>,
                                resp: http::Response<()>|
                 -> Result<_, http::Response<Option<String>>> {
                    if let Some(token) = expected_token {
                        let uri = req.uri().to_string();
                        let has_valid_token = uri
                            .split('?')
                            .nth(1)
                            .unwrap_or("")
                            .split('&')
                            .any(|param| param == format!("token={token}"));
                        if !has_valid_token {
                            warn!("rejected {peer}: invalid token");
                            return Err(http::Response::builder()
                                .status(401)
                                .body(Some("invalid token".into()))
                                .unwrap());
                        }
                    }
                    Ok(resp)
                };

                tokio_tungstenite::accept_hdr_async(stream, callback).await
            };

            match tokio::time::timeout(Duration::from_secs(5), handshake).await {
                Ok(Ok(ws)) => return Ok(ws),
                Ok(Err(e)) => {
                    failed_attempts += 1;
                    warn!(
                        "handshake failed from {peer}: {e} (attempt {failed_attempts}/{MAX_FAILED})"
                    );
                    // SEC-S6: 1s delay after failed attempt to slow brute-force.
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(_) => {
                    failed_attempts += 1;
                    warn!("handshake timeout from {peer} (attempt {failed_attempts}/{MAX_FAILED})");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    };

    tokio::time::timeout(timeout, accept_fut).await.map_err(
        |_| -> Box<dyn std::error::Error + Send + Sync> {
            "timeout waiting for iPhone to connect".into()
        },
    )?
}

/// Build the pairing URL with auth token.
fn build_pairing_url(ip: &str, port: u16, token: Option<&str>) -> String {
    match token {
        Some(t) => format!("ws://{ip}:{port}?token={t}"),
        None => format!("ws://{ip}:{port}"),
    }
}

/// Detect the LAN IP by opening a UDP socket (no traffic sent).
fn local_ip_address() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // Connect to a well-known DNS resolver to determine the outbound interface.
    socket.connect("1.1.1.1:53").ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}
