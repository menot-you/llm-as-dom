#!/usr/bin/env bash
# Run Playwright + lad benches on a list of URLs, emit JSON array.
# Usage: ./bench-all.sh <url> [url ...]
# Output: JSON to stdout + cache written to bench-cache.json

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
CACHE="$(dirname "$HERE")/bench-cache.json"

# Auto-start fixtures server if needed
if ! curl -s --max-time 1 http://localhost:8787/ >/dev/null 2>&1; then
  (cd "$REPO/fixtures" && python3 -m http.server 8787 >/dev/null 2>&1) &
  SRV_PID=$!
  trap "kill $SRV_PID 2>/dev/null || true" EXIT
  sleep 1.5
fi

URLS=("$@")
RESULTS="["
SEP=""

for URL in "${URLS[@]}"; do
  PAGE=$(basename "$URL")
  echo "  measuring $PAGE..." >&2

  PW_JSON=$(node "$HERE/../playwright/bench.js" "$URL")
  LAD_JSON=$("$HERE/bench-lad.sh" "$URL")

  ENTRY=$(python3 -c "
import json
pw = json.loads('''$PW_JSON''')
lad = json.loads('''$LAD_JSON''')
ratio = pw['tokens'] / max(lad['tokens'], 1)
print(json.dumps({
    'page': '$PAGE',
    'url': '$URL',
    'playwright': pw,
    'lad': lad,
    'ratio': round(ratio, 1),
}))
")
  RESULTS="${RESULTS}${SEP}${ENTRY}"
  SEP=","
done

RESULTS="${RESULTS}]"
echo "$RESULTS" > "$CACHE"
echo "$RESULTS"
echo "  cache written: $CACHE" >&2
