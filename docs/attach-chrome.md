# CDP attach mode — drive your own Chrome

Wave 3 added `lad_session action=attach_cdp` — LAD connects to an
already-running Chrome (or any Chromium fork: Chromium, Brave, Edge,
Opera, Arc, Vivaldi) via its remote debugging port, instead of
launching a fresh headless instance. You get browser automation
inside your real authenticated session: cookies, logins, extensions,
VPN — all live, all working, zero re-login.

No Opera Neon subscription. No `$19.90/mo` side dependency. No
bookmarks manager to port. Just your browser + LAD.

┌──────────────────────────────────────────────────────────┐
│  Why you want this                                       │
│                                                          │
│  1. Authenticated sessions — your logged-in Gmail,       │
│     GitHub, Figma, Linear, Notion, Slack, everything.    │
│  2. Extensions active — 1Password, Bitwarden, uBlock,    │
│     Grammarly, React DevTools, anything you use daily.   │
│  3. VPN routes preserved — if your browser goes through  │
│     Mullvad/WireGuard/Tailscale, so does LAD.            │
│  4. No second profile pollution — LAD uses YOUR tabs.    │
│  5. Observability — you SEE what LAD is doing, in real   │
│     time, on your own screen. Not a hidden headless ghost.│
└──────────────────────────────────────────────────────────┘

## Prerequisites

Any Chromium-based browser from the last ~3 years exposes CDP with the
`--remote-debugging-port` flag. Verify with:

```sh
google-chrome --version          # Chrome
chromium --version               # Chromium
brave-browser --version          # Brave
microsoft-edge --version         # Edge
opera --version                  # Opera (standard, not Neon)
```

If any of these prints a version, you're set.

## 1. Start Chrome with the debug port

```sh
google-chrome \
  --remote-debugging-port=9222 \
  --user-data-dir="$HOME/.cache/lad-chrome"
```

┌──────────────────────────────────────────────────────────┐
│  WHY a dedicated user-data-dir?                          │
│                                                          │
│  Chrome refuses to open two instances against the same   │
│  user data dir ("profile appears to be in use"). If you  │
│  already have Chrome running with your normal profile,   │
│  point LAD at a disposable `$HOME/.cache/lad-chrome`.    │
│                                                          │
│  First run: you will need to log back into any sites     │
│  you want LAD to use. That session persists across runs  │
│  — it's your cache, not LAD's.                           │
└──────────────────────────────────────────────────────────┘

Verify Chrome is listening:

```sh
curl -s http://localhost:9222/json/version | jq .
```

Expected JSON (excerpt):

```json
{
  "Browser": "Chrome/145.0.0.0",
  "Protocol-Version": "1.3",
  "webSocketDebuggerUrl": "ws://localhost:9222/devtools/browser/<uuid>"
}
```

If you don't see the `webSocketDebuggerUrl` field, Chrome is not
listening — re-check the `--remote-debugging-port=9222` flag.

## 2. Attach from LAD

From any MCP client (Claude Code, Cursor, nott, the CLI), call:

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

What happens under the hood:

┌──────────────────────────────────────────────────────────┐
│  1. LAD reads the endpoint param                        │
│  2. Loopback-only gate: localhost / 127.0.0.1 / ::1 OK  │
│     Any other host is rejected before any network       │
│     traffic leaves LAD.                                  │
│  3. LAD issues `GET {endpoint}/json/version` (5s        │
│     timeout) and extracts `webSocketDebuggerUrl`.        │
│  4. Second loopback check on the resolved WS URL —      │
│     defeats attackers running a rogue local /json that  │
│     redirects to a remote CDP port.                     │
│  5. `chromiumoxide::Browser::connect(ws_url)` — CDP     │
│     handshake complete.                                  │
│  6. `engine.adopt_existing_pages()` enumerates every    │
│     open tab and wraps each as a LAD tab.                │
└──────────────────────────────────────────────────────────┘

Success response:

```json
{
  "status": "attached",
  "endpoint": "http://localhost:9222",
  "adopted_tabs": 7
}
```

`adopted_tabs` is the number of real browser tabs LAD inherited.
From here on, every existing LAD tool (`lad_snapshot`, `lad_click`,
`lad_type`, `lad_browse`, `lad_fill_form`, ...) operates on the
user's real browser.

If you want a clean slate instead (start with zero tabs, open your
own via `lad_browse`), pass `"adopt_existing": false`.

## 3. Verify the adopted tabs

```json
{ "tool": "lad_tabs_list", "arguments": {} }
```

Expected response — one entry per tab LAD inherited, each with a
stable `tab_id` you can pass to any other tool via the `tab_id`
parameter.

```json
{
  "active_tab_id": 1,
  "tabs": [
    { "id": 1, "url": "https://github.com/menot-you/llm-as-dom/pull/42" },
    { "id": 2, "url": "https://mail.google.com/mail/u/0/#inbox" },
    { "id": 3, "url": "https://linear.app/nott/issue/NOTT-123" }
  ]
}
```

Every `tab_id`-accepting tool (`lad_click`, `lad_type`, `lad_eval`,
`lad_snapshot`, etc.) can now be pointed at any of these tabs.

## 4. Detach when done

```json
{
  "tool": "lad_session",
  "arguments": { "action": "detach" }
}
```

Response: `{"status": "detached"}`

LAD releases the CDP connection and clears its tab map. Chrome keeps
running — your windows, tabs, extensions, scroll positions, form
state — all preserved. Run `attach_cdp` again whenever you want to
reconnect.

If you call `detach` when LAD is not attached, you get
`{"status": "already detached"}` — idempotent, no error.

## Security model — why loopback-only?

CDP is a full remote-code-execution protocol. Whoever can speak
`Runtime.evaluate` to a Chrome debug port can run arbitrary JS on
every open page. That is why Chrome binds the debug port to
`127.0.0.1` by default.

LAD enforces the same invariant in software:

1. The `endpoint` param is parsed and its host is checked against
   the loopback set `{localhost, 127.0.0.1, ::1}`. Any other value
   — including RFC1918 private ranges (`192.168.*`, `10.*`,
   `172.16-31.*`), link-local (`169.254.*`), and "looks local"
   hostnames (`.local`, `.localhost` subdomains, `lvh.me`, `nip.io`)
   — is rejected with an `invalid_params` error.
2. After HTTP discovery, LAD re-validates the returned
   `webSocketDebuggerUrl` against the same loopback gate. A
   malicious local process cannot use `/json/version` to redirect
   LAD into connecting to a remote CDP endpoint.
3. The check lives in `sanitize::is_loopback_only` — this is the
   INVERSE of the SSRF gate (`is_safe_url`). SSRF protection blocks
   loopback targets because they're internal-network attack surface;
   CDP attach REQUIRES loopback because anything else is a remote
   code exec vector.

If you need CDP attach over a trusted network (SSH tunnel, WireGuard),
tunnel the remote port to your local `127.0.0.1:9222` — LAD will
happily talk to the tunnel endpoint because, from its perspective,
the traffic terminates on loopback. This is the correct pattern.

## Troubleshooting

**"attach endpoint must be loopback"**
You passed a hostname like `my-machine.local` or a LAN IP. LAD
refuses by design (see security model). Use `http://localhost:9222`
or `http://127.0.0.1:9222`, or SSH-tunnel the remote port first.

**"failed to GET http://localhost:9222/json/version: ... is Chrome
running with --remote-debugging-port?"**
Chrome is not listening on the port you gave. Check:

```sh
lsof -iTCP:9222 -sTCP:LISTEN
```

If nothing prints, Chrome is not running with
`--remote-debugging-port=9222`. Restart it with the flag.

**"CDP connect failed: ..."**
The HTTP probe succeeded but the WebSocket upgrade failed. This is
almost always a Chromium version mismatch — `chromiumoxide 0.9`
supports CDP 1.3 (Chrome 96+). Anything older than Chrome 96 will
not connect.

**"CDP discovery response missing webSocketDebuggerUrl field"**
You pointed LAD at an HTTP server that responded `200 OK` with JSON
that didn't have the expected shape. Double-check the endpoint —
`/json/version` (not `/json/list` or `/json`) is the correct path,
and LAD appends it for you.

**LAD adopted zero tabs even though Chrome has 5 open**
`adopt_existing` only sees normal Chrome pages — extensions,
service workers, and internal `chrome://` pages are filtered out by
`chromiumoxide::Browser::pages()`. Open a normal web tab first if
you're testing with a fresh Chrome.

**My Chrome crashed after detach**
It shouldn't — `detach` is explicitly a no-op on the underlying
process. If Chrome dies on detach, please open an issue with the
LAD version, Chrome version, and OS.

## Attach mode vs launch mode — quick reference

┌──────────────────────┬────────────────────┬────────────────────┐
│  Property            │  launch (default)  │  attach_cdp        │
├──────────────────────┼────────────────────┼────────────────────┤
│  Who owns Chrome     │  LAD               │  You               │
│  First-call cost     │  ~2-3s (cold start)│  <50ms (HTTP probe)│
│  Persistent profile  │  Opt-in via env    │  Always (yours)    │
│  Logged-in sessions  │  Fresh profile     │  Your real logins  │
│  Extensions          │  None              │  All your real     │
│  Headless            │  Default           │  No                │
│  On close            │  Kills Chrome      │  Leaves alive      │
│  Loopback-only gate  │  N/A               │  Yes               │
│  Opera Neon needed   │  No                │  No                │
└──────────────────────┴────────────────────┴────────────────────┘
