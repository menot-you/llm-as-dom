---
name: Canonical Engineering Pipeline
description: The gold-standard pipeline proven in the LAD v0.8 sprint — adopt for all serious implementation work
type: feedback
---

## The Pipeline (proven in LAD v0.8 session)

### Phase 1: Ultrathink
- `/ultrathink` with the core question
- Launch 3 Explore agents in parallel for codebase understanding
- Plan agent for architecture design
- Multi-model validation: Codex (CLI) + Gemini (CLI) + Opus (@oracle) debate the approach
- **No implementation until architecture is validated by 3 models**

### Phase 2: Implementation Waves
- Decompose into waves with disjoint file scopes
- Launch parallel @rust agents (one per wave, different files)
- Each wave: implement → test → commit → next wave auto-starts
- `cargo test + clippy -D warnings` gate between waves

### Phase 3: Battle Test
- Test live with actual MCP tools against real sites
- Verify the tools work end-to-end, not just unit tests
- Document failures for the next fix cycle

### Phase 4: Deep Refactor
- Launch @simplifier (opus) for full codebase audit
- Categories: GOD FILES, DRY, DEAD CODE, ERROR PATHS, COUPLING, CONFIG, NAMING, TEST GAPS
- Every finding: file:line + severity + one-line fix

### Phase 5: Security Audit
- Launch @sec (opus) for red team analysis
- Include ST3GG steganographic injection vectors
- SSRF, prompt injection, PII leaks, credential exposure
- Score each finding (Severity × Exploitability × Blast radius)

### Phase 6: Multi-Model Review Loop ← THE KEY INNOVATION
```
fix → codex review (CLI gpt-5.4 --full-auto) 
    → gemini review (CLI gemini-3.1-pro-preview)
    → fix what they find
    → repeat until BOTH say LGTM
```
- Add Opus @sec as third angle for different perspectives (race conditions, memory, supply chain)
- Track convergence: findings should DECREASE each round (18→14→13→8→?)
- Each round adds tests (394→437→467→483)

### Phase 7: Test Meta-Audit
- Launch @tester to audit tests for FALSE NEGATIVES
- Tests that pass but don't verify what they claim
- Weak assertions, tautological tests, missing edge cases
- Tests asserting bug behavior as "expected"

### Phase 8: Ship
- Version bump
- Update docs (README, ARCHITECTURE, CONTRIBUTING)
- `cargo publish`
- Launch content (posts, social)

## Rules

1. **No implementation without multi-model architecture validation** (Phase 1)
2. **Parallel waves on disjoint files only** — never two agents on the same file
3. **Review loop doesn't stop until external models say LGTM** — not self-assessed
4. **Every round must add tests** — test count grows monotonically
5. **Fix chaos tests that assert bug behavior** — don't preserve known-bad
6. **Security is not optional** — @sec runs every sprint, ST3GG vectors included
7. **Track convergence** — if findings INCREASE, something is wrong
8. **Codex + Gemini via CLI, not MCP** — they read the actual codebase

**Why:** This pipeline caught 53 findings across 4 rounds that self-review missed entirely. External models find different classes of bugs (SSRF redirect bypass, IPv6 mapped addresses, TOCTOU races, DNS rebinding). The convergence pattern (18→14→13→8) proves diminishing returns — each round is cheaper but finds deeper issues.

**How to apply:** Trigger on any serious implementation task. Skip Phase 1 ultrathink for small fixes. Always run Phase 6 review loop for anything touching security or public API.
