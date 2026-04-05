# lad v0.5.0 Launch Posts

## Twitter/X Thread

**Tweet 1 (Hook)**
Your AI agent wastes 80% of tokens reading HTML.

We built lad — a browser pilot that compresses your DOM to ~300 tokens. Login tests that cost 15K tokens now cost 200.

Open-source, Rust, multi-engine. Thread:

**Tweet 2 (How)**
How it works:

Traditional: Claude → Playwright → 15KB HTML → parse → click → repeat (×4)

lad: Claude → lad_browse("test login") → { success: true }

5 decision tiers. Most actions never hit the LLM — heuristics resolve in 310ns.

**Tweet 3 (Multi-engine)**
v0.5 is browser-agnostic.

Chromium + WebKit (native macOS WKWebView). Same SemanticView, same pilot, different renderers.

Safari handles flexbox, <dialog>, scroll differently. Testing only Chrome misses ~20% of bugs.

**Tweet 4 (Numbers)**
54 adversarial fixtures: shadow DOM, timing attacks, i18n, opacity tricks, token bombs.

The DOM is hostile. lad handles it.

**Tweet 5 (CTA)**
Try it:
cargo install llm-as-dom
lad --url "http://localhost:3000/login" --goal "login as test@example.com"

AGPL-3.0 | Rust + Swift
github.com/menot-you/llm-as-dom

---

## LinkedIn Post

**Your AI agent shouldn't be a frontend developer.**

Every time Claude or GPT navigates a web page through Playwright, it's parsing 15KB of raw HTML. That's 15,000 tokens of noise — divs, classes, SVGs — just to find a login form.

We built **lad** (LLM-as-DOM) to fix this:

- Compresses any page to ~300 tokens using semantic extraction
- 5-tier decision engine: most actions resolve via heuristics in 310ns, no LLM needed
- Browser-agnostic: Chromium and WebKit (native macOS) — same pilot, different engines
- 54 adversarial test fixtures covering shadow DOM, timing attacks, RTL, ARIA contradictions

The result: **60x cheaper** browser automation for AI agents.

v0.5.0 ships multi-engine support. WebKit means zero Chrome install on macOS and real cross-browser testing (Safari renders 20% of the web differently).

Architecture: The entire browser API surface is 9 methods behind a `PageHandle` trait. Everything else — extraction, heuristics, pilot — operates on a compressed SemanticView. Adding a new engine (Firefox, Electron) is ~300 lines.

Open-source under AGPL-3.0. Built with Rust + Swift.

github.com/menot-you/llm-as-dom

#AI #WebAutomation #Rust #OpenSource #DevTools

---

## Hacker News — Show HN

**Title:** Show HN: lad – Browser pilot for AI agents. Compresses DOM to 300 tokens, 60x cheaper.

**Body:**

Hi HN,

We built lad because every AI browser agent we tried (including our own) was hemorrhaging tokens on HTML parsing. A simple login test with Playwright + Claude costs ~15,000 tokens across 4 roundtrips. Most of that is the LLM staring at divs and class names.

lad compresses your page to a SemanticView (~100-300 tokens) and navigates using a 5-tier decision engine:

- Tier 0: Playbook replay (trained flows)
- Tier 1: Developer hints (data-lad attributes)
- Tier 2: Heuristics — 310ns, handles 90% of login/search/form fills
- Tier 3: Cheap LLM (Ollama) for ambiguous cases
- Tier 4: Escalate to orchestrator with screenshot

Most dev testing never hits the LLM.

v0.5 adds multi-engine support. The pilot is browser-agnostic — a PageHandle trait (9 methods) is the entire API surface. We ship Chromium + WebKit (native macOS WKWebView via a Swift sidecar bridge).

Why WebKit matters: Safari handles flexbox, `<dialog>`, scroll, and clipboard differently. Testing only Chrome misses real-world bugs. Plus: zero Chrome install on macOS.

We test against 54 adversarial fixtures (shadow DOM, timing attacks, opacity tricks, CSS illusions, token bombs, RTL, ARIA contradictions) to harden the extractor.

Tech: Rust (11K LOC) + Swift (450 LOC). AGPL-3.0.

Install: `cargo install llm-as-dom`

Repo: https://github.com/menot-you/llm-as-dom

Happy to answer questions about the architecture, heuristic design, or adversarial testing approach.

---

## Blog Post (Long-form)

# How We Made AI Browser Testing 60x Cheaper

## The Token Problem

Every AI agent that browses the web has the same dirty secret: it's burning tokens on HTML.

Consider a simple login test. The agent opens a page, gets 15KB of DOM, sends it to Claude, gets back "click the email field", clicks, gets another 15KB, and repeats. Four roundtrips × 15K tokens = 60,000 tokens for a login. At Claude Sonnet rates, that's roughly $0.02 per login test. Run 1000 tests/day and you're at $600/month — on login tests alone.

The waste is obvious: 90% of those tokens are noise. The agent doesn't need to know about your CSS classes, your SVG icons, or your `<div class="flex items-center gap-2 px-4 py-2">` wrappers. It needs to know there's an email field, a password field, and a submit button.

## The Compression

lad (LLM-as-DOM) compresses your page into a SemanticView. A typical login page goes from 15,000 tokens to ~150:

```
SemanticView: Pet Shop Login (3 elements, ~42 tokens)
  [0] input(email) "Email address" placeholder="you@example.com"
  [1] input(password) "Password"
  [2] button "Sign In"
  [3] link "Forgot password?" → /forgot
  [4] link "Create account" → /register
```

That's it. No HTML, no CSS, no SVG. Just the interactive elements with their semantic labels.

## The Decision Engine

Here's the key insight: **most web actions don't need an LLM at all.**

When you say "login as test@example.com with password secret123", lad doesn't ask Claude what to do. It:

1. Parses the goal: detects "login" intent, extracts credentials
2. Matches fields: email input → email credential, password input → password credential
3. Types values, finds submit button, clicks
4. Detects success (URL change, success message, dashboard redirect)

All in ~310 nanoseconds. No API call, no tokens, no latency.

We call this the 5-tier decision engine:

| Tier | Strategy | When |
|------|----------|------|
| 0 | Playbook | Trained flows |
| 1 | Hints | Developer annotations |
| 2 | Heuristics | Login, search, forms — 90% of actions |
| 3 | Cheap LLM | Ambiguous pages (Ollama, free) |
| 4 | Escalate | Screenshot to orchestrator |

In practice, Tier 2 handles the vast majority of developer testing scenarios. Tier 3 catches the edge cases. Tier 4 is the safety net.

## Multi-Engine: Why We Added WebKit

v0.5 makes lad browser-agnostic. The pilot, heuristics, and LLM reasoning operate on SemanticView — they have no idea which browser is running underneath.

We ship two adapters:
- **Chromium** (default) — uses Chrome DevTools Protocol
- **WebKit** — native macOS WKWebView via a Swift sidecar bridge

Why bother? Three reasons:

1. **Real rendering differences.** Safari handles flexbox, `<dialog>`, scroll snapping, and clipboard API differently from Chrome. If you only test in Chromium, you're missing ~20% of the web.

2. **Zero install on macOS.** WebKit comes with the OS. No 500MB Chrome download. Great for CI on Apple Silicon.

3. **System integration.** WKWebView respects macOS proxy/VPN settings automatically. No special flags needed.

The entire browser API surface is 9 methods behind a `PageHandle` trait. Adding a new engine (Firefox via Marionette, Electron via IPC) means writing a ~300 line bridge.

## Adversarial Testing

The DOM is hostile. We maintain 54 adversarial fixtures designed to break lad:

- **Extraction attacks:** `opacity: 0` but `pointer-events: auto`, `clip-path` that hides elements, shadow DOM (open + closed), CSS `::before` visual buttons
- **Timing attacks:** content that loads after 2.1 seconds (past the 2s wait window), `requestAnimationFrame` rendering, `MutationObserver` insertions
- **Action attacks:** duplicate `data-lad-id` collisions, elements that move on hover, `alert()` that blocks execution
- **Classification attacks:** password fields labeled as search, 404 pages titled "Login", contradictory ARIA attributes
- **LLM confusion:** 4 identical "Submit" buttons with different actions, Thai/Arabic text, Cyrillic lookalike characters

Each fixture tests one specific failure mode. If lad passes all 54, it handles the real web.

## Get Started

```bash
cargo install llm-as-dom

# See what lad "sees"
lad --url "http://localhost:3000/login" --extract-only

# Run a login test
lad --url "http://localhost:3000/login" \
    --goal "login as test@example.com with password secret123"
```

Open-source under AGPL-3.0. Rust + Swift. 11K LOC.

https://github.com/menot-you/llm-as-dom
