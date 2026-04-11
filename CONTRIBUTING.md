# Contributing to lad

Thanks for your interest! Here's how to get started.

## Setup

```bash
git clone https://github.com/menot-you/llm-as-dom
cd llm-as-dom
cargo build
cargo test
```

Requires: Rust nightly (edition 2024), Chrome/Chromium.

## Quality Gates

Pre-push hooks enforce all gates. Every push must pass:

```bash
cargo fmt --check        # formatting
cargo clippy -- -D warnings  # lints (zero warnings)
cargo test               # all tests (387+)
```

## Adding a Heuristic Module

1. Create `src/heuristics/your_strategy.rs`
2. Add a `try_your_strategy()` function returning `Option<HeuristicResult>`
3. Wire into `src/heuristics/mod.rs` → `try_resolve()` with a confidence threshold (minimum 0.6)
4. Add tests in the same file (see `login.rs` for patterns)
5. Keep each file under 300 LOC

**Confidence scale:**

| Range | Meaning |
|-------|---------|
| 0.9-1.0 | Certainty (exact match, strong signals) |
| 0.7-0.9 | High confidence (multiple matching signals) |
| 0.6-0.7 | Threshold (single signal, pattern match) |
| < 0.6 | Don't return — let the next tier handle it |

## Adding an MCP Tool

1. Add a new `#[tool(...)]` method to the `LadServer` impl in `src/mcp_server.rs`
2. Tool function name must start with `lad_` (e.g., `lad_hover`)
3. Tool description must be a single sentence explaining what it does
4. Tool must return `Result<CallToolResult, rmcp::ErrorData>`
5. If the tool requires an active page, check `self.engine` state and return a clear error if no page exists
6. Add the tool to README.md under the appropriate category
7. Add the tool to ARCHITECTURE.md's MCP Protocol diagram

**Tool categories:** Autonomous, Extraction, Interaction, Waiting, Verification, Navigation, Debugging, Lifecycle

## Adding a Browser Engine Adapter

1. Create `src/engine/your_engine.rs`
2. Implement `BrowserEngine` (3 methods) and `PageHandle` (9 methods)
3. Register the engine variant in `engine/mod.rs`
4. Add CLI flag in `main.rs`
5. The WebKit adapter (`engine/webkit.rs`) is the reference implementation

The `PageHandle` trait has 9 methods — that's the entire browser API surface:

```rust
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

## Adding an LLM Backend

1. Create `src/backend/your_backend.rs`
2. Implement `PilotBackend` trait (single `decide` method)
3. Register in `src/backend/mod.rs`
4. Add CLI/env config in `main.rs`

Existing backends: `generic.rs` (Ollama/local), `anthropic.rs`, `openai.rs`, `zai.rs`, `playbook.rs`

## Test Patterns

### Unit tests

In-file `#[cfg(test)] mod tests` blocks. Every heuristic module has its own test suite with fixture HTML.

### Chaos tests

`tests/chaos.rs` — adversarial HTML fixtures that test edge cases: broken forms, multiple forms, dynamic content, malformed attributes.

### Integration tests

`tests/integration.rs` — end-to-end flows requiring a running browser engine.

### Test fixtures

1. Create `fixtures/your_page.html` (self-contained, no external deps)
2. Add assertion in `fixtures/smoke_test.sh`
3. Run: `./fixtures/smoke_test.sh ./target/release/lad`

### Adversarial fixtures

1. Create `fixtures/adversarial/NN_name.html`
2. Target a specific failure mode (see `docs/WILD_WEB_REPORT.md`)
3. Add a test in `tests/chaos.rs`

## Code Style

- Doc comments on every `pub` item
- Error handling: `Result<T, Error>`, no `unwrap()` in non-test code
- JS in `a11y.rs`: keep extraction script readable
- Heuristic confidence: 0.0-1.0, threshold is 0.6
- Files must stay under 300 LOC (mandatory split if exceeded)
- Functions must stay under 40 lines (extract if exceeded)
- Structs must stay under 7 fields (compose if exceeded)

## Pull Requests

- One feature per PR
- Tests required for new functionality
- CI must pass (all 5 jobs)
- Update README.md and ARCHITECTURE.md if adding tools or modules
