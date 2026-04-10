//! Stealth mode — anti-detection patches for Chromium.
//!
//! Patches the well-known automation fingerprints that Google, Cloudflare,
//! Datadome, PerimeterX, and other bot-detection services check. Inspired by
//! [puppeteer-extra-plugin-stealth](https://github.com/berstend/puppeteer-extra/tree/master/packages/puppeteer-extra-plugin-stealth).
//!
//! # What this patches
//!
//! 1. `navigator.webdriver` → `undefined`
//! 2. `navigator.plugins` → fake 3-entry `PluginArray` (PDF viewer, Chrome PDF, Chromium PDF)
//! 3. `navigator.languages` → `['en-US', 'en']`
//! 4. `window.chrome` → `{ runtime, loadTimes, csi, app }`
//! 5. `navigator.permissions.query({name:'notifications'})` → returns `Notification.permission`
//! 6. `WebGLRenderingContext.getParameter(37445/37446)` → `"Intel Inc."` / `"Intel Iris OpenGL Engine"`
//! 7. `WebGL2RenderingContext.getParameter(...)` → same override
//! 8. Removes `HeadlessChrome` from `navigator.userAgent`
//!
//! # How it's applied
//!
//! - The JS payload is injected via CDP `Page.addScriptToEvaluateOnNewDocument`
//!   *before* any navigation. This runs on every new document (including iframes).
//! - The UA is overridden via `Network.setUserAgentOverride` (covers both the
//!   request header and `navigator.userAgent`).
//! - Launch-time Chrome flags disable the `AutomationControlled` Blink feature
//!   and remove the `enable-automation` switch.

use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::network::SetUserAgentOverrideParams;
use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;

/// A real Chrome 131 macOS User-Agent. Matches what a logged-in human user
/// would send. Updated periodically to stay current — mismatched Chrome
/// version + Chromium engine version is itself a detection signal, but
/// bot-detection services primarily key on the "HeadlessChrome" token.
pub const STEALTH_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/131.0.0.0 Safari/537.36";

/// Chrome command-line flags that disable automation indicators.
///
/// - `--disable-blink-features=AutomationControlled`: hides `navigator.webdriver`
///   at the Blink level (before our JS override even runs, as a belt-and-suspenders
///   defense).
/// - `--exclude-switches=enable-automation`: removes the infobar and prevents
///   `chrome.runtime` from being clobbered.
/// - `--disable-features=AutomationControlled`: same effect for newer Chromium.
pub const STEALTH_FLAGS: &[&str] = &[
    "--disable-blink-features=AutomationControlled",
    "--disable-features=AutomationControlled",
];

/// Stealth patches injected into every new document before navigation.
///
/// Keep the script small and idempotent — it runs on every page load including
/// iframes. Patches are wrapped in try/catch so one failure doesn't cascade.
const STEALTH_SCRIPT: &str = r#"
(() => {
  'use strict';

  // 1. navigator.webdriver → undefined
  try {
    Object.defineProperty(Navigator.prototype, 'webdriver', {
      get: () => undefined,
      configurable: true,
    });
  } catch (e) {}

  // 2. navigator.plugins → fake PluginArray with 3 entries
  try {
    const fakePlugins = {
      0: { name: 'PDF Viewer', filename: 'internal-pdf-viewer', description: 'Portable Document Format' },
      1: { name: 'Chrome PDF Viewer', filename: 'internal-pdf-viewer', description: 'Portable Document Format' },
      2: { name: 'Chromium PDF Viewer', filename: 'internal-pdf-viewer', description: 'Portable Document Format' },
      length: 3,
      item: function(i) { return this[i]; },
      namedItem: function(name) {
        for (let i = 0; i < this.length; i++) {
          if (this[i].name === name) return this[i];
        }
        return null;
      },
      refresh: function() {},
    };
    Object.setPrototypeOf(fakePlugins, PluginArray.prototype);
    Object.defineProperty(Navigator.prototype, 'plugins', {
      get: () => fakePlugins,
      configurable: true,
    });
  } catch (e) {}

  // 3. navigator.languages → ['en-US', 'en']
  try {
    Object.defineProperty(Navigator.prototype, 'languages', {
      get: () => ['en-US', 'en'],
      configurable: true,
    });
  } catch (e) {}

  // 4. window.chrome → { runtime, loadTimes, csi, app }
  try {
    if (!window.chrome) {
      window.chrome = {};
    }
    if (!window.chrome.runtime) {
      window.chrome.runtime = {
        OnInstalledReason: {},
        OnRestartRequiredReason: {},
        PlatformArch: {},
        PlatformNaclArch: {},
        PlatformOs: {},
        RequestUpdateCheckStatus: {},
      };
    }
    if (!window.chrome.loadTimes) {
      window.chrome.loadTimes = function() {
        return {
          commitLoadTime: Date.now() / 1000 - Math.random(),
          connectionInfo: 'h2',
          finishDocumentLoadTime: Date.now() / 1000 - Math.random() * 0.5,
          finishLoadTime: Date.now() / 1000,
          firstPaintAfterLoadTime: 0,
          firstPaintTime: Date.now() / 1000 - Math.random() * 0.3,
          navigationType: 'Other',
          npnNegotiatedProtocol: 'h2',
          requestTime: Date.now() / 1000 - Math.random() * 2,
          startLoadTime: Date.now() / 1000 - Math.random() * 2,
          wasAlternateProtocolAvailable: false,
          wasFetchedViaSpdy: true,
          wasNpnNegotiated: true,
        };
      };
    }
    if (!window.chrome.csi) {
      window.chrome.csi = function() {
        return {
          onloadT: Date.now(),
          pageT: Date.now() - performance.timing.navigationStart,
          startE: performance.timing.navigationStart,
          tran: 15,
        };
      };
    }
    if (!window.chrome.app) {
      window.chrome.app = {
        isInstalled: false,
        InstallState: { DISABLED: 'disabled', INSTALLED: 'installed', NOT_INSTALLED: 'not_installed' },
        RunningState: { CANNOT_RUN: 'cannot_run', READY_TO_RUN: 'ready_to_run', RUNNING: 'running' },
      };
    }
  } catch (e) {}

  // 5. navigator.permissions.query({name: 'notifications'}) — must return
  //    Notification.permission (usually 'default'), not 'denied' as headless does.
  try {
    if (window.navigator.permissions && window.navigator.permissions.query) {
      const originalQuery = window.navigator.permissions.query.bind(window.navigator.permissions);
      window.navigator.permissions.query = (parameters) =>
        parameters && parameters.name === 'notifications'
          ? Promise.resolve({ state: Notification.permission, onchange: null })
          : originalQuery(parameters);
    }
  } catch (e) {}

  // 6. WebGL vendor + renderer — headless Chromium reports 'Google Inc.' /
  //    'ANGLE (Google Inc., Vulkan ...)' which flags bots. Override to Intel.
  try {
    const getParameter = WebGLRenderingContext.prototype.getParameter;
    WebGLRenderingContext.prototype.getParameter = function(parameter) {
      if (parameter === 37445) return 'Intel Inc.';
      if (parameter === 37446) return 'Intel Iris OpenGL Engine';
      return getParameter.apply(this, [parameter]);
    };
  } catch (e) {}

  // 7. WebGL2 vendor + renderer (same override)
  try {
    if (typeof WebGL2RenderingContext !== 'undefined') {
      const getParameter2 = WebGL2RenderingContext.prototype.getParameter;
      WebGL2RenderingContext.prototype.getParameter = function(parameter) {
        if (parameter === 37445) return 'Intel Inc.';
        if (parameter === 37446) return 'Intel Iris OpenGL Engine';
        return getParameter2.apply(this, [parameter]);
      };
    }
  } catch (e) {}

  // 8. Hide HeadlessChrome from the UA string in case the outer override misses.
  try {
    const uaPatched = navigator.userAgent.replace(/HeadlessChrome/g, 'Chrome');
    if (uaPatched !== navigator.userAgent) {
      Object.defineProperty(Navigator.prototype, 'userAgent', {
        get: () => uaPatched,
        configurable: true,
      });
    }
  } catch (e) {}
})();
"#;

/// Apply all stealth patches to a freshly-created page.
///
/// Call this **before** navigating to the target URL. The correct pattern is:
///
/// 1. `browser.new_page("about:blank")` — creates the page
/// 2. `apply_stealth(&page)` — installs UA override + document-load script
/// 3. `page.goto(real_url)` — navigates; stealth is already active
///
/// Calling this after navigation still installs the script for *subsequent*
/// navigations but won't retroactively patch the current document.
pub async fn apply_stealth(page: &Page) -> Result<(), crate::Error> {
    // a) User-Agent override via CDP. Covers both the HTTP request header
    //    (Accept-Language, UA-CH hints) and `navigator.userAgent` in JS.
    let ua_params = SetUserAgentOverrideParams::builder()
        .user_agent(STEALTH_USER_AGENT.to_string())
        .accept_language("en-US,en;q=0.9".to_string())
        .platform("MacIntel".to_string())
        .build()
        .map_err(|e| crate::Error::Browser(format!("stealth: UA params build failed: {e}")))?;

    page.execute(ua_params)
        .await
        .map_err(|e| crate::Error::Browser(format!("stealth: UA override failed: {e}")))?;

    // b) Document-load script injection. Runs before any page JS on every
    //    new document (including subframes).
    let script_params = AddScriptToEvaluateOnNewDocumentParams::new(STEALTH_SCRIPT.to_string());
    page.execute(script_params)
        .await
        .map_err(|e| crate::Error::Browser(format!("stealth: script injection failed: {e}")))?;

    tracing::debug!("stealth mode applied: UA override + document-load patches");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stealth_script_is_valid_iife() {
        // The script must be a single self-invoking expression.
        assert!(STEALTH_SCRIPT.trim_start().starts_with("(()"));
        assert!(STEALTH_SCRIPT.trim_end().ends_with(")();"));
    }

    #[test]
    fn stealth_flags_contain_automation_disable() {
        assert!(
            STEALTH_FLAGS
                .iter()
                .any(|f| f.contains("AutomationControlled"))
        );
    }

    #[test]
    fn stealth_user_agent_has_no_headless_marker() {
        assert!(!STEALTH_USER_AGENT.contains("Headless"));
        assert!(STEALTH_USER_AGENT.contains("Chrome/"));
        assert!(STEALTH_USER_AGENT.contains("Macintosh"));
    }
}
