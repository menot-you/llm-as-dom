//! Chrome cookie decryption (macOS Keychain + AES-128-CBC).
//!
//! On macOS, Chrome encrypts every cookie value with a key derived from the
//! "Chrome Safe Storage" Keychain entry via PBKDF2-HMAC-SHA1.
//! On Linux the password is either fetched from the secret service or defaults
//! to `"peanuts"` (1 PBKDF2 iteration).

/// Get the Chrome Safe Storage password from macOS Keychain.
#[cfg(target_os = "macos")]
pub fn get_chrome_password() -> Result<String, crate::Error> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Chrome Safe Storage", "-w"])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| crate::Error::ActionFailed(format!("keychain access failed: {e}")))?;

    if !output.status.success() {
        return Err(crate::Error::ActionFailed(
            "failed to get Chrome Safe Storage password from Keychain \
             (user may have denied access)"
                .into(),
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Linux fallback: hardcoded `"peanuts"` (Chrome's default when no secret service).
#[cfg(target_os = "linux")]
pub fn get_chrome_password() -> Result<String, crate::Error> {
    Ok("peanuts".to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn get_chrome_password() -> Result<String, crate::Error> {
    Err(crate::Error::ActionFailed(
        "cookie decryption not supported on this platform".into(),
    ))
}

/// Derive the AES-128 key from the Chrome password via PBKDF2-HMAC-SHA1.
pub fn derive_key(password: &str) -> [u8; 16] {
    let iterations: u32 = if cfg!(target_os = "macos") { 1003 } else { 1 };
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password.as_bytes(), b"saltysalt", iterations, &mut key);
    key
}

/// Decrypt a Chrome encrypted cookie value.
///
/// Layout: `v10` | `v11` (3-byte prefix) ++ AES-128-CBC ciphertext.
/// IV is 16 bytes of `0x20` (space character).
///
/// FIX-R3-05: The static IV (`[0x20; 16]`) is intentional and matches Chrome's
/// own implementation in `os_crypt_async.cc`. Chrome uses a fixed IV because the
/// encryption key is already derived per-profile via PBKDF2 from the Safe Storage
/// keychain entry. Changing the IV would break compatibility with Chrome's cookie
/// database. This is NOT a vulnerability — it's a protocol requirement.
pub fn decrypt_cookie_value(encrypted: &[u8], key: &[u8; 16]) -> Result<String, crate::Error> {
    if encrypted.len() < 3 {
        return Err(crate::Error::ActionFailed(
            "encrypted value too short".into(),
        ));
    }

    let prefix = &encrypted[..3];
    if prefix != b"v10" && prefix != b"v11" {
        return Err(crate::Error::ActionFailed(format!(
            "unknown encryption version: {:?}",
            std::str::from_utf8(prefix).unwrap_or("???")
        )));
    }

    let ciphertext = &encrypted[3..];
    if ciphertext.is_empty() {
        return Ok(String::new());
    }

    let iv: [u8; 16] = [0x20; 16];

    use aes::Aes128;
    use cbc::cipher::{BlockDecryptMut, KeyIvInit};

    type Aes128CbcDec = cbc::Decryptor<Aes128>;

    let decryptor = Aes128CbcDec::new(key.into(), &iv.into());

    let mut buf = ciphertext.to_vec();
    let plaintext = decryptor
        .decrypt_padded_mut::<cbc::cipher::block_padding::Pkcs7>(&mut buf)
        .map_err(|e| crate::Error::ActionFailed(format!("AES decryption failed: {e}")))?;

    String::from_utf8(plaintext.to_vec())
        .map_err(|e| crate::Error::ActionFailed(format!("decrypted cookie not valid UTF-8: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_key_deterministic() {
        let key1 = derive_key("test_password");
        let key2 = derive_key("test_password");
        assert_eq!(key1, key2);
        assert_ne!(key1, [0u8; 16]);
    }

    #[test]
    fn derive_key_different_passwords() {
        let key1 = derive_key("password1");
        let key2 = derive_key("password2");
        assert_ne!(key1, key2);
    }

    #[test]
    fn decrypt_too_short() {
        let key = derive_key("test");
        assert!(decrypt_cookie_value(b"v1", &key).is_err());
    }

    #[test]
    fn decrypt_unknown_version() {
        let key = derive_key("test");
        assert!(decrypt_cookie_value(b"v99xxxxxx", &key).is_err());
    }

    #[test]
    fn decrypt_empty_after_prefix() {
        let key = derive_key("test");
        let result = decrypt_cookie_value(b"v10", &key);
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        use aes::Aes128;
        use cbc::cipher::{BlockEncryptMut, KeyIvInit};

        let password = "test_password";
        let key = derive_key(password);
        let iv: [u8; 16] = [0x20; 16];
        let plaintext = b"my_secret_cookie_value";

        type Aes128CbcEnc = cbc::Encryptor<Aes128>;
        let encryptor = Aes128CbcEnc::new(&key.into(), &iv.into());

        let ciphertext =
            encryptor.encrypt_padded_vec_mut::<cbc::cipher::block_padding::Pkcs7>(plaintext);

        let mut encrypted = b"v10".to_vec();
        encrypted.extend_from_slice(&ciphertext);

        let decrypted = decrypt_cookie_value(&encrypted, &key).unwrap();
        assert_eq!(decrypted, "my_secret_cookie_value");
    }
}
