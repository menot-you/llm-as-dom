# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.15.0](https://github.com/menot-you/llm-as-dom/compare/v0.14.0...v0.15.0) - 2026-04-23

### Documentation
- *(readme)* Add "Why AI agents pick LAD" section ([#33](https://github.com/menot-you/llm-as-dom/pull/33))
- *(readme)* Visceral narrative rewrite — "AI cosplay" hook ([#32](https://github.com/menot-you/llm-as-dom/pull/32))


### Features
- *(playbook)* Opt-in playbook learning from successful runs ([#35](https://github.com/menot-you/llm-as-dom/pull/35))


## [0.14.0](https://github.com/menot-you/llm-as-dom/compare/v0.13.1...v0.14.0) - 2026-04-23

### Bug Fixes
- *(type)* Tolerate stale CDP context after press_enter nav (BUG-1) ([#47](https://github.com/menot-you/llm-as-dom/pull/47))
- *(a11y)* Dedupe long visible_text in chrome sections (FR-3) ([#46](https://github.com/menot-you/llm-as-dom/pull/46))
- *(a11y)* Article/repo HINT + extended label fallback (FR-4, FR-5) ([#44](https://github.com/menot-you/llm-as-dom/pull/44))
- *(wait,extract)* Text contains + limit/truncated (BUG-3, FR-2) ([#43](https://github.com/menot-you/llm-as-dom/pull/43))
- *(audit)* Opt-in tab promotion + close ephemeral target (BUG-2) ([#42](https://github.com/menot-you/llm-as-dom/pull/42))
- *(extract)* Honor `what` as semantic filter on content-heavy pages ([#36](https://github.com/menot-you/llm-as-dom/pull/36)) ([#41](https://github.com/menot-you/llm-as-dom/pull/41))


### Features
- *(audit)* Broaden a11y + forms + links rule set (FR-6) ([#45](https://github.com/menot-you/llm-as-dom/pull/45))


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
- *(a11y)* Dedupe long repeated sentences in `visible_text`. The JS walker
  previously re-emitted the same sticky `<header>` / `<footer>` / `<aside>` /
  `<nav>` text via both the heading walk AND the span/td fallback, which
  produced outputs like "Hacker Newsnew | past | comments | ask ... Hacker
  Newsnew | past | comments | ask ...". Now long sentences (≥ 5 words)
  inside chrome containers are emitted once per page. Short strings
  ("Page 1 of 10") and content sections (`<main>`, `<article>`) pass
  unchanged so feed entries (e.g. 5 GitHub repo rows all showing "No
  description, website, or topics provided.") and legitimate pagination
  duplicates stay visible to the agent (FR-3 from
  `docs/friction-log-2026-04-22.md`).
- *(type)* Tolerate stale CDP context after `press_enter=true`-triggered
  navigation. `lad_type` with `press_enter=true` used to return a
  `"Cannot find context with specified id"` / `"Execution context was
  destroyed"` error while the navigation it just kicked off actually
  completed successfully — agents saw an error but the page had moved.
  The tool now confirms navigation via URL diff + `wait_for_navigation`
  and silently swallows the stale-context error in that case, returning
  the post-nav view as expected. Set env `LAD_PRESS_ENTER_STRICT=1` at
  process startup for the pre-fix raw-error behavior (rollback escape
  hatch). Non-nav CDP errors (timeout, protocol error) still bubble
  unchanged. New optional `detailed: bool` param prepends a single
  `[outcome: navigated|no_navigation, from: ..., to: ...]` line to the
  output when `press_enter=true` — default-off so existing string
  parsers see byte-identical responses (BUG-1 from
  `docs/friction-log-2026-04-22.md`).

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
- *(audit)* Broaden audit rule set. On real pages the old rules fired
  only once or twice (HN login page surfaced only `A11Y-5 missing
  lang`). Four new rules — one per category — pull in common
  production misses:
  - **A11Y-6** (warning) — missing `<h1>` OR heading hierarchy skip
    (e.g. `<h3>` without a prior `<h2>`); screen-reader outline relies
    on continuous levels.
  - **FORMS-5** (info) — password-bearing `<form>` with no hidden
    anti-forgery marker (`csrf`, `authenticity`, `xsrf`, `nonce`).
    Severity `info` because SameSite=Strict cookies are a legitimate
    alternative — we don't want the audit to scream at correct setups.
  - **FORMS-6** (warning) — sign-in/sign-up form with inputs missing
    the `autocomplete` attribute, which password managers need to
    auto-fill credentials.
  - **LINKS-4** (warning) — `target="_blank"` with `rel="noopener"`
    but missing `rel="noreferrer"`. noopener blocks `window.opener`
    but does NOT suppress the `Referer` header; shipping only one
    leaves a referrer leak (FR-6 from
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

