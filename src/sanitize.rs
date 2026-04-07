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

/// Mask password field values extracted from the DOM.
///
/// Prevents credentials from leaking into LLM prompts.
pub fn mask_sensitive_value(input_type: Option<&str>, value: Option<&str>) -> Option<String> {
    match input_type {
        Some("password") => value.map(|_| "[filled]".to_string()),
        _ => value.map(String::from),
    }
}

/// Check whether a URL is safe for automated navigation.
///
/// Blocks `file://`, `javascript:`, `data:`, `blob:` schemes and
/// private/loopback IP addresses to prevent SSRF.
pub fn is_safe_url(url: &str) -> bool {
    let lower = url.trim().to_lowercase();

    // Block dangerous schemes
    if lower.starts_with("file://")
        || lower.starts_with("javascript:")
        || lower.starts_with("data:")
        || lower.starts_with("blob:")
    {
        return false;
    }

    // Try to parse as absolute URL and check host
    if let Ok(parsed) = url::Url::parse(url)
        && let Some(host) = parsed.host_str()
    {
        return !is_private_host(host);
    }

    // Allow relative URLs and unparseable (browser handles resolution)
    true
}

/// Returns `true` if the host resolves to a private, loopback, or
/// link-local address (SSRF targets).
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
            std::net::IpAddr::V6(v6) => v6.is_loopback(),
        };
    }
    false
}

/// Generate a random 16-character hex string for prompt boundaries.
///
/// Uses `std::collections::hash_map::RandomState` as an entropy source
/// to avoid adding `uuid` or `rand` as dependencies.
pub fn random_boundary() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let state = RandomState::new();
    let mut h1 = state.build_hasher();
    h1.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64,
    );
    let a = h1.finish();

    let mut h2 = state.build_hasher();
    h2.write_u64(a.wrapping_mul(0x517cc1b727220a95));
    let b = h2.finish();

    format!("{a:016x}{b:016x}")
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
            mask_sensitive_value(Some("password"), Some("s3cret")),
            Some("[filled]".to_string()),
        );
    }

    #[test]
    fn preserves_text_field() {
        assert_eq!(
            mask_sensitive_value(Some("text"), Some("hello")),
            Some("hello".to_string()),
        );
    }

    #[test]
    fn preserves_none_value() {
        assert_eq!(mask_sensitive_value(Some("password"), None), None);
    }

    #[test]
    fn no_type_preserves_value() {
        assert_eq!(
            mask_sensitive_value(None, Some("data")),
            Some("data".to_string()),
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
}
