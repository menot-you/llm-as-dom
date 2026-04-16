#!/usr/bin/env bash
# Demo: the 90% path — heuristics-only, no LLM.
# Shows lad resolving a login task purely through pattern-matching in ~milliseconds.

set -euo pipefail
export LC_ALL=en_US.UTF-8 LANG=en_US.UTF-8

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
LAD_BIN="$REPO/target/release/lad"

# Start fixtures server if not running
if ! curl -s --max-time 1 http://localhost:8787/ > /dev/null 2>&1; then
  (cd "$REPO/fixtures" && python3 -m http.server 8787 > /dev/null 2>&1) &
  SRV_PID=$!
  trap "kill $SRV_PID 2>/dev/null || true" EXIT
  sleep 1
fi

R=$'\033[31m'; G=$'\033[32m'; Y=$'\033[33m'; C=$'\033[36m'
M=$'\033[35m'; B=$'\033[1m'; D=$'\033[2m'; X=$'\033[0m'

URL="http://localhost:8787/pages/login.html"

clear
sleep 0.3
printf '\n  %slad · the 90%% path%s\n' "$B" "$X"
printf '  %sheuristics match · no LLM needed · no cloud roundtrip · free%s\n\n' "$D" "$X"
sleep 0.8

printf '  %stask%s  login with username %sadmin%s password %shunter2%s\n' "$D" "$X" "$B" "$X" "$B" "$X"
printf '  %spage%s  %s\n\n' "$D" "$X" "$URL"
sleep 0.9

printf '  %srunning...%s\n\n' "$D" "$X"

# Run lad with multi-step goal, capture tracing
RAW=$(RUST_LOG=info NO_COLOR=1 "$LAD_BIN" \
  --url "$URL" \
  --goal "login with username admin password hunter2" \
  --max-steps 4 2>&1 || true)

# Parse all heuristic decisions
python3 <<PY
import re, time, sys

raw = """$RAW"""
# ANSI
R = "\033[31m"; G = "\033[32m"; Y = "\033[33m"; C = "\033[36m"
M = "\033[35m"; B = "\033[1m"; D = "\033[2m"; X = "\033[0m"

# Parse observed lines
observed = re.findall(r'observed step=(\d+) elements=(\d+) tokens=(\d+)', raw)
decisions = re.findall(
    r"decided step=(\d+) source=(\w+) action=(\w+) \{ ([^}]+) \} duration_ms=(\d+)", raw)
heuristic_reasons = re.findall(
    r'heuristic matched source=\"heuristic\" confidence=(\S+) reason=([^\n]+)', raw)

# Display
step_count = len(decisions)
for i, dec in enumerate(decisions):
    step, source, action, details, ms = dec
    reason = heuristic_reasons[i][1].strip() if i < len(heuristic_reasons) else ''
    conf = heuristic_reasons[i][0] if i < len(heuristic_reasons) else '?'

    # Element + value from details
    el_m = re.search(r'element: (\d+)', details)
    val_m = re.search(r'value: "([^"]+)"', details)
    elem = el_m.group(1) if el_m else '?'
    val = val_m.group(1) if val_m else ''

    if source == 'Heuristic':
        try:
            conf_pct = f"{float(conf) * 100:.0f}%"
        except Exception:
            conf_pct = conf
        print(f"  {C}[step {step}]{X}  {G}{B}heuristic hit{X}  {D}pattern:{X} {reason.strip()[:50]}")
        act_label = f'{action} "{val}" on element [{elem}]' if val else f'{action} element [{elem}]'
        print(f"             action: {B}{act_label}{X}")
        print(f"             {D}{ms}ms · confidence {conf_pct} · 0 LLM tokens · no network{X}")
    else:
        print(f"  {C}[step {step}]{X}  {M}{source.lower()}{X}  {action} element [{elem}]  {D}{ms}ms{X}")
    print()
    time.sleep(0.7)

# Summary
steps_line = re.search(r'Steps: (\d+) \(heuristic: (\d+), llm: (\d+)\)', raw)
if steps_line:
    total, heur, llm = steps_line.groups()
    print(f"  {Y}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{X}")
    print(f"  {B}{heur} heuristic hits{X} · {B}{llm} LLM calls{X} · no network · no cost")
    print(f"  {D}this is how 90% of actions land — zero LLM, pure Rust pattern-match{X}")
    print(f"  {Y}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{X}")
PY

sleep 1.2
printf '\n  %scargo install menot-you-mcp-lad%s\n' "$B" "$X"
printf '  %sgithub.com/menot-you/llm-as-dom%s\n\n' "$D" "$X"
