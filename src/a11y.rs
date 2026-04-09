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
pub async fn extract_semantic_view(page: &dyn PageHandle) -> Result<SemanticView, crate::Error> {
    let url = page.url().await.unwrap_or_else(|_| "unknown".into());
    let title = page.title().await.unwrap_or_else(|_| String::new());

    let js = r#"
        (() => {
            // CHAOS-C3: Override window.close() to prevent hostile pages from
            // killing the browser tab/handle during extraction or navigation.
            try { window.close = function(){}; } catch(_) {}

            const MAX_ELEMENTS = 300;
            const selectors = 'a[href], button, input, textarea, select, [role="button"], [role="link"], [role="checkbox"], [role="radio"], [role="tab"], [role="menuitem"], [onclick]';
            const rawElements = [];
            let id = 0;

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

            // DX-FIX: Detect active modal/dialog and scope extraction to it.
            // This prevents extracting background elements when a modal is open,
            // fixing fill_form wrong-match, click-behind-modal, and modal scroll issues.
            const activeDialog = document.querySelector(
                'dialog[open], [role="dialog"][aria-modal="true"], [role="dialog"]:not([aria-hidden="true"])'
            );
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
                if (parseFloat(style.opacity) === 0) return false;
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
                    if (!isVisible(el)) continue;

                    const tag = el.tagName.toLowerCase();
                    let kind = 'other';
                    if (tag === 'button' || el.getAttribute('role') === 'button' || (tag === 'input' && el.type === 'submit')) kind = 'button';
                    else if (tag === 'input' && el.type !== 'hidden') kind = 'input';
                    else if (tag === 'textarea') kind = 'textarea';
                    else if (tag === 'select') kind = 'select';
                    else if (tag === 'a') kind = 'link';
                    else if (el.getAttribute('role') === 'checkbox' || (tag === 'input' && el.type === 'checkbox')) kind = 'checkbox';
                    else if (el.getAttribute('role') === 'radio' || (tag === 'input' && el.type === 'radio')) kind = 'radio';
                    else if (el.getAttribute('role') === 'tab' || el.getAttribute('role') === 'menuitem') kind = 'button';

                    const ariaLabel = el.getAttribute('aria-label');
                    const labelEl = el.labels?.[0];
                    const labelText = labelEl?.textContent?.trim();
                    const placeholder = el.getAttribute('placeholder');
                    const textContent = el.textContent?.trim()?.substring(0, 80);
                    const elTitle = el.getAttribute('title');
                    const href = el.getAttribute('href') || '';
                    let label = (ariaLabel || labelText || placeholder || textContent || elTitle || '').replace(/\s+/g, ' ').trim();
                    if (!label && kind === 'link' && href) {
                        label = href.split('/').filter(Boolean).pop() || '';
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
                    const checked = (kind === 'checkbox' || kind === 'radio') ? !!el.checked : null;

                    // DX-W2-2: Extract option labels for <select> elements (top 10).
                    let options = null;
                    if (kind === 'select' && el.options) {
                        options = Array.from(el.options).slice(0, 10).map(o => o.textContent.trim());
                    }

                    rawElements.push({
                        el, kind, label: label.substring(0, 80),
                        name: el.getAttribute('name') || null,
                        value: el.value || null,
                        placeholder: placeholder || null,
                        href: href || null,
                        input_type: el.getAttribute('type') || (tag === 'textarea' ? 'textarea' : null),
                        disabled: el.disabled || false,
                        form_index: formIndex,
                        hint_type: hintType,
                        hint_value: hintValue,
                        frame_index: frameIdx,
                        checked: checked,
                        options: options,
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
                });
                id++;
            }

            const textNodes = deepQueryAll(document, 'h1, h2, h3, h4, p, label, legend, [role="heading"]');
            let visibleText = '';
            for (const node of textNodes) {
                const text = node.textContent?.trim();
                if (text && visibleText.length < 500) {
                    if (visibleText) visibleText += ' ';
                    visibleText += text.substring(0, 100);
                }
            }
            // Fallback: collect substantial text from td, span, a when headings/paragraphs yielded little
            if (visibleText.length < 100) {
                const extraNodes = deepQueryAll(document, 'td, span, a');
                for (const node of extraNodes) {
                    const text = node.textContent?.trim();
                    if (text && text.length > 20 && visibleText.length < 500) {
                        if (visibleText) visibleText += ' ';
                        visibleText += text.substring(0, 100);
                    }
                }
            }

            // ── Form metadata ───────────────────────────────────────────
            const forms = Array.from(allForms).map((f, i) => ({
                index: i,
                action: f.getAttribute('action') || null,
                method: (f.getAttribute('method') || 'GET').toUpperCase(),
                id: f.id || null,
                name: f.getAttribute('name') || null,
            }));

            return { elements, visibleText, formCount: allForms.length, elementCap, forms };
        })()
    "#;

    let extraction: JsExtraction = crate::engine::eval_js_into(page, js).await?;

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
            }
        })
        .collect();

    let page_hint = classify_page(&title, &url, &elements);

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

    let mut view = SemanticView {
        url,
        title,
        page_hint,
        elements,
        forms,
        visible_text: extraction.visible_text,
        state: PageState::Ready,
        element_cap: extraction.element_cap,
        blocked_reason: None,
        session_context: None,
    };

    // ── Security: strip steganographic characters + mask passwords ──
    sanitize_view(&mut view);

    // Detect bot-challenge / CAPTCHA pages after extraction.
    if let Some(reason) = detect_bot_challenge(&view) {
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
    #[serde(rename = "formCount")]
    form_count: u32,
    /// `"50/316"` when elements were capped, `null` otherwise.
    #[serde(rename = "elementCap")]
    element_cap: Option<String>,
    /// Form metadata collected from each `<form>` on the page.
    #[serde(default)]
    forms: Vec<JsFormMeta>,
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

/// Classify the page type from its title, URL, and element composition.
fn classify_page(title: &str, url: &str, elements: &[Element]) -> String {
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
pub fn detect_bot_challenge(view: &SemanticView) -> Option<String> {
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

    // 6. CHAOS-C6: CSS cloaking detection — zero interactive elements but
    //    visible text is present. The page may be hiding interactive content
    //    behind CSS (display:none on the container, visible text via
    //    pseudo-elements or aria-hidden tricks).
    if view.elements.is_empty() && !view.visible_text.trim().is_empty() {
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
            }],
            forms: vec![],
            visible_text: String::new(),
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
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
        let view = SemanticView {
            url: "https://example.com".into(),
            title: "Normal Page".into(),
            page_hint: "".into(),
            elements: vec![],
            forms: vec![],
            visible_text: "Some visible content here".into(),
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        };
        let reason = detect_bot_challenge(&view);
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("CSS cloaking"));
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
            }],
            forms: vec![],
            visible_text: "Some text".into(),
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
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
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
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
}
