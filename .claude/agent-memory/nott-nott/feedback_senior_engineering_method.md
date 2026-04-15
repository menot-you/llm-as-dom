---
name: Senior Engineering Method — stop, analyze, THEN solve
description: Hard rule from @tiago.im. Before proposing any solution, STOP. Raise requirements, explore architecture, run blast radius, list hypotheses, question each. Clean > fast. Professional vs amateur separator.
type: feedback
---

## The rule

Whenever a problem appears, **DO NOT** propose the first solution that comes to mind. Stop. Breathe. Go through the method below. Only after that, propose a solution — and the solution must be the **clean correct** one, not the fastest.

**Why:** @tiago.im called this out after I spent an hour in firefighting mode on LAD Kata rust-toolchain flakiness — guessing, patching, retrying, guessing again. The failure mode was not bad code; it was bad **method**. A junior whack-a-moles. A senior pauses, investigates, and lands the clean fix.

**How to apply (every non-trivial problem):**

### Step 1 — Lock requirements before touching anything

Ask what the system actually needs to be:
1. Correctness criteria (must produce X result)
2. Reliability criteria (must not fail more than X% of the time)
3. Performance criteria (must complete in X time)
4. Maintainability (someone reading this in 6 months understands)
5. Reversibility (can we roll back?)
6. Security posture (what trust boundary does this sit on?)
7. Cost envelope (resource, human, operational)

Write these down. Refer back to them when evaluating solutions.

### Step 2 — Map the architecture of the failure surface

List every layer a request/operation traverses. Each layer can fail. Explicitly enumerate:
1. Application layer (what calls what)
2. Runtime layer (language, framework, interpreter)
3. Container/VM layer (runc, Kata, etc)
4. OS/kernel layer
5. Network layer (every hop: pod → bridge → node → LAN → internet → external service)
6. External dependencies (their SLA, failure modes)

You cannot solve what you cannot see.

### Step 3 — Run blast radius

For any proposed change, ask:
1. What breaks if this change is wrong?
2. What is the rollback path?
3. Who/what else depends on the current behavior?
4. Is this reversible at low cost?

If blast radius is large and reversibility is low, proceed only after H/M/L confidence check.

### Step 4 — Enumerate hypotheses honestly

List every hypothesis for the root cause, even unlikely ones. Rank by probability based on evidence. For each:
1. What observation supports it?
2. What observation refutes it?
3. What test would distinguish it from other hypotheses?

Do not skip to the "obviously it's X" hypothesis. That's usually the junior trap.

### Step 5 — Narrow via experiments, not guesses

For the top 2-3 hypotheses, design experiments that differentiate them:
1. Reproducible test (does it always fail in condition Y?)
2. Instrumentation (add logging, trace, network capture)
3. Comparison (works in environment A, fails in B — what's different?)

### Step 6 — Question every solution: clean or gambiarra?

For each candidate solution, ask:
1. Does it eliminate the root cause, or just hide the symptom?
2. Does it create new coupling, tech debt, or maintenance burden?
3. Is it consistent with the existing architectural grain?
4. Will it still be the right answer 6 months from now?
5. Is there a future workload that this will break again?

If a solution hides the root cause, it's a gambiarra. **Call it out explicitly** and present the clean alternative even if slower.

### Step 7 — Propose the clean solution with tradeoffs explicit

Present to @tiago.im:
1. Root cause (evidence-backed)
2. Clean solution (addresses root cause)
3. Alternative(s) with trade-offs named
4. Recommendation with confidence

Do NOT present option dumps disguised as analysis. Have an opinion. But the opinion must come from the method, not from speed bias.

### Step 8 — After fix, verify and write down

1. Verify the fix works deterministically, not flakily
2. Capture the learning (memory file) so next session doesn't relearn
3. If relevant, add regression test or monitoring

## Anti-patterns (junior behavior to AVOID)

1. **First-fix syndrome**: see error → guess cause → push fix → retry. Repeat.
2. **Whack-a-mole**: each retry a new guess, no convergence.
3. **Hidden gambiarra**: "bake it into the image" to avoid a flaky step instead of understanding why the step is flaky.
4. **Option dump**: present 3 options without a recommendation to offload the decision.
5. **Speed bias**: "this is faster" as primary justification.
6. **Skipping the question**: not asking "is this the clean fix or just the quick one?"

## Apply threshold

The full method is for **non-trivial problems**. Trivial (typo, 1-line config) still executes immediately per proactivity rules. But once a problem has already cost one failed fix attempt, it is **no longer trivial** — escalate to this method.
