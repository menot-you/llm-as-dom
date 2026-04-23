# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.14.0](https://github.com/menot-you/llm-as-dom/compare/v0.13.1...v0.14.0) - 2026-04-23

### Bug Fixes
- *(a11y)* Article/repo HINT + extended label fallback (FR-4, FR-5) ([#44](https://github.com/menot-you/llm-as-dom/pull/44))
- *(wait,extract)* Text contains + limit/truncated (BUG-3, FR-2) ([#43](https://github.com/menot-you/llm-as-dom/pull/43))
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
- *(wait)* Honor documented `text contains X` / `page contains X` predicates in
  `lad_wait` and `lad_assert`. These were listed as examples in the tool
  description but fell through to the whole-phrase fallback, which required
  literal words "text" and "contains" to appear on the page. Now they match
  as a union over URL, `<title>`, visible body text, and rendered prompt
  (BUG-3 from `docs/friction-log-2026-04-22.md`).

  **Behavior change**: `page contains X` (and `text contains X`) now also
  match against the URL, where previously they could only match literal
  prose containing the words "page"/"text" and "contains". Anyone who
  relied on the old broken phrase as a never-matches sentinel will start
  getting hits when `X` appears in the URL.

### Features
- *(extract)* Add `limit: Option<u32>` to `lad_extract` with hard cap at 200.
  Applied AFTER strict filtering but BEFORE pagination so `top 5` is honored
  across pages. Response now includes `truncated: bool`, `limit_applied`, and
  `total_before_limit` so iterating callers can detect silent caps. When
  `strict=true` and `limit` is unset (`None` or `0`), a leading numeral in
  `what` (e.g. "top 5 story titles", "primeiras 3 histórias", "best 10
  matches") is parsed as an implicit limit — `top|first|primeir[oa]s?|best|
  melhores` are recognized (en + pt-br only; es/fr extension is a deliberate
  scope decision, not an oversight). `limit=0` is treated as "unset" rather
  than "explicit empty" — falls through to the NL parse / no-limit branch
  to avoid silent empty results. Non-matching phrasing returns the full
  filtered list (FR-2 from `docs/friction-log-2026-04-22.md`).
- *(a11y)* HINT classifier no longer labels `<article>`/repo content as
  `navigation/listing page` just because the DOM carries > 10 links. New
  `article/repo page` hint fires on (a) DOM signal — `<article>`,
  `<main role=main>`, Schema.org `itemtype`, `og:type` meta — OR (b) URL
  pattern `/owner/repo(/issues|pulls|wiki|tree|blob|commits|releases|
  tags|discussions|actions)?` on allow-listed hosts (github.com,
  gitlab.com, bitbucket.org, codeberg.org, sr.ht). Login, search, and
  form detection still win over both branches so auth-gate detection
  does not regress. HN paginator (`news.ycombinator.com/news/2`) and
  other generic sites outside the allowlist keep their existing
  `navigation/listing page` classification (FR-4 from
  `docs/friction-log-2026-04-22.md`).
- *(a11y)* Extended label fallback chain for interactive elements so
  icon-only buttons stop surfacing as `Button type=button ""`. New
  fallback order: `aria-label → <label> → placeholder → textContent →
  title → testid → SVG <title> descendant → aria-describedby resolved
  text → data-label / data-name → <unlabeled:${role}>` sentinel. The
  explicit sentinel replaces silent empty strings on buttons and
  inputs so the agent can tell a genuinely unlabeled control from a
  parse failure (FR-5 from `docs/friction-log-2026-04-22.md`).

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

