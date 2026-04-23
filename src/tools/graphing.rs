//!
//! Expression evaluation is delegated to [`crate::engine::expression`],

//! trig, IEEE-754 semantics).
//!
//! Every entry point emits the structured response envelope. Plot samples use
//! block layout (tabular); solve/find-roots return inline payloads.

use std::collections::HashMap;

use bigdecimal::{BigDecimal, FromPrimitive, ToPrimitive};

use crate::engine::bigdecimal_ext::DECIMAL128_PRECISION;
use crate::engine::expression::{ExpressionError, evaluate_with_variables};
use crate::mcp::message::{ErrorCode, Response, error_with_detail, expression_error_envelope};

const TOOL_PLOT_FUNCTION: &str = "PLOT_FUNCTION";
const TOOL_SOLVE_EQUATION: &str = "SOLVE_EQUATION";
const TOOL_FIND_ROOTS: &str = "FIND_ROOTS";

const MAX_NEWTON_ITERS: i32 = 1000;
const NEWTON_TOLERANCE: f64 = 1e-12;
const DERIVATIVE_STEP: f64 = 1e-8;
const BISECT_ITERS: i32 = 50;
const SCAN_DIVISIONS: i32 = 1000;

/// Map an [`ExpressionError`] into the canonical envelope — delegates to the
/// shared helper so REASON text and DETAIL shape stay consistent.
fn map_expression_error(tool: &str, err: &ExpressionError) -> String {
    expression_error_envelope(tool, err)
}

fn eval_at(tool: &str, expression: &str, variable: &str, x: f64) -> Result<f64, String> {
    let mut vars = HashMap::with_capacity(1);
    vars.insert(variable.to_string(), x);
    evaluate_with_variables(expression, &vars).map_err(|e| map_expression_error(tool, &e))
}

/// Outcome of a single-point evaluation used by plot/find-roots.
///
/// * `Value(y)` — finite, in-domain sample.
/// * `Undefined` — pole (`1/0`), domain excursion (`log(-1)`), or non-finite
///   result. Surfaces as `y = NaN` in plots and a continuity break in root
///   scans, instead of aborting the whole response.
enum SampleKind {
    Value(f64),
    Undefined,
}

/// Try to evaluate at `x`, classifying pointwise singularities as `Undefined`
/// so scanners can keep going. Structural errors (parse, unknown variable,
/// unknown function) are still fatal — they affect every sample, so surfacing
/// the first is the right behaviour.
fn try_sample(expression: &str, variable: &str, x: f64) -> Result<SampleKind, ExpressionError> {
    let mut vars = HashMap::with_capacity(1);
    vars.insert(variable.to_string(), x);
    match evaluate_with_variables(expression, &vars) {
        Ok(y) if y.is_finite() => Ok(SampleKind::Value(y)),
        Ok(_) => Ok(SampleKind::Undefined),
        Err(ExpressionError::DivisionByZero | ExpressionError::DomainError { .. }) => {
            Ok(SampleKind::Undefined)
        }
        Err(e) => Err(e),
    }
}

// --------------------------------------------------------------------------- //
//  plot_function
// --------------------------------------------------------------------------- //

fn plot_finite_decimal(tool: &str, label: &str, value: f64) -> Result<BigDecimal, String> {
    BigDecimal::from_f64(value).ok_or_else(|| {
        error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            &format!("{label} is not a finite decimal"),
            &format!("{label}={value}"),
        )
    })
}

fn sample_plot(
    tool: &str,
    expression: &str,
    variable: &str,
    min: f64,
    max: f64,
    steps: i32,
) -> Result<Vec<(f64, f64)>, String> {
    let bd_min = plot_finite_decimal(tool, "min", min)?;
    let bd_max = plot_finite_decimal(tool, "max", max)?;
    let step_size = (&bd_max - &bd_min).with_prec(DECIMAL128_PRECISION) / BigDecimal::from(steps);
    let capacity = usize::try_from(steps).unwrap_or(0).saturating_add(1);
    let mut rows: Vec<(f64, f64)> = Vec::with_capacity(capacity);
    for idx in 0..=steps {
        let x_bd = &bd_min + &step_size * BigDecimal::from(idx);
        let x = x_bd.to_f64().unwrap_or(f64::NAN);
        let y = match try_sample(expression, variable, x) {
            Ok(SampleKind::Value(v)) => v,
            // Pointwise singularity (e.g. `1/x` at x=0, `log(x)` at x<=0):
            // emit NaN instead of aborting the entire plot.
            Ok(SampleKind::Undefined) => f64::NAN,
            Err(e) => return Err(map_expression_error(tool, &e)),
        };
        rows.push((x, y));
    }
    Ok(rows)
}

/// Sample `expression` at `steps + 1` equally spaced points between `min` and `max`.
#[must_use]
pub fn plot_function(expression: &str, variable: &str, min: f64, max: f64, steps: i32) -> String {
    let tool = TOOL_PLOT_FUNCTION;
    if steps <= 0 {
        return error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "steps must be greater than 0",
            &format!("steps={steps}"),
        );
    }
    if min >= max {
        return error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "min must be less than max",
            &format!("min={min} | max={max}"),
        );
    }
    let rows = match sample_plot(tool, expression, variable, min, max, steps) {
        Ok(v) => v,
        Err(msg) => return msg,
    };
    let mut builder = Response::ok(tool)
        .field("STEPS", steps.to_string())
        .field("MIN", format!("{min:?}"))
        .field("MAX", format!("{max:?}"));
    for (idx, (x, y)) in rows.into_iter().enumerate() {
        let key = format!("ROW_{}", idx + 1);
        let value = format!("x={x:?} | y={y:?}");
        builder = builder.field(key, value);
    }
    builder.block().build()
}

// --------------------------------------------------------------------------- //
//  solve_equation (Newton-Raphson with central-difference derivative)
// --------------------------------------------------------------------------- //

/// Newton-Raphson solver. Returns the root inline or an error envelope.
#[must_use]
pub fn solve_equation(expression: &str, variable: &str, initial_guess: f64) -> String {
    let tool = TOOL_SOLVE_EQUATION;
    let mut guess = initial_guess;

    for _ in 0..MAX_NEWTON_ITERS {
        let f_value = match eval_at(tool, expression, variable, guess) {
            Ok(v) => v,
            Err(msg) => return msg,
        };
        if f_value.abs() < NEWTON_TOLERANCE {
            return Response::ok(tool).result(guess.to_string()).build();
        }

        let f_plus = match eval_at(tool, expression, variable, guess + DERIVATIVE_STEP) {
            Ok(v) => v,
            Err(msg) => return msg,
        };
        let f_minus = match eval_at(tool, expression, variable, guess - DERIVATIVE_STEP) {
            Ok(v) => v,
            Err(msg) => return msg,
        };
        let derivative = (f_plus - f_minus) / (2.0 * DERIVATIVE_STEP);
        if derivative == 0.0 {
            return error_with_detail(
                tool,
                ErrorCode::InvalidInput,
                "did not converge",
                "reason=derivative is zero",
            );
        }

        guess -= f_value / derivative;
    }

    error_with_detail(
        tool,
        ErrorCode::InvalidInput,
        "did not converge",
        &format!("iterations={MAX_NEWTON_ITERS}"),
    )
}

// --------------------------------------------------------------------------- //
//  find_roots (scan + bisect)
// --------------------------------------------------------------------------- //

/// Scan `[min, max]` in `SCAN_DIVISIONS` slices, detecting sign changes and
/// already-at-root samples. Refines bracketed intervals with 50 bisection steps.
#[must_use]
pub fn find_roots(expression: &str, variable: &str, min: f64, max: f64) -> String {
    let tool = TOOL_FIND_ROOTS;
    if min > max {
        return error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "min must be less than or equal to max",
            &format!("min={min} | max={max}"),
        );
    }
    let step = (max - min) / f64::from(SCAN_DIVISIONS);
    let mut roots: Vec<f64> = Vec::new();

    // `prev` is None whenever the previous sample was a singularity or out of
    // domain — a sign change across a pole is not a real root, so the scanner
    // only compares sign changes across two consecutive *defined* samples.
    let mut prev: Option<(f64, f64)> = match try_sample(expression, variable, min) {
        Ok(SampleKind::Value(y)) => {
            if y.abs() < NEWTON_TOLERANCE {
                roots.push(min);
            }
            Some((min, y))
        }
        Ok(SampleKind::Undefined) => None,
        Err(e) => return map_expression_error(tool, &e),
    };

    for idx in 1..=SCAN_DIVISIONS {
        let offset = f64::from(idx) * step;
        let current_x = min + offset;
        let current_f = match try_sample(expression, variable, current_x) {
            Ok(SampleKind::Value(v)) => v,
            Ok(SampleKind::Undefined) => {
                prev = None;
                continue;
            }
            Err(e) => return map_expression_error(tool, &e),
        };

        if current_f.abs() < NEWTON_TOLERANCE {
            push_unique(&mut roots, current_x);
        } else if let Some((prev_x, prev_f)) = prev
            && prev_f * current_f < 0.0
        {
            match bisect(tool, expression, variable, prev_x, current_x) {
                Ok(root) => push_unique(&mut roots, root),
                Err(msg) => return msg,
            }
        }

        prev = Some((current_x, current_f));
    }

    let count = roots.len();
    let values = roots
        .iter()
        .map(|r| format!("{r:?}"))
        .collect::<Vec<_>>()
        .join(",");

    Response::ok(tool)
        .field("COUNT", count.to_string())
        .field("VALUES", values)
        .build()
}

fn push_unique(roots: &mut Vec<f64>, candidate: f64) {
    if roots
        .iter()
        .any(|existing| (existing - candidate).abs() < 1e-6)
    {
        return;
    }
    roots.push(candidate);
}

fn bisect(
    tool: &str,
    expression: &str,
    variable: &str,
    lower_bound: f64,
    upper_bound: f64,
) -> Result<f64, String> {
    let mut lower = lower_bound;
    let mut upper = upper_bound;

    for _ in 0..BISECT_ITERS {
        let mid = f64::midpoint(lower, upper);
        let f_mid = eval_at(tool, expression, variable, mid)?;
        if f_mid.abs() < NEWTON_TOLERANCE {
            return Ok(mid);
        }
        let f_lo = eval_at(tool, expression, variable, lower)?;
        if f_lo * f_mid < 0.0 {
            upper = mid;
        } else {
            lower = mid;
        }
    }
    Ok(f64::midpoint(lower, upper))
}

// --------------------------------------------------------------------------- //
//  Tests
// --------------------------------------------------------------------------- //

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plot_x_squared_endpoints() {
        let out = plot_function("x^2", "x", -2.0, 2.0, 4);
        let expected = "PLOT_FUNCTION: OK\n\
STEPS: 4\n\
MIN: -2.0\n\
MAX: 2.0\n\
ROW_1: x=-2.0 | y=4.0\n\
ROW_2: x=-1.0 | y=1.0\n\
ROW_3: x=0.0 | y=0.0\n\
ROW_4: x=1.0 | y=1.0\n\
ROW_5: x=2.0 | y=4.0";
        assert_eq!(out, expected);
    }

    #[test]
    fn plot_invalid_steps_zero() {
        assert_eq!(
            plot_function("x", "x", 0.0, 1.0, 0),
            "PLOT_FUNCTION: ERROR\nREASON: [INVALID_INPUT] steps must be greater than 0\nDETAIL: steps=0"
        );
    }

    #[test]
    fn plot_invalid_steps_negative() {
        assert_eq!(
            plot_function("x", "x", 0.0, 1.0, -5),
            "PLOT_FUNCTION: ERROR\nREASON: [INVALID_INPUT] steps must be greater than 0\nDETAIL: steps=-5"
        );
    }

    #[test]
    fn plot_invalid_range_inverted() {
        assert_eq!(
            plot_function("x", "x", 5.0, 1.0, 10),
            "PLOT_FUNCTION: ERROR\nREASON: [INVALID_INPUT] min must be less than max\nDETAIL: min=5 | max=1"
        );
    }

    #[test]
    fn plot_invalid_range_equal() {
        assert_eq!(
            plot_function("x", "x", 1.0, 1.0, 10),
            "PLOT_FUNCTION: ERROR\nREASON: [INVALID_INPUT] min must be less than max\nDETAIL: min=1 | max=1"
        );
    }

    #[test]
    fn plot_samples_pole_emit_nan_not_abort() {
        // `1/x` has a pole at x=0 — the rest of the plot must still be drawn.
        let out = plot_function("1/x", "x", -1.0, 1.0, 4);
        assert!(out.starts_with("PLOT_FUNCTION: OK"), "got: {out}");
        assert!(out.contains("y=NaN"), "expected NaN at pole, got: {out}");
        // Endpoints should still be evaluated (1/-1 = -1, 1/1 = 1).
        assert!(out.contains("x=-1.0 | y=-1.0"));
        assert!(out.contains("x=1.0 | y=1.0"));
    }

    #[test]
    fn plot_out_of_domain_points_emit_nan_not_abort() {
        // `log(x)` is undefined for x<=0; the in-domain half of the plot
        // must survive.
        let out = plot_function("log(x)", "x", -1.0, 2.0, 3);
        assert!(out.starts_with("PLOT_FUNCTION: OK"), "got: {out}");
        assert!(out.contains("y=NaN"));
    }

    #[test]
    fn find_roots_ignores_pole_sign_change() {
        // `1/x` changes sign across x=0 but has no real root there — the
        // old scanner aborted with DIVISION_BY_ZERO. Now it reports 0 roots.
        let out = find_roots("1/x", "x", -2.0, 2.0);
        assert!(out.starts_with("FIND_ROOTS: OK"), "got: {out}");
        assert!(out.contains("COUNT: 0"));
    }

    #[test]
    fn plot_bubbles_unknown_variable() {
        assert_eq!(
            plot_function("unknown_var", "x", 0.0, 1.0, 2),
            "PLOT_FUNCTION: ERROR\nREASON: [UNKNOWN_VARIABLE] expression references an unknown variable\nDETAIL: name=unknown_var"
        );
    }

    #[test]
    fn solve_x_squared_minus_four_positive_guess() {
        assert_eq!(
            solve_equation("x^2 - 4", "x", 3.0),
            "SOLVE_EQUATION: OK | RESULT: 2"
        );
    }

    #[test]
    fn solve_x_squared_minus_four_negative_guess() {
        assert_eq!(
            solve_equation("x^2 - 4", "x", -3.0),
            "SOLVE_EQUATION: OK | RESULT: -2"
        );
    }

    #[test]
    fn solve_derivative_zero_error() {
        assert_eq!(
            solve_equation("5 - 5 + 0*x + 1", "x", 0.0),
            "SOLVE_EQUATION: ERROR\nREASON: [INVALID_INPUT] did not converge\nDETAIL: reason=derivative is zero"
        );
    }

    #[test]
    fn solve_bubbles_unknown_variable() {
        assert_eq!(
            solve_equation("bogus_var", "x", 1.0),
            "SOLVE_EQUATION: ERROR\nREASON: [UNKNOWN_VARIABLE] expression references an unknown variable\nDETAIL: name=bogus_var"
        );
    }

    #[test]
    fn find_roots_x_squared_minus_four() {
        assert_eq!(
            find_roots("x^2 - 4", "x", -5.0, 5.0),
            "FIND_ROOTS: OK | COUNT: 2 | VALUES: -2.0,2.0"
        );
    }

    #[test]
    fn find_roots_no_roots_returns_empty() {
        assert_eq!(
            find_roots("x^2 + 1", "x", -5.0, 5.0),
            "FIND_ROOTS: OK | COUNT: 0 | VALUES: "
        );
    }

    #[test]
    fn find_roots_cubic_three_roots() {
        assert_eq!(
            find_roots("x^3 - x", "x", -2.0, 2.0),
            "FIND_ROOTS: OK | COUNT: 3 | VALUES: -1.0,0.0,1.0"
        );
    }

    #[test]
    fn find_roots_bubbles_unknown_variable() {
        assert_eq!(
            find_roots("bogus_var", "x", -1.0, 1.0),
            "FIND_ROOTS: ERROR\nREASON: [UNKNOWN_VARIABLE] expression references an unknown variable\nDETAIL: name=bogus_var"
        );
    }

    #[test]
    fn find_roots_min_greater_than_max() {
        assert_eq!(
            find_roots("x", "x", 5.0, -5.0),
            "FIND_ROOTS: ERROR\nREASON: [INVALID_INPUT] min must be less than or equal to max\nDETAIL: min=5 | max=-5"
        );
    }
}
