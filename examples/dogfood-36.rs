//! Dogfood for issue #36 — `lad_extract` honors `what` on content-heavy pages.
//!
//! Runs the REAL pipeline:
//! 1. Launch Chromium.
//! 2. Navigate to a content-heavy URL (github.com/browser-use/browser-use).
//! 3. Call `extract_semantic_view` — exercises the new JS walker emitting
//!    `text_blocks`.
//! 4. Run the same scorer shape that `tool_lad_extract` runs inline
//!    (same primitives, inlined here because the scorer lives in the
//!    bin crate; this example uses only the lib surface).
//! 5. Print BEFORE (raw banner) and AFTER (top-K matched blocks) so a
//!    human can eyeball that the answer to `what` actually surfaces.
//!
//! Run:
//!     cargo run --example dogfood-36 --release -- <url> <what>
//! Defaults:
//!     url  = https://github.com/browser-use/browser-use
//!     what = "installation command star count"

use llm_as_dom::a11y::extract_semantic_view;
use llm_as_dom::engine::chromium::ChromiumEngine;
use llm_as_dom::engine::{BrowserEngine, EngineConfig};
use llm_as_dom::semantic::SemanticView;
use std::time::Duration;

/// Same scoring math as `tool_lad_extract::apply_what_filter` — kept
/// inline here because the canonical scorer lives in the binary crate.
/// If this ever drifts, move the scorer to the lib crate.
fn apply_what_filter(view: &mut SemanticView, what: &str, max_length: Option<usize>) -> usize {
    let what_lower = what.to_lowercase();
    let words: Vec<String> = what_lower
        .split_whitespace()
        .filter(|w| w.len() >= 2)
        .map(|w| w.to_string())
        .collect();
    if words.is_empty() {
        return 0;
    }

    let score_el = |el: &llm_as_dom::semantic::Element| -> u32 {
        let fields = [
            el.label.to_lowercase(),
            el.name.as_deref().unwrap_or("").to_lowercase(),
            el.placeholder.as_deref().unwrap_or("").to_lowercase(),
            el.href.as_deref().unwrap_or("").to_lowercase(),
        ];
        let mut s = 0u32;
        for w in &words {
            for f in &fields {
                if f.contains(w.as_str()) {
                    s += 1;
                }
            }
        }
        s
    };

    view.elements
        .sort_by_key(|el| std::cmp::Reverse(score_el(el)));
    let relevant = view.elements.iter().filter(|el| score_el(el) > 0).count();

    if !view.text_blocks.is_empty() {
        let score_b = |b: &str| -> u32 {
            let l = b.to_lowercase();
            words.iter().filter(|w| l.contains(w.as_str())).count() as u32
        };
        let mut scored: Vec<(u32, &String)> = view
            .text_blocks
            .iter()
            .map(|b| (score_b(b), b))
            .filter(|(s, _)| *s > 0)
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.len().cmp(&a.1.len())));
        let top_k = max_length
            .map(|m| (m / 240).max(3))
            .unwrap_or(8)
            .clamp(3, 20);
        let joined = scored
            .iter()
            .take(top_k)
            .map(|(_, b)| b.as_str())
            .collect::<Vec<_>>()
            .join(" … ");
        if !joined.is_empty() {
            view.visible_text = joined;
        }
    }
    if let Some(max_len) = max_length
        && view.visible_text.len() > max_len
    {
        let mut end = max_len;
        while !view.visible_text.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        view.visible_text.truncate(end);
    }
    relevant
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .unwrap_or_else(|| "https://github.com/browser-use/browser-use".into());
    let what = args
        .next()
        .unwrap_or_else(|| "installation command star count".into());

    eprintln!("== dogfood issue #36 ==");
    eprintln!("url  = {url}");
    eprintln!("what = {what:?}");
    eprintln!();

    let engine = ChromiumEngine::launch(EngineConfig::default()).await?;
    let page = engine.new_page(&url).await?;
    page.wait_for_navigation().await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let view = extract_semantic_view(page.as_ref()).await?;

    eprintln!("-- BEFORE filter --");
    eprintln!("elements          = {}", view.elements.len());
    eprintln!("text_blocks       = {}", view.text_blocks.len());
    eprintln!("visible_text len  = {}", view.visible_text.len());
    eprintln!(
        "visible_text preview: {:.200}",
        view.visible_text.replace('\n', " ")
    );
    eprintln!();

    let mut filtered = view.clone();
    let relevant = apply_what_filter(&mut filtered, &what, Some(2000));

    eprintln!("-- AFTER filter --");
    eprintln!("relevant_count    = {relevant}");
    eprintln!("visible_text len  = {}", filtered.visible_text.len());
    eprintln!("visible_text:");
    eprintln!("{}", filtered.visible_text);
    eprintln!();

    eprintln!("-- Top 5 scored elements --");
    for el in filtered.elements.iter().take(5) {
        eprintln!(
            "  [{:>3}] {:?} label={:?} href={:?}",
            el.id, el.kind, el.label, el.href
        );
    }

    drop(page);
    engine.close().await?;
    Ok(())
}
