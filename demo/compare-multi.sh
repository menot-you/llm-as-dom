#!/usr/bin/env bash
# Multi-page comparison: reads bench-cache.json and displays a table.
# Regenerate the cache with: ./lib/bench-all.sh <url1> <url2> <url3>

set -euo pipefail
export LC_ALL=en_US.UTF-8 LANG=en_US.UTF-8

HERE="$(cd "$(dirname "$0")" && pwd)"
CACHE="$HERE/bench-cache.json"

if [ ! -f "$CACHE" ]; then
  echo "no cache · run: ./lib/bench-all.sh <urls> to generate" >&2
  exit 1
fi

R=$'\033[31m'; G=$'\033[32m'; Y=$'\033[33m'; C=$'\033[36m'
B=$'\033[1m'; D=$'\033[2m'; X=$'\033[0m'

clear
sleep 0.3
printf '\n  %slad · token compression across page types%s\n' "$B" "$X"
printf '  %sreal measurements — Playwright rendered DOM vs lad SemanticView%s\n\n' "$D" "$X"
sleep 0.8

# Header row
printf '  %s%-24s  %12s  %12s  %8s  %s%s\n' \
  "$B" "page" "Playwright" "lad" "ratio" "page size" "$X"
printf '  %s────────────────────────  ────────────  ────────────  ────────  ─────────%s\n' "$D" "$X"
sleep 0.5

# Data rows from cache
export CACHE
python3 <<'PY'
import json, os, time
with open(os.environ['CACHE']) as f:
    rows = json.load(f)
R = "\033[31m"; G = "\033[32m"; Y = "\033[33m"
B = "\033[1m"; D = "\033[2m"; X = "\033[0m"
for r in rows:
    pw_tok = r['playwright']['tokens']
    lad_tok = r['lad']['tokens']
    ratio = r['ratio']
    pw_kb = r['playwright']['bytes'] / 1024
    page = r['page'][:24]
    print(f"  {B}{page:<24}{X}  {R}{pw_tok:>10,}{X} t  {G}{lad_tok:>10,}{X} t  {Y}{B}{ratio:>6.1f}x{X}  {D}{pw_kb:>6.1f} KB{X}")
    time.sleep(0.4)
PY

sleep 0.8
printf '\n  %s▸ bigger pages → bigger compression (more HTML to strip)%s\n' "$D" "$X"
printf '  %s▸ small forms: ~6-10x · dashboards: ~10-30x · real UI: 100x+%s\n\n' "$D" "$X"
sleep 1

# Pricing footer
printf '  %spricing reference (Claude Sonnet @ $3/M input tokens)%s\n' "$D" "$X"
python3 <<'PY'
import json, os
with open(os.environ['CACHE']) as f:
    rows = json.load(f)
D = "\033[2m"; X = "\033[0m"
for r in rows:
    pw = r['playwright']['tokens'] * 3 / 1_000_000
    lad_c = r['lad']['tokens'] * 3 / 1_000_000
    pct = (1 - r['lad']['tokens'] / max(r['playwright']['tokens'], 1)) * 100
    print(f"    {r['page'][:24]:<24}  ${pw:.4f} -> ${lad_c:.5f}  ({pct:>4.1f}% saved)")
PY

sleep 1.3
printf '\n  %scargo install menot-you-mcp-lad%s\n' "$B" "$X"
printf '  %sgithub.com/menot-you/llm-as-dom%s\n\n' "$D" "$X"
