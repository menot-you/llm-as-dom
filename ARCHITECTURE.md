# Architecture

## Design principles

1. **Orchestrator never sees DOM.** The expensive LLM receives structured JSON, not HTML.
2. **Heuristics first.** Rules resolve 70-90% of actions in milliseconds. LLM is fallback.
3. **LLM-agnostic.** The `PilotBackend` trait abstracts the cheap LLM. Swap Ollama for any provider.
4. **Form-scoped.** When multiple forms exist on a page, heuristics target only the relevant one.
5. **Observable.** Every step logs source (heuristic/LLM), confidence, duration, and action.

## Module map

```
src/
├── main.rs              CLI binary (lad)
├── mcp_server.rs        MCP binary (lad-mcp), 3 semantic tools
├── a11y.rs              DOM extraction + ghost-ID stamping via JS injection
├── semantic.rs          SemanticView data model + prompt serialization
├── pilot.rs             observe → heuristics → LLM → act loop
├── heuristics.rs        Rule engine: credential parsing, form fill, submit detection
├── error.rs             Unified error types
└── backend/
    ├── mod.rs
    └── ollama.rs        Ollama/Qwen3 integration with think-tag stripping
```

## Data flow

```
URL + Goal
    │
    ▼
┌─────────────┐
│  a11y.rs    │  JS injection into Chrome via CDP
│             │  querySelectorAll(interactive elements)
│             │  stamps data-lad-id on each element
│             │  returns JsExtraction { elements, visibleText, formCount }
└──────┬──────┘
       │
       ▼
┌─────────────┐
│ semantic.rs │  Converts JsExtraction → SemanticView
│             │  ~100-300 tokens (vs 15KB raw DOM)
│             │  Classifies page type (login, form, search, etc.)
└──────┬──────┘
       │
       ▼
┌──────────────┐
│  pilot.rs    │  Loop: observe → decide → act
│              │
│  ┌───────────┤
│  │heuristics │  Tier 1: rules (2-3ms)
│  │  .rs      │  - parse credentials from goal
│  │           │  - match fields by name/type/label
│  │           │  - detect submit button
│  │           │  - detect goal completion
│  └───────────┤
│  │ backend/  │  Tier 2: LLM fallback (2-8s)
│  │ ollama.rs │  - builds prompt from SemanticView
│  │           │  - strips <think> tags (Qwen3)
│  │           │  - parses JSON action
│  └───────────┤
│  │ escalate  │  Tier 3: return to orchestrator
│  └───────────┘
└──────┬───────┘
       │
       ▼
  PilotResult { success, steps, heuristic_hits, llm_hits, duration }
```

## Ghost-ID system

Each observation stamps `data-lad-id="N"` on interactive elements via JS.
Actions reference elements by this ID: `document.querySelector('[data-lad-id="2"]').click()`.

IDs are re-stamped on every observation cycle, ensuring they match the current DOM state.
The `acted_on` vector in the pilot tracks which IDs have been acted on to prevent
duplicate actions (clicking the same button twice).

## Form scoping

Pages with multiple `<form>` elements (e.g., HN has login + create account) are handled by:

1. JS extractor assigns a `form_index` to each element (which `<form>` it belongs to)
2. `target_form()` heuristic picks the first form with a password field (for login goals)
3. All field-fill and button-click heuristics filter by `in_target_form()`

## MCP protocol

The MCP server (`lad-mcp`) uses `rmcp 1.3` with stdio transport.

```
Client (Claude)                    lad-mcp
    │                                 │
    ├─ initialize ───────────────────►│
    │◄──── capabilities (tools) ──────┤
    │                                 │
    ├─ tools/call: lad_browse ───────►│
    │              { url, goal }      ├── launch browser (lazy, once)
    │                                 ├── navigate
    │                                 ├── pilot loop (heuristics + LLM)
    │◄──── { success, steps, ... } ───┤
    │                                 │
    ├─ tools/call: lad_extract ──────►│
    │              { url, what }      ├── navigate + extract SemanticView
    │◄──── { elements, text, ... } ───┤
    │                                 │
    ├─ tools/call: lad_assert ───────►│
    │              { url, asserts[] } ├── navigate + check assertions
    │◄──── { all_pass, results[] } ───┤
```

## Extending

### Add a new LLM backend

Implement `PilotBackend` (in `pilot.rs`):

```rust
#[async_trait]
pub trait PilotBackend: Send + Sync {
    async fn decide(
        &self,
        view: &SemanticView,
        goal: &str,
        history: &[Step],
    ) -> Result<Action, Error>;
}
```

### Add a new heuristic

Add a `try_*` function in `heuristics.rs` that returns `Option<HeuristicResult>`,
then wire it into `try_resolve()` with a confidence threshold check.
