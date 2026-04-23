//! Page quality audit engine.
//!
//! Checks accessibility, forms, and links issues by evaluating JS scripts
//! in a browser page. Returns structured `AuditIssue` results.

use serde::{Deserialize, Serialize};

/// Severity level for an audit issue.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Must fix: broken functionality or serious a11y violation.
    Critical,
    /// Should fix: usability or minor a11y issue.
    Warning,
    /// Nice to have: best-practice suggestion.
    Info,
}

/// A single audit finding (may represent multiple identical occurrences).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditIssue {
    /// Category: `"a11y"`, `"forms"`, or `"links"`.
    pub category: String,
    /// How severe this issue is.
    pub severity: Severity,
    /// CSS-like identifier for the element (e.g. `"img#hero"`, `"input[name=email]"`).
    pub element: String,
    /// Human-readable description of the issue.
    pub message: String,
    /// Suggested fix.
    pub suggestion: String,
    /// Number of identical occurrences (grouped by category + message).
    /// `1` for unique issues, `>1` for deduplicated groups.
    #[serde(skip_serializing_if = "is_one")]
    pub count: u32,
}

/// Returns `true` when count is 1, used for serde skip.
fn is_one(val: &u32) -> bool {
    *val == 1
}

/// Summary counts by severity.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditSummary {
    /// Number of critical issues.
    pub critical: usize,
    /// Number of warning issues.
    pub warning: usize,
    /// Number of info issues.
    pub info: usize,
}

/// Complete audit result for a page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditResult {
    /// The audited URL.
    pub url: String,
    /// All issues found.
    pub issues: Vec<AuditIssue>,
    /// Summary counts.
    pub summary: AuditSummary,
}

/// Default audit categories when none specified.
pub fn default_categories() -> Vec<String> {
    vec!["a11y".into(), "forms".into(), "links".into()]
}

/// Build the JS script that runs all requested audit checks in the browser.
///
/// Returns a self-executing JS function that produces a JSON-serializable
/// array of `{ category, severity, element, message, suggestion }` objects.
pub fn build_audit_js(categories: &[String]) -> String {
    let mut js = String::from(
        r#"(() => {
    const issues = [];
    function ident(el) {
        const tag = el.tagName.toLowerCase();
        const id = el.id ? '#' + el.id : '';
        const cls = el.className && typeof el.className === 'string'
            ? '.' + el.className.trim().split(/\s+/).slice(0, 2).join('.')
            : '';
        const name = el.getAttribute('name') ? '[name=' + el.getAttribute('name') + ']' : '';
        return (tag + id + cls + name).substring(0, 80);
    }
"#,
    );

    for cat in categories {
        match cat.as_str() {
            "a11y" => js.push_str(A11Y_CHECKS),
            "forms" => js.push_str(FORMS_CHECKS),
            "links" => js.push_str(LINKS_CHECKS),
            _ => {} // unknown category — skip silently
        }
    }

    js.push_str(
        r#"
    return issues;
})()
"#,
    );
    js
}

/// Parse raw JS audit output into structured `AuditResult`.
///
/// Deduplicates issues: identical `(category, message)` pairs are grouped
/// into a single `AuditIssue` with a `count` field. The summary counts
/// reflect the **deduplicated** issue count (not raw occurrences).
pub fn parse_audit_result(url: &str, raw: Vec<RawAuditIssue>) -> AuditResult {
    use std::collections::HashMap;

    // Group by (category, message) to deduplicate.
    let mut groups: HashMap<(String, String), (RawAuditIssue, u32)> = HashMap::new();
    // Preserve insertion order via a separate key list.
    let mut order: Vec<(String, String)> = Vec::new();

    for r in raw {
        let key = (r.category.clone(), r.message.clone());
        match groups.get_mut(&key) {
            Some((_first, count)) => *count += 1,
            None => {
                order.push(key.clone());
                groups.insert(key, (r, 1));
            }
        }
    }

    let mut summary = AuditSummary::default();
    let issues: Vec<AuditIssue> = order
        .into_iter()
        .filter_map(|key| groups.remove(&key))
        .map(|(r, count)| {
            let severity = match r.severity.as_str() {
                "critical" => {
                    summary.critical += 1;
                    Severity::Critical
                }
                "warning" => {
                    summary.warning += 1;
                    Severity::Warning
                }
                _ => {
                    summary.info += 1;
                    Severity::Info
                }
            };
            AuditIssue {
                category: r.category,
                severity,
                element: r.element,
                message: r.message,
                suggestion: r.suggestion,
                count,
            }
        })
        .collect();

    AuditResult {
        url: url.to_string(),
        issues,
        summary,
    }
}

/// Raw issue shape as returned by the JS audit script.
#[derive(Debug, Deserialize)]
pub struct RawAuditIssue {
    /// Category string.
    pub category: String,
    /// Severity string.
    pub severity: String,
    /// Element identifier.
    pub element: String,
    /// Issue message.
    pub message: String,
    /// Suggestion text.
    pub suggestion: String,
}

// ── JS check fragments ────────────────────────────────────────────

/// Accessibility checks (6 rules).
const A11Y_CHECKS: &str = r#"
    // A11Y-1: Images without alt text
    document.querySelectorAll('img').forEach(el => {
        if (!el.hasAttribute('alt')) {
            issues.push({
                category: 'a11y', severity: 'warning',
                element: ident(el),
                message: 'Image missing alt text',
                suggestion: 'Add alt attribute describing the image content',
            });
        }
    });

    // A11Y-2: Inputs without associated labels
    document.querySelectorAll('input:not([type=hidden]):not([type=submit]):not([type=button])').forEach(el => {
        const hasLabel = el.labels && el.labels.length > 0;
        const hasAriaLabel = el.hasAttribute('aria-label') || el.hasAttribute('aria-labelledby');
        if (!hasLabel && !hasAriaLabel) {
            issues.push({
                category: 'a11y', severity: 'warning',
                element: ident(el),
                message: 'Input without associated label',
                suggestion: 'Add a <label for="..."> element, or aria-label attribute',
            });
        }
    });

    // A11Y-3: Buttons without accessible text
    document.querySelectorAll('button, [role=button]').forEach(el => {
        const text = (el.textContent || '').trim();
        const ariaLabel = el.getAttribute('aria-label') || '';
        const title = el.getAttribute('title') || '';
        if (!text && !ariaLabel && !title) {
            issues.push({
                category: 'a11y', severity: 'warning',
                element: ident(el),
                message: 'Button without accessible text',
                suggestion: 'Add text content, aria-label, or title to the button',
            });
        }
    });

    // A11Y-4: Empty links (no text, no aria-label)
    document.querySelectorAll('a').forEach(el => {
        const text = (el.textContent || '').trim();
        const ariaLabel = el.getAttribute('aria-label') || '';
        const title = el.getAttribute('title') || '';
        const hasImg = el.querySelector('img[alt]');
        if (!text && !ariaLabel && !title && !hasImg) {
            issues.push({
                category: 'a11y', severity: 'warning',
                element: ident(el),
                message: 'Empty link without accessible text',
                suggestion: 'Add text content, aria-label, or title to the link',
            });
        }
    });

    // A11Y-5: Missing lang attribute on <html>
    if (!document.documentElement.hasAttribute('lang')) {
        issues.push({
            category: 'a11y', severity: 'warning',
            element: 'html',
            message: 'Missing lang attribute on <html>',
            suggestion: 'Add lang="en" (or appropriate language) to the <html> tag',
        });
    }

    // A11Y-6 (FR-6): heading hierarchy skips. Either no <h1> at all, or
    // a heading jumps a level (e.g. <h3> with no prior <h2>). Screen
    // readers rely on continuous h1→h2→h3 nesting to expose the page
    // outline.
    //
    // Skip-detection scope: we walk headings INSIDE the primary content
    // landmark (`<main>` if present, else `document.body`). Headings
    // inside complementary landmarks (`<aside>`, `<nav>`, `<header>`,
    // `<footer>`) are excluded because each landmark may legitimately
    // start its own outline (per HTML5 sectioning algorithm). This
    // matches `axe-core`'s `heading-order` behavior and trades a class
    // of false positives (h1 → h3 across landmark boundaries) for the
    // residual document-order limitation that remains within `<main>`.
    // The `h1Count` check still spans the full document — every page
    // should declare exactly one top-level heading regardless of
    // landmark layout.
    (() => {
        const h1Count = document.querySelectorAll('h1').length;
        if (h1Count === 0) {
            issues.push({
                category: 'a11y', severity: 'warning',
                element: 'html',
                message: 'Page has no <h1> heading',
                suggestion: 'Add exactly one <h1> describing the page purpose',
            });
        }
        // Flag each heading that skips from <hN-1> — e.g. h3 with no prior h2.
        // Scope to <main> when present so landmark sub-trees don't
        // generate false positives.
        const scope = document.querySelector('main') || document.body;
        if (!scope) return;
        const headings = Array.from(scope.querySelectorAll('h1, h2, h3, h4, h5, h6'))
            .filter(h => !h.closest('aside, nav, header, footer'));
        let lastLevel = 0;
        for (const h of headings) {
            const level = parseInt(h.tagName.substring(1), 10);
            if (lastLevel > 0 && level > lastLevel + 1) {
                issues.push({
                    category: 'a11y', severity: 'warning',
                    element: ident(h),
                    message: 'Heading skips level (<h' + lastLevel + '> → <h' + level + '>)',
                    suggestion: 'Promote the heading so levels increase by at most 1',
                });
            }
            lastLevel = level;
        }
    })();
"#;

/// Form checks (6 rules).
const FORMS_CHECKS: &str = r#"
    // FORMS-1: Inputs without autocomplete attribute
    document.querySelectorAll('input[type=text], input[type=email], input[type=tel], input[type=password]').forEach(el => {
        if (!el.hasAttribute('autocomplete')) {
            issues.push({
                category: 'forms', severity: 'info',
                element: ident(el),
                message: 'Input missing autocomplete attribute',
                suggestion: 'Add autocomplete="..." to help browsers autofill',
            });
        }
    });

    // FORMS-2: Password fields without minlength
    document.querySelectorAll('input[type=password]').forEach(el => {
        if (!el.hasAttribute('minlength')) {
            issues.push({
                category: 'forms', severity: 'info',
                element: ident(el),
                message: 'Password field without minlength',
                suggestion: 'Add minlength attribute to enforce minimum password length',
            });
        }
    });

    // FORMS-3: Forms without action attribute
    document.querySelectorAll('form').forEach(el => {
        if (!el.hasAttribute('action') && !el.hasAttribute('data-action')) {
            issues.push({
                category: 'forms', severity: 'info',
                element: ident(el),
                message: 'Form without action attribute',
                suggestion: 'Add action attribute or handle submission via JS event listener',
            });
        }
    });

    // FORMS-4: Submit buttons outside form
    document.querySelectorAll('button[type=submit], input[type=submit]').forEach(el => {
        if (!el.closest('form') && !el.hasAttribute('form')) {
            issues.push({
                category: 'forms', severity: 'warning',
                element: ident(el),
                message: 'Submit button outside of a form',
                suggestion: 'Place the submit button inside a <form> or use the form="formId" attribute',
            });
        }
    });

    // FORMS-5 (FR-6): secret-bearing forms without a visible anti-forgery
    // marker. Severity `info` — SameSite=Strict cookies are a legitimate
    // alternative, so flagged as hint, not error. Pattern list is broad
    // enough to cover Rails, Django, Laravel, and generic frameworks.
    document.querySelectorAll('form').forEach(f => {
        const hasSecretInput = f.querySelector('input[type=password]');
        if (!hasSecretInput) return;
        const markerHints = ['csrf', 'authenticity', 'xsrf', 'nonce'];
        let hasMarker = false;
        for (const hint of markerHints) {
            if (f.querySelector('input[type=hidden][name*="' + hint + '" i]')) {
                hasMarker = true;
                break;
            }
        }
        if (!hasMarker) {
            issues.push({
                category: 'forms', severity: 'info',
                element: ident(f),
                message: 'Form has sensitive input but no visible anti-forgery marker',
                suggestion: 'Add a hidden anti-forgery input, or confirm SameSite=Strict cookie protection',
            });
        }
    });

    // FORMS-6 (FR-6): sign-in/sign-up forms with missing autocomplete
    // hints. Modern browsers rely on the autocomplete attribute to
    // auto-fill sign-in data. Missing hints degrade UX for returning
    // users.
    document.querySelectorAll('form').forEach(f => {
        const hasSecretInput = f.querySelector('input[type=password]');
        if (!hasSecretInput) return;
        const missing = [];
        f.querySelectorAll('input[type=text], input[type=email], input[type=tel], input[type=password]').forEach(el => {
            if (!el.hasAttribute('autocomplete')) missing.push(el);
        });
        if (missing.length > 0) {
            issues.push({
                category: 'forms', severity: 'warning',
                element: ident(missing[0]),
                message: 'Sign-in form has input without autocomplete hint',
                suggestion: 'Set autocomplete="username" or the appropriate credential hint so browsers can autofill',
            });
        }
    });
"#;

/// Link checks (4 rules).
const LINKS_CHECKS: &str = r##"
    // LINKS-1: Links with href="javascript:void(0)" or href="#"
    document.querySelectorAll('a[href]').forEach(el => {
        const href = el.getAttribute('href');
        if (href === 'javascript:void(0)' || href === 'javascript:;' || href === '#') {
            issues.push({
                category: 'links', severity: 'warning',
                element: ident(el),
                message: 'Link with non-navigational href ("' + href + '")',
                suggestion: 'Use a <button> instead, or provide a real URL',
            });
        }
    });

    // LINKS-2: Links with target="_blank" without rel="noopener"
    document.querySelectorAll('a[target=_blank]').forEach(el => {
        const rel = (el.getAttribute('rel') || '').toLowerCase();
        if (!rel.includes('noopener')) {
            issues.push({
                category: 'links', severity: 'warning',
                element: ident(el),
                message: 'Link with target="_blank" missing rel="noopener"',
                suggestion: 'Add rel="noopener noreferrer" to prevent tab-nabbing',
            });
        }
    });

    // LINKS-4 (FR-6): links with target="_blank" that set noopener but
    // NOT noreferrer. noopener alone blocks window.opener access, but
    // noreferrer is the flag that suppresses the Referer header — some
    // analytics pipelines treat the two as interchangeable and ship
    // only noopener, leaving the referrer leak intact.
    //
    // The `target` selector uses the case-insensitive `i` flag so
    // `target="_BLANK"` (rare but spec-legal) is also caught. LINKS-2
    // above keeps its case-sensitive form for now — pre-existing rule,
    // out of scope for FR-6; tracked as a follow-up nit.
    document.querySelectorAll('a[target="_blank" i]').forEach(el => {
        const rel = (el.getAttribute('rel') || '').toLowerCase();
        if (rel.includes('noopener') && !rel.includes('noreferrer')) {
            issues.push({
                category: 'links', severity: 'warning',
                element: ident(el),
                message: 'Link with rel="noopener" missing rel="noreferrer"',
                suggestion: 'Append "noreferrer" so the Referer header is suppressed on the new tab',
            });
        }
    });

    // LINKS-3: Empty href attributes
    document.querySelectorAll('a[href=""]').forEach(el => {
        issues.push({
            category: 'links', severity: 'warning',
            element: ident(el),
            message: 'Link with empty href attribute',
            suggestion: 'Provide a valid URL or remove the href attribute',
        });
    });
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_js_includes_a11y_checks() {
        let js = build_audit_js(&["a11y".into()]);
        assert!(js.contains("img"), "should check images");
        assert!(js.contains("aria-label"), "should check aria-label");
        assert!(js.contains("lang"), "should check lang attribute");
    }

    #[test]
    fn build_js_includes_forms_checks() {
        let js = build_audit_js(&["forms".into()]);
        assert!(js.contains("autocomplete"), "should check autocomplete");
        assert!(js.contains("minlength"), "should check minlength");
    }

    #[test]
    fn build_js_includes_links_checks() {
        let js = build_audit_js(&["links".into()]);
        assert!(js.contains("javascript:void"), "should check js void links");
        assert!(js.contains("noopener"), "should check noopener");
    }

    #[test]
    fn build_js_all_categories() {
        let js = build_audit_js(&default_categories());
        assert!(js.contains("A11Y-1"), "should have a11y checks");
        assert!(js.contains("FORMS-1"), "should have forms checks");
        assert!(js.contains("LINKS-1"), "should have links checks");
    }

    #[test]
    fn build_js_unknown_category_ignored() {
        let js = build_audit_js(&["unknown".into()]);
        assert!(
            !js.contains("A11Y-1"),
            "unknown category should not include checks"
        );
        // Should still produce valid JS
        assert!(js.contains("return issues"), "should return issues array");
    }

    // ── FR-6: broader audit rules ──────────────────────────────

    #[test]
    fn build_js_a11y_has_heading_hierarchy_rule() {
        let js = build_audit_js(&["a11y".into()]);
        assert!(
            js.contains("A11Y-6"),
            "A11Y-6 heading hierarchy rule missing"
        );
        assert!(
            js.contains("Page has no <h1> heading"),
            "heading rule should flag missing h1"
        );
        assert!(
            js.contains("Heading skips level"),
            "heading rule should flag level skips"
        );
    }

    #[test]
    fn build_js_forms_has_anti_forgery_rule() {
        let js = build_audit_js(&["forms".into()]);
        assert!(js.contains("FORMS-5"), "FORMS-5 anti-forgery rule missing");
        assert!(
            js.contains("anti-forgery marker"),
            "rule message should mention anti-forgery"
        );
        // Severity must be `info`, not `warning` — SameSite=Strict
        // cookies are a legitimate alternative.
        let idx_forms_5 = js.find("FORMS-5").expect("FORMS-5 marker present");
        let context = &js[idx_forms_5..idx_forms_5 + 2000];
        assert!(
            context.contains("severity: 'info'"),
            "FORMS-5 must be severity `info` (SameSite cookies are a legitimate alt)"
        );
    }

    #[test]
    fn build_js_forms_has_autocomplete_sign_in_rule() {
        let js = build_audit_js(&["forms".into()]);
        assert!(
            js.contains("FORMS-6"),
            "FORMS-6 sign-in autocomplete rule missing"
        );
        assert!(
            js.contains("Sign-in form has input without autocomplete hint"),
            "rule should flag sign-in forms missing autocomplete hints"
        );
    }

    #[test]
    fn build_js_links_has_noreferrer_rule() {
        let js = build_audit_js(&["links".into()]);
        assert!(js.contains("LINKS-4"), "LINKS-4 noreferrer rule missing");
        assert!(
            js.contains("noreferrer"),
            "LINKS-4 should reference the rel=noreferrer token"
        );
    }

    #[test]
    fn build_js_all_new_rules_register_categories() {
        let js = build_audit_js(&default_categories());
        for marker in ["A11Y-6", "FORMS-5", "FORMS-6", "LINKS-4"] {
            assert!(
                js.contains(marker),
                "all-categories build must include {marker}"
            );
        }
    }

    #[test]
    fn parse_audit_result_counts_severities() {
        let raw = vec![
            RawAuditIssue {
                category: "a11y".into(),
                severity: "critical".into(),
                element: "img#hero".into(),
                message: "Missing alt".into(),
                suggestion: "Add alt".into(),
            },
            RawAuditIssue {
                category: "a11y".into(),
                severity: "warning".into(),
                element: "input[name=email]".into(),
                message: "No label".into(),
                suggestion: "Add label".into(),
            },
            RawAuditIssue {
                category: "forms".into(),
                severity: "info".into(),
                element: "input[name=pass]".into(),
                message: "No autocomplete".into(),
                suggestion: "Add autocomplete".into(),
            },
        ];

        let result = parse_audit_result("https://example.com", raw);
        assert_eq!(result.url, "https://example.com");
        assert_eq!(result.issues.len(), 3);
        assert_eq!(result.summary.critical, 1);
        assert_eq!(result.summary.warning, 1);
        assert_eq!(result.summary.info, 1);
        // Each unique issue has count=1.
        assert!(result.issues.iter().all(|i| i.count == 1));
    }

    #[test]
    fn parse_audit_result_empty() {
        let result = parse_audit_result("https://example.com", vec![]);
        assert!(result.issues.is_empty());
        assert_eq!(result.summary.critical, 0);
        assert_eq!(result.summary.warning, 0);
        assert_eq!(result.summary.info, 0);
    }

    #[test]
    fn dedup_identical_issues() {
        // Simulate 25 identical href=# warnings (like chaos.html).
        let raw: Vec<RawAuditIssue> = (0..25)
            .map(|i| RawAuditIssue {
                category: "links".into(),
                severity: "warning".into(),
                element: format!("a.link-{i}"),
                message: r##"Link with non-navigational href ("#")"##.into(),
                suggestion: "Use a <button> instead, or provide a real URL".into(),
            })
            .collect();

        let result = parse_audit_result("https://chaos.html", raw);
        // 25 identical issues should collapse into 1 entry.
        assert_eq!(result.issues.len(), 1, "should deduplicate to 1 issue");
        assert_eq!(result.issues[0].count, 25);
        assert_eq!(result.issues[0].category, "links");
        // Summary counts deduplicated issues, not raw.
        assert_eq!(result.summary.warning, 1);
    }

    #[test]
    fn dedup_mixed_issues() {
        let raw = vec![
            RawAuditIssue {
                category: "links".into(),
                severity: "warning".into(),
                element: "a.nav-1".into(),
                message: "missing noopener".into(),
                suggestion: "add rel".into(),
            },
            RawAuditIssue {
                category: "links".into(),
                severity: "warning".into(),
                element: "a.nav-2".into(),
                message: "missing noopener".into(),
                suggestion: "add rel".into(),
            },
            RawAuditIssue {
                category: "a11y".into(),
                severity: "critical".into(),
                element: "img#hero".into(),
                message: "Missing alt".into(),
                suggestion: "Add alt".into(),
            },
        ];

        let result = parse_audit_result("https://example.com", raw);
        assert_eq!(result.issues.len(), 2, "2 unique (cat, msg) pairs");
        // First group: noopener (count=2)
        assert_eq!(result.issues[0].count, 2);
        assert_eq!(result.issues[0].message, "missing noopener");
        // Second group: alt (count=1)
        assert_eq!(result.issues[1].count, 1);
        assert_eq!(result.issues[1].message, "Missing alt");
        // Summary: 1 warning + 1 critical (deduplicated)
        assert_eq!(result.summary.warning, 1);
        assert_eq!(result.summary.critical, 1);
    }

    #[test]
    fn dedup_count_omitted_in_json_when_one() {
        let issue = AuditIssue {
            category: "a11y".into(),
            severity: Severity::Warning,
            element: "img".into(),
            message: "test".into(),
            suggestion: "fix".into(),
            count: 1,
        };
        let json = serde_json::to_string(&issue).unwrap();
        assert!(
            !json.contains("count"),
            "count=1 should be omitted from JSON"
        );
    }

    #[test]
    fn dedup_count_present_in_json_when_many() {
        let issue = AuditIssue {
            category: "links".into(),
            severity: Severity::Warning,
            element: "a".into(),
            message: "test".into(),
            suggestion: "fix".into(),
            count: 25,
        };
        let json = serde_json::to_string(&issue).unwrap();
        assert!(
            json.contains(r#""count":25"#),
            "count>1 should appear in JSON"
        );
    }

    #[test]
    fn default_categories_has_three() {
        let cats = default_categories();
        assert_eq!(cats.len(), 3);
        assert!(cats.contains(&"a11y".to_string()));
        assert!(cats.contains(&"forms".to_string()));
        assert!(cats.contains(&"links".to_string()));
    }

    #[test]
    fn severity_serialization() {
        assert_eq!(
            serde_json::to_string(&Severity::Critical).unwrap(),
            "\"critical\""
        );
        assert_eq!(
            serde_json::to_string(&Severity::Warning).unwrap(),
            "\"warning\""
        );
        assert_eq!(serde_json::to_string(&Severity::Info).unwrap(), "\"info\"");
    }
}
