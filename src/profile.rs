//! Chrome profile discovery and cookie loading.
//!
//! Supports loading cookies from an existing Chrome profile so lad can
//! browse sites where the user is already logged in.

use std::path::{Path, PathBuf};

/// Resolve a Chrome profile path from a shorthand name.
///
/// - `"default"` -> the user's default Chrome profile
/// - An absolute path -> used as-is
/// - A relative path -> resolved relative to Chrome's data directory
pub fn resolve_profile_path(name: &str) -> Option<PathBuf> {
    if name == "default" || name == "Default" {
        return default_chrome_profile();
    }

    let path = PathBuf::from(name);
    if path.is_absolute() && path.exists() {
        return Some(path);
    }

    // Try relative to Chrome data dir
    if let Some(chrome_dir) = chrome_data_dir() {
        let resolved = chrome_dir.join(name);
        if resolved.exists() {
            return Some(resolved);
        }
    }

    // Try as-is if it exists
    if path.exists() {
        return Some(path);
    }

    None
}

/// Get the default Chrome profile directory.
fn default_chrome_profile() -> Option<PathBuf> {
    chrome_data_dir().map(|d| d.join("Default"))
}

/// Get Chrome's data directory (parent of profile directories).
fn chrome_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        home_dir().map(|h| h.join("Library/Application Support/Google/Chrome"))
    }

    #[cfg(target_os = "linux")]
    {
        home_dir().map(|h| h.join(".config/google-chrome"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// Get home directory from `$HOME` without external crate dependency.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// Extract cookies from a Chrome profile's Cookies SQLite database.
///
/// Returns cookies as `CookieEntry` values. On macOS/Linux, Chrome encrypts
/// cookie values -- encrypted cookies are skipped with a warning.
pub fn extract_cookies_from_profile(
    profile_path: &Path,
) -> Result<Vec<crate::session::CookieEntry>, crate::Error> {
    let cookies_db = profile_path.join("Cookies");
    if !cookies_db.exists() {
        // User may have passed the Chrome data dir, not the profile
        let alt = profile_path.join("Default/Cookies");
        if alt.exists() {
            return extract_cookies_from_db(&alt);
        }
        return Err(crate::Error::ActionFailed(format!(
            "Chrome Cookies database not found at {}",
            cookies_db.display()
        )));
    }

    // Chrome locks the Cookies file when running. Copy to temp.
    let tmp = std::env::temp_dir().join(format!("lad-cookies-{}", std::process::id()));
    std::fs::copy(&cookies_db, &tmp).map_err(|e| {
        crate::Error::ActionFailed(format!(
            "failed to copy Cookies DB (is Chrome running?): {e}"
        ))
    })?;

    let result = extract_cookies_from_db(&tmp);
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Read cookies from a SQLite database file.
fn extract_cookies_from_db(
    db_path: &Path,
) -> Result<Vec<crate::session::CookieEntry>, crate::Error> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| crate::Error::ActionFailed(format!("failed to open Cookies DB: {e}")))?;

    let mut stmt = conn
        .prepare(
            "SELECT host_key, name, value, encrypted_value, path, \
             expires_utc, is_secure, is_httponly, samesite \
             FROM cookies \
             ORDER BY host_key, name",
        )
        .map_err(|e| crate::Error::ActionFailed(format!("SQL prepare error: {e}")))?;

    let mut cookies = Vec::new();
    let mut encrypted_count = 0u32;

    let rows = stmt
        .query_map([], |row| {
            Ok(CookieRow {
                host_key: row.get(0)?,
                name: row.get(1)?,
                value: row.get(2)?,
                encrypted_value: row.get(3)?,
                path: row.get(4)?,
                expires_utc: row.get(5)?,
                is_secure: row.get(6)?,
                is_httponly: row.get(7)?,
                samesite: row.get(8)?,
            })
        })
        .map_err(|e| crate::Error::ActionFailed(format!("SQL query error: {e}")))?;

    for row in rows {
        let r = row.map_err(|e| crate::Error::ActionFailed(format!("row error: {e}")))?;

        // Chrome stores the value either in `value` (plaintext) or
        // `encrypted_value` (encrypted). If `value` is empty and
        // `encrypted_value` is non-empty, the cookie is encrypted.
        if r.value.is_empty() {
            if !r.encrypted_value.is_empty() {
                encrypted_count += 1;
            }
            continue;
        }

        cookies.push(cookie_entry_from_row(&r));
    }

    if encrypted_count > 0 {
        tracing::info!(
            encrypted = encrypted_count,
            loaded = cookies.len(),
            "loaded cookies from Chrome profile (encrypted cookies skipped)"
        );
    } else {
        tracing::info!(loaded = cookies.len(), "loaded cookies from Chrome profile");
    }

    Ok(cookies)
}

/// Intermediate struct for a row from Chrome's `cookies` table.
struct CookieRow {
    host_key: String,
    name: String,
    value: String,
    encrypted_value: Vec<u8>,
    path: String,
    expires_utc: i64,
    is_secure: bool,
    is_httponly: bool,
    samesite: i32,
}

/// Convert a Chrome cookie row into a `CookieEntry`.
fn cookie_entry_from_row(r: &CookieRow) -> crate::session::CookieEntry {
    // Chrome's expires_utc is microseconds since 1601-01-01.
    // Convert to Unix timestamp (seconds since 1970-01-01).
    // Offset: 11_644_473_600 seconds between 1601 and 1970.
    let expires = if r.expires_utc == 0 {
        0.0 // Session cookie
    } else {
        (r.expires_utc as f64 / 1_000_000.0) - 11_644_473_600.0
    };

    let same_site = match r.samesite {
        0 => None,
        1 => Some("Lax".to_string()),
        2 => Some("Strict".to_string()),
        3 => Some("None".to_string()),
        _ => None,
    };

    crate::session::CookieEntry {
        name: r.name.clone(),
        value: r.value.clone(),
        domain: r.host_key.clone(),
        path: r.path.clone(),
        expires,
        secure: r.is_secure,
        http_only: r.is_httponly,
        same_site,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_default_profile() {
        let path = resolve_profile_path("default");
        // Don't assert exists -- CI may not have Chrome installed
        assert!(path.is_some() || cfg!(not(any(target_os = "macos", target_os = "linux"))));
    }

    #[test]
    fn resolve_absolute_path() {
        let path = resolve_profile_path("/tmp");
        assert_eq!(path, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn resolve_nonexistent_returns_none() {
        let path = resolve_profile_path("/this/does/not/exist/anywhere");
        assert!(path.is_none());
    }

    #[test]
    fn chrome_timestamp_conversion() {
        // Chrome epoch: 1601-01-01 00:00:00 UTC
        // Unix epoch:   1970-01-01 00:00:00 UTC
        // Difference: 11_644_473_600 seconds
        // Chrome stores in microseconds
        //
        // 2024-01-01 00:00:00 UTC = 1_704_067_200 Unix
        let chrome_usec: i64 = (1_704_067_200 + 11_644_473_600) * 1_000_000;
        let unix = (chrome_usec as f64 / 1_000_000.0) - 11_644_473_600.0;
        assert!((unix - 1_704_067_200.0).abs() < 1.0);
    }

    #[test]
    fn extract_from_missing_db_returns_error() {
        let result = extract_cookies_from_profile(Path::new("/nonexistent/profile"));
        assert!(result.is_err());
    }

    #[test]
    fn extract_from_sqlite_db() {
        let dir = std::env::temp_dir().join("lad-profile-test");
        let _ = std::fs::create_dir_all(&dir);
        let db_path = dir.join("Cookies");

        // Create a minimal Chrome cookies SQLite database
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cookies (
                host_key TEXT NOT NULL,
                name TEXT NOT NULL,
                value TEXT NOT NULL DEFAULT '',
                encrypted_value BLOB NOT NULL DEFAULT X'',
                path TEXT NOT NULL DEFAULT '/',
                expires_utc INTEGER NOT NULL DEFAULT 0,
                is_secure INTEGER NOT NULL DEFAULT 0,
                is_httponly INTEGER NOT NULL DEFAULT 0,
                samesite INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO cookies (host_key, name, value, path, is_secure)
                VALUES ('.example.com', 'sid', 'abc123', '/', 1);
            INSERT INTO cookies (host_key, name, value, encrypted_value, path)
                VALUES ('.test.com', 'enc', '', X'0102030405', '/');",
        )
        .unwrap();
        drop(conn);

        let cookies = extract_cookies_from_profile(&dir).unwrap();

        // Should have 1 plaintext cookie, skipping the encrypted one
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "sid");
        assert_eq!(cookies[0].value, "abc123");
        assert_eq!(cookies[0].domain, ".example.com");
        assert!(cookies[0].secure);
        assert_eq!(cookies[0].expires, 0.0); // session cookie

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }
}
