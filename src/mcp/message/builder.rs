//! Fluent response builder. All tools construct their output through this.

use std::borrow::Cow;

use super::helpers::{block_line, format_error, header, inline_fragment};

// Token budget: inline is the default for *every* successful response. A single
// long line with `K: v | K: v | …` costs one token per separator; a block
// layout spends one token per key AND one newline per field, so block nearly
// doubles the envelope overhead. Block layout is reserved for tabular payloads
// (rows of records) where the caller explicitly opts in with `.block()`.

/// Canonical error codes. Keep the set small; every variant maps to a stable
/// bracketed tag the LLM can detect without substring gymnastics.
#[derive(Debug, Clone, Copy)]
pub enum ErrorCode {
    /// Input inside the function's domain was rejected (sqrt of negative,
    /// log of non-positive, tan at π/2).
    DomainError,
    /// Numeric input outside the supported range (factorial(25), exponent
    /// overflow, etc.).
    OutOfRange,
    /// Divisor was zero where the operation requires a non-zero value.
    DivisionByZero,
    /// A numeric or structural token failed to parse.
    ParseError,
    /// Argument validation failure (missing field, wrong length, malformed
    /// CSV, unknown mode).
    InvalidInput,
    /// Variable referenced inside an expression was not supplied.
    UnknownVariable,
    /// Function referenced inside an expression is not a builtin.
    UnknownFunction,
    /// Result would not fit in the target numeric type.
    Overflow,
    /// Operation intentionally not implemented for this input shape.
    NotImplemented,
}

impl ErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DomainError => "DOMAIN_ERROR",
            Self::OutOfRange => "OUT_OF_RANGE",
            Self::DivisionByZero => "DIVISION_BY_ZERO",
            Self::ParseError => "PARSE_ERROR",
            Self::InvalidInput => "INVALID_INPUT",
            Self::UnknownVariable => "UNKNOWN_VARIABLE",
            Self::UnknownFunction => "UNKNOWN_FUNCTION",
            Self::Overflow => "OVERFLOW",
            Self::NotImplemented => "NOT_IMPLEMENTED",
        }
    }
}

/// Fluent success response builder. Automatically picks inline layout when the
/// payload has at most [`INLINE_MAX_FIELDS`] fields and no explicit block hint.
pub struct Response<'a> {
    tool: &'a str,
    status: Cow<'a, str>,
    fields: Vec<(Cow<'a, str>, Cow<'a, str>)>,
    force_block: bool,
}

impl<'a> Response<'a> {
    /// Start an `OK` response for the given tool name.
    #[must_use]
    pub fn ok(tool: &'a str) -> Self {
        Self {
            tool,
            status: Cow::Borrowed("OK"),
            fields: Vec::new(),
            force_block: false,
        }
    }

    /// Start a response with a custom status code (e.g. `EMPTY`, `NOOP`).
    /// Keep status tokens SCREAMING_SNAKE_CASE.
    #[must_use]
    pub fn status(tool: &'a str, status: impl Into<Cow<'a, str>>) -> Self {
        Self {
            tool,
            status: status.into(),
            fields: Vec::new(),
            force_block: false,
        }
    }

    /// Append a `KEY: value` field. Keys should be SCREAMING_SNAKE_CASE.
    #[must_use]
    pub fn field(
        mut self,
        key: impl Into<Cow<'a, str>>,
        value: impl Into<Cow<'a, str>>,
    ) -> Self {
        self.fields.push((key.into(), value.into()));
        self
    }

    /// Shorthand for a single `RESULT: value` field, the common case for
    /// scalar tools like `add` or `sqrt`.
    #[must_use]
    pub fn result(self, value: impl Into<Cow<'a, str>>) -> Self {
        self.field("RESULT", value)
    }

    /// Opt into block layout. Reserved for tabular payloads where repeated
    /// keys (`ROW_1`, `ROW_2`, …) carry real information — e.g. amortization
    /// schedules or plot samples. For every other response, leave this unset
    /// and let the default inline layout keep the envelope on one line.
    #[must_use]
    pub const fn block(mut self) -> Self {
        self.force_block = true;
        self
    }

    /// Render to the final `String`. Inline layout by default — every field
    /// joins on ` | `. Block layout (one field per line) only when the caller
    /// explicitly requests it via [`Self::block`].
    #[must_use]
    pub fn build(self) -> String {
        let mut out = header(self.tool, &self.status);
        if self.force_block {
            for (key, value) in &self.fields {
                out.push('\n');
                out.push_str(&block_line(key, value));
            }
        } else {
            for (key, value) in &self.fields {
                out.push_str(" | ");
                out.push_str(&inline_fragment(key, value));
            }
        }
        out
    }
}

/// Convenience for the simple error case (no detail line).
#[must_use]
pub fn error(tool: &str, code: ErrorCode, reason: &str) -> String {
    format_error(tool, code.as_str(), reason, None)
}

/// Error with context detail. The detail is stripped/escaped; pass `""` or
/// whitespace to omit the DETAIL line.
#[must_use]
pub fn error_with_detail(tool: &str, code: ErrorCode, reason: &str, detail: &str) -> String {
    format_error(tool, code.as_str(), reason, Some(detail))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_no_fields_is_header_only() {
        assert_eq!(Response::ok("FOO").build(), "FOO: OK");
    }

    #[test]
    fn ok_single_result_inline() {
        assert_eq!(Response::ok("ADD").result("4").build(), "ADD: OK | RESULT: 4");
    }

    #[test]
    fn ok_three_fields_inline() {
        let out = Response::ok("TRIG")
            .field("SIN", "1")
            .field("COS", "0")
            .field("TAN", "inf")
            .build();
        assert_eq!(out, "TRIG: OK | SIN: 1 | COS: 0 | TAN: inf");
    }

    #[test]
    fn ok_many_fields_stay_inline() {
        // Default is inline regardless of count — block is opt-in only.
        let out = Response::ok("OHMS_LAW")
            .field("VOLTAGE", "12")
            .field("CURRENT", "3")
            .field("RESISTANCE", "4")
            .field("POWER", "36")
            .build();
        assert_eq!(
            out,
            "OHMS_LAW: OK | VOLTAGE: 12 | CURRENT: 3 | RESISTANCE: 4 | POWER: 36"
        );
    }

    #[test]
    fn block_override_forces_multiline() {
        let out = Response::ok("AMORT")
            .field("MONTHS", "12")
            .field("ROW_1", "payment=856 | principal=814")
            .block()
            .build();
        assert_eq!(
            out,
            "AMORT: OK\nMONTHS: 12\nROW_1: payment=856 | principal=814"
        );
    }

    #[test]
    fn custom_status() {
        let out = Response::status("SOLVE", "NO_ROOT").field("REASON", "diverged").build();
        assert_eq!(out, "SOLVE: NO_ROOT | REASON: diverged");
    }

    #[test]
    fn error_codes_format_stable() {
        assert_eq!(ErrorCode::DomainError.as_str(), "DOMAIN_ERROR");
        assert_eq!(ErrorCode::DivisionByZero.as_str(), "DIVISION_BY_ZERO");
        let out = error("SQRT", ErrorCode::DomainError, "square root is undefined for negatives");
        assert_eq!(
            out,
            "SQRT: ERROR\nREASON: [DOMAIN_ERROR] square root is undefined for negatives"
        );
    }

    #[test]
    fn error_with_detail_renders_three_lines() {
        let out = error_with_detail(
            "FACTORIAL",
            ErrorCode::OutOfRange,
            "factorial is defined for integers 0..=20",
            "received=25",
        );
        assert_eq!(
            out,
            "FACTORIAL: ERROR\n\
             REASON: [OUT_OF_RANGE] factorial is defined for integers 0..=20\n\
             DETAIL: received=25"
        );
    }

    #[test]
    fn sanitization_applies_in_fields() {
        let out = Response::ok("PRINT").result("a\nb").build();
        assert_eq!(out, "PRINT: OK | RESULT: a\\nb");
    }
}
