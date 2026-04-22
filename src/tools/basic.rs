//! Arbitrary-precision arithmetic (Java `BigDecimal` parity).
//!
//! Each public function returns a fully formatted response envelope — inline
//! for success, three-line block for errors. Values are parsed as
//! `BigDecimal` so `0.1 + 0.2` yields `0.3`, not `0.30000000000000004`.

use std::str::FromStr;

use bigdecimal::{BigDecimal, RoundingMode};
use num_traits::Zero;

use crate::engine::bigdecimal_ext::{DIVISION_SCALE, strip_plain};
use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};

const TOOL_ADD: &str = "ADD";
const TOOL_SUBTRACT: &str = "SUBTRACT";
const TOOL_MULTIPLY: &str = "MULTIPLY";
const TOOL_DIVIDE: &str = "DIVIDE";
const TOOL_POWER: &str = "POWER";
const TOOL_MODULO: &str = "MODULO";
const TOOL_ABS: &str = "ABS";

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

pub fn add(first: &str, second: &str) -> String {
    let lhs = match parse_or_error(TOOL_ADD, "first", first) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let rhs = match parse_or_error(TOOL_ADD, "second", second) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ok_result(TOOL_ADD, &(&lhs + &rhs).to_plain_string())
}

pub fn subtract(first: &str, second: &str) -> String {
    let lhs = match parse_or_error(TOOL_SUBTRACT, "first", first) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let rhs = match parse_or_error(TOOL_SUBTRACT, "second", second) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ok_result(TOOL_SUBTRACT, &(&lhs - &rhs).to_plain_string())
}

pub fn multiply(first: &str, second: &str) -> String {
    let lhs = match parse_or_error(TOOL_MULTIPLY, "first", first) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let rhs = match parse_or_error(TOOL_MULTIPLY, "second", second) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ok_result(TOOL_MULTIPLY, &(&lhs * &rhs).to_plain_string())
}

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
        return error(TOOL_DIVIDE, ErrorCode::DivisionByZero, "cannot divide by zero");
    }
    let quotient = (&dividend / &divisor).with_scale_round(DIVISION_SCALE, RoundingMode::HalfUp);
    ok_result(TOOL_DIVIDE, &strip_plain(&quotient))
}

pub fn power(base: &str, exponent: &str) -> String {
    let base_value = match parse_or_error(TOOL_POWER, "base", base) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let exp: u32 = match exponent.parse() {
        Ok(e) => e,
        Err(_) => {
            return error_with_detail(
                TOOL_POWER,
                ErrorCode::InvalidInput,
                "exponent must be a non-negative integer",
                &format!("exponent={exponent}"),
            );
        }
    };
    // 0^0 is conventionally 1 (combinatorial identity, IEEE-754, Python,
    // JavaScript, and most CAS systems). `BigDecimal::powi` returns 0^0=0,
    // so we short-circuit here to match the accepted convention.
    if exp == 0 {
        return ok_result(TOOL_POWER, "1");
    }
    ok_result(TOOL_POWER, &base_value.powi(i64::from(exp)).to_plain_string())
}

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
    ok_result(TOOL_MODULO, &(&dividend % &divisor).to_plain_string())
}

pub fn abs(value: &str) -> String {
    match parse_or_error(TOOL_ABS, "value", value) {
        Ok(v) => ok_result(TOOL_ABS, &v.abs().to_plain_string()),
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
    fn power_negative_exponent_rejected() {
        assert_eq!(
            power("2", "-1"),
            "POWER: ERROR\nREASON: [INVALID_INPUT] exponent must be a non-negative integer\nDETAIL: exponent=-1"
        );
    }

    #[test]
    fn power_non_integer_exponent_rejected() {
        assert!(
            power("2", "1.5").starts_with("POWER: ERROR\nREASON: [INVALID_INPUT]"),
        );
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
