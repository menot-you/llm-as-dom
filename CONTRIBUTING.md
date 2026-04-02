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
cargo test               # all tests
```

## Adding a Heuristic

1. Create `src/heuristics/your_strategy.rs`
2. Add `try_your_strategy()` returning `Option<HeuristicResult>`
3. Wire into `src/heuristics/mod.rs` → `try_resolve()`
4. Add tests in the same file
5. Keep each file under 300 LOC

## Adding a Test Fixture

1. Create `fixtures/your_page.html` (self-contained, no external deps)
2. Add assertion in `fixtures/smoke_test.sh`
3. Run: `./fixtures/smoke_test.sh ./target/release/lad`

## Adding an Adversarial Fixture

1. Create `fixtures/adversarial/NN_name.html`
2. Target a specific failure mode (see `docs/WILD_WEB_REPORT.md`)
3. Add a test in `tests/chaos.rs`

## Code Style

- Doc comments on every pub item
- Error handling: `Result<T, Error>`, no `unwrap()` in non-test code
- JS in `a11y.rs`: keep extraction script readable
- Heuristic confidence: 0.0-1.0, threshold is 0.6

## Pull Requests

- One feature per PR
- Tests required for new functionality
- CI must pass (all 5 jobs)
