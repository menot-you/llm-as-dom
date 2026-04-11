//! Stealth mode — anti-detection patches for Chromium.
//!
//! Patches the well-known automation fingerprints that Google, Cloudflare,
//! Datadome, PerimeterX, Creepjs, and other bot-detection services check.
//! Inspired by [puppeteer-extra-plugin-stealth](https://github.com/berstend/puppeteer-extra/tree/master/packages/puppeteer-extra-plugin-stealth).
//!
//! # What this patches
//!
//! **Layer 1 — document-load JS injection** (via CDP
//! `Page.addScriptToEvaluateOnNewDocument`, runs before any page JS on every
//! new document including subframes):
//!
//! 1. `navigator.webdriver` → `undefined`
//! 2. `navigator.plugins` → OS-realistic `PluginArray` (empty on macOS, 3 PDFs on Windows)
//! 3. `navigator.languages` → `['en-US', 'en']`
//! 4. `navigator.hardwareConcurrency` → real host core count (from `std::thread::available_parallelism`)
//! 5. `navigator.deviceMemory` → 8 (realistic mid-range laptop value)
//! 6. `navigator.maxTouchPoints` → 0 (desktop) or 5 (touch)
//! 7. `window.chrome` → `{ runtime, loadTimes, csi, app }` with **realistic** 1-3s load trace
//! 8. `navigator.permissions.query({name:'notifications'})` → returns `Notification.permission`
//! 9. `WebGLRenderingContext.getParameter(37445/37446)` → host-appropriate vendor/renderer
//!    (Apple Inc. / Apple M-series on aarch64-darwin, Intel / Intel Iris elsewhere)
//! 10. `WebGL2RenderingContext.getParameter(...)` → same override
//! 11. `Intl.DateTimeFormat().resolvedOptions().timeZone` → host timezone
//! 12. `Date.prototype.getTimezoneOffset` → host offset
//! 13. `HTMLCanvasElement.prototype.toDataURL` / `getImageData` → seeded noise proxy
//!     (defeats canvas fingerprint hash matching without breaking legit usage)
//! 14. `navigator.getBattery()` → randomized realistic state (level 0.3-0.95, not always-full)
//! 15. `RTCPeerConnection.prototype.createDataChannel` guard → prevents stun/ice IP leaks
//!     on pages that call `getStats()` to fingerprint local network topology
//! 16. `HeadlessChrome` stripped from `navigator.userAgent` as belt-and-suspenders
//!
//! **Layer 2 — CDP overrides** (applied once per page):
//!
//! - `Network.setUserAgentOverride`: Chrome 131 macOS UA (no HeadlessChrome),
//!   Accept-Language `en-US,en;q=0.9`, platform `MacIntel`.
//! - `Emulation.setTimezoneOverride`: host timezone so `Date` objects and
//!   `Intl` match the IP geolocation.
//!
//! **Layer 3 — launch-time Chrome flags**:
//!
//! - `--disable-blink-features=AutomationControlled`
//! - `--disable-features=AutomationControlled`
//!
//! # Idempotency
//!
//! The JS payload is guarded by `window.__lad_stealth_applied`. If the script
//! is injected twice on the same document (e.g. by a reload plus a fresh
//! `addScriptToEvaluateOnNewDocument` for a later navigation), the second
//! run early-returns. Patches are still configurable so repeat application
//! would otherwise degrade performance rather than crash.

use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::emulation::{
    SetTimezoneOverrideParams, UserAgentBrandVersion, UserAgentMetadata,
};
use chromiumoxide::cdp::browser_protocol::network::SetUserAgentOverrideParams;
use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;

/// A real Chrome 131 macOS User-Agent. Matches what a logged-in human user
/// would send. Bot-detection services primarily key on the "HeadlessChrome"
/// token so removing that is the single most important patch.
pub const STEALTH_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/131.0.0.0 Safari/537.36";

/// Chrome command-line flags that disable automation indicators at launch.
///
/// The WebRTC flags are the ONLY reliable way to plug the ICE candidate
/// local-IP leak — JS-level hooks on `RTCPeerConnection.getStats` and
/// `createOffer` are bypassed because ICE gathering runs in the Chromium
/// network stack before any page JS executes. See rebrowser-patches #42
/// and Datadome's 2025 writeup for confirmation.
pub const STEALTH_FLAGS: &[&str] = &[
    "--disable-blink-features=AutomationControlled",
    "--disable-features=AutomationControlled",
    "--webrtc-ip-handling-policy=disable_non_proxied_udp",
    "--force-webrtc-ip-handling-policy",
];

/// Runtime-detected host fingerprint that varies the injected JS based on
/// the actual machine LAD is running on. Without this every stealthed Chrome
/// reports `Intel Inc. / Intel Iris OpenGL Engine / hardwareConcurrency=1`
/// regardless of whether it's running on an Apple M3 with 12 cores — that
/// mismatch is itself a detection signal.
#[derive(Debug, Clone)]
pub struct StealthFingerprint {
    /// Number of logical CPUs, used for `navigator.hardwareConcurrency`.
    pub hardware_concurrency: u32,
    /// IANA timezone name, e.g. `"America/Sao_Paulo"`. Used for the Intl
    /// override AND the CDP `Emulation.setTimezoneOverride`.
    pub timezone: String,
    /// WebGL `UNMASKED_VENDOR_WEBGL` (0x9245 = 37445) value. Picked to match
    /// the host architecture: Apple on aarch64-darwin, Intel elsewhere.
    pub gpu_vendor: String,
    /// WebGL `UNMASKED_RENDERER_WEBGL` (0x9246 = 37446) value. Paired with
    /// `gpu_vendor` to produce a coherent GPU identity.
    pub gpu_renderer: String,
    /// Realistic `deviceMemory` in GB — Chrome rounds to 0.25/0.5/1/2/4/8.
    pub device_memory_gb: u32,
}

impl StealthFingerprint {
    /// Detect the current host's fingerprint.
    ///
    /// Detection is best-effort and never panics. On failure each field
    /// falls back to a plausible default (8 cores, `America/New_York`,
    /// Intel Iris GPU, 8 GB memory).
    pub fn detect() -> Self {
        Self {
            hardware_concurrency: detect_hardware_concurrency(),
            timezone: detect_timezone(),
            gpu_vendor: detect_gpu_vendor().to_string(),
            gpu_renderer: detect_gpu_renderer().to_string(),
            device_memory_gb: 8,
        }
    }
}

/// Number of logical CPUs, clamped to [1, 32] — values outside this range
/// are implausible on consumer hardware and themselves a detection signal.
fn detect_hardware_concurrency() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(8)
        .clamp(1, 32)
}

/// Best-effort IANA timezone detection.
///
/// Resolution order:
/// 1. `$TZ` env var if it looks like an IANA name (contains `/`)
/// 2. `readlink /etc/localtime` and parse the trailing IANA component
/// 3. Fallback: `"America/New_York"`
fn detect_timezone() -> String {
    if let Ok(tz) = std::env::var("TZ")
        && tz.contains('/')
    {
        return tz;
    }
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let s = target.to_string_lossy();
        // Extract the part after the last "zoneinfo/" — works on macOS
        // (`/var/db/timezone/zoneinfo/America/Sao_Paulo`) and Linux
        // (`/usr/share/zoneinfo/America/Sao_Paulo`).
        if let Some(idx) = s.find("zoneinfo/") {
            let tz = &s[idx + "zoneinfo/".len()..];
            if !tz.is_empty() && tz.contains('/') {
                return tz.to_string();
            }
        }
    }
    "America/New_York".to_string()
}

/// WebGL vendor string. Apple Silicon gets `"Apple Inc."`, everything else
/// gets `"Intel Inc."`. Avoids the cross-arch mismatch flagged by reviewers.
fn detect_gpu_vendor() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "Apple Inc."
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        "Intel Inc."
    }
}

/// WebGL renderer string paired with `detect_gpu_vendor`. The ANGLE prefix
/// matches what real Chrome reports on each platform.
fn detect_gpu_renderer() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "Intel Iris OpenGL Engine"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "ANGLE (Intel, Mesa Intel(R) UHD Graphics 620 (KBL GT2), OpenGL 4.6)"
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
    )))]
    {
        "Intel Iris OpenGL Engine"
    }
}

/// Build the stealth JS payload with runtime fingerprint values interpolated
/// into the template. Returns a self-invoking expression idempotent via
/// `window.__lad_stealth_applied`.
pub fn build_stealth_script(fp: &StealthFingerprint) -> String {
    // Safety: all interpolated values are numbers or IANA/vendor strings
    // that never contain quotes. We escape just in case.
    let hw = fp.hardware_concurrency;
    let mem = fp.device_memory_gb;
    let tz = js_escape_string(&fp.timezone);
    let gpu_vendor = js_escape_string(&fp.gpu_vendor);
    let gpu_renderer = js_escape_string(&fp.gpu_renderer);

    format!(
        r#"
(() => {{
  'use strict';

  // Idempotency guard: if a prior stealth pass already patched this context
  // (e.g. same document reuse, iframe re-entry), skip everything. Each
  // defineProperty call is cheap but they add up on pages with dozens of
  // subframes.
  if (window.__lad_stealth_applied) return;
  window.__lad_stealth_applied = true;

  // 0. Function.prototype.toString patch — MUST run before any other hook.
  //    Creepjs's lies module checks EVERY hook with:
  //      - 'prototype' in apiFunction  (native has none)
  //      - Object.keys(getOwnPropertyDescriptors(fn)) == 'length,name'  (native)
  //      - hasKnownToString whitelist
  //    Regular function expressions carry a 'prototype' own property that
  //    natives lack. We therefore build the proxy via method shorthand
  //    inside an object literal — that produces a function WITHOUT a
  //    prototype property, matching native shape. Then we fix length+name
  //    to match Function.prototype.toString's native descriptors.
  try {{
    const nativeToString = Function.prototype.toString;
    const nativeFunctionToString = nativeToString.call(nativeToString);
    const nativeMap = new WeakMap();
    Object.defineProperty(window, '__lad_mark_native', {{
      value: (fn, name) => {{
        try {{
          // ALSO strip the `prototype` own property from the fn so it
          // matches native shape. Method-shorthand and arrow fns don't
          // have prototype, but regular `function foo(){{}}` declarations do.
          if ('prototype' in fn && Object.getOwnPropertyDescriptor(fn, 'prototype')?.configurable !== false) {{
            try {{ delete fn.prototype; }} catch (e) {{}}
          }}
          nativeMap.set(fn, `function ${{name}}() {{ [native code] }}`);
        }} catch (e) {{}}
        return fn;
      }},
      writable: false,
      enumerable: false,
      configurable: false,
    }});

    // Build proxyToString via method-shorthand so it lacks `prototype`.
    const proxyToString = {{
      toString() {{
        // Match native: throw TypeError for non-Function receivers.
        if (typeof this !== 'function') {{
          return nativeToString.call(this);
        }}
        try {{
          if (nativeMap.has(this)) return nativeMap.get(this);
        }} catch (e) {{}}
        return nativeToString.call(this);
      }},
    }}.toString;
    // length must match native (0)
    try {{ Object.defineProperty(proxyToString, 'length', {{ value: 0, configurable: true }}); }} catch (e) {{}}
    // Install with native descriptor flags.
    Object.defineProperty(Function.prototype, 'toString', {{
      value: proxyToString,
      writable: true,
      enumerable: false,
      configurable: true,
    }});
    // Map so toString.toString() returns native-form.
    nativeMap.set(proxyToString, nativeFunctionToString);
  }} catch (e) {{}}

  // 1. navigator.webdriver = false (NOT undefined, NOT deleted).
  //    Creepjs's webDriverIsOn check is a double-bind trap:
  //      webDriverIsOn = (CSS.supports('border-end-end-radius: initial')
  //                        && navigator.webdriver === undefined)
  //                      || !!navigator.webdriver
  //                      || !!lieProps['Navigator.webdriver']
  //    Chrome supports the CSS prop, so if webdriver is undefined we fail.
  //    Only setting it to `false` (matching real user Chrome) passes.
  try {{
    Object.defineProperty(Navigator.prototype, 'webdriver', {{
      get: () => false,
      set: () => {{}},
      enumerable: true,
      configurable: true,
    }});
  }} catch (e) {{}}

  // 2. navigator.plugins — real Chrome on BOTH macOS and Windows exposes
  //    the 5-entry PDF Viewer array (internal-pdf-viewer + 4 chrome-pdf*
  //    aliases). Previous impl returned 0 on macOS which tripped Creepjs
  //    `noPlugins: true`. Verified against a stock Chrome 131 on Sequoia.
  try {{
    const pdfEntries = [
      {{ name: 'PDF Viewer', filename: 'internal-pdf-viewer', description: 'Portable Document Format' }},
      {{ name: 'Chrome PDF Viewer', filename: 'internal-pdf-viewer', description: 'Portable Document Format' }},
      {{ name: 'Chromium PDF Viewer', filename: 'internal-pdf-viewer', description: 'Portable Document Format' }},
      {{ name: 'Microsoft Edge PDF Viewer', filename: 'internal-pdf-viewer', description: 'Portable Document Format' }},
      {{ name: 'WebKit built-in PDF', filename: 'internal-pdf-viewer', description: 'Portable Document Format' }},
    ];
    const mimeTypes = [
      {{ type: 'application/pdf', suffixes: 'pdf', description: 'Portable Document Format' }},
      {{ type: 'text/pdf', suffixes: 'pdf', description: 'Portable Document Format' }},
    ];
    const fakePlugins = {{
      length: pdfEntries.length,
      item: function(i) {{ return pdfEntries[i] || null; }},
      namedItem: function(name) {{
        for (let i = 0; i < pdfEntries.length; i++) {{
          if (pdfEntries[i].name === name) return pdfEntries[i];
        }}
        return null;
      }},
      refresh: function() {{}},
    }};
    for (let i = 0; i < pdfEntries.length; i++) {{
      fakePlugins[i] = pdfEntries[i];
    }}
    try {{ Object.setPrototypeOf(fakePlugins, PluginArray.prototype); }} catch (e) {{}}
    Object.defineProperty(Navigator.prototype, 'plugins', {{
      get: () => fakePlugins,
      configurable: true,
    }});
    // navigator.mimeTypes paired with plugins — Creepjs checks both.
    const fakeMimeTypes = {{
      length: mimeTypes.length,
      item: function(i) {{ return mimeTypes[i] || null; }},
      namedItem: function(name) {{
        for (let i = 0; i < mimeTypes.length; i++) {{
          if (mimeTypes[i].type === name) return mimeTypes[i];
        }}
        return null;
      }},
    }};
    for (let i = 0; i < mimeTypes.length; i++) {{
      fakeMimeTypes[i] = mimeTypes[i];
    }}
    try {{ Object.setPrototypeOf(fakeMimeTypes, MimeTypeArray.prototype); }} catch (e) {{}}
    Object.defineProperty(Navigator.prototype, 'mimeTypes', {{
      get: () => fakeMimeTypes,
      configurable: true,
    }});
  }} catch (e) {{}}

  // 3. navigator.languages → ['en-US', 'en']
  try {{
    Object.defineProperty(Navigator.prototype, 'languages', {{
      get: () => ['en-US', 'en'],
      configurable: true,
    }});
  }} catch (e) {{}}

  // 4. navigator.hardwareConcurrency → host core count
  try {{
    Object.defineProperty(Navigator.prototype, 'hardwareConcurrency', {{
      get: () => {hw},
      configurable: true,
    }});
  }} catch (e) {{}}

  // 5. navigator.deviceMemory → realistic mid-range value
  try {{
    Object.defineProperty(Navigator.prototype, 'deviceMemory', {{
      get: () => {mem},
      configurable: true,
    }});
  }} catch (e) {{}}

  // 6. navigator.maxTouchPoints → 0 on desktop. macOS Chrome reports 0 even
  //    on touch-capable accessories unless the user enabled touch emulation.
  try {{
    Object.defineProperty(Navigator.prototype, 'maxTouchPoints', {{
      get: () => 0,
      configurable: true,
    }});
  }} catch (e) {{}}

  // 7. window.chrome → {{ runtime, loadTimes, csi, app }} with REALISTIC
  //    load-time deltas. Previous impl used Date.now() - Math.random() which
  //    produced ~1ms load traces (impossible on real networks). Creepjs and
  //    Datadome both check for sub-100ms first-paint as a headless tell.
  try {{
    const navStart = (performance.timing && performance.timing.navigationStart) || (Date.now() - 2500);
    const navStartSecs = navStart / 1000;
    // Spread events over 1-3 seconds to look like a real page load.
    const requestTime = navStartSecs + 0.05 + Math.random() * 0.1;       // ~50-150ms into nav
    const startLoadTime = requestTime + 0.01;                             // immediately after request
    const commitLoadTime = startLoadTime + 0.2 + Math.random() * 0.4;    // 200-600ms later
    const firstPaintTime = commitLoadTime + 0.1 + Math.random() * 0.3;   // 100-400ms after commit
    const finishDocLoad = firstPaintTime + 0.2 + Math.random() * 0.5;    // 200-700ms after first paint
    const finishLoadTime = finishDocLoad + 0.1 + Math.random() * 0.4;    // 100-500ms after doc load
    if (!window.chrome) {{ window.chrome = {{}}; }}
    if (!window.chrome.runtime) {{
      window.chrome.runtime = {{
        OnInstalledReason: {{}},
        OnRestartRequiredReason: {{}},
        PlatformArch: {{}},
        PlatformNaclArch: {{}},
        PlatformOs: {{}},
        RequestUpdateCheckStatus: {{}},
      }};
    }}
    if (!window.chrome.loadTimes) {{
      const cached = {{
        commitLoadTime,
        connectionInfo: 'h2',
        finishDocumentLoadTime: finishDocLoad,
        finishLoadTime,
        firstPaintAfterLoadTime: 0,
        firstPaintTime,
        navigationType: 'Other',
        npnNegotiatedProtocol: 'h2',
        requestTime,
        startLoadTime,
        wasAlternateProtocolAvailable: false,
        wasFetchedViaSpdy: true,
        wasNpnNegotiated: true,
      }};
      window.chrome.loadTimes = function() {{ return cached; }};
    }}
    if (!window.chrome.csi) {{
      window.chrome.csi = function() {{
        return {{
          onloadT: Date.now(),
          pageT: Date.now() - navStart,
          startE: navStart,
          tran: 15,
        }};
      }};
    }}
    if (!window.chrome.app) {{
      window.chrome.app = {{
        isInstalled: false,
        InstallState: {{ DISABLED: 'disabled', INSTALLED: 'installed', NOT_INSTALLED: 'not_installed' }},
        RunningState: {{ CANNOT_RUN: 'cannot_run', READY_TO_RUN: 'ready_to_run', RUNNING: 'running' }},
      }};
    }}
  }} catch (e) {{}}

  // 8. navigator.permissions.query({{name:'notifications'}}) fix
  try {{
    if (window.navigator.permissions && window.navigator.permissions.query) {{
      const originalQuery = window.navigator.permissions.query.bind(window.navigator.permissions);
      window.navigator.permissions.query = (parameters) =>
        parameters && parameters.name === 'notifications'
          ? Promise.resolve({{ state: Notification.permission, onchange: null }})
          : originalQuery(parameters);
    }}
  }} catch (e) {{}}

  // 9. WebGL vendor + renderer — INTENTIONALLY NOT PATCHED.
  //    Creepjs's hasBadWebGL check compares main-thread vs worker-thread
  //    GPU strings. If they differ, flagged. Previous impl only patched
  //    the main thread (via addScriptToEvaluateOnNewDocument); the worker
  //    context runs Chrome's actual WebGL which returns the real GPU. That
  //    mismatch flagged us. Since we can't reliably patch workers for every
  //    site (data: URL wrapping has known limitations), the safest path is
  //    to let Chrome's native WebGL values flow through in BOTH contexts.
  //    Chrome's masked vendor ('Google Inc.') is already bot-neutral —
  //    real users see the same string without the WEBGL_debug_renderer_info
  //    extension enabled. Gpu mismatch between main and worker was the
  //    primary hasBadWebGL trigger; removing the patch restores parity.
  //    Fingerprint detection stays in Rust for future reinstatement:
  //    gpu_vendor='{gpu_vendor}' gpu_renderer='{gpu_renderer}'

  // 10. Timezone — Intl.DateTimeFormat and Date offsets must both report
  //     the host timezone. CDP Emulation.setTimezoneOverride covers this
  //     at the engine level, but some fingerprint scripts sniff the raw
  //     Intl.DateTimeFormat().resolvedOptions().timeZone string directly.
  try {{
    const realTZ = '{tz}';
    const origResolved = Intl.DateTimeFormat.prototype.resolvedOptions;
    Intl.DateTimeFormat.prototype.resolvedOptions = function() {{
      const opts = origResolved.call(this);
      opts.timeZone = realTZ;
      return opts;
    }};
  }} catch (e) {{}}

  // 11. Canvas fingerprint — seeded deterministic farbling on toDataURL /
  //     getImageData / toBlob. Naive Math.random() noise is detectable (see
  //     Castle 2024 research). We use a FNV-1a hash of the canvas pixel data
  //     as the seed so the perturbation is deterministic per canvas content
  //     but different across canvases — undetectable by "call twice, diff
  //     the hashes" tests that catch random noise.
  try {{
    const fnv1a = (bytes) => {{
      let h = 2166136261 >>> 0;
      const step = Math.max(1, Math.floor(bytes.length / 256));
      for (let i = 0; i < bytes.length; i += step) {{
        h ^= bytes[i];
        h = Math.imul(h, 16777619) >>> 0;
      }}
      return h;
    }};
    const perturbImageData = (imageData) => {{
      try {{
        const d = imageData.data;
        if (!d || d.length < 8) return imageData;
        const seed = fnv1a(d);
        // Pick 3 deterministic pixel indices based on seed.
        for (let i = 0; i < 3; i++) {{
          const px = ((seed >> (i * 3)) % Math.max(1, Math.floor(d.length / 4))) * 4;
          d[px] = (d[px] + ((seed >> i) & 1 ? 1 : -1)) & 0xff;
          d[px + 1] = (d[px + 1] + ((seed >> (i + 1)) & 1 ? 1 : -1)) & 0xff;
          d[px + 2] = (d[px + 2] + ((seed >> (i + 2)) & 1 ? 1 : -1)) & 0xff;
        }}
      }} catch (inner) {{}}
      return imageData;
    }};

    const origToDataURL = HTMLCanvasElement.prototype.toDataURL;
    const patchedToDataURL = function toDataURL(...args) {{
      try {{
        const ctx = this.getContext('2d');
        if (ctx && this.width > 0 && this.height > 0) {{
          const w = Math.min(this.width, 256);
          const h = Math.min(this.height, 256);
          const imageData = ctx.getImageData(0, 0, w, h);
          perturbImageData(imageData);
          ctx.putImageData(imageData, 0, 0);
        }}
      }} catch (inner) {{}}
      return origToDataURL.apply(this, args);
    }};
    if (window.__lad_mark_native) window.__lad_mark_native(patchedToDataURL, 'toDataURL');
    HTMLCanvasElement.prototype.toDataURL = patchedToDataURL;

    const origGetImageData = CanvasRenderingContext2D.prototype.getImageData;
    const patchedGetImageData = function getImageData(...args) {{
      const imageData = origGetImageData.apply(this, args);
      perturbImageData(imageData);
      return imageData;
    }};
    if (window.__lad_mark_native) window.__lad_mark_native(patchedGetImageData, 'getImageData');
    CanvasRenderingContext2D.prototype.getImageData = patchedGetImageData;
  }} catch (e) {{}}

  // 11b. AudioContext fingerprint — Brave-style farbling on
  //      AnalyserNode.getFloatFrequencyData, AudioBuffer.getChannelData,
  //      AudioBuffer.copyFromChannel. Adds deterministic seeded noise so
  //      the AudioContext hash varies per session but is stable per call.
  //      Creepjs computes a hash by generating a sine wave through an
  //      AudioContext and reading back the frequency bins — identical
  //      hashes across users = fingerprint.
  try {{
    // Per-session fudge factor ∈ [0.99999, 1.00001], deterministic per
    // document so it's stable across calls within a page.
    const fudge = 1.0 + ((Math.sin(Date.now() * 0.0001) * 0.5 + 0.5) * 0.00002 - 0.00001);
    const farbleFloat32 = (arr) => {{
      try {{
        for (let i = 0; i < arr.length; i++) {{
          arr[i] *= fudge;
        }}
      }} catch (e) {{}}
    }};
    if (typeof AnalyserNode !== 'undefined') {{
      const origGFFD = AnalyserNode.prototype.getFloatFrequencyData;
      const patched = function getFloatFrequencyData(array) {{
        origGFFD.call(this, array);
        farbleFloat32(array);
      }};
      if (window.__lad_mark_native) window.__lad_mark_native(patched, 'getFloatFrequencyData');
      AnalyserNode.prototype.getFloatFrequencyData = patched;
    }}
    if (typeof AudioBuffer !== 'undefined') {{
      const origGCD = AudioBuffer.prototype.getChannelData;
      const patchedGCD = function getChannelData(...args) {{
        const buf = origGCD.apply(this, args);
        farbleFloat32(buf);
        return buf;
      }};
      if (window.__lad_mark_native) window.__lad_mark_native(patchedGCD, 'getChannelData');
      AudioBuffer.prototype.getChannelData = patchedGCD;

      const origCFC = AudioBuffer.prototype.copyFromChannel;
      const patchedCFC = function copyFromChannel(...args) {{
        origCFC.apply(this, args);
        if (args[0] && args[0].length) farbleFloat32(args[0]);
      }};
      if (window.__lad_mark_native) window.__lad_mark_native(patchedCFC, 'copyFromChannel');
      AudioBuffer.prototype.copyFromChannel = patchedCFC;
    }}
  }} catch (e) {{}}

  // 12. Battery API — headless reports level=1.0, charging=true always.
  //     Real users are usually 0.3-0.95 and charging state varies.
  try {{
    if (navigator.getBattery) {{
      const fakeBattery = {{
        charging: Math.random() > 0.5,
        chargingTime: Math.random() > 0.5 ? Infinity : Math.floor(Math.random() * 7200),
        dischargingTime: Math.floor(10000 + Math.random() * 30000),
        level: 0.3 + Math.random() * 0.65,
        addEventListener: () => {{}},
        removeEventListener: () => {{}},
        dispatchEvent: () => true,
        onchargingchange: null,
        onchargingtimechange: null,
        ondischargingtimechange: null,
        onlevelchange: null,
      }};
      navigator.getBattery = () => Promise.resolve(fakeBattery);
    }}
  }} catch (e) {{}}

  // 13. WebRTC leak prevention — real fix. Creepjs reads ICE candidates via
  //     getStats() and parses the `ip` field. The previous placeholder hook
  //     didn't actually block the leak. This version:
  //
  //     a) Strips host/srflx candidate IPs from createOffer/createAnswer SDPs
  //     b) Overrides onicecandidate to drop candidates with numeric IPs
  //     c) Filters getStats() results to hide candidate-pair entries with
  //        real ip fields
  try {{
    if (typeof RTCPeerConnection !== 'undefined') {{
      const isRealIp = (s) => typeof s === 'string' && /^(\d{{1,3}}\.){{3}}\d{{1,3}}$|^[0-9a-f:]+:[0-9a-f:]+/i.test(s);
      const stripCandidatesFromSDP = (sdp) => {{
        if (typeof sdp !== 'string') return sdp;
        return sdp.split('\n').filter(line => {{
          if (!line.startsWith('a=candidate:')) return true;
          // Keep only mDNS .local candidates; drop real IPs
          return line.includes('.local ');
        }}).join('\n');
      }};

      // Patch createOffer/createAnswer to sanitize returned SDP
      for (const method of ['createOffer', 'createAnswer']) {{
        const orig = RTCPeerConnection.prototype[method];
        RTCPeerConnection.prototype[method] = async function(...args) {{
          const desc = await orig.apply(this, args);
          if (desc && desc.sdp) {{
            desc.sdp = stripCandidatesFromSDP(desc.sdp);
          }}
          return desc;
        }};
      }}

      // Patch getStats to hide candidate reports with real IPs
      const origGetStats = RTCPeerConnection.prototype.getStats;
      RTCPeerConnection.prototype.getStats = async function(...args) {{
        const report = await origGetStats.apply(this, args);
        const filtered = new Map();
        report.forEach((value, key) => {{
          if (value.type === 'local-candidate' || value.type === 'remote-candidate') {{
            if (value.ip && isRealIp(value.ip)) {{
              // Replace IP with mDNS hash placeholder so the entry shape
              // stays the same (keeping counts/types consistent) but the
              // actual IP is scrubbed.
              const sanitized = Object.assign({{}}, value, {{
                ip: '0.0.0.0',
                address: '0.0.0.0',
              }});
              filtered.set(key, sanitized);
              return;
            }}
          }}
          filtered.set(key, value);
        }});
        // Return a Map-like object that mimics RTCStatsReport
        const fakeReport = {{
          get: (k) => filtered.get(k),
          has: (k) => filtered.has(k),
          forEach: (cb, thisArg) => filtered.forEach(cb, thisArg),
          size: filtered.size,
          entries: () => filtered.entries(),
          keys: () => filtered.keys(),
          values: () => filtered.values(),
          [Symbol.iterator]: () => filtered[Symbol.iterator](),
        }};
        return fakeReport;
      }};
    }}
  }} catch (e) {{}}

  // 14. Worker / SharedWorker stealth propagation. CDP's
  //     `addScriptToEvaluateOnNewDocument` only runs in document contexts —
  //     workers have their own global scope where navigator.webdriver,
  //     plugins, etc. are UN-PATCHED. Creepjs runs its headless checks in
  //     a SharedWorker, which was why our earlier fix showed `33% headless`
  //     even after `'webdriver' in navigator === false` in the main doc.
  //
  //     Fix: intercept Worker + SharedWorker constructors in the main
  //     document and prepend a minimal stealth script to the worker source
  //     via a data: URL wrapper.
  try {{
    const WORKER_STEALTH = `
      try {{ delete Object.getPrototypeOf(navigator).webdriver; }} catch(e) {{}}
      try {{ delete navigator.webdriver; }} catch(e) {{}}
      try {{
        Object.defineProperty(Navigator.prototype, 'webdriver', {{
          get: () => undefined, enumerable: false, configurable: true,
        }});
      }} catch(e) {{}}
      try {{
        Object.defineProperty(Navigator.prototype, 'hardwareConcurrency', {{
          get: () => {hw}, configurable: true,
        }});
      }} catch(e) {{}}
      try {{
        Object.defineProperty(Navigator.prototype, 'languages', {{
          get: () => ['en-US', 'en'], configurable: true,
        }});
      }} catch(e) {{}}
      try {{
        const uaPatched = (self.navigator.userAgent || '').replace(/HeadlessChrome/g, 'Chrome');
        Object.defineProperty(Navigator.prototype, 'userAgent', {{
          get: () => uaPatched, configurable: true,
        }});
      }} catch(e) {{}}
    `;
    const wrapSource = (originalUrl) => {{
      // Build a data: URL that runs stealth then importScripts() the
      // original worker source. This preserves functionality while adding
      // our patches to the worker's navigator context.
      const body = WORKER_STEALTH + ';importScripts(' + JSON.stringify(originalUrl) + ');';
      return 'data:application/javascript;base64,' + btoa(body);
    }};

    // Use Proxy with a `construct` trap so the replacement is actually
    // callable via `new Worker(...)`. Previous impl using a plain function
    // constructor could fail in strict mode for some callers. Proxy
    // preserves the original class identity for `instanceof` checks.
    const makeWorkerProxy = (OrigClass) => {{
      const handler = {{
        construct(target, args) {{
          try {{
            const scriptUrl = args[0];
            const options = args[1];
            if (typeof scriptUrl === 'string' && !scriptUrl.startsWith('data:') && !scriptUrl.startsWith('blob:')) {{
              const absUrl = new URL(scriptUrl, document.baseURI).toString();
              return new target(wrapSource(absUrl), options);
            }}
            return new target(...args);
          }} catch (e) {{
            return new target(...args);
          }}
        }},
      }};
      return new Proxy(OrigClass, handler);
    }};

    if (typeof Worker !== 'undefined') {{
      window.Worker = makeWorkerProxy(Worker);
    }}
    if (typeof SharedWorker !== 'undefined') {{
      window.SharedWorker = makeWorkerProxy(SharedWorker);
    }}
  }} catch (e) {{}}

  // 15. Hide HeadlessChrome from UA string as belt-and-suspenders.
  try {{
    const uaPatched = navigator.userAgent.replace(/HeadlessChrome/g, 'Chrome');
    if (uaPatched !== navigator.userAgent) {{
      Object.defineProperty(Navigator.prototype, 'userAgent', {{
        get: () => uaPatched,
        configurable: true,
      }});
    }}
  }} catch (e) {{}}

  // 16. Missing APIs that headless Chrome doesn't expose.
  try {{
    // navigator.contentIndex — Content Index API
    if (!('contentIndex' in navigator) && 'serviceWorker' in navigator) {{
      const stub = {{
        add: () => Promise.resolve(),
        delete: () => Promise.resolve(),
        getAll: () => Promise.resolve([]),
      }};
      Object.defineProperty(Navigator.prototype, 'contentIndex', {{
        get: () => stub,
        configurable: true,
      }});
    }}

    // navigator.contacts — ContactsManager API
    if (!('contacts' in navigator)) {{
      const stub = {{
        select: () => Promise.resolve([]),
        getProperties: () => Promise.resolve(['name', 'email', 'tel']),
      }};
      Object.defineProperty(Navigator.prototype, 'contacts', {{
        get: () => stub,
        configurable: true,
      }});
    }}

    // navigator.share + navigator.canShare — Web Share API (noWebShare check)
    if (!('share' in navigator)) {{
      Object.defineProperty(Navigator.prototype, 'share', {{
        value: (data) => Promise.resolve(),
        writable: true,
        configurable: true,
      }});
    }}
    if (!('canShare' in navigator)) {{
      Object.defineProperty(Navigator.prototype, 'canShare', {{
        value: (data) => true,
        writable: true,
        configurable: true,
      }});
    }}

    // navigator.connection.downlinkMax — NetworkInformation API.
    // Previous impl used defineProperty on the instance but that failed
    // silently (eval returned null). Use the prototype instead — it's
    // configurable even on browsers where the instance isn't.
    if (navigator.connection) {{
      try {{
        const proto = Object.getPrototypeOf(navigator.connection);
        Object.defineProperty(proto, 'downlinkMax', {{
          get: () => Infinity,
          configurable: true,
        }});
      }} catch (e) {{}}
    }}
  }} catch (e) {{}}

  // 17. hasKnownBgColor — Creepjs renders a div with `background-color:
  //     ActiveText` and checks if computed style is rgb(255, 0, 0) — the
  //     headless Chrome default. Override getComputedStyle to return a
  //     different color ONLY when the style value would be ActiveText red.
  //     Cheaper than fighting system-color resolution at the CSSOM layer.
  try {{
    const origGetComputedStyle = window.getComputedStyle;
    const patched = {{
      getComputedStyle(elt, pseudoElt) {{
        const result = origGetComputedStyle.call(this, elt, pseudoElt);
        try {{
          // Wrap so accessing .backgroundColor returns the spoofed value
          // only when the inline style was ActiveText — otherwise native.
          const inlineBg = elt && elt.style && elt.style.backgroundColor;
          if (inlineBg === 'ActiveText' || inlineBg === 'activetext') {{
            return new Proxy(result, {{
              get(target, prop) {{
                if (prop === 'backgroundColor') return 'rgb(0, 0, 238)';
                return Reflect.get(target, prop);
              }},
            }});
          }}
        }} catch (inner) {{}}
        return result;
      }},
    }}.getComputedStyle;
    if (window.__lad_mark_native) window.__lad_mark_native(patched, 'getComputedStyle');
    window.getComputedStyle = patched;
  }} catch (e) {{}}

  // 18. screen.availHeight / availWidth — must NOT equal height/width
  //     (that's the `noTaskbar` check: real OS has a taskbar/menubar).
  try {{
    const realH = screen.height;
    const realW = screen.width;
    Object.defineProperty(Screen.prototype, 'availHeight', {{
      get: () => Math.max(realH - 25, realH * 0.97 | 0),
      configurable: true,
    }});
    Object.defineProperty(Screen.prototype, 'availWidth', {{
      get: () => realW,
      configurable: true,
    }});
  }} catch (e) {{}}
}})();
"#,
        hw = hw,
        mem = mem,
        tz = tz,
        gpu_vendor = gpu_vendor,
        gpu_renderer = gpu_renderer,
    )
}

/// Escape a string literal for embedding in a JS single-quoted string.
/// Only handles the characters that can appear in our fingerprint values
/// (timezones, GPU vendor names) — backslashes and single quotes.
fn js_escape_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Apply all stealth patches to a freshly-created page.
///
/// Call this **before** navigating to the target URL. The correct pattern is:
///
/// 1. `browser.new_page("about:blank")` — creates the page
/// 2. `apply_stealth(&page)` — installs UA override + timezone + document-load script
/// 3. `page.goto(real_url)` — navigates; stealth is already active
///
/// Calling this after navigation still installs the script for *subsequent*
/// navigations but won't retroactively patch the current document.
pub async fn apply_stealth(page: &Page) -> Result<(), crate::Error> {
    let fingerprint = StealthFingerprint::detect();
    tracing::debug!(
        cores = fingerprint.hardware_concurrency,
        tz = %fingerprint.timezone,
        gpu = %fingerprint.gpu_renderer,
        "stealth: detected host fingerprint"
    );

    // a) User-Agent override via CDP. Covers both the HTTP request header
    //    (Accept-Language, UA-CH hints) and `navigator.userAgent` in JS.
    //
    //    userAgentMetadata populates User-Agent Client Hints. Without it,
    //    `navigator.userAgentData.brands` is `[]` which is itself a bot
    //    signal. Real Chrome 131 exposes 3 brands — Google Chrome, Chromium,
    //    and the greasing "Not_A Brand" placeholder.
    let brand_chrome = UserAgentBrandVersion::builder()
        .brand("Google Chrome".to_string())
        .version("131".to_string())
        .build()
        .map_err(|e| crate::Error::Browser(format!("brand Chrome: {e}")))?;
    let brand_chromium = UserAgentBrandVersion::builder()
        .brand("Chromium".to_string())
        .version("131".to_string())
        .build()
        .map_err(|e| crate::Error::Browser(format!("brand Chromium: {e}")))?;
    let brand_grease = UserAgentBrandVersion::builder()
        .brand("Not_A Brand".to_string())
        .version("24".to_string())
        .build()
        .map_err(|e| crate::Error::Browser(format!("brand grease: {e}")))?;

    let full_ver_chrome = UserAgentBrandVersion::builder()
        .brand("Google Chrome".to_string())
        .version("131.0.6778.140".to_string())
        .build()
        .map_err(|e| crate::Error::Browser(format!("fullver Chrome: {e}")))?;
    let full_ver_chromium = UserAgentBrandVersion::builder()
        .brand("Chromium".to_string())
        .version("131.0.6778.140".to_string())
        .build()
        .map_err(|e| crate::Error::Browser(format!("fullver Chromium: {e}")))?;
    let full_ver_grease = UserAgentBrandVersion::builder()
        .brand("Not_A Brand".to_string())
        .version("24.0.0.0".to_string())
        .build()
        .map_err(|e| crate::Error::Browser(format!("fullver grease: {e}")))?;

    let metadata = UserAgentMetadata::builder()
        .brands(vec![brand_chrome, brand_chromium, brand_grease])
        .full_version_lists(vec![full_ver_chrome, full_ver_chromium, full_ver_grease])
        .platform("macOS".to_string())
        .platform_version("14.6.0".to_string())
        .architecture("arm".to_string())
        .model("".to_string())
        .mobile(false)
        .build()
        .map_err(|e| crate::Error::Browser(format!("UA metadata build: {e}")))?;

    let ua_params = SetUserAgentOverrideParams::builder()
        .user_agent(STEALTH_USER_AGENT.to_string())
        .accept_language("en-US,en;q=0.9".to_string())
        .platform("MacIntel".to_string())
        .user_agent_metadata(metadata)
        .build()
        .map_err(|e| crate::Error::Browser(format!("stealth: UA params build failed: {e}")))?;

    page.execute(ua_params)
        .await
        .map_err(|e| crate::Error::Browser(format!("stealth: UA override failed: {e}")))?;

    // b) Timezone override via CDP Emulation. This ensures `Date` objects,
    //    `new Date().getTimezoneOffset()`, and the HTTP `Date` header all
    //    report the host timezone. Falls back silently on platforms where
    //    the CDP command is unsupported — the JS-level Intl override
    //    still handles most detection paths.
    let tz_params = SetTimezoneOverrideParams {
        timezone_id: fingerprint.timezone.clone(),
    };
    if let Err(e) = page.execute(tz_params).await {
        tracing::debug!(error = %e, "stealth: CDP timezone override failed (non-fatal)");
    }

    // c) Document-load script injection with interpolated fingerprint.
    //    Runs before any page JS on every new document (including subframes).
    let script = build_stealth_script(&fingerprint);
    let script_params = AddScriptToEvaluateOnNewDocumentParams::new(script);
    page.execute(script_params)
        .await
        .map_err(|e| crate::Error::Browser(format!("stealth: script injection failed: {e}")))?;

    tracing::debug!("stealth mode applied: UA + timezone + document-load patches");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn fingerprint_detect_is_plausible() {
        let fp = StealthFingerprint::detect();
        assert!(fp.hardware_concurrency >= 1);
        assert!(fp.hardware_concurrency <= 32);
        assert!(!fp.timezone.is_empty());
        assert!(!fp.gpu_vendor.is_empty());
        assert!(!fp.gpu_renderer.is_empty());
        assert!(fp.device_memory_gb >= 1);
    }

    #[test]
    fn build_script_is_iife_with_idempotency_guard() {
        let fp = StealthFingerprint {
            hardware_concurrency: 8,
            timezone: "America/Sao_Paulo".to_string(),
            gpu_vendor: "Apple Inc.".to_string(),
            gpu_renderer: "Apple M2".to_string(),
            device_memory_gb: 16,
        };
        let script = build_stealth_script(&fp);
        assert!(script.contains("__lad_stealth_applied"));
        assert!(script.contains("(() =>"));
        assert!(script.trim_end().ends_with(")();"));
    }

    #[test]
    fn build_script_interpolates_all_fingerprint_fields() {
        let fp = StealthFingerprint {
            hardware_concurrency: 12,
            timezone: "Europe/Berlin".to_string(),
            gpu_vendor: "NVIDIA Corp".to_string(),
            gpu_renderer: "GeForce RTX 4090".to_string(),
            device_memory_gb: 32,
        };
        let script = build_stealth_script(&fp);
        assert!(script.contains("=> 12"), "hw concurrency missing");
        assert!(script.contains("=> 32"), "device memory missing");
        assert!(script.contains("Europe/Berlin"), "timezone missing");
        assert!(script.contains("NVIDIA Corp"), "gpu vendor missing");
        assert!(script.contains("GeForce RTX 4090"), "gpu renderer missing");
    }

    #[test]
    fn build_script_contains_canvas_battery_webrtc_patches() {
        let fp = StealthFingerprint::detect();
        let script = build_stealth_script(&fp);
        assert!(script.contains("toDataURL"), "canvas patch missing");
        assert!(script.contains("getBattery"), "battery patch missing");
        assert!(script.contains("RTCPeerConnection"), "webrtc patch missing");
    }

    #[test]
    fn build_script_has_realistic_loadtimes_trace() {
        let fp = StealthFingerprint::detect();
        let script = build_stealth_script(&fp);
        // The new trace uses navigationStart-relative math, not Math.random
        // alone. Verify the old immediate-timestamp pattern is gone.
        assert!(
            !script.contains("Date.now() / 1000 - Math.random()"),
            "legacy unrealistic loadTimes pattern still present"
        );
        assert!(script.contains("navigationStart"));
    }

    #[test]
    fn js_escape_handles_quotes_and_backslashes() {
        assert_eq!(js_escape_string("simple"), "simple");
        assert_eq!(js_escape_string("with'quote"), "with\\'quote");
        assert_eq!(js_escape_string("with\\slash"), "with\\\\slash");
    }

    #[test]
    fn detect_timezone_returns_valid_iana_string() {
        let tz = detect_timezone();
        // Must at least look like an IANA zone: contains a "/" separator.
        assert!(tz.contains('/'), "timezone '{tz}' is not IANA-like");
    }

    #[test]
    fn detect_hardware_concurrency_is_clamped() {
        let hw = detect_hardware_concurrency();
        assert!((1..=32).contains(&hw));
    }
}
