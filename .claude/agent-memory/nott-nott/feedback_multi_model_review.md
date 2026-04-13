---
name: Multi-Model Review Pattern
description: The multi-model adversarial review loop pattern — proven to find 53+ bugs that self-review misses. Candidate for SSOT product feature and public documentation.
type: feedback
---

## Pattern: Multi-Model Adversarial Review Loop

**Why:** Self-review is blind by definition. The same model that wrote the code has the same blind spots to review it. Different models trained on different data find different CLASSES of bugs.

**Proven results (LAD v0.8 sprint):**
- 4 rounds, 53 production findings + 22 test false negatives
- Convergence: 18 → 14 → 13 → 8 (diminishing returns prove it works)
- Test growth: 394 → 437 → 467 → 483+ (monotonically increasing)

## What Each Model Finds

```
Codex (GPT-5.4)  → execution paths, redirect chains, functional gaps, state bugs
Gemini (3.1 Pro) → protocol attacks, scheme smuggling, byte-level exploits, spec compliance
Opus (@sec)      → race conditions, memory safety, crypto, supply chain, resource leaks
@tester          → false negatives in tests, silent-pass patterns, bug-masking tests
```

No single model finds all four classes. The intersection is where bugs hide.

## How to apply

Always use CLI (not MCP proxy) — models must read the actual codebase:
- `codex exec -m gpt-5.4 --full-auto "$(cat context.md)"`
- `gemini -m gemini-3.1-pro-preview -p "$(cat context.md)"`

**Why:** validated in production. Self-review would have missed 53 bugs. The convergence curve proves diminishing returns — each round is cheaper but finds deeper issues.

**How to apply:** Run after every significant implementation batch. Minimum 2 models. Add @sec for security-sensitive code. Add @tester for meta-audit after test count stabilizes.
