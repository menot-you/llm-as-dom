#!/usr/bin/env node
// Alias: delegates to @menot-you/mcp-lad
const { execFileSync } = require("child_process");
const path = require("path");

const bin = path.join(
  __dirname,
  "node_modules",
  "@menot-you",
  "mcp-lad",
  "run.js"
);

try {
  execFileSync("node", [bin, ...process.argv.slice(2)], { stdio: "inherit" });
} catch (e) {
  process.exit(e.status || 1);
}
