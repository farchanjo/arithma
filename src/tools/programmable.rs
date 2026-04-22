//! Expression evaluation — f64 and arbitrary-precision variants, formatted
//! through the canonical envelope.

use std::collections::HashMap;

use serde_json::Value;

use crate::engine::expression::{
    evaluate as engine_evaluate, evaluate_with_variables as engine_evaluate_with_variables,
};
use crate::engine::expression_exact;
use crate::mcp::message::{
    ErrorCode, Response, error_with_detail, expression_error_envelope,
};

const TOOL_EVALUATE: &str = "EVALUATE";
const TOOL_EVALUATE_WITH_VARIABLES: &str = "EVALUATE_WITH_VARIABLES";
const TOOL_EVALUATE_EXACT: &str = "EVALUATE_EXACT";
const TOOL_EVALUATE_EXACT_WITH_VARIABLES: &str = "EVALUATE_EXACT_WITH_VARIABLES";

const JSON_DETAIL_MAX: usize = 120;

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
    serde_json::from_str::<HashMap<String, f64>>(json).map_err(|e| {
        error_with_detail(
            tool,
            ErrorCode::ParseError,
            "variables JSON is malformed",
            &format!("json={}, cause={}", truncate_for_detail(json), e),
        )
    })
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

/// Render an `f64` matching Java's `String.valueOf(double)` / `Double.toString`.
fn format_double(value: f64) -> String {
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
        assert_eq!(evaluate("2+3*4"), "EVALUATE: OK | RESULT: 14.0");
    }

    #[test]
    fn evaluate_trig_exact() {
        assert_eq!(evaluate("sin(90)"), "EVALUATE: OK | RESULT: 1.0");
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
            "EVALUATE_WITH_VARIABLES: OK | RESULT: 7.0"
        );
    }

    #[test]
    fn eval_vars_power() {
        assert_eq!(
            evaluate_with_variables("x^2", r#"{"x":5}"#),
            "EVALUATE_WITH_VARIABLES: OK | RESULT: 25.0"
        );
    }

    #[test]
    fn eval_vars_empty_object_is_valid() {
        assert_eq!(
            evaluate_with_variables("1+2", "{}"),
            "EVALUATE_WITH_VARIABLES: OK | RESULT: 3.0"
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
            "EVALUATE_WITH_VARIABLES: OK | RESULT: 4.0"
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
        assert_eq!(
            evaluate_exact("2^10"),
            "EVALUATE_EXACT: OK | RESULT: 1024"
        );
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
        let out = evaluate_exact_with_variables(
            "pi * 2",
            r#"{"pi":"3.1415926535897932384626433"}"#,
        );
        assert!(
            out.starts_with("EVALUATE_EXACT_WITH_VARIABLES: OK | RESULT: 6.2831853071795864769"),
            "got: {out}"
        );
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
