//! Expression evaluation — f64 and arbitrary-precision variants, formatted
//! through the canonical envelope.

use std::collections::HashMap;

use serde_json::Value;

use crate::engine::expression::{
    evaluate as engine_evaluate, evaluate_with_variables as engine_evaluate_with_variables,
};
use crate::engine::expression_exact;
use crate::mcp::message::{ErrorCode, Response, error_with_detail, expression_error_envelope};

const TOOL_EVALUATE: &str = "EVALUATE";
const TOOL_EVALUATE_WITH_VARIABLES: &str = "EVALUATE_WITH_VARIABLES";
const TOOL_EVALUATE_EXACT: &str = "EVALUATE_EXACT";
const TOOL_EVALUATE_EXACT_WITH_VARIABLES: &str = "EVALUATE_EXACT_WITH_VARIABLES";

const JSON_DETAIL_MAX: usize = 120;

/// Names that already bind to engine constants and must not be shadowed by
/// caller-supplied variables. Accepting `{"pi": 3}` would silently wreck any
/// downstream trig call; rejecting it up front surfaces the naming conflict.
const RESERVED_VARIABLE_NAMES: &[&str] = &["pi", "e", "tau", "phi"];

fn reject_reserved_names(tool: &str, name: &str) -> Result<(), String> {
    if RESERVED_VARIABLE_NAMES.contains(&name) {
        Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "variable name is reserved for an engine constant",
            &format!("name={name}"),
        ))
    } else {
        Ok(())
    }
}

fn ok_result(tool: &str, value: String) -> String {
    Response::ok(tool).result(value).build()
}

fn truncate_for_detail(raw: &str) -> String {
    if raw.chars().count() <= JSON_DETAIL_MAX {
        raw.to_string()
    } else {
        let truncated: String = raw.chars().take(JSON_DETAIL_MAX).collect();
        truncated
    }
}

fn parse_variables_f64(tool: &str, json: &str) -> Result<HashMap<String, f64>, String> {
    // Accept both numbers and numeric strings so the f64 and exact evaluators
    // take the same variable shape. Strings like `"3.14"` parse through f64 —
    // anything else surfaces as PARSE_ERROR with the variable name.
    let raw: HashMap<String, Value> = serde_json::from_str(json).map_err(|e| {
        error_with_detail(
            tool,
            ErrorCode::ParseError,
            "variables JSON is malformed",
            &format!("json={}, cause={}", truncate_for_detail(json), e),
        )
    })?;
    let mut out = HashMap::with_capacity(raw.len());
    for (name, value) in raw {
        reject_reserved_names(tool, &name)?;
        let parsed = match value {
            Value::Number(n) => n.as_f64().ok_or_else(|| {
                error_with_detail(
                    tool,
                    ErrorCode::ParseError,
                    "variable value is not a finite number",
                    &format!("name={name}"),
                )
            })?,
            Value::String(s) => s.trim().parse::<f64>().map_err(|_| {
                error_with_detail(
                    tool,
                    ErrorCode::ParseError,
                    "variable value is not a valid number",
                    &format!("name={name}, value={s}"),
                )
            })?,
            _ => {
                return Err(error_with_detail(
                    tool,
                    ErrorCode::ParseError,
                    "variable value must be string or number",
                    &format!("name={name}"),
                ));
            }
        };
        out.insert(name, parsed);
    }
    Ok(out)
}

/// Parse variables JSON where each value must be a string or number. Returns
/// decimal-string values suitable for the arbitrary-precision evaluator.
fn parse_variables_string(tool: &str, json: &str) -> Result<HashMap<String, String>, String> {
    let raw: HashMap<String, Value> = match serde_json::from_str(json) {
        Ok(map) => map,
        Err(e) => {
            return Err(error_with_detail(
                tool,
                ErrorCode::ParseError,
                "variables JSON is malformed",
                &format!("json={}, cause={}", truncate_for_detail(json), e),
            ));
        }
    };
    let mut out = HashMap::with_capacity(raw.len());
    for (name, value) in raw {
        reject_reserved_names(tool, &name)?;
        match value {
            Value::String(s) => {
                out.insert(name, s);
            }
            Value::Number(n) => {
                out.insert(name, n.to_string());
            }
            _ => {
                return Err(error_with_detail(
                    tool,
                    ErrorCode::ParseError,
                    "variable value must be string or number",
                    &format!("name={name}"),
                ));
            }
        }
    }
    Ok(out)
}

/// Render an `f64` result. Whole-number values print as bare integers
/// (`"6"` not `"6.0"`) so the output shape matches `evaluateExact`, which
/// strips the redundant decimal for integer results. Non-integer values
/// fall back to Rust's `Debug` format, matching Java's
/// `String.valueOf(double)` / `Double.toString`.
fn format_double(value: f64) -> String {
    // Print integer-valued doubles without the trailing `.0` by using
    // `{:.0}` format — this avoids narrowing casts that `pedantic` rejects
    // (`as i128` truncation, `i128 as f64` precision loss) while keeping
    // exact integer appearance for values that fit in f64's 53-bit mantissa.
    // Outside the safe integer range the decimal form carries information
    // (magnitude, rounding) so fall back to `Debug`.
    const SAFE_INT_F64: f64 = 9_007_199_254_740_992.0; // 2^53
    if value.is_finite() && value.fract() == 0.0 && value.abs() < SAFE_INT_F64 {
        return format!("{value:.0}");
    }
    format!("{value:?}")
}

// --------------------------------------------------------------------------- //
//  f64 evaluator (preserves legacy numeric semantics)
// --------------------------------------------------------------------------- //

/// Evaluate a math expression without variables.
#[must_use]
pub fn evaluate(expression: &str) -> String {
    match engine_evaluate(expression) {
        Ok(value) => ok_result(TOOL_EVALUATE, format_double(value)),
        Err(err) => expression_error_envelope(TOOL_EVALUATE, &err),
    }
}

/// Evaluate a math expression with variable bindings supplied as a JSON object
/// (e.g. `{"x":3.0,"y":1.5}`).
#[must_use]
pub fn evaluate_with_variables(expression: &str, variables_json: &str) -> String {
    let variables = match parse_variables_f64(TOOL_EVALUATE_WITH_VARIABLES, variables_json) {
        Ok(map) => map,
        Err(e) => return e,
    };
    match engine_evaluate_with_variables(expression, &variables) {
        Ok(value) => ok_result(TOOL_EVALUATE_WITH_VARIABLES, format_double(value)),
        Err(err) => expression_error_envelope(TOOL_EVALUATE_WITH_VARIABLES, &err),
    }
}

// --------------------------------------------------------------------------- //
//  Arbitrary-precision evaluator
// --------------------------------------------------------------------------- //

/// Evaluate a math expression at arbitrary precision, returning a plain-decimal
/// string (so `0.1 + 0.2` yields `0.3`).
#[must_use]
pub fn evaluate_exact(expression: &str) -> String {
    match expression_exact::evaluate(expression) {
        Ok(value) => ok_result(TOOL_EVALUATE_EXACT, value),
        Err(err) => expression_error_envelope(TOOL_EVALUATE_EXACT, &err),
    }
}

/// Evaluate an expression at arbitrary precision with variable bindings. Each
/// JSON value must be a string (preferred — preserves every digit) or number.
#[must_use]
pub fn evaluate_exact_with_variables(expression: &str, variables_json: &str) -> String {
    let variables = match parse_variables_string(TOOL_EVALUATE_EXACT_WITH_VARIABLES, variables_json)
    {
        Ok(map) => map,
        Err(e) => return e,
    };
    match expression_exact::evaluate_with_variables(expression, &variables) {
        Ok(value) => ok_result(TOOL_EVALUATE_EXACT_WITH_VARIABLES, value),
        Err(err) => expression_error_envelope(TOOL_EVALUATE_EXACT_WITH_VARIABLES, &err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- evaluate ----

    #[test]
    fn evaluate_integer_arithmetic() {
        // Whole-number f64 results print as bare integers so the output shape
        // matches evaluateExact (which strips the trailing `.0`). Non-integer
        // f64 results keep their decimal form.
        assert_eq!(evaluate("2+3*4"), "EVALUATE: OK | RESULT: 14");
    }

    #[test]
    fn evaluate_trig_exact() {
        assert_eq!(evaluate("sin(90)"), "EVALUATE: OK | RESULT: 1");
    }

    #[test]
    fn evaluate_empty_is_error() {
        assert_eq!(
            evaluate(""),
            "EVALUATE: ERROR\nREASON: [INVALID_INPUT] expression must not be blank"
        );
    }

    #[test]
    fn evaluate_blank_is_error() {
        assert_eq!(
            evaluate("   \t\n"),
            "EVALUATE: ERROR\nREASON: [INVALID_INPUT] expression must not be blank"
        );
    }

    #[test]
    fn evaluate_unknown_variable_is_error() {
        assert_eq!(
            evaluate("foo + 1"),
            "EVALUATE: ERROR\nREASON: [UNKNOWN_VARIABLE] expression references an unknown variable\nDETAIL: name=foo"
        );
    }

    #[test]
    fn evaluate_unknown_function_is_error() {
        assert_eq!(
            evaluate("bogus(1)"),
            "EVALUATE: ERROR\nREASON: [UNKNOWN_FUNCTION] expression calls an unknown function\nDETAIL: name=bogus"
        );
    }

    #[test]
    fn evaluate_decimal_result_has_no_trailing_zero() {
        // Legacy f64 semantics — exact variant is available separately.
        assert_eq!(
            evaluate("0.1 + 0.2"),
            "EVALUATE: OK | RESULT: 0.30000000000000004"
        );
    }

    // ---- evaluate_with_variables ----

    #[test]
    fn eval_vars_simple() {
        assert_eq!(
            evaluate_with_variables("2*x + y", r#"{"x":3,"y":1}"#),
            "EVALUATE_WITH_VARIABLES: OK | RESULT: 7"
        );
    }

    #[test]
    fn eval_vars_power() {
        assert_eq!(
            evaluate_with_variables("x^2", r#"{"x":5}"#),
            "EVALUATE_WITH_VARIABLES: OK | RESULT: 25"
        );
    }

    #[test]
    fn eval_vars_empty_object_is_valid() {
        assert_eq!(
            evaluate_with_variables("1+2", "{}"),
            "EVALUATE_WITH_VARIABLES: OK | RESULT: 3"
        );
    }

    #[test]
    fn eval_vars_invalid_json_is_error() {
        let out = evaluate_with_variables("x+1", "not-json");
        assert!(
            out.starts_with(
                "EVALUATE_WITH_VARIABLES: ERROR\nREASON: [PARSE_ERROR] variables JSON is malformed\nDETAIL: json=not-json"
            ),
            "got: {out}"
        );
    }

    #[test]
    fn eval_vars_accepts_numeric_string() {
        // Shape parity with the exact evaluator: strings that parse as numbers
        // are accepted so callers can pass the same JSON to both tools.
        assert_eq!(
            evaluate_with_variables("x*2", r#"{"x":"5.5"}"#),
            "EVALUATE_WITH_VARIABLES: OK | RESULT: 11"
        );
    }

    #[test]
    fn eval_vars_rejects_non_numeric_string() {
        let out = evaluate_with_variables("x+1", r#"{"x":"hello"}"#);
        assert!(
            out.starts_with("EVALUATE_WITH_VARIABLES: ERROR\nREASON: [PARSE_ERROR]"),
            "got: {out}"
        );
        assert!(out.contains("value=hello"));
    }

    // ---- regression: base^frac for negative base is DomainError ----

    #[test]
    fn evaluate_negative_base_fractional_exp_is_domain_error() {
        let out = evaluate("(-1)^0.5");
        assert!(
            out.starts_with("EVALUATE: ERROR\nREASON: [DOMAIN_ERROR]"),
            "expected DOMAIN_ERROR, got: {out}"
        );
    }

    #[test]
    fn evaluate_factorial_over_cap_is_out_of_range() {
        let out = evaluate("factorial(25)");
        assert!(
            out.starts_with("EVALUATE: ERROR\nREASON: [OUT_OF_RANGE]"),
            "expected OUT_OF_RANGE, got: {out}"
        );
        assert!(out.contains("op=factorial"));
        assert!(out.contains("max=20"));
    }

    #[test]
    fn evaluate_exact_factorial_over_cap_is_out_of_range() {
        let out = evaluate_exact("factorial(2000)");
        assert!(
            out.starts_with("EVALUATE_EXACT: ERROR\nREASON: [OUT_OF_RANGE]"),
            "expected OUT_OF_RANGE, got: {out}"
        );
        assert!(out.contains("max=1000"));
    }

    #[test]
    fn evaluate_exact_large_exponent_fractional_base_converges_to_e() {
        // `(1 + 1/n)^n → e`. The naive repeated-multiplication path used to
        // return OVERFLOW because the decimal scale ballooned; the BigFloat
        // fallback converges. Don't pin the full string (platform float
        // formatting varies); just check the magnitude.
        let out = evaluate_exact("(1 + 1/1000000)^1000000");
        assert!(
            out.starts_with("EVALUATE_EXACT: OK"),
            "expected OK, got: {out}"
        );
        assert!(out.contains("2.71828"), "expected ~e, got: {out}");
    }

    #[test]
    fn evaluate_exact_power_small_integer_stays_exact() {
        // Regression: small integer exponents must still use the exact
        // BigDecimal path — any drift here would corrupt every integer power.
        assert_eq!(
            evaluate_exact("2^64"),
            "EVALUATE_EXACT: OK | RESULT: 18446744073709551616"
        );
    }

    #[test]
    fn eval_vars_unknown_variable_is_error() {
        assert_eq!(
            evaluate_with_variables("z + 1", r#"{"x":1}"#),
            "EVALUATE_WITH_VARIABLES: ERROR\nREASON: [UNKNOWN_VARIABLE] expression references an unknown variable\nDETAIL: name=z"
        );
    }

    #[test]
    fn eval_vars_empty_expression_is_error() {
        assert_eq!(
            evaluate_with_variables("", "{}"),
            "EVALUATE_WITH_VARIABLES: ERROR\nREASON: [INVALID_INPUT] expression must not be blank"
        );
    }

    #[test]
    fn eval_vars_float_values() {
        assert_eq!(
            evaluate_with_variables("x + y", r#"{"x":1.5,"y":2.5}"#),
            "EVALUATE_WITH_VARIABLES: OK | RESULT: 4"
        );
    }

    // ---- evaluate_exact ----

    #[test]
    fn evaluate_exact_avoids_binary_drift() {
        // f64 evaluator returns 0.30000000000000004; exact variant is clean.
        assert_eq!(
            evaluate_exact("0.1 + 0.2"),
            "EVALUATE_EXACT: OK | RESULT: 0.3"
        );
    }

    #[test]
    fn evaluate_exact_integer_arithmetic() {
        assert_eq!(evaluate_exact("2+3*4"), "EVALUATE_EXACT: OK | RESULT: 14");
        assert_eq!(evaluate_exact("2^10"), "EVALUATE_EXACT: OK | RESULT: 1024");
    }

    #[test]
    fn evaluate_exact_empty_is_error() {
        assert_eq!(
            evaluate_exact(""),
            "EVALUATE_EXACT: ERROR\nREASON: [INVALID_INPUT] expression must not be blank"
        );
    }

    #[test]
    fn evaluate_exact_unknown_variable_is_error() {
        assert_eq!(
            evaluate_exact("foo + 1"),
            "EVALUATE_EXACT: ERROR\nREASON: [UNKNOWN_VARIABLE] expression references an unknown variable\nDETAIL: name=foo"
        );
    }

    #[test]
    fn evaluate_exact_unknown_function_is_error() {
        assert_eq!(
            evaluate_exact("bogus(1)"),
            "EVALUATE_EXACT: ERROR\nREASON: [UNKNOWN_FUNCTION] expression calls an unknown function\nDETAIL: name=bogus"
        );
    }

    #[test]
    fn evaluate_exact_sqrt_of_negative_is_domain_error() {
        // Regression: previously returned `OK | RESULT: 0` because astro-float
        // NaN was silently mapped to zero.
        assert_eq!(
            evaluate_exact("sqrt(-2)"),
            "EVALUATE_EXACT: ERROR\nREASON: [DOMAIN_ERROR] operation is undefined for this input\nDETAIL: op=sqrt, value=-2"
        );
    }

    #[test]
    fn evaluate_exact_log_of_zero_is_domain_error() {
        assert_eq!(
            evaluate_exact("log(0)"),
            "EVALUATE_EXACT: ERROR\nREASON: [DOMAIN_ERROR] operation is undefined for this input\nDETAIL: op=log, value=0"
        );
    }

    #[test]
    fn evaluate_exact_log_of_negative_is_domain_error() {
        assert_eq!(
            evaluate_exact("log(-1)"),
            "EVALUATE_EXACT: ERROR\nREASON: [DOMAIN_ERROR] operation is undefined for this input\nDETAIL: op=log, value=-1"
        );
    }

    #[test]
    fn evaluate_exact_log10_of_zero_is_domain_error() {
        assert_eq!(
            evaluate_exact("log10(0)"),
            "EVALUATE_EXACT: ERROR\nREASON: [DOMAIN_ERROR] operation is undefined for this input\nDETAIL: op=log10, value=0"
        );
    }

    // ---- evaluate_exact_with_variables ----

    #[test]
    fn evaluate_exact_vars_string_value_round_trips() {
        // 25-digit variable that would be lossy through f64 — exact path keeps every digit.
        // Uses `my_pi` rather than `pi` because the constant name is reserved.
        let out = evaluate_exact_with_variables(
            "my_pi * 2",
            r#"{"my_pi":"3.1415926535897932384626433"}"#,
        );
        assert!(
            out.starts_with("EVALUATE_EXACT_WITH_VARIABLES: OK | RESULT: 6.2831853071795864769"),
            "got: {out}"
        );
    }

    #[test]
    fn evaluate_with_variables_rejects_reserved_pi() {
        let out = evaluate_with_variables("pi * 2", r#"{"pi": 100}"#);
        assert!(out.starts_with("EVALUATE_WITH_VARIABLES: ERROR"));
        assert!(out.contains("variable name is reserved"));
    }

    #[test]
    fn evaluate_exact_with_variables_rejects_reserved_e() {
        let out = evaluate_exact_with_variables("e + 1", r#"{"e": "5"}"#);
        assert!(out.starts_with("EVALUATE_EXACT_WITH_VARIABLES: ERROR"));
        assert!(out.contains("variable name is reserved"));
    }

    #[test]
    fn evaluate_exact_vars_number_value_works() {
        assert_eq!(
            evaluate_exact_with_variables("x + y", r#"{"x":3,"y":4}"#),
            "EVALUATE_EXACT_WITH_VARIABLES: OK | RESULT: 7"
        );
    }

    #[test]
    fn evaluate_exact_vars_empty_object_is_valid() {
        assert_eq!(
            evaluate_exact_with_variables("1+2", "{}"),
            "EVALUATE_EXACT_WITH_VARIABLES: OK | RESULT: 3"
        );
    }

    #[test]
    fn evaluate_exact_vars_invalid_json_is_error() {
        let out = evaluate_exact_with_variables("x+1", "not-json");
        assert!(
            out.starts_with(
                "EVALUATE_EXACT_WITH_VARIABLES: ERROR\nREASON: [PARSE_ERROR] variables JSON is malformed\nDETAIL: json=not-json"
            ),
            "got: {out}"
        );
    }

    #[test]
    fn evaluate_exact_vars_rejects_non_scalar_value() {
        assert_eq!(
            evaluate_exact_with_variables("x + 1", r#"{"x":[1,2,3]}"#),
            "EVALUATE_EXACT_WITH_VARIABLES: ERROR\nREASON: [PARSE_ERROR] variable value must be string or number\nDETAIL: name=x"
        );
    }

    #[test]
    fn evaluate_exact_vars_unknown_variable_is_error() {
        assert_eq!(
            evaluate_exact_with_variables("z + 1", r#"{"x":1}"#),
            "EVALUATE_EXACT_WITH_VARIABLES: ERROR\nREASON: [UNKNOWN_VARIABLE] expression references an unknown variable\nDETAIL: name=z"
        );
    }
}
