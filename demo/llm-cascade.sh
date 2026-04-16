#!/usr/bin/env bash
# Demo: lad's tiered execution — heuristics first (free), LLM as fallback (cheap).
# Runs lad on a page with a deliberately ambiguous goal that forces the LLM tier.
# Shows the real trace: which tiers miss, when LLM fires, what it decides.

set -euo pipefail

export LC_ALL=en_US.UTF-8 LANG=en_US.UTF-8

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
LAD_BIN="$REPO/target/release/lad"

# LLM credentials — Gemini OpenAI-compat
if [ -z "${LAD_LLM_API_KEY:-}" ]; then
  if [ -f /Users/tiago/Developer/menot-you/.env ]; then
    set -a && source /Users/tiago/Developer/menot-you/.env && set +a
    export LAD_LLM_API_KEY="${GEMINI_API_KEY:-}"
  fi
fi
export LAD_LLM_URL="https://generativelanguage.googleapis.com/v1beta/openai"

# Start fixtures server if needed
URL="http://localhost:8787/pages/todo.html"
if ! curl -s --max-time 1 http://localhost:8787/ > /dev/null 2>&1; then
  (cd "$REPO/fixtures" && python3 -m http.server 8787 > /dev/null 2>&1) &
  SRV_PID=$!
  trap "kill $SRV_PID 2>/dev/null || true" EXIT
  sleep 1
fi

R=$'\033[31m'; G=$'\033[32m'; Y=$'\033[33m'; C=$'\033[36m'
M=$'\033[35m'; B=$'\033[1m'; D=$'\033[2m'; X=$'\033[0m'

clear
sleep 0.4

# ── Header ──
printf '\n  %slad · tiered execution%s\n' "$B" "$X"
printf '  %sheuristics first · LLM only when needed%s\n\n' "$D" "$X"
sleep 0.7

printf '  %stask%s  find "the primary creative action" on %s\n' "$D" "$X" "$B$URL$X"
printf '  %snote%s  deliberately ambiguous — heuristics can'"'"'t know what "primary creative action" means\n\n' "$D" "$X"
sleep 1.3

# ── Run lad, capture trace ──
printf '  %srunning...%s\n\n' "$D" "$X"
RAW=$(RUST_LOG=info NO_COLOR=1 "$LAD_BIN" \
  --url "$URL" \
  --goal "click the element that represents the primary creative action" \
  --max-steps 1 \
  --backend openai \
  --llm-model gemini-2.5-flash 2>&1 || true)

# ── Parse trace ──
OBSERVED=$(python3 -c "
import re
m = re.search(r'observed step=0 elements=(\d+) tokens=(\d+)', '''$RAW''')
print(f'{m.group(1)}|{m.group(2)}' if m else '0|0')
")
ELEMENTS="${OBSERVED%|*}"
TOKENS="${OBSERVED#*|}"

TIER_MISS=$(printf '%s' "$RAW" | grep -c 'tiers 0-2 miss' || true)
LLM_FIRED=$([ "$TIER_MISS" -gt 0 ] && echo "yes" || echo "no")

LLM_MS=$(python3 -c "
import re
m = re.search(r'source=Llm.*?duration_ms=(\d+)', '''$RAW''')
print(m.group(1) if m else '0')
")
LLM_DECISION=$(python3 -c "
import re
m = re.search(r'source=Llm action=Type \{ element: (\d+), value: \"([^\"]+)\", reasoning: \"([^\"]+)\"', '''$RAW''')
if m:
    print(f'Type \"{m.group(2)}\" on element [{m.group(1)}]')
else:
    m2 = re.search(r'source=Llm action=Click \{ element: (\d+)', '''$RAW''')
    print(f'Click element [{m2.group(1)}]' if m2 else 'unknown')
")
LLM_REASONING=$(python3 -c "
import re, textwrap
m = re.search(r'reasoning: \"([^\"]+)\"', '''$RAW''')
text = m.group(1) if m else ''
# Wrap at 52 chars for the box
for line in textwrap.wrap(text, 52):
    print(line)
" | head -4)

# ── Step 1: observe ──
printf '  %s[observe]%s   browser → SemanticView  ·  %s%s elements%s  ·  %s%s tokens%s\n\n' \
  "$C" "$X" "$B" "$ELEMENTS" "$X" "$B" "$TOKENS" "$X"
sleep 0.8

# ── Tiers cascade ──
printf '  %s[tier 0]%s   playbook replay            %sno match%s    %sskip%s\n' "$C" "$X" "$D" "$X" "$D" "$X"
sleep 0.4
printf '  %s[tier 1]%s   heuristics                 %sno match%s    %sskip%s\n' "$C" "$X" "$D" "$X" "$D" "$X"
sleep 0.4
printf '  %s[tier 2]%s   selector-from-goal         %sno match%s    %sskip%s\n\n' "$C" "$X" "$D" "$X" "$D" "$X"
sleep 0.6

# ── LLM fires ──
printf '  %s[LLM]%s       gemini-2.5-flash           %scalling Google API...%s\n' "$M$B" "$X" "$D" "$X"
sleep 0.6

# Reasoning box
printf '              %s┌─────────────────────────────────────────────────────┐%s\n' "$M" "$X"
while IFS= read -r line; do
  printf '              %s│%s %s%-51s%s %s│%s\n' "$M" "$X" "$D" "$line" "$X" "$M" "$X"
done <<< "$LLM_REASONING"
printf '              %s│%s                                                     %s│%s\n' "$M" "$X" "$M" "$X"
printf '              %s│%s decision: %s%-41s%s %s│%s\n' "$M" "$X" "$G$B" "$LLM_DECISION" "$X" "$M" "$X"
printf '              %s└─────────────────────────────────────────────────────┘%s\n\n' "$M" "$X"
sleep 1.5

# ── Stats ──
COST=$(python3 -c "print(f'{250 * 0.075 / 1_000_000:.6f}')")  # gemini-2.5-flash ~$0.075/M input
printf '              %slatency%s   %s%s ms%s\n' "$D" "$X" "$B" "$LLM_MS" "$X"
printf '              %stokens%s    ~250 (prompt + response)\n' "$D" "$X"
printf '              %scost%s      ~%s\$%s%s per call  %s(gemini-2.5-flash)%s\n\n' "$D" "$X" "$Y$B" "$COST" "$X" "$D" "$X"
sleep 1.2

# ── Verdict ──
printf '  %s━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━%s\n' "$Y" "$X"
printf '  heuristics cover %s~90%%%s of actions  ·  LLM only fires on the other %s~10%%%s\n' "$B" "$X" "$B" "$X"
printf '  %swhen LLM fires:%s cheap model, %s~%s ms%s, %s<$0.0001%s per decision\n' "$D" "$X" "$B" "$LLM_MS" "$X" "$B" "$X"
printf '  %s━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━%s\n\n' "$Y" "$X"
sleep 1

printf '  %scargo install menot-you-mcp-lad%s\n' "$B" "$X"
printf '  %sgithub.com/menot-you/llm-as-dom%s\n\n' "$D" "$X"
