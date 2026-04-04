#!/usr/bin/env bash
# Scaffold a new lad test fixture.
#
# Usage:
#   ./fixtures/new-fixture.sh
#
# Interactive — prompts for name, category, and metadata.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ── Colors ─────────────────────────────────────────────────────────

bold() { printf "\033[1m%s\033[0m" "$1"; }
green() { printf "\033[32m%s\033[0m" "$1"; }
yellow() { printf "\033[33m%s\033[0m" "$1"; }

# ── Prompts ────────────────────────────────────────────────────────

echo ""
bold "🧪 New lad fixture"
echo ""

# Category
echo "Category:"
echo "  1) pages        — real-world page simulation"
echo "  2) edge-cases   — tricky pattern lad hit in the wild"
echo "  3) adversarial  — intentional attack vector"
echo ""
read -rp "Choose [1-3]: " cat_choice

case "$cat_choice" in
    1) CATEGORY="pages" ;;
    2) CATEGORY="edge-cases" ;;
    3) CATEGORY="adversarial" ;;
    *) echo "Invalid choice"; exit 1 ;;
esac

# Name
echo ""
read -rp "Fixture name (snake_case, no extension): " NAME

if [[ -z "$NAME" ]]; then
    echo "Name cannot be empty"; exit 1
fi

# ── Adversarial-specific ───────────────────────────────────────────

if [[ "$CATEGORY" == "adversarial" ]]; then
    # Auto-detect next number
    LAST_NUM=$(ls "$SCRIPT_DIR/adversarial/"*.html 2>/dev/null \
        | sed 's/.*\/\([0-9]*\)_.*/\1/' \
        | sort -n \
        | tail -1)
    NEXT_NUM=$(printf "%02d" $(( ${LAST_NUM:-0} + 1 )))
    FILENAME="${NEXT_NUM}_${NAME}.html"

    echo ""
    echo "Attack type:"
    echo "  1) extraction      — visibility, DOM structure tricks"
    echo "  2) timing          — race conditions, delayed content"
    echo "  3) action          — interaction failures"
    echo "  4) classification  — misleading semantics/labels"
    echo "  5) llm-confusion   — LLM-specific weaknesses"
    echo ""
    read -rp "Choose [1-5]: " attack_choice

    case "$attack_choice" in
        1) ATTACK_TYPE="Extraction attack" ;;
        2) ATTACK_TYPE="Timing attack" ;;
        3) ATTACK_TYPE="Action attack" ;;
        4) ATTACK_TYPE="Classification attack" ;;
        5) ATTACK_TYPE="LLM confusion" ;;
        *) echo "Invalid choice"; exit 1 ;;
    esac

    echo ""
    read -rp "Attack description (what it tests): " ATTACK_DESC
    read -rp "Expected failure (how lad fails without fix): " EXPECTED_FAILURE

    FILEPATH="$SCRIPT_DIR/adversarial/$FILENAME"
else
    FILENAME="${NAME}.html"
    FILEPATH="$SCRIPT_DIR/$CATEGORY/$FILENAME"
fi

# ── Check collision ────────────────────────────────────────────────

if [[ -f "$FILEPATH" ]]; then
    echo "ERROR: $FILEPATH already exists"
    exit 1
fi

# ── Generate HTML ──────────────────────────────────────────────────

TITLE=$(echo "$NAME" | tr '_' ' ' | sed 's/\b\(.\)/\u\1/g')

cat > "$FILEPATH" <<HTMLEOF
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <title>$TITLE</title>
  <style>
    body { font-family: system-ui; margin: 20px; }
    /* TODO: add fixture styles */
  </style>
</head>
<body>
  <!-- TODO: add fixture HTML -->
  <p>$TITLE fixture — replace this with your test content.</p>

  <script>
    // TODO: add dynamic behavior (if needed)
  </script>
</body>
</html>
HTMLEOF

echo ""
green "✓ Created: $FILEPATH"

# ── Update manifest (adversarial only) ─────────────────────────────

if [[ "$CATEGORY" == "adversarial" ]]; then
    MANIFEST="$SCRIPT_DIR/adversarial/manifest.json"

    if [[ -f "$MANIFEST" ]]; then
        # Remove trailing ] and add new entry
        # Read the HTML we just generated as a single line for the manifest
        HTML_CONTENT=$(cat "$FILEPATH" | python3 -c "import sys,json; print(json.dumps(sys.stdin.read()))" | sed 's/^"//;s/"$//')

        # Use python3 to safely append to JSON array
        python3 -c "
import json, sys

with open('$MANIFEST', 'r') as f:
    data = json.load(f)

data.append({
    'name': '$TITLE',
    'attack': '$ATTACK_TYPE: $ATTACK_DESC',
    'expected_failure': '$EXPECTED_FAILURE',
    'html': open('$FILEPATH').read()
})

with open('$MANIFEST', 'w') as f:
    json.dump(data, f, indent=2)
    f.write('\n')
"
        green "✓ Updated: adversarial/manifest.json"
    fi

    echo ""
    yellow "Next steps:"
    echo "  1. Edit $FILEPATH with your attack HTML"
    echo "  2. Re-run: python3 to update manifest.json html field"
    echo "     (or run this script again to regenerate)"
else
    echo ""
    yellow "Next steps:"
    echo "  1. Edit $FILEPATH with your fixture HTML"
    echo "  2. Add to smoke_test.sh:"
    echo "     check_fixture $CATEGORY/$NAME  <min_elements>  \"keyword1\""
fi

echo ""
