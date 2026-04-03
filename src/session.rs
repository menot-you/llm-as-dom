//! Session state: cookies, navigation history, and page memory across navigations.
//!
//! Tracks browser state across multiple page navigations within a pilot session,
//! including cookies, auth status, and per-page memory.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A browser cookie extracted from CDP or set by the application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieEntry {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Domain the cookie belongs to (e.g. `.example.com`).
    pub domain: String,
    /// URL path scope (e.g. `/` or `/app`).
    pub path: String,
    /// Unix timestamp (seconds). `0.0` means session cookie (no expiry).
    pub expires: f64,
    /// Whether the cookie requires HTTPS.
    pub secure: bool,
    /// Whether the cookie is inaccessible to JavaScript.
    pub http_only: bool,
    /// SameSite attribute (`Strict`, `Lax`, `None`), if set.
    #[serde(default)]
    pub same_site: Option<String>,
}

/// What happened on a particular page during the session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigationEntry {
    /// Full URL of the page.
    pub url: String,
    /// Document title.
    pub title: String,
    /// Unix timestamp in milliseconds when this page was visited.
    pub timestamp_ms: u64,
    /// Actions performed on this page (e.g. "typed email", "clicked login").
    pub actions_taken: Vec<String>,
    /// Whether a form was submitted on this page.
    pub form_submitted: bool,
    /// Whether authentication was detected (login form, OAuth redirect, etc.).
    pub auth_related: bool,
}

/// Authentication state tracked across the session.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthState {
    /// No authentication activity detected.
    #[default]
    None,
    /// Currently in an auth flow (login form found, OAuth redirect in progress).
    InProgress,
    /// Authentication completed (success page detected, token/cookie received).
    Authenticated,
    /// Authentication failed (error message detected).
    Failed,
}

/// Persistent state across page navigations within a pilot session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    /// All cookies accumulated during the session.
    pub cookies: Vec<CookieEntry>,
    /// Ordered history of pages visited.
    pub navigation_history: Vec<NavigationEntry>,
    /// Key-value memory for pages (keyed by URL pattern or identifier).
    pub page_memory: HashMap<String, serde_json::Value>,
    /// Current authentication state.
    pub auth_state: AuthState,
    /// The original URL the session started from (for redirect-back detection).
    pub origin_url: Option<String>,
}

/// Common auth-related cookie name patterns.
const AUTH_COOKIE_PATTERNS: &[&str] = &[
    "session",
    "token",
    "auth",
    "sid",
    "jwt",
    "access_token",
    "refresh_token",
    "id_token",
    "csrf",
    "xsrf",
];

impl SessionState {
    /// Create a new empty session state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or update a cookie. Replaces existing cookie with same
    /// `(name, domain, path)` triple.
    pub fn add_cookie(&mut self, cookie: CookieEntry) {
        if let Some(existing) = self
            .cookies
            .iter_mut()
            .find(|c| c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path)
        {
            *existing = cookie;
        } else {
            self.cookies.push(cookie);
        }
    }

    /// Return cookies whose domain and path match the given URL.
    ///
    /// Domain matching: a cookie with domain `.example.com` matches
    /// `sub.example.com` and `example.com`. A cookie with domain
    /// `example.com` (no leading dot) matches only `example.com` exactly.
    ///
    /// Path matching: cookie path `/foo` matches request path `/foo/bar`
    /// but not `/bar`.
    pub fn get_cookies_for_url(&self, url: &str) -> Vec<&CookieEntry> {
        let (host, path) = parse_host_path(url);

        self.cookies
            .iter()
            .filter(|c| domain_matches(&c.domain, &host) && path_matches(&c.path, &path))
            .collect()
    }

    /// Check whether any cookie name matches common auth-related patterns.
    pub fn has_auth_cookies(&self) -> bool {
        self.cookies.iter().any(|c| {
            let lower = c.name.to_lowercase();
            AUTH_COOKIE_PATTERNS.iter().any(|pat| lower.contains(pat))
        })
    }

    /// Record a page navigation with the current timestamp.
    pub fn record_navigation(
        &mut self,
        url: String,
        title: String,
        actions: Vec<String>,
        form_submitted: bool,
        auth_related: bool,
    ) {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        self.navigation_history.push(NavigationEntry {
            url,
            title,
            timestamp_ms,
            actions_taken: actions,
            form_submitted,
            auth_related,
        });
    }

    /// Store a value in page memory under the given key.
    pub fn remember(&mut self, key: String, value: serde_json::Value) {
        self.page_memory.insert(key, value);
    }

    /// Retrieve a value from page memory.
    pub fn recall(&self, key: &str) -> Option<&serde_json::Value> {
        self.page_memory.get(key)
    }

    /// The URL of the most recently visited page, if any.
    pub fn current_url(&self) -> Option<&str> {
        self.navigation_history.last().map(|e| e.url.as_str())
    }

    /// The URL of the page visited before the current one, if any.
    pub fn previous_url(&self) -> Option<&str> {
        let len = self.navigation_history.len();
        if len >= 2 {
            Some(self.navigation_history[len - 2].url.as_str())
        } else {
            None
        }
    }

    /// Whether the last navigation crossed domain boundaries (redirect detection).
    pub fn was_redirected(&self) -> bool {
        let len = self.navigation_history.len();
        if len < 2 {
            return false;
        }
        let prev_host = parse_host_path(&self.navigation_history[len - 2].url).0;
        let curr_host = parse_host_path(&self.navigation_history[len - 1].url).0;
        prev_host != curr_host
    }

    /// Count how many navigation entries contain the given URL pattern
    /// (substring match).
    pub fn visit_count(&self, url_pattern: &str) -> usize {
        self.navigation_history
            .iter()
            .filter(|e| e.url.contains(url_pattern))
            .count()
    }

    /// Remove cookies whose `expires` timestamp is in the past.
    /// Session cookies (expires == 0.0) are kept.
    pub fn clear_expired_cookies(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        self.cookies.retain(|c| c.expires == 0.0 || c.expires > now);
    }
}

// ---------------------------------------------------------------------------
// URL parsing helpers (no external `url` crate needed)
// ---------------------------------------------------------------------------

/// Extract host and path from a URL string.
/// Returns `("", "/")` on parse failure.
fn parse_host_path(url: &str) -> (String, String) {
    // Strip scheme: "https://foo.com/bar" -> "foo.com/bar"
    let without_scheme = url.find("://").map(|i| &url[i + 3..]).unwrap_or(url);

    // Split host from path at first '/'
    let (host_port, path) = match without_scheme.find('/') {
        Some(i) => (&without_scheme[..i], &without_scheme[i..]),
        None => (without_scheme, "/"),
    };

    // Strip port: "foo.com:8080" -> "foo.com"
    let host = host_port.split(':').next().unwrap_or(host_port);

    (host.to_lowercase(), path.to_string())
}

/// Cookie domain matching per RFC 6265 (simplified).
///
/// - Cookie `.example.com` matches `example.com` and `sub.example.com`.
/// - Cookie `example.com` matches `example.com` only.
fn domain_matches(cookie_domain: &str, request_host: &str) -> bool {
    let cd = cookie_domain.to_lowercase();
    let rh = request_host.to_lowercase();

    if let Some(bare) = cd.strip_prefix('.') {
        // ".example.com" matches "example.com" and "sub.example.com"
        rh == bare || rh.ends_with(&cd) // ends with ".example.com"
    } else {
        // Exact match
        rh == cd
    }
}

/// Cookie path matching: cookie path `/foo` matches request `/foo`, `/foo/`,
/// and `/foo/bar`, but not `/foobar` or `/bar`.
fn path_matches(cookie_path: &str, request_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if let Some(after) = request_path.strip_prefix(cookie_path) {
        // Ensure we match at a boundary: /foo matches /foo/bar but not /foobar
        return cookie_path.ends_with('/') || after.starts_with('/');
    }
    false
}

// ---------------------------------------------------------------------------
// CDP cookie integration
// ---------------------------------------------------------------------------

/// Extract cookies from the browser via the page handle's cookie API.
///
/// This captures non-httpOnly cookies via JavaScript `document.cookie`.
pub async fn extract_cookies_cdp(
    page: &dyn crate::engine::PageHandle,
) -> Result<Vec<CookieEntry>, crate::Error> {
    page.cookies().await
}

/// Inject cookies into the browser via the page handle's cookie API.
///
/// Note: httpOnly cookies cannot be set via JavaScript. This covers the
/// common case of setting session/auth cookies for testing.
pub async fn inject_cookies_cdp(
    page: &dyn crate::engine::PageHandle,
    cookies: &[CookieEntry],
) -> Result<(), crate::Error> {
    page.set_cookies(cookies).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cookie(name: &str, domain: &str, path: &str) -> CookieEntry {
        CookieEntry {
            name: name.to_string(),
            value: "val".to_string(),
            domain: domain.to_string(),
            path: path.to_string(),
            expires: 0.0,
            secure: false,
            http_only: false,
            same_site: None,
        }
    }

    #[test]
    fn test_cookie_upsert() {
        let mut state = SessionState::new();
        let c1 = CookieEntry {
            value: "old".to_string(),
            ..make_cookie("sid", ".example.com", "/")
        };
        let c2 = CookieEntry {
            value: "new".to_string(),
            ..make_cookie("sid", ".example.com", "/")
        };

        state.add_cookie(c1);
        assert_eq!(state.cookies.len(), 1);
        assert_eq!(state.cookies[0].value, "old");

        state.add_cookie(c2);
        assert_eq!(state.cookies.len(), 1);
        assert_eq!(state.cookies[0].value, "new");
    }

    #[test]
    fn test_cookie_domain_matching() {
        let mut state = SessionState::new();
        state.add_cookie(make_cookie("a", ".example.com", "/"));
        state.add_cookie(make_cookie("b", "other.com", "/"));

        // .example.com matches sub.example.com
        let matched = state.get_cookies_for_url("https://sub.example.com/page");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].name, "a");

        // .example.com also matches example.com itself
        let matched2 = state.get_cookies_for_url("https://example.com/");
        assert_eq!(matched2.len(), 1);
        assert_eq!(matched2[0].name, "a");
    }

    #[test]
    fn test_cookie_path_matching() {
        let mut state = SessionState::new();
        state.add_cookie(make_cookie("a", "example.com", "/foo"));
        state.add_cookie(make_cookie("b", "example.com", "/bar"));

        // /foo matches /foo/bar
        let matched = state.get_cookies_for_url("https://example.com/foo/bar");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].name, "a");

        // /foo does NOT match /bar
        let matched2 = state.get_cookies_for_url("https://example.com/bar");
        assert_eq!(matched2.len(), 1);
        assert_eq!(matched2[0].name, "b");
    }

    #[test]
    fn test_has_auth_cookies() {
        let mut state = SessionState::new();
        assert!(!state.has_auth_cookies());

        state.add_cookie(make_cookie("theme", "example.com", "/"));
        assert!(!state.has_auth_cookies());

        state.add_cookie(make_cookie("session_id", "example.com", "/"));
        assert!(state.has_auth_cookies());
    }

    #[test]
    fn test_navigation_history() {
        let mut state = SessionState::new();
        state.record_navigation(
            "https://a.com".into(),
            "Page A".into(),
            vec!["clicked button".into()],
            false,
            false,
        );
        state.record_navigation("https://b.com".into(), "Page B".into(), vec![], true, true);

        assert_eq!(state.navigation_history.len(), 2);
        assert_eq!(state.navigation_history[0].url, "https://a.com");
        assert!(state.navigation_history[1].form_submitted);
        assert!(state.navigation_history[1].auth_related);
        // Timestamps should be monotonically increasing
        assert!(
            state.navigation_history[0].timestamp_ms <= state.navigation_history[1].timestamp_ms
        );
    }

    #[test]
    fn test_page_memory() {
        let mut state = SessionState::new();
        assert!(state.recall("key").is_none());

        state.remember("key".into(), serde_json::json!({"email": "a@b.com"}));
        let val = state.recall("key").expect("should exist");
        assert_eq!(val["email"], "a@b.com");

        // Overwrite
        state.remember("key".into(), serde_json::json!("updated"));
        assert_eq!(state.recall("key").unwrap(), &serde_json::json!("updated"));
    }

    #[test]
    fn test_was_redirected() {
        let mut state = SessionState::new();
        assert!(!state.was_redirected()); // no history

        state.record_navigation("https://a.com".into(), "A".into(), vec![], false, false);
        assert!(!state.was_redirected()); // only one entry

        state.record_navigation(
            "https://b.com/callback".into(),
            "B".into(),
            vec![],
            false,
            false,
        );
        assert!(state.was_redirected()); // different domains

        state.record_navigation(
            "https://b.com/dashboard".into(),
            "B2".into(),
            vec![],
            false,
            false,
        );
        assert!(!state.was_redirected()); // same domain
    }

    #[test]
    fn test_visit_count() {
        let mut state = SessionState::new();
        state.record_navigation(
            "https://example.com/login".into(),
            "Login".into(),
            vec![],
            false,
            true,
        );
        state.record_navigation(
            "https://example.com/dashboard".into(),
            "Dash".into(),
            vec![],
            false,
            false,
        );
        state.record_navigation(
            "https://example.com/login".into(),
            "Login".into(),
            vec![],
            false,
            true,
        );

        assert_eq!(state.visit_count("login"), 2);
        assert_eq!(state.visit_count("dashboard"), 1);
        assert_eq!(state.visit_count("example.com"), 3);
        assert_eq!(state.visit_count("nonexistent"), 0);
    }

    #[test]
    fn test_clear_expired_cookies() {
        let mut state = SessionState::new();

        // Session cookie (expires=0, should be kept)
        state.add_cookie(make_cookie("session", "example.com", "/"));

        // Expired cookie (timestamp in the past)
        let mut expired = make_cookie("old", "example.com", "/");
        expired.expires = 1000.0; // way in the past
        state.add_cookie(expired);

        // Future cookie
        let mut future = make_cookie("future", "example.com", "/");
        future.expires = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64())
            + 86400.0;
        state.add_cookie(future);

        assert_eq!(state.cookies.len(), 3);
        state.clear_expired_cookies();
        assert_eq!(state.cookies.len(), 2);

        let names: Vec<&str> = state.cookies.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"session"));
        assert!(names.contains(&"future"));
        assert!(!names.contains(&"old"));
    }

    #[test]
    fn test_current_and_previous_url() {
        let mut state = SessionState::new();
        assert!(state.current_url().is_none());
        assert!(state.previous_url().is_none());

        state.record_navigation("https://a.com".into(), "A".into(), vec![], false, false);
        assert_eq!(state.current_url(), Some("https://a.com"));
        assert!(state.previous_url().is_none());

        state.record_navigation("https://b.com".into(), "B".into(), vec![], false, false);
        assert_eq!(state.current_url(), Some("https://b.com"));
        assert_eq!(state.previous_url(), Some("https://a.com"));
    }

    #[test]
    fn test_session_default() {
        let state = SessionState::default();
        assert!(state.cookies.is_empty());
        assert!(state.navigation_history.is_empty());
        assert!(state.page_memory.is_empty());
        assert_eq!(state.auth_state, AuthState::None);
        assert!(state.origin_url.is_none());
    }

    #[test]
    fn test_cookie_for_url_no_match() {
        let mut state = SessionState::new();
        state.add_cookie(make_cookie("sid", "other.com", "/"));

        let matched = state.get_cookies_for_url("https://example.com/page");
        assert!(matched.is_empty());
    }

    // -----------------------------------------------------------------------
    // Internal helper tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_host_path_basic() {
        let (h, p) = parse_host_path("https://example.com/foo/bar");
        assert_eq!(h, "example.com");
        assert_eq!(p, "/foo/bar");
    }

    #[test]
    fn test_parse_host_path_with_port() {
        let (h, p) = parse_host_path("http://localhost:3000/api");
        assert_eq!(h, "localhost");
        assert_eq!(p, "/api");
    }

    #[test]
    fn test_parse_host_path_no_path() {
        let (h, p) = parse_host_path("https://example.com");
        assert_eq!(h, "example.com");
        assert_eq!(p, "/");
    }

    #[test]
    fn test_domain_matches_exact() {
        assert!(domain_matches("example.com", "example.com"));
        assert!(!domain_matches("example.com", "sub.example.com"));
    }

    #[test]
    fn test_domain_matches_wildcard() {
        assert!(domain_matches(".example.com", "example.com"));
        assert!(domain_matches(".example.com", "sub.example.com"));
        assert!(!domain_matches(".example.com", "notexample.com"));
    }

    #[test]
    fn test_path_matches_exact_and_prefix() {
        assert!(path_matches("/foo", "/foo"));
        assert!(path_matches("/foo", "/foo/bar"));
        assert!(!path_matches("/foo", "/foobar"));
        assert!(!path_matches("/foo", "/bar"));
        assert!(path_matches("/", "/anything"));
    }
}
