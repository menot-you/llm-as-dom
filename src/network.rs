//! Network traffic capture and semantic classification.
//!
//! Automatically categorizes requests (auth, API, asset, navigation)
//! and captures relevant request/response data for debugging and
//! auth flow detection.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Classification of a network request by its semantic purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// Authentication endpoints (/oauth, /token, /login, /session, /auth)
    Auth,
    /// API calls (JSON responses, REST/GraphQL endpoints)
    Api,
    /// Static assets (CSS, JS, images, fonts) — usually ignored
    Asset,
    /// HTML document loads (page navigations)
    Navigation,
    /// WebSocket connections
    WebSocket,
    /// Unknown / unclassified
    Other,
}

/// A captured network request with classification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedRequest {
    /// Unique request identifier.
    pub request_id: String,
    /// Request URL.
    pub url: String,
    /// HTTP method (GET, POST, etc.).
    pub method: String,
    /// Semantic classification.
    pub kind: RequestKind,
    /// HTTP status code (0 if no response yet).
    pub status: u16,
    /// Response MIME type.
    pub mime_type: Option<String>,
    /// Whether the response set cookies (Set-Cookie header present).
    pub has_set_cookie: bool,
    /// Request timestamp (milliseconds since epoch).
    pub timestamp_ms: u64,
    /// Response body size in bytes.
    pub response_size: Option<u64>,
    /// POST body (truncated to 1KB, only for Auth/API requests).
    pub post_data: Option<String>,
}

/// Summary of network traffic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSummary {
    pub total_requests: u32,
    pub auth_count: u32,
    pub api_count: u32,
    pub navigation_count: u32,
    pub asset_count: u32,
    pub other_count: u32,
    pub total_bytes: u64,
    /// How many auth endpoints responded with Set-Cookie.
    pub auth_with_cookies: u32,
}

/// Network traffic collector — accumulates requests during a pilot run.
#[derive(Debug, Default)]
pub struct NetworkCapture {
    /// All captured requests, keyed by request ID.
    pub requests: HashMap<String, CapturedRequest>,
    /// Auth-related requests (subset, for quick access).
    pub auth_requests: Vec<String>,
    /// API requests (subset).
    pub api_requests: Vec<String>,
}

impl NetworkCapture {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a new request.
    pub fn on_request(
        &mut self,
        request_id: String,
        url: String,
        method: String,
        post_data: Option<String>,
    ) {
        let kind = classify_url(&url, &method);
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let truncated_post = post_data.map(|d| {
            if d.len() > 1024 {
                d[..1024].to_string()
            } else {
                d
            }
        });

        // Only keep post data for auth/api requests
        let keep_post = matches!(kind, RequestKind::Auth | RequestKind::Api);

        let captured = CapturedRequest {
            request_id: request_id.clone(),
            url,
            method,
            kind,
            status: 0,
            mime_type: None,
            has_set_cookie: false,
            timestamp_ms,
            response_size: None,
            post_data: if keep_post { truncated_post } else { None },
        };

        match kind {
            RequestKind::Auth => self.auth_requests.push(request_id.clone()),
            RequestKind::Api => self.api_requests.push(request_id.clone()),
            _ => {}
        }

        self.requests.insert(request_id, captured);
    }

    /// Update a request with response data.
    pub fn on_response(
        &mut self,
        request_id: &str,
        status: u16,
        mime_type: Option<String>,
        headers: &HashMap<String, String>,
    ) {
        if let Some(req) = self.requests.get_mut(request_id) {
            req.status = status;
            req.has_set_cookie = headers.keys().any(|k| k.eq_ignore_ascii_case("set-cookie"));

            // Reclassify based on response MIME if still unclassified
            if req.kind == RequestKind::Other
                && let Some(ref mime) = mime_type
            {
                if mime.contains("json") || mime.contains("graphql") {
                    req.kind = RequestKind::Api;
                    self.api_requests.push(request_id.to_string());
                } else if mime.contains("html") {
                    req.kind = RequestKind::Navigation;
                }
            }

            req.mime_type = mime_type;

            // If response sets cookies on an auth endpoint, flag it
            if req.has_set_cookie && req.kind == RequestKind::Auth {
                tracing::info!(url = %req.url, status, "auth endpoint set cookies");
            }
        }
    }

    /// Update response size when loading finishes.
    pub fn on_loading_finished(&mut self, request_id: &str, encoded_data_length: f64) {
        if let Some(req) = self.requests.get_mut(request_id) {
            req.response_size = Some(encoded_data_length as u64);
        }
    }

    /// Get a summary of captured traffic.
    pub fn summary(&self) -> NetworkSummary {
        let mut auth_count = 0u32;
        let mut api_count = 0u32;
        let mut nav_count = 0u32;
        let mut asset_count = 0u32;
        let mut other_count = 0u32;
        let mut total_bytes: u64 = 0;
        let mut auth_with_cookies = 0u32;

        for req in self.requests.values() {
            match req.kind {
                RequestKind::Auth => {
                    auth_count += 1;
                    if req.has_set_cookie {
                        auth_with_cookies += 1;
                    }
                }
                RequestKind::Api => api_count += 1,
                RequestKind::Navigation => nav_count += 1,
                RequestKind::Asset => asset_count += 1,
                RequestKind::WebSocket | RequestKind::Other => other_count += 1,
            }
            if let Some(size) = req.response_size {
                total_bytes += size;
            }
        }

        NetworkSummary {
            total_requests: self.requests.len() as u32,
            auth_count,
            api_count,
            navigation_count: nav_count,
            asset_count,
            other_count,
            total_bytes,
            auth_with_cookies,
        }
    }

    /// Get auth-related requests for debugging OAuth/login flows.
    pub fn auth_flow(&self) -> Vec<&CapturedRequest> {
        self.auth_requests
            .iter()
            .filter_map(|id| self.requests.get(id))
            .collect()
    }
}

/// Auth-related URL patterns.
const AUTH_PATTERNS: &[&str] = &[
    "/oauth",
    "/auth",
    "/login",
    "/signin",
    "/sign-in",
    "/sign_in",
    "/token",
    "/session",
    "/callback",
    "/authorize",
    "/sso",
    "/saml",
    "/api/auth",
    "/api/login",
    "/api/session",
    "accounts.google.com",
    "github.com/login",
    "login.microsoftonline.com",
    "facebook.com/dialog",
    "appleid.apple.com",
];

/// Asset file extension patterns.
const ASSET_EXTENSIONS: &[&str] = &[
    ".css", ".js", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".woff", ".woff2", ".ttf", ".eot",
    ".ico", ".webp", ".map", ".br", ".gz",
];

/// Extract the path component from a URL (after the host).
///
/// e.g. `"https://example.com/dashboard?q=1"` → `"/dashboard"`
fn extract_url_path(url: &str) -> &str {
    let no_query = url.split('?').next().unwrap_or(url);
    no_query
        .find("://")
        .and_then(|scheme_end| {
            let after_scheme = scheme_end + 3;
            no_query[after_scheme..]
                .find('/')
                .map(|slash| &no_query[after_scheme + slash..])
        })
        .unwrap_or("/")
}

/// Returns `true` if the URL matches a known auth-related pattern.
fn is_auth_url(url_lower: &str, method: &str) -> bool {
    if AUTH_PATTERNS.iter().any(|p| url_lower.contains(p)) {
        return true;
    }
    // POST to login-like path segments
    method == "POST"
        && url_lower.split('?').next().is_some_and(|path_str| {
            path_str.split('/').any(|p| {
                matches!(
                    p,
                    "login" | "signin" | "auth" | "token" | "register" | "signup"
                )
            })
        })
}

/// Classify a URL by its semantic purpose.
pub fn classify_url(url: &str, method: &str) -> RequestKind {
    let url_lower = url.to_lowercase();

    if url_lower.starts_with("ws://") || url_lower.starts_with("wss://") {
        return RequestKind::WebSocket;
    }
    if is_auth_url(&url_lower, method) {
        return RequestKind::Auth;
    }

    let url_no_query = url_lower.split('?').next().unwrap_or(&url_lower);
    if ASSET_EXTENSIONS
        .iter()
        .any(|ext| url_no_query.ends_with(ext))
    {
        return RequestKind::Asset;
    }
    if url_lower.contains("/api/")
        || url_lower.contains("/graphql")
        || url_lower.contains("/v1/")
        || url_lower.contains("/v2/")
    {
        return RequestKind::Api;
    }

    let url_path = extract_url_path(&url_lower);
    if method == "GET" && !url_path.contains('.') {
        return RequestKind::Navigation;
    }

    RequestKind::Other
}

/// Enable network tracking on a page handle.
///
/// Must be called before any network events will fire.
pub async fn enable_network_tracking(
    page: &dyn crate::engine::PageHandle,
) -> Result<(), crate::Error> {
    page.enable_network_monitoring().await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── classify_url tests ───────────────────────────────

    #[test]
    fn classify_oauth_url() {
        assert_eq!(
            classify_url("https://accounts.google.com/o/oauth2/auth", "GET"),
            RequestKind::Auth
        );
    }

    #[test]
    fn classify_login_post() {
        assert_eq!(
            classify_url("https://example.com/api/login", "POST"),
            RequestKind::Auth
        );
    }

    #[test]
    fn classify_token_endpoint() {
        assert_eq!(
            classify_url("https://example.com/oauth/token", "POST"),
            RequestKind::Auth
        );
    }

    #[test]
    fn classify_css_asset() {
        assert_eq!(
            classify_url("https://cdn.example.com/style.css", "GET"),
            RequestKind::Asset
        );
    }

    #[test]
    fn classify_js_asset() {
        assert_eq!(
            classify_url("https://cdn.example.com/app.js?v=123", "GET"),
            RequestKind::Asset
        );
    }

    #[test]
    fn classify_api_call() {
        assert_eq!(
            classify_url("https://example.com/api/users/me", "GET"),
            RequestKind::Api
        );
    }

    #[test]
    fn classify_graphql() {
        assert_eq!(
            classify_url("https://example.com/graphql", "POST"),
            RequestKind::Api
        );
    }

    #[test]
    fn classify_websocket() {
        assert_eq!(
            classify_url("wss://example.com/ws", "GET"),
            RequestKind::WebSocket
        );
    }

    #[test]
    fn classify_navigation() {
        assert_eq!(
            classify_url("https://example.com/dashboard", "GET"),
            RequestKind::Navigation
        );
    }

    #[test]
    fn classify_unknown() {
        assert_eq!(
            classify_url("https://example.com/something.unknown", "GET"),
            RequestKind::Other
        );
    }

    // ── NetworkCapture tests ─────────────────────────────

    #[test]
    fn capture_request_and_response() {
        let mut capture = NetworkCapture::new();
        capture.on_request(
            "1".into(),
            "https://example.com/api/login".into(),
            "POST".into(),
            Some("user=test".into()),
        );

        assert_eq!(capture.requests.len(), 1);
        assert_eq!(capture.auth_requests.len(), 1);

        let mut headers = HashMap::new();
        headers.insert("Set-Cookie".to_string(), "session=abc".to_string());
        capture.on_response("1", 200, Some("application/json".into()), &headers);

        let req = capture.requests.get("1").unwrap();
        assert_eq!(req.status, 200);
        assert!(req.has_set_cookie);
        assert_eq!(req.post_data.as_deref(), Some("user=test"));
    }

    #[test]
    fn capture_summary() {
        let mut capture = NetworkCapture::new();
        capture.on_request(
            "1".into(),
            "https://example.com/api/login".into(),
            "POST".into(),
            None,
        );
        capture.on_request(
            "2".into(),
            "https://example.com/style.css".into(),
            "GET".into(),
            None,
        );
        capture.on_request(
            "3".into(),
            "https://example.com/api/data".into(),
            "GET".into(),
            None,
        );

        let summary = capture.summary();
        assert_eq!(summary.total_requests, 3);
        assert_eq!(summary.auth_count, 1);
        assert_eq!(summary.asset_count, 1);
        assert_eq!(summary.api_count, 1);
    }

    #[test]
    fn reclassify_on_json_response() {
        let mut capture = NetworkCapture::new();
        // Use POST so the initial classification is Other (not Navigation)
        capture.on_request(
            "1".into(),
            "https://example.com/rpc".into(),
            "POST".into(),
            None,
        );

        assert_eq!(capture.requests.get("1").unwrap().kind, RequestKind::Other);

        capture.on_response("1", 200, Some("application/json".into()), &HashMap::new());
        assert_eq!(capture.requests.get("1").unwrap().kind, RequestKind::Api);
    }

    #[test]
    fn post_data_truncated() {
        let mut capture = NetworkCapture::new();
        let long_data = "x".repeat(2000);
        capture.on_request(
            "1".into(),
            "https://example.com/auth/login".into(),
            "POST".into(),
            Some(long_data),
        );

        let req = capture.requests.get("1").unwrap();
        assert_eq!(req.post_data.as_ref().unwrap().len(), 1024);
    }

    #[test]
    fn post_data_only_for_auth_api() {
        let mut capture = NetworkCapture::new();
        capture.on_request(
            "1".into(),
            "https://cdn.example.com/track.gif".into(),
            "POST".into(),
            Some("data=x".into()),
        );

        let req = capture.requests.get("1").unwrap();
        assert!(req.post_data.is_none()); // Asset, post data dropped
    }

    #[test]
    fn auth_flow_returns_ordered() {
        let mut capture = NetworkCapture::new();
        capture.on_request(
            "1".into(),
            "https://example.com/login".into(),
            "POST".into(),
            None,
        );
        capture.on_request(
            "2".into(),
            "https://example.com/oauth/callback".into(),
            "GET".into(),
            None,
        );

        let flow = capture.auth_flow();
        assert_eq!(flow.len(), 2);
        assert!(flow[0].url.contains("login"));
        assert!(flow[1].url.contains("callback"));
    }
}
