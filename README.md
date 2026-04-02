<div align="center">

# lad

### Your AI agent's browser

**Test your app 60x cheaper. lad compresses your DOM so Claude never parses HTML.**

[![CI](https://github.com/example-org/llm-as-dom/actions/workflows/ci.yml/badge.svg)](https://github.com/example-org/llm-as-dom/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

[Quick Start](#quick-start) · [How It Works](#how-it-works) · [MCP Server](#mcp-server) · [Benchmarks](#benchmarks)

![lad demo](assets/demo.gif)

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
cargo install llm-as-dom

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

### Three Decision Tiers

| Tier | Speed | Cost | When |
|------|-------|------|------|
| **Heuristics** | **310ns** | **Free** | Login, search, form fill — 90% of dev testing |
| **Cheap LLM** | 0.4s | Free (Ollama) | Ambiguous elements, unknown pages |
| **Escalate** | — | — | Screenshot sent to orchestrator |

Most dev testing **never hits the LLM**. Heuristics parse your goal, match form fields by name/type/label, find submit buttons, and detect success — all in nanoseconds.

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

### Staging E2E
```bash
lad --url "https://staging.myapp.com/login" \
    --goal "login as qa@test.com with password staging123" \
    --backend zai --model glm-4.7  # cloud LLM for complex pages
```

## MCP Server

`lad-mcp` turns your browser into a tool that Claude can call directly.

```bash
lad-mcp  # starts MCP server (stdio)
```

| Tool | What it does |
|------|-------------|
| `lad_browse` | Navigate + accomplish a goal autonomously |
| `lad_extract` | Extract structured page info (never raw HTML) |
| `lad_assert` | Verify assertions ("has login form", "title contains Dashboard") |

<details>
<summary>Claude Desktop config</summary>

```json
{
  "mcpServers": {
    "lad": {
      "command": "lad-mcp",
      "env": {
        "LAD_OLLAMA_URL": "http://localhost:11434",
        "LAD_MODEL": "qwen2.5:7b"
      }
    }
  }
}
```
</details>

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

## Test Suite

- **101 tests** (unit + chaos + integration)
- **66 HTML fixtures** (12 standard + 54 adversarial)
- **8 micro-benchmarks** (criterion)
- **5 CI jobs** — all green

## Requirements

- Chrome/Chromium (system install)
- Ollama with `qwen2.5:7b` — optional, only for LLM fallback

```bash
cargo install llm-as-dom  # installs both lad and lad-mcp
```

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full technical deep-dive.

## License

MIT — use it however you want.
