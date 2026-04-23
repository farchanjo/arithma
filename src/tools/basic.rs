//! Arbitrary-precision arithmetic (Java `BigDecimal` parity).
//!
//! Each public function returns a fully formatted response envelope — inline
//! for success, three-line block for errors. Values are parsed as
//! `BigDecimal` so `0.1 + 0.2` yields `0.3`, not `0.30000000000000004`.

use std::str::FromStr;

use bigdecimal::{BigDecimal, RoundingMode};
use num_traits::Zero;

use std::num::NonZeroU64;

use crate::engine::bigdecimal_ext::{DECIMAL128_PRECISION, DIVISION_SCALE, strip_plain};
use crate::engine::expression_exact;
use crate::mcp::message::{
    ErrorCode, Response, error, error_with_detail, expression_error_envelope,
};

const TOOL_ADD: &str = "ADD";
const TOOL_SUBTRACT: &str = "SUBTRACT";
const TOOL_MULTIPLY: &str = "MULTIPLY";
const TOOL_DIVIDE: &str = "DIVIDE";
const TOOL_POWER: &str = "POWER";
const TOOL_MODULO: &str = "MODULO";
const TOOL_ABS: &str = "ABS";

/// Precision for the `divide` fallback when scale-rounding would zero the
/// result. `DECIMAL128_PRECISION` is a compile-time non-zero constant, so the
/// `unwrap` here can never fire at runtime — const-folded into a `NonZeroU64`
/// without a panic site.
const DIVIDE_PRECISION: NonZeroU64 = match NonZeroU64::new(DECIMAL128_PRECISION) {
    Some(p) => p,
    None => unreachable!(),
};

// Cap on the estimated printed length (in characters) of a `power` result.
// Chosen so legitimate arbitrary-precision work is unaffected while rejecting
// exponents that would blow up the MCP response payload (e.g. 2^1_000_000 is
// ~301k digits). The upper bound `len(base) * exp` is loose but safe.
const MAX_POWER_RESULT_LEN: u64 = 10_000;

fn parse_or_error(tool: &str, label: &str, raw: &str) -> Result<BigDecimal, String> {
    BigDecimal::from_str(raw).map_err(|_| {
        error_with_detail(
            tool,
            ErrorCode::ParseError,
            "operand is not a valid decimal number",
            &format!("{label}={raw}"),
        )
    })
}

fn ok_result(tool: &str, value: &str) -> String {
    Response::ok(tool).result(value).build()
}

/// Render a `BigDecimal` while collapsing exact zero to `"0"`.
///
/// `BigDecimal` preserves scale through arithmetic, so `-0.0 * 12.34567891` keeps
/// the factor's 8-digit scale and prints as `"0.00000000"`. That's cosmetically
/// wrong — the result is exactly zero — and confuses downstream parsers. For
/// non-zero results, scale is preserved (so `2.5 * 2 = "5.0"` still holds).
fn format_bd(value: &BigDecimal) -> String {
    if value.is_zero() {
        "0".to_string()
    } else {
        value.to_plain_string()
    }
}

#[must_use]
pub fn add(first: &str, second: &str) -> String {
    let lhs = match parse_or_error(TOOL_ADD, "first", first) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let rhs = match parse_or_error(TOOL_ADD, "second", second) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ok_result(TOOL_ADD, &format_bd(&(&lhs + &rhs)))
}

#[must_use]
pub fn subtract(first: &str, second: &str) -> String {
    let lhs = match parse_or_error(TOOL_SUBTRACT, "first", first) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let rhs = match parse_or_error(TOOL_SUBTRACT, "second", second) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ok_result(TOOL_SUBTRACT, &format_bd(&(&lhs - &rhs)))
}

#[must_use]
pub fn multiply(first: &str, second: &str) -> String {
    let lhs = match parse_or_error(TOOL_MULTIPLY, "first", first) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let rhs = match parse_or_error(TOOL_MULTIPLY, "second", second) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ok_result(TOOL_MULTIPLY, &format_bd(&(&lhs * &rhs)))
}

#[must_use]
pub fn divide(first: &str, second: &str) -> String {
    let dividend = match parse_or_error(TOOL_DIVIDE, "first", first) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let divisor = match parse_or_error(TOOL_DIVIDE, "second", second) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if divisor.is_zero() {
        return error(
            TOOL_DIVIDE,
            ErrorCode::DivisionByZero,
            "cannot divide by zero",
        );
    }
    let raw = &dividend / &divisor;
    let quotient = raw.with_scale_round(DIVISION_SCALE, RoundingMode::HalfUp);
    // Scale-rounding at 20 decimal places preserves the documented
    // "20-digit precision" behaviour for everyday ratios (`10/3 =
    // 3.33333333333333333333`), but silently truncates sub-`10⁻²⁰` values
    // like `1/1e25` to a misleading `0`. When that happens — dividend is
    // non-zero but scale-rounding collapsed the quotient — fall back to
    // precision-based rounding so the caller still sees the real magnitude.
    let final_quotient = if quotient.is_zero() && !dividend.is_zero() {
        (&dividend / &divisor).with_precision_round(DIVIDE_PRECISION, RoundingMode::HalfUp)
    } else {
        quotient
    };
    ok_result(TOOL_DIVIDE, &strip_plain(&final_quotient))
}

#[must_use]
pub fn power(base: &str, exponent: &str) -> String {
    let base_value = match parse_or_error(TOOL_POWER, "base", base) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // Fast path: non-negative integer exponent keeps exact BigDecimal
    // arithmetic with `powi`. Fractional or negative exponents fall back to
    // the 128-bit exact evaluator so callers can express things like
    // `2^0.5` or `2^-3` without having to know which entry point to use.
    if let Ok(exp) = exponent.parse::<u32>() {
        // 0^0 is conventionally 1 (combinatorial identity, IEEE-754, Python,
        // JavaScript, and most CAS systems). `BigDecimal::powi` returns 0^0=0,
        // so we short-circuit here to match the accepted convention.
        if exp == 0 {
            return ok_result(TOOL_POWER, "1");
        }
        if base_value.is_zero() {
            return ok_result(TOOL_POWER, "0");
        }
        let base_len = base_value.to_plain_string().len() as u64;
        let estimated_len = base_len.saturating_mul(u64::from(exp));
        if estimated_len > MAX_POWER_RESULT_LEN {
            return error_with_detail(
                TOOL_POWER,
                ErrorCode::Overflow,
                "exponent would produce a result that exceeds the maximum output size",
                &format!("estimated_digits={estimated_len}, max={MAX_POWER_RESULT_LEN}"),
            );
        }
        return ok_result(TOOL_POWER, &format_bd(&base_value.powi(i64::from(exp))));
    }
    // Fallback: real/negative exponent via the exact evaluator.
    let expression = format!("({base}) ^ ({exponent})");
    match expression_exact::evaluate(&expression) {
        Ok(result) => ok_result(TOOL_POWER, &result),
        Err(err) => expression_error_envelope(TOOL_POWER, &err),
    }
}

#[must_use]
pub fn modulo(first: &str, second: &str) -> String {
    let dividend = match parse_or_error(TOOL_MODULO, "first", first) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let divisor = match parse_or_error(TOOL_MODULO, "second", second) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if divisor.is_zero() {
        return error(
            TOOL_MODULO,
            ErrorCode::DivisionByZero,
            "cannot take modulo by zero",
        );
    }
    ok_result(TOOL_MODULO, &format_bd(&(&dividend % &divisor)))
}

#[must_use]
pub fn abs(value: &str) -> String {
    match parse_or_error(TOOL_ABS, "value", value) {
        Ok(v) => ok_result(TOOL_ABS, &format_bd(&v.abs())),
        Err(e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_integers() {
        assert_eq!(add("1", "2"), "ADD: OK | RESULT: 3");
    }

    #[test]
    fn add_avoids_binary_float_drift() {
        assert_eq!(add("0.1", "0.2"), "ADD: OK | RESULT: 0.3");
    }

    #[test]
    fn subtract_integers() {
        assert_eq!(subtract("10", "3"), "SUBTRACT: OK | RESULT: 7");
    }

    #[test]
    fn multiply_integers() {
        assert_eq!(multiply("3", "4"), "MULTIPLY: OK | RESULT: 12");
    }

    #[test]
    fn multiply_preserves_decimal_scale() {
        assert_eq!(multiply("2.5", "2"), "MULTIPLY: OK | RESULT: 5.0");
    }

    #[test]
    fn multiply_by_zero_collapses_cosmetic_trailing_zeros() {
        // Regression: `-0.0 * 999999999999999.999999999999` used to render as
        // `"0.0000000000000"` — BigDecimal preserves the scale of the non-zero
        // factor through a zero multiplicand. Exact zero must print as `"0"`.
        assert_eq!(
            multiply("-0.0", "999999999999999.999999999999"),
            "MULTIPLY: OK | RESULT: 0"
        );
        assert_eq!(multiply("0", "1.23456789"), "MULTIPLY: OK | RESULT: 0");
    }

    #[test]
    fn modulo_exact_zero_collapses_scale() {
        // 6.0 % 2 has mantissa 0 with scale 1 → used to render as "0.0".
        assert_eq!(modulo("6.0", "2"), "MODULO: OK | RESULT: 0");
    }

    #[test]
    fn add_exact_zero_collapses_scale() {
        assert_eq!(add("1.5", "-1.5"), "ADD: OK | RESULT: 0");
    }

    #[test]
    fn divide_twenty_digit_precision() {
        assert_eq!(
            divide("10", "3"),
            "DIVIDE: OK | RESULT: 3.33333333333333333333"
        );
    }

    #[test]
    fn divide_strips_trailing_zeros() {
        assert_eq!(divide("1", "2"), "DIVIDE: OK | RESULT: 0.5");
        assert_eq!(divide("10", "2"), "DIVIDE: OK | RESULT: 5");
    }

    #[test]
    fn divide_by_zero_returns_error_envelope() {
        assert_eq!(
            divide("10", "0"),
            "DIVIDE: ERROR\nREASON: [DIVISION_BY_ZERO] cannot divide by zero"
        );
    }

    #[test]
    fn divide_preserves_sub_scale_quotients() {
        // Regression: scale-rounding at 20 decimal places truncated
        // `1 / 1e25 = 1e-25` to `0`. With the precision-based fallback, a
        // non-zero dividend always surfaces the true quotient magnitude.
        let out = divide("1", "1e25");
        assert!(out.starts_with("DIVIDE: OK"), "got {out}");
        assert!(!out.ends_with(" RESULT: 0"), "got {out}");
        let anchor = out.find("RESULT: ").unwrap() + "RESULT: ".len();
        let rest = out[anchor..].trim();
        let val: f64 = rest.parse().expect("parses as f64");
        assert!(val > 1e-26 && val < 1e-24, "val={val}, got {out}");

        // A tiny dividend / unit divisor hits the same guard.
        let out2 = divide("1e-30", "1");
        assert!(!out2.ends_with(" RESULT: 0"), "got {out2}");

        // Everyday ratios still return the documented 20-decimal form —
        // the fallback only fires when scale-rounding would lose info.
        assert_eq!(
            divide("10", "3"),
            "DIVIDE: OK | RESULT: 3.33333333333333333333"
        );
    }

    #[test]
    fn power_integer_base() {
        assert_eq!(power("2", "10"), "POWER: OK | RESULT: 1024");
    }

    #[test]
    fn power_zero_exponent_is_one() {
        // Regression: previously returned 0 for 0^0. Any finite base with
        // exponent 0 is 1 by convention.
        assert_eq!(power("0", "0"), "POWER: OK | RESULT: 1");
        assert_eq!(power("5", "0"), "POWER: OK | RESULT: 1");
        assert_eq!(power("-3.14", "0"), "POWER: OK | RESULT: 1");
    }

    #[test]
    fn power_negative_exponent_falls_back_to_exact() {
        // 2^-3 = 1/8 = 0.125 — the exact evaluator handles reciprocal powers.
        assert_eq!(power("2", "-3"), "POWER: OK | RESULT: 0.125");
        assert_eq!(power("2", "-1"), "POWER: OK | RESULT: 0.5");
    }

    #[test]
    fn power_fractional_exponent_falls_back_to_exact() {
        // 2^0.5 = √2 at 128-bit precision. Compare only the 15-digit prefix
        // to stay robust across astro-float precision tweaks.
        let out = power("2", "0.5");
        assert!(
            out.starts_with("POWER: OK | RESULT: 1.41421356237"),
            "got {out}"
        );
    }

    #[test]
    fn power_fractional_exponent_negative_base_domain_errors() {
        // (-2)^0.5 has no real value — exact evaluator returns DOMAIN_ERROR.
        let out = power("-2", "0.5");
        assert!(out.contains("POWER: ERROR"), "got {out}");
    }

    #[test]
    fn power_rejects_exponent_that_would_exceed_output_cap() {
        // Regression: 2^1_000_000 previously produced a ~301k-character
        // payload that blew past MCP client token limits. We now reject any
        // exponent whose estimated output length exceeds MAX_POWER_RESULT_LEN.
        let out = power("2", "1000000");
        assert!(
            out.starts_with("POWER: ERROR\nREASON: [OVERFLOW]"),
            "unexpected: {out}"
        );
    }

    #[test]
    fn power_allows_trivial_bases_with_large_exponent() {
        assert_eq!(power("0", "1000000"), "POWER: OK | RESULT: 0");
    }

    #[test]
    fn modulo_integers() {
        assert_eq!(modulo("10", "3"), "MODULO: OK | RESULT: 1");
    }

    #[test]
    fn modulo_by_zero() {
        assert_eq!(
            modulo("5", "0"),
            "MODULO: ERROR\nREASON: [DIVISION_BY_ZERO] cannot take modulo by zero"
        );
    }

    #[test]
    fn abs_negative() {
        assert_eq!(abs("-5"), "ABS: OK | RESULT: 5");
    }

    #[test]
    fn abs_preserves_scale() {
        assert_eq!(abs("3.14"), "ABS: OK | RESULT: 3.14");
    }

    #[test]
    fn parse_error_reports_label_and_value() {
        assert_eq!(
            add("abc", "1"),
            "ADD: ERROR\nREASON: [PARSE_ERROR] operand is not a valid decimal number\nDETAIL: first=abc"
        );
        assert_eq!(
            subtract("1", "xyz"),
            "SUBTRACT: ERROR\nREASON: [PARSE_ERROR] operand is not a valid decimal number\nDETAIL: second=xyz"
        );
    }
}
