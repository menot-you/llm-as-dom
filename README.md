# lad

**LLM-as-DOM** — AI browser pilot. Cheap LLM navigates the web so expensive models don't have to.

```
Traditional:  Claude → Playwright → 15KB HTML → Claude parses DOM → click → 15KB HTML → ...
lad:          Claude → lad_browse("login as user") → { success: true, steps: 4 }
```

## What it does

`lad` opens a headless browser, compresses the page to ~100-300 tokens, and uses heuristics + a cheap local LLM to accomplish goals autonomously. The orchestrator (Claude, GPT) never sees raw DOM.

**Token savings: 10-30x.** A login flow costs ~300 tokens instead of ~15,000.

## Quick start

```bash
# Extract a page (no LLM needed)
lad --url "https://github.com/login" --extract-only

# Pilot a login flow (needs Ollama with qwen2.5:7b)
lad --url "https://news.ycombinator.com/login" \
    --goal "login as testuser with password test123"

# Show the browser window for debugging
lad --url "https://example.com" --goal "click About" --visible
```

## MCP server

```bash
# Start as MCP server (stdio, for Claude Desktop / VS Code)
lad-mcp
```

Add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "lad": {
      "command": "/path/to/lad-mcp",
      "env": {
        "LAD_OLLAMA_URL": "http://localhost:11434",
        "LAD_MODEL": "qwen2.5:7b"
      }
    }
  }
}
```

### MCP tools

| Tool | Description |
|------|-------------|
| `lad_browse` | Navigate + accomplish a goal autonomously (login, fill form, search) |
| `lad_extract` | Extract structured page info (elements, text, page type) |
| `lad_assert` | Verify assertions about a page ("has login form", "title contains X") |

## How it works

```
┌─────────────────────────────────────────────┐
│  Orchestrator (Claude/GPT via MCP)           │
│  Sends: lad_browse(url, goal)                │
│  Gets:  { success, steps, duration }         │
│  Token cost: ~300 per goal                   │
└──────────────┬──────────────────────────────┘
               │
┌──────────────▼──────────────────────────────┐
│  Pilot (Rust, heuristics-first)              │
│                                              │
│  Tier 1: Rule engine         [2-3ms, free]   │
│  ├── Credential parsing from goal text       │
│  ├── Form field matching (name/type/label)   │
│  ├── Submit button detection                 │
│  └── Goal completion detection               │
│                                              │
│  Tier 2: Cheap LLM fallback  [2-8s, ~$0.001]│
│  └── Ollama (Qwen3 8B) for ambiguity        │
│                                              │
│  Tier 3: Escalate to orchestrator            │
│  └── When pilot can't resolve after retries  │
└──────────────┬──────────────────────────────┘
               │ CDP (Chrome DevTools Protocol)
┌──────────────▼──────────────────────────────┐
│  chromiumoxide (headless Chrome)             │
│  DOM extraction via JS + ghost-ID stamping   │
└─────────────────────────────────────────────┘
```

## SemanticView

Instead of 15KB HTML, lad compresses to ~100-300 tokens:

```
URL: https://github.com/login
TITLE: Sign in to GitHub
HINT: login page
STATE: Ready

ELEMENTS:
[1] Input type=text "Username or email address" name="login"
[2] Input type=password "Password" name="password"
[4] Button type=submit "" name="commit" val="Sign in"
[5] Button type=submit "Continue with Google"
[7] Link "Create an account" href="/signup"
```

## Performance

| Metric | lad (heuristics) | lad (LLM) | Playwright MCP |
|--------|-------------------|-----------|----------------|
| HN login | **1.6s** (3 steps) | 8.1s (4 steps) | ~20s (4 roundtrips) |
| Tokens per step | **0** | ~500 | ~3,000-5,000 |
| Decision latency | **2-3ms** | 2-8s | N/A (human token cost) |

## Requirements

- Rust 1.86+
- Chrome/Chromium (system install)
- Ollama with `qwen2.5:7b` (only for LLM fallback)

```bash
# Build
cargo build --release

# Run tests
cargo test
```

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for technical deep-dive.

## License

MIT
