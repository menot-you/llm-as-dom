#!/usr/bin/env bash
# Fixture-based smoke tests for the lad browser pilot.
#
# Usage:
#   ./fixtures/smoke_test.sh [path-to-lad-binary]
#
# Starts a Python HTTP server on port 8789, runs extraction tests against
# every fixture, and reports pass/fail.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
LAD="${1:-./target/release/lad}"
PORT=8789
BASE="http://localhost:${PORT}"
PASS=0
FAIL=0
ERRORS=""

# ── Helpers ─────────────────────────────────────────────────────────

cleanup() {
    if [[ -n "${SERVER_PID:-}" ]]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# Portable timeout wrapper. Uses coreutils timeout/gtimeout on Linux,
# falls back to perl alarm() on macOS (always available).
portable_timeout() {
    local secs="$1"; shift
    if command -v timeout >/dev/null 2>&1; then
        timeout "$secs" "$@"
    elif command -v gtimeout >/dev/null 2>&1; then
        gtimeout "$secs" "$@"
    else
        perl -e '
            use POSIX ":sys_wait_h";
            $SIG{ALRM} = sub { kill 9, $pid if $pid; exit 124; };
            alarm(shift @ARGV);
            $pid = fork();
            if ($pid == 0) { exec @ARGV; die "exec: $!"; }
            waitpid($pid, 0);
            exit($? >> 8);
        ' "$secs" "$@"
    fi
}

start_server() {
    python3 -m http.server "$PORT" --directory "$SCRIPT_DIR" >/dev/null 2>&1 &
    SERVER_PID=$!
    # Wait for server to be ready
    for _ in $(seq 1 20); do
        if curl -sf "http://localhost:${PORT}/" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.25
    done
    echo "FATAL: HTTP server did not start on port ${PORT}"
    exit 1
}

# Run extraction against a fixture and assert keywords.
# Usage: check_fixture <fixture> <min_elements> <keyword1> [keyword2...]
check_fixture() {
    local fixture="$1"
    local min_elements="$2"
    shift 2
    local keywords=("$@")
    local url="${BASE}/${fixture}.html"
    local tmpfile
    tmpfile=$(mktemp)

    if ! portable_timeout 45 "$LAD" --url "$url" --extract-only >"$tmpfile" 2>&1; then
        FAIL=$((FAIL + 1))
        ERRORS+="  FAIL: ${fixture} -- lad exited non-zero\n"
        rm -f "$tmpfile"
        return
    fi

    local output
    output=$(cat "$tmpfile")
    rm -f "$tmpfile"

    # Extract element count from the SemanticView header: "(<N> elements, ~<M> tokens)"
    local count
    count=$(echo "$output" | sed -n 's/.*(\([0-9][0-9]*\) elements.*/\1/p' | head -1)
    count="${count:-0}"

    if [[ "$count" -lt "$min_elements" ]]; then
        FAIL=$((FAIL + 1))
        ERRORS+="  FAIL: ${fixture} -- expected >= ${min_elements} elements, got ${count}\n"
        return
    fi

    # Check keywords (case-insensitive)
    for kw in "${keywords[@]}"; do
        if ! echo "$output" | grep -qi "$kw"; then
            FAIL=$((FAIL + 1))
            ERRORS+="  FAIL: ${fixture} -- missing keyword '${kw}'\n"
            return
        fi
    done

    PASS=$((PASS + 1))
    echo "  PASS: ${fixture} (${count} elements)"
}

# ── Main ────────────────────────────────────────────────────────────

if [[ ! -x "$LAD" ]]; then
    echo "FATAL: lad binary not found or not executable at ${LAD}"
    echo "Build with: cargo build --release --bin lad"
    exit 1
fi

echo "=== Fixture Smoke Tests ==="
echo "Binary:  ${LAD}"
echo "Server:  ${BASE}"
echo ""

start_server
echo "Server PID: ${SERVER_PID}"
echo ""

# ── Fixture assertions ──────────────────────────────────────────────
# Format: check_fixture <name> <min_elements> <keywords...>

check_fixture login       3  "login"  "password"
check_fixture search      2  "search"
check_fixture register    3  "input"
check_fixture todo        1  "input"
check_fixture dashboard   2  "link"
check_fixture slow        0  "SemanticView"
check_fixture modal       2  "button"
check_fixture spa         1  "link"
check_fixture multistep   2  "input"
check_fixture chaos      10  "button"
check_fixture broken      1  "input"
check_fixture iframe_mess 4  "input"

# ── Report ──────────────────────────────────────────────────────────

echo ""
echo "=== Results: ${PASS} passed, ${FAIL} failed ==="
if [[ "$FAIL" -gt 0 ]]; then
    echo ""
    echo -e "$ERRORS"
    exit 1
fi
echo "All fixtures passed."
