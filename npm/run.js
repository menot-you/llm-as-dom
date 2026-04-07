#!/usr/bin/env node
// Wrapper that spawns the llm-as-dom-mcp binary with passthrough args.

const { execFileSync } = require("child_process");
const path = require("path");
const fs = require("fs");

const BINARY = "llm-as-dom-mcp";
const binaryPath = path.join(__dirname, BINARY);

if (!fs.existsSync(binaryPath)) {
  console.error(
    `llm-as-dom: binary not found at ${binaryPath}\n` +
      `This usually means the postinstall download failed.\n` +
      `\nAlternatives:\n` +
      `  cargo install example-org-mcp-lad    # if you have Rust\n` +
      `  curl -fsSL https://raw.githubusercontent.com/example-org/llm-as-dom/main/install.sh | sh\n`
  );
  process.exit(1);
}

try {
  execFileSync(binaryPath, process.argv.slice(2), {
    stdio: "inherit",
    env: process.env,
  });
} catch (err) {
  process.exit(err.status || 1);
}
