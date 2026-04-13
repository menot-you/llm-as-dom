//! Stale Chrome `Singleton{Lock,Socket,Cookie}` cleanup.
//!
//! DX-SL1 (bug 1): Chromium writes three lock-family files in its user-data
//! directory to prevent concurrent access:
//!
//! * `SingletonLock`   — symlink `hostname-PID` (Linux/macOS) or text file.
//! * `SingletonSocket` — UNIX socket file.
//! * `SingletonCookie` — nonce file used for the IPC handshake.
//!
//! If Chrome crashes, these files survive and block relaunch with errors
//! like "The profile appears to be in use by another Chromium process".
//! chromiumoxide does not clean them up on its own, so the user ends up
//! with a failed launch until they `rm -rf` the user-data-dir manually.
//!
//! This module removes them when the owning process is definitively dead,
//! preserving them when the owner is still alive.

use std::path::Path;

/// The three singleton lock-family files Chromium creates in the
/// user-data directory. Cleaning any one of them in isolation is not enough
/// — if `SingletonSocket` is stale while `SingletonLock` is gone, Chrome
/// still refuses to start.
const LOCK_FILE_NAMES: &[&str] = &["SingletonLock", "SingletonSocket", "SingletonCookie"];

/// Remove stale Chrome singleton lock files in `user_data_dir`.
///
/// For each of the three lock files:
///
/// 1. Skip if the file doesn't exist.
/// 2. If the file is a symlink, read its target (`hostname-PID` format).
///    * If the PID is alive → keep the file (real owner still running).
///    * If the PID is dead or unparseable → remove it.
/// 3. If the file is NOT a symlink (plain file / socket), just remove it.
///    Chromium recreates these on launch, so removing a stale plain file
///    is always safe.
///
/// Errors from `readlink` / `remove_file` are logged but do not propagate.
/// A best-effort cleanup is the right semantics here — the worst case is
/// that `Browser::launch` fails with the existing "profile in use" error.
pub fn cleanup_stale_singleton_locks(user_data_dir: &Path) {
    for name in LOCK_FILE_NAMES {
        let path = user_data_dir.join(name);
        cleanup_one(&path);
    }
}

/// Cleanup logic for a single lock file.
fn cleanup_one(path: &Path) {
    // `symlink_metadata` does NOT follow symlinks — we need this so we can
    // detect a dangling symlink (target file gone) as `is_symlink() == true`.
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::debug!(path = %path.display(), error = %e, "cannot stat singleton lock file");
            return;
        }
    };

    if meta.file_type().is_symlink() {
        cleanup_symlink_lock(path);
    } else {
        // Plain file / socket / FIFO. Not the usual Chromium layout, but we
        // still remove it — Chrome will recreate it and this avoids false
        // negatives on filesystems where symlink() fell back to a copy.
        tracing::warn!(
            path = %path.display(),
            "removing non-symlink singleton lock file (Chromium will recreate)"
        );
        if let Err(e) = std::fs::remove_file(path) {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to remove non-symlink singleton lock"
            );
        }
    }
}

/// Handle the usual case: the lock is a symlink whose target is
/// `hostname-PID`. Parse the PID and remove only if the process is gone.
fn cleanup_symlink_lock(path: &Path) {
    let target = match std::fs::read_link(path) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "cannot readlink singleton lock — removing"
            );
            let _ = std::fs::remove_file(path);
            return;
        }
    };

    // Symlink target is stringly-typed on Unix (`hostname-12345`).
    let target_str = target.to_string_lossy();
    let pid = parse_pid_from_target(target_str.as_ref());

    match pid {
        Some(pid) if process_is_alive(pid) => {
            tracing::debug!(
                path = %path.display(),
                pid,
                "singleton lock owner is alive — leaving it"
            );
        }
        Some(pid) => {
            tracing::warn!(
                path = %path.display(),
                pid,
                target = %target_str,
                "removing stale singleton lock (owner dead)"
            );
            if let Err(e) = std::fs::remove_file(path) {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to remove stale singleton lock"
                );
            }
        }
        None => {
            tracing::warn!(
                path = %path.display(),
                target = %target_str,
                "singleton lock symlink has unparseable target — removing"
            );
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Parse the trailing PID from a `hostname-PID` target string.
///
/// Accepts:
/// * `"hostname-12345"` → `Some(12345)`
/// * `"host-with-dashes-12345"` → `Some(12345)` (only last segment counts)
/// * `"12345"` → `Some(12345)` (missing hostname)
/// * `"notanumber"` → `None`
/// * `""` → `None`
pub(crate) fn parse_pid_from_target(target: &str) -> Option<u32> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return None;
    }
    let last_segment = trimmed.rsplit('-').next().unwrap_or(trimmed);
    last_segment.parse::<u32>().ok()
}

/// Check whether `pid` is alive via `kill -0 PID`.
///
/// `kill -0` is a POSIX no-op signal that returns success if the PID is
/// reachable and ESRCH otherwise. We shell out instead of using `libc::kill`
/// to avoid pulling in an extra dependency just for the stale-lock path.
fn process_is_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pid_accepts_hostname_pid() {
        assert_eq!(parse_pid_from_target("MacBook-12345"), Some(12345));
    }

    #[test]
    fn parse_pid_accepts_multi_dash_hostname() {
        assert_eq!(parse_pid_from_target("host-with-dashes-777"), Some(777));
    }

    #[test]
    fn parse_pid_accepts_bare_pid() {
        assert_eq!(parse_pid_from_target("42"), Some(42));
    }

    #[test]
    fn parse_pid_rejects_empty() {
        assert_eq!(parse_pid_from_target(""), None);
        assert_eq!(parse_pid_from_target("   "), None);
    }

    #[test]
    fn parse_pid_rejects_non_numeric_tail() {
        assert_eq!(parse_pid_from_target("host-xyz"), None);
    }

    #[test]
    fn process_is_alive_detects_self() {
        // Our own PID must be alive.
        assert!(process_is_alive(std::process::id()));
    }

    #[test]
    fn process_is_alive_detects_dead_pid() {
        // PID 999999 is higher than /proc/sys/kernel/pid_max on most Linux
        // systems (32768) and basically unreachable on macOS — treating it
        // as "dead" is reliable enough for this test.
        assert!(!process_is_alive(999_999));
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_removes_stale_symlink_but_keeps_live() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        // Stale: symlink to "host-999999" (dead PID).
        let stale = dir.join("SingletonLock");
        symlink("host-999999", &stale).expect("symlink stale");

        // Live: symlink to "host-{self_pid}".
        let live_name = "SingletonCookie";
        let live = dir.join(live_name);
        let live_target = format!("host-{}", std::process::id());
        symlink(&live_target, &live).expect("symlink live");

        cleanup_stale_singleton_locks(dir);

        assert!(
            !stale.exists() && std::fs::symlink_metadata(&stale).is_err(),
            "stale SingletonLock should be removed"
        );
        assert!(
            std::fs::symlink_metadata(&live).is_ok(),
            "live SingletonCookie should be preserved"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_removes_non_symlink_lock() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("SingletonSocket");
        std::fs::write(&path, b"stale socket placeholder").expect("write");

        cleanup_stale_singleton_locks(tmp.path());

        assert!(
            !path.exists(),
            "non-symlink SingletonSocket should be removed"
        );
    }

    #[test]
    fn cleanup_noop_on_empty_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Must not panic / error.
        cleanup_stale_singleton_locks(tmp.path());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_removes_unparseable_symlink_target() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("SingletonLock");
        symlink("not-a-pid-target", &path).expect("symlink");

        cleanup_stale_singleton_locks(tmp.path());

        assert!(
            std::fs::symlink_metadata(&path).is_err(),
            "unparseable target symlink should be removed"
        );
    }
}
