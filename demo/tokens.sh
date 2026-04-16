#!/usr/bin/env bash
# Side-by-side token comparison: Playwright vs lad.
# Numbers: README.md compression table (measured on github.com/login).

set -euo pipefail

R=$'\033[31m'   # red — expensive
G=$'\033[32m'   # green — cheap
Y=$'\033[33m'   # yellow — headline
B=$'\033[1m'
D=$'\033[2m'
X=$'\033[0m'

clear
sleep 0.4
printf '\n  %stask%s  extract login form elements from github.com/login\n' "$D" "$X"
printf '  %sgoal%s  same output. same correctness. different token bill.\n\n' "$D" "$X"
sleep 0.9

# Playwright row
printf '  %sPlaywright%s  %s— send the full rendered DOM%s\n  ' "$R$B" "$X" "$D" "$X"
for _ in $(seq 1 52); do printf '%s█%s' "$R" "$X"; sleep 0.012; done
printf '  %s25,000 tokens%s\n' "$R$B" "$X"
printf '  %scost / call%s  %s~$0.075%s  %s(Claude Sonnet input)%s\n\n' "$D" "$X" "$R" "$X" "$D" "$X"
sleep 1.1

# lad row
printf '  %slad%s  %s— send a SemanticView%s\n  ' "$G$B" "$X" "$D" "$X"
printf '%s█%s' "$G" "$X"; sleep 0.2
printf '  %s343 tokens%s\n' "$G$B" "$X"
printf '  %scost / call%s  %s~$0.001%s\n\n' "$D" "$X" "$G" "$X"
sleep 1.1

# Verdict
printf '  %s73x%s fewer tokens · %s73x%s lower cost · same result\n\n' "$Y$B" "$X" "$Y$B" "$X"
sleep 0.9

# Compression table — context
printf '  %sin the wild%s\n' "$D" "$X"
printf '    login form     8,000 → %s   91%s     %s88x%s\n' "$G$B" "$X" "$Y" "$X"
printf '    github login  25,000 → %s  343%s     %s73x%s\n' "$G$B" "$X" "$Y" "$X"
printf '    complex SPA   40,000 → %s  606%s     %s66x%s\n\n' "$G$B" "$X" "$Y" "$X"
sleep 1.5

# CTA
printf '  %s$ cargo install menot-you-mcp-lad%s\n' "$B" "$X"
printf '  %sgithub.com/menot-you/llm-as-dom%s\n\n' "$D" "$X"
sleep 0.5
