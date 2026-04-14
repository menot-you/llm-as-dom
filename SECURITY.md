# Security Policy

## Supported Versions

Currently, only the latest release of LLM-as-DOM (LAD) is supported with security updates. 

| Version | Supported          |
| ------- | ------------------ |
| 0.12.x  | :white_check_mark: |
| < 0.12  | :x:                |

## Reporting a Vulnerability

If you discover a security vulnerability within LAD, please **do not** open a public issue. Instead, report it privately to the maintainer:

**Email:** tiago@docouto.dev

Please include:
* A description of the vulnerability.
* Steps to reproduce the issue (using standard MCP tools or the CLI).
* The context in which it occurs (e.g., specific websites, payloads, or configurations).

We will acknowledge your report within 48 hours and work on a fix as quickly as possible.

## Threat Model & Design Security

LAD is designed as a bridge between LLMs and a real browser. By design, the browser *executes* untrusted JavaScript from third-party sites. Security in this context primarily means **containing the LLM** and **preventing malicious sites from breaking out of the sandbox**.

### What we protect against (In Scope)
* **MCP Server Escapes:** A malicious webpage should not be able to execute arbitrary code on the host machine by manipulating the responses parsed by the LLM.
* **Denial of Service (DoS):**
  * *Infinite loops:* `while(true){}` in evaluated JavaScript (prevented by strict timeouts on `eval_js`).
  * *OOM via screenshot:* Preventing massive memory allocation if a page forces its height to 50,000px.
  * *Shadow DOM bombs:* Recursion limits (cap at 500 levels deep) to prevent stack overflow during DOM extraction.
* **Payload Truncation:** Setting rigid limits on output token counts so hostile sites cannot flood the LLM context.

### What we DO NOT protect against (Out of Scope)
* **LLM Prompt Injection via DOM Content:** If a webpage contains text like "Ignore previous instructions and do X", the LLM reading the Semantic DOM might follow it. This is considered an intrinsic risk of agentic browsing and must be mitigated at the LLM orchestration layer, not at the DOM extraction layer.
* **Browser 0-days:** LAD uses headless Chromium/Google Chrome (via the user's system). If there is a V8/Chrome sandbox escape, it is the user's responsibility to keep their browser updated. LAD does not sandbox the browser process beyond standard Chromium defaults.
* **Secret Leakage by the LLM:** If you use LAD to log into a banking website, the LLM will "see" the data. We do not scrub PII from the Semantic DOM before sending it to the MCP client.

## Safe Usage Recommendations

1. **Use Ephemeral Profiles:** Set `LAD_EPHEMERAL=1` (or use the `--ephemeral` flag) to ensure no cookies or local storage persist between sessions unless explicitly required.
2. **Restrict MCP Tool Access:** Run your orchestrator (e.g., Claude Desktop, Gemini CLI) in a restricted environment if possible. 
3. **Use the `LAD_ALLOW_EVAL` Flag Carefully:** The `lad_eval` tool allows the LLM to execute arbitrary JavaScript in the browser context. While isolated to the browser tab, this can be risky if the LLM is compromised by a prompt injection on the page. Use it only when necessary.