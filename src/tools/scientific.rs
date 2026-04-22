//! Transcendentals and factorial — formatted through the canonical envelope.
//!
//! Preserves the exact-table behavior for notable trig angles. Non-exact
//! results render through `{value:?}` (Java `String.valueOf(double)` parity).

use std::collections::HashMap;
use std::sync::LazyLock;

#[allow(unused_imports)]
use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};

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

fn is_integer_angle(degrees: f64) -> bool {
    degrees == degrees.floor() && degrees.is_finite()
}

fn normalize_angle(degrees: f64) -> i32 {
    let angle = (degrees as i32) % FULL_CIRCLE;
    if angle < 0 {
        angle + FULL_CIRCLE
    } else {
        angle
    }
}

fn exact_lookup(table: &HashMap<i32, f64>, degrees: f64) -> Option<f64> {
    if !is_integer_angle(degrees) {
        return None;
    }
    table.get(&normalize_angle(degrees)).copied()
}

/// Compute square root of a number.
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

/// Compute factorial (n!) for integers in `[0, 20]`.
pub fn factorial(num: i64) -> String {
    if !(0..=20).contains(&num) {
        return error_with_detail(
            TOOL_FACTORIAL,
            ErrorCode::OutOfRange,
            "factorial is defined for integers 0..=20",
            &format!("n={num}"),
        );
    }
    let mut value: u64 = 1;
    for idx in 2..=num {
        value *= idx as u64;
    }
    Response::ok(TOOL_FACTORIAL).result(value.to_string()).build()
}

/// Compute sine of an angle in degrees.
pub fn sin(degrees: f64) -> String {
    let value = exact_lookup(&SIN_TABLE, degrees).unwrap_or_else(|| degrees.to_radians().sin());
    Response::ok(TOOL_SIN).result(format!("{value:?}")).build()
}

/// Compute cosine of an angle in degrees.
pub fn cos(degrees: f64) -> String {
    let value = exact_lookup(&COS_TABLE, degrees).unwrap_or_else(|| degrees.to_radians().cos());
    Response::ok(TOOL_COS).result(format!("{value:?}")).build()
}

/// Compute tangent of an angle in degrees.
pub fn tan(degrees: f64) -> String {
    if is_integer_angle(degrees) {
        let normalized = normalize_angle(degrees);
        if normalized == 90 || normalized == 270 {
            return error_with_detail(
                TOOL_TAN,
                ErrorCode::DomainError,
                "tangent is undefined at 90 and 270 degrees (vertical asymptote)",
                &format!("degrees={}", degrees as i32),
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
        assert_eq!(
            factorial(20),
            "FACTORIAL: OK | RESULT: 2432902008176640000"
        );
    }

    #[test]
    fn factorial_negative_reports_error() {
        assert_eq!(
            factorial(-1),
            "FACTORIAL: ERROR\nREASON: [OUT_OF_RANGE] factorial is defined for integers 0..=20\nDETAIL: n=-1"
        );
    }

    #[test]
    fn factorial_above_range_reports_error() {
        assert_eq!(
            factorial(21),
            "FACTORIAL: ERROR\nREASON: [OUT_OF_RANGE] factorial is defined for integers 0..=20\nDETAIL: n=21"
        );
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
