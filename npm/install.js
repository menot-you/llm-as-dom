#!/usr/bin/env node
// Postinstall script: downloads the llm-as-dom-mcp binary from GitHub Releases.

const { execSync } = require("child_process");
const fs = require("fs");
const path = require("path");
const https = require("https");

const VERSION = require("./package.json").version;
const BINARY = "llm-as-dom-mcp";
const REPO = "menot-you/llm-as-dom";

function getPlatform() {
  const platform = process.platform;
  const arch = process.arch;

  if (platform === "darwin" && arch === "arm64") return "macos-arm64";
  if (platform === "linux" && arch === "x64") return "linux-x86_64";

  throw new Error(
    `Unsupported platform: ${platform}-${arch}. ` +
      `Supported: darwin-arm64, linux-x64. ` +
      `Install from source: cargo install menot-you-mcp-lad`
  );
}

function download(url) {
  return new Promise((resolve, reject) => {
    https
      .get(url, (res) => {
        // Follow redirects (GitHub releases redirect to S3)
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          return download(res.headers.location).then(resolve, reject);
        }
        if (res.statusCode !== 200) {
          return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
        }
        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () => resolve(Buffer.concat(chunks)));
        res.on("error", reject);
      })
      .on("error", reject);
  });
}

async function main() {
  const platform = getPlatform();
  const artifact = `${BINARY}-${platform}`;
  const url = `https://github.com/${REPO}/releases/download/v${VERSION}/${artifact}`;

  console.log(`llm-as-dom: downloading ${BINARY} v${VERSION} for ${platform}...`);

  try {
    const data = await download(url);
    const dest = path.join(__dirname, BINARY);
    fs.writeFileSync(dest, data);
    fs.chmodSync(dest, 0o755);
    console.log(`llm-as-dom: installed ${BINARY} (${(data.length / 1024 / 1024).toFixed(1)} MB)`);
  } catch (err) {
    console.error(`llm-as-dom: failed to download binary — ${err.message}`);
    console.error(`llm-as-dom: you can install from source: cargo install menot-you-mcp-lad`);
    // Don't fail the install — the run.js wrapper will show a helpful error
  }
}

main();
