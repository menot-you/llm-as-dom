# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.14.0](https://github.com/menot-you/llm-as-dom/compare/v0.13.1...v0.14.0) - 2026-04-22

### Bug Fixes
- *(audit)* Opt-in tab promotion + close ephemeral target (BUG-2) ([#42](https://github.com/menot-you/llm-as-dom/pull/42))
- *(extract)* Honor `what` as semantic filter on content-heavy pages ([#36](https://github.com/menot-you/llm-as-dom/pull/36)) ([#41](https://github.com/menot-you/llm-as-dom/pull/41))


### Bug Fixes
- *(audit)* Prevent ephemeral Chrome target leak and make active-tab lifecycle
  explicit. `lad_audit` now always returns `audit_ephemeral: bool` and
  `audit_tab: null | {tab_id, url}`. Default behavior (`return_tab=false`) closes
  the audit page via a new `PageHandle::close()` trait method so the CDP target
  is released; the previously active tab (e.g. a logged-in session) is
  preserved. Passing `return_tab=true` promotes the audit page into the tab
  pool and exposes its `tab_id` for follow-up tools (BUG-2 from
  `docs/friction-log-2026-04-22.md`).

## [0.13.1](https://github.com/menot-you/llm-as-dom/compare/v0.13.0...v0.13.1) - 2026-04-17

### Bug Fixes
- *(release)* rewrite publish-ecosystems Python build for actual layout (#27)
- *(ci)* bump cosign-installer to v4.1.1 (fixes key validation)
- *(ci)* temporarily disable cosign signing (sigstore infra broken)

Note: v0.13.0 shipped to crates.io but the GitHub Release was permanently
locked as immutable at create time; binaries never attached. v0.13.1
re-runs the pipeline end-to-end against a non-immutable release to
validate the dedupe refactor (#20) + Python publish fix (#27) land
correctly across crates.io, npm, PyPI, and GitHub binaries.

## [0.13.0](https://github.com/menot-you/llm-as-dom/compare/v0.12.0...v0.13.0) - 2026-04-17

### Bug Fixes
- *(backend)* Plumb --llm-url to openai/anthropic constructors
- *(chromium)* Add --single-process + --disable-dev-shm-usage
- *(chromium)* Pass --no-zygote when sandbox is disabled
- *(python)* Copy README.md to each python package to fix OutsideDestinationError
- *(deny)* Allow Unicode-3.0, MPL-2.0, CDLA-Permissive-2.0 licenses
- *(npm)* Restore org scope and remove manual action
- *(npm)* Remove org scope to match unscoped published package name
- *(ci)* Dead code gate, audit build tooling, pin ecosystems publish


### Features
- Add LLM-as-DOM demonstration scripts and Python wrapper for MCP server binary distribution

