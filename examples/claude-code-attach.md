# Claude Code + LAD attached to your real Chrome

End-to-end walkthrough: configure Claude Code to talk to LAD, start your
Chrome with CDP enabled, attach LAD to it, and let Claude operate inside
your real authenticated browsing session — logged-in Gmail, GitHub,
Linear, Notion, whatever you already use daily — with zero re-login.

No headless ghost browser. No $19.90/mo subscription. No bespoke OAuth
dance. Just Claude Code + LAD + the Chrome you already trust.

## Prerequisites

1. **Claude Code** installed: `npm i -g @anthropic-ai/claude-code`
2. **LAD** installed: `cargo install menot-you-mcp-lad` (or `npx
   @menot-you/mcp-lad`, or `pip install menot-you-mcp-lad`)
3. **Chromium-based browser** from the last ~3 years: Chrome, Chromium,
   Brave, Edge, Opera, Arc, Vivaldi — anything that supports
   `--remote-debugging-port`. Verify with `google-chrome --version`
   (≥96).

## 1. Register LAD as an MCP server in Claude Code

Create or edit `.claude/settings.json` at your project root:

```json
{
  "mcpServers": {
    "lad": {
      "command": "llm-as-dom-mcp",
      "env": {
        "LAD_ENGINE": "chromium",
        "LAD_LLM_URL": "http://localhost:11434",
        "LAD_LLM_MODEL": "qwen2.5:7b"
      }
    }
  }
}
```

For user-wide availability across every project, use `~/.claude/settings.json`
instead.

Restart Claude Code. Verify LAD is connected:

```sh
/mcp
```

You should see `lad` in the connected servers list and `lad_browse`,
`lad_snapshot`, `lad_tabs_list`, `lad_session`, etc. in the tools list.

## 2. Start Chrome with CDP enabled

Open a fresh terminal (do not reuse the one running Claude Code):

```sh
google-chrome \
  --remote-debugging-port=9222 \
  --user-data-dir="$HOME/.cache/lad-chrome"
```

┌──────────────────────────────────────────────────────────┐
│  First run                                               │
│                                                          │
│  Chrome opens a blank window with a fresh profile.       │
│  Log into the services you want LAD to reach — GitHub,  │
│  Gmail, Linear, Notion, Figma, Slack, whatever. These    │
│  sessions persist in $HOME/.cache/lad-chrome across      │
│  runs. You only do this once.                            │
│                                                          │
│  Why a dedicated user-data-dir? Chrome refuses to open   │
│  two instances against the same profile ("profile       │
│  appears to be in use"). Using a disposable dir means    │
│  your daily-driver Chrome keeps running on your normal   │
│  profile without conflict.                               │
└──────────────────────────────────────────────────────────┘

Sanity check the debug port:

```sh
curl -s http://localhost:9222/json/version | jq -r .Browser
```

Should print something like `Chrome/145.0.0.0`. If it errors, Chrome is
not listening — recheck the flag.

## 3. Attach LAD to Chrome

In Claude Code, ask:

> Attach LAD to my local Chrome on port 9222 and tell me how many tabs
> you adopted.

Claude calls:

```json
{
  "tool": "lad_session",
  "arguments": {
    "action": "attach_cdp",
    "endpoint": "http://localhost:9222",
    "adopt_existing": true
  }
}
```

Expected response:

```json
{
  "status": "attached",
  "endpoint": "http://localhost:9222",
  "adopted_tabs": 3
}
```

`adopted_tabs` is the number of real Chrome tabs LAD inherited. Every
open non-internal tab becomes a LAD tab with a stable `tab_id`.

## 4. First real browse

> List my adopted tabs.

Claude calls `lad_tabs_list`. Response:

```json
{
  "active_tab_id": 1,
  "tabs": [
    { "id": 1, "url": "https://github.com/notifications" },
    { "id": 2, "url": "https://mail.google.com/mail/u/0/#inbox" },
    { "id": 3, "url": "https://linear.app/nott" }
  ]
}
```

Now ask Claude to actually do something on one of them:

> On the GitHub notifications tab, summarize the top 5 unread items.

Claude uses `lad_tabs_switch tab_id=1`, then `lad_snapshot` to read the
page structure, then `lad_jq` with a query like
`.elements | map(select(.role == "link")) | .[:5]` to grab the first
five notification links — paying ~300 tokens instead of the ~15k a raw
DOM dump would cost.

You never pasted a credential. You never saw a login prompt. Claude is
operating inside your real authenticated session.

## 5. Practical workflows

**Check your Linear ticket status:**

> Open Linear in a new tab and show me all tickets assigned to me that
> are in "In Progress". I don't need the descriptions, just titles.

**Scrape a private Notion page:**

> Go to https://notion.so/nott/q4-goals and extract the bulleted list
> into a JSON array.

**Verify a production deploy:**

> Open https://staging.nott.io and run `lad_audit` to flag broken
> links, missing alt text, and labels without inputs.

**Multi-tab reconciliation:**

> Compare the pricing page in tab 4 (staging) with tab 5 (production)
> and tell me what differs.

**Repeat a login flow with your real extensions:**

> Log into https://myapp.local using the credentials in my 1Password
> vault.

That last one works because 1Password's browser extension is active in
your real Chrome — LAD rides on your existing extensions automatically.

## 6. Detach when done

> Detach LAD from Chrome.

Claude calls:

```json
{ "tool": "lad_session", "arguments": { "action": "detach" } }
```

Response: `{"status": "detached"}`.

LAD releases the CDP connection and clears its tab map. Chrome keeps
running — your tabs, extensions, scroll positions, form state — all
preserved. Reattach anytime by repeating step 3.

## Troubleshooting

See [`docs/attach-chrome.md`](../docs/attach-chrome.md) for the full
security model, threat analysis, and error reference. Quick hits:

- **"attach endpoint must be loopback"** — you passed a LAN IP or
  hostname. LAD rejects anything but `localhost` / `127.0.0.1` / `::1`
  (CDP is a full RCE vector — loopback-only by design).
- **"failed to GET http://localhost:9222/json/version"** — Chrome is
  not listening. Check with `lsof -iTCP:9222 -sTCP:LISTEN`.
- **"CDP connect failed"** — Chromium version mismatch. LAD needs
  Chrome 96+.
- **"Zero adopted tabs"** — your fresh Chrome has only internal
  (`chrome://`) pages, which LAD filters out. Open a normal web tab
  first, then reattach.

## Why this beats the alternatives

1. **Playwright MCP** — works, but always spawns fresh headless Chrome
   (no auth), ~60× more tokens per operation, zero access to your real
   sessions. Good for CI, wrong for interactive dev.
2. **Opera Neon MCP Connector** — requires a $19.90/mo subscription
   and a running Opera Neon process alongside your daily browser.
   Useful if you want Opera's proprietary Neon Do / Make / ODRA agents,
   unnecessary otherwise.
3. **LAD + CDP attach** — free, self-hosted, works with any
   Chromium-based browser (not just Opera), lets Claude operate inside
   the same browser you're already logged into.

LAD is the only option that combines **real authenticated sessions**,
**CI-friendly headless mode**, and **zero subscriptions** in one tool.
