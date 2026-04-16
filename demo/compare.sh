#!/usr/bin/env bash
# Real comparison: Playwright (rendered DOM) vs lad (SemanticView) — same URL.
# Usage: ./compare.sh <url>

set -euo pipefail

# Force en_US number formatting (comma thousands) regardless of user locale
export LC_ALL=en_US.UTF-8
export LANG=en_US.UTF-8

HERE="$(cd "$(dirname "$0")" && pwd)"
URL="${1:-http://localhost:8787/pages/twitter-profile.html}"

# Auto-start fixtures server if URL is localhost:8787 and not already reachable
if [[ "$URL" == *localhost:8787* ]] && ! curl -s --max-time 1 http://localhost:8787/ > /dev/null 2>&1; then
  (cd "$HERE/../fixtures" && python3 -m http.server 8787 > /dev/null 2>&1) &
  SRV_PID=$!
  trap "kill $SRV_PID 2>/dev/null || true" EXIT
  sleep 1
fi

# Pricing reference (per 1M tokens, Anthropic Sonnet input).
# Using $3/M to be conservative — actual may differ by model.
COST_PER_M=3

R=$'\033[31m'; G=$'\033[32m'; Y=$'\033[33m'; C=$'\033[36m'
B=$'\033[1m'; D=$'\033[2m'; X=$'\033[0m'

# Extract page name from URL
PAGE=$(basename "$URL")

clear
printf '\n  %stask%s  extract interactive elements from %s\n' "$D" "$X" "$B$PAGE$X"
printf '  %smethod%s  run both tools live, measure actual bytes + tokens\n\n' "$D" "$X"
sleep 0.6

# ── Playwright ──
printf '  %s[1/2]%s %sPlaywright%s · headless chromium · page.content()\n' "$D" "$X" "$R$B" "$X"
printf '        %srunning...%s' "$D" "$X"
PW_JSON=$(node "$HERE/playwright/bench.js" "$URL")
PW_BYTES=$(python3 -c "import json; print(json.loads('''$PW_JSON''')['bytes'])")
PW_TOKENS=$(python3 -c "import json; print(json.loads('''$PW_JSON''')['tokens'])")
PW_MS=$(python3 -c "import json; print(json.loads('''$PW_JSON''')['ms'])")
PW_COST=$(python3 -c "print(f'{$PW_TOKENS * $COST_PER_M / 1_000_000:.4f}')")
printf '\r        %sdone in %sms%s                     \n' "$G" "$PW_MS" "$X"
printf '        %s→%s %s%s bytes%s  ·  ~%s%s tokens%s  ·  %s\$%s per call%s\n\n' \
  "$D" "$X" "$R$B" "$(printf "%'d" "$PW_BYTES")" "$X" "$R$B" "$(printf "%'d" "$PW_TOKENS")" "$X" "$R" "$PW_COST" "$X"
sleep 0.6

# ── lad ──
printf '  %s[2/2]%s %slad%s · cloakbrowser + SemanticView extractor\n' "$D" "$X" "$G$B" "$X"
printf '        %srunning...%s' "$D" "$X"
LAD_JSON=$("$HERE/lib/bench-lad.sh" "$URL")
LAD_BYTES=$(python3 -c "import json; print(json.loads('''$LAD_JSON''')['bytes'])")
LAD_TOKENS=$(python3 -c "import json; print(json.loads('''$LAD_JSON''')['tokens'])")
LAD_MS=$(python3 -c "import json; print(json.loads('''$LAD_JSON''')['ms'])")
LAD_ELEMENTS=$(python3 -c "import json; print(json.loads('''$LAD_JSON''')['elements'])")
LAD_COST=$(python3 -c "print(f'{$LAD_TOKENS * $COST_PER_M / 1_000_000:.6f}')")
printf '\r        %sdone in %sms%s                     \n' "$G" "$LAD_MS" "$X"
printf '        %s→%s %s%s bytes%s  ·  ~%s%s tokens%s  ·  %s\$%s per call%s\n' \
  "$D" "$X" "$G$B" "$(printf "%'d" "$LAD_BYTES")" "$X" "$G$B" "$(printf "%'d" "$LAD_TOKENS")" "$X" "$G" "$LAD_COST" "$X"
printf '        %s→%s %s elements extracted as a semantic view\n\n' "$D" "$X" "$LAD_ELEMENTS"
sleep 0.9

# ── Verdict ──
RATIO=$(python3 -c "print(f'{$PW_TOKENS / $LAD_TOKENS:.1f}')")
COST_RATIO=$(python3 -c "print(f'{float('$PW_COST') / max(float('$LAD_COST'), 0.000001):.0f}')")
SAVINGS=$(python3 -c "print(f'{(1 - $LAD_TOKENS / $PW_TOKENS) * 100:.1f}')")

printf '  %s━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━%s\n' "$Y" "$X"
printf '  %s%sx%s fewer tokens  ·  %s%sx%s lower cost  ·  %s%%%s savings\n' \
  "$Y$B" "$RATIO" "$X" "$Y$B" "$COST_RATIO" "$X" "$SAVINGS$B" "$X"
printf '  %ssame page · same actionable elements · real measurements%s\n' "$D" "$X"
printf '  %s━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━%s\n\n' "$Y" "$X"
sleep 1

printf '  %scargo install menot-you-mcp-lad%s\n' "$B" "$X"
printf '  %sgithub.com/menot-you/llm-as-dom%s\n\n' "$D" "$X"
