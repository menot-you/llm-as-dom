//! Utility helpers for the pilot module.

/// Escape a string for safe embedding inside a JavaScript single-quoted
/// string literal.
///
/// Handles all characters that could break out of the string context:
/// - Backslash, single quote, double quote, backtick (string delimiters)
/// - `$` (template literal injection via `${}`)
/// - Newline, carriage return (line terminator injection)
/// - Null byte (string truncation in some JS engines)
/// - `</` (prevents `</script>` tag breakout in HTML contexts)
pub fn js_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 8);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '`' => out.push_str("\\`"),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            '<' => {
                // Only escape `</` to prevent `</script>` breakout.
                // Peek-ahead is not needed: we always emit `<\/` for `<`
                // followed by `/` but we process char-by-char. Instead,
                // we escape every `<` that precedes a `/` — but since we
                // only see one char at a time, we use a simpler strategy:
                // always escape `<` as `\\u003c` would be overkill, so we
                // push `<` and let the next char handle `/`.
                out.push('<');
            }
            '/' => {
                // If the previous character was `<`, replace the pair with `<\/`.
                if out.ends_with('<') {
                    out.pop();
                    out.push_str("<\\/");
                } else {
                    out.push('/');
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::js_escape;

    #[test]
    fn escapes_backslash() {
        assert_eq!(js_escape(r"a\b"), r"a\\b");
    }

    #[test]
    fn escapes_single_quote() {
        assert_eq!(js_escape("it's"), "it\\'s");
    }

    #[test]
    fn escapes_double_quote() {
        assert_eq!(js_escape(r#"say "hi""#), r#"say \"hi\""#);
    }

    #[test]
    fn escapes_backtick() {
        assert_eq!(js_escape("foo`bar"), "foo\\`bar");
    }

    #[test]
    fn escapes_dollar_sign() {
        assert_eq!(js_escape("${alert(1)}"), "\\${alert(1)}");
    }

    #[test]
    fn escapes_template_literal_injection() {
        // Full template literal attack: `${...}`
        assert_eq!(
            js_escape("`${document.cookie}`"),
            "\\`\\${document.cookie}\\`"
        );
    }

    #[test]
    fn escapes_newline_and_carriage_return() {
        assert_eq!(js_escape("line1\nline2\rline3"), "line1\\nline2\\rline3");
    }

    #[test]
    fn escapes_null_byte() {
        assert_eq!(js_escape("before\0after"), "before\\0after");
    }

    #[test]
    fn escapes_script_tag_breakout() {
        assert_eq!(js_escape("</script>"), "<\\/script>");
    }

    #[test]
    fn escapes_script_tag_case_variants() {
        // `</SCRIPT>` should also be escaped (same `</` prefix)
        assert_eq!(js_escape("</SCRIPT>"), "<\\/SCRIPT>");
    }

    #[test]
    fn preserves_safe_slash() {
        // A `/` not preceded by `<` should pass through.
        assert_eq!(js_escape("a/b"), "a/b");
    }

    #[test]
    fn preserves_safe_angle_bracket() {
        // A `<` not followed by `/` should pass through.
        assert_eq!(js_escape("<div>"), "<div>");
    }

    #[test]
    fn combined_adversarial_input() {
        let input = "'; alert(1); //";
        let escaped = js_escape(input);
        assert_eq!(escaped, "\\'; alert(1); //");
        // The escaped value, when placed in `'...'`, yields:
        //   '\'; alert(1); //'
        // which is a valid string literal, not an injection.
    }

    #[test]
    fn xss_via_type_value() {
        // Simulates what an LLM might produce as a Type action value.
        let input = "test' + alert(1) + '";
        let escaped = js_escape(input);
        assert_eq!(escaped, "test\\' + alert(1) + \\'");
    }

    #[test]
    fn empty_string() {
        assert_eq!(js_escape(""), "");
    }

    #[test]
    fn plain_text_unchanged() {
        assert_eq!(js_escape("hello world 123"), "hello world 123");
    }

    #[test]
    fn multiple_consecutive_escapes() {
        assert_eq!(js_escape("\\\\''"), "\\\\\\\\\\'\\'");
    }

    #[test]
    fn script_close_mid_string() {
        assert_eq!(js_escape("foo</script>bar"), "foo<\\/script>bar");
    }
}
