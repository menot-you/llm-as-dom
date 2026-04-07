# Architecture

## Design Principles

1. **Orchestrator never sees DOM.** The expensive LLM receives structured JSON, not HTML.
2. **Heuristics first.** Rules resolve 70-90% of actions in nanoseconds. LLM is fallback.
3. **Engine-agnostic.** The `BrowserEngine` / `PageHandle` traits abstract the browser. Chromium and WebKit ship today. Adding Firefox or Electron means writing a ~300 line bridge.
4. **LLM-agnostic.** The `PilotBackend` trait abstracts the cheap LLM. Swap Ollama for any provider.
5. **Form-scoped.** When multiple forms exist on a page, heuristics target only the relevant one.
6. **Observable.** Every step logs source (playbook/hints/heuristic/LLM), confidence, duration, and action.

## Module Map

```
src/
├── main.rs              CLI binary (lad --engine chromium|webkit)
├── mcp_server.rs        MCP binary (llm-as-dom-mcp), 18 semantic tools
├── lib.rs               Library root
│
├── engine/              Browser engine abstraction
│   ├── mod.rs           BrowserEngine + PageHandle traits, EngineConfig
│   ├── chromium.rs      Chromium adapter (wraps chromiumoxide/CDP)
│   ├── webkit.rs        WebKit adapter (stdin/stdout NDJSON to Swift sidecar)
│   └── webkit_proto.rs  Wire protocol types for WebKit bridge
│
├── a11y.rs              DOM extraction + ghost-ID stamping via JS injection
│                        (deepQueryAll for shadow DOM, same-origin iframe traversal)
├── semantic.rs          SemanticView data model + prompt serialization
├── session.rs           Cookie/navigation/auth state across pages
├── network.rs           Network traffic capture + classification
├── playbook.rs          Tier 0: trained playbook replay
│
├── pilot/               5-tier observe → decide → act loop (split from pilot.rs)
│   ├── mod.rs           Types, traits, re-exports (DecisionSource, PilotBackend, etc.)
│   ├── runner.rs        Main pilot loop: observe → decide → act → repeat
│   ├── decide.rs        5-tier dispatch: playbook → hints → heuristics → LLM → escalate
│   ├── action.rs        Action enum + execution (click, type, select, navigate, etc.)
│   ├── captcha.rs       Challenge detection + handling (Cloudflare, CAPTCHA, WAF)
│   └── util.rs          Helpers (JS escaping, etc.)
│
├── watch.rs             Persistent page monitoring with semantic diffing
├── observer.rs          SemanticView differ (added/removed/changed elements)
│
├── heuristics/          Tier 2: 11 rule-based modules
│   ├── mod.rs           Router: try_resolve() dispatches to all modules
│   ├── login.rs         Credential parsing + login form detection
│   ├── form.rs          Generic form fill by field name/type/label
│   ├── search.rs        Search bar detection + query entry
│   ├── navigation.rs    Link matching + page navigation
│   ├── hints.rs         Tier 1: @lad/hints (data-lad attributes)
│   ├── oauth.rs         OAuth provider detection + flow handling
│   ├── mfa.rs           MFA/2FA detection + TOTP support
│   ├── ecommerce.rs     Cart, checkout, product interaction
│   ├── validation.rs    Form validation error detection
│   ├── multistep.rs     Multi-step wizard detection
│   └── selector.rs      Semantic selector engine for heuristic matching
│
├── backend/             LLM backends (5 adapters)
│   ├── mod.rs           PilotBackend trait + backend registry
│   ├── generic.rs       Generic/Ollama integration (local models)
│   ├── anthropic.rs     Anthropic (Claude) API
│   ├── openai.rs        OpenAI-compatible API
│   ├── zai.rs           Z.AI (GLM) API
│   └── playbook.rs      Playbook backend helpers
│
├── audit.rs             Page quality auditing (a11y, forms, links)
├── locate.rs            Source-map element location (React, data-ds)
├── selector.rs          Semantic selector engine
├── oauth.rs             OAuth flow state machine
├── profile.rs           Chrome profile cookie import
├── crypto.rs            Chrome Safe Storage decryption (macOS)
└── error.rs             Unified error types

webkit-bridge/           Swift macOS sidecar app
├── Package.swift
└── Sources/
    └── main.swift       WKWebView + stdin/stdout NDJSON bridge (~576 LOC)
```

## Engine Abstraction

The critical insight: **80% of lad's browser interaction is JavaScript evaluation.** DOM extraction, ghost-ID stamping, element clicking, form filling, scrolling — all JavaScript. The remaining 20% is navigation, screenshots, and cookie management.

This means the `PageHandle` trait has only 9 methods:

```rust
pub trait PageHandle: Send + Sync {
    async fn eval_js(&self, script: &str) -> Result<Value>;       // ~80% of all calls
    async fn navigate(&self, url: &str) -> Result<()>;
    async fn wait_for_navigation(&self) -> Result<()>;
    async fn url(&self) -> Result<String>;
    async fn title(&self) -> Result<String>;
    async fn screenshot_png(&self) -> Result<Vec<u8>>;
    async fn cookies(&self) -> Result<Vec<CookieEntry>>;
    async fn set_cookies(&self, cookies: &[CookieEntry]) -> Result<()>;
    async fn enable_network_monitoring(&self) -> Result<bool>;    // optional
}
```

### Chromium Adapter

Wraps `chromiumoxide` (CDP over WebSocket). The adapter translates trait methods to CDP calls. All `chromiumoxide` imports are confined to `engine/chromium.rs` — no other file in the crate touches CDP types.

### WebKit Adapter

Communicates with a Swift macOS app (`lad-webkit-bridge`) that embeds `WKWebView`. The protocol is newline-delimited JSON (NDJSON) over stdin/stdout:

```
Rust (lad)                          Swift (lad-webkit-bridge)
    │                                       │
    ├─ {"cmd":"navigate","url":"..."}  ────►│  WKWebView.load()
    │◄─ {"ok":true}  ──────────────────────┤
    │◄─ {"event":"load","url":"..."}  ─────┤  WKNavigationDelegate
    │                                       │
    ├─ {"cmd":"eval_js","script":"..."}───►│  evaluateJavaScript()
    │◄─ {"ok":true,"value":{...}}  ────────┤
    │                                       │
    │◄─ {"event":"console","level":"error"} │  WKScriptMessageHandler
```

Key properties:
- **No CDP** — uses Apple's stable public API (`WKWebView`)
- **No patches** — unlike Playwright which patches WebKit source
- **Process isolation** — bridge crash doesn't take down lad
- **Protocol is yours** — simple enough to implement in any language

## Data Flow

```
URL + Goal
    │
    ▼
┌─────────────────────┐
│  BrowserEngine      │  Spawns browser or sidecar process
│  .new_page(url)     │  Returns Box<dyn PageHandle>
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│  a11y.rs            │  JS injection via page.eval_js()
│                     │  querySelectorAll(interactive elements)
│                     │  stamps data-lad-id on each
│                     │  returns JsExtraction { elements, visibleText, formCount }
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│  semantic.rs        │  JsExtraction → SemanticView
│                     │  ~100-300 tokens (vs 15KB raw DOM)
│                     │  page_hint: "login page" / "form page" / etc.
└──────────┬──────────┘
           │
           ▼
┌──────────────────────┐
│  pilot.rs            │  Loop: observe → decide → act
│                      │
│  ┌───────────────────┤
│  │ Tier 0: playbook  │  Trained flows (0.99 confidence)
│  ├───────────────────┤
│  │ Tier 1: hints     │  @lad/hints dev annotations (0.98)
│  ├───────────────────┤
│  │ Tier 2: heuristic │  11 rule modules (0.7-0.95)
│  │  login, form,     │  - parse credentials from goal
│  │  search, nav,     │  - match fields by name/type/label
│  │  oauth, mfa,      │  - detect submit button
│  │  ecommerce, ...   │  - detect success/failure
│  ├───────────────────┤
│  │ Tier 3: LLM       │  Cheap model fallback (0.4-0.5)
│  ├───────────────────┤
│  │ Tier 4: escalate  │  Screenshot to orchestrator
│  └───────────────────┘
└──────────┬───────────┘
           │
           ▼
  PilotResult { success, steps, playbook/hints/heuristic/llm_hits, duration }
```

## Ghost-ID System

Each observation stamps `data-lad-id="N"` on interactive elements via JS.
Actions reference elements by this ID: `document.querySelector('[data-lad-id="2"]').click()`.

IDs are re-stamped on every observation cycle, ensuring they match the current DOM state.
The `acted_on` vector in the pilot tracks which IDs have been acted on to prevent
duplicate actions (clicking the same button twice).

## Form Scoping

Pages with multiple `<form>` elements are handled by:

1. JS extractor assigns a `form_index` to each element
2. `target_form()` heuristic picks the form most relevant to the goal
3. All field-fill and button-click heuristics filter by `in_target_form()`

## Session Management

Multi-page flows (OAuth redirects, wizard forms) maintain state across navigations:

- **Cookies** — extracted after each action, accumulated in `SessionState`
- **Navigation history** — URL, title, actions taken, timestamps
- **Auth state machine** — None → InProgress → Authenticated/Failed
- **Page memory** — key-value store for cross-page context

## Challenge Detection

Bot challenges (Cloudflare, CAPTCHA, WAF) are detected and classified:

| Kind | Behavior |
|------|----------|
| Cloudflare Turnstile | Auto-wait 5s (may self-resolve) |
| CAPTCHA (hCaptcha, reCAPTCHA) | Interactive mode: pause for human |
| WAF block | Escalate immediately |
| Auth wall | Continue pilot (heuristics handle login) |

## MCP Protocol

The MCP server (`llm-as-dom-mcp`) uses `rmcp 1.3` with stdio transport. Exposes **18 tools** across 8 categories.

```
Client (Claude)                    llm-as-dom-mcp
    │                                 │
    ├─ initialize ───────────────────►│
    │◄──── capabilities (18 tools) ───┤
    │                                 │
    │  ── Autonomous ──────────────── │
    ├─ lad_browse { url, goal } ─────►│── pilot loop (heuristics → LLM)
    │◄──── { success, steps, ... } ───┤
    │                                 │
    │  ── Extraction ──────────────── │
    ├─ lad_extract { url, what } ────►│── SemanticView (never raw HTML)
    ├─ lad_snapshot ─────────────────►│── elements with IDs for click/type
    ├─ lad_screenshot ───────────────►│── base64 PNG
    │                                 │
    │  ── Interaction ─────────────── │
    ├─ lad_click { id } ────────────►│── click element by ghost-ID
    ├─ lad_type { id, text } ───────►│── type into element
    ├─ lad_select { id, value } ────►│── dropdown selection
    ├─ lad_press_key { key, id? } ──►│── keyboard input
    │                                 │
    │  ── Waiting ─────────────────── │
    ├─ lad_wait { condition } ──────►│── block until condition met
    ├─ lad_watch { action } ────────►│── start/events/stop monitoring
    │                                 │   (push via MCP resource notifications)
    │                                 │
    │  ── Verification ────────────── │
    ├─ lad_assert { url, asserts[] }►│── check semantic assertions
    ├─ lad_audit { url, categories }►│── a11y/forms/links audit
    │                                 │
    │  ── Navigation ──────────────── │
    ├─ lad_back ─────────────────────►│── browser history back
    │                                 │
    │  ── Debugging ───────────────── │
    ├─ lad_eval { script } ─────────►│── raw JS escape hatch
    ├─ lad_network { filter? } ─────►│── network traffic inspection
    ├─ lad_locate { url, selector } ►│── source-map lookup
    │                                 │
    │  ── Lifecycle ───────────────── │
    ├─ lad_close ────────────────────►│── release all resources
    ├─ lad_session { action } ──────►│── get/clear session state
```

## Watch System

The watch system provides persistent page monitoring with semantic diffing.

### Components

```
lad_watch(start)
    │
    ▼
┌──────────────────────┐
│  WatchState           │  Spawns a tokio task
│  ├── url              │
│  ├── interval_ms      │
│  └── JoinHandle       │
└──────────┬───────────┘
           │  polling loop
           ▼
┌──────────────────────┐      ┌──────────────────────┐
│  a11y.rs + semantic  │─────►│  observer::diff()     │
│  (extract view)      │      │  (old vs new view)    │
└──────────────────────┘      └──────────┬───────────┘
                                         │ non-empty diff
                                         ▼
                              ┌──────────────────────┐
                              │  EventBuffer          │
                              │  (ring buffer, 1000)  │
                              │  monotonic seq nums   │
                              └──────────┬───────────┘
                                         │
                                         ▼
                              ┌──────────────────────┐
                              │  Peer::notify_        │
                              │  resource_updated     │
                              │  (watch://url)        │
                              └──────────────────────┘
```

### Lifecycle

1. **Start** — `lad_watch(action="start", url="...", interval_ms=2000)` spawns a background task
2. **Poll** — every tick, extract `SemanticView` from the page
3. **Diff** — `observer::diff(old, new)` computes added/removed/changed elements
4. **Store** — non-empty diffs become `WatchEvent`s in the ring buffer (FIFO, cap 1,000)
5. **Notify** — MCP resource notification pushed via `Peer::notify_resource_updated` for `watch://url`
6. **Query** — `lad_watch(action="events", since_seq=N)` returns events newer than sequence N
7. **Stop** — `lad_watch(action="stop")` cancels the background task and cleans up

### Observer

`observer.rs` diffs two `SemanticView`s using `lad-id` as the primary key:

- **Added** — elements in new view not present in old
- **Removed** — elements in old view not present in new
- **Changed** — same `lad-id` but different value, disabled state, label, or attributes
- **Notifications** — human-readable descriptions of changes ("Text changed in 'Search'", "Element 'Submit' disabled")

## Pilot Module Split

The original `pilot.rs` was split into 5 sub-modules for maintainability:

| Module | Responsibility | LOC |
|--------|---------------|-----|
| `pilot/mod.rs` | Types (`DecisionSource`, `PilotBackend` trait), re-exports | ~100 |
| `pilot/runner.rs` | Main loop: observe → decide → act → check termination | ~296 |
| `pilot/decide.rs` | 5-tier dispatch: playbook → hints → heuristics → LLM → escalate | ~200 |
| `pilot/action.rs` | `Action` enum + `execute_action()` with retry logic | ~200 |
| `pilot/captcha.rs` | Challenge detection (Cloudflare, CAPTCHA, WAF) + resolution | ~258 |
| `pilot/util.rs` | Helpers: JS escaping, etc. | ~50 |

Public API surface is unchanged — `pilot::run_pilot()`, `pilot::Action`, `pilot::DecisionSource`.

## Shadow DOM + iframe Support

`a11y.rs` handles complex DOM structures that trip up naive extractors:

### Shadow DOM

`deepQueryAll(root, selector)` recursively traverses shadow roots:

```javascript
function deepQueryAll(root, sel) {
    const results = [...root.querySelectorAll(sel)];
    for (const el of root.querySelectorAll('*')) {
        if (el.shadowRoot) {
            results.push(...deepQueryAll(el.shadowRoot, sel));
        }
    }
    return results;
}
```

All element queries (interactive elements, forms, text nodes) use `deepQueryAll` instead of `querySelectorAll`, ensuring Web Components with shadow DOM are fully visible to heuristics.

### Same-origin iframe Traversal

Same-origin iframes are traversed via `contentDocument`:

1. Enumerate all `<iframe>` elements on the page
2. Attempt `iframes[i].contentDocument` access (catches cross-origin `SecurityError`)
3. Run `deepQueryAll` inside the iframe document
4. Elements from iframes get a `frame_index` field (`Option<u32>`) identifying which iframe they belong to
5. Cross-origin iframes are silently skipped

This means lad can interact with embedded forms, login widgets, and payment iframes as long as they share the origin.

## Extending

### Add a new browser engine

1. Create `src/engine/your_engine.rs`
2. Implement `BrowserEngine` (3 methods) and `PageHandle` (9 methods)
3. Register in `engine/mod.rs`
4. Add CLI flag in `main.rs`

The WebKit adapter is the reference implementation — 428 lines of Rust + 576 lines of Swift.

### Add a new LLM backend

Implement `PilotBackend`:

```rust
#[async_trait]
pub trait PilotBackend: Send + Sync {
    async fn decide(
        &self,
        view: &SemanticView,
        goal: &str,
        history: &[Step],
    ) -> Result<Action, Error>;
}
```

### Add a new heuristic

Add a `try_*` function in `heuristics/` that returns `Option<HeuristicResult>`,
then wire it into `try_resolve()` with a confidence threshold.
