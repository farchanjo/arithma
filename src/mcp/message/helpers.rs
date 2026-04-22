//! Low-level primitives for emitting the response envelope.
//!
//! Keeps rendering purely textual — no JSON, no structured data. Every field
//! value is sanitized so embedded newlines cannot forge header lines inside
//! the envelope.

/// Escape control characters that would otherwise corrupt the line-oriented
/// envelope: `\n`, `\r`, `\t` become their two-character escape sequences.
#[must_use]
pub fn sanitize_value(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

/// Render the status header line: `TOOL: STATUS`.
#[must_use]
pub fn header(tool: &str, status: &str) -> String {
    format!("{tool}: {status}")
}

/// Render an inline `KEY: value` fragment to append after a header or another
/// fragment (callers add the ` | ` separator).
#[must_use]
pub fn inline_fragment(key: &str, value: &str) -> String {
    format!("{key}: {}", sanitize_value(value))
}

/// Render a block `KEY: value` line.
#[must_use]
pub fn block_line(key: &str, value: &str) -> String {
    format!("{key}: {}", sanitize_value(value))
}

/// Render the three-line error envelope.
///
/// ```text
/// TOOL: ERROR
/// REASON: [CODE] reason text
/// DETAIL: optional context
/// ```
///
/// The `DETAIL` line is omitted when `detail` is `None` or empty after trim.
#[must_use]
pub fn format_error(tool: &str, code: &str, reason: &str, detail: Option<&str>) -> String {
    let mut out = format!(
        "{tool}: ERROR\nREASON: [{code}] {}",
        sanitize_value(reason)
    );
    if let Some(raw) = detail {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            out.push_str("\nDETAIL: ");
            out.push_str(&sanitize_value(trimmed));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_escapes_control_chars() {
        assert_eq!(sanitize_value("a\nb\tc\rd"), "a\\nb\\tc\\rd");
    }

    #[test]
    fn sanitize_leaves_printable_intact() {
        assert_eq!(sanitize_value("hello world | 1 + 2"), "hello world | 1 + 2");
    }

    #[test]
    fn format_error_omits_empty_detail() {
        assert_eq!(
            format_error("FOO", "BAR", "bad thing", None),
            "FOO: ERROR\nREASON: [BAR] bad thing"
        );
        assert_eq!(
            format_error("FOO", "BAR", "bad thing", Some("   ")),
            "FOO: ERROR\nREASON: [BAR] bad thing"
        );
    }

    #[test]
    fn format_error_includes_detail_when_present() {
        assert_eq!(
            format_error("FOO", "BAR", "bad thing", Some("x=5")),
            "FOO: ERROR\nREASON: [BAR] bad thing\nDETAIL: x=5"
        );
    }

    #[test]
    fn format_error_sanitizes_both_reason_and_detail() {
        let out = format_error("FOO", "BAR", "line1\nline2", Some("k=v\nnext"));
        assert_eq!(
            out,
            "FOO: ERROR\nREASON: [BAR] line1\\nline2\nDETAIL: k=v\\nnext"
        );
    }
}
