#!/usr/bin/env bash
# Run lad --extract-only on a URL, emit JSON {bytes, tokens, ms, elements}.
# Parses lad's own "tokens=N elements=M" tracing line for the real SemanticView size.

set -euo pipefail

URL="$1"
LAD_BIN="${LAD_BIN:-$(cd "$(dirname "$0")/../.." && pwd)/target/release/lad}"

t0=$(python3 -c "import time; print(time.time())")
# Disable ANSI in tracing output so we can parse it cleanly
RAW=$(RUST_LOG=info NO_COLOR=1 "$LAD_BIN" --url "$URL" --extract-only 2>&1 || true)
t1=$(python3 -c "import time; print(time.time())")

# Strip any remaining ANSI
OUT=$(printf '%s' "$RAW" | sed -E $'s/\x1b\\[[0-9;]*m//g')

# Use Python for robust parsing: pull lad's own (elements, tokens) from the header
# and measure the SemanticView text section bytes (what the LLM actually consumes).
read ELEMENTS TOKENS SV_BYTES < <(python3 -c "
import sys, re
out = sys.stdin.read()
m = re.search(r'SemanticView \((\d+) elements, ~(\d+) tokens\)', out)
elements = int(m.group(1)) if m else 0
tokens = int(m.group(2)) if m else 0
# Bytes of the text SemanticView section (up to === JSON or end)
body = re.search(r'=== SemanticView.*?(?====\s*JSON|\Z)', out, re.S)
bytes_ = len(body.group(0)) if body else 0
print(elements, tokens, bytes_)
" <<< "$OUT")

MS=$(python3 -c "print(int(($t1 - $t0) * 1000))")

python3 -c "
import json
print(json.dumps({
    'bytes': $SV_BYTES,
    'tokens': $TOKENS,
    'elements': $ELEMENTS,
    'ms': $MS,
}))
"
