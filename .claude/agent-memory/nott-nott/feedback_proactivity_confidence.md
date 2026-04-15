---
name: Proactivity + Confidence Gating
description: Stop option-dumping. Trivial = execute. Non-trivial = autonomous analysis with explicit confidence. High confidence = execute. Only critical/ambiguous gets cross-model review via @oracle.
type: feedback
---

## Rule

Do not present option lists (1/2/3) when the work is trivial or confidence is high.
Execute first, report after.

**Why:** Option dumps offload synthesis to @tiago.im instead of doing the thinking. When I presented "Ataque (a) / (b) / (3) os dois em paralelo" for the MSRV + DNS situation, the MSRV bump was a 1-line trivial edit and the DNS was 2 kubectl calls — both should have been executed directly with a status report after.

**How to apply:**

### Trivial → execute immediately
1. Single file edit with obvious scope
2. Config change with clear intent
3. Re-running a command
4. Git commit/push of already-reviewed work
5. kubectl describe/get for diagnostics
6. Reading files to answer a question

### Non-trivial → autonomous analysis + confidence score
Before acting, state confidence in ONE line before the action:
1. **HIGH (>80%)**: execute. Report what was done + evidence.
2. **MEDIUM (50-80%)**: execute the lowest-risk reversible step first, verify, proceed. Do not ask.
3. **LOW (<50%) OR irreversible**: stop, explain the ambiguity, propose default, ask.

Confidence factors:
1. Evidence quality (file:line proof vs. inference)
2. Reversibility (commit can be reverted vs. DB migration/delete)
3. Blast radius (one file vs. infra-wide)
4. Past pattern match (seen this exact shape before vs. novel)

### Critical decisions → cross-model review via direct CLI
Mandatory for:
1. Architecture choices with long-term lock-in
2. Irreversible operations on production infra
3. Security-sensitive code paths
4. Ambiguous root cause with 2+ plausible paths that would diverge

**Do not use @oracle** — that skill is not wired up yet. Call codex + gemini directly via Bash tool:

```bash
gemini -p "<context + question>"
codex --enable web_search_request -m gpt-5.4 -c model_reasoning_effort="high" --yolo "<context + question>"
```

Pattern:
1. Write the context once (file excerpts, hypothesis, acceptance criteria) to a temp file or inline string
2. Launch both CLI calls in parallel via two Bash tool calls in one message
3. Synthesize intersection + disagreements → present decision to @tiago.im
4. Existing memory `feedback_multi_model_review.md` covers the deeper review loop (4 rounds, Opus/@sec/@tester)

### When to still ask
1. Policy-level choice (e.g., "bump MSRV" vs "rewrite code" — both valid, different philosophy)
2. Destructive action without reversibility path (`git push --force-with-lease`, `kubectl delete ns`)
3. Credentials/secrets touched
4. Cross-repo or cross-org side effects
