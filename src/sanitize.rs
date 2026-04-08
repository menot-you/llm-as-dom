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

/// Mask sensitive field values extracted from the DOM.
///
/// Prevents credentials from leaking into LLM prompts.
/// FIX-10: Also checks element `name` for sensitive patterns, not just `type`.
pub fn mask_sensitive_value(
    input_type: Option<&str>,
    name: Option<&str>,
    value: Option<&str>,
) -> Option<String> {
    let is_sensitive = input_type.is_some_and(|t| t.eq_ignore_ascii_case("password"))
        || name.is_some_and(|n| {
            let lower = n.to_lowercase();
            lower.contains("password") || lower.contains("passwd") || lower.contains("secret")
        });
    if is_sensitive {
        value.map(|_| "[filled]".to_string())
    } else {
        value.map(String::from)
    }
}

/// Schemes that must never be navigated to.
const BLOCKED_SCHEMES: &[&str] = &["file:", "javascript:", "data:", "blob:", "vbscript:"];

/// Check whether a URL is safe for automated navigation.
///
/// FIX-2: Deny-by-default on parse failure. Strips control chars before
/// scheme check so `java\x0Bscript:` is caught. Only allows unparseable
/// URLs that look like relative paths (no scheme-like prefix).
///
/// FIX-14: Blocks known DNS rebinding hostnames (nip.io, sslip.io, etc.)
/// and documents the limitation that async DNS resolution is needed for
/// full rebinding protection in production deployments.
pub fn is_safe_url(url: &str) -> bool {
    // Strip control chars for scheme check (catches java\x0Bscript: etc.)
    let cleaned: String = url.chars().filter(|c| !c.is_control()).collect();
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
                if is_suspicious_hostname(host) {
                    return false;
                }
                return !is_private_host(host);
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
}
