#!/usr/bin/env node
// Navigate a URL with Playwright, emit the rendered DOM size.
// Playwright's equivalent of what lad is replacing: full HTML dump sent to the LLM.
// Usage: node bench.js <url>

const { chromium } = require('playwright');

(async () => {
  const url = process.argv[2];
  if (!url) {
    console.error('usage: node bench.js <url>');
    process.exit(1);
  }
  const t0 = Date.now();
  // Use the full chromium (headless_shell may still be downloading on fresh install)
  const path = require('path');
  const os = require('os');
  const fullChromium = path.join(
    os.homedir(),
    'Library/Caches/ms-playwright/chromium-1217/chrome-mac-arm64',
    'Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing',
  );
  const browser = await chromium.launch({
    headless: true,
    executablePath: fullChromium,
  });
  try {
    const page = await browser.newPage();
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 15000 });
    const html = await page.content();
    const ms = Date.now() - t0;
    process.stdout.write(JSON.stringify({
      bytes: html.length,
      tokens: Math.round(html.length / 4),
      ms,
    }));
  } finally {
    await browser.close();
  }
})().catch(e => {
  process.stderr.write(`playwright error: ${e.message}\n`);
  process.exit(1);
});
