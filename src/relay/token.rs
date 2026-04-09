//! One-time auth token generation for relay pairing.

/// SEC-S5: Generate a cryptographically random 12-character alphanumeric token.
/// Entropy: 36^12 ≈ 4.7 × 10^18 (vs old 10^6).
pub fn generate() -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut buf = [0u8; 12];
    getrandom::getrandom(&mut buf).expect("getrandom failed");
    buf.iter()
        .map(|b| CHARSET[(*b as usize) % CHARSET.len()] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_12_chars_alphanumeric() {
        let t = generate();
        assert_eq!(t.len(), 12);
        assert!(
            t.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
    }

    #[test]
    fn tokens_are_unique() {
        let tokens: Vec<_> = (0..10).map(|_| generate()).collect();
        let unique: std::collections::HashSet<_> = tokens.iter().collect();
        assert!(unique.len() > 1, "all tokens identical — RNG broken");
    }
}
