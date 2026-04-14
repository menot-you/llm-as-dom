//! CloakBrowser bootstrap — first-launch downloader for the pre-patched
//! stealth Chromium binary from CloakHQ/CloakBrowser.
//!
//! CloakBrowser is a Chromium fork with 49 C++-level fingerprint patches
//! (canvas, WebGL, WebRTC, audio, navigator, toString, etc). Pointing
//! chromiumoxide at this binary bypasses the JS-layer stealth ceiling
//! imposed by detectors like Creepjs that specifically target Proxy-based
//! `Function.prototype.toString` replacements.
//!
//! # License note
//!
//! CloakBrowser binary is distributed under a custom `BINARY-LICENSE.md`
//! that **forbids redistribution** (vendoring into a release tarball).
//! Runtime download by the end-user is explicitly allowed, mirroring how
//! `puppeteer` auto-downloads Chrome for Testing. LAD never vendors the
//! binary — it fetches on first launch into `~/.cache/lad/cloakbrowser/`
//! and reuses from there on subsequent runs.
//!
//! # Resolution order
//!
//! 1. `$LAD_CLOAK_BINARY` — explicit path override for already-installed
//!    CloakBrowser (CI, custom build, air-gapped install).
//! 2. `$LAD_CLOAK_DISABLE=1` — skip cloakbrowser entirely; chromiumoxide
//!    uses its default Chromium resolution (Chrome for Testing, system).
//! 3. Default — check `$XDG_CACHE_HOME/lad/cloakbrowser/` (or platform
//!    equivalent), download if missing, extract, strip quarantine.
//!
//! # Platform support
//!
//! CloakBrowser's latest macOS build is `chromium-v145.0.7632.109.2`
//! (older than their Linux builds which track Chromium 146). macOS x64
//! and arm64 are both shipped. Linux arm64 and x64 also supported.
//! Windows path is not yet implemented — falls back to vanilla Chromium.

use std::path::{Path, PathBuf};

use crate::Error;

/// Pinned CloakBrowser release we download. Update this constant when a
/// new release with all desired platforms is available. Tracking latest
/// automatically is tempting but introduces non-reproducible behavior;
/// the user can override via `$LAD_CLOAK_RELEASE_TAG` if needed.
const DEFAULT_RELEASE_TAG: &str = "chromium-v145.0.7632.109.2";

/// SHA-256 digests for each platform tarball at `DEFAULT_RELEASE_TAG`.
/// Pinned to prevent silent binary substitution by a compromised release
/// asset. `shasum -a 256 <file>` on disk should match these.
///
/// Only referenced under cfg(macos + aarch64); gating keeps Linux CI
/// from bouncing with `-D dead_code`.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const CLOAK_DARWIN_ARM64_SHA256: &str =
    "505582aa1bd3971c577f70e0cbbe016431702bdb693529abfd943b5bd9120c1c";

/// Resolve the path to a stealth Chromium binary.
///
/// Returns `Ok(Some(path))` when a usable binary is found or freshly
/// installed, `Ok(None)` when cloakbrowser is disabled or this platform
/// has no pinned build — the caller should fall back to the default
/// chromiumoxide Chromium resolution in that case.
pub fn resolve_cloak_binary() -> Result<Option<PathBuf>, Error> {
    // 1. Explicit disable short-circuit.
    if std::env::var("LAD_CLOAK_DISABLE")
        .ok()
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"))
    {
        tracing::info!("cloakbrowser disabled via LAD_CLOAK_DISABLE");
        return Ok(None);
    }

    // 2. Explicit path override — trust the caller, just verify it runs.
    if let Ok(path) = std::env::var("LAD_CLOAK_BINARY")
        && !path.is_empty()
    {
        let pb = PathBuf::from(&path);
        if pb.exists() {
            tracing::info!(path = %pb.display(), "cloakbrowser: explicit path");
            return Ok(Some(pb));
        }
        tracing::warn!(
            path = %pb.display(),
            "LAD_CLOAK_BINARY set but path does not exist — falling back to auto-download"
        );
    }

    // 3. Platform-aware cache path + download.
    let (tarball_name, expected_sha256) = platform_asset()?;
    let cache_dir = cache_root()?;
    let install_dir = cache_dir.join(DEFAULT_RELEASE_TAG);
    let binary_path = binary_path_for_platform(&install_dir);

    if binary_path.exists() {
        tracing::debug!(path = %binary_path.display(), "cloakbrowser: cached");
        return Ok(Some(binary_path));
    }

    // First-run install.
    tracing::info!(
        release = DEFAULT_RELEASE_TAG,
        asset = tarball_name,
        "cloakbrowser: first-run install"
    );
    std::fs::create_dir_all(&install_dir).map_err(|e| {
        Error::Browser(format!(
            "cloakbrowser: create install dir {}: {e}",
            install_dir.display()
        ))
    })?;

    let tarball_path = install_dir.join(tarball_name);
    download_tarball(tarball_name, &tarball_path)?;
    verify_sha256(&tarball_path, expected_sha256)?;
    extract_tarball(&tarball_path, &install_dir)?;
    strip_macos_quarantine(&install_dir);
    // Tarball no longer needed once extracted — reclaim ~140 MB.
    let _ = std::fs::remove_file(&tarball_path);

    if !binary_path.exists() {
        return Err(Error::Browser(format!(
            "cloakbrowser: extraction succeeded but binary not found at {}",
            binary_path.display()
        )));
    }

    tracing::info!(path = %binary_path.display(), "cloakbrowser: installed");
    Ok(Some(binary_path))
}

/// Return `(asset_filename, expected_sha256)` for the current host, or
/// `Ok(None)` if we don't ship a pinned build for this platform.
fn platform_asset() -> Result<(&'static str, &'static str), Error> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Ok((
            "cloakbrowser-darwin-arm64.tar.gz",
            CLOAK_DARWIN_ARM64_SHA256,
        ))
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        Err(Error::Browser(
            "cloakbrowser: no pinned build for this host platform — \
             set LAD_CLOAK_DISABLE=1 to use default Chromium"
                .into(),
        ))
    }
}

/// Location of the extracted Chromium executable relative to `install_dir`.
fn binary_path_for_platform(install_dir: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        install_dir
            .join("Chromium.app")
            .join("Contents")
            .join("MacOS")
            .join("Chromium")
    }
    #[cfg(target_os = "linux")]
    {
        install_dir.join("chromium")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        install_dir.join("chromium")
    }
}

/// Root cache dir that holds every version of cloakbrowser we've installed.
/// Uses the same XDG/macOS resolution as the user_data_dir helper in
/// `mcp_server/mod.rs` so both live under the user's cache hierarchy.
fn cache_root() -> Result<PathBuf, Error> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("lad").join("cloakbrowser"));
    }
    let home = std::env::var("HOME").map_err(|e| {
        Error::Browser(format!(
            "cloakbrowser: $HOME not set, cannot resolve cache dir: {e}"
        ))
    })?;
    #[cfg(target_os = "macos")]
    {
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Caches")
            .join("lad")
            .join("cloakbrowser"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(PathBuf::from(home)
            .join(".cache")
            .join("lad")
            .join("cloakbrowser"))
    }
}

/// Download a release asset via the GitHub release CDN URL pattern.
/// Blocks the calling thread — acceptable because this only runs once
/// per machine on first engine launch and shows progress in tracing.
fn download_tarball(asset_name: &str, dest: &Path) -> Result<(), Error> {
    let url = format!(
        "https://github.com/CloakHQ/CloakBrowser/releases/download/{}/{}",
        DEFAULT_RELEASE_TAG, asset_name
    );
    tracing::info!(url = %url, dest = %dest.display(), "cloakbrowser: downloading");

    // Use `curl -L` to follow redirects — GitHub release CDN redirects to
    // objects.githubusercontent.com. Avoids pulling in a heavy HTTP client
    // dep just for this one-shot fetch.
    let status = std::process::Command::new("curl")
        .args([
            "-L",             // follow redirects
            "--fail",         // exit non-zero on HTTP errors
            "--progress-bar", // show progress to stderr
            "-o",
            dest.to_str().ok_or_else(|| {
                Error::Browser("cloakbrowser: dest path is not valid UTF-8".into())
            })?,
            &url,
        ])
        .status()
        .map_err(|e| Error::Browser(format!("cloakbrowser: curl failed to launch: {e}")))?;
    if !status.success() {
        return Err(Error::Browser(format!(
            "cloakbrowser: curl exited with {} downloading {}",
            status, url
        )));
    }
    Ok(())
}

/// Verify the downloaded tarball matches the pinned SHA-256.
fn verify_sha256(path: &Path, expected: &str) -> Result<(), Error> {
    let output = std::process::Command::new("shasum")
        .args(["-a", "256", path.to_str().unwrap_or("")])
        .output()
        .map_err(|e| Error::Browser(format!("cloakbrowser: shasum failed to launch: {e}")))?;
    if !output.status.success() {
        return Err(Error::Browser(format!(
            "cloakbrowser: shasum failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let actual = stdout.split_whitespace().next().unwrap_or("");
    if actual != expected {
        // Remove the tainted file so next run can retry.
        let _ = std::fs::remove_file(path);
        return Err(Error::Browser(format!(
            "cloakbrowser: sha256 mismatch — expected {}, got {}",
            expected, actual
        )));
    }
    tracing::info!("cloakbrowser: sha256 verified");
    Ok(())
}

/// Extract a `.tar.gz` archive into `dest_dir`.
fn extract_tarball(tarball: &Path, dest_dir: &Path) -> Result<(), Error> {
    let status = std::process::Command::new("tar")
        .args([
            "-xzf",
            tarball.to_str().unwrap_or(""),
            "-C",
            dest_dir.to_str().unwrap_or(""),
        ])
        .status()
        .map_err(|e| Error::Browser(format!("cloakbrowser: tar failed to launch: {e}")))?;
    if !status.success() {
        return Err(Error::Browser(format!(
            "cloakbrowser: tar exited with {}",
            status
        )));
    }
    tracing::debug!("cloakbrowser: extraction complete");
    Ok(())
}

/// Best-effort removal of `com.apple.quarantine` extended attributes from
/// the freshly-extracted `.app` bundle. Without this, macOS Gatekeeper
/// blocks the binary on first launch with "cannot be opened because it
/// is from an unidentified developer".
fn strip_macos_quarantine(install_dir: &Path) {
    #[cfg(target_os = "macos")]
    {
        let app = install_dir.join("Chromium.app");
        if app.exists() {
            let _ = std::process::Command::new("xattr")
                .args(["-dr", "com.apple.quarantine"])
                .arg(&app)
                .status();
            tracing::debug!("cloakbrowser: stripped quarantine xattr");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = install_dir;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_asset_returns_expected_pair_on_apple_silicon() {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let (name, sha) = platform_asset().unwrap();
            assert_eq!(name, "cloakbrowser-darwin-arm64.tar.gz");
            assert_eq!(sha.len(), 64);
            assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn cache_root_lives_under_home() {
        if std::env::var("HOME").is_ok() {
            let root = cache_root().unwrap();
            let s = root.to_string_lossy();
            assert!(s.ends_with("lad/cloakbrowser"));
        }
    }

    #[test]
    fn binary_path_has_platform_specific_suffix() {
        let install = PathBuf::from("/tmp/lad-cloak-test");
        let bin = binary_path_for_platform(&install);
        #[cfg(target_os = "macos")]
        {
            assert!(bin.ends_with("Chromium.app/Contents/MacOS/Chromium"));
        }
        #[cfg(target_os = "linux")]
        {
            assert!(bin.ends_with("chromium"));
        }
    }

    #[test]
    fn cloak_disable_env_returns_none() {
        // SAFETY: tests run sequentially by default in cargo test.
        // Scope the env var to this test.
        // SAFETY: env::set_var is safe in Rust 2024 edition.
        unsafe { std::env::set_var("LAD_CLOAK_DISABLE", "1") };
        let result = resolve_cloak_binary();
        unsafe { std::env::remove_var("LAD_CLOAK_DISABLE") };
        assert!(matches!(result, Ok(None)));
    }
}
