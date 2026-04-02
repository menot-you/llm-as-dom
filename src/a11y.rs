//! Accessibility tree extraction via JS injection.
//!
//! Falls back from CDP Accessibility API to direct JS DOM walking
//! because chromiumoxide's CDP bindings have serde issues with some AX nodes.

use chromiumoxide::Page;
use serde::Deserialize;

use crate::semantic::{Element, ElementKind, PageState, SemanticView};

/// Extract page structure via JS and compress to a [`SemanticView`].
///
/// Stamps each interactive element with a `data-lad-id` attribute so that
/// subsequent actions can target elements by stable numeric ID.
/// Also tracks which `<form>` each element belongs to for scoping.
pub async fn extract_semantic_view(page: &Page) -> Result<SemanticView, crate::Error> {
    let url = page.url().await?.unwrap_or_else(|| "unknown".into());
    let title = page.get_title().await?.unwrap_or_default();

    let js = r#"
        (() => {
            const MAX_ELEMENTS = 50;
            const selectors = 'a[href], button, input, textarea, select, [role="button"], [role="link"], [role="checkbox"], [role="radio"], [role="tab"], [role="menuitem"]';
            const els = document.querySelectorAll(selectors);
            const rawElements = [];
            let id = 0;

            // Build a form index: map each <form> to a sequential number
            const allForms = document.querySelectorAll('form');
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
                if (name === 'website' || name === 'url' || name === 'honeypot') return true;
                const ac = (el.getAttribute('autocomplete') || '').toLowerCase();
                const ti = el.getAttribute('tabindex');
                const style = window.getComputedStyle(el);
                const invisible = style.display === 'none' || style.visibility === 'hidden'
                    || parseFloat(style.opacity) === 0;
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
                if (rect.right < 0 || rect.bottom < 0
                    || rect.left > window.innerWidth
                    || rect.top > window.innerHeight) return false;
                if (hasZeroAncestorOpacity(el, 3)) return false;
                if (isHoneypot(el)) return false;
                return true;
            }

            // ── Collect visible elements ────────────────────────────────
            for (const el of els) {
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
                const label = ariaLabel || labelText || placeholder || textContent || elTitle || '';

                const closestForm = el.closest('form');
                const formIndex = closestForm ? (formMap.get(closestForm) ?? null) : null;

                // ── Relevance score (used when cap triggers) ────────────
                let score = 0;
                if (closestForm) score += 3;
                if (kind === 'input' || kind === 'textarea' || kind === 'select'
                    || kind === 'checkbox' || kind === 'radio') score += 5;
                if (kind === 'button') score += 4;
                if (tag === 'input' && el.type === 'submit') score += 2;
                if (ariaLabel) score += 2;
                const href = el.getAttribute('href') || '';
                if (kind === 'link') {
                    if (href === '#' || href.startsWith('#')) score -= 2;
                    const lcHref = href.toLowerCase();
                    if (lcHref.includes('facebook.com') || lcHref.includes('twitter.com')
                        || lcHref.includes('instagram.com') || lcHref.includes('linkedin.com')
                        || lcHref.includes('youtube.com') || lcHref.includes('tiktok.com')) score -= 3;
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
                    score,
                    isActionable: kind !== 'link' && kind !== 'other',
                });
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
                });
                id++;
            }

            const textNodes = document.querySelectorAll('h1, h2, h3, h4, p, label, legend, [role="heading"]');
            let visibleText = '';
            for (const node of textNodes) {
                const text = node.textContent?.trim();
                if (text && visibleText.length < 500) {
                    if (visibleText) visibleText += ' ';
                    visibleText += text.substring(0, 100);
                }
            }

            return { elements, visibleText, formCount: allForms.length, elementCap };
        })()
    "#;

    let result = page.evaluate(js).await?;
    let extraction: JsExtraction = result
        .into_value()
        .map_err(|e| crate::Error::Backend(format!("JS extraction parse failed: {e:?}")))?;

    tracing::info!(
        elements = extraction.elements.len(),
        forms = extraction.form_count,
        visible_text_len = extraction.visible_text.len(),
        "DOM extracted via JS"
    );

    let elements: Vec<Element> = extraction
        .elements
        .into_iter()
        .map(|e| Element {
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
        })
        .collect();

    let page_hint = classify_page(&title, &url, &elements);

    let mut view = SemanticView {
        url,
        title,
        page_hint,
        elements,
        visible_text: extraction.visible_text,
        state: PageState::Ready,
        element_cap: extraction.element_cap,
        blocked_reason: None,
    };

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
];

/// Detect whether a [`SemanticView`] looks like a bot-challenge or CAPTCHA page.
///
/// Returns `Some(reason)` when a challenge is detected, `None` otherwise.
pub fn detect_bot_challenge(view: &SemanticView) -> Option<String> {
    let lower_title = view.title.to_lowercase();
    let lower_text = view.visible_text.to_lowercase();

    // 1. Title match
    for kw in CHALLENGE_TITLES {
        if lower_title.contains(kw) {
            return Some(format!("title matches challenge keyword: \"{kw}\""));
        }
    }

    // 2. Visible text match
    for kw in CHALLENGE_TEXTS {
        if lower_text.contains(kw) {
            return Some(format!("page text matches challenge keyword: \"{kw}\""));
        }
    }

    // 3. Very few interactive elements + challenge-like URL or title
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
        let lower_url = view.url.to_lowercase();
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

    None
}

// ── SPA wait strategy ──────────────────────────────────────────────

/// Default SPA wait timeout in seconds.
pub const DEFAULT_WAIT_TIMEOUT: u64 = 5;

/// Wait for interactive content to appear and stabilise on a page.
///
/// Polls every 200ms. Returns early once the interactive element count
/// is > 0 and unchanged for two consecutive checks (content stable).
/// If `timeout_secs` elapses with zero elements, returns anyway
/// (the page may be a bot-challenge or truly empty).
pub async fn wait_for_content(page: &Page, timeout_secs: u64) -> Result<(), crate::Error> {
    use std::time::{Duration, Instant};

    let poll_interval = Duration::from_millis(200);
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);

    let js = r#"document.querySelectorAll('input, button, a[href], select, textarea, [role="button"]').length"#;

    let mut prev_count: Option<i64> = None;
    let mut stable_hits = 0u32;

    while Instant::now() < deadline {
        let count: i64 = page
            .evaluate(js)
            .await
            .ok()
            .and_then(|v| v.into_value().ok())
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
