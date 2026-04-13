---
name: Quality Tier System (Canonical)
description: SSS-to-F quality rating system adopted as canonical nomenclature for all nott projects
type: feedback
---

## Tier Definitions

- **SSS** — World reference. Nobody does it better. Paper-worthy.
- **SS** — Enterprise gold. Passes Fortune 500 audit with zero findings.
- **S** — Production-hardened. Multi-model reviewed, battle tested.
- **A** — Solid. Few findings, all low/minor.
- **B** — Functional. Works but has known tech debt.
- **C** — MVP. Happy path works, breaks on edge cases.
- **D** — Prototype. Stubs, TODOs, untested paths.
- **E** — Broken. Compiles but has critical bugs.
- **F** — Non-functional. Doesn't compile or run.

## How to Apply
Rate each dimension separately, then compute overall as weighted average:
- Security (weight 3x)
- Architecture (weight 2x)
- Test Coverage (weight 2x)
- DX/UX (weight 2x)
- Resilience (weight 1x)
- Performance (weight 1x)
- Documentation (weight 1x)
- DRY/SOLID (weight 1x)

**Why:** Adopted during LAD v0.8 sprint. Use for all project assessments. The tier system maps to review loop exit criteria: S = review loop LGTM from external models.
