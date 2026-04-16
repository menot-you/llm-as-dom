#!/usr/bin/env node
// Simulate how an LLM-driven agent would use Playwright to fill a login form:
// it has to re-read the DOM after each action to know what state the page is in.
// Emits JSON with cumulative token counts (bytes/4) and the per-call breakdown.
// Usage: node pw-login.js <url>

const { chromium } = require('playwright');
const path = require('path');
const os = require('os');

(async () => {
  const url = process.argv[2];
  if (!url) { console.error('usage: node pw-login.js <url>'); process.exit(1); }

  const fullChromium = path.join(
    os.homedir(),
    'Library/Caches/ms-playwright/chromium-1217/chrome-mac-arm64',
    'Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing',
  );
  const browser = await chromium.launch({ headless: true, executablePath: fullChromium });
  const page = await browser.newPage();

  const steps = [];

  const snapshot = async (step) => {
    const html = await page.content();
    steps.push({
      step,
      action: step.action,
      dom_bytes: html.length,
      dom_tokens: Math.round(html.length / 4),
    });
  };

  try {
    // [Tool call 1] navigate — LLM asks: "go to login page + show me DOM"
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 15000 });
    steps.push({ step: 1, action: 'navigate', dom_bytes: 0, dom_tokens: 0 });
    const html1 = await page.content();
    steps[steps.length - 1].dom_bytes = html1.length;
    steps[steps.length - 1].dom_tokens = Math.round(html1.length / 4);

    // [Tool call 2] fill email — LLM re-reads DOM to verify field exists + state
    await page.locator('input[name="email"]').fill('admin@example.com');
    const html2 = await page.content();
    steps.push({ step: 2, action: 'fill email', dom_bytes: html2.length, dom_tokens: Math.round(html2.length / 4) });

    // [Tool call 3] fill password
    await page.locator('input[name="password"]').fill('hunter2');
    const html3 = await page.content();
    steps.push({ step: 3, action: 'fill password', dom_bytes: html3.length, dom_tokens: Math.round(html3.length / 4) });

    // [Tool call 4] click submit — we DON'T click to avoid navigation, but we report it
    // (final step would add another ~780 tokens in a real flow)
    steps.push({ step: 4, action: 'click submit', dom_bytes: html3.length, dom_tokens: Math.round(html3.length / 4) });

    const total_tokens = steps.reduce((a, s) => a + s.dom_tokens, 0);
    process.stdout.write(JSON.stringify({
      tool_calls: steps.length,
      total_tokens,
      steps,
    }));
  } finally {
    await browser.close();
  }
})().catch(e => {
  process.stderr.write(`playwright error: ${e.message}\n`);
  process.exit(1);
});
