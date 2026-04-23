# llm-as-dom — build + safe install
#
# Why this exists: macOS kills running MCP processes with
# `SIGKILL (Code Signature Invalid) / CODESIGNING / Invalid Page`
# when the binary on disk is overwritten in place AND the running
# process needs to page-in cold code under memory pressure
# (kernel re-hashes the page → mismatch → kill).
#
# Pattern: cargo install does atomic rename to ~/.cargo/bin/<name>,
# so running processes keep their original inode mapping intact.
# We additionally re-codesign both target/release/ and ~/.cargo/bin/
# copies to refresh the adhoc signature.
#
# Usage:
#   just              # → install (default)
#   just build        # build + sign in target/release/ only
#   just install      # build + atomic install to ~/.cargo/bin/ + sign
#   just verify       # confirm codesign state of installed binary

set shell := ["bash", "-uc"]

default: install

# Build release binary in target/release/ and sign it in place
build:
    cargo build --release --bin llm-as-dom-mcp
    codesign --force --sign - target/release/llm-as-dom-mcp

# Atomic install to ~/.cargo/bin/ + sign destination
# Safe to run while other Claude Code sessions are alive — cargo's atomic
# rename preserves the inode that running MCP processes have mapped.
install:
    cargo install --path . --bin llm-as-dom-mcp --force
    codesign --force --sign - ~/.cargo/bin/llm-as-dom-mcp

# Confirm codesign state of installed binary
verify:
    @echo "== ~/.cargo/bin/llm-as-dom-mcp =="
    codesign -dv ~/.cargo/bin/llm-as-dom-mcp 2>&1 | head -10
    @echo ""
    @echo "Expected: Format=Mach-O thin (arm64) / Signature=adhoc"
