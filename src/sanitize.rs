//! Steganographic prompt-injection defense.
//!
//! Strips invisible Unicode characters that adversarial pages use to embed
//! hidden instructions in DOM text, validates navigation URLs against SSRF,
//! and masks sensitive form values.

/// Strip characters commonly used for steganographic prompt injection.
///
/// Removes zero-width joiners, bidi overrides, Unicode tag characters,
/// variation selectors, and other invisible formatters that can carry
/// hidden payloads through DOM extraction into LLM prompts.
pub fn sanitize_text(input: &str) -> String {
    input.chars().filter(|c| !is_steganographic(*c)).collect()
}

/// Returns `true` for Unicode code points used in steganographic attacks.
fn is_steganographic(c: char) -> bool {
    matches!(c,
        // Zero-width characters
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{2060}' |
        '\u{2061}' | '\u{2062}' | '\u{2063}' | '\u{2064}' |
        // Bidi overrides (text direction manipulation)
        '\u{200E}' | '\u{200F}' |
        '\u{202A}' | '\u{202B}' | '\u{202C}' | '\u{202D}' | '\u{202E}' |
        '\u{2066}' | '\u{2067}' | '\u{2068}' | '\u{2069}' |
        // Unicode tag block (encode hidden ASCII text)
        '\u{E0001}'..='\u{E007F}' |
        // Soft hyphen, combining grapheme joiner, Arabic letter mark
        '\u{00AD}' | '\u{034F}' | '\u{061C}' |
        // Variation selectors (encode data via glyph variants)
        '\u{FE00}'..='\u{FE0F}'
    )
}

/// FIX-R4-03: Broadened sensitive field name patterns.
/// Covers OTP, CVV, API key, auth code, MFA, and other credential-adjacent fields.
const SENSITIVE_NAME_PATTERNS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "otp",
    "totp",
    "cvv",
    "cvc",
    "pin",
    "api_key",
    "apikey",
    "auth_code",
    "verification",
    "security_code",
    "mfa",
    "2fa",
];

/// Mask sensitive field values extracted from the DOM.
///
/// Prevents credentials from leaking into LLM prompts.
/// FIX-10: Also checks element `name` for sensitive patterns, not just `type`.
/// FIX-R4-03: Broadened to cover OTP, CVV, API key, MFA, etc.
pub fn mask_sensitive_value(
    input_type: Option<&str>,
    name: Option<&str>,
    value: Option<&str>,
) -> Option<String> {
    let is_sensitive = input_type.is_some_and(|t| t.eq_ignore_ascii_case("password"))
        || name.is_some_and(|n| {
            let lower = n.to_lowercase();
            SENSITIVE_NAME_PATTERNS
                .iter()
                .any(|pat| lower.contains(pat))
        });
    if is_sensitive {
        value.map(|_| "[filled]".to_string())
    } else {
        value.map(String::from)
    }
}

/// Schemes that must never be navigated to.
///
/// Wave 1: Extended to cover browser-internal URLs (chrome:, opera:, about:,
/// devtools:, view-source:, edge:, brave:) and raw WebSocket protocols
/// (ws:, wss:). These paths let an agent reach settings pages, extensions,
/// internal devtools, and raw sockets that should never be driven by MCP.
const BLOCKED_SCHEMES: &[&str] = &[
    "file:",
    "javascript:",
    "data:",
    "blob:",
    "vbscript:",
    "chrome:",
    "chrome-extension:",
    "opera:",
    "about:",
    "devtools:",
    "view-source:",
    "edge:",
    "brave:",
    "ws:",
    "wss:",
];

/// Check whether a URL is safe for automated navigation.
///
/// FIX-2: Deny-by-default on parse failure. Strips control chars before
/// scheme check so `java\x0Bscript:` is caught. Only allows unparseable
/// URLs that look like relative paths (no scheme-like prefix).
///
///// Check whether the SSRF bypass escape hatch is currently active.
///
/// Returns `true` only when BOTH conditions hold:
/// 1. The build is compiled with `debug_assertions` (i.e. debug/dev build,
///    never a `--release` binary that ships to users).
/// 2. The `LAD_EVAL_BYPASS_SSRF` environment variable is set to `1`.
///
/// This is a defense-in-depth gate against the footgun where a production
/// operator accidentally inherits the env var from a parent shell, a
/// dotfile, or a supply-chain compromise and silently disables SSRF
/// protection. In release builds the env var is a no-op.
///
/// When the bypass is active, the caller of `is_safe_url` emits an
/// `ERROR`-level tracing event on every bypass decision so it is
/// auditable in logs.
fn ssrf_bypass_active() -> bool {
    #[cfg(debug_assertions)]
    {
        std::env::var("LAD_EVAL_BYPASS_SSRF").ok().as_deref() == Some("1")
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

/// FIX-14: Blocks known DNS rebinding hostnames (nip.io, sslip.io, etc.)
/// and documents the limitation that async DNS resolution is needed for
/// full rebinding protection in production deployments.
pub fn is_safe_url(url: &str) -> bool {
    // PERF-P5: Use Cow — avoid allocation when URL has no control chars (common case).
    let cleaned: std::borrow::Cow<'_, str> = if url.chars().any(|c| c.is_control()) {
        std::borrow::Cow::Owned(url.chars().filter(|c| !c.is_control()).collect())
    } else {
        std::borrow::Cow::Borrowed(url)
    };
    let lower = cleaned.trim().to_lowercase();

    // Block dangerous schemes even on raw string (before parsing).
    for scheme in BLOCKED_SCHEMES {
        if lower.starts_with(scheme) {
            return false;
        }
    }

    // Authoritative check: parse the URL and inspect the scheme + host.
    match url::Url::parse(url) {
        Ok(parsed) => {
            // Block dangerous schemes (catches edge cases the prefix missed).
            let scheme_with_colon = format!("{}:", parsed.scheme());
            if BLOCKED_SCHEMES.contains(&scheme_with_colon.as_str()) {
                return false;
            }
            // Check for private/loopback hosts (SSRF targets).
            if let Some(host) = parsed.host_str() {
                let bypass = ssrf_bypass_active();
                if is_suspicious_hostname(host) && !bypass {
                    return false;
                }
                if is_private_host(host) && !bypass {
                    return false;
                }
                if bypass {
                    tracing::error!(
                        host = %host,
                        "LAD_EVAL_BYPASS_SSRF active — allowing private/loopback host. \
                         This MUST NOT be used in production."
                    );
                }
                return true;
            }
            // No host = relative URL, allow
            true
        }
        Err(_) => {
            // FIX-2: Unparseable — only allow if it looks like a relative path
            // (no scheme-like prefix). Deny by default.
            !lower.contains("://") && !lower.contains(':')
        }
    }
}

/// Returns `true` if the host resolves to a private, loopback, or
/// link-local address (SSRF targets).
///
/// FIX-3: Covers IPv6 unique-local (fc00::/7), link-local (fe80::/10),
/// IPv4-mapped (::ffff:x.x.x.x), and unspecified (::) addresses.
fn is_private_host(host: &str) -> bool {
    // Strip brackets from IPv6 addresses (url crate returns e.g. "[::1]")
    let bare = host.trim_start_matches('[').trim_end_matches(']');

    // Explicit localhost variants
    if bare == "localhost" || bare == "127.0.0.1" || bare == "::1" || bare == "0.0.0.0" {
        return true;
    }
    // AWS IMDS endpoint
    if bare == "169.254.169.254" {
        return true;
    }
    // Parse as IP and check ranges
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback()                             // ::1
                || v6.is_unspecified()                       // ::
                || (v6.segments()[0] & 0xfe00) == 0xfc00     // fc00::/7 (unique local)
                || (v6.segments()[0] & 0xffc0) == 0xfe80     // fe80::/10 (link-local)
                || is_ipv4_mapped_private(v6) // ::ffff:127.0.0.1 etc.
            }
        };
    }
    false
}

/// Check if an IPv6 address is an IPv4-mapped address (::ffff:x.x.x.x)
/// pointing to a private/loopback/link-local IPv4 address.
fn is_ipv4_mapped_private(v6: std::net::Ipv6Addr) -> bool {
    let s = v6.segments();
    // ::ffff:x.x.x.x format: first 5 segments are 0, segment 5 is 0xffff
    if s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0xffff {
        let mapped =
            std::net::Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
        return mapped.is_private() || mapped.is_loopback() || mapped.is_link_local();
    }
    false
}

/// FIX-14: Detect known DNS rebinding hostnames that resolve to private IPs
/// but pass hostname string checks.
///
/// NOTE: This is a best-effort blocklist. For production deployments,
/// network-level egress filtering (firewall rules blocking RFC1918/loopback
/// destinations) is the recommended defense against DNS rebinding, since
/// attackers can register arbitrary domains resolving to private IPs.
/// Full protection requires async DNS resolution + re-checking the resolved
/// IP, which is expensive and not done here.
fn is_suspicious_hostname(host: &str) -> bool {
    let lower = host.to_lowercase();
    // Exact matches
    if lower == "localhost"
        || lower == "localtest.me"
        || lower == "lvh.me"
        || lower == "nip.io"
        || lower == "sslip.io"
        || lower == "xip.io"
    {
        return true;
    }
    // Subdomain matches
    lower.ends_with(".nip.io")
        || lower.ends_with(".sslip.io")
        || lower.ends_with(".localtest.me")
        || lower.ends_with(".lvh.me")
        || lower.ends_with(".xip.io")
        || lower.ends_with(".localhost")
}

/// FIX-4: Validate that an upload file path is within allowed roots.
///
/// Default allowed roots: current working directory, `/tmp/`, and the OS
/// temp directory. The `LAD_UPLOAD_ROOT` env var adds a custom root.
/// Rejects paths outside allowed roots to prevent uploading `/etc/passwd`,
/// SSH keys, or other sensitive files to attacker-controlled pages.
pub fn is_safe_upload_path(path: &std::path::Path) -> bool {
    let canonical = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => return false,
    };

    // Build allowed roots, canonicalizing each to resolve symlinks
    // (e.g. /tmp -> /private/tmp on macOS).
    let raw_roots = [
        Some(std::path::PathBuf::from("/tmp")),
        Some(std::env::temp_dir()),
        std::env::current_dir().ok(),
    ];

    for raw in raw_roots.iter().flatten() {
        if let Ok(resolved) = raw.canonicalize()
            && canonical.starts_with(&resolved)
        {
            return true;
        }
    }

    // Custom root from env var
    if let Ok(custom_root) = std::env::var("LAD_UPLOAD_ROOT")
        && let Ok(resolved) = std::path::Path::new(&custom_root).canonicalize()
        && canonical.starts_with(&resolved)
    {
        return true;
    }

    false
}

/// FIX-R4-02: Redact sensitive query parameters from URLs.
///
/// Strips OAuth codes, tokens, magic links, API keys, and other secrets
/// that would otherwise leak into SemanticView prompts, session history,
/// and pilot step logs.
pub fn redact_url_secrets(url: &str) -> String {
    if let Ok(mut parsed) = url::Url::parse(url) {
        let has_query = parsed.query().is_some_and(|q| !q.is_empty());
        if !has_query {
            // Strip fragment unconditionally (may contain tokens in SPA auth flows)
            parsed.set_fragment(None);
            return parsed.to_string();
        }

        const SENSITIVE_KEYS: &[&str] = &[
            "token",
            "code",
            "key",
            "secret",
            "password",
            "auth",
            "access_token",
            "refresh_token",
            "api_key",
            "session",
            "otp",
            "magic",
            "reset",
            "confirm",
        ];

        let filtered: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(k, v)| {
                let lower_k = k.to_lowercase();
                if SENSITIVE_KEYS.iter().any(|s| lower_k.contains(s)) {
                    (k.into_owned(), "[REDACTED]".to_string())
                } else {
                    (k.into_owned(), v.into_owned())
                }
            })
            .collect();

        parsed.query_pairs_mut().clear();
        for (k, v) in &filtered {
            parsed.query_pairs_mut().append_pair(k, v);
        }
        // Strip fragment (may contain tokens in SPA auth flows)
        parsed.set_fragment(None);
        parsed.to_string()
    } else {
        url.to_string()
    }
}

/// PERF-P1: Compiled regex for redacting Type action values.
static RE_ACTION_DEBUG: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#"Type \{ element: (\d+), value: "[^"]*""#).expect("valid regex")
});

/// PERF-P1: Compiled regex for redacting credentials from goal strings.
static RE_CREDENTIALS_GOAL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)(password|passwd|secret|token|otp|pin|key|pass|pw)\s+(\S+)")
        .expect("valid regex")
});

/// Wave 3: inverse-SSRF check for CDP attach URLs.
///
/// Returns `true` ONLY when the URL points to a loopback host
/// (`localhost`, `127.0.0.1`, `::1`). Rejects anything else — including
/// RFC1918 private ranges (`192.168.*`, `10.*`, `172.16-31.*`),
/// link-local (`169.254.*`), and public hosts. The caller is expected
/// to pass a resolved CDP endpoint (either an `http://` debug endpoint
/// or a `ws://` WebSocket URL) to this function before calling
/// `chromiumoxide::Browser::connect`.
///
/// The contract is the *inverse* of [`is_safe_url`]: SSRF protection
/// normally blocks loopback as the target and allows public hosts,
/// but here we want to connect to the user's LOCAL Chrome ONLY. Any
/// remote host on this code path would let a malicious MCP client
/// point LAD at an attacker-controlled CDP endpoint — a full RCE
/// through the debug protocol. Hence loopback-only.
///
/// Unparseable URLs, URLs without a host component, and malformed
/// schemes all return `false` (deny by default).
pub fn is_loopback_only(url: &str) -> bool {
    // Strip control chars defensively — matches `is_safe_url`'s approach.
    let cleaned: String = url.chars().filter(|c| !c.is_control()).collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return false;
    }

    let parsed = match url::Url::parse(trimmed) {
        Ok(u) => u,
        Err(_) => return false,
    };

    // Scheme must be one of the four we care about: ws/wss/http/https.
    match parsed.scheme() {
        "ws" | "wss" | "http" | "https" => {}
        _ => return false,
    }

    let host = match parsed.host_str() {
        Some(h) => h,
        None => return false,
    };
    // `url` keeps IPv6 brackets in `host_str` — strip them for the
    // literal equality checks below.
    let bare = host.trim_start_matches('[').trim_end_matches(']');

    // Explicit loopback hostnames — the ONLY values we accept.
    if bare.eq_ignore_ascii_case("localhost") || bare == "127.0.0.1" || bare == "::1" {
        return true;
    }

    // Any parsed IP that is loopback in IPv4 or IPv6 counts (handles
    // weird forms like `127.0.0.2`, `0:0:0:0:0:0:0:1`, etc.). Private
    // and link-local IPs explicitly DO NOT count — this is stricter
    // than `is_private_host`.
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback(),
            std::net::IpAddr::V6(v6) => v6.is_loopback(),
        };
    }

    false
}

/// FIX-2: Redact `Action::Type` values in serialized output.
///
/// When rendering pilot actions for the MCP response, masks the typed
/// value so passwords and secrets are not leaked to the caller.
/// PERF-P1: Uses pre-compiled regex via LazyLock.
pub fn redact_action_debug(action_debug: &str) -> String {
    RE_ACTION_DEBUG
        .replace_all(action_debug, r#"Type { element: $1, value: "[REDACTED]""#)
        .into_owned()
}

/// FIX-6: Redact credential values from goal strings before storage.
///
/// Replaces values following credential keywords (password, passwd, secret,
/// token, otp, pin, key) with `[REDACTED]`. Handles both quoted and unquoted values.
/// PERF-P1: Uses pre-compiled regex via LazyLock.
pub fn redact_credentials_from_goal(goal: &str) -> String {
    RE_CREDENTIALS_GOAL
        .replace_all(goal, "$1 [REDACTED]")
        .into_owned()
}

/// Generate a cryptographically random 32-character hex string for prompt boundaries.
///
/// FIX-R3-06: Uses `getrandom` (CSPRNG) instead of `RandomState` + system time,
/// which was not cryptographically secure and could be predicted.
pub fn random_boundary() -> String {
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf).expect("failed to get random bytes from OS CSPRNG");
    buf.iter().map(|b| format!("{b:02x}")).collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── sanitize_text ───────────────────────────────────────

    #[test]
    fn strips_zero_width_chars() {
        let input = "hello\u{200B}\u{200C}\u{200D}world";
        assert_eq!(sanitize_text(input), "helloworld");
    }

    #[test]
    fn strips_bidi_overrides() {
        let input = "click \u{202E}ereh\u{202C} now";
        assert_eq!(sanitize_text(input), "click ereh now");
    }

    #[test]
    fn strips_unicode_tags() {
        // U+E0001 (language tag) + U+E0041..U+E005A encode hidden "AZ"
        let input = "visible\u{E0001}\u{E0041}\u{E005A}text";
        assert_eq!(sanitize_text(input), "visibletext");
    }

    #[test]
    fn strips_variation_selectors() {
        let input = "emoji\u{FE0F}\u{FE00}text";
        assert_eq!(sanitize_text(input), "emojitext");
    }

    #[test]
    fn strips_soft_hyphen_and_friends() {
        let input = "soft\u{00AD}hyphen\u{034F}join\u{061C}mark";
        assert_eq!(sanitize_text(input), "softhyphenjoinmark");
    }

    #[test]
    fn preserves_normal_text() {
        let input = "Hello, world! 日本語 café 🚀";
        assert_eq!(sanitize_text(input), input);
    }

    #[test]
    fn handles_empty_string() {
        assert_eq!(sanitize_text(""), "");
    }

    #[test]
    fn mixed_steganographic_and_normal() {
        let input = "Ig\u{200B}no\u{FEFF}re \u{200D}all\u{2060} prev";
        assert_eq!(sanitize_text(input), "Ignore all prev");
    }

    // ── mask_sensitive_value ────────────────────────────────

    #[test]
    fn masks_password_field() {
        assert_eq!(
            mask_sensitive_value(Some("password"), None, Some("s3cret")),
            Some("[filled]".to_string()),
        );
    }

    #[test]
    fn preserves_text_field() {
        assert_eq!(
            mask_sensitive_value(Some("text"), None, Some("hello")),
            Some("hello".to_string()),
        );
    }

    #[test]
    fn preserves_none_value() {
        assert_eq!(mask_sensitive_value(Some("password"), None, None), None);
    }

    #[test]
    fn no_type_preserves_value() {
        assert_eq!(
            mask_sensitive_value(None, None, Some("data")),
            Some("data".to_string()),
        );
    }

    // FIX-10: Name-based masking
    #[test]
    fn masks_by_name_password() {
        assert_eq!(
            mask_sensitive_value(Some("text"), Some("password"), Some("s3cret")),
            Some("[filled]".to_string()),
        );
    }

    #[test]
    fn masks_by_name_passwd() {
        assert_eq!(
            mask_sensitive_value(Some("text"), Some("user_passwd"), Some("s3cret")),
            Some("[filled]".to_string()),
        );
    }

    #[test]
    fn masks_by_name_passwd_confirm() {
        // Edge case: password confirmation field with type="text" and name containing "passwd"
        assert_eq!(
            mask_sensitive_value(Some("text"), Some("passwd_confirm"), Some("s3cret")),
            Some("[filled]".to_string()),
        );
    }

    #[test]
    fn masks_by_name_secret() {
        assert_eq!(
            mask_sensitive_value(None, Some("api_secret"), Some("s3cret")),
            Some("[filled]".to_string()),
        );
    }

    #[test]
    fn does_not_mask_normal_name() {
        assert_eq!(
            mask_sensitive_value(Some("text"), Some("username"), Some("alice")),
            Some("alice".to_string()),
        );
    }

    // ── is_safe_url ────────────────────────────────────────

    #[test]
    fn blocks_file_scheme() {
        assert!(!is_safe_url("file:///etc/passwd"));
    }

    #[test]
    fn blocks_javascript_scheme() {
        assert!(!is_safe_url("javascript:alert(1)"));
    }

    #[test]
    fn blocks_data_scheme() {
        assert!(!is_safe_url("data:text/html,<h1>hi</h1>"));
    }

    #[test]
    fn blocks_blob_scheme() {
        assert!(!is_safe_url("blob:http://example.com/abc"));
    }

    // ── Wave 1: extended BLOCKED_SCHEMES — browser-internal + ws ──

    #[test]
    fn blocks_chrome_scheme() {
        assert!(!is_safe_url("chrome://settings"));
        assert!(!is_safe_url("chrome://flags"));
    }

    #[test]
    fn blocks_chrome_extension_scheme() {
        assert!(!is_safe_url(
            "chrome-extension://abcdefghijklmnopqrstuvwxyz/index.html"
        ));
    }

    #[test]
    fn blocks_opera_scheme() {
        assert!(!is_safe_url("opera://settings"));
    }

    #[test]
    fn blocks_about_blank() {
        assert!(!is_safe_url("about:blank"));
        assert!(!is_safe_url("about:config"));
    }

    #[test]
    fn blocks_devtools() {
        assert!(!is_safe_url("devtools://devtools/bundled/inspector.html"));
    }

    #[test]
    fn blocks_view_source() {
        assert!(!is_safe_url("view-source:https://example.com"));
    }

    #[test]
    fn blocks_edge_scheme() {
        assert!(!is_safe_url("edge://settings"));
    }

    #[test]
    fn blocks_brave_scheme() {
        assert!(!is_safe_url("brave://wallet"));
    }

    #[test]
    fn blocks_ws_scheme() {
        assert!(!is_safe_url("ws://example.com:9000/socket"));
    }

    #[test]
    fn blocks_wss_scheme() {
        assert!(!is_safe_url("wss://example.com/socket"));
    }

    #[test]
    fn blocks_localhost() {
        assert!(!is_safe_url("http://localhost:8080/admin"));
    }

    #[test]
    fn blocks_127_0_0_1() {
        assert!(!is_safe_url("http://127.0.0.1:3000"));
    }

    #[test]
    fn blocks_ipv6_loopback() {
        assert!(!is_safe_url("http://[::1]/secret"));
    }

    #[test]
    fn blocks_private_10_range() {
        assert!(!is_safe_url("http://10.0.0.1/internal"));
    }

    #[test]
    fn blocks_private_172_range() {
        assert!(!is_safe_url("http://172.16.0.1/internal"));
    }

    #[test]
    fn blocks_private_192_range() {
        assert!(!is_safe_url("http://192.168.1.1/router"));
    }

    #[test]
    fn blocks_aws_imds() {
        assert!(!is_safe_url("http://169.254.169.254/latest/meta-data/"));
    }

    #[test]
    fn blocks_link_local() {
        assert!(!is_safe_url("http://169.254.1.1/"));
    }

    #[test]
    fn allows_https() {
        assert!(is_safe_url("https://example.com/page"));
    }

    #[test]
    fn allows_http() {
        assert!(is_safe_url("http://example.com/page"));
    }

    #[test]
    fn allows_relative_url() {
        assert!(is_safe_url("/dashboard"));
    }

    #[test]
    fn case_insensitive_scheme_block() {
        assert!(!is_safe_url("JAVASCRIPT:alert(1)"));
        assert!(!is_safe_url("File:///etc/shadow"));
    }

    #[test]
    fn blocks_file_single_slash() {
        // FIX-1: `file:/etc/passwd` (single slash) must be blocked
        assert!(!is_safe_url("file:/etc/passwd"));
        assert!(!is_safe_url("FILE:/etc/shadow"));
    }

    #[test]
    fn blocks_file_no_authority() {
        // Various file: scheme edge cases
        assert!(!is_safe_url("file:///tmp/secret"));
        assert!(!is_safe_url("file://localhost/etc/passwd"));
    }

    // ── mask_sensitive_value (case-insensitive) ────────────

    #[test]
    fn masks_password_field_uppercase() {
        assert_eq!(
            mask_sensitive_value(Some("PASSWORD"), None, Some("s3cret")),
            Some("[filled]".to_string()),
        );
    }

    #[test]
    fn masks_password_field_mixed_case() {
        assert_eq!(
            mask_sensitive_value(Some("Password"), None, Some("s3cret")),
            Some("[filled]".to_string()),
        );
    }

    // ── random_boundary ────────────────────────────────────

    #[test]
    fn boundary_is_32_hex_chars() {
        let b = random_boundary();
        assert_eq!(b.len(), 32);
        assert!(b.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn boundaries_are_unique() {
        let a = random_boundary();
        let b = random_boundary();
        assert_ne!(a, b);
    }

    // ── FIX-2: deny-by-default on parse failure ───────────

    #[test]
    fn blocks_javascript_with_control_chars() {
        // java\x0Bscript:alert(1) — vertical tab bypasses naive prefix check
        assert!(!is_safe_url("java\x0Bscript:alert(1)"));
    }

    #[test]
    fn blocks_vbscript() {
        assert!(!is_safe_url("vbscript:msgbox(1)"));
    }

    #[test]
    fn blocks_unparseable_with_colon_prefix() {
        // Starts with `:` — not a valid relative path, deny by default
        assert!(!is_safe_url(":some-stuff"));
    }

    #[test]
    fn blocks_javascript_with_whitespace_bypass() {
        // Null bytes / control chars stripped before scheme check
        assert!(!is_safe_url("java\x00script:alert(1)"));
        assert!(!is_safe_url("java\tscript:alert(1)"));
    }

    #[test]
    fn allows_relative_path_no_scheme() {
        assert!(is_safe_url("/dashboard"));
        assert!(is_safe_url("about"));
    }

    // ── FIX-3: IPv6 SSRF bypass ──────────────────────────

    #[test]
    fn blocks_ipv6_unique_local() {
        // fd00::/7 — unique local address
        assert!(!is_safe_url("http://[fd12::1]/secret"));
    }

    #[test]
    fn blocks_ipv6_link_local() {
        // fe80::/10 — link-local
        assert!(!is_safe_url("http://[fe80::1]/secret"));
    }

    #[test]
    fn blocks_ipv4_mapped_loopback() {
        // ::ffff:127.0.0.1
        assert!(!is_safe_url("http://[::ffff:127.0.0.1]/secret"));
    }

    #[test]
    fn blocks_ipv4_mapped_private() {
        // ::ffff:192.168.1.1
        assert!(!is_safe_url("http://[::ffff:192.168.1.1]/secret"));
    }

    #[test]
    fn blocks_ipv6_unspecified() {
        assert!(!is_safe_url("http://[::]/"));
    }

    // ── FIX-14: DNS rebinding hostname check ──────────────

    #[test]
    fn blocks_nip_io() {
        assert!(!is_safe_url("http://127.0.0.1.nip.io/admin"));
    }

    #[test]
    fn blocks_sslip_io() {
        assert!(!is_safe_url("http://10.0.0.1.sslip.io/admin"));
    }

    #[test]
    fn blocks_localtest_me() {
        assert!(!is_safe_url("http://localtest.me/admin"));
    }

    #[test]
    fn blocks_lvh_me() {
        assert!(!is_safe_url("http://sub.lvh.me/admin"));
    }

    #[test]
    fn blocks_dot_localhost() {
        assert!(!is_safe_url("http://foo.localhost:8080/admin"));
    }

    // ── FIX-4: upload path sandboxing ─────────────────────

    #[test]
    fn upload_path_allows_tmp() {
        let tmp = std::env::temp_dir().join("test_file.txt");
        std::fs::write(&tmp, "test").ok();
        if tmp.exists() {
            assert!(is_safe_upload_path(&tmp));
            std::fs::remove_file(&tmp).ok();
        }
    }

    #[test]
    fn upload_path_blocks_etc() {
        // /etc/hosts always exists on macOS/Linux
        assert!(!is_safe_upload_path(std::path::Path::new("/etc/hosts")));
    }

    #[test]
    fn upload_path_blocks_nonexistent() {
        assert!(!is_safe_upload_path(std::path::Path::new(
            "/nonexistent/path/file.txt"
        )));
    }

    // -- FIX-R4-02: redact_url_secrets --

    #[test]
    fn redact_strips_oauth_code() {
        let url = "https://example.com/cb?code=abc&state=xyz";
        let redacted = redact_url_secrets(url);
        assert!(!redacted.contains("abc"));
        assert!(redacted.contains("state=xyz"));
    }

    #[test]
    fn redact_strips_access_token() {
        let url = "https://api.example.com/d?access_token=jwt&page=1";
        let redacted = redact_url_secrets(url);
        assert!(!redacted.contains("=jwt"));
        assert!(redacted.contains("page=1"));
    }

    #[test]
    fn redact_preserves_safe_url() {
        let url = "https://example.com/page?q=search&page=2";
        let redacted = redact_url_secrets(url);
        assert!(redacted.contains("q=search"));
        assert!(redacted.contains("page=2"));
    }

    #[test]
    fn redact_strips_fragment() {
        let url = "https://example.com/page#tok";
        let redacted = redact_url_secrets(url);
        assert!(!redacted.contains('#'));
    }

    #[test]
    fn redact_handles_unparseable_url() {
        let url = "not-a-url";
        assert_eq!(redact_url_secrets(url), url);
    }

    #[test]
    fn redact_handles_magic_link() {
        let url = "https://example.com/verify?magic=m1&user=bob";
        let redacted = redact_url_secrets(url);
        assert!(!redacted.contains("=m1"));
        assert!(redacted.contains("user=bob"));
    }

    // -- FIX-R4-03: broadened sensitive masking --

    #[test]
    fn masks_by_name_otp() {
        assert_eq!(
            mask_sensitive_value(Some("text"), Some("otp_code"), Some("123456")),
            Some("[filled]".to_string()),
        );
    }

    #[test]
    fn masks_by_name_cvv() {
        assert_eq!(
            mask_sensitive_value(None, Some("card_cvv"), Some("123")),
            Some("[filled]".to_string()),
        );
    }

    #[test]
    fn masks_by_name_mfa() {
        assert_eq!(
            mask_sensitive_value(None, Some("mfa_code"), Some("654321")),
            Some("[filled]".to_string()),
        );
    }

    #[test]
    fn masks_by_name_verification() {
        assert_eq!(
            mask_sensitive_value(None, Some("verification_code"), Some("ABCDEF")),
            Some("[filled]".to_string()),
        );
    }

    #[test]
    fn masks_by_name_2fa() {
        assert_eq!(
            mask_sensitive_value(None, Some("2fa_code"), Some("987654")),
            Some("[filled]".to_string()),
        );
    }

    #[test]
    fn masks_by_name_token_field() {
        assert_eq!(
            mask_sensitive_value(None, Some("auth_token"), Some("xyz")),
            Some("[filled]".to_string()),
        );
    }

    // -- FIX-2: redact_action_debug --

    #[test]
    fn redact_action_debug_masks_type_value() {
        let input = r#"Type { element: 3, value: "s3cret_password", reasoning: "fill pw" }"#;
        let redacted = redact_action_debug(input);
        assert!(!redacted.contains("s3cret_password"));
        assert!(redacted.contains("[REDACTED]"));
        assert!(redacted.contains("element: 3"));
    }

    #[test]
    fn redact_action_debug_preserves_click() {
        let input = r#"Click { element: 5, reasoning: "click submit" }"#;
        let redacted = redact_action_debug(input);
        assert_eq!(redacted, input);
    }

    // -- FIX-6: redact_credentials_from_goal --

    #[test]
    fn redact_goal_password() {
        let goal = "login as admin password secret123";
        let redacted = redact_credentials_from_goal(goal);
        assert!(!redacted.contains("secret123"));
        assert!(redacted.contains("password [REDACTED]"));
    }

    #[test]
    fn redact_goal_pw() {
        let goal = "login as testuser pw hunter2";
        let redacted = redact_credentials_from_goal(goal);
        assert!(!redacted.contains("hunter2"));
        assert!(redacted.contains("pw [REDACTED]"));
    }

    #[test]
    fn redact_goal_token() {
        let goal = "authenticate with token abc123def";
        let redacted = redact_credentials_from_goal(goal);
        assert!(!redacted.contains("abc123def"));
        assert!(redacted.contains("token [REDACTED]"));
    }

    #[test]
    fn redact_goal_no_credentials() {
        let goal = "navigate to the dashboard page";
        let redacted = redact_credentials_from_goal(goal);
        assert_eq!(redacted, goal);
    }

    // -- SS-1: Property-based tests (proptest) --

    mod prop {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// sanitize_text never produces steganographic chars.
            #[test]
            fn sanitize_text_removes_all_steganographic(s in "\\PC*") {
                let out = sanitize_text(&s);
                for ch in out.chars() {
                    prop_assert!(!is_steganographic(ch), "steganographic char U+{:04X} survived", ch as u32);
                }
            }

            /// sanitize_text preserves all non-steganographic chars.
            #[test]
            fn sanitize_text_preserves_normal(s in "[a-zA-Z0-9 .,!?@#%^&*()-=+]{0,200}") {
                prop_assert_eq!(sanitize_text(&s), s);
            }

            /// is_safe_url never panics on arbitrary input.
            #[test]
            fn is_safe_url_never_panics(s in "\\PC{0,500}") {
                let _ = is_safe_url(&s);
            }

            /// redact_url_secrets never panics on arbitrary input.
            #[test]
            fn redact_url_secrets_never_panics(s in "\\PC{0,500}") {
                let _ = redact_url_secrets(&s);
            }

            /// redact_url_secrets: known sensitive param values do not appear in output.
            /// Uses a fixed safe param name that won't match any SENSITIVE_KEYS substring.
            #[test]
            fn redact_url_secrets_hides_token(
                token_val in "[a-zA-Z0-9]{5,20}",
                safe_val in "[a-z]{3,8}",
            ) {
                let url = format!(
                    "https://example.com/cb?access_token={token_val}&page={safe_val}"
                );
                let redacted = redact_url_secrets(&url);
                prop_assert!(!redacted.contains(&token_val), "token value leaked: {redacted}");
                prop_assert!(redacted.contains(&safe_val), "safe value lost: {redacted}");
            }
        }
    }

    // ── is_loopback_only (Wave 3 — inverse SSRF for CDP attach) ──

    #[test]
    fn loopback_allows_ws_localhost() {
        assert!(is_loopback_only("ws://localhost:9222/devtools/browser/abc"));
    }

    #[test]
    fn loopback_allows_ws_127_0_0_1() {
        assert!(is_loopback_only("ws://127.0.0.1:9222/devtools/browser/abc"));
    }

    #[test]
    fn loopback_allows_ws_ipv6_bracketed() {
        assert!(is_loopback_only("ws://[::1]:9222/devtools/browser/abc"));
    }

    #[test]
    fn loopback_allows_wss_localhost() {
        assert!(is_loopback_only(
            "wss://localhost:9222/devtools/browser/abc"
        ));
    }

    #[test]
    fn loopback_allows_http_localhost() {
        assert!(is_loopback_only("http://localhost:9222"));
    }

    #[test]
    fn loopback_allows_http_localhost_with_path() {
        assert!(is_loopback_only("http://localhost:9222/json/version"));
    }

    #[test]
    fn loopback_allows_http_127_0_0_1() {
        assert!(is_loopback_only("http://127.0.0.1:9222"));
    }

    #[test]
    fn loopback_allows_https_localhost() {
        assert!(is_loopback_only("https://localhost:9222"));
    }

    #[test]
    fn loopback_allows_localhost_case_insensitive() {
        assert!(is_loopback_only("http://LocalHost:9222"));
    }

    #[test]
    fn loopback_rejects_private_192() {
        assert!(!is_loopback_only(
            "ws://192.168.1.1:9222/devtools/browser/x"
        ));
    }

    #[test]
    fn loopback_rejects_private_10() {
        assert!(!is_loopback_only("http://10.0.0.1:9222"));
    }

    #[test]
    fn loopback_rejects_private_172() {
        assert!(!is_loopback_only("http://172.16.0.1:9222"));
    }

    #[test]
    fn loopback_rejects_link_local() {
        assert!(!is_loopback_only("http://169.254.1.1:9222"));
    }

    #[test]
    fn loopback_rejects_public_host() {
        assert!(!is_loopback_only("ws://evil.com:9222/devtools/browser/x"));
    }

    #[test]
    fn loopback_rejects_dns_rebinding_like_hostnames() {
        // .localhost TLD, .local TLD etc. — these can resolve anywhere.
        assert!(!is_loopback_only("http://evil.localhost:9222"));
        assert!(!is_loopback_only("http://mybox.local:9222"));
    }

    #[test]
    fn loopback_rejects_empty_string() {
        assert!(!is_loopback_only(""));
    }

    #[test]
    fn loopback_rejects_whitespace_only() {
        assert!(!is_loopback_only("   "));
    }

    #[test]
    fn loopback_rejects_bare_path() {
        assert!(!is_loopback_only("/devtools/browser/abc"));
    }

    #[test]
    fn loopback_rejects_missing_scheme() {
        assert!(!is_loopback_only("localhost:9222"));
    }

    #[test]
    fn loopback_rejects_wrong_scheme() {
        assert!(!is_loopback_only("file://localhost/etc/passwd"));
        assert!(!is_loopback_only("chrome://localhost/"));
    }

    #[test]
    fn loopback_rejects_url_without_host() {
        // `http:` with no authority fails — no host.
        assert!(!is_loopback_only("http:///abc"));
    }

    #[test]
    fn loopback_rejects_garbage() {
        assert!(!is_loopback_only("not a url at all"));
    }
}
