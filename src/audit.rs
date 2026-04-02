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

/// A single audit finding.
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
pub fn parse_audit_result(url: &str, raw: Vec<RawAuditIssue>) -> AuditResult {
    let mut summary = AuditSummary::default();
    let issues: Vec<AuditIssue> = raw
        .into_iter()
        .map(|r| {
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

/// Accessibility checks (5 rules).
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
"#;

/// Form checks (4 rules).
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
"#;

/// Link checks (3 rules).
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
