//! Port of `CalculusTool.java` — numerical calculus (derivatives and integrals)
//! built on top of [`crate::engine::expression`].
//!
//! Algorithms:
//! * Derivative — five-point central difference.
//! * Nth-order derivative — finite difference with binomial coefficients.
//! * Definite integral — composite Simpson's rule with 10,000 intervals.
//! * Tangent line — derivative combined with function value.
//!
//! Every entry point emits the structured response envelope. Failures coming
//! from the expression evaluator are mapped into canonical [`ErrorCode`]
//! variants that mirror the programmable tool's mapping.

use std::collections::HashMap;

use crate::engine::expression::{ExpressionError, evaluate_with_variables};
use crate::mcp::message::{
    ErrorCode, Response, error_with_detail, expression_error_envelope,
};

const TOOL_DERIVATIVE: &str = "DERIVATIVE";
const TOOL_NTH_DERIVATIVE: &str = "NTH_DERIVATIVE";
const TOOL_DEFINITE_INTEGRAL: &str = "DEFINITE_INTEGRAL";
const TOOL_TANGENT_LINE: &str = "TANGENT_LINE";

const DERIVATIVE_STEP: f64 = 1e-6;
const SIMPSON_INTERVALS: i32 = 10_000;
const MAX_ORDER: i32 = 10;

/// Format an f64 using Rust's debug representation (closest available match to
/// Java's `String.valueOf(double)` — emits `1.0` for whole doubles).
fn format_f64(value: f64) -> String {
    format!("{value:?}")
}

/// Evaluate `expression` with `variable` bound to `value`.
fn eval(expression: &str, variable: &str, value: f64) -> Result<f64, ExpressionError> {
    let mut vars: HashMap<String, f64> = HashMap::with_capacity(1);
    vars.insert(variable.to_string(), value);
    evaluate_with_variables(expression, &vars)
}

/// Map an [`ExpressionError`] into the canonical envelope — delegates to the
/// shared helper so the REASON text stays consistent across every tool that
/// wraps the expression evaluator.
fn map_expression_error(tool: &str, err: &ExpressionError) -> String {
    expression_error_envelope(tool, err)
}

/// Five-point central difference: `f'(x) ≈ (-f(x+2h) + 8f(x+h) - 8f(x-h) + f(x-2h)) / (12h)`.
fn central_difference(
    expression: &str,
    variable: &str,
    point: f64,
    step: f64,
) -> Result<f64, ExpressionError> {
    let two_step = 2.0 * step;
    let f_minus2 = eval(expression, variable, point - two_step)?;
    let f_minus1 = eval(expression, variable, point - step)?;
    let f_plus1 = eval(expression, variable, point + step)?;
    let f_plus2 = eval(expression, variable, point + two_step)?;
    let eight_f_plus1 = 8.0 * f_plus1;
    let eight_f_minus1 = 8.0 * f_minus1;
    let numerator = -f_plus2 + eight_f_plus1 - eight_f_minus1 + f_minus2;
    Ok(numerator / (12.0 * step))
}

fn binomial_coeff(total: i32, choose: i32) -> f64 {
    let mut result: f64 = 1.0;
    let bound = choose.min(total - choose);
    for idx in 0..bound {
        result = result * f64::from(total - idx) / f64::from(idx + 1);
    }
    result
}

/// Finite-difference approximation of the `order`-th derivative at `point`.
fn nth_deriv(
    expression: &str,
    variable: &str,
    point: f64,
    order: i32,
) -> Result<f64, ExpressionError> {
    let step = DERIVATIVE_STEP.powf(1.0 / f64::from(order)) * 10.0;
    let half_n = order / 2;
    let mut result = 0.0;

    for idx in 0..=order {
        let x_sample = point + f64::from(idx - half_n) * step;
        let f_sample = eval(expression, variable, x_sample)?;
        let coeff = binomial_coeff(order, idx) * (-1.0_f64).powi(order - idx);
        result += coeff * f_sample;
    }
    Ok(result / step.powi(order))
}

/// Composite Simpson's rule with `SIMPSON_INTERVALS` subintervals.
fn simpson(
    expression: &str,
    variable: &str,
    lower: f64,
    upper: f64,
) -> Result<f64, ExpressionError> {
    let intervals = SIMPSON_INTERVALS;
    let step = (upper - lower) / f64::from(intervals);
    let mut sum = eval(expression, variable, lower)? + eval(expression, variable, upper)?;

    for idx in 1..intervals {
        let offset = f64::from(idx) * step;
        let x_val = lower + offset;
        let f_val = eval(expression, variable, x_val)?;
        let multiplier = if idx % 2 == 0 { 2.0 } else { 4.0 };
        sum += multiplier * f_val;
    }
    Ok(sum * step / 3.0)
}

// --------------------------------------------------------------------------- //
//  Public tool entry points
// --------------------------------------------------------------------------- //

/// Guard against evaluating a derivative at a point where the underlying
/// function is itself undefined. Central differences only sample `point±h`
/// and `point±2h`, so a pole such as `1/x` at `x=0` used to return a huge
/// but finite spurious value — surface it as a `DOMAIN_ERROR` instead.
///
/// When `eval` fails with `DivisionByZero` or `DomainError` at the point,
/// both are reported uniformly as `DOMAIN_ERROR` with a "singularity" reason
/// — from the caller's perspective, the derivative simply doesn't exist
/// there, and that framing is more useful than the raw evaluator error.
fn ensure_defined_at(
    tool: &str,
    expression: &str,
    variable: &str,
    point: f64,
) -> Result<(), String> {
    let undefined = || {
        error_with_detail(
            tool,
            ErrorCode::DomainError,
            "function is not defined at the evaluation point",
            &format!("{variable}={point}"),
        )
    };
    match eval(expression, variable, point) {
        Ok(v) if v.is_finite() => Ok(()),
        Ok(_) => Err(undefined()),
        Err(ExpressionError::DivisionByZero | ExpressionError::DomainError { .. }) => {
            Err(undefined())
        }
        Err(err) => Err(map_expression_error(tool, &err)),
    }
}

/// Compute the numerical derivative of `expression` w.r.t. `variable` at `point`.
#[must_use]
pub fn derivative(expression: &str, variable: &str, point: f64) -> String {
    let tool = TOOL_DERIVATIVE;
    if let Err(msg) = ensure_defined_at(tool, expression, variable, point) {
        return msg;
    }
    match central_difference(expression, variable, point, DERIVATIVE_STEP) {
        Ok(value) if !value.is_finite() => error_with_detail(
            tool,
            ErrorCode::DomainError,
            "derivative diverges at the evaluation point",
            &format!("{variable}={point}"),
        ),
        Ok(value) => Response::ok(tool).result(format_f64(value)).build(),
        Err(err) => map_expression_error(tool, &err),
    }
}

/// Compute the nth-order numerical derivative. `order` must be in `[1, 10]`.
#[must_use]
pub fn nth_derivative(expression: &str, variable: &str, point: f64, order: i32) -> String {
    let tool = TOOL_NTH_DERIVATIVE;
    if !(1..=MAX_ORDER).contains(&order) {
        return error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            &format!("order must be between 1 and {MAX_ORDER}"),
            &format!("order={order}"),
        );
    }
    if let Err(msg) = ensure_defined_at(tool, expression, variable, point) {
        return msg;
    }
    match nth_deriv(expression, variable, point, order) {
        Ok(value) if !value.is_finite() => error_with_detail(
            tool,
            ErrorCode::DomainError,
            "derivative diverges at the evaluation point",
            &format!("{variable}={point}"),
        ),
        Ok(value) => Response::ok(tool).result(format_f64(value)).build(),
        Err(err) => map_expression_error(tool, &err),
    }
}

/// Definite integral over `[lower, upper]` by composite Simpson's rule.
#[must_use] 
pub fn definite_integral(expression: &str, variable: &str, lower: f64, upper: f64) -> String {
    let tool = TOOL_DEFINITE_INTEGRAL;
    match simpson(expression, variable, lower, upper) {
        Ok(value) => {
            if !value.is_finite() {
                // Improper integrals (singularity inside the interval or
                // unbounded integrand) used to leak `inf`/`NaN` to the caller.
                return error_with_detail(
                    tool,
                    ErrorCode::DomainError,
                    "integrand diverges on the given interval",
                    &format!("lower={lower}, upper={upper}"),
                );
            }
            Response::ok(tool).result(format_f64(value)).build()
        }
        Err(err) => map_expression_error(tool, &err),
    }
}

/// Compute the tangent line to `f(x)` at `point`.
///
/// Emits `SLOPE`, `INTERCEPT`, and `EQUATION` inline fields.
#[must_use]
pub fn tangent_line(expression: &str, variable: &str, point: f64) -> String {
    let tool = TOOL_TANGENT_LINE;
    if let Err(msg) = ensure_defined_at(tool, expression, variable, point) {
        return msg;
    }
    let f_at_point = match eval(expression, variable, point) {
        Ok(v) => v,
        Err(err) => return map_expression_error(tool, &err),
    };
    let slope = match central_difference(expression, variable, point, DERIVATIVE_STEP) {
        Ok(v) => v,
        Err(err) => return map_expression_error(tool, &err),
    };
    if !slope.is_finite() {
        return error_with_detail(
            tool,
            ErrorCode::DomainError,
            "tangent slope diverges at the evaluation point",
            &format!("{variable}={point}"),
        );
    }
    let y_intercept = slope.mul_add(-point, f_at_point);
    let slope_s = format_f64(slope);
    let intercept_s = format_f64(y_intercept);
    let equation = format!("y = {slope_s}*x + {intercept_s}");
    Response::ok(tool)
        .field("SLOPE", slope_s)
        .field("INTERCEPT", intercept_s)
        .field("EQUATION", equation)
        .build()
}

// --------------------------------------------------------------------------- //
//  Tests
// --------------------------------------------------------------------------- //

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_result(envelope: &str) -> f64 {
        // Parse the RESULT field out of `TOOL: OK | RESULT: <f64>` envelopes.
        let prefix = envelope.find("RESULT: ").expect("has RESULT field");
        let tail = &envelope[prefix + "RESULT: ".len()..];
        tail.split(" | ")
            .next()
            .expect("non-empty")
            .parse::<f64>()
            .unwrap_or_else(|_| panic!("not a float in {envelope}"))
    }

    // ---- derivative ----

    #[test]
    fn derivative_of_x_squared_at_3() {
        let out = derivative("x^2", "x", 3.0);
        assert!(out.starts_with("DERIVATIVE: OK | RESULT: "), "got {out}");
        assert!((extract_result(&out) - 6.0).abs() < 1e-6, "got {out}");
    }

    #[test]
    fn derivative_of_constant_is_zero() {
        assert_eq!(derivative("7", "x", 2.0), "DERIVATIVE: OK | RESULT: 0.0");
    }

    #[test]
    fn derivative_of_cubic() {
        let out = derivative("x^3", "x", 2.0);
        assert!(out.starts_with("DERIVATIVE: OK | RESULT: "));
        assert!((extract_result(&out) - 12.0).abs() < 1e-5, "got {out}");
    }

    #[test]
    fn derivative_invalid_expression_parse_error() {
        assert_eq!(
            derivative("1+", "x", 0.0),
            "DERIVATIVE: ERROR\nREASON: [PARSE_ERROR] unexpected end of expression"
        );
    }

    #[test]
    fn derivative_empty_expression() {
        assert_eq!(
            derivative("", "x", 0.0),
            "DERIVATIVE: ERROR\nREASON: [INVALID_INPUT] expression must not be blank"
        );
    }

    #[test]
    fn derivative_unknown_variable() {
        assert_eq!(
            derivative("y + 1", "x", 0.0),
            "DERIVATIVE: ERROR\nREASON: [UNKNOWN_VARIABLE] expression references an unknown variable\nDETAIL: name=y"
        );
    }

    #[test]
    fn derivative_unknown_function() {
        assert_eq!(
            derivative("bogus(x)", "x", 0.0),
            "DERIVATIVE: ERROR\nREASON: [UNKNOWN_FUNCTION] expression calls an unknown function\nDETAIL: name=bogus"
        );
    }

    // ---- nth_derivative ----

    #[test]
    fn nth_derivative_second_of_x_squared_is_two() {
        let out = nth_derivative("x^2", "x", 5.0, 2);
        assert!(out.starts_with("NTH_DERIVATIVE: OK | RESULT: "));
        assert!((extract_result(&out) - 2.0).abs() < 1e-3, "got {out}");
    }

    #[test]
    fn nth_derivative_first_matches_central_diff() {
        let first = derivative("x^3", "x", 2.0);
        let out = nth_derivative("x^3", "x", 2.0, 1);
        assert!(
            (extract_result(&first) - extract_result(&out)).abs() < 1e-2,
            "first={first}, out={out}"
        );
    }

    #[test]
    fn nth_derivative_order_below_range() {
        assert_eq!(
            nth_derivative("x^2", "x", 1.0, 0),
            "NTH_DERIVATIVE: ERROR\nREASON: [INVALID_INPUT] order must be between 1 and 10\nDETAIL: order=0"
        );
    }

    #[test]
    fn nth_derivative_order_above_range() {
        assert_eq!(
            nth_derivative("x^2", "x", 1.0, 11),
            "NTH_DERIVATIVE: ERROR\nREASON: [INVALID_INPUT] order must be between 1 and 10\nDETAIL: order=11"
        );
    }

    #[test]
    fn nth_derivative_negative_order() {
        assert_eq!(
            nth_derivative("x^2", "x", 1.0, -1),
            "NTH_DERIVATIVE: ERROR\nREASON: [INVALID_INPUT] order must be between 1 and 10\nDETAIL: order=-1"
        );
    }

    #[test]
    fn nth_derivative_bubbles_parse_error() {
        assert_eq!(
            nth_derivative("1+", "x", 0.0, 2),
            "NTH_DERIVATIVE: ERROR\nREASON: [PARSE_ERROR] unexpected end of expression"
        );
    }

    // ---- definite_integral ----

    #[test]
    fn integral_of_x_squared_0_to_1() {
        let out = definite_integral("x^2", "x", 0.0, 1.0);
        assert!(out.starts_with("DEFINITE_INTEGRAL: OK | RESULT: "));
        assert!((extract_result(&out) - (1.0 / 3.0)).abs() < 1e-6, "got {out}");
    }

    #[test]
    fn integral_of_constant() {
        assert_eq!(
            definite_integral("4", "x", 0.0, 5.0),
            "DEFINITE_INTEGRAL: OK | RESULT: 20.0"
        );
    }

    #[test]
    fn integral_of_x_from_minus1_to_1_is_zero() {
        let out = definite_integral("x", "x", -1.0, 1.0);
        assert!(out.starts_with("DEFINITE_INTEGRAL: OK | RESULT: "));
        assert!(extract_result(&out).abs() < 1e-6, "got {out}");
    }

    #[test]
    fn integral_invalid_expression() {
        assert_eq!(
            definite_integral("bogus(x)", "x", 0.0, 1.0),
            "DEFINITE_INTEGRAL: ERROR\nREASON: [UNKNOWN_FUNCTION] expression calls an unknown function\nDETAIL: name=bogus"
        );
    }

    #[test]
    fn integral_with_singularity_inside_interval_errors() {
        // Regression: `1/x` on [-1, 1] crosses x=0. Simpson's rule samples
        // x=0 exactly (10 000 even-numbered intervals), which triggers the
        // expression evaluator's division-by-zero guard. Previously returned
        // `RESULT: inf`.
        let out = definite_integral("1/x", "x", -1.0, 1.0);
        assert!(
            out.starts_with("DEFINITE_INTEGRAL: ERROR\nREASON: ["),
            "got {out}"
        );
    }

    // ---- tangent_line ----

    #[test]
    fn tangent_line_of_x_squared_at_3() {
        let out = tangent_line("x^2", "x", 3.0);
        // Central-difference slope ≈ 6.000000001282757, intercept ≈ -9.000000003848271.
        assert!(
            out.starts_with("TANGENT_LINE: OK | SLOPE: "),
            "got {out}"
        );
        assert!(out.contains(" | INTERCEPT: "), "got {out}");
        assert!(
            out.contains(" | EQUATION: y = "),
            "got {out}"
        );
    }

    #[test]
    fn tangent_line_of_linear_function() {
        let out = tangent_line("2*x + 5", "x", 7.0);
        assert!(out.starts_with("TANGENT_LINE: OK | SLOPE: "));
        // Walk the inline fields to pull the slope and intercept.
        let slope_marker = "SLOPE: ";
        let rest = &out[out.find(slope_marker).unwrap() + slope_marker.len()..];
        let slope: f64 = rest.split(" | ").next().unwrap().parse().unwrap();
        assert!((slope - 2.0).abs() < 1e-5, "slope={slope}");
        let intercept_marker = "INTERCEPT: ";
        let rest = &out[out.find(intercept_marker).unwrap() + intercept_marker.len()..];
        let intercept: f64 = rest.split(" | ").next().unwrap().parse().unwrap();
        assert!((intercept - 5.0).abs() < 1e-5, "intercept={intercept}");
    }

    #[test]
    fn tangent_line_invalid_expression() {
        assert_eq!(
            tangent_line("unknown_fn(x)", "x", 0.0),
            "TANGENT_LINE: ERROR\nREASON: [UNKNOWN_FUNCTION] expression calls an unknown function\nDETAIL: name=unknown_fn"
        );
    }

    #[test]
    fn derivative_rejects_singularity_at_origin() {
        // Regression: 1/x at x=0 used to return a huge spurious value
        // (≈ 1.25e12) because central differences only sample point±h.
        let out = derivative("1/x", "x", 0.0);
        assert!(out.contains("DERIVATIVE: ERROR"), "got {out}");
        assert!(out.contains("DOMAIN_ERROR"), "got {out}");
        assert!(
            out.contains("function is not defined at the evaluation point"),
            "got {out}"
        );
    }

    #[test]
    fn nth_derivative_rejects_singularity() {
        let out = nth_derivative("1/x", "x", 0.0, 2);
        assert!(out.contains("NTH_DERIVATIVE: ERROR"), "got {out}");
        assert!(out.contains("DOMAIN_ERROR"), "got {out}");
    }

    #[test]
    fn tangent_line_rejects_singularity() {
        let out = tangent_line("1/x", "x", 0.0);
        assert!(out.contains("TANGENT_LINE: ERROR"), "got {out}");
        assert!(out.contains("DOMAIN_ERROR"), "got {out}");
    }

    #[test]
    fn derivative_still_works_near_singularity() {
        // Smoke-check: 1/x at x=1 is -1, and the guard should not interfere.
        let out = derivative("1/x", "x", 1.0);
        assert!(out.starts_with("DERIVATIVE: OK"), "got {out}");
        let value = extract_result(&out);
        assert!((value - -1.0).abs() < 1e-5, "value={value}");
    }
}
