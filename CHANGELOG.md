# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.12.0](https://github.com/menot-you/llm-as-dom/releases/tag/v0.12.0) - 2026-04-14

### Bug Fixes

- *(ci)* update release-plz.toml to new commit_parsers syntax
- *(ci)* remove deprecated advisory keys from deny.toml
- *(security)* remove duplicate format_session_context (FIX-R4-02 regression)
- *(ui)* refine product hunt badge layout
- remove unused import in webkit_integration.rs
- harden WebKit adapter process lifecycle (Drop, crash detection, handshake)
- harden Swift WebKit bridge (serialization, isolation, lifecycle)
- pypi readme path + npm publish step name
- collapse clippy match warning in integration tests
- stale streak considers acted_on changes + expand error keywords
- add session_context to bench helpers (CI fix)
- add session_context to W4 test helpers (cross-wave merge fix)
- 6 DX issues from battle test (Chrome lock, labels, text, locate, stale-state)
- *(audit+mcp)* dedup audit issues, screenshot on success, form context, progress logging
- suppress clippy lints in chaos tests (CI -D warnings)
- wire #[tool_handler] to ServerHandler — MCP E2E now works
- bot-challenge detection + SPA wait strategy
- *(a11y)* element cap field in SemanticView + prompt output
- *(security)* proper JS string escaping in execute_action
- *(tests)* calibrate 3 chaos test assertions to match actual behavior
- slow.html timing 3s to 1.5s for smoke test compat
- pin all GitHub Actions to full SHA (org policy requirement)
- clippy warnings + missing fields from W9-W11 merge
- zero clippy warnings, Codex review fixes applied

### Documentation

- *(security)* add SECURITY.md
- update README + ARCHITECTURE for multi-engine, add launch posts
- battle test results — live MCP metrics for publication
- CONTRIBUTING.md + 3 issue templates (bug, feature, site compat)
- README rewrite — dev testing angle, zero-LLM heuristics explained
- wild web report + launch plan (174 scenarios, 97 failure modes, 9 bugs)
- README.md + ARCHITECTURE.md (W7)
- enterprise quality polish via @rust agent

### Features

- *(observability)* Sentry SDK across 3 binaries + hygiene unblockers
- landing page layout, assets, and cloudflare pages mapping
- wire selector engine into pilot + add WebKit integration tests
- add WebKit sidecar bridge (Swift macOS app)
- add WebKit browser engine adapter via macOS sidecar bridge
- *(v0.6)* macOS Chrome cookie decryption via Keychain
- *(v0.6)* network traffic capture and semantic classification
- *(v0.6)* semantic selector engine — find elements by description
- *(v0.5)* interactive mode with --app Chrome + captcha handling
- multi-channel distribution — npm, pypi, curl installer + auto-release pipeline
- unify LLM env vars — LAD_LLM_URL, LAD_LLM_MODEL, LAD_LLM_API_KEY
- accept goal as positional CLI argument
- *(v0.4)* hard scenario heuristics + MCP session + release v0.4.0 (Wave 4)
- *(v0.3)* security fixes + arch improvements (Wave 1+2)
- lad_locate + lad_audit MCP tools — source mapping and page quality audit
- hints reading + 5-tier dispatcher + 5 hints tests (Wave A complete)
- playbook system + hints stub + 23 new tests (Wave A part 1)
- playbook system — deterministic step replay for known workflows
- VHS terminal demo (GIF + MP4) + Claude Desktop MCP config + README hero image
- Z.AI LLM smoke test in CI + benchmark model auto-detection
- screenshot fallback on escalation + fixture-based CI smoke tests
- chaos test fixtures for adversarial scenarios
- 8 test fixtures (pet-shop kitchen-sink) + 3 from Codex review
- lib restructure, ZAI backend, CI/CD, benchmarks, integration tests
- CI/CD pipeline, integration tests, criterion benchmarks
- benchmark suite + results (W8)
- form scoping + acted-on dedup (W6)
- MCP server with 3 semantic tools (Wave 3)
- heuristics-first architecture (Wave 2)
- LLM-as-DOM POC — AI browser pilot with cheap LLM

### Performance

- default model → qwen2.5:7b (5.5x faster, benchmark winner)

### Refactoring

- introduce BrowserEngine + PageHandle traits, decouple from chromiumoxide
- rename lad-mcp binary to llm-as-dom-mcp
