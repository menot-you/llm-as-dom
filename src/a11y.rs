//! Accessibility tree extraction via JS injection.
//! Falls back from CDP Accessibility API to direct JS DOM walking
//! because chromiumoxide's CDP bindings have serde issues with some AX nodes.

use chromiumoxide::Page;
use serde::Deserialize;

use crate::semantic::{Element, ElementKind, PageState, SemanticView};

/// Extract page structure via JS and compress to SemanticView.
pub async fn extract_semantic_view(page: &Page) -> Result<SemanticView, crate::Error> {
    let url = page.url().await?.unwrap_or_else(|| "unknown".into());
    let title = page.get_title().await?.unwrap_or_default();

    // Extract interactive elements + visible text via JS
    let js = r#"
        (() => {
            // Collect interactive elements
            const selectors = 'a[href], button, input, textarea, select, [role="button"], [role="link"], [role="checkbox"], [role="radio"], [role="tab"], [role="menuitem"]';
            const els = document.querySelectorAll(selectors);
            const elements = [];
            let id = 0;
            
            for (const el of els) {
                // Skip hidden elements
                const style = window.getComputedStyle(el);
                if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') continue;
                const rect = el.getBoundingClientRect();
                if (rect.width === 0 && rect.height === 0) continue;
                
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
                
                // Build label from multiple sources
                const ariaLabel = el.getAttribute('aria-label');
                const labelEl = el.labels?.[0];
                const labelText = labelEl?.textContent?.trim();
                const placeholder = el.getAttribute('placeholder');
                const textContent = el.textContent?.trim()?.substring(0, 80);
                const title = el.getAttribute('title');
                const label = ariaLabel || labelText || placeholder || textContent || title || '';
                
                // Stamp a data attribute for action execution
                el.setAttribute('data-lad-id', String(id));
                
                elements.push({
                    id: id,
                    kind: kind,
                    label: label.substring(0, 80),
                    name: el.getAttribute('name') || null,
                    value: el.value || null,
                    placeholder: placeholder || null,
                    href: el.getAttribute('href') || null,
                    input_type: el.getAttribute('type') || (tag === 'textarea' ? 'textarea' : null),
                    disabled: el.disabled || false,
                });
                id++;
            }
            
            // Collect visible text (headings + paragraphs)
            const textNodes = document.querySelectorAll('h1, h2, h3, h4, p, label, legend, [role="heading"]');
            let visibleText = '';
            for (const node of textNodes) {
                const text = node.textContent?.trim();
                if (text && visibleText.length < 500) {
                    if (visibleText) visibleText += ' ';
                    visibleText += text.substring(0, 100);
                }
            }
            
            return { elements, visibleText };
        })()
    "#;

    let result = page.evaluate(js).await?;
    let extraction: JsExtraction = result.into_value().map_err(|e| {
        crate::Error::Backend(format!("JS extraction parse failed: {e:?}"))
    })?;

    tracing::info!(
        elements = extraction.elements.len(),
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
            context: None,
        })
        .collect();

    let page_hint = classify_page(&title, &url, &elements);

    Ok(SemanticView {
        url,
        title,
        page_hint,
        elements,
        visible_text: extraction.visible_text,
        state: PageState::Ready,
    })
}

#[derive(Deserialize)]
struct JsExtraction {
    elements: Vec<JsElement>,
    #[serde(rename = "visibleText")]
    visible_text: String,
}

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
}

fn parse_kind(s: &str) -> ElementKind {
    match s {
        "button" => ElementKind::Button,
        "input" => ElementKind::Input,
        "link" => ElementKind::Link,
        "select" => ElementKind::Select,
        "textarea" => ElementKind::Input,
        "checkbox" => ElementKind::Checkbox,
        "radio" => ElementKind::Radio,
        _ => ElementKind::Other,
    }
}

fn classify_page(title: &str, url: &str, elements: &[Element]) -> String {
    let lower_title = title.to_lowercase();
    let lower_url = url.to_lowercase();

    let has_password = elements.iter().any(|e| e.input_type.as_deref() == Some("password"));
    let has_inputs = elements.iter().any(|e| e.kind == ElementKind::Input);
    let has_submit = elements.iter().any(|e| {
        e.kind == ElementKind::Button
            && (e.label.to_lowercase().contains("submit")
                || e.label.to_lowercase().contains("sign")
                || e.label.to_lowercase().contains("log"))
    });

    if has_password || lower_title.contains("login") || lower_title.contains("sign in") || lower_url.contains("login") {
        "login page".into()
    } else if lower_url.contains("search") || lower_title.contains("search") {
        "search page".into()
    } else if has_inputs && has_submit {
        "form page".into()
    } else if elements.iter().filter(|e| e.kind == ElementKind::Link).count() > 10 {
        "navigation/listing page".into()
    } else if has_inputs {
        "interactive page".into()
    } else {
        "content page".into()
    }
}
