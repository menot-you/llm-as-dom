---
name: LAD DX Roadmap
description: Consolidated DX findings from Gemini (product designer) + Opus (agent consumer). Priority-ranked for implementation.
type: project
---

## DX Wave 1 — DONE ✅
1. lad_snapshot url optional
2. lad_browse returns SemanticView
3. Tool descriptions fixed
4. lad_type press_enter
5. lad_scroll tool added

## DX Wave 2 — HIGH PRIORITY
1. lad_assert + lad_extract url optional (operate on active page)
2. Checkbox/radio `checked` state + select options in SemanticView
3. lad_fill_form batch tool (fields={}, submit=true)
4. form_index rendered in to_prompt()
5. lad_select by label (not just value)

## DX Wave 3 — MEDIUM PRIORITY
1. lad_wait with OR conditions (mode="any")
2. lad_refresh explicit reload
3. lad_clear for controlled components
4. lad_dialog auto-accept default (no temporal trap)
5. Element count summary in snapshot header
6. lad_extract format="prompt" option (consistency with snapshot)
7. lad_network show actual HTTP methods + status codes

## DX Wave 4 — NICE TO HAVE
1. lad_hover delay_ms parameter
2. Multiple concurrent watches
3. Action string enums (not free-form)
4. lad_browse page reuse (don't always create new page)

## Sources
- Gemini 3.1 Pro: product designer angle (8 findings, LGTM after wave 1)
- Opus @investigator: agent consumer angle (10 delights, 9 friction, 7 missing, 5 confusing)

**Why:** DX is as important as security. A tool that's secure but frustrating to use will be abandoned. These findings came from asking "what would I want as an AI agent?" — a question no security reviewer asks.
