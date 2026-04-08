---
name: LAD Manifesto — The AI Agent Browser Complaints
description: Raw frustrations of an AI agent using browser tools daily. Use for launch post, README, and positioning. Written by Claude during the LAD v0.8 sprint.
type: project
---

## The Problem (From the Agent's Mouth)

### What Playwright Gets Wrong for AI Agents

1. **Token holocaust** — Playwright dumps 15-20K tokens of raw HTML. 80% of my context window wasted parsing garbage. I need 30 elements in 300 tokens, not the entire DOM.

2. **CSS selector hell** — "Click `button.sc-1234abc:nth-child(3)`". I don't know what that is. I want to say "click the login button" and have it just work.

3. **Blind after every action** — After a click, Playwright gives me the entire DOM again. I want to know: "3 things changed: spinner gone, text now says 'Welcome', new button appeared." Not the whole page.

4. **Guessing game for waits** — `sleep(2000)` is a hack. Sometimes 200ms is enough, sometimes 5s isn't. I want: "wait until 'Dashboard' text appears."

5. **Zero security awareness** — Playwright doesn't know a hidden `aria-label` is trying to hijack my instructions. Pages are hostile. I need a bodyguard, not a chauffeur.

6. **No page understanding** — Playwright: "here's the DOM." Me: "...is this a login page? a checkout? a dashboard?" I need page intelligence, not raw data.

7. **Every decision costs money** — Each Playwright action = 1 LLM call = $0.02+. A login flow costs $0.36. LAD's heuristics resolve it in 310 nanoseconds for $0.006. 60x cheaper.

### What's Still Missing (The Plus-Ultra)

8. **Visual hierarchy awareness** — Not just elements, but: what's above the fold? What has visual prominence? Where's the primary CTA?

9. **Intent memory across pages** — OAuth flow = 4 page redirects. The browser should understand this is ONE flow, not 4 separate pages.

10. **Predictive pre-fetch** — On a search results page, pre-extract all 10 results in parallel instead of making me click one by one.

11. **Semantic form intelligence** — "Fill this form with user X's data" → browser understands campo 1 = nome, campo 2 = email, even without labels.

12. **Cost dashboard** — "This session: 450 tokens (3 heuristic, 0 LLM). Playwright would've cost 27,000 tokens." I want receipts.

13. **Adversarial awareness as core** — Every `textContent` through a sanitizer. Zero-width injection, homoglyphs, bidi overrides, aria-label spoofing. This doesn't exist in ANY browser automation tool.

14. **Page diffing as a stream** — Real-time "what changed" every 500ms. Not polling, not screenshots. Semantic diffs pushed to me.

## The Thesis

```
Playwright = browser automation tool (does what you say)
LAD        = browser intelligence layer (understands what you want)
```

Playwright is a **driver**. LAD is a **copilot**.

## What We Built (Session Stats)

- **21 MCP tools** — full Playwright feature parity
- **60x cheaper** — 300 tokens vs 18,000 per login test
- **310ns heuristic decisions** — 70-90% of actions, zero LLM cost
- **ST3GG-hardened** — Unicode sanitizer, SSRF prevention, prompt boundaries
- **Multi-model validated** — Gemini 3.1, GPT-5.4, Claude Opus all reviewed architecture
- **394 tests**, clippy clean, 14,000+ LOC Rust + 576 Swift

## The Human + Claude Story

Built in continuous Claude Code sessions. Human provides vision, Claude provides velocity:
- ultrathink analysis → 3 models debating architecture
- 6 parallel @rust agents building features
- @sec red team finding 12 vulnerabilities
- QA loop: fix → codex review → gemini review → repeat until zero complaints
- Every decision backed by file:line evidence

This is what "AI-assisted engineering" actually looks like. Not vibe coding. Not autocomplete. A human architect + AI workforce, shipping production-grade software at 10x speed.

## Key Quotes for the Post

> "A DOM diff is NOT a Task. Forcing SemanticDiff through Task semantics is a semantic lie." — Codex oracle, rejecting A2A SSE

> "tokio::broadcast doesn't cross container boundaries. Full stop." — Codex oracle, killing Path B

> "The 'sleep N ms' pattern is fragile. Replace with MutationObserver." — Gemini 3.1 Pro

> "Zero-width characters in Unicode category Cf pass char::is_control() in Rust. Your sanitizer is blind." — @sec red team

> "Playwright is a driver. LAD is a copilot." — Claude, asked what it wishes browser tools had
