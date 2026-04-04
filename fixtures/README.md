# lad test fixtures

Static HTML pages used by lad's test suite and smoke tests. Each fixture simulates a real-world or adversarial scenario that lad must handle correctly.

## Directory structure

```
fixtures/
├── pages/           # Real-world page simulations (login, search, dashboard...)
├── edge-cases/      # Tricky patterns (broken HTML, chaos pages, iframes, slow loads)
├── adversarial/     # 54 numbered attack vectors with manifest.json
├── serve.sh         # Dev server (python3, port 8787)
├── smoke_test.sh    # Automated smoke tests against all fixtures
└── new-fixture.sh   # Scaffold a new fixture
```

## How fixtures are used

### Smoke tests (CI + local)

```bash
# Build lad first
cargo build --release --bin lad

# Run all smoke tests
./fixtures/smoke_test.sh

# Or with a custom binary path
./fixtures/smoke_test.sh ./target/debug/lad
```

The smoke test spins up a local HTTP server on port 8789, runs `lad --url <fixture> --extract-only` against each fixture, and asserts:
- Minimum element count extracted
- Required keywords present in the SemanticView output

### Dev server (manual testing)

```bash
./fixtures/serve.sh
# Open http://localhost:8787/pages/login.html
```

### Rust integration tests

Tests in `tests/integration.rs` and `tests/chaos.rs` use **mock SemanticViews** (not HTML fixtures directly). The fixtures exist for browser-level testing via `smoke_test.sh` and the `--ignored` integration tests that need a real browser.

## Fixture categories

### `pages/` — real-world scenarios

Standard pages a web agent would encounter. Each tests core extraction: forms, buttons, links, inputs.

| Fixture | Tests |
|---------|-------|
| `login.html` | Email/password form, submit button, error states |
| `register.html` | Multi-field form, validation |
| `dashboard.html` | Navigation links, data display |
| `search.html` | Search input, results list |
| `todo.html` | Dynamic list, add/remove interactions |
| `modal.html` | Overlay dialogs, backdrop clicks |
| `multistep.html` | Multi-step wizard, state transitions |
| `spa.html` | Client-side routing, dynamic content |

### `edge-cases/` — tricky patterns

Pages that break naive extractors. These exist because lad hit them in the wild.

| Fixture | Tests |
|---------|-------|
| `broken.html` | Malformed HTML, unclosed tags |
| `chaos.html` | Comic Sans, marquee, cookie popups, fake buttons, nested tables |
| `iframe_mess.html` | Deeply nested iframes with cross-origin-like structures |
| `slow.html` | Content that loads after delays |
| `hinted_login.html` | Login with `data-lad-hint` attributes |

### `adversarial/` — attack vectors

54 numbered scenarios designed to break lad. Each file targets a specific failure mode. Metadata lives in `manifest.json` with fields:

```json
{
  "name": "Human-readable name",
  "attack": "What this tests (extraction/timing/action/classification/llm-confusion)",
  "expected_failure": "How lad is expected to fail without mitigation",
  "html": "..."
}
```

**Attack categories:**

| Category | IDs | What it tests |
|----------|-----|---------------|
| Extraction | 01-05, 08-10, 34-40, 41-48, 50-54 | Visibility tricks, shadow DOM, CSS illusions, non-semantic elements |
| Timing | 06, 11-16, 49 | Race conditions, delayed content, dynamic state changes |
| Action | 07, 17-21 | ID collisions, moving elements, alert blocks, token bombs |
| Classification | 22-25, 30-31 | Misleading labels, wrong page types, ARIA contradictions |
| LLM confusion | 26-29, 32-33 | Identical labels, i18n, unicode lookalikes, high-cardinality |

## Adding a new fixture

### Quick: use the scaffold script

```bash
./fixtures/new-fixture.sh
```

It will ask for name, category, and (if adversarial) attack metadata. Generates the HTML boilerplate and updates `manifest.json`.

### Manual: copy the template

**For `pages/` or `edge-cases/`:**

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <title>Fixture Name</title>
  <style>
    /* Keep styles minimal — test the DOM, not the CSS */
    body { font-family: system-ui; margin: 20px; }
  </style>
</head>
<body>
  <!-- Your fixture HTML here -->

  <script>
    // Optional: dynamic behavior
  </script>
</body>
</html>
```

Then add an assertion in `smoke_test.sh`:

```bash
check_fixture pages/my_fixture  3  "keyword1"  "keyword2"
```

**For `adversarial/`:**

1. Create `NN_snake_case_name.html` (next number in sequence)
2. Add an entry to `adversarial/manifest.json`
3. No smoke_test.sh entry needed — adversarial fixtures are tested separately

### Naming conventions

- `pages/`: lowercase, descriptive (`login.html`, `checkout.html`)
- `edge-cases/`: lowercase, describes the problem (`broken.html`, `slow.html`)
- `adversarial/`: `NN_snake_case_description.html` — number is permanent, never reuse

### What makes a good fixture

1. **Self-contained** — no external dependencies, no CDN links
2. **Minimal** — test one thing per fixture (adversarial) or one page type (pages)
3. **Deterministic** — same output every run (avoid `Math.random()` in assertions)
4. **Documented** — comment blocks explaining what lad should extract
