#!/usr/bin/env node
// Alias: delegates to @example-org/mcp-lad
const { execFileSync } = require("child_process");
const path = require("path");

const bin = path.join(
  __dirname,
  "node_modules",
  "@example-org",
  "mcp-lad",
  "run.js"
);

try {
  execFileSync("node", [bin, ...process.argv.slice(2)], { stdio: "inherit" });
} catch (e) {
  process.exit(e.status || 1);
}
