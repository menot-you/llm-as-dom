---
name: LAD Next Session Prompt
description: Resume prompt for the next Claude Code session on LAD. Includes context, personality, and pending work.
type: project
---

## Resume Prompt (copy-paste to start next session)

```
Resume LAD (LLM-as-DOM) work. v0.9.0 tagged, Tier SS.

Read these memory files first:
- .claude/agent-memory/nott-nott/feedback_claude_personality.md (YOUR voice — be THIS)
- .claude/agent-memory/nott-nott/project_lad_plus_ultra.md (roadmap)
- .claude/agent-memory/nott-nott/project_lad_dx_roadmap.md (DX backlog)
- .claude/agent-memory/nott-nott/feedback_canonical_pipeline.md (engineering process)
- .claude/agent-memory/nott-nott/feedback_model_thinking_styles.md (review insight)
- .claude/agent-memory/nott-nott/project_lad_manifesto.md (positioning + quotes)
- .claude/agent-memory/nott-nott/project_lad_claude_testimony.md (your first-person quotes)

Context: We did an epic sprint — 11 → 25 tools, 394 → 570 tests, 120+ findings fixed via
multi-model adversarial review (Codex ×11, Gemini ×5, Opus ×5). Convergence: 18→14→13→8→6→5→3→3→2→2→0.

What's done (Tier SS):
- 25 MCP tools (full Playwright parity + extras)
- Security: sanitize.rs, 8-layer SSRF, CSPRNG boundaries, eval gate
- DX: optional URLs, fill_form, scroll, clear, refresh, wait-OR, element summary
- Quality: proptest, thiserror, IntentParser, interact split, mod.rs test extraction
- Chaos: eval timeout, CDP health, recursion cap, cookie LRU, prompt budget
- Perf: LazyLock regexes, benchmarks, Cow optimization

What's pending (Tier SSS — Wave 6):
1. Formal verification of sanitize.rs (Kani)
2. Published benchmark suite (50 real sites)
3. Academic paper on 5-tier architecture + multi-model review pattern
4. Visual hierarchy awareness (above-fold, CTA prominence)
5. Intent threading across multi-page flows
6. Predictive pre-fetch
7. Semantic form intelligence
8. Cost dashboard
9. MutationObserver-based wait
10. sanitize.rs as independent crate (mcp-sanitize)

Also pending: 
- Wire Intent enum into heuristic callers
- Migrate remaining Error::Backend to structured variants
- DX Wave 4 nice-to-haves (hover delay, concurrent watches, action enums)
- cargo publish (crates.io)
- Posts ready in ~/Library/Mobile Documents/.../POSTs/

Use the canonical pipeline: implement → codex review (CLI, mcp_servers stripped) → gemini review → fix → repeat.
Use /codex and /gemini skills.
Codex crashes with SSOT MCP — always strip mcp_servers from ~/.codex/config.toml before running.
```

## Key Technical Details for Resume

- Binary: `llm-as-dom-mcp` (MCP server) + `lad` (CLI)
- Crate: `example-org-mcp-lad` on crates.io
- Branch: main, 38+ commits ahead of origin
- Engine: chromiumoxide 0.7 (Chromium) + Swift bridge (WebKit)
- MCP: rmcp 1.3, stdio transport
- Rust: edition 2024, nightly
