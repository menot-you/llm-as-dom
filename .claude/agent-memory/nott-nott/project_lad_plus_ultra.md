---
name: LAD Plus-Ultra Plan
description: Roadmap to make LAD definitively replace Playwright — security hardened, enterprise gold, feature complete
type: project
---

## Status (2026-04-08)

### Completed This Session
1. W0: pilot.rs split (1,147 → 5 modules), shadow DOM, iframe traversal
2. W1: 4 escape hatch tools (eval, close, press_key, back)
3. W2.5: Gold lad_watch (observer revival, ring buffer, MCP resource push)
4. W2: 3 smart tools (wait, screenshot, network) + peer notify wire
5. W3: 3 interaction tools (hover, dialog, upload) — 21 tool parity
6. Docs: README, ARCHITECTURE, CONTRIBUTING updated
7. Battle test: 6/6 live tests passed
8. Multi-model architecture review: Gemini + Codex + Opus validated watch design

### In Progress
- R1: mcp_server.rs split (2,140 LOC god file → modules)
- Codex GPT-5.4 enterprise review (running)
- Security hardening (12 findings from @sec, 4 CRITICAL)

### Pending (Plus-Ultra Roadmap)
- Security Phase 1: sanitize.rs, URL allowlist, random prompt boundaries, lad_eval audit
- Security Phase 2: aria-label divergence, credential masking, password scrubbing
- Refactor R2: PilotResult constructor, backend factory, handle_blocked_page DRY
- Refactor R3: include_str! JS, observer tests, goal parsing consolidation
- Gemini suggestions: Actor model for browser state, MutationObserver, CDP preload
- Chrome lock file: ephemeral profiles + stale lock reaper + Drop trait SIGKILL
- QA loop: fix → codex review → gemini review → repeat until zero complaints
- Final: /plan plus-ultra for LAD to box Playwright forever

### Architecture Decision (Multi-Model Consensus)
- A2A stays untouched (task execution only)
- LAD watch uses MCP native push (Peer::notify_resource_updated)
- Claude orchestrator routes watch events to A2A agents
- No new protocols, no new crates

**Why:** validated by Gemini 3.1, Codex, and Opus. All agreed A2A SSE and tokio::broadcast were wrong paths.

**How to apply:** When resuming, check R1/Codex/security status, then continue the QA loop.
