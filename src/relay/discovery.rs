//! Bonjour/mDNS discovery for `_lad._tcp` services.
//!
//! Supports both **publishing** (relay server announces itself) and
//! **browsing** (finding remote engines on the network).

use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tracing::{info, warn};

/// Service type for LAD remote browser engines.
const SERVICE_TYPE: &str = "_lad._tcp.local.";

/// Guard that keeps a Bonjour service registration alive.
/// The service is unregistered when this guard is dropped.
pub struct BonjourGuard {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Drop for BonjourGuard {
    fn drop(&mut self) {
        info!("unregistering Bonjour service: {}", self.fullname);
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// Publish a `_lad._tcp` service on the local network via Bonjour.
///
/// Returns a guard that keeps the registration alive until dropped.
pub fn publish_service(
    port: u16,
) -> Result<BonjourGuard, Box<dyn std::error::Error + Send + Sync>> {
    let daemon = ServiceDaemon::new().map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
        format!("mDNS daemon init failed: {e}").into()
    })?;

    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "lad-relay".into());

    let instance_name = format!("lad-relay-{hostname}");

    let service = ServiceInfo::new(
        SERVICE_TYPE,
        &instance_name,
        &format!("{hostname}.local."),
        "",
        port,
        None,
    )
    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
        format!("ServiceInfo creation failed: {e}").into()
    })?;

    let fullname = service.get_fullname().to_string();

    daemon
        .register(service)
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
            format!("mDNS register failed: {e}").into()
        })?;

    info!("Bonjour: published {fullname} on port {port}");

    Ok(BonjourGuard { daemon, fullname })
}

/// Discover a `_lad._tcp` service on the local network via mDNS/Bonjour.
///
/// Returns the first found `ws://host:port` URL, or an error on timeout.
#[allow(dead_code)]
pub async fn find_lad_service(
    timeout: Duration,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    tokio::task::spawn_blocking(move || find_blocking(timeout)).await?
}

fn find_blocking(timeout: Duration) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mdns = ServiceDaemon::new().map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
        format!("mDNS daemon init failed: {e}").into()
    })?;
    let receiver =
        mdns.browse(SERVICE_TYPE)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("mDNS browse failed: {e}").into()
            })?;

    let deadline = std::time::Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            mdns.shutdown().ok();
            return Err(format!("no _lad._tcp service found within {timeout:?}").into());
        }

        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let port = info.get_port();
                let addr = info
                    .get_addresses()
                    .iter()
                    .find(|a| a.is_ipv4())
                    .or_else(|| info.get_addresses().iter().next());

                if let Some(ip) = addr {
                    let url = format!("ws://{ip}:{port}");
                    info!(
                        "discovered: {} at {url} ({})",
                        info.get_fullname(),
                        info.get_hostname()
                    );
                    mdns.shutdown().ok();
                    return Ok(url);
                }
                warn!(
                    "service {} resolved but no addresses found",
                    info.get_fullname()
                );
            }
            Ok(ServiceEvent::SearchStarted(_)) => {
                info!("mDNS search started");
            }
            Ok(_) => {}
            Err(flume::RecvTimeoutError::Timeout) => {
                mdns.shutdown().ok();
                return Err(format!("no _lad._tcp service found within {timeout:?}").into());
            }
            Err(e) => {
                mdns.shutdown().ok();
                return Err(format!("mDNS recv error: {e}").into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_type_is_valid() {
        assert!(SERVICE_TYPE.starts_with('_'));
        assert!(SERVICE_TYPE.ends_with(".local."));
    }

    #[tokio::test]
    async fn discovery_times_out_gracefully() {
        let result = find_lad_service(Duration::from_millis(100)).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no _lad._tcp service found"));
    }
}
