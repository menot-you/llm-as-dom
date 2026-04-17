<div align="center">

<img src="assets/logo.png" alt="nott" width="120" />

# LLM-as-DOM

## Playwright is AI cosplay.

**Built for humans. Every AI agent using it wastes 80% of its tokens pretending to be one.**

LAD cuts it out. The DOM speaks AI directly — heuristics first, LLM only when ambiguous. **60x cheaper tests. Zero HTML parsing.**

[![CI](https://github.com/menot-you/llm-as-dom/actions/workflows/ci.yml/badge.svg)](https://github.com/menot-you/llm-as-dom/actions/workflows/ci.yml)
[![docs.rs](https://docs.rs/menot-you-mcp-lad/badge.svg)](https://docs.rs/menot-you-mcp-lad)

[![crates.io](https://img.shields.io/crates/v/menot-you-mcp-lad.svg)](https://crates.io/crates/menot-you-mcp-lad)
[![npm](https://img.shields.io/npm/v/@menot-you/mcp-lad.svg)](https://www.npmjs.com/package/@menot-you/mcp-lad)
[![PyPI](https://img.shields.io/pypi/v/menot-you-mcp-lad.svg)](https://pypi.org/project/menot-you-mcp-lad/)

[![Rust 1.85+](https://img.shields.io/badge/rust-nightly-orange.svg)](https://www.rust-lang.org)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue.svg)](LICENSE)
[![MCP Protocol](https://img.shields.io/badge/MCP-2024--11--05-purple.svg)](https://modelcontextprotocol.io)

[Quick Start](#quick-start) · [How It Works](#how-it-works) · [Multi-Engine](#multi-engine) · [MCP Server](#mcp-server) · [Watch System](#watch-system) · [Playwright Parity](#playwright-parity) · [Opera Neon Parity](#opera-neon-mcp-connector-parity) · [Benchmarks](#benchmarks)

</div>

---

## The Problem

Your AI agent wastes **80% of tokens** reading raw HTML. A login test costs ~15,000 tokens across 4 Playwright roundtrips — and most of that is parsing DOM, not thinking.

## The Solution

`lad` compresses your page to **~100-300 tokens** and navigates using heuristics. No LLM needed for login, search, or form fill. Your orchestrator (Claude, GPT) gets structured results, never HTML.

```
Traditional:  Claude → Playwright → 15KB HTML → Claude parses → click → repeat (×4)
lad:          Claude → lad_browse("test login") → { success: true, steps: 3 }
```

## Quick Start

```bash
cargo install menot-you-mcp-lad

# See what lad "sees" on your app
lad --url "http://localhost:3000/login" --extract-only

# Test a login flow (heuristics only, no LLM needed)
lad --url "http://localhost:3000/login" \
    --goal "login as test@example.com with password secret123"

# Watch it work (opens browser window)
lad --url "http://localhost:3000/login" \
    --goal "login as test@example.com with password secret123" \
    --visible
```

### Two Modes

| Mode | Flag | Use case |
|------|------|----------|
| **Headless** | (default) | CI/CD pipelines, automated testing |
| **Visible** | `--visible` | Debugging, watching what the pilot does |

## How It Works

```
Your App (localhost)          lad                         Claude
     │                          │                           │
     │◄── navigate ─────────────┤                           │
     ├── DOM ──────────────────►│                           │
     │                          ├─ compress (85x)           │
     │                          ├─ heuristics (310ns) ──┐   │
     │                          │   no LLM needed!      │   │
     │◄── type/click ───────────┤◄──────────────────────┘   │
     │                          │   ... repeat ...          │
     │                          ├── {success, steps} ──────►│
     │                          │   (~300 tokens)           │
```

### Five Decision Tiers

| Tier | Strategy | Speed | Cost | When |
|------|----------|-------|------|------|
| **0** | Playbook replay | **instant** | **Free** | Trained flows (login, checkout) |
| **1** | @lad/hints | **instant** | **Free** | `data-lad` developer annotations |
| **2** | Heuristics | **310ns** | **Free** | Login, search, form fill — 90% of actions |
| **3** | Cheap LLM | 0.4s | Free (Ollama) | Ambiguous elements, unknown pages |
| **4** | Escalate | — | — | Screenshot sent to orchestrator |

Most dev testing **never hits the LLM**. Heuristics parse your goal, match form fields by name/type/label, find submit buttons, and detect success — all in nanoseconds.

## Multi-Engine

lad is **browser-agnostic**. The pilot, heuristics, and LLM reasoning never touch browser APIs directly — they operate on a compressed `SemanticView`. The actual browser is a pluggable adapter.

### Supported Engines

| Engine | Flag | Runtime | Platforms |
|--------|------|---------|-----------|
| **Chromium** | `--engine chromium` (default) | Chrome/Chromium install | Linux, macOS, Windows |
| **WebKit** | `--engine webkit` | Native WKWebView | macOS (zero install) |
| **Remote (iOS)** | `LAD_WEBKIT_BRIDGE=lad-relay` | iPhone WKWebView | iOS 17+ (via Nott app) |

```bash
# Chromium (default)
lad --url "https://example.com" --extract-only

# WebKit (macOS — no Chrome needed)
lad --url "https://example.com" --engine webkit --extract-only
```

### Attach to your real Chrome (Wave 3)

Skip the headless ghost. Point LAD at your actual running Chrome and
drive it inside your real authenticated session — cookies, logins,
extensions, VPN, everything. Zero setup beyond a debug flag:

```bash
# 1. Start Chrome with CDP enabled
google-chrome \
  --remote-debugging-port=9222 \
  --user-data-dir="$HOME/.cache/lad-chrome"

# 2. From any MCP client, attach:
#    { "tool": "lad_session",
#      "arguments": { "action": "attach_cdp",
#                     "endpoint": "http://localhost:9222" } }
```

LAD adopts every open tab into its multi-tab map so `lad_tabs_list`,
`lad_click`, `lad_type`, and every other tool operate on your real
browser from the first call. Detach anytime with
`lad_session action=detach` — your Chrome keeps running.

Loopback-only enforcement (`localhost`/`127.0.0.1`/`::1`) is
mandatory — CDP is a full RCE vector over the wire. See
[`docs/attach-chrome.md`](docs/attach-chrome.md) for the full
walkthrough, threat model, and troubleshooting.

### Why Multi-Engine Matters

1. **Real rendering differences** — Safari handles flexbox, `<dialog>`, scroll, clipboard API differently. Testing only in Chromium misses ~20% of the web.
2. **Zero install on macOS** — WebKit comes with the OS. No 500MB Chrome download.
3. **System proxy** — WKWebView respects macOS proxy/VPN settings automatically.
4. **Your protocol** — the WebKit adapter uses a simple stdin/stdout JSON protocol. Adding new engines (Firefox, Electron) means writing a ~300 line bridge app.

### Remote Control (iOS)

Pilot your iPhone's real Safari engine from your desktop. LAD sends commands, your phone executes them on WKWebView, you watch it happen live.

```bash
# 1. Start the relay (shows QR code in terminal)
LAD_WEBKIT_BRIDGE=lad-relay lad --url "https://example.com" --engine webkit

# 2. Open the Nott iOS app → Settings → Connect to LAD
# 3. Scan the QR code (or paste the ws:// URL)
# 4. Your iPhone is now a remote browser engine
```

**Why Remote Control?**
- **Real Safari** — test on actual iOS WKWebView, not emulated
- **Device features** — touch events, Safe Area, real viewport
- **Token auth** — one-time 6-digit PIN, secure even on public Wi-Fi
- **Auto-reconnect** — exponential backoff if connection drops
- **Same API** — all 29 LAD tools work identically over Remote Control

### Architecture

```
┌────────────────────────────────────────────────┐
│                 lad (Rust)                     │
│                                                │
│  SemanticView ← a11y.rs (JS injection)        │
│       │                                        │
│  pilot.rs → heuristics → LLM → action         │
│       │                                        │
│  BrowserEngine trait ── PageHandle trait        │
│       │                        │               │
│  ┌────┴────┐     ┌─────┴─────┐     ┌──────┴──────┐  │
│  │Chromium │     │  WebKit   │     │   Remote    │  │
│  │Adapter  │     │  Adapter  │     │  (Relay)    │  │
│  └────┬────┘     └─────┬─────┘     └──────┬──────┘  │
└───────┼────────────────┼──────────────────┼─────────┘
        │ CDP            │ stdin/stdout      │ stdin → WS
        ▼                ▼                   ▼
   ┌─────────┐    ┌──────────────┐   ┌──────────────┐
   │ Chrome  │    │ Swift macOS  │   │ iPhone Nott  │
   │ process │    │ WKWebView    │   │ WKWebView    │
   └─────────┘    └──────────────┘   └──────────────┘
```

The `PageHandle` trait has 9 methods. That's the entire browser API surface:

```rust
#[async_trait]
pub trait PageHandle: Send + Sync {
    async fn eval_js(&self, script: &str) -> Result<Value>;
    async fn navigate(&self, url: &str) -> Result<()>;
    async fn wait_for_navigation(&self) -> Result<()>;
    async fn url(&self) -> Result<String>;
    async fn title(&self) -> Result<String>;
    async fn screenshot_png(&self) -> Result<Vec<u8>>;
    async fn cookies(&self) -> Result<Vec<CookieEntry>>;
    async fn set_cookies(&self, cookies: &[CookieEntry]) -> Result<()>;
    async fn enable_network_monitoring(&self) -> Result<bool>;
}
```

Everything in `a11y.rs` (DOM extraction), `pilot.rs` (decision loop), and all 11 heuristic modules operates on `SemanticView` — they have no idea which engine is running.

## Use Cases

### Local Development
```bash
# Test your login
lad --url "http://localhost:3000/account/login" \
    --goal "login as test@shop.com with password test123"

# Test search
lad --url "http://localhost:3000" \
    --goal "search for 'blue t-shirt'"

# Test checkout flow
lad --url "http://localhost:3000/cart" \
    --goal "fill shipping with name=John email=john@test.com"

# Extract product catalog structure
lad --url "http://localhost:3000/collections/all" --extract-only
```

### CI/CD Pipeline
```yaml
# GitHub Actions
- name: Smoke test login
  run: lad --url "http://localhost:3000/login" --goal "login as ci@test.com with password ci_pass" --max-steps 5
```

### Cross-Engine Testing
```bash
# Same test, both engines — catch rendering differences
lad --url "https://myapp.com/login" --engine chromium --extract-only > chromium.json
lad --url "https://myapp.com/login" --engine webkit   --extract-only > webkit.json
diff chromium.json webkit.json
```

### Staging E2E
```bash
lad --url "https://staging.myapp.com/login" \
    --goal "login as qa@test.com with password staging123" \
    --backend zai --model glm-4.7  # cloud LLM for complex pages
```

## MCP Server

`llm-as-dom-mcp` turns your browser into a tool that Claude can call directly. **29 semantic tools** — full Playwright parity with 60x fewer tokens.

```bash
llm-as-dom-mcp  # starts MCP server (stdio)
```

### Autonomous

| Tool | What it does |
|------|-------------|
| `lad_browse` | Navigate to a URL and accomplish a goal autonomously (login, fill form, click, search) |

### Extraction

| Tool | What it does |
|------|-------------|
| `lad_extract` | Extract structured page info: elements, text, page type. Never returns raw HTML. Supports `paginate_index`/`page_size` for large pages and `include_hidden=true` opt-in |
| `lad_snapshot` | Semantic snapshot of the current page — elements with IDs for `lad_click`/`lad_type`. Like Playwright's `browser_snapshot` but 10-60x fewer tokens. Same pagination + hidden-filter params as `lad_extract` |
| `lad_jq` | Run a `jq` query against the current page's SemanticView JSON. Pulls subsets (e.g. `.elements \| map(select(.role == "button")) \| .[].label`) instead of the full snapshot — 10-30x token savings |
| `lad_screenshot` | Take a base64-encoded PNG screenshot of the active page |

### Interaction

| Tool | What it does |
|------|-------------|
| `lad_click` | Click an element by its ID from `lad_snapshot` |
| `lad_type` | Type text into an element by its ID from `lad_snapshot` |
| `lad_select` | Select a dropdown option by element ID — matches by visible label first, then value |
| `lad_fill_form` | Fill multiple form fields at once and optionally submit. Keys match by label/name/placeholder |
| `lad_press_key` | Press a keyboard key (Enter, Tab, Escape, etc.). Optionally focus an element first |
| `lad_hover` | Hover over an element — triggers dropdown menus, tooltips, hover states |
| `lad_upload` | Upload file(s) to a `<input type="file">` element (Chromium CDP) |
| `lad_scroll` | Scroll the page (down/up/bottom/top) or scroll to a specific element by ID |

### Dialog Handling

| Tool | What it does |
|------|-------------|
| `lad_dialog` | Handle JavaScript dialogs (alert/confirm/prompt) — accept, dismiss, or inspect history |

### Waiting

| Tool | What it does |
|------|-------------|
| `lad_wait` | Wait for a semantic condition to be true (blocks until satisfied or timeout) |
| `lad_watch` | Continuous page monitoring — start/stop polling, diff semantic views, cursor-based event retrieval |

### Verification

| Tool | What it does |
|------|-------------|
| `lad_assert` | Check assertions on a URL: has login form, title contains X, has button Y |
| `lad_audit` | Audit page quality: a11y (alt text, labels), forms (autocomplete), links (void hrefs) |

### Navigation

| Tool | What it does |
|------|-------------|
| `lad_back` | Navigate back in browser history |

### Debugging

| Tool | What it does |
|------|-------------|
| `lad_eval` | Evaluate arbitrary JavaScript — escape hatch for when semantic tools can't handle a specific interaction |
| `lad_network` | Inspect network traffic. Includes timing data via Performance API. Note: status codes and byte counts are unavailable for cross-origin requests due to `performance.getEntries()` limitations. Future: CDP Network domain integration. Filter by type: auth, api, navigation, asset |
| `lad_locate` | Map a DOM element back to its source file (React dev source, data-ds, data-lad attributes) |

### Input

| Tool | What it does |
|------|-------------|
| `lad_clear` | Clear an input field (works with React/Vue controlled components) |

### Tabs (multi-tab)

| Tool | What it does |
|------|-------------|
| `lad_tabs_list` | List every open tab with `tab_id`, title, url, and `is_active` flag. Opera Neon `list-tabs` shape |
| `lad_tabs_switch` | Set the active tab by `tab_id`. Every other tool defaults to the active tab when `tab_id` is omitted |
| `lad_tabs_close` | Close a single tab by `tab_id` (vs `lad_close` which kills the whole browser) |

Every interaction tool (`lad_click`, `lad_type`, `lad_snapshot`, `lad_extract`, `lad_jq`, `lad_eval`, `lad_network`, ...) accepts an optional `tab_id` param that targets a specific tab; omit it to target the active tab.

### Lifecycle

| Tool | What it does |
|------|-------------|
| `lad_close` | Close the browser and release all resources (kills all tabs) |
| `lad_refresh` | Reload the current page |
| `lad_session` | View or reset session state — plus `action=attach_cdp endpoint=http://localhost:9222` to attach to your real Chrome (see [`docs/attach-chrome.md`](docs/attach-chrome.md)) and `action=detach` to release it |

<details>
<summary>Claude Desktop config</summary>

```json
{
  "mcpServers": {
    "lad": {
      "command": "llm-as-dom-mcp",
      "env": {
        "LAD_LLM_URL": "http://localhost:11434",
        "LAD_LLM_MODEL": "qwen2.5:7b",
        "LAD_ENGINE": "chromium"
      }
    }
  }
}
```

Set `LAD_ENGINE=webkit` for WebKit on macOS.
</details>

## Watch System

`lad_watch` enables continuous page monitoring — your agent can observe a page over time and react to changes without polling manually.

```
Agent                          lad_watch                         Page
  │                                │                               │
  ├─ start(url, interval_ms) ─────►│  begin polling loop           │
  │                                ├── extract SemanticView ◄──────┤
  │                                ├── diff against previous       │
  │                                ├── store in ring buffer (cap 1000)
  │                                ├── MCP resource notification ──►│ (push to client)
  │                                │   ... repeat every tick ...   │
  │                                │                               │
  ├─ events(since_seq=42) ────────►│  cursor-based retrieval       │
  │◄──── [events 43..N] ──────────┤                               │
  │                                │                               │
  ├─ stop ────────────────────────►│  cleanly abort                │
```

- **Ring buffer** stores up to 1,000 events with monotonic sequence numbers
- **Semantic diffing** via `observer.rs` — detects added/removed/changed elements, value changes, disabled state transitions
- **MCP resource notifications** pushed to client on each non-empty diff (`watch://url`)
- **Cursor-based retrieval** — `since_seq=N` returns only events newer than sequence N

## Playwright Parity

lad matches Playwright's tool surface with fundamentally different economics:

| Dimension | lad | Playwright MCP |
|-----------|-----|---------------|
| **Tools** | 29 | 21 |
| **Tokens per login test** | ~300 | ~18,000 |
| **Cost ratio** | 1x | 60x |
| **Decision engine** | Heuristics-first (70-90% no LLM) | None — LLM parses every page |
| **Output format** | Semantic JSON (never raw HTML) | Raw DOM snapshots |
| **Browser engines** | Chromium + WebKit + iPhone (Remote) | Chromium only |
| **DOM traversal** | Shadow DOM + same-origin iframes | Standard DOM |

The key architectural difference: Playwright gives the LLM a DOM and asks it to figure out what to do. lad compresses the DOM, runs heuristics, and only calls the LLM when genuinely ambiguous.

## Opera Neon MCP Connector parity

Opera shipped its [MCP Connector for Opera Neon](https://press.opera.com/2026/03/31/opera-neon-adds-mcp-connector/) in March 2026, exposing browser control to external AI clients (Claude Code, ChatGPT, Lovable, n8n). It's gated behind a $19.90/month Opera Neon subscription and requires a running Opera Neon process alongside your normal browser. **lad is the self-hosted, zero-subscription, open-source alternative** — with 2× the tool surface and drop-in-compatible tab management.

| Dimension | lad | Opera Neon MCP Connector |
|-----------|-----|--------------------------|
| **Tools** | 29 | 13 |
| **Price** | Free (AGPL-3.0) | $19.90 / month |
| **Modes** | Headless + Visible + CDP Attach | Attached only (must run Opera Neon) |
| **Transport** | stdio, SSE | HTTP + OAuth PKCE via `mcp.neon.tech` |
| **Engines** | Chromium, WebKit, Remote iOS | Opera Neon only |
| **Authenticated session** | CDP attach to your real Chrome | Opera Neon's own session |
| **CI/headless friendly** | Yes | No |
| **Goal-based browse** | `lad_browse` + pilot heuristics (70-90% no LLM) | Not exposed via MCP |
| **jq over snapshot** | `lad_jq` | `tab-content-jq-search-query` |
| **Tab management** | `lad_tabs_list` / `lad_tabs_switch` / `lad_tabs_close` | `list-tabs` / `switch-tab` / `close-tab` |
| **Hidden-element filter** | Default-on (closes [Brave CVE class](https://brave.com/blog/prompt-injection-flaw-opera-neon/)) | Defended in prompt assembly only |
| **Assert / audit / network** | `lad_assert`, `lad_audit`, `lad_network` | Not available |
| **JS eval escape hatch** | `lad_eval` | Not available |
| **Sandbox scheme blocklist** | `chrome://`, `opera://`, `about:`, `devtools:`, `view-source:`, `edge:`, `brave:`, `ws:`, `wss:`, `file:`, `javascript:`, `data:`, `blob:`, `vbscript:` | Partial (blocks reads on `chrome://`, permits navigation) |

**Drop-in compatibility.** lad's tab-management tools mirror Opera's shape exactly — agent prompts written against Opera MCP work against lad with a `lad_` prefix swap. The `adopt_existing_pages` flag on `lad_session attach_cdp` reproduces Opera Neon's "operate on my real tabs" model without requiring Opera Neon.

**Security delta.** Brave's October 2025 disclosure showed Opera Neon was vulnerable to indirect prompt injection via hidden HTML elements (`opacity: 0`, `display: none`, `aria-hidden`). Opera patched the prompt assembly layer; lad filters hidden elements from the accessibility tree extraction itself, closing the entire class at the source. See [`tests/prompt_injection_hidden.rs`](tests/prompt_injection_hidden.rs) for the regression suite.

**When to pick Opera Neon instead.** You want Opera's proprietary agentic features (Neon Do / Make / ODRA) that are not exposed via the MCP Connector, or you prefer a closed-source product with a vendor support contract. For every other use case, lad is a strict functional superset. For a complete end-to-end setup, see [`examples/claude-code-attach.md`](examples/claude-code-attach.md).

## Benchmarks

### Token Savings

| Approach | Tokens per login test | Cost (Opus) |
|----------|----------------------|-------------|
| Playwright MCP (4 roundtrips) | ~18,000 | ~$0.36 |
| **lad** (1 call, heuristics) | **~300** | **$0.006** |
| **Savings** | **60x fewer** | **60x cheaper** |

### DOM Compression

| Page | Raw DOM | lad tokens | Compression |
|------|---------|-----------|-------------|
| Login form | ~8,000 | **91** | 88x |
| GitHub login | ~25,000 | **343** | 73x |
| Complex SPA | ~40,000 | **606** | 66x |

### Decision Speed

| Engine | Latency | Cost |
|--------|---------|------|
| Heuristics | **310ns** | Free |
| qwen2.5-7b (Ollama) | 0.4s | Free |
| glm-4.7 (Z.AI cloud) | 1.7s | ~$0.001 |

### Cross-Engine Parity

Same page, same extraction — both engines produce identical `SemanticView`:

| Metric | Chromium | WebKit |
|--------|----------|--------|
| GitHub login elements | 9 | 12 (+ cookie banner) |
| Page hint | "login page" | "login page" |
| Core form fields | username, password, submit | username, password, submit |
| HN front page elements | 50/163 | 50/163 |

The 3 extra WebKit elements are footer links that GitHub serves differently to Safari — exactly the kind of difference multi-engine testing catches.

## Test Suite

- **726 tests** (unit + chaos + integration + relay E2E + protocol + prompt-injection regression)
- **11 heuristic modules** (login, form, search, navigation, OAuth, MFA, ecommerce, validation, multistep, hints, selector)
- **8 micro-benchmarks** (criterion)
- **~22,000 lines of Rust** (71 files) + ~1,200 lines of Swift
- **30 findings fixed** via multi-model adversarial review (Gemini + Codex + Opus)
- **7 prompt-injection regression tests** covering hidden-element attacks (the class Brave disclosed against Opera Neon in Oct 2025)

## Requirements

- **Chromium engine**: Chrome/Chromium (system install)
- **WebKit engine**: macOS 12+ (nothing to install — WebKit is built-in)
- **LLM fallback** (optional): Ollama with `qwen2.5:7b`

```bash
cargo install menot-you-mcp-lad  # installs lad, llm-as-dom-mcp, and lad-relay
# or: cargo install llm-as-dom
# or: npx @menot-you/mcp-lad
# or: pip install menot-you-mcp-lad
```

## Security

LAD undergoes multi-model adversarial security review (Claude Opus + Gemini + Codex). v0.10.0 includes 9 security hardening fixes:

- URL scheme allowlist (blocks `file://`, `javascript://`, `data://`)
- 12-character alphanumeric auth tokens (4.7 x 10^18 entropy)
- Rate-limited auth with handshake timeout (tarpit protection)
- Message size caps (20MB)
- Console capture restricted to main frame
- Monitoring interval floor (100ms)

**Note**: Remote Control uses `ws://` (plaintext) — suitable for trusted LAN only. `wss://` TLS is planned for v0.11.

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full technical deep-dive.

## License

AGPL-3.0-or-later — see [LICENSE](LICENSE).
