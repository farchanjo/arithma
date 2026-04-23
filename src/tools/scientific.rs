//! Transcendentals and factorial — formatted through the canonical envelope.
//!
//! Preserves the exact-table behavior for notable trig angles. Non-exact
//! results render through `{value:?}` (Java `String.valueOf(double)` parity).

use std::collections::HashMap;
use std::sync::LazyLock;

use num_bigint::BigUint;
use num_traits::One;

use crate::mcp::message::{ErrorCode, Response, error_with_detail};

const TOOL_SQRT: &str = "SQRT";
const TOOL_LOG: &str = "LOG";
const TOOL_LOG10: &str = "LOG10";
const TOOL_FACTORIAL: &str = "FACTORIAL";
const TOOL_SIN: &str = "SIN";
const TOOL_COS: &str = "COS";
const TOOL_TAN: &str = "TAN";

const FULL_CIRCLE: i32 = 360;

static SQRT2_OVER_2: LazyLock<f64> = LazyLock::new(|| (2.0_f64).sqrt() / 2.0);
static SQRT3_OVER_2: LazyLock<f64> = LazyLock::new(|| (3.0_f64).sqrt() / 2.0);
static SQRT3: LazyLock<f64> = LazyLock::new(|| (3.0_f64).sqrt());
static ONE_OVER_SQRT3: LazyLock<f64> = LazyLock::new(|| 1.0 / (3.0_f64).sqrt());

static SIN_TABLE: LazyLock<HashMap<i32, f64>> = LazyLock::new(|| {
    HashMap::from([
        (0, 0.0),
        (180, 0.0),
        (30, 0.5),
        (150, 0.5),
        (45, *SQRT2_OVER_2),
        (135, *SQRT2_OVER_2),
        (60, *SQRT3_OVER_2),
        (120, *SQRT3_OVER_2),
        (90, 1.0),
        (210, -0.5),
        (330, -0.5),
        (225, -*SQRT2_OVER_2),
        (315, -*SQRT2_OVER_2),
        (240, -*SQRT3_OVER_2),
        (300, -*SQRT3_OVER_2),
        (270, -1.0),
    ])
});

static COS_TABLE: LazyLock<HashMap<i32, f64>> = LazyLock::new(|| {
    HashMap::from([
        (0, 1.0),
        (30, *SQRT3_OVER_2),
        (330, *SQRT3_OVER_2),
        (45, *SQRT2_OVER_2),
        (315, *SQRT2_OVER_2),
        (60, 0.5),
        (300, 0.5),
        (90, 0.0),
        (270, 0.0),
        (120, -0.5),
        (240, -0.5),
        (135, -*SQRT2_OVER_2),
        (225, -*SQRT2_OVER_2),
        (150, -*SQRT3_OVER_2),
        (210, -*SQRT3_OVER_2),
        (180, -1.0),
    ])
});

static TAN_TABLE: LazyLock<HashMap<i32, f64>> = LazyLock::new(|| {
    HashMap::from([
        (0, 0.0),
        (180, 0.0),
        (30, *ONE_OVER_SQRT3),
        (210, *ONE_OVER_SQRT3),
        (45, 1.0),
        (225, 1.0),
        (60, *SQRT3),
        (240, *SQRT3),
        (120, -*SQRT3),
        (300, -*SQRT3),
        (135, -1.0),
        (315, -1.0),
        (150, -*ONE_OVER_SQRT3),
        (330, -*ONE_OVER_SQRT3),
    ])
});

/// Convert `degrees` to an `i32` only when it represents an exact integer.
///
/// Uses a tolerance-based comparison instead of `==` to stay within
/// clippy's float-equality rules, and routes the cast through `NumCast` so
/// no truncating `as` conversion is performed.
fn integer_degrees(degrees: f64) -> Option<i32> {
    if !degrees.is_finite() {
        return None;
    }
    let floored = degrees.floor();
    if (degrees - floored).abs() > f64::EPSILON {
        return None;
    }
    <i32 as num_traits::NumCast>::from(floored)
}

fn normalized_degrees(degrees: f64) -> Option<i32> {
    integer_degrees(degrees).map(|d| {
        let angle = d % FULL_CIRCLE;
        if angle < 0 {
            angle + FULL_CIRCLE
        } else {
            angle
        }
    })
}

fn exact_lookup(table: &HashMap<i32, f64>, degrees: f64) -> Option<f64> {
    normalized_degrees(degrees).and_then(|angle| table.get(&angle).copied())
}

/// Compute square root of a number.
#[must_use]
pub fn sqrt(number: f64) -> String {
    if number < 0.0 {
        return error_with_detail(
            TOOL_SQRT,
            ErrorCode::DomainError,
            "square root is undefined for negative numbers",
            &format!("number={number:?}"),
        );
    }
    Response::ok(TOOL_SQRT)
        .result(format!("{:?}", number.sqrt()))
        .build()
}

/// Compute natural logarithm (ln) of a number.
#[must_use]
pub fn log(number: f64) -> String {
    if number <= 0.0 {
        return error_with_detail(
            TOOL_LOG,
            ErrorCode::DomainError,
            "natural logarithm is undefined for non-positive numbers",
            &format!("number={number:?}"),
        );
    }
    Response::ok(TOOL_LOG)
        .result(format!("{:?}", number.ln()))
        .build()
}

/// Compute base-10 logarithm of a number.
#[must_use]
pub fn log10(number: f64) -> String {
    if number <= 0.0 {
        return error_with_detail(
            TOOL_LOG10,
            ErrorCode::DomainError,
            "base-10 logarithm is undefined for non-positive numbers",
            &format!("number={number:?}"),
        );
    }
    Response::ok(TOOL_LOG10)
        .result(format!("{:?}", number.log10()))
        .build()
}

/// Upper bound on factorial input. `1000!` has 2 568 digits, comfortably below
/// the MCP envelope cap; anything larger produces payloads that are more
/// wasteful than useful over stdio. The lower-bound cap (u64) used to truncate
/// at `20!`; `BigUint` removes that artificial limit entirely.
const MAX_FACTORIAL_INPUT: i64 = 1_000;

/// Compute factorial (n!) for integers in `[0, 1000]`.
///
/// Uses arbitrary-precision integers: `20!` fits in `u64`, anything larger
/// would overflow a primitive, so the computation runs through [`BigUint`]
/// end-to-end.
///
/// # Panics
///
/// Panics only if the internal `u64::try_from` fails on a value already
/// validated to be within `[0, MAX_FACTORIAL_INPUT]` — impossible in practice.
/// Left as an `expect` instead of a silent `as` cast so any future contract
/// violation is loud.
#[must_use]
pub fn factorial(num: i64) -> String {
    if !(0..=MAX_FACTORIAL_INPUT).contains(&num) {
        return error_with_detail(
            TOOL_FACTORIAL,
            ErrorCode::OutOfRange,
            &format!("factorial is defined for integers 0..={MAX_FACTORIAL_INPUT}"),
            &format!("n={num}"),
        );
    }
    let mut value = BigUint::one();
    for idx in 2..=num {
        // `idx` is in `[2, MAX_FACTORIAL_INPUT]` — well within u64 — so the
        // `try_from` path is infallible and avoids silent lossy casts.
        let factor = u64::try_from(idx).expect("factorial input fits in u64");
        value *= factor;
    }
    Response::ok(TOOL_FACTORIAL)
        .result(value.to_string())
        .build()
}

/// Compute sine of an angle in degrees.
#[must_use]
pub fn sin(degrees: f64) -> String {
    let value = exact_lookup(&SIN_TABLE, degrees).unwrap_or_else(|| degrees.to_radians().sin());
    Response::ok(TOOL_SIN).result(format!("{value:?}")).build()
}

/// Compute cosine of an angle in degrees.
#[must_use]
pub fn cos(degrees: f64) -> String {
    let value = exact_lookup(&COS_TABLE, degrees).unwrap_or_else(|| degrees.to_radians().cos());
    Response::ok(TOOL_COS).result(format!("{value:?}")).build()
}

/// Compute tangent of an angle in degrees.
pub fn tan(degrees: f64) -> String {
    if let Some(as_int) = integer_degrees(degrees) {
        let normalized = {
            let angle = as_int % FULL_CIRCLE;
            if angle < 0 {
                angle + FULL_CIRCLE
            } else {
                angle
            }
        };
        if normalized == 90 || normalized == 270 {
            return error_with_detail(
                TOOL_TAN,
                ErrorCode::DomainError,
                "tangent is undefined at 90 and 270 degrees (vertical asymptote)",
                &format!("degrees={as_int}"),
            );
        }
        if let Some(exact) = TAN_TABLE.get(&normalized).copied() {
            return Response::ok(TOOL_TAN).result(format!("{exact:?}")).build();
        }
    }
    Response::ok(TOOL_TAN)
        .result(format!("{:?}", degrees.to_radians().tan()))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- sqrt ----

    #[test]
    fn sqrt_perfect_square() {
        assert_eq!(sqrt(16.0), "SQRT: OK | RESULT: 4.0");
    }

    #[test]
    fn sqrt_irrational() {
        assert_eq!(sqrt(2.0), "SQRT: OK | RESULT: 1.4142135623730951");
    }

    #[test]
    fn sqrt_negative_reports_error() {
        assert_eq!(
            sqrt(-1.0),
            "SQRT: ERROR\nREASON: [DOMAIN_ERROR] square root is undefined for negative numbers\nDETAIL: number=-1.0"
        );
    }

    // ---- log ----

    #[test]
    fn log_of_e_is_one() {
        assert_eq!(log(std::f64::consts::E), "LOG: OK | RESULT: 1.0");
    }

    #[test]
    fn log_zero_reports_error() {
        assert_eq!(
            log(0.0),
            "LOG: ERROR\nREASON: [DOMAIN_ERROR] natural logarithm is undefined for non-positive numbers\nDETAIL: number=0.0"
        );
    }

    #[test]
    fn log_negative_reports_error() {
        assert_eq!(
            log(-1.0),
            "LOG: ERROR\nREASON: [DOMAIN_ERROR] natural logarithm is undefined for non-positive numbers\nDETAIL: number=-1.0"
        );
    }

    // ---- log10 ----

    #[test]
    fn log10_hundred() {
        assert_eq!(log10(100.0), "LOG10: OK | RESULT: 2.0");
    }

    #[test]
    fn log10_thousand() {
        assert_eq!(log10(1000.0), "LOG10: OK | RESULT: 3.0");
    }

    #[test]
    fn log10_zero_reports_error() {
        assert_eq!(
            log10(0.0),
            "LOG10: ERROR\nREASON: [DOMAIN_ERROR] base-10 logarithm is undefined for non-positive numbers\nDETAIL: number=0.0"
        );
    }

    // ---- factorial ----

    #[test]
    fn factorial_zero() {
        assert_eq!(factorial(0), "FACTORIAL: OK | RESULT: 1");
    }

    #[test]
    fn factorial_one() {
        assert_eq!(factorial(1), "FACTORIAL: OK | RESULT: 1");
    }

    #[test]
    fn factorial_five() {
        assert_eq!(factorial(5), "FACTORIAL: OK | RESULT: 120");
    }

    #[test]
    fn factorial_twenty() {
        assert_eq!(factorial(20), "FACTORIAL: OK | RESULT: 2432902008176640000");
    }

    #[test]
    fn factorial_negative_reports_error() {
        assert_eq!(
            factorial(-1),
            "FACTORIAL: ERROR\nREASON: [OUT_OF_RANGE] factorial is defined for integers 0..=1000\nDETAIL: n=-1"
        );
    }

    #[test]
    fn factorial_above_range_reports_error() {
        assert_eq!(
            factorial(1001),
            "FACTORIAL: ERROR\nREASON: [OUT_OF_RANGE] factorial is defined for integers 0..=1000\nDETAIL: n=1001"
        );
    }

    #[test]
    fn factorial_twenty_one_now_exact_past_u64() {
        // Regression: used to error because `u64` overflowed at 21!. With
        // BigUint the answer is the exact 19-digit 51_090_942_171_709_440_000.
        assert_eq!(
            factorial(21),
            "FACTORIAL: OK | RESULT: 51090942171709440000"
        );
    }

    #[test]
    fn factorial_fifty_digits_match_reference() {
        // 50! = 30414093201713378043612608166064768844377641568960512000000000000
        let out = factorial(50);
        assert!(
            out.contains("30414093201713378043612608166064768844377641568960512000000000000"),
            "got: {out}"
        );
    }

    #[test]
    fn factorial_hundred_ends_with_expected_trailing_zeros() {
        // 100! ends in 24 trailing zeros (Legendre's formula: floor(100/5) +
        // floor(100/25) + … = 24).
        let out = factorial(100);
        assert!(
            out.starts_with("FACTORIAL: OK | RESULT: 9332621"),
            "got: {out}"
        );
        // Strip the envelope and count trailing zeros on the number.
        let number = out
            .strip_prefix("FACTORIAL: OK | RESULT: ")
            .expect("ok envelope");
        let trailing_zeros = number.chars().rev().take_while(|&c| c == '0').count();
        assert_eq!(trailing_zeros, 24, "got: {out}");
    }

    #[test]
    fn factorial_max_input_fits_in_payload() {
        let out = factorial(MAX_FACTORIAL_INPUT);
        assert!(out.starts_with("FACTORIAL: OK | RESULT: "), "got: {out}");
        let number = out
            .strip_prefix("FACTORIAL: OK | RESULT: ")
            .expect("ok envelope");
        // 1000! has exactly 2568 decimal digits.
        assert_eq!(number.len(), 2568, "got length: {}", number.len());
    }

    // ---- sin ----

    #[test]
    fn sin_zero() {
        assert_eq!(sin(0.0), "SIN: OK | RESULT: 0.0");
    }

    #[test]
    fn sin_thirty() {
        assert_eq!(sin(30.0), "SIN: OK | RESULT: 0.5");
    }

    #[test]
    fn sin_ninety() {
        assert_eq!(sin(90.0), "SIN: OK | RESULT: 1.0");
    }

    #[test]
    fn sin_one_eighty() {
        assert_eq!(sin(180.0), "SIN: OK | RESULT: 0.0");
    }

    #[test]
    fn sin_full_circle_normalizes_to_zero() {
        assert_eq!(sin(360.0), "SIN: OK | RESULT: 0.0");
    }

    #[test]
    fn sin_negative_thirty_normalizes_to_three_thirty() {
        assert_eq!(sin(-30.0), "SIN: OK | RESULT: -0.5");
    }

    #[test]
    fn sin_forty_five_uses_exact_table() {
        let expected = format!("SIN: OK | RESULT: {:?}", *SQRT2_OVER_2);
        assert_eq!(sin(45.0), expected);
    }

    #[test]
    fn sin_non_notable_angle_falls_back() {
        let expected = format!("SIN: OK | RESULT: {:?}", 15.0_f64.to_radians().sin());
        assert_eq!(sin(15.0), expected);
    }

    #[test]
    fn sin_non_integer_angle_falls_back() {
        let expected = format!("SIN: OK | RESULT: {:?}", 45.5_f64.to_radians().sin());
        assert_eq!(sin(45.5), expected);
    }

    // ---- cos ----

    #[test]
    fn cos_zero() {
        assert_eq!(cos(0.0), "COS: OK | RESULT: 1.0");
    }

    #[test]
    fn cos_ninety() {
        assert_eq!(cos(90.0), "COS: OK | RESULT: 0.0");
    }

    #[test]
    fn cos_sixty() {
        assert_eq!(cos(60.0), "COS: OK | RESULT: 0.5");
    }

    #[test]
    fn cos_one_eighty() {
        assert_eq!(cos(180.0), "COS: OK | RESULT: -1.0");
    }

    // ---- tan ----

    #[test]
    fn tan_forty_five() {
        assert_eq!(tan(45.0), "TAN: OK | RESULT: 1.0");
    }

    #[test]
    fn tan_zero() {
        assert_eq!(tan(0.0), "TAN: OK | RESULT: 0.0");
    }

    #[test]
    fn tan_one_eighty() {
        assert_eq!(tan(180.0), "TAN: OK | RESULT: 0.0");
    }

    #[test]
    fn tan_ninety_is_asymptote() {
        assert_eq!(
            tan(90.0),
            "TAN: ERROR\nREASON: [DOMAIN_ERROR] tangent is undefined at 90 and 270 degrees (vertical asymptote)\nDETAIL: degrees=90"
        );
    }

    #[test]
    fn tan_two_seventy_is_asymptote() {
        assert_eq!(
            tan(270.0),
            "TAN: ERROR\nREASON: [DOMAIN_ERROR] tangent is undefined at 90 and 270 degrees (vertical asymptote)\nDETAIL: degrees=270"
        );
    }

    #[test]
    fn tan_negative_two_seventy_is_asymptote() {
        assert_eq!(
            tan(-270.0),
            "TAN: ERROR\nREASON: [DOMAIN_ERROR] tangent is undefined at 90 and 270 degrees (vertical asymptote)\nDETAIL: degrees=-270"
        );
    }
}
