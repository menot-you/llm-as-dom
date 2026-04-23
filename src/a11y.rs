//! Accessibility tree extraction via JS injection.
//!
//! Falls back from CDP Accessibility API to direct JS DOM walking
//! because chromiumoxide's CDP bindings have serde issues with some AX nodes.

use serde::Deserialize;

use crate::engine::PageHandle;
use crate::semantic::{Element, ElementHint, ElementKind, FormMeta, PageState, SemanticView};

/// Extract page structure via JS and compress to a [`SemanticView`].
///
/// Stamps each interactive element with a `data-lad-id` attribute so that
/// subsequent actions can target elements by stable numeric ID.
/// Also tracks which `<form>` each element belongs to for scoping.
///
/// Defaults to dropping hidden elements at the JS-DOM walker layer. Call
/// [`extract_semantic_view_with_options`] with `include_hidden=true` to bypass
/// the gate (audit / debugging).
pub async fn extract_semantic_view(page: &dyn PageHandle) -> Result<SemanticView, crate::Error> {
    extract_semantic_view_with_options(page, false, false).await
}

/// Extract page structure with an explicit `include_hidden` flag.
///
/// Wave 5 (Pain #10 fix): threads the flag into the JS DOM walker so Layer 1
/// no longer drops hidden elements before the Rust-side filter runs. When
/// `include_hidden=true`, the JS walker keeps every element and Rust-side
/// `retain_visible_elements` is skipped by the tool entrypoints — the caller
/// then sees elements with `is_visible: Some(false)` instead of losing them
/// entirely. Default path (`false`) is byte-for-byte equivalent to the
/// previous behavior.
pub async fn extract_semantic_view_with_options(
    page: &dyn PageHandle,
    include_hidden: bool,
    include_cards: bool,
) -> Result<SemanticView, crate::Error> {
    let url = page.url().await.unwrap_or_else(|_| "unknown".into());
    let title = page.title().await.unwrap_or_else(|_| String::new());

    // Rust `bool::to_string()` emits the literal JS tokens `true`/`false`.
    // Kept as an explicit match for clarity; we splice this into the raw JS
    // string below by replacing a sentinel token rather than going through
    // `format!` (that would force us to escape every `{`/`}` in the walker).
    let include_hidden_js = if include_hidden { "true" } else { "false" };
    let include_cards_js = if include_cards { "true" } else { "false" };
    let js_template = r#"
        (() => {
            // CHAOS-C3: Override window.close() to prevent hostile pages from
            // killing the browser tab/handle during extraction or navigation.
            try { window.close = function(){}; } catch(_) {}

            // Wave 5 (Pain #10): honor the include_hidden flag. When true,
            // Layer 1's isVisible() gate becomes a pass-through so hidden
            // elements reach the Rust side (where is_visible=false flags
            // them for downstream filtering). Substituted at runtime.
            const includeHidden = __LAD_INCLUDE_HIDDEN__;
            // BUG-4 + FR-1 follow-up: gate the structural cards walker so
            // opt-out callers don't pay the DOM traversal + querySelectorAll
            // cost. Default path (includeCards=false) skips the whole try
            // block — zero walker work when the feature is off.
            const includeCards = __LAD_INCLUDE_CARDS__;

            const MAX_ELEMENTS = 300;
            // DX-CE3 (bug 3): include contenteditable roots, [role="textbox"],
            // and [aria-multiline="true"]. These are how Twitter/Discord/
            // Slack/Notion/Gmail/LinkedIn/Substack/Medium render their
            // text inputs (Draft.js, Lexical, ProseMirror, Slate, etc.).
            const selectors = 'a[href], button, input, textarea, select, [role="button"], [role="link"], [role="checkbox"], [role="radio"], [role="tab"], [role="menuitem"], [onclick], [contenteditable="true"], [contenteditable=""], [role="textbox"], [aria-multiline="true"]';
            const rawElements = [];
            let id = 0;

            // DX-CE3 (bug 3): is this element a rich-text editor target
            // (contenteditable root, [role="textbox"], etc.)?
            function isEditorTarget(el) {
                const ce = el.getAttribute('contenteditable');
                if (ce === 'true' || ce === '') return true;
                if (el.isContentEditable === true) return true;
                if (el.getAttribute('role') === 'textbox') return true;
                if (el.getAttribute('aria-multiline') === 'true') return true;
                return false;
            }

            // ── Shadow DOM + light DOM recursive query ─────────────────
            // CHAOS-03: maxDepth=5 prevents unbounded recursion.
            function deepQueryAll(root, sel, depth) {
                if (depth === undefined) depth = 0;
                if (depth > 5) return [];
                const results = [];
                try { results.push(...root.querySelectorAll(sel)); } catch(_) {}
                // Walk all elements looking for shadow roots
                const allEls = root.querySelectorAll('*');
                for (const el of allEls) {
                    if (el.shadowRoot) {
                        try { results.push(...deepQueryAll(el.shadowRoot, sel, depth + 1)); } catch(_) {}
                    }
                }
                return results;
            }

            // DX-FIX + DX-MZ4 (bug 4): Detect active modal/dialog and scope
            // extraction to it. This prevents extracting background elements
            // when a modal is open, fixing fill_form wrong-match, click-
            // behind-modal, and modal scroll issues.
            //
            // When multiple candidate dialogs are present (e.g. Twitter
            // renders a backdrop dialog + a keyboard-shortcut dialog),
            // pick the topmost by (1) highest computed z-index and then
            // (2) last in source order as the tiebreaker. This matches
            // the visual "top" of the modal stack and avoids the
            // historical bug where x.com/compose/post's element [0]
            // was a keyboard-shortcuts link masquerading as a close button.
            const dialogCandidates = Array.from(document.querySelectorAll(
                'dialog[open], [role="dialog"][aria-modal="true"], [role="dialog"]:not([aria-hidden="true"])'
            ));
            function dialogZIndex(el) {
                // Walk up to the nearest z-index'd ancestor (dialogs often
                // inherit their stacking context from a parent).
                let cur = el;
                while (cur && cur.nodeType === 1) {
                    const z = parseInt(window.getComputedStyle(cur).zIndex, 10);
                    if (!Number.isNaN(z)) return z;
                    cur = cur.parentElement;
                }
                return 0;
            }
            let activeDialog = null;
            if (dialogCandidates.length > 0) {
                let bestZ = -Infinity;
                for (const cand of dialogCandidates) {
                    // Skip dialogs that are themselves hidden or inert — they
                    // cannot be the active modal.
                    const cs = window.getComputedStyle(cand);
                    if (cs.display === 'none' || cs.visibility === 'hidden') continue;
                    if (cand.closest('[inert]')) continue;
                    const z = dialogZIndex(cand);
                    if (z >= bestZ) {
                        bestZ = z;
                        activeDialog = cand; // ties → last in source order
                    }
                }
            }
            const extractionRoot = activeDialog || document;

            // If modal detected, scroll it to show all content before extraction.
            if (activeDialog) {
                const scrollable = activeDialog.querySelector('[style*="overflow"], [class*="scroll"]')
                    || activeDialog;
                if (scrollable.scrollHeight > scrollable.clientHeight) {
                    // Scroll to bottom then back to top to force lazy content to load.
                    scrollable.scrollTop = scrollable.scrollHeight;
                    scrollable.scrollTop = 0;
                }
            }

            const els = deepQueryAll(extractionRoot, selectors);

            // Build a form index: map each <form> to a sequential number
            const allForms = deepQueryAll(extractionRoot, 'form');
            const formMap = new Map();
            allForms.forEach((f, i) => formMap.set(f, i));

            // ── Visibility helpers ──────────────────────────────────────
            function hasZeroAncestorOpacity(el, maxDepth) {
                let cur = el.parentElement;
                for (let d = 0; d < maxDepth && cur; d++, cur = cur.parentElement) {
                    if (parseFloat(window.getComputedStyle(cur).opacity) === 0) return true;
                }
                return false;
            }

            function isHoneypot(el) {
                const name = (el.getAttribute('name') || '').toLowerCase();
                const ac = (el.getAttribute('autocomplete') || '').toLowerCase();
                const ti = el.getAttribute('tabindex');
                const style = window.getComputedStyle(el);
                const invisible = style.display === 'none' || style.visibility === 'hidden'
                    || parseFloat(style.opacity) === 0;
                // DX-14 FIX: Only treat "website"/"url"/"honeypot" as honeypot if INVISIBLE.
                // Visible fields named "website" are legitimate (e.g. Twitter Edit Profile).
                if ((name === 'website' || name === 'url' || name === 'honeypot') && invisible) return true;
                if (name === 'honeypot') return true; // "honeypot" name is always suspicious.
                if (ac === 'off' && invisible) return true;
                if (ti === '-1' && invisible) return true;
                return false;
            }

            function isVisible(el) {
                const style = window.getComputedStyle(el);
                if (style.display === 'none' || style.visibility === 'hidden') return false;
                // DX-MZ4 (bug 4): visibility:collapse hides table rows/cells
                // identically to display:none. Treat it as invisible too.
                if (style.visibility === 'collapse') return false;
                if (parseFloat(style.opacity) === 0) return false;
                // DX-MZ4: pointer-events:none means the element cannot be
                // clicked, so collecting it as a clickable would produce
                // a phantom target whose click goes to whatever is behind.
                if (style.pointerEvents === 'none') return false;
                // DX-MZ4: [inert] attribute disables an entire subtree —
                // background content behind an open aria-modal dialog is
                // often marked inert. element.closest('[inert]') walks the
                // ancestor chain for us.
                if (el.closest('[inert]')) return false;
                const rect = el.getBoundingClientRect();
                if (rect.width === 0 && rect.height === 0) return false;
                // DX-FIX: When inside a modal, check against modal bounds, not window.
                // For full-page extraction, check against window viewport.
                if (!activeDialog) {
                    if (rect.right < 0 || rect.bottom < 0
                        || rect.left > window.innerWidth
                        || rect.top > window.innerHeight) return false;
                }
                // Inside a modal: skip viewport clipping — extract ALL elements
                // in the dialog regardless of scroll position. This fixes the
                // "fields below modal scroll" blind spot.
                if (hasZeroAncestorOpacity(el, 3)) return false;
                if (isHoneypot(el)) return false;
                return true;
            }

            // ── Collect visible elements from a list ───────────────────
            function collectElements(elList, frameIdx) {
                for (const el of elList) {
                    // Wave 5 (Pain #10): honor includeHidden. When the caller
                    // explicitly asks for hidden elements we still compute
                    // is_visible below (Layer 2 via isVisibleStrict) so Rust
                    // can tag them without dropping them here.
                    if (!includeHidden && !isVisible(el)) continue;

                    const tag = el.tagName.toLowerCase();
                    const editor = isEditorTarget(el);
                    let kind = 'other';
                    if (tag === 'button' || el.getAttribute('role') === 'button' || (tag === 'input' && el.type === 'submit')) kind = 'button';
                    else if (tag === 'input' && el.type !== 'hidden') kind = 'input';
                    else if (tag === 'textarea') kind = 'textarea';
                    else if (tag === 'select') kind = 'select';
                    else if (tag === 'a') kind = 'link';
                    else if (el.getAttribute('role') === 'checkbox' || (tag === 'input' && el.type === 'checkbox')) kind = 'checkbox';
                    else if (el.getAttribute('role') === 'radio' || (tag === 'input' && el.type === 'radio')) kind = 'radio';
                    else if (el.getAttribute('role') === 'tab' || el.getAttribute('role') === 'menuitem') kind = 'button';
                    // DX-CE3 (bug 3): reuse 'input' kind for contenteditable /
                    // role=textbox / aria-multiline. Constraint: do NOT add a
                    // new ElementKind — reuse Input. The input_type field
                    // ("contenteditable") is the disambiguator.
                    else if (editor) kind = 'input';

                    const ariaLabel = el.getAttribute('aria-label');
                    const labelEl = el.labels?.[0];
                    const labelText = labelEl?.textContent?.trim();
                    // DX-CE3: many SPAs expose an `aria-placeholder` attribute
                    // on contenteditable roots (e.g. Twitter's "What is happening?").
                    const placeholder = el.getAttribute('placeholder') || el.getAttribute('aria-placeholder');
                    // DX-CE3: for contenteditable roots, textContent IS the
                    // current document value — using it as the label would
                    // echo whatever the user already typed. Skip textContent
                    // as a label fallback for editor targets.
                    const textContent = editor ? '' : (el.textContent?.trim()?.substring(0, 80) || '');
                    const elTitle = el.getAttribute('title');
                    const href = el.getAttribute('href') || '';
                    // DX-CE3: data-testid fallback for unlabelled editor roots.
                    const testId = editor ? (el.getAttribute('data-testid') || '') : '';
                    // FR-5 (friction-log-2026-04-22): extended fallback
                    // chain so icon-only buttons (close X, menu ≡, search ⌕)
                    // stop surfacing as `Button type=button ""`. Priority
                    // order keeps explicit labels (aria, label, placeholder)
                    // over DOM-inferred ones (svg title, data-*).
                    //   1. aria-label
                    //   2. <label> text
                    //   3. placeholder / aria-placeholder
                    //   4. textContent (skipped for editors)
                    //   5. title attribute
                    //   6. data-testid (editors only, already fallbacks)
                    //   7. SVG <title> descendant — icon-only buttons
                    //   8. aria-describedby text — resolved from referenced node
                    //   9. data-label — common on Tailwind/SaaS toolkits
                    const svgTitle = (() => {
                        try {
                            return el.querySelector('svg > title, svg > desc')?.textContent?.trim() || '';
                        } catch (_) { return ''; }
                    })();
                    const describedByText = (() => {
                        const id = el.getAttribute('aria-describedby');
                        if (!id) return '';
                        try {
                            const refs = id.split(/\s+/).filter(Boolean);
                            const texts = refs.map(r => document.getElementById(r)?.textContent?.trim() || '');
                            return texts.filter(Boolean).join(' ').substring(0, 80);
                        } catch (_) { return ''; }
                    })();
                    const dataLabel = el.getAttribute('data-label') || el.getAttribute('data-name') || '';
                    let label = (ariaLabel || labelText || placeholder || textContent || elTitle || testId || svgTitle || describedByText || dataLabel || '').replace(/\s+/g, ' ').trim();
                    if (!label && kind === 'link' && href) {
                        label = href.split('/').filter(Boolean).pop() || '';
                    }
                    if (!label && editor) {
                        label = 'text editor';
                    }
                    // FR-5: explicit marker for still-unlabeled interactive
                    // elements so agents see `[unlabeled:button]` instead
                    // of a silent empty string. Skip links: link labels
                    // already fall back to the href tail above.
                    //
                    // Bracket form (`[unlabeled:...]`) chosen over chevron
                    // (`<...>`) so downstream markdown renderers and HTML
                    // sanitizers don't strip or escape the sentinel.
                    if (!label && (kind === 'button' || kind === 'input')) {
                        label = `[unlabeled:${kind}]`;
                    }

                    const closestForm = el.closest('form');
                    const formIndex = closestForm ? (formMap.get(closestForm) ?? null) : null;

                    // ── Relevance score (used when cap triggers) ────────
                    let score = 0;
                    if (closestForm) score += 3;
                    if (kind === 'input' || kind === 'textarea' || kind === 'select'
                        || kind === 'checkbox' || kind === 'radio') score += 5;
                    if (kind === 'button') score += 4;
                    if (tag === 'input' && el.type === 'submit') score += 2;
                    if (ariaLabel) score += 2;
                    if (kind === 'link') {
                        if (href === '#' || href.startsWith('#')) score -= 2;
                        const lcHref = href.toLowerCase();
                        if (lcHref.includes('facebook.com') || lcHref.includes('twitter.com')
                            || lcHref.includes('instagram.com') || lcHref.includes('linkedin.com')
                            || lcHref.includes('youtube.com') || lcHref.includes('tiktok.com')) score -= 3;
                    }

                    // ── @lad/hints detection ─────────────────────────
                    let hintType = null;
                    let hintValue = null;
                    const ladHint = el.getAttribute('data-lad');
                    if (ladHint) {
                        const colonIdx = ladHint.indexOf(':');
                        if (colonIdx > 0) {
                            hintType = ladHint.substring(0, colonIdx);
                            hintValue = ladHint.substring(colonIdx + 1);
                        }
                    }

                    // DX-W2-2: Extract checked state for checkbox/radio.
                    // Wave 5c hotfix (Pain #13 regression): the if/else chain
                    // above lands <input type=radio|checkbox> in kind='input'
                    // (line 227 matches first), so gating on `kind` here
                    // always yielded null for the live cases. Read `el.type`
                    // directly — it's the canonical source of truth for the
                    // DOM element, regardless of our semantic kind mapping.
                    const checked = (el.type === 'checkbox' || el.type === 'radio') ? !!el.checked : null;

                    // DX-W2-2: Extract option labels for <select> elements (top 10).
                    let options = null;
                    if (kind === 'select' && el.options) {
                        options = Array.from(el.options).slice(0, 10).map(o => o.textContent.trim());
                    }

                    // DX-CE3 (bug 3): editor targets report their current
                    // value via innerText (capped) and a synthetic
                    // input_type of "contenteditable" so the type handler
                    // can branch on it.
                    let editorValue = null;
                    let editorType = null;
                    if (editor) {
                        try {
                            const text = (el.innerText || el.textContent || '').trim();
                            if (text) editorValue = text.substring(0, 200);
                        } catch (_) {}
                        editorType = 'contenteditable';
                    }

                    // Wave 1 — strict visibility flag. Closes a class of
                    // prompt injection (Brave disclosure, Oct 2025) where
                    // adversarial pages smuggle instructions into nodes
                    // marked aria-hidden or [hidden] that slip past the
                    // existing isVisible() filter above. Defense in depth:
                    // isVisible() drops most hidden nodes; this flag lets
                    // Rust drop the rest by default.
                    let isVisibleStrict = true;
                    try {
                        const cs2 = window.getComputedStyle(el);
                        const rect2 = el.getBoundingClientRect();
                        isVisibleStrict =
                            cs2.display !== 'none' &&
                            cs2.visibility !== 'hidden' &&
                            parseFloat(cs2.opacity) > 0 &&
                            !el.hidden &&
                            el.getAttribute('aria-hidden') !== 'true' &&
                            rect2.width > 0 &&
                            rect2.height > 0 &&
                            rect2.bottom > 0 &&
                            rect2.right > 0;
                    } catch (_) {
                        isVisibleStrict = true;
                    }

                    rawElements.push({
                        el, kind, label: label.substring(0, 80),
                        name: el.getAttribute('name') || null,
                        value: editorValue || el.value || null,
                        placeholder: placeholder || null,
                        href: href || null,
                        input_type: editorType || el.getAttribute('type') || (tag === 'textarea' ? 'textarea' : null),
                        disabled: el.disabled || false,
                        form_index: formIndex,
                        hint_type: hintType,
                        hint_value: hintValue,
                        frame_index: frameIdx,
                        checked: checked,
                        options: options,
                        is_visible: isVisibleStrict,
                        score,
                        isActionable: kind !== 'link' && kind !== 'other',
                    });
                }
            }

            // Collect from main document (including shadow DOM)
            collectElements(els, null);

            // ── iframe same-origin traversal ───────────────────────────
            const iframes = document.querySelectorAll('iframe');
            for (let fi = 0; fi < iframes.length; fi++) {
                try {
                    const iframeDoc = iframes[fi].contentDocument;
                    if (!iframeDoc) continue;
                    // Same-origin iframe accessible — collect elements
                    const iframeEls = deepQueryAll(iframeDoc, selectors);
                    collectElements(iframeEls, fi);
                    // Also collect forms from iframe
                    const iframeForms = deepQueryAll(iframeDoc, 'form');
                    iframeForms.forEach(f => {
                        if (!formMap.has(f)) {
                            const idx = formMap.size;
                            formMap.set(f, idx);
                        }
                    });
                } catch(_) {
                    // Cross-origin iframe — silently skip
                }
            }

            // ── Element cap: keep top MAX_ELEMENTS by score ─────────────
            const totalCount = rawElements.length;
            let kept = rawElements;
            let elementCap = null;
            if (totalCount > MAX_ELEMENTS) {
                const actionable = rawElements.filter(e => e.isActionable);
                const rest = rawElements.filter(e => !e.isActionable);
                rest.sort((a, b) => b.score - a.score);
                const slotsLeft = Math.max(0, MAX_ELEMENTS - actionable.length);
                kept = actionable.concat(rest.slice(0, slotsLeft));
                elementCap = kept.length + '/' + totalCount;
            }

            // ── Assign stable IDs and build output ──────────────────────
            const elements = [];
            for (const raw of kept) {
                raw.el.setAttribute('data-lad-id', String(id));
                elements.push({
                    id: id,
                    kind: raw.kind,
                    label: raw.label,
                    name: raw.name,
                    value: raw.value,
                    placeholder: raw.placeholder,
                    href: raw.href,
                    input_type: raw.input_type,
                    disabled: raw.disabled,
                    form_index: raw.form_index,
                    hint_type: raw.hint_type,
                    hint_value: raw.hint_value,
                    frame_index: raw.frame_index,
                    checked: raw.checked,
                    options: raw.options,
                    is_visible: raw.is_visible,
                });
                id++;
            }

            const textNodes = deepQueryAll(document, 'h1, h2, h3, h4, p, label, legend, [role="heading"]');
            let visibleText = '';
            // Issue #36 — collect individual blocks for downstream scoring.
            // Per-block capped at 240 chars, list capped at 200 entries.
            // `visibleText` still emitted for back-compat (lad_snapshot,
            // heuristics, wait). Scoring happens only in `tool_lad_extract`.
            const textBlocks = [];
            const BLOCK_CAP = 240;
            const LIST_CAP = 200;
            const pushBlock = (s) => {
                if (textBlocks.length < LIST_CAP) {
                    textBlocks.push(s.length > BLOCK_CAP ? s.substring(0, BLOCK_CAP) : s);
                }
            };

            // ── FR-3 (friction-log-2026-04-22): visible_text dedup ─────
            // The same long sentence was repeating 2-3× in `visibleText`
            // whenever sticky `<header>` / `<footer>` / `<aside>` /
            // `<nav>` text was reachable both via the heading/paragraph
            // walk AND via the span/td fallback. Dedupe long sentences
            // (≥ 5 words) that live inside these chrome containers only.
            // Short strings ("Page 1 of 10") and content sections
            // (`<main>`, `<article>`) pass unchanged so feed entries
            // (e.g. 5 GitHub repo rows with "No description, website, or
            // topics provided.") and legitimate pagination duplicates
            // stay visible to the agent.
            const CHROME_TAGS = new Set(['HEADER', 'FOOTER', 'ASIDE', 'NAV']);
            const chromeSeen = new Set();
            function inChromeSection(node) {
                let cur = node;
                while (cur && cur !== document.body && cur !== document.documentElement) {
                    const tag = cur.tagName;
                    if (CHROME_TAGS.has(tag)) return true;
                    // <main>/<article> short-circuits — the node is
                    // content, NOT chrome, so it must NOT be deduped.
                    if (tag === 'MAIN' || tag === 'ARTICLE') return false;
                    cur = cur.parentElement;
                }
                return false;
            }
            function normalizeForDedup(text) {
                return text.replace(/\s+/g, ' ').trim();
            }
            function shouldEmitForDedup(node, text) {
                const norm = normalizeForDedup(text);
                if (!norm) return true;
                // Short strings always pass — pagination labels, ship
                // warnings, small captions legitimately repeat.
                if (norm.split(' ').length < 5) return true;
                if (!inChromeSection(node)) return true;
                if (chromeSeen.has(norm)) return false;
                chromeSeen.add(norm);
                return true;
            }

            for (const node of textNodes) {
                const text = node.textContent?.trim();
                if (text && shouldEmitForDedup(node, text)) {
                    pushBlock(text);
                    if (visibleText.length < 500) {
                        if (visibleText) visibleText += ' ';
                        visibleText += text.substring(0, 100);
                    }
                }
            }
            // Fallback: collect substantial text from td, span, a, pre, code,
            // textarea when headings/paragraphs yielded little.
            // Wave 5b (Pain #14): added pre/code/textarea so JSON response
            // pages rendered as <pre>{...}</pre> (e.g. httpbin.org/post)
            // don't emit empty visible_text.
            // Issue #36 — always feed text_blocks from this pool too, so
            // content-heavy pages (GitHub, HN) have scoring material even
            // when headings alone already filled the 500-char visibleText.
            const extraNodes = deepQueryAll(document, 'td, span, a, pre, code, textarea');
            for (const node of extraNodes) {
                const text = node.textContent?.trim();
                if (text && text.length > 20 && shouldEmitForDedup(node, text)) {
                    pushBlock(text);
                    if (visibleText.length < 100 && visibleText.length < 500) {
                        if (visibleText) visibleText += ' ';
                        visibleText += text.substring(0, 100);
                    }
                }
                if (textBlocks.length >= LIST_CAP) break;
            }

            // Last-resort fallback: if still near-empty, fall back to
            // document.body.innerText so pages with unusual DOM shapes
            // (SPAs rendering everything into custom elements, etc.) still
            // report *something*. Same 500-char total cap.
            // Wave 5b (Pain #14).
            if (visibleText.length < 50) {
                try {
                    const bodyText = document.body?.innerText?.trim();
                    if (bodyText) {
                        visibleText = bodyText.substring(0, 500);
                        // Issue #36 — when only innerText exists, split on
                        // blank lines to seed text_blocks so the scorer has
                        // something to work with on pathological SPAs.
                        if (textBlocks.length === 0) {
                            const parts = bodyText.split(/\n{2,}|\r\n{2,}/);
                            for (const part of parts) {
                                const t = part.trim();
                                if (t.length > 20) pushBlock(t);
                                if (textBlocks.length >= LIST_CAP) break;
                            }
                        }
                    }
                } catch (_) {}
            }

            // ── Form metadata ───────────────────────────────────────────
            const forms = Array.from(allForms).map((f, i) => ({
                index: i,
                action: f.getAttribute('action') || null,
                method: (f.getAttribute('method') || 'GET').toUpperCase(),
                id: f.id || null,
                name: f.getAttribute('name') || null,
            }));

            // ── FR-4: article/repo structural signal ────────────────────
            // True when the DOM advertises itself as an article or a
            // code-hosting repository. Branches:
            //   1. Top-level structural element (`body > article`,
            //      `body > main[role=main]`) carrying substantial text.
            //      Scoping to direct body children avoids false positives
            //      on SPA dashboards that stamp `<article>` per card or
            //      list row.
            //   2. Schema.org `itemtype` anywhere in the document — these
            //      tags are intentionally placed and rarely abused.
            //   3. `og:type` meta in {article, repository, object, blog,
            //      news}. Non-canonical values (`blog`, `news`,
            //      `repository`, `object`) are accepted intentionally:
            //      they appear in the wild on GitHub/blog platforms even
            //      though strict OG spec only lists `article`.
            // Used by Rust-side classify_page() as Branch A, winning over
            // the "many links → listing" fallback for content-heavy pages
            // like GitHub repo roots that carry a README + 40+ nav links.
            //
            // The text-content gate (>= 200 chars) on top-level <article>
            // and <main> filters out wrapper-only usage (e.g. an empty
            // <main> wrapping a SPA route shell). 200 chars ≈ a short
            // paragraph; cards/rows alone rarely cross that bar.
            const ARTICLE_MIN_TEXT = 200;
            let articleSignal = false;
            try {
                const topLevel = document.querySelector('body > article, body > main[role="main"]');
                if (topLevel && (topLevel.textContent || '').trim().length >= ARTICLE_MIN_TEXT) {
                    articleSignal = true;
                } else if (document.querySelector('[itemtype*="SoftwareSourceCode"], [itemtype*="Article"], [itemtype*="BlogPosting"], [itemtype*="NewsArticle"]')) {
                    articleSignal = true;
                } else {
                    const og = document.querySelector('meta[property="og:type"]')?.getAttribute('content');
                    if (og && /^(article|repository|object|blog|news)$/i.test(og.trim())) {
                        articleSignal = true;
                    }
                }
            } catch (_) { articleSignal = false; }

            // ── BUG-4 + FR-1 (friction-log-2026-04-22): cards detector ──
            // Find containers with >= 3 repeated structural siblings
            // (children sharing a dominant tagName for >= 80% of the
            // first 20 positions). Matches HN's `<table class="itemlist">`
            // with `<tr class="athing">` rows, Reddit post feeds, generic
            // `<ol>/<ul>` listings — no hostname baked in.
            //
            // Per sibling we emit a Card with:
            // - title: first heading OR first external/anchor link text.
            // - metadata: regex matches over sibling text for points /
            //   comments / author / age. Non-matching siblings still
            //   become cards with empty metadata.
            // - child_element_ids: descendants carrying `data-lad-id`.
            //
            // Cards are informational grouping. Clicking still routes
            // through `elements` — no new tool surface.
            const cards = [];
            if (includeCards) try {
                const CARD_LIST_MIN_CHILDREN = 3;
                const CARD_SAMPLE_DEPTH = 20;
                const CARD_LIST_CAP = 50;
                const META_REGEXES = [
                    [/(\d+)\s+(point|points|vote|votes|upvote|upvotes)\b/i, 'points'],
                    [/(\d+)\s+(comment|comments|reply|replies|response|responses)\b/i, 'comments'],
                    [/(\d+)\s+(view|views|read|reads)\b/i, 'views'],
                    [/\bby\s+([A-Za-z0-9_\-\.]{2,32})\b/, 'author'],
                    [/\b(\d+\s+(?:second|minute|hour|day|week|month|year)s?\s+ago)\b/i, 'age'],
                ];
                const candidates = deepQueryAll(document, 'ol, ul, tbody, section, main, article, div[class*="feed"], div[class*="list"]');
                let cardId = 0;
                outer: for (const container of candidates) {
                    if (cards.length >= CARD_LIST_CAP) break;
                    const children = Array.from(container.children || []);
                    if (children.length < CARD_LIST_MIN_CHILDREN) continue;
                    const sample = children.slice(0, CARD_SAMPLE_DEPTH);
                    const tagCounts = new Map();
                    for (const c of sample) tagCounts.set(c.tagName, (tagCounts.get(c.tagName) || 0) + 1);
                    let domTag = null; let domCount = 0;
                    for (const [tag, count] of tagCounts.entries()) {
                        if (count > domCount) { domTag = tag; domCount = count; }
                    }
                    if (!domTag) continue;
                    if (domCount / sample.length < 0.8) continue;
                    for (const sib of children) {
                        if (sib.tagName !== domTag) continue;
                        if (cards.length >= CARD_LIST_CAP) break outer;
                        const titleEl = sib.querySelector('h1, h2, h3, h4, h5, h6')
                            || sib.querySelector('a[href^="http"], a[href^="/"]');
                        const title = (titleEl?.textContent?.trim()?.substring(0, 160)) || '';
                        if (!title) continue;
                        const sibText = (sib.textContent || '').replace(/\s+/g, ' ').trim();
                        const metadata = [];
                        const metaSeen = new Set();
                        for (const [re, key] of META_REGEXES) {
                            const m = sibText.match(re);
                            if (m && !metaSeen.has(key)) {
                                metaSeen.add(key);
                                metadata.push([key, m[1]]);
                            }
                        }
                        const childIds = [];
                        sib.querySelectorAll('[data-lad-id]').forEach(el => {
                            const id = parseInt(el.getAttribute('data-lad-id'), 10);
                            if (!Number.isNaN(id)) childIds.push(id);
                        });
                        cards.push({
                            id: 'c' + cardId++,
                            title,
                            metadata,
                            child_element_ids: childIds,
                        });
                    }
                }
            } catch (_) { /* fail closed — no cards is fine */ }

            return { elements, visibleText, textBlocks, formCount: allForms.length, elementCap, forms, articleSignal, cards };
        })()
    "#;

    // Splice the Rust-side flags into the JS walker. Safe because
    // `bool::to_string()` only yields "true"/"false" — no escaping required,
    // no XSS surface (this runs inside our own controlled page).
    let js = js_template
        .replace("__LAD_INCLUDE_HIDDEN__", include_hidden_js)
        .replace("__LAD_INCLUDE_CARDS__", include_cards_js);

    let mut extraction: JsExtraction = crate::engine::eval_js_into(page, &js).await?;
    let mut shell_markers = crate::cloaking::probe_shell_markers(page).await;

    // DX-CL2 (bug 2): Twitter/X and other React SPAs render a shell-only
    // HTML response — interactive count is legitimately 0 for a few hundred
    // milliseconds during hydration. If we see "zero interactive elements +
    // SPA shell markers", wait 1.5s and retry once before letting the
    // cloaking detector touch the view.
    let interactive_raw = count_interactive(&extraction.elements);
    if interactive_raw == 0 && shell_markers.looks_like_spa_shell() {
        tracing::debug!(
            markers = ?shell_markers,
            "zero interactive elements on SPA shell — retrying extraction after 1500ms"
        );
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        extraction = crate::engine::eval_js_into(page, &js).await?;
        shell_markers = crate::cloaking::probe_shell_markers(page).await;
    }

    tracing::info!(
        elements = extraction.elements.len(),
        forms = extraction.form_count,
        visible_text_len = extraction.visible_text.len(),
        "DOM extracted via JS"
    );

    let elements: Vec<Element> = extraction
        .elements
        .into_iter()
        .map(|e| {
            let hint = match (e.hint_type, e.hint_value) {
                (Some(ht), Some(hv)) => Some(ElementHint {
                    hint_type: ht,
                    value: hv,
                }),
                _ => None,
            };
            Element {
                id: e.id,
                kind: parse_kind(&e.kind),
                label: e.label,
                name: e.name,
                value: e.value,
                placeholder: e.placeholder,
                href: e.href,
                input_type: e.input_type,
                disabled: e.disabled,
                form_index: e.form_index,
                context: None,
                hint,
                checked: e.checked,
                options: e.options,
                frame_index: e.frame_index,
                // Wave 1: Map the JS-emitted `is_visible` flag through. `None`
                // means the extractor didn't compute one → treat as visible.
                is_visible: e.is_visible,
            }
        })
        .collect();

    let page_hint = classify_page(&title, &url, &elements, extraction.article_signal);

    let forms: Vec<FormMeta> = extraction
        .forms
        .into_iter()
        .map(|f| FormMeta {
            index: f.index,
            action: f.action,
            method: f.method,
            id: f.id,
            name: f.name,
        })
        .collect();

    // BUG-4 + FR-1: the JS walker only runs when `include_cards=true` was
    // passed into this function (see `includeCards` gate in the JS
    // template). On the default path the cards array is always empty and
    // callers pay zero walker cost. Tool layer still strips
    // `view.cards = None` belt-and-suspenders when the caller didn't opt in.
    let raw_cards: Vec<crate::semantic::Card> =
        extraction.cards.into_iter().map(Into::into).collect();

    let mut view = SemanticView {
        url,
        title,
        page_hint,
        elements,
        forms,
        visible_text: extraction.visible_text,
        text_blocks: extraction.text_blocks,
        state: PageState::Ready,
        element_cap: extraction.element_cap,
        blocked_reason: None,
        session_context: None,
        cards: if raw_cards.is_empty() {
            None
        } else {
            Some(raw_cards)
        },
    };

    // ── Security: strip steganographic characters + mask passwords ──
    sanitize_view(&mut view);

    // Detect bot-challenge / CAPTCHA pages after extraction.
    // DX-CL2: pass SPA shell markers so the CSS cloaking heuristic can
    // suppress itself on mid-hydration Next.js / React pages.
    if let Some(reason) = detect_bot_challenge_with_markers(&view, &shell_markers) {
        tracing::warn!(reason = %reason, "bot challenge detected");
        view.state = PageState::Blocked(reason.clone());
        view.blocked_reason = Some(reason);
    }

    Ok(view)
}

/// Raw JS extraction result (mirrors the JS object shape).
#[derive(Deserialize)]
struct JsExtraction {
    elements: Vec<JsElement>,
    #[serde(rename = "visibleText")]
    visible_text: String,
    /// Issue #36 — individual text blocks (headings/paragraphs/td/span/a/
    /// pre/code) pre-concatenation, per-block capped at 240 chars, list
    /// capped at 200. Raw material for `tool_lad_extract`'s `what` scorer.
    /// Defaults to empty for back-compat with any pinned legacy JS.
    #[serde(rename = "textBlocks", default)]
    text_blocks: Vec<String>,
    #[serde(rename = "formCount")]
    form_count: u32,
    /// `"50/316"` when elements were capped, `null` otherwise.
    #[serde(rename = "elementCap")]
    element_cap: Option<String>,
    /// Form metadata collected from each `<form>` on the page.
    #[serde(default)]
    forms: Vec<JsFormMeta>,
    /// FR-4: true when the DOM advertises itself as an article or a
    /// code-hosting repository via `<article>`, `<main role=main>`,
    /// Schema.org `itemtype`, or `og:type` meta. Drives the
    /// `article/repo page` classification in [`classify_page`].
    #[serde(default, rename = "articleSignal")]
    article_signal: bool,
    /// BUG-4 + FR-1: structural cards detected by the walker. Always
    /// populated by the JS walker; Rust-side gating on `include_cards`
    /// happens at the tool layer before the `SemanticView` is
    /// serialized to the caller.
    #[serde(default)]
    cards: Vec<JsCard>,
}

/// Raw card payload from the JS walker. Mirrors `semantic::Card`.
#[derive(Deserialize, Clone)]
struct JsCard {
    id: String,
    title: String,
    #[serde(default)]
    metadata: Vec<(String, String)>,
    #[serde(rename = "child_element_ids", default)]
    child_element_ids: Vec<u32>,
}

impl From<JsCard> for crate::semantic::Card {
    fn from(c: JsCard) -> Self {
        crate::semantic::Card {
            id: c.id,
            title: c.title,
            metadata: c.metadata,
            child_element_ids: c.child_element_ids,
        }
    }
}

/// Form metadata as returned by the JS extractor.
#[derive(Deserialize)]
struct JsFormMeta {
    index: u32,
    action: Option<String>,
    method: String,
    /// DX-16: HN returns `"id": {}` (empty object) instead of a string.
    /// Use Value to accept any type, then convert to Option<String>.
    #[serde(default, deserialize_with = "deserialize_string_or_null")]
    id: Option<String>,
    name: Option<String>,
}

/// Accept string, null, or any other type (coerce non-strings to None).
fn deserialize_string_or_null<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: serde_json::Value = serde::Deserialize::deserialize(deserializer)?;
    match value {
        serde_json::Value::String(s) if !s.is_empty() => Ok(Some(s)),
        _ => Ok(None), // null, empty string, object, array → None
    }
}

/// A single element as returned by the JS extractor.
#[derive(Deserialize)]
struct JsElement {
    id: u32,
    kind: String,
    label: String,
    name: Option<String>,
    value: Option<String>,
    placeholder: Option<String>,
    href: Option<String>,
    input_type: Option<String>,
    #[serde(default)]
    disabled: bool,
    form_index: Option<u32>,
    /// `@lad/hints` hint type (e.g. `"field"`, `"form"`, `"action"`).
    hint_type: Option<String>,
    /// `@lad/hints` hint value (e.g. `"email"`, `"login"`, `"submit"`).
    hint_value: Option<String>,
    /// Index of the iframe this element belongs to (`null` if in the main document).
    #[serde(default)]
    frame_index: Option<u32>,
    /// Whether checkbox/radio is checked (`null` for other element types).
    #[serde(default)]
    checked: Option<bool>,
    /// Visible option labels for `<select>` elements (top 10).
    #[serde(default)]
    options: Option<Vec<String>>,
    /// Wave 1: visibility flag emitted by the JS accessibility walker.
    /// `Some(false)` for elements flagged hidden (aria-hidden, display:none,
    /// opacity:0, zero bounds). `None` when the extractor didn't compute one
    /// (legacy fixtures / old JS) — treated as visible by the Rust side.
    #[serde(default)]
    is_visible: Option<bool>,
}

/// Count elements whose kind is `button | input | textarea | select` — the
/// set used by cloaking / challenge heuristics as "interactive".
///
/// Operates on the raw JS extraction to avoid re-running the ElementKind
/// classifier before the Rust-side `Element`s have been built.
fn count_interactive(elements: &[JsElement]) -> usize {
    elements
        .iter()
        .filter(|e| matches!(e.kind.as_str(), "button" | "input" | "textarea" | "select"))
        .count()
}

/// Map a JS kind string to the strongly-typed [`ElementKind`].
fn parse_kind(s: &str) -> ElementKind {
    match s {
        "button" => ElementKind::Button,
        "input" => ElementKind::Input,
        "link" => ElementKind::Link,
        "select" => ElementKind::Select,
        "textarea" => ElementKind::Textarea,
        "checkbox" => ElementKind::Checkbox,
        "radio" => ElementKind::Radio,
        _ => ElementKind::Other,
    }
}

/// FR-4 (friction-log-2026-04-22): hosts where a two-segment path
/// (`/owner/repo`) reliably denotes a code-hosting repository root.
/// URL-based Branch B of the `article/repo page` classifier runs ONLY
/// for these hosts, so `news.ycombinator.com/news/2` does not get
/// misclassified as a repo just because it matches the generic pattern.
const REPO_URL_HOST_ALLOWLIST: &[&str] = &[
    "github.com",
    "gitlab.com",
    "bitbucket.org",
    "codeberg.org",
    "sr.ht",
];

/// FR-4: URL paths within an allow-listed repo host that should be
/// classified as `article/repo page`. Covers the repo root and the
/// common subpages (issues, pulls, wiki, tree, blob, commits, releases,
/// tags) that carry long prose + many ambient links.
fn url_matches_repo_pattern(host: &str, path: &str) -> bool {
    if !REPO_URL_HOST_ALLOWLIST
        .iter()
        .any(|allowed| host.eq_ignore_ascii_case(allowed))
    {
        return false;
    }
    // Trim trailing slash so `/foo/bar/` collapses to `/foo/bar`.
    let trimmed = path.trim_end_matches('/');
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    // Minimum depth for any repo URL: /owner/repo → 2 segments.
    if segments.len() < 2 {
        return false;
    }
    // Repo root: exactly two segments.
    if segments.len() == 2 {
        return true;
    }
    // Canonical subpages — matched by the third segment.
    matches!(
        segments[2],
        "issues"
            | "pulls"
            | "pull"
            | "wiki"
            | "tree"
            | "blob"
            | "commit"
            | "commits"
            | "releases"
            | "tags"
            | "discussions"
            | "actions"
    )
}

/// Classify the page type from its title, URL, and element composition.
///
/// FR-4 (friction-log-2026-04-22): the `article/repo page` variant
/// prevents content-heavy pages (GitHub repo roots, blog posts, docs)
/// from being labelled `navigation/listing page` just because they
/// happen to carry > 10 ambient links. Detection has two branches:
/// - **Branch A (DOM)** — JS walker sets `article_signal=true` when
///   the DOM has `<article>`, `<main role=main>`, a Schema.org
///   `itemtype`, or an `og:type` meta. Wins over everything below
///   `login`/`search`/`form` to keep authenticated-gate detection
///   intact.
/// - **Branch B (URL)** — for hosts in `REPO_URL_HOST_ALLOWLIST`, a
///   `/owner/repo(/issues|pulls|...)?` pathname matches even when the
///   DOM signal is missing (e.g. during the React mount race on
///   GitHub).
fn classify_page(title: &str, url: &str, elements: &[Element], article_signal: bool) -> String {
    let lower_title = title.to_lowercase();
    let lower_url = url.to_lowercase();

    let has_password = elements
        .iter()
        .any(|e| e.input_type.as_deref() == Some("password"));
    let has_inputs = elements.iter().any(|e| e.kind == ElementKind::Input);
    let has_submit = elements.iter().any(|e| {
        e.kind == ElementKind::Button
            && (e.label.to_lowercase().contains("submit")
                || e.label.to_lowercase().contains("sign")
                || e.label.to_lowercase().contains("log"))
    });

    // FR-4 Branch B: URL-based repo classification. Parse once so we
    // don't re-tokenize on every heuristic below.
    let url_says_repo = match url::Url::parse(url) {
        Ok(parsed) => parsed
            .host_str()
            .is_some_and(|host| url_matches_repo_pattern(host, parsed.path())),
        Err(_) => false,
    };

    if has_password
        || lower_title.contains("login")
        || lower_title.contains("sign in")
        || lower_url.contains("login")
    {
        "login page".into()
    } else if lower_url.contains("search") || lower_title.contains("search") {
        "search page".into()
    } else if has_inputs && has_submit {
        "form page".into()
    } else if article_signal || url_says_repo {
        // FR-4: Branch A (DOM signal) OR Branch B (URL pattern on
        // allow-listed host) wins over the listing heuristic. DOM
        // signal gets priority when both fire — that's already the
        // case since we OR them.
        "article/repo page".into()
    } else if elements
        .iter()
        .filter(|e| e.kind == ElementKind::Link)
        .count()
        > 10
    {
        "navigation/listing page".into()
    } else if has_inputs {
        "interactive page".into()
    } else {
        "content page".into()
    }
}

// ── Bot-challenge detection ────────────────────────────────────────

/// Challenge-page title keywords (Cloudflare, Akamai, generic WAF).
const CHALLENGE_TITLES: &[&str] = &[
    "just a moment",
    "attention required",
    "access denied",
    "verify you are human",
    "please wait",
    "checking your browser",
    "one more step",
    "security check",
];

/// Challenge-page body text signals.
const CHALLENGE_TEXTS: &[&str] = &[
    "checking your browser",
    "captcha",
    "security check",
    "please verify",
    "enable javascript and cookies",
    "ray id",
    "cf-browser-verification",
    "hcaptcha",
    "recaptcha",
    "challenge-platform",
    // Turnstile-specific
    "cf-turnstile",
    "turnstile",
    "confirme que é humano",
    "confirm you are human",
    "verify you are not a robot",
];

/// URL path/query patterns that indicate a challenge or verification gate.
const CHALLENGE_URL_PATTERNS: &[&str] = &["challenge", "captcha", "verify", "security_check"];

/// Title patterns that indicate an error page (404, auth wall, etc.).
const ERROR_PAGE_TITLES: &[&str] = &[
    "page not found",
    "404",
    "not found",
    "forbidden",
    "unauthorized",
];

/// Detect whether a [`SemanticView`] looks like a bot-challenge, CAPTCHA page,
/// or error/auth-wall page.
///
/// Returns `Some(reason)` when a challenge or error is detected, `None` otherwise.
///
/// Thin wrapper over [`detect_bot_challenge_with_markers`] that supplies
/// default (empty) SPA markers. Use when you don't have live DOM access —
/// e.g. in unit tests over a statically-constructed `SemanticView`.
pub fn detect_bot_challenge(view: &SemanticView) -> Option<String> {
    detect_bot_challenge_with_markers(view, &crate::cloaking::ShellMarkers::default())
}

/// Variant of [`detect_bot_challenge`] that consults SPA shell markers.
///
/// DX-CL2 (bug 2): The CSS cloaking branch now uses
/// [`crate::cloaking::is_css_cloaking`] which raises the text threshold and
/// suppresses the classification when the page is a legitimate SPA shell
/// (Next.js, React, Vue) that is still hydrating.
pub fn detect_bot_challenge_with_markers(
    view: &SemanticView,
    markers: &crate::cloaking::ShellMarkers,
) -> Option<String> {
    let lower_title = view.title.to_lowercase();
    let lower_text = view.visible_text.to_lowercase();
    let lower_url = view.url.to_lowercase();

    // 1. Title match (challenge pages)
    for kw in CHALLENGE_TITLES {
        if lower_title.contains(kw) {
            return Some(format!("title matches challenge keyword: \"{kw}\""));
        }
    }

    // 2. Error page detection (404, auth wall, access denied)
    for kw in ERROR_PAGE_TITLES {
        if lower_title.contains(kw) {
            return Some(format!("title matches error page keyword: \"{kw}\""));
        }
    }

    // 3. Visible text match
    for kw in CHALLENGE_TEXTS {
        if lower_text.contains(kw) {
            return Some(format!("page text matches challenge keyword: \"{kw}\""));
        }
    }

    // 4. URL pattern match (challenge/captcha/verify gates like Reddit's
    //    `?js_challenge=1&token=...`)
    for pattern in CHALLENGE_URL_PATTERNS {
        if lower_url.contains(pattern) {
            return Some(format!("URL contains challenge pattern: \"{pattern}\""));
        }
    }

    // 5. Very few interactive elements + challenge-like URL or title
    let interactive_count = view
        .elements
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                ElementKind::Button
                    | ElementKind::Input
                    | ElementKind::Textarea
                    | ElementKind::Select
            )
        })
        .count();

    if interactive_count < 3 {
        let has_challenge_signal = lower_url.contains("challenge")
            || lower_url.contains("captcha")
            || lower_url.contains("cdn-cgi")
            || lower_title.contains("cloudflare");
        if has_challenge_signal {
            return Some(format!(
                "few interactive elements ({interactive_count}) with challenge URL/title"
            ));
        }
    }

    // 6. CHAOS-C6 + DX-CL2: CSS cloaking detection — zero interactive
    //    elements but substantial visible text is present, AND the page does
    //    not look like a SPA shell mid-hydration. The page may be hiding
    //    interactive content behind CSS (display:none on the container,
    //    visible text via pseudo-elements or aria-hidden tricks).
    if crate::cloaking::is_css_cloaking(interactive_count, &view.visible_text, markers) {
        return Some(
            "possible CSS cloaking: no interactive elements but text is visible".to_string(),
        );
    }

    None
}

/// Classification of detected bot-challenge type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeKind {
    /// Cloudflare Turnstile — may auto-resolve without interaction.
    CloudflareTurnstile,
    /// Interactive CAPTCHA (hCaptcha, reCAPTCHA) — requires human.
    Captcha,
    /// WAF/IP block — human cannot resolve.
    WafBlock,
    /// Login/auth wall — needs credentials, not a captcha.
    AuthWall,
}

/// Classify a blocked-reason string into a [`ChallengeKind`].
///
/// Used by the pilot to decide whether to auto-wait (Turnstile),
/// pause for human interaction (Captcha), or escalate immediately
/// (WafBlock/AuthWall).
pub fn classify_challenge(reason: &str) -> ChallengeKind {
    let lower = reason.to_lowercase();
    if lower.contains("turnstile")
        || lower.contains("just a moment")
        || lower.contains("checking your browser")
    {
        ChallengeKind::CloudflareTurnstile
    } else if lower.contains("hcaptcha") || lower.contains("recaptcha") || lower.contains("captcha")
    {
        ChallengeKind::Captcha
    } else if lower.contains("access denied")
        || lower.contains("forbidden")
        || lower.contains("403")
    {
        ChallengeKind::WafBlock
    } else if lower.contains("unauthorized") || lower.contains("login") {
        ChallengeKind::AuthWall
    } else {
        // Default to interactive captcha (safe fallback).
        ChallengeKind::Captcha
    }
}

// ── Steganographic sanitization ───────────────────────────────────

/// Strip steganographic characters and mask sensitive values in a
/// [`SemanticView`] before any LLM sees the data.
fn sanitize_view(view: &mut SemanticView) {
    use crate::sanitize::{mask_sensitive_value, sanitize_text};

    view.title = sanitize_text(&view.title);
    view.visible_text = sanitize_text(&view.visible_text);

    for el in &mut view.elements {
        el.label = sanitize_text(&el.label);
        // FIX-3: sanitize name, href, context, and input_type — these flow
        // into to_prompt() raw and could carry steganographic payloads.
        if let Some(ref name) = el.name {
            el.name = Some(sanitize_text(name));
        }
        if let Some(ref href) = el.href {
            // FIX-3: Redact URL secrets from hrefs (tokens in query params).
            let cleaned = sanitize_text(href);
            el.href = Some(crate::sanitize::redact_url_secrets(&cleaned));
        }
        if let Some(ref ph) = el.placeholder {
            el.placeholder = Some(sanitize_text(ph));
        }
        if let Some(ref ctx) = el.context {
            el.context = Some(sanitize_text(ctx));
        }
        if let Some(ref itype) = el.input_type {
            el.input_type = Some(sanitize_text(itype));
        }
        // DX-W2-2: Sanitize select option labels.
        if let Some(ref opts) = el.options {
            el.options = Some(opts.iter().map(|o| sanitize_text(o)).collect());
        }
        // FIX-10: Mask sensitive values by type AND name
        el.value = mask_sensitive_value(
            el.input_type.as_deref(),
            el.name.as_deref(),
            el.value.as_deref(),
        );
        // Sanitize remaining non-masked values
        let is_masked = el
            .input_type
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("password"))
            || el.name.as_deref().is_some_and(|n| {
                let lower = n.to_lowercase();
                lower.contains("password") || lower.contains("passwd") || lower.contains("secret")
            });
        if !is_masked && let Some(ref v) = el.value {
            el.value = Some(sanitize_text(v));
        }
    }
}

// ── SPA wait strategy ──────────────────────────────────────────────

/// Default SPA wait timeout in seconds.
///
/// CHAOS-C5: Increased from 5s to 15s for SPAs that hydrate slowly.
/// Callers that need env-var configurability should use [`configured_wait_timeout`].
pub const DEFAULT_WAIT_TIMEOUT: u64 = 15;

/// SPA wait timeout in seconds, configurable via `LAD_WAIT_TIMEOUT` env var.
///
/// Falls back to [`DEFAULT_WAIT_TIMEOUT`] (15s) when the env var is unset or invalid.
pub fn configured_wait_timeout() -> u64 {
    std::env::var("LAD_WAIT_TIMEOUT")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_WAIT_TIMEOUT)
}

/// Wait for interactive content to appear and stabilise on a page.
///
/// Polls every 200ms. Returns early once the interactive element count
/// is > 0 and unchanged for two consecutive checks (content stable).
/// If `timeout_secs` elapses with zero elements, returns anyway
/// (the page may be a bot-challenge or truly empty).
pub async fn wait_for_content(
    page: &dyn PageHandle,
    timeout_secs: u64,
) -> Result<(), crate::Error> {
    use std::time::{Duration, Instant};

    let poll_interval = Duration::from_millis(200);
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);

    let js = r#"document.querySelectorAll('input, button, a[href], select, textarea, [role="button"]').length"#;

    let mut prev_count: Option<i64> = None;
    let mut stable_hits = 0u32;

    while Instant::now() < deadline {
        let count: i64 = page
            .eval_js(js)
            .await
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or(0);

        if count > 0 {
            if prev_count == Some(count) {
                stable_hits += 1;
                if stable_hits >= 2 {
                    tracing::info!(elements = count, "content stable after polling");
                    return Ok(());
                }
            } else {
                stable_hits = 0;
            }
        }

        prev_count = Some(count);
        tokio::time::sleep(poll_interval).await;
    }

    tracing::info!(
        final_count = prev_count.unwrap_or(0),
        "wait_for_content timeout reached"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── FIX-3: sanitize_view covers name, href, context, input_type ──

    #[test]
    fn sanitize_view_cleans_name_and_href() {
        let mut view = SemanticView {
            url: String::new(),
            title: String::new(),
            page_hint: String::new(),
            elements: vec![Element {
                id: 0,
                kind: ElementKind::Link,
                label: String::new(),
                name: Some("my\u{200B}name".into()),
                value: None,
                placeholder: None,
                href: Some("https://evil\u{200D}.com".into()),
                input_type: Some("text\u{FEFF}".into()),
                disabled: false,
                form_index: None,
                context: Some("ctx\u{200C}val".into()),
                hint: None,
                checked: None,
                options: None,
                frame_index: None,
                is_visible: None,
            }],
            forms: vec![],
            visible_text: String::new(),
            text_blocks: vec![],
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
            cards: None,
        };
        sanitize_view(&mut view);
        assert_eq!(view.elements[0].name.as_deref(), Some("myname"));
        // URL normalization by redact_url_secrets adds trailing slash.
        assert_eq!(view.elements[0].href.as_deref(), Some("https://evil.com/"));
        assert_eq!(view.elements[0].input_type.as_deref(), Some("text"));
        assert_eq!(view.elements[0].context.as_deref(), Some("ctxval"));
    }

    #[test]
    fn classify_turnstile_from_title() {
        assert_eq!(
            classify_challenge("title matches challenge keyword: \"just a moment\""),
            ChallengeKind::CloudflareTurnstile,
        );
    }

    #[test]
    fn classify_turnstile_from_text() {
        assert_eq!(
            classify_challenge("page text matches challenge keyword: \"cf-turnstile\""),
            ChallengeKind::CloudflareTurnstile,
        );
    }

    #[test]
    fn classify_turnstile_checking_browser() {
        assert_eq!(
            classify_challenge("page text matches challenge keyword: \"checking your browser\""),
            ChallengeKind::CloudflareTurnstile,
        );
    }

    #[test]
    fn classify_hcaptcha() {
        assert_eq!(
            classify_challenge("page text matches challenge keyword: \"hcaptcha\""),
            ChallengeKind::Captcha,
        );
    }

    #[test]
    fn classify_recaptcha() {
        assert_eq!(
            classify_challenge("page text matches challenge keyword: \"recaptcha\""),
            ChallengeKind::Captcha,
        );
    }

    #[test]
    fn classify_generic_captcha() {
        assert_eq!(
            classify_challenge("page text matches challenge keyword: \"captcha\""),
            ChallengeKind::Captcha,
        );
    }

    #[test]
    fn classify_waf_forbidden() {
        assert_eq!(
            classify_challenge("title matches error page keyword: \"forbidden\""),
            ChallengeKind::WafBlock,
        );
    }

    #[test]
    fn classify_waf_access_denied() {
        assert_eq!(
            classify_challenge("title matches challenge keyword: \"access denied\""),
            ChallengeKind::WafBlock,
        );
    }

    #[test]
    fn classify_auth_wall_unauthorized() {
        assert_eq!(
            classify_challenge("title matches error page keyword: \"unauthorized\""),
            ChallengeKind::AuthWall,
        );
    }

    #[test]
    fn classify_auth_wall_login() {
        assert_eq!(
            classify_challenge("page requires login"),
            ChallengeKind::AuthWall,
        );
    }

    #[test]
    fn classify_unknown_defaults_to_captcha() {
        assert_eq!(
            classify_challenge("something unknown happened"),
            ChallengeKind::Captcha,
        );
    }

    // ── CHAOS-C6: CSS cloaking detection ──────────────────────

    #[test]
    fn detect_css_cloaking_no_elements_with_text() {
        // DX-CL2 (bug 2): raised threshold to 500 chars AND requires absence
        // of SPA shell markers. We feed it 600 chars of static text with
        // default (all-false) markers, which is the true cloaking case.
        let long_text = "x ".repeat(400); // 800 chars.
        let view = SemanticView {
            url: "https://example.com".into(),
            title: "Normal Page".into(),
            page_hint: "".into(),
            elements: vec![],
            forms: vec![],
            visible_text: long_text,
            text_blocks: vec![],
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
            cards: None,
        };
        let reason = detect_bot_challenge(&view);
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("CSS cloaking"));
    }

    #[test]
    fn no_css_cloaking_below_text_threshold() {
        // DX-CL2: short text should no longer trip the detector.
        let view = SemanticView {
            url: "https://example.com".into(),
            title: "Normal Page".into(),
            page_hint: "".into(),
            elements: vec![],
            forms: vec![],
            visible_text: "Some visible content here".into(),
            text_blocks: vec![],
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
            cards: None,
        };
        assert!(detect_bot_challenge(&view).is_none());
    }

    #[test]
    fn no_css_cloaking_on_spa_shell() {
        // DX-CL2 (bug 2): long text + zero elements + SPA shell markers
        // (Next.js) must NOT be classified as cloaking. Same case as
        // detect_css_cloaking_no_elements_with_text but with markers.
        let long_text = "x ".repeat(400);
        let view = SemanticView {
            url: "https://twitter.com".into(),
            title: "X".into(),
            page_hint: "".into(),
            elements: vec![],
            forms: vec![],
            visible_text: long_text,
            text_blocks: vec![],
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
            cards: None,
        };
        let markers = crate::cloaking::ShellMarkers {
            ready_complete: true,
            has_next_data: true,
            has_framework_root: true,
            script_tag_count: 10,
        };
        assert!(detect_bot_challenge_with_markers(&view, &markers).is_none());
    }

    #[test]
    fn no_css_cloaking_when_elements_present() {
        let view = SemanticView {
            url: "https://example.com".into(),
            title: "Normal Page".into(),
            page_hint: "".into(),
            elements: vec![Element {
                id: 0,
                kind: ElementKind::Button,
                label: "Click me".into(),
                name: None,
                value: None,
                placeholder: None,
                href: None,
                input_type: None,
                disabled: false,
                form_index: None,
                context: None,
                hint: None,
                checked: None,
                options: None,
                frame_index: None,
                is_visible: None,
            }],
            forms: vec![],
            visible_text: "Some text".into(),
            text_blocks: vec![],
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
            cards: None,
        };
        // Has elements, so no cloaking detection
        assert!(detect_bot_challenge(&view).is_none());
    }

    #[test]
    fn no_css_cloaking_when_no_text() {
        let view = SemanticView {
            url: "https://example.com".into(),
            title: "Empty Page".into(),
            page_hint: "".into(),
            elements: vec![],
            forms: vec![],
            visible_text: String::new(),
            text_blocks: vec![],
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
            cards: None,
        };
        // No elements AND no text — not cloaking, just empty
        assert!(detect_bot_challenge(&view).is_none());
    }

    // ── CHAOS-C5: Configurable wait timeout ──────────────────

    #[test]
    fn default_wait_timeout_is_15() {
        // Without env var, should be 15 seconds.
        assert_eq!(DEFAULT_WAIT_TIMEOUT, 15);
    }

    // ── DX-16: HN profile form.id = {} parsing ──────────────────────

    #[test]
    fn js_form_meta_deserializes_string_id() {
        let json = r#"{"index":0,"action":"/xuser","method":"POST","id":"myform","name":null}"#;
        let meta: JsFormMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.id, Some("myform".into()));
    }

    #[test]
    fn js_form_meta_deserializes_null_id() {
        let json = r#"{"index":0,"action":"/xuser","method":"POST","id":null,"name":null}"#;
        let meta: JsFormMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.id, None);
    }

    #[test]
    fn js_form_meta_deserializes_empty_object_id() {
        // HN returns form.id as {} (empty object from DOM element without id attribute).
        let json = r#"{"index":0,"action":"/xuser","method":"POST","id":{},"name":null}"#;
        let meta: JsFormMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.id, None);
    }

    #[test]
    fn js_form_meta_deserializes_missing_id() {
        let json = r#"{"index":0,"action":"/xuser","method":"POST","name":null}"#;
        let meta: JsFormMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.id, None);
    }

    #[test]
    fn js_extraction_with_hn_form() {
        // Minimal JsExtraction mimicking HN profile page with form.id = {}.
        let json = r#"{
            "elements": [],
            "visibleText": "Hacker News profile",
            "formCount": 1,
            "elementCap": null,
            "forms": [{"index":0,"action":"/xuser","method":"POST","id":{},"name":null}]
        }"#;
        let extraction: JsExtraction = serde_json::from_str(json).unwrap();
        assert_eq!(extraction.form_count, 1);
        assert_eq!(extraction.forms.len(), 1);
        assert_eq!(extraction.forms[0].id, None);
        assert_eq!(extraction.forms[0].action, Some("/xuser".into()));
    }

    // ── FR-4: article/repo classifier ─────────────────────────

    fn link(id: u32, label: &str, href: &str) -> Element {
        Element {
            id,
            kind: ElementKind::Link,
            label: label.into(),
            name: None,
            value: None,
            placeholder: None,
            href: Some(href.into()),
            input_type: None,
            disabled: false,
            form_index: None,
            context: None,
            hint: None,
            checked: None,
            options: None,
            frame_index: None,
            is_visible: None,
        }
    }

    fn many_links(n: usize) -> Vec<Element> {
        (0..n)
            .map(|i| link(i as u32, &format!("link {i}"), &format!("/p/{i}")))
            .collect()
    }

    #[test]
    fn classify_article_signal_wins_over_listing() {
        // 40 ambient links would normally trigger navigation/listing, but
        // DOM's `<article>` signal promotes the page to article/repo.
        let elements = many_links(40);
        let hint = classify_page(
            "GitHub",
            "https://github.com/anthropics/claude-code",
            &elements,
            true,
        );
        assert_eq!(hint, "article/repo page");
    }

    #[test]
    fn classify_url_pattern_repo_root_without_dom_signal() {
        // No <article>, but URL is /owner/repo on a repo-host allowlist.
        let elements = many_links(15);
        let hint = classify_page(
            "anthropics/claude-code · GitHub",
            "https://github.com/anthropics/claude-code",
            &elements,
            false,
        );
        assert_eq!(hint, "article/repo page");
    }

    #[test]
    fn classify_url_pattern_repo_issues() {
        let hint = classify_page(
            "Issue #42",
            "https://github.com/anthropics/claude-code/issues/42",
            &many_links(30),
            false,
        );
        assert_eq!(hint, "article/repo page");
    }

    #[test]
    fn classify_url_pattern_gitlab_blob() {
        let hint = classify_page(
            "file.rs",
            "https://gitlab.com/owner/repo/blob/main/src/file.rs",
            &many_links(12),
            false,
        );
        assert_eq!(hint, "article/repo page");
    }

    #[test]
    fn classify_hn_news_pagination_not_a_repo() {
        // FR-4 protects against the false positive on `/owner/bar` shape
        // outside the allowlist — HN paginator at news.ycombinator.com
        // must stay a navigation/listing page.
        let hint = classify_page(
            "Hacker News",
            "https://news.ycombinator.com/news/2",
            &many_links(40),
            false,
        );
        assert_eq!(hint, "navigation/listing page");
    }

    #[test]
    fn classify_article_signal_wins_over_url_pattern() {
        // DOM signal beats URL absence — blog post on a custom domain
        // still classifies as article/repo when <article> is present.
        let hint = classify_page(
            "Post",
            "https://blog.example.com/2026/post",
            &many_links(25),
            true,
        );
        assert_eq!(hint, "article/repo page");
    }

    #[test]
    fn classify_listing_preserved_when_no_signal_and_not_repo_host() {
        // Regression: plain listing pages without the article signal
        // keep their existing classification.
        let hint = classify_page("News", "https://example.com/news", &many_links(15), false);
        assert_eq!(hint, "navigation/listing page");
    }

    #[test]
    fn classify_login_wins_over_article_signal() {
        // <article>-bearing login pages (rare but possible) still
        // register as login so auth-gate detection does not regress.
        let mut elements = many_links(5);
        elements.push(Element {
            id: 99,
            kind: ElementKind::Input,
            label: "password".into(),
            name: Some("password".into()),
            value: None,
            placeholder: None,
            href: None,
            input_type: Some("password".into()),
            disabled: false,
            form_index: Some(0),
            context: None,
            hint: None,
            checked: None,
            options: None,
            frame_index: None,
            is_visible: None,
        });
        let hint = classify_page("Sign in", "https://example.com/login", &elements, true);
        assert_eq!(hint, "login page");
    }

    #[test]
    fn url_repo_pattern_allowlist_gates() {
        // github.com: /owner/repo → match.
        assert!(url_matches_repo_pattern(
            "github.com",
            "/anthropics/claude-code"
        ));
        // gitlab.com: /owner/repo/pulls → match.
        assert!(url_matches_repo_pattern("gitlab.com", "/owner/repo/pulls"));
        // codeberg.org: commits subpage.
        assert!(url_matches_repo_pattern(
            "codeberg.org",
            "/owner/repo/commits/main"
        ));
        // Non-allowlisted host: same shape, should NOT match.
        assert!(!url_matches_repo_pattern(
            "news.ycombinator.com",
            "/user/pg"
        ));
        assert!(!url_matches_repo_pattern("news.ycombinator.com", "/news/2"));
        // Allowlisted host but depth-1 path: not a repo.
        assert!(!url_matches_repo_pattern("github.com", "/settings"));
        // Allowlisted host, repo root with trailing slash.
        assert!(url_matches_repo_pattern("github.com", "/owner/repo/"));
        // Random subpage outside the canonical set on a repo host.
        assert!(!url_matches_repo_pattern(
            "github.com",
            "/owner/repo/something-random"
        ));
    }

    #[test]
    fn url_repo_pattern_case_insensitive_host() {
        assert!(url_matches_repo_pattern("GitHub.com", "/owner/repo"));
    }
}
