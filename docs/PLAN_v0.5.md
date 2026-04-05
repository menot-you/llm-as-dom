# lad v0.5.0 — Multi-Engine Release Plan

## Context: Two Sessions → One Release

**Session 1** (d94ebfce): v0.3→v0.4→v0.5 + start of v0.6
- Built: session/cookies, multi-page, OAuth, hard scenarios, interactive mode, Chrome --app
- Built: crypto.rs (Keychain decrypt), selector.rs, network.rs
- Designed: observer/watcher (ultrathink approved, never implemented)
- Created: CI pipeline (crates.io + npm + pypi + GitHub Releases + install.sh)
- Secrets: CARGO_REGISTRY_TOKEN, NPM_TOKEN, PYPI_TOKEN, Z_AI_API_KEY — all configured

**Session 2** (current): multi-engine + WebKit
- Built: BrowserEngine + PageHandle traits (9 methods)
- Built: ChromiumAdapter, WebKitAdapter (Rust 530 LOC)
- Built: Swift sidecar lad-webkit-bridge (441 LOC)
- Proven: E2E works on example.com, HN, GitHub login
- Oracle review: Codex + Gemini identified 11 gaps

## Current State

```
Crates.io published:  0.4.1
Local HEAD:           482cc0e (6 unpushed commits)
Remote HEAD:          33116ce
Tests:                341 (all passing)
LOC:                  11,013 Rust + 441 Swift
Modules:              31 .rs files + 1 .swift
CI:                   Full pipeline (check → build → release → publish ×3)
Secrets:              All 4 configured
```

## What's Unpushed (6 commits)

```
d0091d0  feat(v0.6): macOS Chrome cookie decryption via Keychain
3d89ce7  feat(v0.6): network traffic capture and semantic classification
d213342  refactor: introduce BrowserEngine + PageHandle traits
36b1796  feat: add WebKit browser engine adapter via macOS sidecar bridge
045cc06  feat: add WebKit sidecar bridge (Swift macOS app)
482cc0e  docs: update README + ARCHITECTURE for multi-engine, add launch posts
```

---

## Gaps: Consolidated from Oracle Reviews + Session Audit

### P0 — Block Release

| # | Gap | Source | File(s) | Impact |
|---|-----|--------|---------|--------|
| 1 | **Sidecar orphan on crash** — no Drop, no kill, no cleanup | Codex C1, Gemini | engine/webkit.rs | Zombie processes in production |
| 2 | **Bridge crash = 30s timeout** — pending map not drained on EOF | Codex C2, Gemini | engine/webkit.rs | Silent failures, bad DX |
| 3 | **Startup handshake missing** — sleep(500ms) instead of "ready" wait | Codex H5 | engine/webkit.rs | Launch succeeds with dead bridge |
| 4 | **Pending leak on write failure** — id inserted before write | Codex H4 | engine/webkit.rs | Memory leak, orphan channels |

### P1 — Ship Quality

| # | Gap | Source | File(s) | Impact |
|---|-----|--------|---------|--------|
| 5 | **NSDate/NSData serialization** — non-JSON types crash | Gemini | main.swift | JS Date() breaks extraction |
| 6 | **Autoreleasepool** in readLoop — memory leak in long sessions | Gemini | main.swift | RAM growth over time |
| 7 | **NavDelegate race** — Array instead of Set for pending waits | Gemini | main.swift | Timeout dedup bug |
| 8 | **HTTPCookie httpOnly** not set during set_cookies | Gemini | main.swift | Cookie behavior mismatch |
| 9 | **Session isolation** — user_data_dir ignored by WebKit | Codex C3 | main.swift, webkit.rs | Cookie cross-contamination |
| 10 | **Version handshake** — no protocol version check | Codex H6 | main.swift, webkit.rs | Silent version mismatches |
| 11 | **selector.rs not wired** into pilot — exists but unused | Session 1 | pilot.rs, heuristics | CSS selectors don't work |

### P2 — Next Release

| # | Gap | Source | File(s) | Impact |
|---|-----|--------|---------|--------|
| 12 | **observer.rs + lad_watch** — approved but never built | Session 1 ultrathink | NEW files | Blocks "lad as sensor" |
| 13 | **Network monitoring on WebKit** — hardcoded false | Both oracles | main.swift, webkit.rs | Feature parity gap |
| 14 | **CI macOS job** for Swift build + webkit-bridge artifact | Both oracles | ci.yml | WebKit not in releases |
| 15 | **Notarization** — Gatekeeper blocks unsigned binary | Gemini | CI | macOS distribution blocked |
| 16 | **Multi-page session test** on WebKit | Audit | tests/ | OAuth/redirect untested |
| 17 | **Multi-tab support** | Session 1 | engine trait? | Feature gap |

---

## Execution Plan: 5 Waves

### Wave 1 — WebKit Process Hardening (Rust)
**Files:** `src/engine/webkit.rs`
**Gaps:** #1, #2, #3, #4
**Parallelizable with:** Wave 2

```
1. Implement Drop for WebKitEngine
   - Close stdin pipe (signals EOF to Swift → clean exit)
   - Abort reader task
   - Wait child with 3s timeout
   - Force kill if timeout

2. Bridge crash detection
   - On read_line() → Ok(0) or Err: drain all pending with BridgeExited error
   - Set bridge_alive: AtomicBool → false
   - Future requests fail-fast: check alive before write

3. Startup handshake
   - Create ready_rx: oneshot in launch()
   - Reader loop resolves it on {"event":"ready"}
   - launch() awaits with 5s timeout → Error if timeout

4. Pending insert after write
   - Serialize + write + flush first
   - Only then insert (id, tx) into pending map
   - On write failure: don't insert, return error immediately
```

**Tests:**
- Unit: BridgeConnection with mock stdin/stdout
- Verify: Drop kills child process
- Verify: dead bridge → immediate error (not 30s timeout)

**Estimate:** ~1.5h

---

### Wave 2 — Swift Hardening
**Files:** `webkit-bridge/Sources/main.swift`
**Gaps:** #5, #6, #7, #8, #9, #10
**Parallelizable with:** Wave 1

```
1. serializeJSResult edge cases
   - case NSDate: return timeIntervalSince1970
   - case NSData: return NSNull() + log warning
   - default: return NSNull() instead of String(describing:)

2. Autoreleasepool
   - Wrap readLoop body in autoreleasepool { }

3. NavDelegate pendingWaits
   - Change from [UInt64] to Set<UInt64>
   - Prevents duplicate timeout handling

4. HTTPCookie httpOnly
   - Add httpOnly property when setting cookies
   - if cd.httpOnly == true { props[.init("HttpOnly")] = "TRUE" }

5. Session isolation
   - Accept LAD_WEBKIT_DATA_DIR env var
   - If set: create WKWebsiteDataStore with custom identifier
   - If not set: use .nonPersistent() for test isolation
   - Default behavior: ephemeral (no leak between sessions)

6. Version handshake
   - Change ready event to: {"event":"ready","version":"0.1.0"}
   - Rust side validates version on startup
```

**Tests:**
- Manual: verify NSDate → number, NSData → null
- Manual: 2 sessions don't share cookies

**Estimate:** ~1.5h

---

### Wave 3 — Wire selector.rs + Integration Tests
**Files:** `src/heuristics/mod.rs`, `src/pilot.rs`, `tests/`
**Gaps:** #11, #16
**Depends on:** Waves 1+2 merged

```
1. Wire selector.rs into heuristics
   - Import semantic selector in heuristics/mod.rs
   - Add try_selector() as Tier 2.5 (after form/login, before LLM)
   - Parse CSS-like selectors from goal: "click the .submit-btn"

2. WebKit integration tests (#[ignore] — need macOS + bridge)
   - test_webkit_extract_example_com
   - test_webkit_login_heuristics
   - test_webkit_session_isolation
   - test_webkit_navigate_and_extract
   - test_webkit_screenshot

3. Cross-engine parity test
   - Same URL through both engines
   - Assert SemanticView structure matches (element kinds, count range)
```

**Estimate:** ~1.5h

---

### Wave 4 — CI + Release
**Files:** `.github/workflows/ci.yml`, `Cargo.toml`
**Gaps:** #14
**Depends on:** Waves 1-3 merged

```
1. CI: Add macOS Swift build job
   - runs-on: macos-14 (arm64)
   - swift build -c release
   - Upload lad-webkit-bridge as artifact
   - Include in GitHub Release alongside Rust binaries

2. Version bump
   - Cargo.toml: 0.4.1 → 0.5.0
   - npm/package.json: sync
   - python/pyproject.toml: sync

3. Git push + tag
   - Push 6 existing + new commits
   - git tag v0.5.0
   - Push tag → triggers full CI pipeline

4. Verify pipeline
   - CI green on all jobs
   - crates.io publishes 0.5.0
   - npm publishes 0.5.0
   - pypi publishes 0.5.0
   - GitHub Release has: lad + llm-as-dom-mcp (linux, macos) + lad-webkit-bridge (macos)
```

**Estimate:** ~1h

---

### Wave 5 — Observer + lad_watch (P2, post-release)
**Files:** NEW `src/observer.rs`, `src/mcp_server.rs`
**Gaps:** #12, #13
**Depends on:** v0.5.0 released

```
1. observer.rs — Semantic DOM diffing
   - SemanticDiff { added: Vec<Element>, removed: Vec<Element>, changed: Vec<(Element, Element)> }
   - diff(old: &SemanticView, new: &SemanticView) → SemanticDiff
   - Classify changes: "new element appeared", "text changed", "form submitted"

2. lad_watch MCP tool
   - Start polling: url + interval_ms + optional JS script
   - Push notifications via MCP notification protocol
   - Stop watching on command

3. Swift: add start_monitoring command
   - {"cmd":"start_monitoring","script":"...","interval":1000}
   - Timer-based re-evaluation, push events to stdout

4. WebKit network monitoring via URLProtocol
   - Subclass URLProtocol to intercept requests
   - Classify as Auth/API/Asset/Navigation (reuse network.rs logic)
   - enable_network_monitoring() → true
```

**Estimate:** ~4h (post-release, separate PR)

---

## Dependency Graph

```
Wave 1 (Rust hardening) ─────┐
                              ├──► Wave 3 (integration) ──► Wave 4 (release)
Wave 2 (Swift hardening) ─────┘                                    │
                                                                   ▼
                                                          Wave 5 (observer)
```

W1 ∥ W2 → W3 → W4 → W5

## Estimates

| Wave | Effort | Tokens (est) | Parallelizable |
|------|--------|-------------|----------------|
| W1 Rust hardening | 1.5h | ~80K | ∥ with W2 |
| W2 Swift hardening | 1.5h | ~60K | ∥ with W1 |
| W3 Integration | 1.5h | ~70K | after W1+W2 |
| W4 CI + Release | 1h | ~30K | after W3 |
| W5 Observer | 4h | ~150K | post-release |
| **Total to release** | **~5.5h** | **~240K** | |
| **Total incl. observer** | **~9.5h** | **~390K** | |

## Release Checklist

```
[ ] W1: Drop + crash detection + handshake + pending fix
[ ] W2: NSDate + autorelease + Set + httpOnly + isolation + version
[ ] W3: selector wired + webkit integration tests
[ ] W4: CI macOS job + version bump + push + tag + verify pipeline
[ ] Posts: publish from docs/POST_MULTI_ENGINE.md
[ ] W5: observer + lad_watch (separate release)
```

## Success Criteria

v0.5.0 is shippable when:
1. `cargo test` — 341+ tests, 0 failures
2. `lad --engine webkit --url https://github.com/login --extract-only` — works
3. `lad --engine webkit --url https://news.ycombinator.com/login --goal "login as x"` — 3 heuristic hits
4. CI pipeline green on tag push
5. crates.io + npm + pypi all publish
6. GitHub Release includes lad-webkit-bridge binary for macOS arm64
7. WebKit bridge doesn't orphan on lad crash (verified)
8. WebKit bridge crash → immediate error, not 30s timeout (verified)
