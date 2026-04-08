//! One-time auth token generation for relay pairing.

/// Generate a cryptographically random 6-digit numeric token.
pub fn generate() -> String {
    let mut buf = [0u8; 4];
    getrandom::getrandom(&mut buf).expect("getrandom failed");
    let n = u32::from_le_bytes(buf) % 1_000_000;
    format!("{n:06}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_6_digits() {
        let t = generate();
        assert_eq!(t.len(), 6);
        assert!(t.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn tokens_are_unique() {
        let tokens: Vec<_> = (0..10).map(|_| generate()).collect();
        let unique: std::collections::HashSet<_> = tokens.iter().collect();
        assert!(unique.len() > 1, "all tokens identical — RNG broken");
    }
}
