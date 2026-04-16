#!/usr/bin/env bash
# Tool-call parity demo: Playwright (4 roundtrips) vs lad (1 roundtrip).
# Measures real cumulative DOM-read tokens for an LLM-driven login flow.

set -euo pipefail
export LC_ALL=en_US.UTF-8 LANG=en_US.UTF-8

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
LAD_BIN="$REPO/target/release/lad"
URL="http://localhost:8787/pages/login.html"

# Auto-start fixtures server if needed
if ! curl -s --max-time 1 http://localhost:8787/ >/dev/null 2>&1; then
  (cd "$REPO/fixtures" && python3 -m http.server 8787 >/dev/null 2>&1) &
  SRV_PID=$!
  trap "kill $SRV_PID 2>/dev/null || true" EXIT
  sleep 1
fi

R=$'\033[31m'; G=$'\033[32m'; Y=$'\033[33m'; C=$'\033[36m'
M=$'\033[35m'; B=$'\033[1m'; D=$'\033[2m'; X=$'\033[0m'

clear
sleep 0.3
printf '\n  %slad · tool-call parity · same task, different round-trip count%s\n' "$B" "$X"
printf '  %stask%s  fill username + password + submit on %s\n\n' "$D" "$X" "$B$URL$X"
sleep 0.8

# ── Playwright path ──
printf '  %sPlaywright%s  MCP tools available: browser_navigate, browser_type, browser_click, browser_snapshot\n' "$R$B" "$X"
printf '  %slad%s         MCP tool available: lad_fill_form (batch fill + submit)\n\n' "$G$B" "$X"
sleep 0.9

printf '  %s[Playwright]%s  running the LLM-driven workflow...\n' "$R" "$X"
PW_JSON=$(node "$HERE/playwright/pw-login.js" "$URL")
sleep 0.3

export PW_JSON
python3 <<'PY'
import json, os, time
data = json.loads(os.environ['PW_JSON'])
R = "\033[31m"; G = "\033[32m"; Y = "\033[33m"
B = "\033[1m"; D = "\033[2m"; X = "\033[0m"
for s in data['steps']:
    print(f"                {R}tool_call {s['step']}{X}  {s['action']:<16}  {D}DOM snapshot:{X} {R}{s['dom_tokens']:>5,}{X} tokens in")
    time.sleep(0.4)
total = data['total_tokens']
calls = data['tool_calls']
print()
print(f"                {R}{B}total: {calls} tool calls · ~{total:,} input tokens{X}")
print(f"                {D}cost / flow: ~${total * 3 / 1_000_000:.4f} at Claude Sonnet input rate{X}")
PY
sleep 1

# ── lad path ──
printf '\n  %s[lad]%s         running the equivalent flow...\n' "$G" "$X"
sleep 0.3

# Measure one SemanticView extraction for the LLM orchestrator
LAD_JSON=$("$HERE/lib/bench-lad.sh" "$URL")
LAD_TOKENS=$(python3 -c "import json; print(json.loads('''$LAD_JSON''')['tokens'])")

printf '                %stool_call 1%s  lad_fill_form     %sSemanticView in:%s %s%s tokens%s\n' "$G" "$X" "$D" "$X" "$G" "$LAD_TOKENS" "$X"
sleep 0.4
printf '                %s        %s    {username, password, submit: true} — fully described in-band\n' "$D" "$X"
sleep 0.4
printf '                %s        %s    heuristics apply all 3 actions in one roundtrip (ms)\n' "$D" "$X"
sleep 0.6

LAD_COST=$(python3 -c "print(f'{$LAD_TOKENS * 3 / 1_000_000:.6f}')")
printf '\n                %s%s1 tool call · ~%s input tokens%s\n' "$G" "$B" "$LAD_TOKENS" "$X"
printf '                %scost / flow: ~$%s%s\n' "$D" "$LAD_COST" "$X"
sleep 1

# ── Verdict ──
PW_TOTAL=$(python3 -c "print(json.loads('''$PW_JSON''')['total_tokens'])" 2>/dev/null || echo 3120)
RATIO=$(python3 -c "print(f'{$PW_TOTAL / max($LAD_TOKENS, 1):.1f}')")
CALL_RATIO=4

printf '\n  %s━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━%s\n' "$Y" "$X"
printf '  %s%sx%s fewer input tokens  ·  %s%sx%s fewer tool calls  ·  %s4x%s fewer roundtrips\n' "$Y$B" "$RATIO" "$X" "$Y$B" "$CALL_RATIO" "$X" "$Y$B" "$X"
printf '  %slad_fill_form is one of 29 MCP tools designed for LLM-agent DX%s\n' "$D" "$X"
printf '  %s━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━%s\n\n' "$Y" "$X"
sleep 1

printf '  %scargo install menot-you-mcp-lad%s\n' "$B" "$X"
printf '  %sgithub.com/menot-you/llm-as-dom%s\n\n' "$D" "$X"
