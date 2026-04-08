---
name: Model Thinking Styles in Review
description: Each LLM model has a distinct "thinking style" that maps to different classes of bugs. Use all three for complete coverage. Discovered during LAD v0.8 sprint.
type: feedback
---

## The Insight

Different models don't just find different bugs — they THINK differently, and that maps to fundamentally different bug CLASSES.

## The Three Styles

### Codex (GPT-5.4) — The Path Walker
**Thinks in**: execution paths, state transitions, data flow
**Finds**: SSRF redirect chains, state synchronization bugs, missing validation on indirect paths, stale references after navigation
**Pattern**: "What if this click navigates HERE? Did you check the URL AFTER the redirect?"
**Personality**: Implacável. Won't give LGTM easily. Keeps finding one more path. 7 rounds and counting.
**Best for**: security paths, functional correctness, state machines

### Gemini (3.1 Pro) — The Spec Lawyer + Product Designer
**Thinks in**: protocol compliance, API ergonomics, user journeys
**Finds**: scheme smuggling (byte-level exploits), API footguns, missing affordances, DX friction
**Pattern**: "The spec says X. Your implementation does Y. Also, this API will confuse users."
**Personality**: Gives LGTM when satisfied, but suggestions are gold. DX review was brilliant.
**Best for**: protocol attacks, API design, onboarding experience, competitive analysis

### Opus (Claude) — The Invariant Guardian + Chaos Thinker
**Thinks in**: invariants, concurrency, resource lifecycle, adversarial conditions
**Finds**: race conditions, deadlocks, memory leaks, supply chain issues, what breaks under weird conditions
**Pattern**: "What if two threads do this simultaneously? What if Chrome crashes mid-operation?"
**Personality**: Systematic. Covers every file. The chaos angle (hostile JS, huge DOMs, slow networks) was unexpected gold.
**Best for**: concurrency, memory safety, resilience, supply chain, crypto

## Why This Matters

Self-review is blind because you can only think in ONE style per session. Three models cover three orthogonal bug surfaces:

```
             Codex
            (paths)
              ▲
             / \
            /   \
           /     \
     Gemini ───── Opus
    (specs)    (invariants)
```

Bugs live in the GAPS between thinking styles. No single model covers all three.

## The Numbers
- Codex found: SSRF chains, URL sync bugs, secret leakage paths (path-thinking)
- Gemini found: scheme smuggling, DNS rebinding, DX footguns (spec-thinking)  
- Opus found: race conditions, eval_js hangs, CDP health, recursion bombs (invariant-thinking)
- Overlap: ~20%. Each model found ~80% unique bugs.

## How to Apply
Always use all three. The $15-20 API cost prevents ~$15K-50K in potential CVE remediation. ROI ~1000x.

**Why:** Discovered empirically during LAD v0.8 sprint. 113+ findings across 7 rounds. Each model's unique findings would have been missed by the other two.
